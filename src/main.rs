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
use spotify::auth::{load_initial_auth_plan, prompt_interactive_credentials};
use spotify::client::{CollectionBatchUpdate, SpotifyClient};
use spotify::models::{
    AlbumItem, BrowserView, CollectionKind, CollectionView, LibraryCategory, PlaylistItem,
    SearchCategory, SearchResults, TrackLyrics, TrackMetadata,
};
use spotify::player::{SpotifyPlayer, build_connect_track_window};
use std::env;
use std::io::stdout;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::mpsc::{self, UnboundedSender};
use tokio::time::MissedTickBehavior;
use ui::draw_ui;

enum AsyncResult {
    SearchCompleted(Result<SearchResults, String>),
    LibraryPlaylistsLoaded {
        request_id: u64,
        playlists: Result<Vec<PlaylistItem>, String>,
    },
    LibraryAlbumsLoaded {
        request_id: u64,
        albums: Result<Vec<AlbumItem>, String>,
    },
    LibraryLikedBatch {
        request_id: u64,
        batch: CollectionBatchUpdate,
    },
    LibraryFailed {
        request_id: u64,
        error: String,
    },
    CollectionTracksBatch {
        request_id: u64,
        kind: CollectionKind,
        uri: String,
        title: String,
        subtitle: String,
        batch: CollectionBatchUpdate,
        auto_play_all: bool,
        connect_already_started: bool,
    },
    CollectionTracksFailed {
        request_id: u64,
        kind: CollectionKind,
        title: String,
        error: String,
    },
    CoverLoaded {
        url: String,
        image: DynamicImage,
    },
    LyricsLoaded {
        uri: String,
        lyrics: TrackLyrics,
    },
}

fn spawn_library_fetch(
    state: &mut AppState,
    client: &SpotifyClient,
    tx: &UnboundedSender<AsyncResult>,
    library_cancel: &Arc<AtomicU64>,
) {
    let request_id = library_cancel.fetch_add(1, Ordering::Relaxed) + 1;
    state.library_loading = true;
    state.library.liked_loading_more = true;

    let client_pl = client.clone();
    let tx_pl = tx.clone();
    let cancel_pl = library_cancel.clone();
    tokio::spawn(async move {
        let res = client_pl
            .get_user_playlists()
            .await
            .map_err(|e| e.to_string());
        if cancel_pl.load(Ordering::Relaxed) == request_id {
            let _ = tx_pl.send(AsyncResult::LibraryPlaylistsLoaded {
                request_id,
                playlists: res,
            });
        }
    });

    let client_col = client.clone();
    let tx_col = tx.clone();
    let cancel_col = library_cancel.clone();
    tokio::spawn(async move {
        let (track_uris, album_uris) = client_col.fetch_collection_uris_for_library().await;
        if cancel_col.load(Ordering::Relaxed) != request_id {
            return;
        }

        let client_al = client_col.clone();
        let tx_al = tx_col.clone();
        let cancel_al = cancel_col.clone();
        tokio::spawn(async move {
            let res = if !album_uris.is_empty() {
                client_al
                    .resolve_saved_albums(album_uris)
                    .await
                    .map_err(|e| e.to_string())
            } else {
                client_al
                    .get_saved_albums()
                    .await
                    .map_err(|e| e.to_string())
            };
            if cancel_al.load(Ordering::Relaxed) == request_id {
                let _ = tx_al.send(AsyncResult::LibraryAlbumsLoaded {
                    request_id,
                    albums: res,
                });
            }
        });

        let tx_liked = tx_col.clone();
        let res = client_col
            .stream_liked_tracks(track_uris, cancel_col.clone(), request_id, move |batch| {
                let _ = tx_liked.send(AsyncResult::LibraryLikedBatch { request_id, batch });
            })
            .await;

        if let Err(err) = res
            && cancel_col.load(Ordering::Relaxed) == request_id
        {
            let _ = tx_col.send(AsyncResult::LibraryFailed {
                request_id,
                error: err.to_string(),
            });
        }
    });
}

fn spawn_lyrics_fetch_if_needed(
    state: &AppState,
    client: &SpotifyClient,
    tx: &UnboundedSender<AsyncResult>,
    track: &TrackMetadata,
) {
    if track.uri.is_empty() || state.lyrics_cache.contains_key(&track.uri) {
        return;
    }
    let uri = track.uri.clone();
    let title = track.title.clone();
    let artist = track.artist.clone();
    let album = track.album.clone();
    let duration_ms = track.duration_ms;
    let client_clone = client.clone();
    let tx_clone = tx.clone();
    tokio::spawn(async move {
        let lyrics = client_clone
            .fetch_lyrics(&title, &artist, &album, duration_ms)
            .await;
        let _ = tx_clone.send(AsyncResult::LyricsLoaded { uri, lyrics });
    });
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
    spawn_lyrics_fetch_if_needed(state, client, tx, track);
    if state.audio_error.is_some() {
        spawn_cover_fetch_if_needed(state, client, tx);
        return;
    }
    if player.is_connect_active() {
        if !state.queue.is_empty() {
            let (uris, local_idx) = build_connect_track_window(&state.queue, state.queue_index);
            player.load_tracks(uris, local_idx);
        } else {
            player.load_tracks(vec![track.uri.clone()], 0);
        }
        spawn_cover_fetch_if_needed(state, client, tx);
    } else if let Ok(uri) = SpotifyUri::from_uri(&track.uri) {
        player.load(uri);
        spawn_cover_fetch_if_needed(state, client, tx);
    } else {
        state.set_playback_failed(format!("Invalid track URI: {}", track.uri));
    }
}

fn load_context_or_queue_into_player(
    player: &SpotifyPlayer,
    state: &mut AppState,
    client: &SpotifyClient,
    tx: &UnboundedSender<AsyncResult>,
    context_uri: Option<String>,
    track: &TrackMetadata,
    status_message: String,
) {
    if state.audio_error.is_none() {
        state.status_message = Some(status_message);
    }
    spawn_lyrics_fetch_if_needed(state, client, tx, track);
    if state.audio_error.is_some() {
        spawn_cover_fetch_if_needed(state, client, tx);
        return;
    }
    if player.is_connect_active() {
        if !state.queue.is_empty() {
            let (uris, local_idx) = build_connect_track_window(&state.queue, state.queue_index);
            player.load_tracks(uris, local_idx);
        } else if let Some(ctx_uri) = context_uri.filter(|u| !u.trim().is_empty()) {
            player.load_context(ctx_uri, Some(track.uri.clone()), true);
        } else {
            player.load_tracks(vec![track.uri.clone()], 0);
        }
        spawn_cover_fetch_if_needed(state, client, tx);
    } else if let Ok(uri) = SpotifyUri::from_uri(&track.uri) {
        player.load(uri);
        spawn_cover_fetch_if_needed(state, client, tx);
    } else {
        state.set_playback_failed(format!("Invalid track URI: {}", track.uri));
    }
}

fn trigger_collection_load(
    state: &mut AppState,
    player: &SpotifyPlayer,
    client: &SpotifyClient,
    tx: &UnboundedSender<AsyncResult>,
    collection_cancel: &Arc<AtomicU64>,
    auto_play_all: bool,
) {
    let target = match state.browser_view {
        BrowserView::Search => match state.search_category {
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
        },
        BrowserView::Library => match state.library_category {
            LibraryCategory::Playlists => state
                .library
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
            LibraryCategory::SavedAlbums => state
                .library
                .saved_albums
                .get(state.selected_index)
                .map(|a| {
                    (
                        CollectionKind::Album,
                        a.uri.clone(),
                        a.name.clone(),
                        a.artist.clone(),
                    )
                }),
            LibraryCategory::LikedSongs => None,
        },
    };

    if let Some((kind, uri, title, subtitle)) = target {
        let request_id = collection_cancel.fetch_add(1, Ordering::Relaxed) + 1;
        state.is_loading = true;

        let connect_already_started = if auto_play_all
            && state.audio_error.is_none()
            && player.is_connect_active()
            && !uri.trim().is_empty()
        {
            state.playback_status = PlaybackStatus::Loading;
            state.loading_elapsed_ms = 0;
            state.is_playing = false;
            player.load_context(uri.clone(), None, true);
            state.status_message = Some(format!(
                "Starting {} '{}' & streaming tracks...",
                kind.label(),
                title
            ));
            true
        } else {
            state.status_message = Some(format!("Loading {} '{}'...", kind.label(), title));
            false
        };

        let client_clone = client.clone();
        let tx_clone = tx.clone();
        let cancel_clone = collection_cancel.clone();
        tokio::spawn(async move {
            let tx_batch = tx_clone.clone();
            let uri_for_cb = uri.clone();
            let title_for_cb = title.clone();
            let subtitle_for_cb = subtitle.clone();

            let res = client_clone
                .stream_collection_tracks(
                    kind,
                    &uri,
                    &title,
                    cancel_clone.clone(),
                    request_id,
                    move |batch| {
                        let _ = tx_batch.send(AsyncResult::CollectionTracksBatch {
                            request_id,
                            kind,
                            uri: uri_for_cb.clone(),
                            title: title_for_cb.clone(),
                            subtitle: subtitle_for_cb.clone(),
                            batch,
                            auto_play_all,
                            connect_already_started,
                        });
                    },
                )
                .await;

            if let Err(err) = res
                && cancel_clone.load(Ordering::Relaxed) == request_id
            {
                let _ = tx_clone.send(AsyncResult::CollectionTracksFailed {
                    request_id,
                    kind,
                    title,
                    error: err.to_string(),
                });
            }
        });
    }
}

fn handle_key_event(
    key: KeyEvent,
    state: &mut AppState,
    player: &mut SpotifyPlayer,
    client: &SpotifyClient,
    tx: &UnboundedSender<AsyncResult>,
    collection_cancel: &Arc<AtomicU64>,
    library_cancel: &Arc<AtomicU64>,
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
                    collection_cancel.fetch_add(1, Ordering::Relaxed);
                    state.is_loading = true;
                    state.opened_collection = None;
                    state.browser_view = BrowserView::Search;
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

    if key.modifiers.contains(KeyModifiers::CONTROL) {
        match key.code {
            KeyCode::Char('u') | KeyCode::Char('U') => {
                state.move_selection_page_up(15);
                state.schedule_preload_for_selection();
                return false;
            }
            KeyCode::Char('d') | KeyCode::Char('D') => {
                state.move_selection_page_down(15);
                state.schedule_preload_for_selection();
                return false;
            }
            _ => {}
        }
    }

    match key.code {
        KeyCode::Char('q') => {
            return true;
        }
        KeyCode::Esc => {
            if !state.close_collection() {
                return true;
            }
            collection_cancel.fetch_add(1, Ordering::Relaxed);
            state.is_loading = false;
            let view_label = match state.browser_view {
                BrowserView::Search => "search results",
                BrowserView::Library => "library",
            };
            state.status_message = Some(format!("Returned to {view_label}"));
        }
        KeyCode::Backspace => {
            if state.close_collection() {
                collection_cancel.fetch_add(1, Ordering::Relaxed);
                state.is_loading = false;
                let view_label = match state.browser_view {
                    BrowserView::Search => "search results",
                    BrowserView::Library => "library",
                };
                state.status_message = Some(format!("Returned to {view_label}"));
            }
        }
        KeyCode::Char('/') => {
            collection_cancel.fetch_add(1, Ordering::Relaxed);
            state.set_browser_view(BrowserView::Search);
            state.input_mode = InputMode::EditingSearch;
            state.status_message =
                Some("Type your search query and press [Enter]".to_string());
        }
        KeyCode::Char('l') => {
            collection_cancel.fetch_add(1, Ordering::Relaxed);
            state.is_loading = false;
            state.toggle_browser_view();
            state.schedule_preload_for_selection();
            if state.browser_view == BrowserView::Library {
                if !state.library.loaded && !state.library_loading {
                    spawn_library_fetch(state, client, tx, library_cancel);
                    state.status_message = Some("Loading your Spotify Library...".to_string());
                } else {
                    state.status_message = Some(format!(
                        "Library: {} playlists, {} liked songs, {} saved albums",
                        state.library.playlists.len(),
                        state.library.liked_tracks.len(),
                        state.library.saved_albums.len()
                    ));
                }
            } else {
                state.status_message = Some("Switched to Search view".to_string());
            }
        }
        KeyCode::Char('r') => {
            if state.browser_view == BrowserView::Library {
                client.clear_collection_cache();
                spawn_library_fetch(state, client, tx, library_cancel);
                state.status_message = Some("Refreshing your Spotify Library...".to_string());
            }
        }
        KeyCode::Tab => {
            collection_cancel.fetch_add(1, Ordering::Relaxed);
            state.is_loading = false;
            state.switch_category_next();
            state.schedule_preload_for_selection();
        }
        KeyCode::BackTab => {
            collection_cancel.fetch_add(1, Ordering::Relaxed);
            state.is_loading = false;
            state.switch_category_prev();
            state.schedule_preload_for_selection();
        }
        KeyCode::Char('1') => {
            collection_cancel.fetch_add(1, Ordering::Relaxed);
            state.is_loading = false;
            match state.browser_view {
                BrowserView::Search => state.set_category(SearchCategory::Tracks),
                BrowserView::Library => state.set_library_category(LibraryCategory::Playlists),
            }
            state.schedule_preload_for_selection();
        }
        KeyCode::Char('2') => {
            collection_cancel.fetch_add(1, Ordering::Relaxed);
            state.is_loading = false;
            match state.browser_view {
                BrowserView::Search => state.set_category(SearchCategory::Albums),
                BrowserView::Library => state.set_library_category(LibraryCategory::LikedSongs),
            }
            state.schedule_preload_for_selection();
        }
        KeyCode::Char('3') => {
            collection_cancel.fetch_add(1, Ordering::Relaxed);
            state.is_loading = false;
            match state.browser_view {
                BrowserView::Search => state.set_category(SearchCategory::Artists),
                BrowserView::Library => state.set_library_category(LibraryCategory::SavedAlbums),
            }
            state.schedule_preload_for_selection();
        }
        KeyCode::Char('4') => {
            if state.browser_view == BrowserView::Search {
                collection_cancel.fetch_add(1, Ordering::Relaxed);
                state.is_loading = false;
                state.set_category(SearchCategory::Playlists);
                state.schedule_preload_for_selection();
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            state.move_selection_up();
            state.schedule_preload_for_selection();
        }
        KeyCode::Down | KeyCode::Char('j') => {
            state.move_selection_down();
            state.schedule_preload_for_selection();
        }
        KeyCode::PageUp => {
            state.move_selection_page_up(15);
            state.schedule_preload_for_selection();
        }
        KeyCode::PageDown => {
            state.move_selection_page_down(15);
            state.schedule_preload_for_selection();
        }
        KeyCode::Home | KeyCode::Char('g') => {
            state.move_selection_top();
            state.schedule_preload_for_selection();
        }
        KeyCode::End | KeyCode::Char('G') => {
            state.move_selection_bottom();
            state.schedule_preload_for_selection();
        }
        KeyCode::Enter => {
            if let Some(ref col) = state.opened_collection {
                let tracks = col.tracks.clone();
                let ctx_uri = col.uri.clone();
                let idx = state.collection_selected_index;
                if let Some(track) = state.start_playback_queue(tracks, idx) {
                    let msg = format!("Loading '{}' — {}", track.title, track.artist);
                    load_context_or_queue_into_player(
                        player,
                        state,
                        client,
                        tx,
                        Some(ctx_uri),
                        &track,
                        msg,
                    );
                }
            } else if state.browser_view == BrowserView::Search
                && state.search_category == SearchCategory::Tracks
            {
                let tracks = state.search_results.tracks.clone();
                let idx = state.selected_index;
                if let Some(track) = state.start_playback_queue(tracks, idx) {
                    let msg = format!("Loading '{}' — {}", track.title, track.artist);
                    load_track_with_status(player, state, client, tx, &track, msg);
                }
            } else if state.browser_view == BrowserView::Library
                && state.library_category == LibraryCategory::LikedSongs
            {
                let tracks = state.library.liked_tracks.clone();
                let idx = state.selected_index;
                let ctx_uri = if state.username.is_empty() {
                    None
                } else {
                    Some(format!("spotify:user:{}:collection", state.username))
                };
                if let Some(track) = state.start_playback_queue(tracks, idx) {
                    let msg = format!("Loading '{}' — {}", track.title, track.artist);
                    load_context_or_queue_into_player(
                        player, state, client, tx, ctx_uri, &track, msg,
                    );
                }
            } else {
                trigger_collection_load(state, player, client, tx, collection_cancel, false);
            }
        }
        KeyCode::Char('p') => {
            if let Some(ref col) = state.opened_collection {
                let tracks = col.tracks.clone();
                let title = col.title.clone();
                let ctx_uri = col.uri.clone();
                if let Some(track) = state.start_playback_queue(tracks, 0) {
                    let msg = format!("Loading from '{title}': '{}'", track.title);
                    load_context_or_queue_into_player(
                        player,
                        state,
                        client,
                        tx,
                        Some(ctx_uri),
                        &track,
                        msg,
                    );
                }
            } else if state.browser_view == BrowserView::Search
                && state.search_category == SearchCategory::Tracks
            {
                let tracks = state.search_results.tracks.clone();
                let idx = state.selected_index;
                if let Some(track) = state.start_playback_queue(tracks, idx) {
                    let msg = format!("Loading '{}' — {}", track.title, track.artist);
                    load_track_with_status(player, state, client, tx, &track, msg);
                }
            } else if state.browser_view == BrowserView::Library
                && state.library_category == LibraryCategory::LikedSongs
            {
                let tracks = state.library.liked_tracks.clone();
                let ctx_uri = if state.username.is_empty() {
                    None
                } else {
                    Some(format!("spotify:user:{}:collection", state.username))
                };
                if let Some(track) = state.start_playback_queue(tracks, 0) {
                    let msg = format!("Loading Liked Songs: '{}'", track.title);
                    load_context_or_queue_into_player(
                        player, state, client, tx, ctx_uri, &track, msg,
                    );
                }
            } else {
                trigger_collection_load(state, player, client, tx, collection_cancel, true);
            }
        }
        KeyCode::Char('n') => {
            if let Some(track) = state.next_track() {
                if player.is_connect_active() {
                    if state.audio_error.is_none() {
                        state.status_message =
                            Some(format!("Next track: '{}' — {}", track.title, track.artist));
                    }
                    spawn_lyrics_fetch_if_needed(state, client, tx, &track);
                    spawn_cover_fetch_if_needed(state, client, tx);
                    player.next();
                } else {
                    let msg = format!("Next track: '{}' — {}", track.title, track.artist);
                    load_track_with_status(player, state, client, tx, &track, msg);
                }
            } else if player.is_connect_active() {
                state.status_message = Some("Next track (Spotify Connect)...".to_string());
                player.next();
            } else {
                state.status_message = Some("End of playback queue".to_string());
            }
        }
        KeyCode::Char('b') => {
            if let Some(track) = state.prev_track() {
                if player.is_connect_active() {
                    if state.audio_error.is_none() {
                        state.status_message = Some(format!(
                            "Previous track: '{}' — {}",
                            track.title, track.artist
                        ));
                    }
                    spawn_lyrics_fetch_if_needed(state, client, tx, &track);
                    spawn_cover_fetch_if_needed(state, client, tx);
                    player.prev();
                } else {
                    let msg = format!("Previous track: '{}' — {}", track.title, track.artist);
                    load_track_with_status(player, state, client, tx, &track, msg);
                }
            } else if player.is_connect_active() {
                state.status_message = Some("Previous track (Spotify Connect)...".to_string());
                player.prev();
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
        KeyCode::Char('w') | KeyCode::Char('[') => {
            state.scroll_lyrics_up();
        }
        KeyCode::Char('s') | KeyCode::Char(']') => {
            state.scroll_lyrics_down();
        }
        _ => {}
    }

    false
}

fn process_async_result(
    async_res: AsyncResult,
    state: &mut AppState,
    player: &SpotifyPlayer,
    client: &SpotifyClient,
    async_tx: &UnboundedSender<AsyncResult>,
    collection_cancel: &Arc<AtomicU64>,
    library_cancel: &Arc<AtomicU64>,
) {
    match async_res {
        AsyncResult::CoverLoaded { url, image } => {
            state.cache_and_apply_cover(url, image);
        }
        AsyncResult::LyricsLoaded { uri, lyrics } => {
            state.cache_and_apply_lyrics(uri, lyrics);
        }
        AsyncResult::LibraryPlaylistsLoaded {
            request_id,
            playlists,
        } => {
            if library_cancel.load(Ordering::Relaxed) != request_id {
                return;
            }
            if let Ok(list) = playlists {
                state.apply_library_playlists(list);
                if state.browser_view == BrowserView::Library {
                    state.status_message = Some(format!(
                        "Library: {} playlists, {} liked songs, {} saved albums",
                        state.library.playlists.len(),
                        state.library.liked_tracks.len(),
                        state.library.saved_albums.len()
                    ));
                }
            }
        }
        AsyncResult::LibraryAlbumsLoaded { request_id, albums } => {
            if library_cancel.load(Ordering::Relaxed) != request_id {
                return;
            }
            if let Ok(list) = albums {
                state.apply_library_albums(list);
                if state.browser_view == BrowserView::Library {
                    state.status_message = Some(format!(
                        "Library: {} playlists, {} liked songs, {} saved albums",
                        state.library.playlists.len(),
                        state.library.liked_tracks.len(),
                        state.library.saved_albums.len()
                    ));
                }
            }
        }
        AsyncResult::LibraryLikedBatch { request_id, batch } => {
            if library_cancel.load(Ordering::Relaxed) != request_id {
                return;
            }
            let was_first = batch.is_first;
            state.apply_library_liked_batch(
                batch.tracks,
                batch.total_expected,
                batch.is_first,
                batch.is_done,
            );
            if was_first {
                state.schedule_preload_for_selection();
            }
            if state.browser_view == BrowserView::Library {
                state.status_message = Some(format!(
                    "Library: {} playlists, {} liked songs, {} saved albums",
                    state.library.playlists.len(),
                    state.library.liked_tracks.len(),
                    state.library.saved_albums.len()
                ));
            }
        }
        AsyncResult::LibraryFailed { request_id, error } => {
            if library_cancel.load(Ordering::Relaxed) != request_id {
                return;
            }
            state.library_loading = false;
            state.library.liked_loading_more = false;
            if state.browser_view == BrowserView::Library {
                state.status_message = Some(format!("Library load failed: {error}"));
            }
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
            state.schedule_preload_for_selection();
        }
        AsyncResult::SearchCompleted(Err(err)) => {
            state.is_loading = false;
            state.status_message = Some(format!("Search failed: {err}"));
        }
        AsyncResult::CollectionTracksBatch {
            request_id,
            kind,
            uri,
            title,
            subtitle,
            batch,
            auto_play_all,
            connect_already_started,
        } => {
            if collection_cancel.load(Ordering::Relaxed) != request_id {
                return;
            }
            if batch.is_first {
                state.is_loading = false;
                if batch.tracks.is_empty() && batch.is_done {
                    state.status_message = Some(format!(
                        "No playable tracks found in {} '{title}'",
                        kind.label()
                    ));
                    return;
                }
                let first_tracks = batch.tracks.clone();
                state.open_streaming_collection(CollectionView {
                    kind,
                    uri: uri.clone(),
                    title: title.clone(),
                    subtitle,
                    tracks: first_tracks.clone(),
                    total_tracks: batch.total_expected,
                    is_loading_more: !batch.is_done,
                });
                if !auto_play_all {
                    state.schedule_preload_for_selection();
                }
                if auto_play_all {
                    if connect_already_started {
                        state.queue = first_tracks.clone();
                        state.queue_index = 0;
                        if state.track_uri.is_empty()
                            && let Some(first) = first_tracks.first()
                        {
                            state.set_track(first.clone());
                            spawn_cover_fetch_if_needed(state, client, async_tx);
                            spawn_lyrics_fetch_if_needed(state, client, async_tx, first);
                        }
                    } else if let Some(first) = state.start_playback_queue(first_tracks, 0) {
                        let msg = format!(
                            "Playing {} '{title}' — '{}'",
                            kind.label(),
                            first.title
                        );
                        load_context_or_queue_into_player(
                            player,
                            state,
                            client,
                            async_tx,
                            Some(uri),
                            &first,
                            msg,
                        );
                    }
                } else {
                    let loaded = state
                        .opened_collection
                        .as_ref()
                        .map(|c| c.tracks.len())
                        .unwrap_or(0);
                    let status = if batch.is_done {
                        format!("Opened {} '{title}' ({loaded} tracks)", kind.label())
                    } else if let Some(total) = batch.total_expected {
                        format!(
                            "Opened {} '{title}' ({loaded} / {total} tracks loaded...)",
                            kind.label()
                        )
                    } else {
                        format!(
                            "Opened {} '{title}' ({loaded}+ tracks loaded...)",
                            kind.label()
                        )
                    };
                    state.status_message = Some(status);
                }
            } else {
                let batch_for_queue = batch.tracks.clone();
                state.append_collection_tracks(
                    batch.tracks,
                    batch.total_expected,
                    batch.is_done,
                );
                if auto_play_all && !batch_for_queue.is_empty() {
                    state.queue.extend(batch_for_queue);
                }
                if let Some(ref col) = state.opened_collection {
                    let loaded = col.tracks.len();
                    if batch.is_done {
                        state.status_message = Some(format!(
                            "Loaded all {loaded} tracks in {} '{title}'",
                            kind.label()
                        ));
                    } else if let Some(total) = col.total_tracks {
                        state.status_message = Some(format!(
                            "Loading {} '{title}' ({loaded} / {total} tracks...)",
                            kind.label()
                        ));
                    }
                }
            }
        }
        AsyncResult::CollectionTracksFailed {
            request_id,
            kind,
            title,
            error,
        } => {
            if collection_cancel.load(Ordering::Relaxed) != request_id {
                return;
            }
            state.is_loading = false;
            if let Some(ref mut col) = state.opened_collection {
                col.is_loading_more = false;
            }
            state.status_message = Some(format!(
                "Failed to load {} '{title}': {error}",
                kind.label()
            ));
        }
    }
}

async fn init_session_and_player(
    device_name: &str,
) -> Result<(librespot::core::session::Session, SpotifyPlayer), Box<dyn std::error::Error>> {
    let plan = load_initial_auth_plan();

    if let Some(cached_creds) = plan.cached_credentials {
        println!("Found cached Spotify credentials, connecting...");
        match SpotifyPlayer::connect_with_spirc(cached_creds, plan.cache.clone(), device_name).await
        {
            Ok((session, player)) => {
                println!("Logged in as: {}", session.username());
                return Ok((session, player));
            }
            Err(e) => {
                eprintln!(
                    "Cached credentials expired or failed ({e}). Falling back to interactive login..."
                );
            }
        }
    }

    let credentials = if let Some(env_creds) = plan.env_credentials {
        env_creds
    } else {
        prompt_interactive_credentials().await?
    };

    println!("Connecting to Spotify...");
    let (session, player) =
        SpotifyPlayer::connect_with_spirc(credentials, plan.cache, device_name).await?;
    println!("Logged in as: {}", session.username());
    Ok((session, player))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    spotify::init_tls_provider();
    spotify::logger::init();
    spotify::player::init_audio_fetch_params();
    let device_name = "crystalify".to_string();
    let (session, mut player) = init_session_and_player(&device_name).await?;
    let username = session.username();

    let client = SpotifyClient::new(session.clone());
    let initial_volume = player.volume();

    let mut state = AppState::new(username, device_name, initial_volume);
    state.set_audio_info(player.backend_label.clone(), player.audio_error.clone());
    state.connect_ready = player.is_connect_active();

    let (async_tx, mut async_rx) = mpsc::unbounded_channel::<AsyncResult>();
    let collection_cancel = Arc::new(AtomicU64::new(0));
    let library_cancel = Arc::new(AtomicU64::new(0));

    spawn_library_fetch(&mut state, &client, &async_tx, &library_cancel);

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

    let (input_tx, mut input_rx) = mpsc::unbounded_channel::<Event>();
    let stop_input = Arc::new(AtomicBool::new(false));
    let stop_input_thread = stop_input.clone();
    let input_handle = std::thread::spawn(move || {
        while !stop_input_thread.load(Ordering::Relaxed) {
            if let Ok(true) = event::poll(Duration::from_millis(50))
                && let Ok(ev) = event::read()
                && input_tx.send(ev).is_err()
            {
                break;
            }
        }
    });

    let mut tick_interval = tokio::time::interval(Duration::from_millis(33));
    tick_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut needs_redraw = true;

    'main_loop: loop {
        if needs_redraw {
            terminal.draw(|frame| draw_ui(frame, &state))?;
            needs_redraw = false;
        }

        tokio::select! {
            biased;
            Some(ev) = input_rx.recv() => {
                let mut should_quit = false;
                let mut current_ev = Some(ev);
                while let Some(e) = current_ev {
                    match e {
                        Event::Key(key) if key.kind == KeyEventKind::Press => {
                            needs_redraw = true;
                            if handle_key_event(
                                key,
                                &mut state,
                                &mut player,
                                &client,
                                &async_tx,
                                &collection_cancel,
                                &library_cancel,
                            ) {
                                should_quit = true;
                                break;
                            }
                        }
                        Event::Resize(_, _) => {
                            needs_redraw = true;
                        }
                        _ => {}
                    }
                    current_ev = input_rx.try_recv().ok();
                }
                if should_quit {
                    break 'main_loop;
                }
            }
            Some(event) = player.events.recv() => {
                let is_playing_event = matches!(event, librespot::playback::player::PlayerEvent::Playing { .. });
                state.handle_player_event(event);
                needs_redraw = true;
                if is_playing_event && state.pending_preload_uri.is_none() {
                    state.schedule_preload_for_next_in_queue();
                }
                if state.pending_media_fetch {
                    state.pending_media_fetch = false;
                    spawn_cover_fetch_if_needed(&state, &client, &async_tx);
                    if let Some(track) = state.current_track_metadata() {
                        spawn_lyrics_fetch_if_needed(&state, &client, &async_tx, &track);
                    }
                }
                if state.pending_next_track {
                    state.pending_next_track = false;
                    if !player.is_connect_active()
                        && let Some(next) = state.next_track()
                    {
                        let msg = format!("Auto-playing next: '{}' — {}", next.title, next.artist);
                        load_track_with_status(&player, &mut state, &client, &async_tx, &next, msg);
                    }
                }
            }
            Some(err) = player.audio_errors.recv() => {
                state.set_audio_info(player.backend_label.clone(), Some(err));
                needs_redraw = true;
            }
            Some(async_res) = async_rx.recv() => {
                needs_redraw = true;
                let mut current_res = Some(async_res);
                while let Some(res) = current_res {
                    process_async_result(
                        res,
                        &mut state,
                        &player,
                        &client,
                        &async_tx,
                        &collection_cancel,
                        &library_cancel,
                    );
                    current_res = async_rx.try_recv().ok();
                }
            }
            _ = tick_interval.tick() => {
                if state.needs_animation_tick() {
                    state.tick(33);
                    if state.audio_error.is_none()
                        && let Some(uri_str) = state.poll_ready_preload(33)
                        && let Ok(uri) = SpotifyUri::from_uri(&uri_str)
                    {
                        player.preload(uri);
                    }
                    needs_redraw = true;
                }
            }
        }
    }

    stop_input.store(true, Ordering::Relaxed);
    player.shutdown().await;
    let _ = input_handle.join();
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    Ok(())
}
