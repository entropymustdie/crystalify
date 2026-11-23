use crate::spotify::models::TrackMetadata;
use cpal::traits::{DeviceTrait, HostTrait};
use librespot::audio::AudioFetchParams;
use librespot::connect::{
    ConnectConfig, LoadRequest, LoadRequestOptions, PlayingTrack, Spirc,
};
use librespot::core::authentication::Credentials;
use librespot::core::cache::Cache;
use librespot::core::config::DeviceType;
use librespot::core::session::Session;
use librespot::core::spotify_uri::SpotifyUri;
use librespot::metadata::{Metadata, Track};
use librespot::playback::audio_backend::{Sink, SinkError, SinkResult};
use librespot::playback::config::{Bitrate, PlayerConfig};
use librespot::playback::convert::Converter;
use librespot::playback::decoder::AudioPacket;
use librespot::playback::mixer::softmixer::SoftMixer;
use librespot::playback::mixer::{Mixer, MixerConfig};
use librespot::playback::player::{Player, PlayerEvent};
use librespot::playback::{NUM_CHANNELS, SAMPLE_RATE};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;

const AUDIO_DEVICE_NOT_FOUND: &str = "Audio output device not found";
const INITIAL_VOLUME: u16 = 45000;
const MAX_SINK_CHUNKS: usize = 8;
const CONNECT_WINDOW_SIZE: usize = 100;
const CONNECT_WINDOW_PRE_CONTEXT: usize = 20;

pub fn init_audio_fetch_params() {
    let params = AudioFetchParams {
        minimum_download_size: 64 * 1024,
        minimum_throughput: 8 * 1024,
        initial_ping_time_estimate: Duration::from_millis(150),
        maximum_assumed_ping_time: Duration::from_millis(1000),
        read_ahead_before_playback: Duration::from_millis(300),
        read_ahead_during_playback: Duration::from_secs(5),
        prefetch_threshold_factor: 4.0,
        download_timeout: Duration::from_secs(8),
    };
    let _ = AudioFetchParams::set(params);
}

pub fn build_connect_track_window(
    tracks: &[TrackMetadata],
    selected_idx: usize,
) -> (Vec<String>, usize) {
    if tracks.is_empty() {
        return (Vec::new(), 0);
    }
    let clamped = selected_idx.min(tracks.len() - 1);
    let mut start = clamped.saturating_sub(CONNECT_WINDOW_PRE_CONTEXT);
    if start + CONNECT_WINDOW_SIZE > tracks.len() {
        start = tracks.len().saturating_sub(CONNECT_WINDOW_SIZE);
    }
    let end = (start + CONNECT_WINDOW_SIZE).min(tracks.len());
    let uris = tracks[start..end].iter().map(|t| t.uri.clone()).collect();
    let local_idx = clamped.saturating_sub(start);
    (uris, local_idx)
}

struct LowLatencyRodioSink {
    rodio_sink: rodio::Sink,
    _stream: rodio::OutputStream,
    flush_flag: Arc<AtomicBool>,
}

impl LowLatencyRodioSink {
    fn open(flush_flag: Arc<AtomicBool>) -> Result<Self, String> {
        let host = cpal::default_host();
        let cpal_device = host
            .default_output_device()
            .ok_or_else(|| AUDIO_DEVICE_NOT_FOUND.to_string())?;

        let default_config = cpal_device
            .default_output_config()
            .map_err(|_| AUDIO_DEVICE_NOT_FOUND.to_string())?;
        let config = cpal_device
            .supported_output_configs()
            .ok()
            .and_then(|mut cfgs| {
                cfgs.find(|c| c.channels() == NUM_CHANNELS as cpal::ChannelCount)
                    .and_then(|c| {
                        c.try_with_sample_rate(cpal::SampleRate(SAMPLE_RATE))
                            .or_else(|| c.try_with_sample_rate(default_config.sample_rate()))
                    })
            })
            .unwrap_or(default_config);

        let mut stream = match rodio::OutputStreamBuilder::default()
            .with_device(cpal_device.clone())
            .with_config(&config.config())
            .with_sample_format(cpal::SampleFormat::F32)
            .open_stream()
        {
            Ok(exact) => exact,
            Err(_) => rodio::OutputStreamBuilder::from_device(cpal_device)
                .map_err(|e| e.to_string())?
                .open_stream_or_fallback()
                .map_err(|e| e.to_string())?,
        };

        stream.log_on_drop(false);
        let rodio_sink = rodio::Sink::connect_new(stream.mixer());
        Ok(Self {
            rodio_sink,
            _stream: stream,
            flush_flag,
        })
    }
}

impl Sink for LowLatencyRodioSink {
    fn start(&mut self) -> SinkResult<()> {
        self.rodio_sink.play();
        Ok(())
    }

    fn stop(&mut self) -> SinkResult<()> {
        self.flush_flag.store(false, Ordering::Release);
        if !self.rodio_sink.empty() {
            self.rodio_sink.clear();
        } else {
            self.rodio_sink.pause();
        }
        Ok(())
    }

    fn write(&mut self, packet: AudioPacket, converter: &mut Converter) -> SinkResult<()> {
        if self.flush_flag.swap(false, Ordering::AcqRel) {
            if !self.rodio_sink.empty() {
                self.rodio_sink.clear();
            }
            self.rodio_sink.play();
        }
        let samples = packet
            .samples()
            .map_err(|e| SinkError::OnWrite(e.to_string()))?;
        if samples.is_empty() {
            return Ok(());
        }
        let samples_f32: Vec<f32> = converter.f64_to_f32(samples);
        let source = rodio::buffer::SamplesBuffer::new(
            NUM_CHANNELS as cpal::ChannelCount,
            SAMPLE_RATE,
            samples_f32,
        );
        self.rodio_sink.append(source);

        while self.rodio_sink.len() > MAX_SINK_CHUNKS {
            if self.flush_flag.load(Ordering::Acquire) {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok(())
    }
}

struct AudioPipeline {
    player: Option<Arc<Player>>,
    mixer: Arc<SoftMixer>,
    flush_sink: Arc<AtomicBool>,
    events: UnboundedReceiver<PlayerEvent>,
    audio_errors: UnboundedReceiver<String>,
    backend_label: String,
    audio_error: Option<String>,
    dummy_event_tx: Option<UnboundedSender<PlayerEvent>>,
    audio_error_tx: UnboundedSender<String>,
}

fn init_audio_pipeline(session: Session) -> Result<AudioPipeline, Box<dyn std::error::Error>> {
    init_audio_fetch_params();
    let mixer = Arc::new(SoftMixer::open(MixerConfig::default())?);
    mixer.set_volume(INITIAL_VOLUME);
    let volume_getter = mixer.get_soft_volume();
    let flush_sink = Arc::new(AtomicBool::new(false));

    let (audio_error_tx, audio_errors) = mpsc::unbounded_channel();
    let (backend_label, audio_error) = detect_audio_output();

    if audio_error.is_none() {
        let player_config = PlayerConfig {
            bitrate: Bitrate::Bitrate320,
            gapless: true,
            position_update_interval: Some(Duration::from_millis(250)),
            ..Default::default()
        };

        let err_tx = audio_error_tx.clone();
        let flush_for_sink = flush_sink.clone();
        let player = Player::new(player_config, session, volume_getter, move || {
            let f_flag = flush_for_sink.clone();
            match catch_unwind(AssertUnwindSafe(|| LowLatencyRodioSink::open(f_flag))) {
                Ok(Ok(sink)) => Box::new(sink),
                Ok(Err(msg)) => {
                    let _ = err_tx.send(msg.clone());
                    Box::new(FailingSink { reason: msg })
                }
                Err(_) => {
                    let msg = AUDIO_DEVICE_NOT_FOUND.to_string();
                    let _ = err_tx.send(msg.clone());
                    Box::new(FailingSink { reason: msg })
                }
            }
        });
        let events = player.get_player_event_channel();

        Ok(AudioPipeline {
            player: Some(player),
            mixer,
            flush_sink,
            events,
            audio_errors,
            backend_label,
            audio_error,
            dummy_event_tx: None,
            audio_error_tx,
        })
    } else {
        let (dummy_event_tx, events) = mpsc::unbounded_channel();
        Ok(AudioPipeline {
            player: None,
            mixer,
            flush_sink,
            events,
            audio_errors,
            backend_label,
            audio_error,
            dummy_event_tx: Some(dummy_event_tx),
            audio_error_tx,
        })
    }
}

struct FailingSink {
    reason: String,
}

impl Sink for FailingSink {
    fn start(&mut self) -> SinkResult<()> {
        Err(SinkError::NotConnected(self.reason.clone()))
    }

    fn write(&mut self, _packet: AudioPacket, _converter: &mut Converter) -> SinkResult<()> {
        Err(SinkError::NotConnected(self.reason.clone()))
    }
}

pub fn detect_audio_output() -> (String, Option<String>) {
    let result = catch_unwind(AssertUnwindSafe(|| {
        let host = cpal::default_host();
        let host_name = host.id().name();
        match host.default_output_device() {
            Some(device) => {
                let dev_name = device
                    .name()
                    .unwrap_or_else(|_| "Default Device".to_string());
                (format!("rodio ({host_name}) — {dev_name}"), None)
            }
            None => (
                format!("rodio ({host_name}) — not found"),
                Some(AUDIO_DEVICE_NOT_FOUND.to_string()),
            ),
        }
    }));

    match result {
        Ok(info) => info,
        Err(_) => (
            "rodio — not found".to_string(),
            Some(AUDIO_DEVICE_NOT_FOUND.to_string()),
        ),
    }
}

fn should_send_spirc_activate(spirc_active: &AtomicBool) -> bool {
    !spirc_active.swap(true, Ordering::AcqRel)
}

pub struct SpotifyPlayer {
    player: Option<Arc<Player>>,
    mixer: Arc<SoftMixer>,
    flush_sink: Arc<AtomicBool>,
    spirc_active: Arc<AtomicBool>,
    spirc: Option<Spirc>,
    spirc_task: Option<JoinHandle<()>>,
    spirc_watcher_task: Option<JoinHandle<()>>,
    pub events: UnboundedReceiver<PlayerEvent>,
    pub audio_errors: UnboundedReceiver<String>,
    pub backend_label: String,
    pub audio_error: Option<String>,
    _dummy_event_tx: Option<UnboundedSender<PlayerEvent>>,
    _audio_error_tx: UnboundedSender<String>,
}

impl SpotifyPlayer {
    #[allow(dead_code)]
    pub fn new(session: Session) -> Result<Self, Box<dyn std::error::Error>> {
        crate::spotify::init_tls_provider();
        let pipeline = init_audio_pipeline(session)?;
        Ok(Self {
            player: pipeline.player,
            mixer: pipeline.mixer,
            flush_sink: pipeline.flush_sink,
            spirc_active: Arc::new(AtomicBool::new(false)),
            spirc: None,
            spirc_task: None,
            spirc_watcher_task: None,
            events: pipeline.events,
            audio_errors: pipeline.audio_errors,
            backend_label: pipeline.backend_label,
            audio_error: pipeline.audio_error,
            _dummy_event_tx: pipeline.dummy_event_tx,
            _audio_error_tx: pipeline.audio_error_tx,
        })
    }

    pub async fn connect_with_spirc(
        credentials: Credentials,
        cache: Option<Cache>,
        device_name: &str,
    ) -> Result<(Session, Self), Box<dyn std::error::Error>> {
        crate::spotify::init_tls_provider();
        let session = crate::spotify::auth::create_session(cache);
        let pipeline = init_audio_pipeline(session.clone())?;
        let spirc_active = Arc::new(AtomicBool::new(false));

        if let Some(ref player) = pipeline.player {
            let connect_config = ConnectConfig {
                name: device_name.to_string(),
                device_type: DeviceType::Computer,
                initial_volume: INITIAL_VOLUME,
                is_group: false,
                disable_volume: false,
                volume_steps: 64,
                emit_set_queue_events: true,
            };

            let (spirc, spirc_future) = Spirc::new(
                connect_config,
                session.clone(),
                credentials,
                player.clone(),
                pipeline.mixer.clone(),
            )
            .await?;

            let mut spirc_state_events = player.get_player_event_channel();
            let spirc_active_watcher = Arc::clone(&spirc_active);
            let spirc_watcher_handle = tokio::spawn(async move {
                while let Some(event) = spirc_state_events.recv().await {
                    match event {
                        PlayerEvent::SessionConnected { .. } => {
                            spirc_active_watcher.store(true, Ordering::Release);
                        }
                        PlayerEvent::SessionDisconnected { .. } => {
                            spirc_active_watcher.store(false, Ordering::Release);
                        }
                        _ => {}
                    }
                }
            });

            let spirc_handle = tokio::spawn(spirc_future);
            crate::spotify::auth::wait_for_session_country(&session).await;

            Ok((
                session,
                Self {
                    player: pipeline.player,
                    mixer: pipeline.mixer,
                    flush_sink: pipeline.flush_sink,
                    spirc_active,
                    spirc: Some(spirc),
                    spirc_task: Some(spirc_handle),
                    spirc_watcher_task: Some(spirc_watcher_handle),
                    events: pipeline.events,
                    audio_errors: pipeline.audio_errors,
                    backend_label: pipeline.backend_label,
                    audio_error: pipeline.audio_error,
                    _dummy_event_tx: pipeline.dummy_event_tx,
                    _audio_error_tx: pipeline.audio_error_tx,
                },
            ))
        } else {
            session.connect(credentials, true).await?;
            crate::spotify::auth::wait_for_session_country(&session).await;
            Ok((
                session,
                Self {
                    player: None,
                    mixer: pipeline.mixer,
                    flush_sink: pipeline.flush_sink,
                    spirc_active: Arc::new(AtomicBool::new(false)),
                    spirc: None,
                    spirc_task: None,
                    spirc_watcher_task: None,
                    events: pipeline.events,
                    audio_errors: pipeline.audio_errors,
                    backend_label: pipeline.backend_label,
                    audio_error: pipeline.audio_error,
                    _dummy_event_tx: pipeline.dummy_event_tx,
                    _audio_error_tx: pipeline.audio_error_tx,
                },
            ))
        }
    }

    pub async fn fetch_metadata(
        session: &Session,
        uri: &SpotifyUri,
    ) -> Result<TrackMetadata, Box<dyn std::error::Error>> {
        let track = Track::get(session, uri).await?;
        let artist = track
            .artists
            .iter()
            .map(|a| a.name.clone())
            .collect::<Vec<_>>()
            .join(", ");

        let cover_url = crate::spotify::client::select_cover_url_from_images(&track.album.covers)
            .or_else(|| crate::spotify::client::select_cover_url_from_images(&track.album.cover_group));

        Ok(TrackMetadata {
            uri: uri.to_uri(),
            title: track.name,
            artist,
            album: track.album.name,
            duration_ms: track.duration as u32,
            cover_url,
        })
    }

    pub fn is_connect_active(&self) -> bool {
        self.spirc.is_some()
    }

    fn request_sink_flush(&self) {
        self.flush_sink.store(true, Ordering::Release);
    }

    fn ensure_spirc_active(&self) {
        if let Some(ref spirc) = self.spirc
            && should_send_spirc_activate(&self.spirc_active)
        {
            let _ = spirc.activate();
        }
    }

    pub fn preload(&self, uri: SpotifyUri) {
        if let Some(ref player) = self.player {
            player.preload(uri);
        }
    }

    pub fn load(&self, uri: SpotifyUri) {
        crate::spotify::logger::clear_last_error();
        self.request_sink_flush();
        if let Some(ref player) = self.player {
            player.load(uri, true, 0);
        }
    }

    pub fn load_context(
        &self,
        context_uri: String,
        start_track_uri: Option<String>,
        start_playing: bool,
    ) {
        crate::spotify::logger::clear_last_error();
        self.request_sink_flush();
        if let Some(ref track_uri) = start_track_uri
            && let Ok(parsed_uri) = SpotifyUri::from_uri(track_uri)
        {
            self.preload(parsed_uri);
        }
        if let Some(ref spirc) = self.spirc {
            self.ensure_spirc_active();
            let options = LoadRequestOptions {
                start_playing,
                seek_to: 0,
                context_options: None,
                playing_track: start_track_uri.map(PlayingTrack::Uri),
            };
            let _ = spirc.load(LoadRequest::from_context_uri(context_uri, options));
        } else if let Some(track_uri) = start_track_uri
            && let Ok(uri) = SpotifyUri::from_uri(&track_uri)
        {
            self.load(uri);
        }
    }

    pub fn load_tracks(&self, track_uris: Vec<String>, start_index: usize) {
        crate::spotify::logger::clear_last_error();
        if track_uris.is_empty() {
            return;
        }
        self.request_sink_flush();
        let clamped_idx = start_index.min(track_uris.len() - 1);
        if let Some(target_str) = track_uris.get(clamped_idx)
            && let Ok(target_uri) = SpotifyUri::from_uri(target_str)
        {
            self.preload(target_uri);
        }
        if let Some(ref spirc) = self.spirc {
            self.ensure_spirc_active();
            let options = LoadRequestOptions {
                start_playing: true,
                seek_to: 0,
                context_options: None,
                playing_track: Some(PlayingTrack::Index(clamped_idx as u32)),
            };
            let _ = spirc.load(LoadRequest::from_tracks(track_uris, options));
        } else if let Some(track_uri) = track_uris.get(clamped_idx)
            && let Ok(uri) = SpotifyUri::from_uri(track_uri)
        {
            self.load(uri);
        }
    }

    pub fn play(&self) {
        if let Some(ref spirc) = self.spirc {
            let _ = spirc.play();
        } else if let Some(ref player) = self.player {
            player.play();
        }
    }

    pub fn pause(&self) {
        if let Some(ref spirc) = self.spirc {
            let _ = spirc.pause();
        } else if let Some(ref player) = self.player {
            player.pause();
        }
    }

    pub fn next(&self) {
        self.request_sink_flush();
        if let Some(ref spirc) = self.spirc {
            let _ = spirc.next();
        }
    }

    pub fn prev(&self) {
        self.request_sink_flush();
        if let Some(ref spirc) = self.spirc {
            let _ = spirc.prev();
        }
    }

    pub fn stop(&self) {
        self.request_sink_flush();
        if let Some(ref player) = self.player {
            player.stop();
        }
    }

    pub async fn shutdown(&mut self) {
        if let Some(ref spirc) = self.spirc {
            let _ = spirc.shutdown();
        }
        if let Some(handle) = self.spirc_task.take() {
            let _ = tokio::time::timeout(Duration::from_millis(800), handle).await;
        }
        self.stop();
        if let Some(handle) = self.spirc_watcher_task.take() {
            handle.abort();
        }
    }

    pub fn seek(&self, position_ms: u32) {
        self.request_sink_flush();
        if let Some(ref spirc) = self.spirc {
            let _ = spirc.set_position_ms(position_ms);
        } else if let Some(ref player) = self.player {
            player.seek(position_ms);
        }
    }

    pub fn volume(&self) -> u16 {
        self.mixer.volume()
    }

    pub fn set_volume(&mut self, volume: u16) {
        self.mixer.set_volume(volume);
        if let Some(ref spirc) = self.spirc {
            let _ = spirc.set_volume(volume);
        } else if let Some(ref player) = self.player {
            player.emit_volume_changed_event(volume);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_audio_output_does_not_panic() {
        let (label, err) = detect_audio_output();
        assert!(label.starts_with("rodio"));
        if let Some(msg) = err {
            assert!(!msg.is_empty());
        }
    }

    #[test]
    fn test_init_audio_fetch_params() {
        init_audio_fetch_params();
        let params = AudioFetchParams::get();
        assert_eq!(params.read_ahead_before_playback, Duration::from_millis(300));
        assert_eq!(params.initial_ping_time_estimate, Duration::from_millis(150));
    }

    #[test]
    fn test_should_send_spirc_activate_transitions() {
        let active = AtomicBool::new(false);
        assert!(should_send_spirc_activate(&active));
        assert!(active.load(Ordering::Acquire));
        assert!(!should_send_spirc_activate(&active));
        assert!(!should_send_spirc_activate(&active));

        active.store(false, Ordering::Release);
        assert!(should_send_spirc_activate(&active));
        assert!(!should_send_spirc_activate(&active));
    }

    #[test]
    fn test_build_connect_track_window() {
        assert_eq!(build_connect_track_window(&[], 0), (Vec::new(), 0));

        let tracks: Vec<TrackMetadata> = (0..890)
            .map(|i| TrackMetadata {
                uri: format!("spotify:track:t{i}"),
                title: format!("Track {i}"),
                artist: "Artist".to_string(),
                album: "Album".to_string(),
                duration_ms: 180_000,
                cover_url: None,
            })
            .collect();

        let (uris_start, idx_start) = build_connect_track_window(&tracks, 0);
        assert_eq!(uris_start.len(), 100);
        assert_eq!(idx_start, 0);
        assert_eq!(uris_start[idx_start], "spotify:track:t0");

        let (uris_mid, idx_mid) = build_connect_track_window(&tracks, 550);
        assert_eq!(uris_mid.len(), 100);
        assert_eq!(idx_mid, 20);
        assert_eq!(uris_mid[idx_mid], "spotify:track:t550");

        let (uris_end, idx_end) = build_connect_track_window(&tracks, 889);
        assert_eq!(uris_end.len(), 100);
        assert_eq!(uris_end[idx_end], "spotify:track:t889");
        assert_eq!(idx_end, 99);

        let (uris_over, idx_over) = build_connect_track_window(&tracks, 999);
        assert_eq!(uris_over.len(), 100);
        assert_eq!(uris_over[idx_over], "spotify:track:t889");
        assert_eq!(idx_over, 99);
    }

    #[tokio::test]
    #[ignore]
    async fn live_integration_player_load_and_audio_check_test() {
        use crate::app::{AppState, PlaybackStatus};
        use crate::spotify::auth::init_cache;
        use crate::spotify::client::SpotifyClient;
        use librespot::core::config::SessionConfig;

        crate::spotify::logger::init();
        let cache = init_cache().expect("cache");
        let creds = cache.credentials().expect("cached creds");
        let session_config = SessionConfig::default();
        let _ = std::fs::create_dir_all(&session_config.tmp_dir);
        let session = Session::new(session_config, Some(cache));
        session.connect(creds, true).await.expect("connect");

        for _ in 0..40 {
            if !session.country().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        println!("Session country: '{}'", session.country());

        let client = SpotifyClient::new(session.clone());
        let results = client.search("Tool").await.expect("live search should succeed");
        let track = results.tracks.first().expect("at least one track").clone();

        let mut player = SpotifyPlayer::new(session).expect("SpotifyPlayer::new must return Ok");
        let mut state = AppState::new("tester".to_string(), "crystalify".to_string(), player.volume());
        state.set_audio_info(player.backend_label.clone(), player.audio_error.clone());
        state.set_track(track.clone());

        if let Some(ref err) = player.audio_error {
            println!("No audio device in env: label='{}', error='{}'", player.backend_label, err);
            assert_eq!(state.playback_status, PlaybackStatus::Failed(err.clone()));
            assert!(!state.is_playing);
            return;
        }

        let uri = SpotifyUri::from_uri(&track.uri).expect("valid track uri");
        player.load(uri);

        let deadline = tokio::time::sleep(Duration::from_secs(15));
        tokio::pin!(deadline);

        loop {
            tokio::select! {
                _ = &mut deadline => {
                    panic!("Timed out waiting for Playing or audio error; status={:?}", state.playback_status);
                }
                Some(ev) = player.events.recv() => {
                    state.handle_player_event(ev);
                    if matches!(state.playback_status, PlaybackStatus::Playing) {
                        println!("Live Playing event received: label='{}'", player.backend_label);
                        assert!(state.is_playing);
                        player.stop();
                        return;
                    }
                    if let PlaybackStatus::Failed(ref err) = state.playback_status {
                        player.stop();
                        panic!("Playback failed unexpectedly with audio device '{}': {}", player.backend_label, err);
                    }
                }
                Some(err) = player.audio_errors.recv() => {
                    state.set_audio_info(player.backend_label.clone(), Some(err.clone()));
                    println!("Audio error channel delivered cleanly without panic: {}", err);
                    assert_eq!(state.playback_status, PlaybackStatus::Failed(err));
                    assert!(!state.is_playing);
                    player.stop();
                    return;
                }
            }
        }
    }

    #[tokio::test]
    #[ignore]
    async fn live_integration_spirc_connect_and_playback_test() {
        use crate::app::{AppState, PlaybackStatus};
        use crate::spotify::auth::init_cache;

        crate::spotify::logger::init();
        let cache = init_cache().expect("cache");
        let creds = cache.credentials().expect("cached creds");

        let (session, mut player) =
            SpotifyPlayer::connect_with_spirc(creds, Some(cache), "crystalify-test")
                .await
                .expect("connect_with_spirc must succeed");

        let mut state = AppState::new(
            session.username(),
            "crystalify-test".to_string(),
            player.volume(),
        );
        state.set_audio_info(player.backend_label.clone(), player.audio_error.clone());
        state.connect_ready = player.is_connect_active();

        if let Some(ref err) = player.audio_error {
            println!("No audio device in env: label='{}', error='{}'", player.backend_label, err);
            assert_eq!(state.playback_status, PlaybackStatus::Failed(err.clone()));
            player.shutdown().await;
            return;
        }

        assert!(player.is_connect_active(), "Spirc should be active");
        let track_uri = "spotify:track:55mJleti2WfWEFNFcBduhc".to_string();
        player.load_tracks(vec![track_uri], 0);

        let deadline = tokio::time::sleep(Duration::from_secs(15));
        tokio::pin!(deadline);

        let mut saw_track_changed = false;
        let mut saw_playing = false;
        let mut saw_session_connected = false;

        loop {
            tokio::select! {
                _ = &mut deadline => {
                    player.shutdown().await;
                    panic!(
                        "Timed out in Spirc live test: session_connected={saw_session_connected}, track_changed={saw_track_changed}, playing={saw_playing}, status={:?}",
                        state.playback_status
                    );
                }
                Some(ev) = player.events.recv() => {
                    match &ev {
                        PlayerEvent::SessionConnected { .. } => {
                            saw_session_connected = true;
                        }
                        PlayerEvent::TrackChanged { .. } => {
                            saw_track_changed = true;
                        }
                        PlayerEvent::Playing { .. } => {
                            saw_playing = true;
                        }
                        _ => {}
                    }
                    state.handle_player_event(ev);
                    if let PlaybackStatus::Failed(ref err) = state.playback_status {
                        player.shutdown().await;
                        panic!("Spirc playback failed: {err}");
                    }
                    if saw_track_changed && saw_playing {
                        println!(
                            "Live Spirc OK: connected={}, track='{}' — '{}', pending_media_fetch={}",
                            saw_session_connected,
                            state.track_title,
                            state.track_artist,
                            state.pending_media_fetch
                        );
                        assert!(!state.track_title.is_empty());
                        assert_ne!(state.track_title, "No Track Playing");
                        assert!(state.is_playing);
                        assert!(state.pending_media_fetch);
                        player.shutdown().await;
                        return;
                    }
                }
                Some(err) = player.audio_errors.recv() => {
                    player.shutdown().await;
                    panic!("Spirc audio error: {err}");
                }
            }
        }
    }

    #[tokio::test]
    #[ignore]
    async fn live_integration_playback_latency_test() {
        use crate::spotify::auth::init_cache;
        use std::time::Instant;

        crate::spotify::logger::init();
        init_audio_fetch_params();
        let cache = init_cache().expect("cache");
        let creds = cache.credentials().expect("cached creds");

        let (_session, mut player) =
            SpotifyPlayer::connect_with_spirc(creds, Some(cache), "crystalify-latency-test")
                .await
                .expect("connect_with_spirc must succeed");

        if let Some(ref err) = player.audio_error {
            println!("No audio device in env: {err}");
            player.shutdown().await;
            return;
        }

        async fn wait_for_playing(
            player: &mut SpotifyPlayer,
            expected_uri: &str,
            started: Instant,
        ) -> (Option<u128>, u128) {
            let deadline = tokio::time::sleep(Duration::from_secs(15));
            tokio::pin!(deadline);
            let mut loading_ms: Option<u128> = None;
            loop {
                tokio::select! {
                    _ = &mut deadline => {
                        player.shutdown().await;
                        panic!("Timed out waiting for Playing on {expected_uri}");
                    }
                    Some(ev) = player.events.recv() => {
                        let elapsed = started.elapsed().as_millis();
                        match ev {
                            PlayerEvent::SessionConnected { .. } => {
                                println!("  [+{elapsed}ms] SessionConnected");
                            }
                            PlayerEvent::Loading { track_id, .. }
                                if track_id.to_uri() == expected_uri =>
                            {
                                if loading_ms.is_none() {
                                    loading_ms = Some(elapsed);
                                }
                                println!("  [+{elapsed}ms] Loading {expected_uri}");
                            }
                            PlayerEvent::TrackChanged { audio_item } => {
                                println!(
                                    "  [+{elapsed}ms] TrackChanged {}",
                                    audio_item.track_id.to_uri()
                                );
                            }
                            PlayerEvent::Playing { track_id, .. }
                                if track_id.to_uri() == expected_uri =>
                            {
                                println!("  [+{elapsed}ms] Playing {expected_uri}");
                                return (loading_ms, elapsed);
                            }
                            PlayerEvent::Unavailable { track_id, .. }
                                if track_id.to_uri() == expected_uri =>
                            {
                                player.shutdown().await;
                                panic!("Track unavailable: {expected_uri}");
                            }
                            _ => {}
                        }
                    }
                    Some(err) = player.audio_errors.recv() => {
                        player.shutdown().await;
                        panic!("Audio error: {err}");
                    }
                }
            }
        }

        let track1 = "spotify:track:55mJleti2WfWEFNFcBduhc".to_string();
        let track2 = "spotify:track:7tvuLLroI0n6uYBWuFig5d".to_string();
        let queue = vec![track1.clone(), track2.clone(), track1.clone()];

        while player.events.try_recv().is_ok() {}
        let t1 = Instant::now();
        player.load_tracks(queue.clone(), 0);
        let (cold_loading_ms, cold_ms) = wait_for_playing(&mut player, &track1, t1).await;
        println!(
            "(1) Cold/initial start: loading={cold_loading_ms:?}ms, Playing={cold_ms}ms"
        );

        let track2_uri = SpotifyUri::from_uri(&track2).expect("valid track2 uri");
        let preload_started = Instant::now();
        player.preload(track2_uri);
        let preload_deadline = tokio::time::sleep(Duration::from_secs(10));
        tokio::pin!(preload_deadline);
        loop {
            tokio::select! {
                _ = &mut preload_deadline => {
                    player.shutdown().await;
                    panic!("Timed out waiting for Preloading on {track2}");
                }
                Some(ev) = player.events.recv() => {
                    if let PlayerEvent::Preloading { track_id } = ev
                        && track_id.to_uri() == track2
                    {
                        let preload_ready_ms = preload_started.elapsed().as_millis();
                        println!("(2a) Preload ready for track2 in {preload_ready_ms}ms");
                        break;
                    }
                }
                Some(err) = player.audio_errors.recv() => {
                    player.shutdown().await;
                    panic!("Audio error during preload: {err}");
                }
            }
        }
        let settle = Duration::from_millis(1500);
        if let Some(remaining) = settle.checked_sub(preload_started.elapsed()) {
            tokio::time::sleep(remaining).await;
        }
        while player.events.try_recv().is_ok() {}

        let t2 = Instant::now();
        player.load_tracks(queue.clone(), 1);
        let (preloaded_loading_ms, preloaded_ms) =
            wait_for_playing(&mut player, &track2, t2).await;
        println!(
            "(2b) Preloaded start load_tracks -> loading={preloaded_loading_ms:?}ms, Playing={preloaded_ms}ms"
        );

        tokio::time::sleep(Duration::from_millis(1500)).await;
        while player.events.try_recv().is_ok() {}
        let t3 = Instant::now();
        player.load_tracks(queue, 0);
        let (warm_loading_ms, warm_ms) = wait_for_playing(&mut player, &track1, t3).await;
        println!(
            "(3) Warm replay start load_tracks -> loading={warm_loading_ms:?}ms, Playing={warm_ms}ms"
        );

        player.shutdown().await;

        assert!(
            preloaded_ms < 250,
            "Expected preloaded start < 250ms, got {preloaded_ms}ms"
        );
    }
}
