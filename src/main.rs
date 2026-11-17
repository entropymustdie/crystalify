mod app;
mod spotify;
mod ui;

use app::{AppState, InputMode};
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use librespot::core::spotify_uri::SpotifyUri;
use ratatui::{backend::CrosstermBackend, Terminal};
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
}

fn load_track_into_player(player: &SpotifyPlayer, state: &mut AppState, track: &TrackMetadata) {
    if let Ok(uri) = SpotifyUri::from_uri(&track.uri) {
        player.load(uri);
    } else {
        state.status_message = Some(format!("Invalid track URI: {}", track.uri));
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

    // Normal mode
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
                    state.status_message =
                        Some(format!("Playing '{}' — {}", track.title, track.artist));
                    load_track_into_player(player, state, &track);
                }
            } else if state.search_category == SearchCategory::Tracks {
                let tracks = state.search_results.tracks.clone();
                let idx = state.selected_index;
                if let Some(track) = state.start_playback_queue(tracks, idx) {
                    state.status_message =
                        Some(format!("Playing '{}' — {}", track.title, track.artist));
                    load_track_into_player(player, state, &track);
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
                    state.status_message =
                        Some(format!("Playing all from '{title}': '{}'", track.title));
                    load_track_into_player(player, state, &track);
                }
            } else if state.search_category == SearchCategory::Tracks {
                let tracks = state.search_results.tracks.clone();
                let idx = state.selected_index;
                if let Some(track) = state.start_playback_queue(tracks, idx) {
                    state.status_message =
                        Some(format!("Playing '{}' — {}", track.title, track.artist));
                    load_track_into_player(player, state, &track);
                }
            } else {
                trigger_collection_load(state, client, tx, true);
            }
        }
        KeyCode::Char('n') => {
            if let Some(track) = state.next_track() {
                state.status_message =
                    Some(format!("Next track: '{}' — {}", track.title, track.artist));
                load_track_into_player(player, state, &track);
            } else {
                state.status_message = Some("End of playback queue".to_string());
            }
        }
        KeyCode::Char('b') => {
            if let Some(track) = state.prev_track() {
                state.status_message = Some(format!(
                    "Previous track: '{}' — {}",
                    track.title, track.artist
                ));
                load_track_into_player(player, state, &track);
            } else {
                state.status_message = Some("Already at start of queue".to_string());
            }
        }
        KeyCode::Char(' ') => {
            if state.is_playing {
                player.pause();
                state.is_playing = false;
            } else if !state.track_uri.is_empty() {
                player.play();
                state.is_playing = true;
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
    let session = authenticate().await?;
    let username = session.username();
    let device_name = "crystalify".to_string();

    let client = SpotifyClient::new(session.clone());
    let mut player = SpotifyPlayer::new(session.clone())?;
    let initial_volume = player.volume();

    let mut state = AppState::new(username, device_name, initial_volume);

    if let Some(track_arg) = env::args().nth(1)
        && let Ok(track_uri) = SpotifyUri::from_uri(&track_arg)
    {
        if let Ok(meta) = SpotifyPlayer::fetch_metadata(&session, &track_uri).await {
            state.start_playback_queue(vec![meta], 0);
        }
        player.load(track_uri);
    }

    let (async_tx, mut async_rx) = mpsc::unbounded_channel::<AsyncResult>();

    let default_panic = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(stdout(), LeaveAlternateScreen);
        default_panic(info);
    }));

    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen)?;
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
                        state.status_message = Some(format!(
                            "Auto-playing next: '{}' — {}",
                            next.title, next.artist
                        ));
                        load_track_into_player(&player, &mut state, &next);
                    }
                }
            }
            Some(async_res) = async_rx.recv() => {
                state.is_loading = false;
                match async_res {
                    AsyncResult::SearchCompleted(Ok(results)) => {
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
                        state.status_message = Some(format!("Search failed: {err}"));
                    }
                    AsyncResult::CollectionTracksLoaded {
                        kind,
                        title,
                        subtitle,
                        tracks: Ok(tracks),
                        auto_play_all,
                    } => {
                        if tracks.is_empty() {
                            state.status_message = Some(format!(
                                "No playable tracks found in {} '{title}'",
                                kind.label()
                            ));
                        } else {
                            state.open_collection(kind, title.clone(), subtitle, tracks.clone());
                            if auto_play_all {
                                if let Some(first) = state.start_playback_queue(tracks, 0) {
                                    state.status_message = Some(format!(
                                        "Playing {} '{title}' — '{}'",
                                        kind.label(),
                                        first.title
                                    ));
                                    load_track_into_player(&player, &mut state, &first);
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
