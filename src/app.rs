use crate::spotify::models::{
    CollectionKind, CollectionView, SearchCategory, SearchResults, TrackMetadata,
};
use librespot::playback::player::PlayerEvent;

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
    pub duration_ms: u32,
    pub position_ms: u32,
    pub is_playing: bool,
    pub volume: u16,
    pub username: String,
    pub device_name: String,

    // Search & browser state
    pub input_mode: InputMode,
    pub search_query: String,
    pub is_loading: bool,
    pub status_message: Option<String>,
    pub search_category: SearchCategory,
    pub search_results: SearchResults,
    pub selected_index: usize,
    pub opened_collection: Option<CollectionView>,
    pub collection_selected_index: usize,

    // Playback queue
    pub queue: Vec<TrackMetadata>,
    pub queue_index: usize,
    pub pending_next_track: bool,
}

impl AppState {
    pub fn new(username: String, device_name: String, volume: u16) -> Self {
        Self {
            track_uri: String::new(),
            track_title: "No Track Playing".to_string(),
            track_artist: "Press [/] to search songs, albums, artists, playlists".to_string(),
            track_album: "—".to_string(),
            duration_ms: 0,
            position_ms: 0,
            is_playing: false,
            volume,
            username,
            device_name,
            input_mode: InputMode::Normal,
            search_query: String::new(),
            is_loading: false,
            status_message: Some(
                "Press [/] to search songs, albums, artists & playlists".to_string(),
            ),
            search_category: SearchCategory::Tracks,
            search_results: SearchResults::default(),
            selected_index: 0,
            opened_collection: None,
            collection_selected_index: 0,
            queue: Vec::new(),
            queue_index: 0,
            pending_next_track: false,
        }
    }

    pub fn set_track(&mut self, meta: TrackMetadata) {
        self.track_uri = meta.uri;
        self.track_title = meta.title;
        self.track_artist = meta.artist;
        self.track_album = meta.album;
        self.duration_ms = meta.duration_ms;
        self.position_ms = 0;
        self.is_playing = true;
    }

    pub fn current_list_len(&self) -> usize {
        if let Some(ref col) = self.opened_collection {
            col.tracks.len()
        } else {
            match self.search_category {
                SearchCategory::Tracks => self.search_results.tracks.len(),
                SearchCategory::Albums => self.search_results.albums.len(),
                SearchCategory::Artists => self.search_results.artists.len(),
                SearchCategory::Playlists => self.search_results.playlists.len(),
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

    pub fn switch_category_next(&mut self) {
        self.opened_collection = None;
        self.search_category = self.search_category.next();
        self.selected_index = 0;
    }

    pub fn switch_category_prev(&mut self) {
        self.opened_collection = None;
        self.search_category = self.search_category.prev();
        self.selected_index = 0;
    }

    pub fn set_category(&mut self, category: SearchCategory) {
        self.opened_collection = None;
        self.search_category = category;
        self.selected_index = 0;
    }

    pub fn open_collection(
        &mut self,
        kind: CollectionKind,
        title: String,
        subtitle: String,
        tracks: Vec<TrackMetadata>,
    ) {
        self.opened_collection = Some(CollectionView {
            kind,
            title,
            subtitle,
            tracks,
        });
        self.collection_selected_index = 0;
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

    pub fn handle_player_event(&mut self, event: PlayerEvent) {
        match event {
            PlayerEvent::Playing { position_ms, .. } => {
                self.position_ms = position_ms;
                self.is_playing = true;
            }
            PlayerEvent::Paused { position_ms, .. } => {
                self.position_ms = position_ms;
                self.is_playing = false;
            }
            PlayerEvent::Stopped { .. } => {
                self.is_playing = false;
            }
            PlayerEvent::EndOfTrack { .. } => {
                self.is_playing = false;
                self.pending_next_track = true;
            }
            PlayerEvent::PositionChanged { position_ms, .. }
            | PlayerEvent::PositionCorrection { position_ms, .. }
            | PlayerEvent::Seeked { position_ms, .. } => {
                self.position_ms = position_ms;
            }
            PlayerEvent::VolumeChanged { volume } => {
                self.volume = volume;
            }
            _ => {}
        }
    }

    pub fn tick(&mut self, delta_ms: u32) {
        if self.is_playing && self.position_ms < self.duration_ms {
            self.position_ms = (self.position_ms + delta_ms).min(self.duration_ms);
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
            },
            TrackMetadata {
                uri: "spotify:track:2".to_string(),
                title: "Song Two".to_string(),
                artist: "Artist B".to_string(),
                album: "Album Y".to_string(),
                duration_ms: 200_000,
            },
            TrackMetadata {
                uri: "spotify:track:3".to_string(),
                title: "Song Three".to_string(),
                artist: "Artist C".to_string(),
                album: "Album Z".to_string(),
                duration_ms: 220_000,
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
        // Wraps around to 0
        state.move_selection_down();
        assert_eq!(state.selected_index, 0);
        // Wraps around to end when moving up from 0
        state.move_selection_up();
        assert_eq!(state.selected_index, 2);

        // Open collection and verify independent selection index
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
        assert_eq!(state.selected_index, 2); // search selection preserved

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
        assert!(state.is_playing);

        let next = state.next_track().expect("Should advance to index 2");
        assert_eq!(next.title, "Song Three");
        assert_eq!(state.queue_index, 2);
        assert!(state.next_track().is_none());

        let prev = state.prev_track().expect("Should go back to index 1");
        assert_eq!(prev.title, "Song Two");
        assert_eq!(state.queue_index, 1);

        // Simulate EndOfTrack event
        let dummy_uri = SpotifyUri::from_uri("spotify:track:7tvuLLroI0n6uYBWuFig5d").unwrap();
        state.handle_player_event(PlayerEvent::EndOfTrack {
            play_request_id: 1,
            track_id: dummy_uri,
        });
        assert!(!state.is_playing);
        assert!(state.pending_next_track);

        // Ensure Albums category length works too
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

        // 1. Initial empty state
        terminal.draw(|f| draw_ui(f, &state)).unwrap();

        // 2. Editing search mode with Cyrillic & special chars
        state.input_mode = InputMode::EditingSearch;
        state.search_query = "Океан Ельзи & Tool".to_string();
        terminal.draw(|f| draw_ui(f, &state)).unwrap();

        // 3. Populated search results across all 4 tabs
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

        // 4. Opened collection state & error message display
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

        let primary_rgb = Color::Rgb(236, 153, 255); // #EC99FF
        let text_rgb = Color::Rgb(221, 230, 246); // #DDE6F6
        let muted_rgb = Color::Rgb(87, 124, 171); // #577CAB

        assert_eq!(PRIMARY, primary_rgb);
        assert_eq!(TEXT, text_rgb);
        assert_eq!(MUTED, muted_rgb);

        let backend = TestBackend::new(120, 35);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = AppState::new("tester".to_string(), "crystalify".to_string(), 32768);
        state.search_results.tracks = sample_tracks();

        // Render in PAUSED state first
        terminal.draw(|f| draw_ui(f, &state)).unwrap();
        let buf = terminal.backend().buffer();

        // Collect all rendered text from buffer
        let mut full_text = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                full_text.push_str(buf[(x, y)].symbol());
            }
            full_text.push('\n');
        }

        // Verify "Spotify Terminal Player" is completely gone and " crystalify " is present
        assert!(
            !full_text.contains("Spotify Terminal Player"),
            "Buffer must not contain 'Spotify Terminal Player'"
        );
        assert!(
            full_text.contains(" crystalify "),
            "Buffer must contain ' crystalify ' title"
        );

        // Verify root border top-left corner cell has PRIMARY fg (#EC99FF)
        assert_eq!(buf[(0, 0)].fg, primary_rgb);

        // Find "[PAUSED]" badge and verify its background is MUTED (#577CAB)
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

        // Now switch to PLAYING state at 50% progress and verify [PLAYING], Gauge, and track colors
        state.start_playback_queue(sample_tracks(), 0);
        state.position_ms = 90_000; // 50% of 180_000ms
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
            if let Some(col) = row_str.find("Song One") {
                // In Now Playing card, Song One has TEXT fg
                if col < 40 {
                    assert_eq!(buf[(col as u16, y)].fg, text_rgb);
                    found_song_title_text = true;
                }
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
}
