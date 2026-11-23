#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TrackMetadata {
    pub uri: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_ms: u32,
    pub cover_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlbumItem {
    pub uri: String,
    pub id: String,
    pub name: String,
    pub artist: String,
    pub total_tracks: u32,
    pub year: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtistItem {
    pub uri: String,
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaylistItem {
    pub uri: String,
    pub id: String,
    pub name: String,
    pub owner: String,
    pub total_tracks: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchCategory {
    #[default]
    Tracks,
    Albums,
    Artists,
    Playlists,
}

impl SearchCategory {
    pub fn next(self) -> Self {
        match self {
            Self::Tracks => Self::Albums,
            Self::Albums => Self::Artists,
            Self::Artists => Self::Playlists,
            Self::Playlists => Self::Tracks,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Self::Tracks => Self::Playlists,
            Self::Albums => Self::Tracks,
            Self::Artists => Self::Albums,
            Self::Playlists => Self::Artists,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Tracks => "Tracks",
            Self::Albums => "Albums",
            Self::Artists => "Artists",
            Self::Playlists => "Playlists",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SearchResults {
    pub tracks: Vec<TrackMetadata>,
    pub albums: Vec<AlbumItem>,
    pub artists: Vec<ArtistItem>,
    pub playlists: Vec<PlaylistItem>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectionKind {
    Album,
    Artist,
    Playlist,
}

impl CollectionKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Album => "Album",
            Self::Artist => "Artist",
            Self::Playlist => "Playlist",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectionView {
    pub kind: CollectionKind,
    pub uri: String,
    pub title: String,
    pub subtitle: String,
    pub tracks: Vec<TrackMetadata>,
    pub total_tracks: Option<usize>,
    pub is_loading_more: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BrowserView {
    #[default]
    Search,
    Library,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LibraryCategory {
    #[default]
    Playlists,
    LikedSongs,
    SavedAlbums,
}

impl LibraryCategory {
    pub fn next(self) -> Self {
        match self {
            Self::Playlists => Self::LikedSongs,
            Self::LikedSongs => Self::SavedAlbums,
            Self::SavedAlbums => Self::Playlists,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Self::Playlists => Self::SavedAlbums,
            Self::LikedSongs => Self::Playlists,
            Self::SavedAlbums => Self::LikedSongs,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Playlists => "Your Playlists",
            Self::LikedSongs => "Liked Songs",
            Self::SavedAlbums => "Saved Albums",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LibraryData {
    pub playlists: Vec<PlaylistItem>,
    pub liked_tracks: Vec<TrackMetadata>,
    pub saved_albums: Vec<AlbumItem>,
    pub loaded: bool,
    pub liked_total_expected: Option<usize>,
    pub liked_loading_more: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncedLyricLine {
    pub time_ms: u32,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum TrackLyrics {
    #[default]
    Empty,
    Loading,
    Instrumental,
    Synced(Vec<SyncedLyricLine>),
    Unsynced(Vec<String>),
    Unavailable,
}
