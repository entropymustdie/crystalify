use crate::spotify::models::TrackMetadata;
use cpal::traits::{DeviceTrait, HostTrait};
use librespot::core::session::Session;
use librespot::core::spotify_uri::SpotifyUri;
use librespot::metadata::{Metadata, Track};
use librespot::playback::audio_backend::{self, Sink, SinkError, SinkResult};
use librespot::playback::config::{AudioFormat, PlayerConfig};
use librespot::playback::convert::Converter;
use librespot::playback::decoder::AudioPacket;
use librespot::playback::mixer::softmixer::SoftMixer;
use librespot::playback::mixer::{Mixer, MixerConfig};
use librespot::playback::player::{Player, PlayerEvent};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

const AUDIO_DEVICE_NOT_FOUND: &str = "Audio output device not found";

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

pub struct SpotifyPlayer {
    player: Option<Arc<Player>>,
    mixer: SoftMixer,
    pub events: UnboundedReceiver<PlayerEvent>,
    pub audio_errors: UnboundedReceiver<String>,
    pub backend_label: String,
    pub audio_error: Option<String>,
    _dummy_event_tx: Option<UnboundedSender<PlayerEvent>>,
    _audio_error_tx: UnboundedSender<String>,
}

impl SpotifyPlayer {
    pub fn new(session: Session) -> Result<Self, Box<dyn std::error::Error>> {
        crate::spotify::init_tls_provider();
        let mixer = SoftMixer::open(MixerConfig::default())?;
        let initial_volume = 45000;
        mixer.set_volume(initial_volume);
        let volume_getter = mixer.get_soft_volume();

        let (audio_error_tx, audio_errors) = mpsc::unbounded_channel();
        let (backend_label, mut audio_error) = detect_audio_output();

        let backend = if audio_error.is_none() {
            audio_backend::find(None)
        } else {
            None
        };

        if audio_error.is_none() && backend.is_none() {
            audio_error = Some(AUDIO_DEVICE_NOT_FOUND.to_string());
        }

        if let Some(backend_fn) = backend {
            let player_config = PlayerConfig {
                position_update_interval: Some(Duration::from_millis(250)),
                ..Default::default()
            };

            let err_tx = audio_error_tx.clone();
            let player = Player::new(
                player_config,
                session,
                volume_getter,
                move || {
                    match catch_unwind(AssertUnwindSafe(|| {
                        backend_fn(None, AudioFormat::F32)
                    })) {
                        Ok(sink) => sink,
                        Err(_) => {
                            let msg = AUDIO_DEVICE_NOT_FOUND.to_string();
                            let _ = err_tx.send(msg.clone());
                            Box::new(FailingSink { reason: msg })
                        }
                    }
                },
            );
            let events = player.get_player_event_channel();

            Ok(Self {
                player: Some(player),
                mixer,
                events,
                audio_errors,
                backend_label,
                audio_error,
                _dummy_event_tx: None,
                _audio_error_tx: audio_error_tx,
            })
        } else {
            let (dummy_event_tx, events) = mpsc::unbounded_channel();
            Ok(Self {
                player: None,
                mixer,
                events,
                audio_errors,
                backend_label,
                audio_error,
                _dummy_event_tx: Some(dummy_event_tx),
                _audio_error_tx: audio_error_tx,
            })
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

    pub fn load(&self, uri: SpotifyUri) {
        crate::spotify::logger::clear_last_error();
        if let Some(ref player) = self.player {
            player.load(uri, true, 0);
        }
    }

    pub fn play(&self) {
        if let Some(ref player) = self.player {
            player.play();
        }
    }

    pub fn pause(&self) {
        if let Some(ref player) = self.player {
            player.pause();
        }
    }

    pub fn stop(&self) {
        if let Some(ref player) = self.player {
            player.stop();
        }
    }

    pub fn seek(&self, position_ms: u32) {
        if let Some(ref player) = self.player {
            player.seek(position_ms);
        }
    }

    pub fn volume(&self) -> u16 {
        self.mixer.volume()
    }

    pub fn set_volume(&mut self, volume: u16) {
        self.mixer.set_volume(volume);
        if let Some(ref player) = self.player {
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
}
