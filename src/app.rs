use crate::spotify::client::select_best_cover_url_from_audio_item;
use crate::spotify::models::{
    BrowserView, CollectionKind, CollectionView, LibraryCategory, LibraryData, SearchCategory,
    SearchResults, TrackLyrics, TrackMetadata,
};
use image::DynamicImage;
use librespot::metadata::audio::{AudioItem, UniqueFields};
use librespot::playback::player::PlayerEvent;
use ratatui::layout::Size;
use ratatui_image::{
    FilterType, Resize,
    picker::Picker,
    protocol::Protocol,
};
use std::collections::HashMap;

pub const COVER_WIDTH_CELLS: u16 = 10;
pub const COVER_HEIGHT_CELLS: u16 = 5;
pub const PLAYBACK_LOADING_TIMEOUT_MS: u32 = 10_000;
pub const PRELOAD_DEBOUNCE_MS: u32 = 180;
const GAPLESS_PRELOAD_GUARD_MS: u32 = 12_000;

pub fn track_metadata_from_audio_item(audio_item: &AudioItem) -> TrackMetadata {
    let uri = if audio_item.uri.trim().is_empty() {
        audio_item.track_id.to_uri()
    } else {
        audio_item.uri.clone()
    };
    let title = if audio_item.name.trim().is_empty() {
        "Unknown Track".to_string()
    } else {
        audio_item.name.clone()
    };
    let (artist, album) = match &audio_item.unique_fields {
        UniqueFields::Track {
            artists, album, ..
        } => {
            let joined = artists
                .0
                .iter()
                .map(|a| a.name.as_str())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(", ");
            let artist_out = if joined.is_empty() {
                "Unknown Artist".to_string()
            } else {
                joined
            };
            let album_out = if album.trim().is_empty() {
                "Unknown Album".to_string()
            } else {
                album.clone()
            };
            (artist_out, album_out)
        }
        UniqueFields::Episode { show_name, .. } => {
            let show = if show_name.trim().is_empty() {
                "Podcast".to_string()
            } else {
                show_name.clone()
            };
            (show, "Podcast Episode".to_string())
        }
        UniqueFields::Local {
            artists, album, ..
        } => {
            let artist_out = artists
                .clone()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| "Local Artist".to_string());
            let album_out = album
                .clone()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| "Local Files".to_string());
            (artist_out, album_out)
        }
    };
    let cover_url = select_best_cover_url_from_audio_item(&audio_item.covers);

    TrackMetadata {
        uri,
        title,
        artist,
        album,
        duration_ms: audio_item.duration_ms,
        cover_url,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum PlaybackStatus {
    #[default]
    Idle,
    Loading,
    Playing,
    Paused,
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InputMode {
    #[default]
    Normal,
    EditingSearch,
}

pub struct AppState {
    pub track_uri: String,
    pub track_title: String,
    pub track_artist: String,
    pub track_album: String,
    pub track_cover_url: Option<String>,
    pub duration_ms: u32,
    pub position_ms: u32,
    pub is_playing: bool,
    pub playback_status: PlaybackStatus,
    pub loading_elapsed_ms: u32,
    pub audio_backend_label: String,
    pub audio_error: Option<String>,
    pub volume: u16,
    pub username: String,
    pub device_name: String,

    pub picker: Picker,
    pub cover_image: Option<Protocol>,
    pub cover_cache: HashMap<String, DynamicImage>,

    pub lyrics: TrackLyrics,
    pub lyrics_cache: HashMap<String, TrackLyrics>,
    pub lyrics_manual_index: usize,
    pub lyrics_gradient_phase: f32,

    pub input_mode: InputMode,
    pub browser_view: BrowserView,
    pub search_query: String,
    pub is_loading: bool,
    pub status_message: Option<String>,
    pub search_category: SearchCategory,
    pub search_results: SearchResults,
    pub library_category: LibraryCategory,
    pub library: LibraryData,
    pub library_loading: bool,
    pub selected_index: usize,
    pub opened_collection: Option<CollectionView>,
    pub collection_selected_index: usize,

    pub queue: Vec<TrackMetadata>,
    pub queue_index: usize,
    pub pending_next_track: bool,

    pub connect_ready: bool,
    pub connect_active: bool,
    pub connect_client: Option<String>,
    pub connect_queue_position: Option<(usize, usize)>,
    pub pending_media_fetch: bool,

    pub pending_preload_uri: Option<String>,
    pub preload_debounce_ms: u32,
    pub last_preloaded_uri: Option<String>,
}

impl AppState {
    pub fn new(username: String, device_name: String, volume: u16) -> Self {
        Self {
            track_uri: String::new(),
            track_title: "No Track Playing".to_string(),
            track_artist: "Press [/] to search or [l] to open your library".to_string(),
            track_album: "—".to_string(),
            track_cover_url: None,
            duration_ms: 0,
            position_ms: 0,
            is_playing: false,
            playback_status: PlaybackStatus::Idle,
            loading_elapsed_ms: 0,
            audio_backend_label: "Unknown".to_string(),
            audio_error: None,
            volume,
            username,
            device_name,
            picker: Picker::halfblocks(),
            cover_image: None,
            cover_cache: HashMap::new(),
            lyrics: TrackLyrics::Empty,
            lyrics_cache: HashMap::new(),
            lyrics_manual_index: 0,
            lyrics_gradient_phase: 0.0,
            input_mode: InputMode::Normal,
            browser_view: BrowserView::Search,
            search_query: String::new(),
            is_loading: false,
            status_message: Some(
                "Press [/] to search songs, albums, artists & playlists or [l] for Library"
                    .to_string(),
            ),
            search_category: SearchCategory::Tracks,
            search_results: SearchResults::default(),
            library_category: LibraryCategory::Playlists,
            library: LibraryData::default(),
            library_loading: false,
            selected_index: 0,
            opened_collection: None,
            collection_selected_index: 0,
            queue: Vec::new(),
            queue_index: 0,
            pending_next_track: false,
            connect_ready: false,
            connect_active: false,
            connect_client: None,
            connect_queue_position: None,
            pending_media_fetch: false,
            pending_preload_uri: None,
            preload_debounce_ms: 0,
            last_preloaded_uri: None,
        }
    }

    pub fn set_picker(&mut self, picker: Picker) {
        self.picker = picker;
        self.refresh_cover_protocol();
    }

    pub fn set_audio_info(&mut self, backend_label: String, audio_error: Option<String>) {
        self.audio_backend_label = backend_label;
        self.audio_error = audio_error.clone();
        if let Some(err) = audio_error {
            self.set_playback_failed(err);
        }
    }

    pub fn set_playback_failed(&mut self, message: impl Into<String>) {
        let msg = message.into();
        self.is_playing = false;
        self.loading_elapsed_ms = 0;
        self.playback_status = PlaybackStatus::Failed(msg.clone());
        self.status_message = Some(msg);
    }

    pub fn current_track_metadata(&self) -> Option<TrackMetadata> {
        if self.track_uri.is_empty() {
            return None;
        }
        Some(TrackMetadata {
            uri: self.track_uri.clone(),
            title: self.track_title.clone(),
            artist: self.track_artist.clone(),
            album: self.track_album.clone(),
            duration_ms: self.duration_ms,
            cover_url: self.track_cover_url.clone(),
        })
    }

    pub fn set_track(&mut self, meta: TrackMetadata) {
        if self.last_preloaded_uri.as_deref() == Some(meta.uri.as_str()) {
            self.last_preloaded_uri = None;
        }
        if self.pending_preload_uri.as_deref() == Some(meta.uri.as_str()) {
            self.pending_preload_uri = None;
            self.preload_debounce_ms = 0;
        }
        self.track_uri = meta.uri;
        self.track_title = meta.title;
        self.track_artist = meta.artist;
        self.track_album = meta.album;
        self.track_cover_url = meta.cover_url;
        self.duration_ms = meta.duration_ms;
        self.position_ms = 0;
        self.is_playing = false;
        self.loading_elapsed_ms = 0;
        if let Some(err) = self.audio_error.clone() {
            self.set_playback_failed(err);
        } else {
            self.playback_status = PlaybackStatus::Loading;
        }
        self.lyrics_manual_index = 0;
        if let Some(cached) = self.lyrics_cache.get(&self.track_uri).cloned() {
            self.lyrics = cached;
        } else {
            self.lyrics = TrackLyrics::Loading;
        }
        self.refresh_cover_protocol();
    }

    pub fn cache_and_apply_lyrics(&mut self, uri: String, lyrics: TrackLyrics) {
        self.lyrics_cache.insert(uri.clone(), lyrics.clone());
        if self.track_uri == uri {
            self.lyrics_manual_index = 0;
            self.lyrics = lyrics;
        }
    }

    pub fn active_synced_lyric_index(&self) -> Option<usize> {
        let TrackLyrics::Synced(ref lines) = self.lyrics else {
            return None;
        };
        if lines.is_empty() {
            return None;
        }
        let idx = lines.partition_point(|l| l.time_ms <= self.position_ms);
        if idx == 0 {
            None
        } else {
            Some(idx - 1)
        }
    }

    pub fn current_lyric_display(&self) -> (String, bool) {
        match &self.lyrics {
            TrackLyrics::Synced(lines) => {
                if lines.is_empty() {
                    ("No lyrics available".to_string(), false)
                } else if let Some(idx) = self.active_synced_lyric_index() {
                    (lines[idx].text.clone(), true)
                } else {
                    ("♪".to_string(), true)
                }
            }
            TrackLyrics::Unsynced(lines) => {
                if lines.is_empty() {
                    ("No lyrics available".to_string(), false)
                } else {
                    let idx = self.lyrics_manual_index.min(lines.len() - 1);
                    (lines[idx].clone(), true)
                }
            }
            TrackLyrics::Instrumental => ("♪ Instrumental ♪".to_string(), true),
            TrackLyrics::Loading => ("Loading lyrics...".to_string(), false),
            TrackLyrics::Unavailable => ("No lyrics available".to_string(), false),
            TrackLyrics::Empty => (String::new(), false),
        }
    }

    pub fn scroll_lyrics_up(&mut self) {
        if let TrackLyrics::Unsynced(ref lines) = self.lyrics
            && !lines.is_empty()
        {
            let clamped = self.lyrics_manual_index.min(lines.len() - 1);
            self.lyrics_manual_index = clamped.saturating_sub(1);
        }
    }

    pub fn scroll_lyrics_down(&mut self) {
        if let TrackLyrics::Unsynced(ref lines) = self.lyrics
            && !lines.is_empty()
        {
            self.lyrics_manual_index = (self.lyrics_manual_index + 1).min(lines.len() - 1);
        }
    }

    pub fn cache_and_apply_cover(&mut self, url: String, img: DynamicImage) {
        self.cover_cache.insert(url.clone(), img);
        if self.track_cover_url.as_deref() == Some(url.as_str()) {
            self.refresh_cover_protocol();
        }
    }

    pub fn refresh_cover_protocol(&mut self) {
        let Some(ref url) = self.track_cover_url else {
            self.cover_image = None;
            return;
        };
        let Some(dyn_img) = self.cover_cache.get(url) else {
            self.cover_image = None;
            return;
        };
        let target_size = Size::new(COVER_WIDTH_CELLS, COVER_HEIGHT_CELLS);
        self.cover_image = self
            .picker
            .new_protocol(
                dyn_img.clone(),
                target_size,
                Resize::Fit(Some(FilterType::Triangle)),
            )
            .ok();
    }

    pub fn current_list_len(&self) -> usize {
        if let Some(ref col) = self.opened_collection {
            col.tracks.len()
        } else {
            match self.browser_view {
                BrowserView::Search => match self.search_category {
                    SearchCategory::Tracks => self.search_results.tracks.len(),
                    SearchCategory::Albums => self.search_results.albums.len(),
                    SearchCategory::Artists => self.search_results.artists.len(),
                    SearchCategory::Playlists => self.search_results.playlists.len(),
                },
                BrowserView::Library => match self.library_category {
                    LibraryCategory::Playlists => self.library.playlists.len(),
                    LibraryCategory::LikedSongs => self.library.liked_tracks.len(),
                    LibraryCategory::SavedAlbums => self.library.saved_albums.len(),
                },
            }
        }
    }

    pub fn active_selected_index(&self) -> usize {
        if self.opened_collection.is_some() {
            self.collection_selected_index
        } else {
            self.selected_index
        }
    }

    pub fn move_selection_up(&mut self) {
        let len = self.current_list_len();
        if len == 0 {
            return;
        }
        if self.opened_collection.is_some() {
            if self.collection_selected_index > 0 {
                self.collection_selected_index -= 1;
            } else {
                self.collection_selected_index = len - 1;
            }
        } else if self.selected_index > 0 {
            self.selected_index -= 1;
        } else {
            self.selected_index = len - 1;
        }
    }

    pub fn move_selection_down(&mut self) {
        let len = self.current_list_len();
        if len == 0 {
            return;
        }
        if self.opened_collection.is_some() {
            self.collection_selected_index = (self.collection_selected_index + 1) % len;
        } else {
            self.selected_index = (self.selected_index + 1) % len;
        }
    }

    pub fn move_selection_page_up(&mut self, step: usize) {
        let len = self.current_list_len();
        if len == 0 {
            return;
        }
        let step = step.max(1);
        if self.opened_collection.is_some() {
            self.collection_selected_index =
                self.collection_selected_index.saturating_sub(step).min(len - 1);
        } else {
            self.selected_index = self.selected_index.saturating_sub(step).min(len - 1);
        }
    }

    pub fn move_selection_page_down(&mut self, step: usize) {
        let len = self.current_list_len();
        if len == 0 {
            return;
        }
        let step = step.max(1);
        if self.opened_collection.is_some() {
            self.collection_selected_index = (self.collection_selected_index + step).min(len - 1);
        } else {
            self.selected_index = (self.selected_index + step).min(len - 1);
        }
    }

    pub fn move_selection_top(&mut self) {
        if self.opened_collection.is_some() {
            self.collection_selected_index = 0;
        } else {
            self.selected_index = 0;
        }
    }

    pub fn move_selection_bottom(&mut self) {
        let len = self.current_list_len();
        if len == 0 {
            return;
        }
        if self.opened_collection.is_some() {
            self.collection_selected_index = len - 1;
        } else {
            self.selected_index = len - 1;
        }
    }

    pub fn switch_category_next(&mut self) {
        self.opened_collection = None;
        match self.browser_view {
            BrowserView::Search => {
                self.search_category = self.search_category.next();
            }
            BrowserView::Library => {
                self.library_category = self.library_category.next();
            }
        }
        self.selected_index = 0;
    }

    pub fn switch_category_prev(&mut self) {
        self.opened_collection = None;
        match self.browser_view {
            BrowserView::Search => {
                self.search_category = self.search_category.prev();
            }
            BrowserView::Library => {
                self.library_category = self.library_category.prev();
            }
        }
        self.selected_index = 0;
    }

    pub fn set_category(&mut self, category: SearchCategory) {
        self.opened_collection = None;
        self.browser_view = BrowserView::Search;
        self.search_category = category;
        self.selected_index = 0;
    }

    pub fn set_library_category(&mut self, category: LibraryCategory) {
        self.opened_collection = None;
        self.browser_view = BrowserView::Library;
        self.library_category = category;
        self.selected_index = 0;
    }

    pub fn toggle_browser_view(&mut self) {
        self.opened_collection = None;
        self.selected_index = 0;
        self.browser_view = match self.browser_view {
            BrowserView::Search => BrowserView::Library,
            BrowserView::Library => BrowserView::Search,
        };
    }

    pub fn set_browser_view(&mut self, view: BrowserView) {
        if self.browser_view != view {
            self.opened_collection = None;
            self.selected_index = 0;
            self.browser_view = view;
        }
    }

    #[allow(dead_code)]
    pub fn apply_library(&mut self, library: LibraryData) {
        self.library = library;
        self.library_loading = false;
        let len = self.current_list_len();
        if self.selected_index >= len {
            self.selected_index = 0;
        }
    }

    pub fn apply_library_playlists(&mut self, playlists: Vec<crate::spotify::models::PlaylistItem>) {
        self.library.playlists = playlists;
        self.library.loaded = true;
        if self.browser_view == BrowserView::Library
            && self.library_category == LibraryCategory::Playlists
        {
            let len = self.current_list_len();
            if self.selected_index >= len {
                self.selected_index = 0;
            }
        }
    }

    pub fn apply_library_albums(&mut self, albums: Vec<crate::spotify::models::AlbumItem>) {
        self.library.saved_albums = albums;
        self.library.loaded = true;
        if self.browser_view == BrowserView::Library
            && self.library_category == LibraryCategory::SavedAlbums
        {
            let len = self.current_list_len();
            if self.selected_index >= len {
                self.selected_index = 0;
            }
        }
    }

    pub fn apply_library_liked_batch(
        &mut self,
        tracks: Vec<TrackMetadata>,
        total_expected: Option<usize>,
        reset: bool,
        is_done: bool,
    ) {
        if reset {
            self.library.liked_tracks = tracks;
        } else {
            self.library.liked_tracks.extend(tracks);
        }
        if total_expected.is_some() {
            self.library.liked_total_expected = total_expected;
        }
        self.library.liked_loading_more = !is_done;
        self.library.loaded = true;
        if is_done {
            self.library_loading = false;
        }
        if self.browser_view == BrowserView::Library
            && self.library_category == LibraryCategory::LikedSongs
        {
            let len = self.current_list_len();
            if self.selected_index >= len && len > 0 {
                self.selected_index = len - 1;
            }
        }
    }

    #[allow(dead_code)]
    pub fn open_collection(
        &mut self,
        kind: CollectionKind,
        title: String,
        subtitle: String,
        tracks: Vec<TrackMetadata>,
    ) {
        self.open_collection_with_uri(kind, String::new(), title, subtitle, tracks);
    }

    pub fn open_collection_with_uri(
        &mut self,
        kind: CollectionKind,
        uri: String,
        title: String,
        subtitle: String,
        tracks: Vec<TrackMetadata>,
    ) {
        let total = tracks.len();
        self.open_streaming_collection(CollectionView {
            kind,
            uri,
            title,
            subtitle,
            tracks,
            total_tracks: Some(total),
            is_loading_more: false,
        });
    }

    pub fn open_streaming_collection(&mut self, view: CollectionView) {
        self.opened_collection = Some(view);
        self.collection_selected_index = 0;
    }

    pub fn append_collection_tracks(
        &mut self,
        tracks: Vec<TrackMetadata>,
        total_expected: Option<usize>,
        is_done: bool,
    ) {
        if let Some(ref mut col) = self.opened_collection {
            col.tracks.extend(tracks);
            if total_expected.is_some() {
                col.total_tracks = total_expected;
            } else if is_done {
                col.total_tracks = Some(col.tracks.len());
            }
            col.is_loading_more = !is_done;
        }
    }

    pub fn close_collection(&mut self) -> bool {
        if self.opened_collection.is_some() {
            self.opened_collection = None;
            self.collection_selected_index = 0;
            true
        } else {
            false
        }
    }

    pub fn start_playback_queue(
        &mut self,
        tracks: Vec<TrackMetadata>,
        start_index: usize,
    ) -> Option<TrackMetadata> {
        if tracks.is_empty() || start_index >= tracks.len() {
            return None;
        }
        let track = tracks[start_index].clone();
        self.queue = tracks;
        self.queue_index = start_index;
        self.set_track(track.clone());
        Some(track)
    }

    pub fn next_track(&mut self) -> Option<TrackMetadata> {
        if self.queue.is_empty() || self.queue_index + 1 >= self.queue.len() {
            return None;
        }
        self.queue_index += 1;
        let track = self.queue[self.queue_index].clone();
        self.set_track(track.clone());
        Some(track)
    }

    pub fn prev_track(&mut self) -> Option<TrackMetadata> {
        if self.queue.is_empty() || self.queue_index == 0 {
            return None;
        }
        self.queue_index -= 1;
        let track = self.queue[self.queue_index].clone();
        self.set_track(track.clone());
        Some(track)
    }

    pub fn highlighted_track_uri(&self) -> Option<&str> {
        if let Some(ref col) = self.opened_collection {
            col.tracks
                .get(self.collection_selected_index)
                .map(|t| t.uri.as_str())
                .filter(|u| !u.is_empty())
        } else {
            match self.browser_view {
                BrowserView::Search if self.search_category == SearchCategory::Tracks => self
                    .search_results
                    .tracks
                    .get(self.selected_index)
                    .map(|t| t.uri.as_str())
                    .filter(|u| !u.is_empty()),
                BrowserView::Library if self.library_category == LibraryCategory::LikedSongs => {
                    self.library
                        .liked_tracks
                        .get(self.selected_index)
                        .map(|t| t.uri.as_str())
                        .filter(|u| !u.is_empty())
                }
                _ => None,
            }
        }
    }

    pub fn schedule_preload_for_selection(&mut self) {
        let Some(uri) = self.highlighted_track_uri().map(str::to_string) else {
            self.pending_preload_uri = None;
            self.preload_debounce_ms = 0;
            return;
        };
        if uri == self.track_uri || self.last_preloaded_uri.as_deref() == Some(uri.as_str()) {
            self.pending_preload_uri = None;
            self.preload_debounce_ms = 0;
            return;
        }
        if self.pending_preload_uri.as_deref() != Some(uri.as_str()) {
            self.pending_preload_uri = Some(uri);
            self.preload_debounce_ms = 0;
        }
    }

    pub fn schedule_preload_for_next_in_queue(&mut self) {
        if self.pending_preload_uri.is_some() {
            return;
        }
        let Some(next) = self.queue.get(self.queue_index + 1) else {
            return;
        };
        if next.uri.is_empty()
            || next.uri == self.track_uri
            || self.last_preloaded_uri.as_deref() == Some(next.uri.as_str())
        {
            return;
        }
        self.pending_preload_uri = Some(next.uri.clone());
        self.preload_debounce_ms = 0;
    }

    pub fn poll_ready_preload(&mut self, delta_ms: u32) -> Option<String> {
        let target = self.pending_preload_uri.clone()?;
        if target == self.track_uri || self.last_preloaded_uri.as_deref() == Some(target.as_str()) {
            self.pending_preload_uri = None;
            self.preload_debounce_ms = 0;
            return None;
        }
        if matches!(self.playback_status, PlaybackStatus::Playing)
            && self.duration_ms > 0
            && self.duration_ms.saturating_sub(self.position_ms) < GAPLESS_PRELOAD_GUARD_MS
        {
            return None;
        }
        self.preload_debounce_ms = self.preload_debounce_ms.saturating_add(delta_ms);
        if self.preload_debounce_ms >= PRELOAD_DEBOUNCE_MS {
            self.pending_preload_uri = None;
            self.preload_debounce_ms = 0;
            self.last_preloaded_uri = Some(target.clone());
            Some(target)
        } else {
            None
        }
    }

    pub fn handle_player_event(&mut self, event: PlayerEvent) {
        match event {
            PlayerEvent::TrackChanged { audio_item } => {
                let meta = track_metadata_from_audio_item(&audio_item);
                let is_new_track = self.track_uri != meta.uri;
                if self.last_preloaded_uri.as_deref() == Some(meta.uri.as_str()) {
                    self.last_preloaded_uri = None;
                }
                if self.pending_preload_uri.as_deref() == Some(meta.uri.as_str()) {
                    self.pending_preload_uri = None;
                    self.preload_debounce_ms = 0;
                }
                self.track_uri = meta.uri.clone();
                if is_new_track || self.track_title == "No Track Playing" || !meta.title.is_empty()
                {
                    self.track_title = meta.title;
                }
                if is_new_track || !meta.artist.is_empty() {
                    self.track_artist = meta.artist;
                }
                if is_new_track || !meta.album.is_empty() {
                    self.track_album = meta.album;
                }
                if meta.duration_ms > 0 || is_new_track {
                    self.duration_ms = meta.duration_ms;
                }
                if meta.cover_url.is_some() || is_new_track {
                    self.track_cover_url = meta.cover_url;
                }
                if is_new_track {
                    self.position_ms = 0;
                    self.lyrics_manual_index = 0;
                    if let Some(cached) = self.lyrics_cache.get(&self.track_uri).cloned() {
                        self.lyrics = cached;
                    } else {
                        self.lyrics = TrackLyrics::Loading;
                    }
                    if let Some(pos) = self.queue.iter().position(|t| t.uri == self.track_uri) {
                        self.queue_index = pos;
                    }
                } else if matches!(self.lyrics, TrackLyrics::Empty) {
                    if let Some(cached) = self.lyrics_cache.get(&self.track_uri).cloned() {
                        self.lyrics = cached;
                    } else {
                        self.lyrics = TrackLyrics::Loading;
                    }
                }
                self.refresh_cover_protocol();
                self.pending_media_fetch = true;
            }
            PlayerEvent::Loading { .. } => {
                self.is_playing = false;
                self.loading_elapsed_ms = 0;
                if self.audio_error.is_none() {
                    self.playback_status = PlaybackStatus::Loading;
                }
            }
            PlayerEvent::Playing { position_ms, .. } => {
                self.position_ms = position_ms;
                self.is_playing = true;
                self.loading_elapsed_ms = 0;
                self.playback_status = PlaybackStatus::Playing;
            }
            PlayerEvent::Paused { position_ms, .. } => {
                self.position_ms = position_ms;
                self.is_playing = false;
                self.loading_elapsed_ms = 0;
                if !matches!(self.playback_status, PlaybackStatus::Failed(_)) {
                    self.playback_status = PlaybackStatus::Paused;
                }
            }
            PlayerEvent::Stopped { .. } => {
                self.is_playing = false;
                self.loading_elapsed_ms = 0;
                if !matches!(self.playback_status, PlaybackStatus::Failed(_)) {
                    self.playback_status = PlaybackStatus::Idle;
                }
            }
            PlayerEvent::EndOfTrack { .. } => {
                self.is_playing = false;
                self.loading_elapsed_ms = 0;
                if !matches!(self.playback_status, PlaybackStatus::Failed(_)) {
                    self.playback_status = PlaybackStatus::Idle;
                }
                self.pending_next_track = true;
            }
            PlayerEvent::Unavailable { .. } => {
                let msg = match crate::spotify::logger::take_last_error() {
                    Some(detail) if !detail.trim().is_empty() => {
                        format!("Failed to load stream: {detail}")
                    }
                    _ => "Track unavailable or failed to load stream".to_string(),
                };
                self.set_playback_failed(msg);
            }
            PlayerEvent::PositionChanged { position_ms, .. }
            | PlayerEvent::PositionCorrection { position_ms, .. }
            | PlayerEvent::Seeked { position_ms, .. } => {
                self.position_ms = position_ms;
            }
            PlayerEvent::VolumeChanged { volume } => {
                self.volume = volume;
            }
            PlayerEvent::SessionConnected { .. } => {
                self.connect_ready = true;
                self.connect_active = true;
            }
            PlayerEvent::SessionDisconnected { .. } => {
                self.connect_active = false;
                self.connect_client = None;
                self.connect_queue_position = None;
            }
            PlayerEvent::SessionClientChanged { client_name, .. } => {
                let trimmed = client_name.trim();
                self.connect_client = if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                };
            }
            PlayerEvent::SetQueue {
                current_track,
                next_tracks,
                prev_tracks,
                ..
            } => {
                let current_count = usize::from(current_track.is_some());
                let total = prev_tracks.len() + current_count + next_tracks.len();
                if total > 0 {
                    let current_pos = prev_tracks.len() + current_count;
                    self.connect_queue_position = Some((current_pos.max(1), total));
                } else {
                    self.connect_queue_position = None;
                }
            }
            _ => {}
        }
    }

    pub fn needs_animation_tick(&self) -> bool {
        matches!(
            self.playback_status,
            PlaybackStatus::Playing | PlaybackStatus::Loading
        ) || matches!(
            self.lyrics,
            TrackLyrics::Synced(_) | TrackLyrics::Unsynced(_) | TrackLyrics::Instrumental
        ) || self.pending_preload_uri.is_some()
    }

    pub fn tick(&mut self, delta_ms: u32) {
        self.lyrics_gradient_phase =
            (self.lyrics_gradient_phase + delta_ms as f32 * 0.00035).rem_euclid(1.0);
        match self.playback_status {
            PlaybackStatus::Playing => {
                if self.position_ms < self.duration_ms {
                    self.position_ms = (self.position_ms + delta_ms).min(self.duration_ms);
                }
            }
            PlaybackStatus::Loading => {
                self.loading_elapsed_ms = self.loading_elapsed_ms.saturating_add(delta_ms);
                if self.loading_elapsed_ms >= PLAYBACK_LOADING_TIMEOUT_MS {
                    self.set_playback_failed("Playback did not start: check audio device or network");
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spotify::models::AlbumItem;
    use librespot::core::spotify_uri::SpotifyUri;

    fn sample_tracks() -> Vec<TrackMetadata> {
        vec![
            TrackMetadata {
                uri: "spotify:track:1".to_string(),
                title: "Song One".to_string(),
                artist: "Artist A".to_string(),
                album: "Album X".to_string(),
                duration_ms: 180_000,
                cover_url: Some("https://i.scdn.co/image/ab67616d00004851111111111111111111111111".to_string()),
            },
            TrackMetadata {
                uri: "spotify:track:2".to_string(),
                title: "Song Two".to_string(),
                artist: "Artist B".to_string(),
                album: "Album Y".to_string(),
                duration_ms: 200_000,
                cover_url: None,
            },
            TrackMetadata {
                uri: "spotify:track:3".to_string(),
                title: "Song Three".to_string(),
                artist: "Artist C".to_string(),
                album: "Album Z".to_string(),
                duration_ms: 220_000,
                cover_url: None,
            },
        ]
    }

    #[test]
    fn test_category_switching() {
        let mut state = AppState::new("user".to_string(), "crystalify".to_string(), 30000);
        assert_eq!(state.search_category, SearchCategory::Tracks);

        state.switch_category_next();
        assert_eq!(state.search_category, SearchCategory::Albums);
        state.switch_category_next();
        assert_eq!(state.search_category, SearchCategory::Artists);
        state.switch_category_next();
        assert_eq!(state.search_category, SearchCategory::Playlists);
        state.switch_category_next();
        assert_eq!(state.search_category, SearchCategory::Tracks);

        state.switch_category_prev();
        assert_eq!(state.search_category, SearchCategory::Playlists);
    }

    #[test]
    fn test_selection_navigation_in_search_and_collection() {
        let mut state = AppState::new("user".to_string(), "crystalify".to_string(), 30000);
        state.search_results.tracks = sample_tracks();
        assert_eq!(state.current_list_len(), 3);
        assert_eq!(state.selected_index, 0);

        state.move_selection_down();
        assert_eq!(state.selected_index, 1);
        state.move_selection_down();
        assert_eq!(state.selected_index, 2);
        state.move_selection_down();
        assert_eq!(state.selected_index, 0);
        state.move_selection_up();
        assert_eq!(state.selected_index, 2);

        state.open_collection(
            CollectionKind::Album,
            "My Album".to_string(),
            "Artist".to_string(),
            sample_tracks()[..2].to_vec(),
        );
        assert_eq!(state.current_list_len(), 2);
        assert_eq!(state.active_selected_index(), 0);
        state.move_selection_down();
        assert_eq!(state.collection_selected_index, 1);
        assert_eq!(state.selected_index, 2);

        assert!(state.close_collection());
        assert!(state.opened_collection.is_none());
        assert_eq!(state.active_selected_index(), 2);
    }

    #[test]
    fn test_playback_queue_next_prev_and_end_of_track() {
        let mut state = AppState::new("user".to_string(), "crystalify".to_string(), 30000);
        let tracks = sample_tracks();

        let started = state
            .start_playback_queue(tracks.clone(), 1)
            .expect("Should start track at index 1");
        assert_eq!(started.title, "Song Two");
        assert_eq!(state.track_title, "Song Two");
        assert_eq!(state.queue_index, 1);
        assert!(!state.is_playing);
        assert_eq!(state.playback_status, PlaybackStatus::Loading);

        let dummy_uri = SpotifyUri::from_uri("spotify:track:7tvuLLroI0n6uYBWuFig5d").unwrap();
        state.handle_player_event(PlayerEvent::Playing {
            play_request_id: 1,
            track_id: dummy_uri.clone(),
            position_ms: 0,
        });
        assert!(state.is_playing);
        assert_eq!(state.playback_status, PlaybackStatus::Playing);

        let next = state.next_track().expect("Should advance to index 2");
        assert_eq!(next.title, "Song Three");
        assert_eq!(state.queue_index, 2);
        assert_eq!(state.playback_status, PlaybackStatus::Loading);
        assert!(state.next_track().is_none());

        let prev = state.prev_track().expect("Should go back to index 1");
        assert_eq!(prev.title, "Song Two");
        assert_eq!(state.queue_index, 1);
        assert_eq!(state.playback_status, PlaybackStatus::Loading);

        state.handle_player_event(PlayerEvent::EndOfTrack {
            play_request_id: 1,
            track_id: dummy_uri,
        });
        assert!(!state.is_playing);
        assert!(state.pending_next_track);

        state.set_category(SearchCategory::Albums);
        state.search_results.albums.push(AlbumItem {
            uri: "spotify:album:1".to_string(),
            id: "1".to_string(),
            name: "Album 1".to_string(),
            artist: "Art".to_string(),
            total_tracks: 10,
            year: Some(2001),
        });
        assert_eq!(state.current_list_len(), 1);
    }

    #[test]
    fn test_ui_rendering_all_states_and_error_messages() {
        use crate::spotify::models::{ArtistItem, PlaylistItem};
        use crate::ui::draw_ui;
        use ratatui::{Terminal, backend::TestBackend};

        let backend = TestBackend::new(120, 35);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = AppState::new("tester".to_string(), "crystalify".to_string(), 32768);

        terminal.draw(|f| draw_ui(f, &state)).unwrap();

        state.input_mode = InputMode::EditingSearch;
        state.search_query = "Океан Ельзи & Tool".to_string();
        terminal.draw(|f| draw_ui(f, &state)).unwrap();

        state.input_mode = InputMode::Normal;
        state.search_results.tracks = sample_tracks();
        state.search_results.albums.push(AlbumItem {
            uri: "spotify:album:a1".to_string(),
            id: "a1".to_string(),
            name: "Lateralus".to_string(),
            artist: "TOOL".to_string(),
            total_tracks: 13,
            year: Some(2001),
        });
        state.search_results.artists.push(ArtistItem {
            uri: "spotify:artist:ar1".to_string(),
            id: "ar1".to_string(),
            name: "TOOL".to_string(),
        });
        state.search_results.playlists.push(PlaylistItem {
            uri: "spotify:playlist:pl1".to_string(),
            id: "pl1".to_string(),
            name: "Tool Best of".to_string(),
            owner: "Spotify".to_string(),
            total_tracks: 35,
        });

        for cat in [
            SearchCategory::Tracks,
            SearchCategory::Albums,
            SearchCategory::Artists,
            SearchCategory::Playlists,
        ] {
            state.set_category(cat);
            terminal.draw(|f| draw_ui(f, &state)).unwrap();
        }

        state.open_collection(
            CollectionKind::Album,
            "Lateralus".to_string(),
            "TOOL".to_string(),
            sample_tracks(),
        );
        state.status_message = Some("Search failed: Custom test error".to_string());
        terminal.draw(|f| draw_ui(f, &state)).unwrap();
    }

    #[test]
    fn test_ui_title_and_small_switch_palette_buffer_cells() {
        use crate::ui::draw_ui;
        use crate::ui::theme::{MUTED, PRIMARY, TEXT};
        use ratatui::{Terminal, backend::TestBackend, style::Color};

        let primary_rgb = Color::Rgb(236, 153, 255);
        let text_rgb = Color::Rgb(221, 230, 246);
        let muted_rgb = Color::Rgb(87, 124, 171);

        assert_eq!(PRIMARY, primary_rgb);
        assert_eq!(TEXT, text_rgb);
        assert_eq!(MUTED, muted_rgb);

        let backend = TestBackend::new(120, 35);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = AppState::new("tester".to_string(), "crystalify".to_string(), 32768);
        state.search_results.tracks = sample_tracks();

        terminal.draw(|f| draw_ui(f, &state)).unwrap();
        let buf = terminal.backend().buffer();

        let mut full_text = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                full_text.push_str(buf[(x, y)].symbol());
            }
            full_text.push('\n');
        }

        assert!(
            !full_text.contains("Spotify Terminal Player"),
            "Buffer must not contain 'Spotify Terminal Player'"
        );
        assert!(
            full_text.contains(" crystalify "),
            "Buffer must contain ' crystalify ' title"
        );

        assert_eq!(buf[(0, 0)].fg, primary_rgb);

        let mut found_paused_bg = false;
        let mut found_active_tab_bg = false;
        let mut found_inactive_tab_fg = false;
        for y in 0..buf.area.height {
            let row_str: String = (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol())
                .collect();
            if let Some(col) = row_str.find("[PAUSED]") {
                assert_eq!(buf[(col as u16, y)].bg, muted_rgb);
                found_paused_bg = true;
            }
            if let Some(col) = row_str.find("[1] Tracks") {
                assert_eq!(buf[(col as u16, y)].bg, primary_rgb);
                found_active_tab_bg = true;
            }
            if let Some(col) = row_str.find("[2] Albums") {
                assert_eq!(buf[(col as u16, y)].fg, muted_rgb);
                found_inactive_tab_fg = true;
            }
        }
        assert!(found_paused_bg, "Should find [PAUSED] badge with MUTED bg");
        assert!(
            found_active_tab_bg,
            "Should find active tab [1] Tracks with PRIMARY bg"
        );
        assert!(
            found_inactive_tab_fg,
            "Should find inactive tab [2] Albums with MUTED fg"
        );

        state.start_playback_queue(sample_tracks(), 0);
        let dummy_uri = SpotifyUri::from_uri("spotify:track:7tvuLLroI0n6uYBWuFig5d").unwrap();
        state.handle_player_event(PlayerEvent::Playing {
            play_request_id: 1,
            track_id: dummy_uri,
            position_ms: 90_000,
        });
        terminal.draw(|f| draw_ui(f, &state)).unwrap();
        let buf = terminal.backend().buffer();

        let mut found_playing_bg = false;
        let mut found_title_label_muted = false;
        let mut found_song_title_text = false;
        let mut found_gauge_primary = false;
        let mut found_gauge_muted = false;

        for y in 0..buf.area.height {
            let row_str: String = (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol())
                .collect();
            if let Some(col) = row_str.find("[PLAYING]") {
                assert_eq!(buf[(col as u16, y)].bg, primary_rgb);
                found_playing_bg = true;
            }
            if let Some(col) = row_str.find("Title:") {
                assert_eq!(buf[(col as u16, y)].fg, muted_rgb);
                found_title_label_muted = true;
            }
            if let Some(col) = row_str.find("Song One")
                && col < 40
            {
                assert_eq!(buf[(col as u16, y)].fg, text_rgb);
                found_song_title_text = true;
            }
            for x in 1..40 {
                let cell = &buf[(x, y)];
                if cell.fg == primary_rgb && cell.bg == muted_rgb {
                    found_gauge_primary = true;
                    found_gauge_muted = true;
                }
            }
        }

        assert!(
            found_playing_bg,
            "Should find [PLAYING] badge with PRIMARY bg"
        );
        assert!(
            found_title_label_muted,
            "Should find 'Title:' label with MUTED fg"
        );
        assert!(
            found_song_title_text,
            "Should find 'Song One' in Now Playing with TEXT fg"
        );
        assert!(
            found_gauge_primary && found_gauge_muted,
            "Should find Gauge cells with PRIMARY fg and MUTED bg"
        );
    }

    #[test]
    fn test_cover_caching_and_now_playing_rendering_with_and_without_cover() {
        use crate::ui::draw_ui;
        use image::{DynamicImage, Rgba, RgbaImage};
        use ratatui::{Terminal, backend::TestBackend};

        let mut state = AppState::new("tester".to_string(), "crystalify".to_string(), 32768);
        let tracks = sample_tracks();
        let cover_url = tracks[0].cover_url.clone().unwrap();

        state.start_playback_queue(tracks.clone(), 0);
        assert!(state.cover_image.is_none());

        let img = DynamicImage::ImageRgba8(RgbaImage::from_pixel(64, 64, Rgba([236, 153, 255, 255])));
        state.cache_and_apply_cover(cover_url.clone(), img);
        assert!(state.cover_image.is_some());
        assert!(state.cover_cache.contains_key(&cover_url));

        state.next_track();
        assert!(state.cover_image.is_none());

        state.prev_track();
        assert!(state.cover_image.is_some());

        let dummy_uri = SpotifyUri::from_uri("spotify:track:7tvuLLroI0n6uYBWuFig5d").unwrap();
        state.handle_player_event(PlayerEvent::Playing {
            play_request_id: 1,
            track_id: dummy_uri,
            position_ms: 0,
        });

        let backend = TestBackend::new(120, 35);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw_ui(f, &state)).unwrap();
        let buf = terminal.backend().buffer();

        let mut full_text = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                full_text.push_str(buf[(x, y)].symbol());
            }
            full_text.push('\n');
        }
        assert!(full_text.contains("[PLAYING]"));
        assert!(full_text.contains("Title:  Song One"));
        assert!(full_text.contains("Artist: Artist A"));
        assert!(full_text.contains("Album:  Album X"));

        let mut has_halfblock_in_top_right_of_now_playing = false;
        for y in 5..12 {
            for x in 35..53 {
                let sym = buf[(x, y)].symbol();
                if sym == "▀" || sym == "▄" || sym == "█" || buf[(x, y)].bg != ratatui::style::Color::Reset {
                    has_halfblock_in_top_right_of_now_playing = true;
                }
            }
        }
        assert!(has_halfblock_in_top_right_of_now_playing);

        for (w, h) in [(30, 12), (18, 8), (10, 5)] {
            let small_backend = TestBackend::new(w, h);
            let mut small_term = Terminal::new(small_backend).unwrap();
            small_term.draw(|f| draw_ui(f, &state)).unwrap();
        }
    }

    #[test]
    fn test_playback_status_transitions_by_events() {
        let mut state = AppState::new("tester".to_string(), "crystalify".to_string(), 30000);
        assert_eq!(state.playback_status, PlaybackStatus::Idle);
        assert!(!state.is_playing);

        let tracks = sample_tracks();
        state.set_track(tracks[0].clone());
        assert_eq!(state.playback_status, PlaybackStatus::Loading);
        assert!(!state.is_playing);
        assert_eq!(state.position_ms, 0);

        let dummy_uri = SpotifyUri::from_uri("spotify:track:7tvuLLroI0n6uYBWuFig5d").unwrap();

        state.handle_player_event(PlayerEvent::Loading {
            play_request_id: 1,
            track_id: dummy_uri.clone(),
            position_ms: 0,
        });
        assert_eq!(state.playback_status, PlaybackStatus::Loading);
        assert!(!state.is_playing);

        state.handle_player_event(PlayerEvent::Playing {
            play_request_id: 1,
            track_id: dummy_uri.clone(),
            position_ms: 1200,
        });
        assert_eq!(state.playback_status, PlaybackStatus::Playing);
        assert!(state.is_playing);
        assert_eq!(state.position_ms, 1200);

        state.handle_player_event(PlayerEvent::Paused {
            play_request_id: 1,
            track_id: dummy_uri.clone(),
            position_ms: 1500,
        });
        assert_eq!(state.playback_status, PlaybackStatus::Paused);
        assert!(!state.is_playing);
        assert_eq!(state.position_ms, 1500);

        state.handle_player_event(PlayerEvent::Unavailable {
            play_request_id: 1,
            track_id: dummy_uri.clone(),
        });
        assert!(matches!(state.playback_status, PlaybackStatus::Failed(_)));
        assert!(!state.is_playing);
        assert!(state.status_message.is_some());

        state.handle_player_event(PlayerEvent::EndOfTrack {
            play_request_id: 1,
            track_id: dummy_uri,
        });
        assert!(matches!(state.playback_status, PlaybackStatus::Failed(_)));
        assert!(state.pending_next_track);
    }

    #[test]
    fn test_tick_only_advances_in_playing_and_times_out_in_loading() {
        let mut state = AppState::new("tester".to_string(), "crystalify".to_string(), 30000);
        let tracks = sample_tracks();

        state.tick(5000);
        assert_eq!(state.position_ms, 0);
        assert_eq!(state.playback_status, PlaybackStatus::Idle);

        state.set_track(tracks[0].clone());
        assert_eq!(state.playback_status, PlaybackStatus::Loading);

        state.tick(5000);
        assert_eq!(state.position_ms, 0);
        assert_eq!(state.playback_status, PlaybackStatus::Loading);
        assert_eq!(state.loading_elapsed_ms, 5000);

        state.tick(5000);
        assert_eq!(state.position_ms, 0);
        assert!(matches!(state.playback_status, PlaybackStatus::Failed(_)));
        assert!(!state.is_playing);
        assert!(state.status_message.is_some());

        state.tick(5000);
        assert_eq!(state.position_ms, 0);

        let dummy_uri = SpotifyUri::from_uri("spotify:track:7tvuLLroI0n6uYBWuFig5d").unwrap();
        state.handle_player_event(PlayerEvent::Playing {
            play_request_id: 2,
            track_id: dummy_uri.clone(),
            position_ms: 1000,
        });
        state.tick(250);
        assert_eq!(state.position_ms, 1250);

        state.handle_player_event(PlayerEvent::Paused {
            play_request_id: 2,
            track_id: dummy_uri,
            position_ms: 1250,
        });
        state.tick(1000);
        assert_eq!(state.position_ms, 1250);
    }

    #[test]
    fn test_set_audio_info_and_set_track_with_audio_error() {
        let mut state = AppState::new("tester".to_string(), "crystalify".to_string(), 30000);
        state.set_audio_info(
            "rodio (ALSA) — not found".to_string(),
            Some("Audio output device not found".to_string()),
        );
        assert_eq!(
            state.playback_status,
            PlaybackStatus::Failed("Audio output device not found".to_string())
        );

        state.set_track(sample_tracks()[0].clone());
        assert_eq!(
            state.playback_status,
            PlaybackStatus::Failed("Audio output device not found".to_string())
        );
        assert!(!state.is_playing);
    }

    #[test]
    fn test_ui_renders_all_playback_badges_error_line_and_no_pipewire() {
        use crate::ui::draw_ui;
        use crate::ui::theme::{ERROR, MUTED, PRIMARY, TEXT};
        use ratatui::{Terminal, backend::TestBackend};

        let backend = TestBackend::new(120, 35);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = AppState::new("tester".to_string(), "crystalify".to_string(), 32768);
        state.set_audio_info("rodio (WASAPI) — Speakers".to_string(), None);
        state.set_track(sample_tracks()[0].clone());

        terminal.draw(|f| draw_ui(f, &state)).unwrap();
        let buf = terminal.backend().buffer();
        let mut full_text = String::new();
        let mut found_loading = false;
        for y in 0..buf.area.height {
            let row_str: String = (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol())
                .collect();
            if let Some(col) = row_str.find("[LOADING]") {
                assert_eq!(buf[(col as u16, y)].bg, TEXT);
                found_loading = true;
            }
            full_text.push_str(&row_str);
            full_text.push('\n');
        }
        assert!(found_loading, "Should render [LOADING] badge with TEXT bg");
        assert!(!full_text.contains("PipeWire"));
        assert!(full_text.contains("rodio (WASAPI) — Speakers"));

        let dummy_uri = SpotifyUri::from_uri("spotify:track:7tvuLLroI0n6uYBWuFig5d").unwrap();
        state.handle_player_event(PlayerEvent::Playing {
            play_request_id: 1,
            track_id: dummy_uri.clone(),
            position_ms: 5000,
        });
        terminal.draw(|f| draw_ui(f, &state)).unwrap();
        let buf = terminal.backend().buffer();
        let mut found_playing = false;
        for y in 0..buf.area.height {
            let row_str: String = (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol())
                .collect();
            if let Some(col) = row_str.find("[PLAYING]") {
                assert_eq!(buf[(col as u16, y)].bg, PRIMARY);
                found_playing = true;
            }
        }
        assert!(found_playing, "Should render [PLAYING] badge with PRIMARY bg");

        state.handle_player_event(PlayerEvent::Paused {
            play_request_id: 1,
            track_id: dummy_uri,
            position_ms: 5000,
        });
        terminal.draw(|f| draw_ui(f, &state)).unwrap();
        let buf = terminal.backend().buffer();
        let mut found_paused = false;
        for y in 0..buf.area.height {
            let row_str: String = (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol())
                .collect();
            if let Some(col) = row_str.find("[PAUSED]") {
                assert_eq!(buf[(col as u16, y)].bg, MUTED);
                found_paused = true;
            }
        }
        assert!(found_paused, "Should render [PAUSED] badge with MUTED bg");

        state.set_audio_info(
            "rodio (WASAPI) — not found".to_string(),
            Some("Audio output device not found".to_string()),
        );
        terminal.draw(|f| draw_ui(f, &state)).unwrap();
        let buf = terminal.backend().buffer();
        let mut full_text = String::new();
        let mut found_error_badge = false;
        let mut found_error_line = false;
        for y in 0..buf.area.height {
            let row_str: String = (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol())
                .collect();
            if let Some(col) = row_str.find("[ERROR]") {
                assert_eq!(buf[(col as u16, y)].bg, ERROR);
                found_error_badge = true;
            }
            if let Some(col) = row_str.find("Error:  Audio output device") {
                assert_eq!(buf[(col as u16, y)].fg, ERROR);
                found_error_line = true;
            }
            full_text.push_str(&row_str);
            full_text.push('\n');
        }
        assert!(found_error_badge, "Should render [ERROR] badge with ERROR bg");
        assert!(found_error_line, "Should render Error line with ERROR fg");
        assert!(full_text.contains("not found"));
        assert!(!full_text.contains("PipeWire"));
    }

    #[test]
    fn test_unavailable_uses_english_fallback_and_timeout_is_english() {
        crate::spotify::logger::clear_last_error();
        let mut state = AppState::new("tester".to_string(), "crystalify".to_string(), 30000);
        let dummy_uri = SpotifyUri::from_uri("spotify:track:7tvuLLroI0n6uYBWuFig5d").unwrap();

        state.handle_player_event(PlayerEvent::Unavailable {
            play_request_id: 1,
            track_id: dummy_uri,
        });
        assert_eq!(
            state.playback_status,
            PlaybackStatus::Failed("Track unavailable or failed to load stream".to_string())
        );

        state.set_track(sample_tracks()[0].clone());
        state.tick(PLAYBACK_LOADING_TIMEOUT_MS);
        assert_eq!(
            state.playback_status,
            PlaybackStatus::Failed(
                "Playback did not start: check audio device or network".to_string()
            )
        );
    }

    #[test]
    fn test_cover_layout_padding_alignment_and_error_below() {
        use crate::ui::components::player::render_now_playing;
        use image::{DynamicImage, Rgba, RgbaImage};
        use ratatui::{Terminal, backend::TestBackend, layout::Rect, style::Color};

        assert_eq!(COVER_WIDTH_CELLS, 10);
        assert_eq!(COVER_HEIGHT_CELLS, 5);

        let mut state = AppState::new("tester".to_string(), "crystalify".to_string(), 32768);
        let tracks = sample_tracks();
        let cover_url = tracks[0].cover_url.clone().unwrap();
        state.start_playback_queue(tracks, 0);
        let img = DynamicImage::ImageRgba8(RgbaImage::from_pixel(64, 64, Rgba([236, 153, 255, 255])));
        state.cache_and_apply_cover(cover_url, img);
        state.set_playback_failed("Stream error detail");

        let area = Rect::new(0, 0, 52, 16);
        let backend = TestBackend::new(area.width, area.height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render_now_playing(f, &state, area)).unwrap();
        let buf = terminal.backend().buffer();

        for x in 1..(area.width - 1) {
            assert_eq!(buf[(x, 1)].symbol(), " ");
        }

        let mut badge_y = None;
        let mut album_y = None;
        let mut error_y = None;
        let mut cover_min_x = u16::MAX;
        let mut cover_max_x = 0u16;
        let mut cover_min_y = u16::MAX;
        let mut cover_max_y = 0u16;

        for y in 1..(area.height - 1) {
            let row_str: String = (0..area.width)
                .map(|x| buf[(x, y)].symbol())
                .collect();
            if row_str.contains("[ERROR]") {
                badge_y = Some(y);
            }
            if row_str.contains("Album:  Album X") {
                album_y = Some(y);
            }
            if row_str.contains("Error:  Stream error detail") {
                error_y = Some(y);
            }
            for x in 30..(area.width - 1) {
                let sym = buf[(x, y)].symbol();
                if sym == "▀" || sym == "▄" || sym == "█" || buf[(x, y)].bg != Color::Reset {
                    cover_min_x = cover_min_x.min(x);
                    cover_max_x = cover_max_x.max(x);
                    cover_min_y = cover_min_y.min(y);
                    cover_max_y = cover_max_y.max(y);
                }
            }
        }

        let badge_y = badge_y.expect("badge must render");
        let album_y = album_y.expect("album line must render");
        let error_y = error_y.expect("error line must render");

        assert_eq!(badge_y, 2);
        assert_eq!(cover_min_y, badge_y);
        assert!(cover_max_y <= album_y);
        assert!(cover_max_x <= area.width - 4);
        assert!(cover_min_x >= 35);
        assert!(error_y > album_y);
        assert!(error_y > cover_max_y);
    }

    #[test]
    fn test_lyrics_synced_selection_unsynced_scroll_and_cache() {
        use crate::spotify::models::SyncedLyricLine;

        let mut state = AppState::new("tester".to_string(), "crystalify".to_string(), 30000);
        let tracks = sample_tracks();
        state.set_track(tracks[0].clone());
        assert_eq!(state.lyrics, TrackLyrics::Loading);
        assert_eq!(
            state.current_lyric_display(),
            ("Loading lyrics...".to_string(), false)
        );

        let synced = TrackLyrics::Synced(vec![
            SyncedLyricLine {
                time_ms: 5_000,
                text: "First verse".to_string(),
            },
            SyncedLyricLine {
                time_ms: 15_000,
                text: "Second verse".to_string(),
            },
        ]);
        state.cache_and_apply_lyrics(tracks[0].uri.clone(), synced.clone());
        assert_eq!(state.active_synced_lyric_index(), None);
        assert_eq!(state.current_lyric_display(), ("♪".to_string(), true));

        state.position_ms = 5_000;
        assert_eq!(state.active_synced_lyric_index(), Some(0));
        assert_eq!(
            state.current_lyric_display(),
            ("First verse".to_string(), true)
        );

        state.position_ms = 18_000;
        assert_eq!(state.active_synced_lyric_index(), Some(1));
        assert_eq!(
            state.current_lyric_display(),
            ("Second verse".to_string(), true)
        );

        state.set_track(tracks[1].clone());
        assert_eq!(state.lyrics, TrackLyrics::Loading);
        state.cache_and_apply_lyrics(
            tracks[1].uri.clone(),
            TrackLyrics::Unsynced(vec![
                "Line 1".to_string(),
                "Line 2".to_string(),
                "Line 3".to_string(),
            ]),
        );
        assert_eq!(state.current_lyric_display(), ("Line 1".to_string(), true));
        state.scroll_lyrics_up();
        assert_eq!(state.lyrics_manual_index, 0);
        state.scroll_lyrics_down();
        assert_eq!(state.lyrics_manual_index, 1);
        assert_eq!(state.current_lyric_display(), ("Line 2".to_string(), true));
        state.scroll_lyrics_down();
        state.scroll_lyrics_down();
        assert_eq!(state.lyrics_manual_index, 2);
        assert_eq!(state.current_lyric_display(), ("Line 3".to_string(), true));

        state.set_track(tracks[0].clone());
        assert_eq!(state.lyrics, synced);
    }

    #[test]
    fn test_ui_lyrics_panel_single_line_gradient_animation_and_45_55_split() {
        use crate::spotify::models::SyncedLyricLine;
        use crate::ui::draw_ui;
        use ratatui::{Terminal, backend::TestBackend, style::Color};
        use std::collections::HashSet;

        let mut state = AppState::new("tester".to_string(), "crystalify".to_string(), 32768);
        let tracks = sample_tracks();
        state.start_playback_queue(tracks.clone(), 0);
        let dummy_uri = SpotifyUri::from_uri("spotify:track:7tvuLLroI0n6uYBWuFig5d").unwrap();
        state.handle_player_event(PlayerEvent::Playing {
            play_request_id: 1,
            track_id: dummy_uri,
            position_ms: 10_000,
        });
        state.cache_and_apply_lyrics(
            tracks[0].uri.clone(),
            TrackLyrics::Synced(vec![SyncedLyricLine {
                time_ms: 5_000,
                text: "I know the pieces fit".to_string(),
            }]),
        );

        let backend = TestBackend::new(140, 35);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw_ui(f, &state)).unwrap();
        let buf = terminal.backend().buffer();

        let mut lyrics_title_y = None;
        let mut lyric_text_y = None;
        let mut lyric_start_x = None;
        let mut search_border_x = None;
        let mut full_text = String::new();

        for y in 0..buf.area.height {
            let row_str: String = (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol())
                .collect();
            if row_str.contains("Lyrics [Synced]") {
                lyrics_title_y = Some(y);
            }
            if let Some(byte_col) = row_str.find("I know the pieces fit") {
                lyric_text_y = Some(y);
                lyric_start_x = Some(row_str[..byte_col].chars().count() as u16);
            }
            if y == 4 {
                for x in 55..buf.area.width {
                    if buf[(x, y)].symbol() == "╭" {
                        search_border_x = Some(x);
                        break;
                    }
                }
            }
            full_text.push_str(&row_str);
            full_text.push('\n');
        }

        let lyrics_title_y = lyrics_title_y.expect("Lyrics [Synced] sub-panel must render");
        let lyric_text_y = lyric_text_y.expect("Lyric text must render in single line");
        let lyric_start_x = lyric_start_x.expect("Lyric start x must be found");
        let search_border_x = search_border_x.expect("Search panel must render");

        assert!((60..=66).contains(&search_border_x));
        assert_eq!(lyric_text_y, lyrics_title_y + 1);
        assert!(full_text.contains("[w/s] Lyrics"));

        let initial_colors: Vec<Color> = (0..21u16)
            .map(|offset| buf[(lyric_start_x + offset, lyric_text_y)].fg)
            .collect();
        let unique_colors: HashSet<Color> = initial_colors.iter().copied().collect();
        assert!(unique_colors.len() >= 5);
        assert!(initial_colors.iter().all(|c| matches!(c, Color::Rgb(_, _, _))));

        for _ in 0..10 {
            state.tick(30);
        }
        terminal.draw(|f| draw_ui(f, &state)).unwrap();
        let buf_after = terminal.backend().buffer();
        let updated_colors: Vec<Color> = (0..21u16)
            .map(|offset| buf_after[(lyric_start_x + offset, lyric_text_y)].fg)
            .collect();
        assert_ne!(initial_colors, updated_colors);

        for (w, h) in [(30, 12), (18, 8), (10, 5)] {
            let small_backend = TestBackend::new(w, h);
            let mut small_term = Terminal::new(small_backend).unwrap();
            small_term.draw(|f| draw_ui(f, &state)).unwrap();
        }
    }

    #[test]
    fn test_track_metadata_from_audio_item_and_connect_events() {
        use librespot::metadata::artist::{ArtistRole, ArtistWithRole};
        use librespot::metadata::audio::item::CoverImage;
        use librespot::metadata::audio::{AudioFiles, AudioItem, UniqueFields};
        use librespot::metadata::image::ImageSize;
        use librespot::playback::player::QueueTrack;

        let track_uri = SpotifyUri::from_uri("spotify:track:55mJleti2WfWEFNFcBduhc").unwrap();
        let audio_item = AudioItem {
            track_id: track_uri.clone(),
            uri: track_uri.to_uri(),
            files: AudioFiles::default(),
            name: "Schism".to_string(),
            covers: vec![CoverImage {
                url: "https://i.scdn.co/image/schism64".to_string(),
                size: ImageSize::SMALL,
                width: 64,
                height: 64,
            }],
            language: Vec::new(),
            duration_ms: 407_000,
            is_explicit: false,
            availability: Ok(()),
            alternatives: None,
            unique_fields: UniqueFields::Track {
                artists: librespot::metadata::artist::ArtistsWithRole(vec![ArtistWithRole {
                    id: SpotifyUri::from_uri("spotify:artist:2yEwvVSSSUkcLeSTNyHKh8").unwrap(),
                    name: "TOOL".to_string(),
                    role: ArtistRole::ARTIST_ROLE_MAIN_ARTIST,
                }]),
                album: "Lateralus".to_string(),
                album_artists: Vec::new(),
                popularity: 70,
                number: 5,
                disc_number: 1,
            },
        };

        let meta = track_metadata_from_audio_item(&audio_item);
        assert_eq!(meta.title, "Schism");
        assert_eq!(meta.artist, "TOOL");
        assert_eq!(meta.album, "Lateralus");
        assert_eq!(meta.duration_ms, 407_000);
        assert_eq!(
            meta.cover_url,
            Some("https://i.scdn.co/image/schism64".to_string())
        );

        let mut state = AppState::new("tester".to_string(), "crystalify".to_string(), 32768);
        state.connect_ready = true;

        state.handle_player_event(PlayerEvent::SessionConnected {
            connection_id: "conn1".to_string(),
            user_name: "tester".to_string(),
        });
        assert!(state.connect_active);

        state.handle_player_event(PlayerEvent::SessionClientChanged {
            client_id: "cid1".to_string(),
            client_name: "Pixel 8".to_string(),
            client_brand_name: "Google".to_string(),
            client_model_name: "Pixel 8".to_string(),
        });
        assert_eq!(state.connect_client, Some("Pixel 8".to_string()));

        state.handle_player_event(PlayerEvent::SetQueue {
            context_uri: "spotify:playlist:abc".to_string(),
            current_track: Some(QueueTrack {
                uri: track_uri.to_uri(),
                provider: "context".to_string(),
            }),
            next_tracks: vec![QueueTrack {
                uri: track_uri.to_uri(),
                provider: "context".to_string(),
            }],
            prev_tracks: vec![QueueTrack {
                uri: track_uri.to_uri(),
                provider: "context".to_string(),
            }],
        });
        assert_eq!(state.connect_queue_position, Some((2, 3)));

        state.handle_player_event(PlayerEvent::TrackChanged {
            audio_item: Box::new(audio_item),
        });
        assert_eq!(state.track_title, "Schism");
        assert_eq!(state.track_artist, "TOOL");
        assert_eq!(state.track_album, "Lateralus");
        assert_eq!(state.lyrics, TrackLyrics::Loading);
        assert!(state.pending_media_fetch);

        state.handle_player_event(PlayerEvent::SessionDisconnected {
            connection_id: "conn1".to_string(),
            user_name: "tester".to_string(),
        });
        assert!(!state.connect_active);
        assert_eq!(state.connect_client, None);
        assert_eq!(state.connect_queue_position, None);
    }

    #[test]
    fn test_library_view_navigation_and_ui_rendering_with_connect_status() {
        use crate::spotify::models::{AlbumItem, PlaylistItem};
        use crate::ui::draw_ui;
        use ratatui::{Terminal, backend::TestBackend};

        let mut state = AppState::new("tester".to_string(), "crystalify".to_string(), 32768);
        state.apply_library(LibraryData {
            playlists: vec![PlaylistItem {
                uri: "spotify:playlist:my1".to_string(),
                id: "my1".to_string(),
                name: "Favorites".to_string(),
                owner: "tester".to_string(),
                total_tracks: 15,
            }],
            liked_tracks: sample_tracks(),
            saved_albums: vec![AlbumItem {
                uri: "spotify:album:al1".to_string(),
                id: "al1".to_string(),
                name: "Lateralus".to_string(),
                artist: "TOOL".to_string(),
                total_tracks: 13,
                year: Some(2001),
            }],
            loaded: true,
            liked_total_expected: None,
            liked_loading_more: false,
        });

        state.toggle_browser_view();
        assert_eq!(state.browser_view, BrowserView::Library);
        assert_eq!(state.library_category, LibraryCategory::Playlists);
        assert_eq!(state.current_list_len(), 1);

        state.switch_category_next();
        assert_eq!(state.library_category, LibraryCategory::LikedSongs);
        assert_eq!(state.current_list_len(), 3);
        state.move_selection_down();
        assert_eq!(state.selected_index, 1);

        state.switch_category_next();
        assert_eq!(state.library_category, LibraryCategory::SavedAlbums);
        assert_eq!(state.current_list_len(), 1);

        state.connect_ready = true;
        state.connect_active = true;
        state.connect_client = Some("Pixel 8".to_string());

        let backend = TestBackend::new(140, 35);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw_ui(f, &state)).unwrap();
        let buf = terminal.backend().buffer();
        let mut full_text = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                full_text.push_str(buf[(x, y)].symbol());
            }
            full_text.push('\n');
        }
        assert!(full_text.contains("My Library"));
        assert!(full_text.contains("[LIBRARY]"));
        assert!(full_text.contains("[1] Playlists (1)"));
        assert!(full_text.contains("[2] Liked Songs (3)"));
        assert!(full_text.contains("[3] Saved Albums (1)"));
        assert!(full_text.contains("Active (via Pixel 8)"));
        assert!(full_text.contains("[l] Library"));

        state.connect_active = false;
        state.connect_client = None;
        terminal.draw(|f| draw_ui(f, &state)).unwrap();
        let buf = terminal.backend().buffer();
        let mut text_standby = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                text_standby.push_str(buf[(x, y)].symbol());
            }
            text_standby.push('\n');
        }
        assert!(text_standby.contains("Standby (ready for remote)"));

        state.connect_ready = false;
        terminal.draw(|f| draw_ui(f, &state)).unwrap();
        let buf = terminal.backend().buffer();
        let mut text_disabled = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                text_disabled.push_str(buf[(x, y)].symbol());
            }
            text_disabled.push('\n');
        }
        assert!(text_disabled.contains("Disabled"));
    }

    #[test]
    fn test_compute_list_window_bounds_and_large_lists() {
        use crate::ui::components::playlist::compute_list_window;

        assert_eq!(compute_list_window(0, 0, 20), (0, 0));
        assert_eq!(compute_list_window(10, 0, 0), (0, 0));
        assert_eq!(compute_list_window(5, 0, 20), (0, 5));
        assert_eq!(compute_list_window(5, 4, 20), (0, 5));
        assert_eq!(compute_list_window(5, 99, 20), (0, 5));

        assert_eq!(compute_list_window(10_000, 0, 20), (0, 20));
        assert_eq!(compute_list_window(10_000, 5, 20), (0, 20));
        assert_eq!(compute_list_window(10_000, 5_000, 20), (4_990, 5_010));
        assert_eq!(compute_list_window(10_000, 9_999, 20), (9_980, 10_000));
        assert_eq!(compute_list_window(10_000, 12_345, 20), (9_980, 10_000));
    }

    #[test]
    fn test_streaming_collection_library_batches_paging_and_animation_tick() {
        use crate::spotify::models::{PlaylistItem, SyncedLyricLine};

        let mut state = AppState::new("tester".to_string(), "crystalify".to_string(), 32768);
        assert!(!state.needs_animation_tick());

        state.playback_status = PlaybackStatus::Loading;
        assert!(state.needs_animation_tick());
        state.playback_status = PlaybackStatus::Playing;
        assert!(state.needs_animation_tick());
        state.playback_status = PlaybackStatus::Paused;
        assert!(!state.needs_animation_tick());
        state.lyrics = TrackLyrics::Synced(vec![SyncedLyricLine {
            time_ms: 1000,
            text: "Hello".to_string(),
        }]);
        assert!(state.needs_animation_tick());
        state.lyrics = TrackLyrics::Unavailable;
        assert!(!state.needs_animation_tick());

        let make_track = |i: usize| TrackMetadata {
            uri: format!("spotify:track:trk{i}"),
            title: format!("Track {i}"),
            artist: format!("Artist {i}"),
            album: format!("Album {i}"),
            duration_ms: 180_000,
            cover_url: None,
        };

        let batch1: Vec<TrackMetadata> = (1..=50).map(make_track).collect();
        let batch2: Vec<TrackMetadata> = (51..=120).map(make_track).collect();

        state.open_streaming_collection(CollectionView {
            kind: CollectionKind::Playlist,
            uri: "spotify:playlist:big1".to_string(),
            title: "Big Playlist".to_string(),
            subtitle: "by tester".to_string(),
            tracks: batch1,
            total_tracks: Some(120),
            is_loading_more: true,
        });
        assert_eq!(state.collection_selected_index, 0);
        assert_eq!(state.current_list_len(), 50);
        assert!(state.opened_collection.as_ref().unwrap().is_loading_more);

        state.move_selection_page_down(15);
        assert_eq!(state.collection_selected_index, 15);
        state.move_selection_page_down(15);
        assert_eq!(state.collection_selected_index, 30);

        state.append_collection_tracks(batch2, Some(120), true);
        assert_eq!(state.collection_selected_index, 30);
        assert_eq!(state.current_list_len(), 120);
        assert!(!state.opened_collection.as_ref().unwrap().is_loading_more);
        assert_eq!(
            state.opened_collection.as_ref().unwrap().total_tracks,
            Some(120)
        );

        state.move_selection_bottom();
        assert_eq!(state.collection_selected_index, 119);
        state.move_selection_page_up(15);
        assert_eq!(state.collection_selected_index, 104);
        state.move_selection_top();
        assert_eq!(state.collection_selected_index, 0);
        state.move_selection_page_up(15);
        assert_eq!(state.collection_selected_index, 0);

        state.close_collection();
        state.browser_view = BrowserView::Library;
        state.library_loading = true;
        state.apply_library_playlists(vec![PlaylistItem {
            uri: "spotify:playlist:p1".to_string(),
            id: "p1".to_string(),
            name: "PL 1".to_string(),
            owner: "tester".to_string(),
            total_tracks: 120,
        }]);
        assert!(state.library.loaded);
        assert_eq!(state.library.playlists.len(), 1);

        state.apply_library_albums(vec![AlbumItem {
            uri: "spotify:album:a1".to_string(),
            id: "a1".to_string(),
            name: "Album 1".to_string(),
            artist: "Artist 1".to_string(),
            total_tracks: 10,
            year: Some(2020),
        }]);
        assert_eq!(state.library.saved_albums.len(), 1);

        state.set_library_category(LibraryCategory::LikedSongs);
        state.apply_library_liked_batch((1..=40).map(make_track).collect(), Some(90), true, false);
        assert_eq!(state.library.liked_tracks.len(), 40);
        assert_eq!(state.library.liked_total_expected, Some(90));
        assert!(state.library.liked_loading_more);
        assert!(state.library_loading);

        state.move_selection_page_down(15);
        assert_eq!(state.selected_index, 15);

        state.apply_library_liked_batch((41..=90).map(make_track).collect(), Some(90), false, true);
        assert_eq!(state.library.liked_tracks.len(), 90);
        assert!(!state.library.liked_loading_more);
        assert!(!state.library_loading);
        assert_eq!(state.selected_index, 15);

        state.move_selection_bottom();
        assert_eq!(state.selected_index, 89);
        state.move_selection_top();
        assert_eq!(state.selected_index, 0);
    }

    #[test]
    fn test_virtualized_500_track_playlist_render_and_loading_indicator() {
        use crate::ui::draw_ui;
        use ratatui::{Terminal, backend::TestBackend};

        let mut state = AppState::new("tester".to_string(), "crystalify".to_string(), 32768);
        let tracks: Vec<TrackMetadata> = (1..=500)
            .map(|i| TrackMetadata {
                uri: format!("spotify:track:pl500_{i}"),
                title: format!("MegaSong #{i}"),
                artist: format!("Band {i}"),
                album: "MegaAlbum".to_string(),
                duration_ms: 210_000,
                cover_url: None,
            })
            .collect();

        state.open_streaming_collection(CollectionView {
            kind: CollectionKind::Playlist,
            uri: "spotify:playlist:mega500".to_string(),
            title: "Mega 500".to_string(),
            subtitle: "by tester".to_string(),
            tracks,
            total_tracks: Some(750),
            is_loading_more: true,
        });
        state.collection_selected_index = 249;

        let backend = TestBackend::new(140, 35);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw_ui(f, &state)).unwrap();
        let buf = terminal.backend().buffer();
        let mut rendered = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                rendered.push_str(buf[(x, y)].symbol());
            }
            rendered.push('\n');
        }

        assert!(
            rendered.contains("250. MegaSong #250"),
            "Expected track #250 to be rendered in virtualized window"
        );
        assert!(
            rendered.contains("500 / 750 tracks — loading..."),
            "Expected loading... indicator while is_loading_more == true"
        );
        assert!(
            !rendered.contains("MegaSong #1 "),
            "Virtualized window around #250 must not render #1"
        );

        state.append_collection_tracks(Vec::new(), Some(500), true);
        terminal.draw(|f| draw_ui(f, &state)).unwrap();
        let buf_done = terminal.backend().buffer();
        let mut rendered_done = String::new();
        for y in 0..buf_done.area.height {
            for x in 0..buf_done.area.width {
                rendered_done.push_str(buf_done[(x, y)].symbol());
            }
            rendered_done.push('\n');
        }
        assert!(rendered_done.contains("500 tracks"));
        assert!(!rendered_done.contains("loading..."));
    }

    #[test]
    fn test_preload_debounce_hover_next_and_gapless_guard() {
        let mut state = AppState::new("tester".to_string(), "crystalify".to_string(), 32768);
        let tracks = sample_tracks();
        state.search_results.tracks = tracks.clone();
        state.selected_index = 0;

        assert!(!state.needs_animation_tick());
        state.schedule_preload_for_selection();
        assert_eq!(state.pending_preload_uri.as_deref(), Some("spotify:track:1"));
        assert!(state.needs_animation_tick());

        assert_eq!(state.poll_ready_preload(100), None);
        assert_eq!(state.poll_ready_preload(79), None);
        assert_eq!(
            state.poll_ready_preload(1),
            Some("spotify:track:1".to_string())
        );
        assert!(state.pending_preload_uri.is_none());
        assert_eq!(state.last_preloaded_uri.as_deref(), Some("spotify:track:1"));

        state.schedule_preload_for_selection();
        assert!(state.pending_preload_uri.is_none());

        state.start_playback_queue(tracks.clone(), 0);
        state.playback_status = PlaybackStatus::Playing;
        state.duration_ms = 180_000;
        state.position_ms = 170_000;

        state.selected_index = 0;
        state.schedule_preload_for_selection();
        assert!(state.pending_preload_uri.is_none());

        state.selected_index = 1;
        state.schedule_preload_for_selection();
        assert_eq!(state.pending_preload_uri.as_deref(), Some("spotify:track:2"));
        assert_eq!(state.poll_ready_preload(250), None);

        state.position_ms = 100_000;
        assert_eq!(
            state.poll_ready_preload(180),
            Some("spotify:track:2".to_string())
        );
        assert_eq!(state.last_preloaded_uri.as_deref(), Some("spotify:track:2"));

        state.schedule_preload_for_next_in_queue();
        assert!(state.pending_preload_uri.is_none());

        state.queue_index = 1;
        state.track_uri = "spotify:track:2".to_string();
        state.schedule_preload_for_next_in_queue();
        assert_eq!(state.pending_preload_uri.as_deref(), Some("spotify:track:3"));
        assert_eq!(
            state.poll_ready_preload(180),
            Some("spotify:track:3".to_string())
        );
    }
}
