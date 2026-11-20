mod app;
mod spotify;
mod ui;

use app::{AppState, InputMode, PlaybackStatus};
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use image::DynamicImage;
use librespot::core::spotify_uri::SpotifyUri;
use ratatui::{backend::CrosstermBackend, Terminal};
use ratatui_image::picker::Picker;
use spotify::auth::authenticate;
use spotify::client::SpotifyClient;
use spotify::models::{CollectionKind, SearchCategory, SearchResults, TrackMetadata};
use spotify::player::SpotifyPlayer;
use std::env;
use std::io::stdout;
use std::time::Duration;
use tokio::sync::mpsc::{self, UnboundedSender};
use ui::draw_ui;

enum AsyncResult {
    SearchCompleted(Result<SearchResults, String>),
    CollectionTracksLoaded {
        kind: CollectionKind,
        title: String,
        subtitle: String,
        tracks: Result<Vec<TrackMetadata>, String>,
        auto_play_all: bool,
    },
    CoverLoaded {
        url: String,
        image: DynamicImage,
    },
}

fn spawn_cover_fetch_if_needed(
    state: &AppState,
    client: &SpotifyClient,
    tx: &UnboundedSender<AsyncResult>,
) {
    let Some(ref url) = state.track_cover_url else {
        return;
    };
    if url.is_empty() || state.cover_cache.contains_key(url) {
        return;
    }
    let url_clone = url.clone();
    let client_clone = client.clone();
    let tx_clone = tx.clone();
    tokio::spawn(async move {
        if let Ok(bytes) = client_clone.fetch_image_bytes(&url_clone).await
            && let Ok(decoded) = image::load_from_memory(&bytes)
        {
            let _ = tx_clone.send(AsyncResult::CoverLoaded {
                url: url_clone,
                image: decoded,
            });
        }
    });
}

fn load_track_with_status(
    player: &SpotifyPlayer,
    state: &mut AppState,
    client: &SpotifyClient,
    tx: &UnboundedSender<AsyncResult>,
    track: &TrackMetadata,
    status_message: String,
) {
    if state.audio_error.is_none() {
        state.status_message = Some(status_message);
    }
    load_track_into_player(player, state, client, tx, track);
}

fn load_track_into_player(
    player: &SpotifyPlayer,
    state: &mut AppState,
    client: &SpotifyClient,
    tx: &UnboundedSender<AsyncResult>,
    track: &TrackMetadata,
) {
    if state.audio_error.is_some() {
        spawn_cover_fetch_if_needed(state, client, tx);
        return;
    }
    if let Ok(uri) = SpotifyUri::from_uri(&track.uri) {
        player.load(uri);
        spawn_cover_fetch_if_needed(state, client, tx);
    } else {
        state.set_playback_failed(format!("Invalid track URI: {}", track.uri));
    }
}

fn trigger_collection_load(
    state: &mut AppState,
    client: &SpotifyClient,
    tx: &UnboundedSender<AsyncResult>,
    auto_play_all: bool,
) {
    let target = match state.search_category {
        SearchCategory::Albums => state
            .search_results
            .albums
            .get(state.selected_index)
            .map(|a| {
                (
                    CollectionKind::Album,
                    a.uri.clone(),
                    a.name.clone(),
                    a.artist.clone(),
                )
            }),
        SearchCategory::Artists => state
            .search_results
            .artists
            .get(state.selected_index)
            .map(|ar| {
                (
                    CollectionKind::Artist,
                    ar.uri.clone(),
                    ar.name.clone(),
                    "Top Tracks".to_string(),
                )
            }),
        SearchCategory::Playlists => state
            .search_results
            .playlists
            .get(state.selected_index)
            .map(|pl| {
                (
                    CollectionKind::Playlist,
                    pl.uri.clone(),
                    pl.name.clone(),
                    format!("by {}", pl.owner),
                )
            }),
        SearchCategory::Tracks => None,
    };

    if let Some((kind, uri, title, subtitle)) = target {
        state.is_loading = true;
        state.status_message = Some(format!("Loading {} '{}'...", kind.label(), title));
        let client_clone = client.clone();
        let tx_clone = tx.clone();
        tokio::spawn(async move {
            let res = client_clone
                .fetch_collection_tracks(kind, &uri, &title)
                .await
                .map_err(|e| e.to_string());
            let _ = tx_clone.send(AsyncResult::CollectionTracksLoaded {
                kind,
                title,
                subtitle,
                tracks: res,
                auto_play_all,
            });
        });
    }
}

fn handle_key_event(
    key: KeyEvent,
    state: &mut AppState,
    player: &mut SpotifyPlayer,
    client: &SpotifyClient,
    tx: &UnboundedSender<AsyncResult>,
) -> bool {
    if state.input_mode == InputMode::EditingSearch {
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C'))
        {
            state.input_mode = InputMode::Normal;
            return false;
        }
        match key.code {
            KeyCode::Esc => {
                state.input_mode = InputMode::Normal;
                state.status_message = Some("Search cancelled".to_string());
            }
            KeyCode::Backspace => {
                state.search_query.pop();
            }
            KeyCode::Enter => {
                state.input_mode = InputMode::Normal;
                let query = state.search_query.trim().to_string();
                if !query.is_empty() {
                    state.is_loading = true;
                    state.opened_collection = None;
                    state.status_message = Some(format!("Searching for '{query}'..."));
                    let client_clone = client.clone();
                    let tx_clone = tx.clone();
                    tokio::spawn(async move {
                        let res = client_clone
                            .search(&query)
                            .await
                            .map_err(|e| e.to_string());
                        let _ = tx_clone.send(AsyncResult::SearchCompleted(res));
                    });
                }
            }
            KeyCode::Char(c) => {
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT)
                {
                    state.search_query.push(c);
                }
            }
            _ => {}
        }
        return false;
    }

    match key.code {
        KeyCode::Char('q') => {
            player.stop();
            return true;
        }
        KeyCode::Esc => {
            if !state.close_collection() {
                player.stop();
                return true;
            }
            state.status_message = Some("Returned to search results".to_string());
        }
        KeyCode::Backspace => {
            if state.close_collection() {
                state.status_message = Some("Returned to search results".to_string());
            }
        }
        KeyCode::Char('/') => {
            state.input_mode = InputMode::EditingSearch;
            state.status_message =
                Some("Type your search query and press [Enter]".to_string());
        }
        KeyCode::Tab => {
            state.switch_category_next();
        }
        KeyCode::BackTab => {
            state.switch_category_prev();
        }
        KeyCode::Char('1') => {
            state.set_category(SearchCategory::Tracks);
        }
        KeyCode::Char('2') => {
            state.set_category(SearchCategory::Albums);
        }
        KeyCode::Char('3') => {
            state.set_category(SearchCategory::Artists);
        }
        KeyCode::Char('4') => {
            state.set_category(SearchCategory::Playlists);
        }
        KeyCode::Up | KeyCode::Char('k') => {
            state.move_selection_up();
        }
        KeyCode::Down | KeyCode::Char('j') => {
            state.move_selection_down();
        }
        KeyCode::Enter => {
            if let Some(ref col) = state.opened_collection {
                let tracks = col.tracks.clone();
                let idx = state.collection_selected_index;
                if let Some(track) = state.start_playback_queue(tracks, idx) {
                    let msg = format!("Loading '{}' — {}", track.title, track.artist);
                    load_track_with_status(player, state, client, tx, &track, msg);
                }
            } else if state.search_category == SearchCategory::Tracks {
                let tracks = state.search_results.tracks.clone();
                let idx = state.selected_index;
                if let Some(track) = state.start_playback_queue(tracks, idx) {
                    let msg = format!("Loading '{}' — {}", track.title, track.artist);
                    load_track_with_status(player, state, client, tx, &track, msg);
                }
            } else {
                trigger_collection_load(state, client, tx, false);
            }
        }
        KeyCode::Char('p') => {
            if let Some(ref col) = state.opened_collection {
                let tracks = col.tracks.clone();
                let title = col.title.clone();
                if let Some(track) = state.start_playback_queue(tracks, 0) {
                    let msg = format!("Loading from '{title}': '{}'", track.title);
                    load_track_with_status(player, state, client, tx, &track, msg);
                }
            } else if state.search_category == SearchCategory::Tracks {
                let tracks = state.search_results.tracks.clone();
                let idx = state.selected_index;
                if let Some(track) = state.start_playback_queue(tracks, idx) {
                    let msg = format!("Loading '{}' — {}", track.title, track.artist);
                    load_track_with_status(player, state, client, tx, &track, msg);
                }
            } else {
                trigger_collection_load(state, client, tx, true);
            }
        }
        KeyCode::Char('n') => {
            if let Some(track) = state.next_track() {
                let msg = format!("Next track: '{}' — {}", track.title, track.artist);
                load_track_with_status(player, state, client, tx, &track, msg);
            } else {
                state.status_message = Some("End of playback queue".to_string());
            }
        }
        KeyCode::Char('b') => {
            if let Some(track) = state.prev_track() {
                let msg = format!("Previous track: '{}' — {}", track.title, track.artist);
                load_track_with_status(player, state, client, tx, &track, msg);
            } else {
                state.status_message = Some("Already at start of queue".to_string());
            }
        }
        KeyCode::Char(' ') => {
            if !state.track_uri.is_empty() {
                match state.playback_status.clone() {
                    PlaybackStatus::Playing => {
                        player.pause();
                        state.playback_status = PlaybackStatus::Paused;
                        state.is_playing = false;
                    }
                    PlaybackStatus::Paused | PlaybackStatus::Idle => {
                        if let Some(err) = state.audio_error.clone() {
                            state.set_playback_failed(err);
                        } else {
                            player.play();
                            state.playback_status = PlaybackStatus::Loading;
                            state.loading_elapsed_ms = 0;
                        }
                    }
                    PlaybackStatus::Failed(_) => {
                        if let Some(track) = state.current_track_metadata() {
                            state.set_track(track.clone());
                            let msg = format!("Retrying '{}' — {}", track.title, track.artist);
                            load_track_with_status(player, state, client, tx, &track, msg);
                        }
                    }
                    PlaybackStatus::Loading => {}
                }
            }
        }
        KeyCode::Right => {
            let new_pos = (state.position_ms + 5000).min(state.duration_ms);
            player.seek(new_pos);
            state.position_ms = new_pos;
        }
        KeyCode::Left => {
            let new_pos = state.position_ms.saturating_sub(5000);
            player.seek(new_pos);
            state.position_ms = new_pos;
        }
        KeyCode::Char('+') | KeyCode::Char('=') => {
            let new_vol = state.volume.saturating_add(3276);
            player.set_volume(new_vol);
            state.volume = new_vol;
        }
        KeyCode::Char('-') => {
            let new_vol = state.volume.saturating_sub(3276);
            player.set_volume(new_vol);
            state.volume = new_vol;
        }
        _ => {}
    }

    false
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    spotify::init_tls_provider();
    spotify::logger::init();
    let session = authenticate().await?;
    let username = session.username();
    let device_name = "crystalify".to_string();

    let client = SpotifyClient::new(session.clone());
    let mut player = SpotifyPlayer::new(session.clone())?;
    let initial_volume = player.volume();

    let mut state = AppState::new(username, device_name, initial_volume);
    state.set_audio_info(player.backend_label.clone(), player.audio_error.clone());

    let (async_tx, mut async_rx) = mpsc::unbounded_channel::<AsyncResult>();

    if let Some(track_arg) = env::args().nth(1)
        && let Ok(track_uri) = SpotifyUri::from_uri(&track_arg)
    {
        if let Ok(meta) = SpotifyPlayer::fetch_metadata(&session, &track_uri).await
            && let Some(track) = state.start_playback_queue(vec![meta], 0)
        {
            load_track_into_player(&player, &mut state, &client, &async_tx, &track);
        } else {
            player.load(track_uri);
        }
    }

    let default_panic = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(stdout(), LeaveAlternateScreen);
        default_panic(info);
    }));

    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let picker = Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks());
    state.set_picker(picker);
    let backend_terminal = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend_terminal)?;

    'main_loop: loop {
        terminal.draw(|frame| draw_ui(frame, &state))?;

        tokio::select! {
            Some(event) = player.events.recv() => {
                state.handle_player_event(event);
                if state.pending_next_track {
                    state.pending_next_track = false;
                    if let Some(next) = state.next_track() {
                        let msg = format!("Auto-playing next: '{}' — {}", next.title, next.artist);
                        load_track_with_status(&player, &mut state, &client, &async_tx, &next, msg);
                    }
                }
            }
            Some(err) = player.audio_errors.recv() => {
                state.set_audio_info(player.backend_label.clone(), Some(err));
            }
            Some(async_res) = async_rx.recv() => {
                match async_res {
                    AsyncResult::CoverLoaded { url, image } => {
                        state.cache_and_apply_cover(url, image);
                    }
                    AsyncResult::SearchCompleted(Ok(results)) => {
                        state.is_loading = false;
                        let summary = format!(
                            "Found {} tracks, {} albums, {} artists, {} playlists",
                            results.tracks.len(),
                            results.albums.len(),
                            results.artists.len(),
                            results.playlists.len()
                        );
                        state.search_results = results;
                        state.selected_index = 0;
                        state.opened_collection = None;
                        state.status_message = Some(summary);
                    }
                    AsyncResult::SearchCompleted(Err(err)) => {
                        state.is_loading = false;
                        state.status_message = Some(format!("Search failed: {err}"));
                    }
                    AsyncResult::CollectionTracksLoaded {
                        kind,
                        title,
                        subtitle,
                        tracks: Ok(tracks),
                        auto_play_all,
                    } => {
                        state.is_loading = false;
                        if tracks.is_empty() {
                            state.status_message = Some(format!(
                                "No playable tracks found in {} '{title}'",
                                kind.label()
                            ));
                        } else {
                            state.open_collection(kind, title.clone(), subtitle, tracks.clone());
                            if auto_play_all {
                                if let Some(first) = state.start_playback_queue(tracks, 0) {
                                    let msg = format!(
                                        "Playing {} '{title}' — '{}'",
                                        kind.label(),
                                        first.title
                                    );
                                    load_track_with_status(
                                        &player,
                                        &mut state,
                                        &client,
                                        &async_tx,
                                        &first,
                                        msg,
                                    );
                                }
                            } else {
                                state.status_message = Some(format!(
                                    "Opened {} '{title}' ({} tracks)",
                                    kind.label(),
                                    tracks.len()
                                ));
                            }
                        }
                    }
                    AsyncResult::CollectionTracksLoaded {
                        kind,
                        title,
                        tracks: Err(err),
                        ..
                    } => {
                        state.is_loading = false;
                        state.status_message = Some(format!(
                            "Failed to load {} '{title}': {err}",
                            kind.label()
                        ));
                    }
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(30)) => {
                state.tick(30);

                while event::poll(Duration::from_millis(0))? {
                    if let Event::Key(key) = event::read()?
                        && key.kind == KeyEventKind::Press
                        && handle_key_event(key, &mut state, &mut player, &client, &async_tx)
                    {
                        break 'main_loop;
                    }
                }
            }
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    Ok(())
}
