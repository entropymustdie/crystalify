use crate::spotify::models::{
    AlbumItem, ArtistItem, CollectionKind, PlaylistItem, SearchResults, TrackMetadata,
};
use futures_util::stream::{self, StreamExt};
use librespot::core::session::Session;
use librespot::core::spotify_uri::SpotifyUri;
use librespot::metadata::{Album, Artist, Metadata, Playlist, Track};
use reqwest::Client;
use serde_json::{Value, json};
use std::env;

const SPOTIFY_API_BASE: &str = "https://api.spotify.com/v1";
const PATHFINDER_URL: &str = "https://api-partner.spotify.com/pathfinder/v1/query";
const SEARCH_DESKTOP_HASH: &str =
    "fcad5a3e0d5af727fb76966f06971c19cfa2275e6ff7671196753e008611873c";

pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Clone)]
pub struct SpotifyClient {
    session: Session,
    http: Client,
}

/// Extract the base62 ID from a Spotify URI (`spotify:album:123` -> `123`) or return the string if already an ID.
pub fn extract_spotify_id(uri_or_id: &str) -> &str {
    let trimmed = uri_or_id.trim();
    trimmed.rsplit(':').next().unwrap_or(trimmed)
}

/// Percent-encode a query parameter for URL construction.
pub fn urlencode_query(input: &str) -> String {
    let mut out = String::with_capacity(input.len() * 2);
    for b in input.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            b' ' => out.push_str("%20"),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn parse_artists_list(val: &Value) -> String {
    let list = val
        .get("artists")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|a| a.get("name").and_then(Value::as_str))
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();

    if list.is_empty() {
        "Unknown Artist".to_string()
    } else {
        list
    }
}

fn parse_pathfinder_artists(val: &Value) -> String {
    let list = val
        .get("artists")
        .and_then(|a| a.get("items"))
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|a| {
                    a.get("profile")
                        .and_then(|p| p.get("name"))
                        .and_then(Value::as_str)
                        .or_else(|| a.get("name").and_then(Value::as_str))
                })
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();

    if list.is_empty() {
        "Unknown Artist".to_string()
    } else {
        list
    }
}

fn parse_track_item(item: &Value, fallback_album: Option<&str>) -> Option<TrackMetadata> {
    if item.is_null() {
        return None;
    }
    let id = item.get("id").and_then(Value::as_str).unwrap_or_default();
    let uri = item
        .get("uri")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| {
            if !id.is_empty() {
                format!("spotify:track:{id}")
            } else {
                String::new()
            }
        });

    if uri.is_empty() || !uri.starts_with("spotify:track:") {
        return None;
    }

    let title = item
        .get("name")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())?
        .to_string();

    let artist = parse_artists_list(item);

    let album = item
        .get("album")
        .and_then(|a| a.get("name"))
        .and_then(Value::as_str)
        .or(fallback_album)
        .unwrap_or("Unknown Album")
        .to_string();

    let duration_ms = item
        .get("duration_ms")
        .and_then(Value::as_u64)
        .unwrap_or(0) as u32;

    Some(TrackMetadata {
        uri,
        title,
        artist,
        album,
        duration_ms,
    })
}

/// Parse Spotify's GraphQL Pathfinder `searchV2` response (`data.searchV2`) into [`SearchResults`].
pub fn parse_pathfinder_search_response(json: &Value) -> SearchResults {
    let Some(search_v2) = json.get("data").and_then(|d| d.get("searchV2")) else {
        return SearchResults::default();
    };

    let tracks = search_v2
        .get("tracksV2")
        .and_then(|t| t.get("items"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|entry| {
                    let data = entry
                        .get("item")
                        .and_then(|i| i.get("data"))
                        .or_else(|| entry.get("data"))?;
                    let uri = data.get("uri").and_then(Value::as_str)?.to_string();
                    if !uri.starts_with("spotify:track:") {
                        return None;
                    }
                    let title = data
                        .get("name")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())?
                        .to_string();
                    let artist = parse_pathfinder_artists(data);
                    let album = data
                        .get("albumOfTrack")
                        .and_then(|a| a.get("name"))
                        .and_then(Value::as_str)
                        .unwrap_or("Unknown Album")
                        .to_string();
                    let duration_ms = data
                        .get("duration")
                        .and_then(|d| d.get("totalMilliseconds"))
                        .and_then(Value::as_u64)
                        .unwrap_or(0) as u32;

                    Some(TrackMetadata {
                        uri,
                        title,
                        artist,
                        album,
                        duration_ms,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let albums = search_v2
        .get("albumsV2")
        .and_then(|a| a.get("items"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|entry| {
                    let data = entry.get("data").unwrap_or(entry);
                    let uri = data.get("uri").and_then(Value::as_str)?.to_string();
                    if !uri.starts_with("spotify:album:") {
                        return None;
                    }
                    let id = extract_spotify_id(&uri).to_string();
                    let name = data
                        .get("name")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())?
                        .to_string();
                    let artist = parse_pathfinder_artists(data);
                    let year = data
                        .get("date")
                        .and_then(|d| d.get("year"))
                        .and_then(Value::as_u64)
                        .map(|y| y as u32);
                    let total_tracks = data
                        .get("tracks")
                        .and_then(|t| t.get("totalCount"))
                        .and_then(Value::as_u64)
                        .unwrap_or(0) as u32;

                    Some(AlbumItem {
                        uri,
                        id,
                        name,
                        artist,
                        total_tracks,
                        year,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let artists = search_v2
        .get("artists")
        .and_then(|a| a.get("items"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|entry| {
                    let data = entry.get("data").unwrap_or(entry);
                    let uri = data.get("uri").and_then(Value::as_str)?.to_string();
                    if !uri.starts_with("spotify:artist:") {
                        return None;
                    }
                    let id = extract_spotify_id(&uri).to_string();
                    let name = data
                        .get("profile")
                        .and_then(|p| p.get("name"))
                        .and_then(Value::as_str)
                        .or_else(|| data.get("name").and_then(Value::as_str))
                        .filter(|s| !s.is_empty())?
                        .to_string();

                    Some(ArtistItem { uri, id, name })
                })
                .collect()
        })
        .unwrap_or_default();

    let playlists = search_v2
        .get("playlists")
        .and_then(|p| p.get("items"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|entry| {
                    let data = entry.get("data").unwrap_or(entry);
                    let uri = data.get("uri").and_then(Value::as_str)?.to_string();
                    if !uri.starts_with("spotify:playlist:") {
                        return None;
                    }
                    let id = extract_spotify_id(&uri).to_string();
                    let name = data
                        .get("name")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())?
                        .to_string();
                    let owner = data
                        .get("ownerV2")
                        .and_then(|o| o.get("data"))
                        .and_then(|d| {
                            d.get("name")
                                .and_then(Value::as_str)
                                .or_else(|| d.get("username").and_then(Value::as_str))
                        })
                        .unwrap_or("Spotify")
                        .to_string();
                    let total_tracks = data
                        .get("tracks")
                        .and_then(|t| t.get("totalCount"))
                        .and_then(Value::as_u64)
                        .unwrap_or(0) as u32;

                    Some(PlaylistItem {
                        uri,
                        id,
                        name,
                        owner,
                        total_tracks,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    SearchResults {
        tracks,
        albums,
        artists,
        playlists,
    }
}

/// Pure function to parse `/v1/search` (or Pathfinder `data.searchV2`) JSON into [`SearchResults`].
pub fn parse_search_response(json: &Value) -> SearchResults {
    if json.get("data").and_then(|d| d.get("searchV2")).is_some() {
        return parse_pathfinder_search_response(json);
    }

    let tracks = json
        .get("tracks")
        .and_then(|t| t.get("items"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| parse_track_item(item, None))
                .collect()
        })
        .unwrap_or_default();

    let albums = json
        .get("albums")
        .and_then(|a| a.get("items"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter(|item| !item.is_null())
                .filter_map(|item| {
                    let id = item.get("id").and_then(Value::as_str)?.to_string();
                    let uri = item
                        .get("uri")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("spotify:album:{id}"));
                    let name = item.get("name").and_then(Value::as_str)?.to_string();
                    let artist = parse_artists_list(item);
                    let total_tracks = item
                        .get("total_tracks")
                        .and_then(Value::as_u64)
                        .unwrap_or(0) as u32;
                    let year = item
                        .get("release_date")
                        .and_then(Value::as_str)
                        .and_then(|d| d.get(0..4))
                        .and_then(|y| y.parse::<u32>().ok());
                    Some(AlbumItem {
                        uri,
                        id,
                        name,
                        artist,
                        total_tracks,
                        year,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let artists = json
        .get("artists")
        .and_then(|a| a.get("items"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter(|item| !item.is_null())
                .filter_map(|item| {
                    let id = item.get("id").and_then(Value::as_str)?.to_string();
                    let uri = item
                        .get("uri")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("spotify:artist:{id}"));
                    let name = item.get("name").and_then(Value::as_str)?.to_string();
                    Some(ArtistItem { uri, id, name })
                })
                .collect()
        })
        .unwrap_or_default();

    let playlists = json
        .get("playlists")
        .and_then(|p| p.get("items"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter(|item| !item.is_null())
                .filter_map(|item| {
                    let id = item.get("id").and_then(Value::as_str)?.to_string();
                    let uri = item
                        .get("uri")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("spotify:playlist:{id}"));
                    let name = item.get("name").and_then(Value::as_str)?.to_string();
                    let owner = item
                        .get("owner")
                        .and_then(|o| {
                            o.get("display_name")
                                .and_then(Value::as_str)
                                .or_else(|| o.get("id").and_then(Value::as_str))
                        })
                        .unwrap_or("Spotify")
                        .to_string();
                    let total_tracks = item
                        .get("tracks")
                        .and_then(|t| t.get("total"))
                        .and_then(Value::as_u64)
                        .unwrap_or(0) as u32;
                    Some(PlaylistItem {
                        uri,
                        id,
                        name,
                        owner,
                        total_tracks,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    SearchResults {
        tracks,
        albums,
        artists,
        playlists,
    }
}

/// Pure function to parse `/v1/albums/{id}/tracks` JSON into a list of [`TrackMetadata`].
pub fn parse_album_tracks_response(json: &Value, album_name: &str) -> Vec<TrackMetadata> {
    json.get("items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| parse_track_item(item, Some(album_name)))
                .collect()
        })
        .unwrap_or_default()
}

/// Pure function to parse `/v1/artists/{id}/top-tracks` JSON into a list of [`TrackMetadata`].
pub fn parse_artist_top_tracks_response(json: &Value) -> Vec<TrackMetadata> {
    json.get("tracks")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| parse_track_item(item, None))
                .collect()
        })
        .unwrap_or_default()
}

/// Pure function to parse `/v1/playlists/{id}/tracks` JSON into a list of [`TrackMetadata`].
pub fn parse_playlist_tracks_response(json: &Value) -> Vec<TrackMetadata> {
    json.get("items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter(|entry| !entry.is_null())
                .filter_map(|entry| {
                    let track_obj = entry.get("track").unwrap_or(entry);
                    parse_track_item(track_obj, None)
                })
                .collect()
        })
        .unwrap_or_default()
}

impl SpotifyClient {
    pub fn new(session: Session) -> Self {
        Self {
            session,
            http: Client::new(),
        }
    }

    async fn get_access_token(&self) -> Result<String, BoxError> {
        if let Ok(token) = env::var("SPOTIFY_ACCESS_TOKEN")
            && !token.trim().is_empty()
        {
            return Ok(token.trim().to_string());
        }
        let token = self.session.login5().auth_token().await?;
        Ok(token.access_token)
    }

    async fn search_via_pathfinder(&self, query: &str) -> Result<SearchResults, BoxError> {
        let token = self.get_access_token().await?;
        let client_token = self.session.spclient().client_token().await.ok();

        let gql_body = json!({
            "variables": {
                "searchTerm": query,
                "offset": 0,
                "limit": 20,
                "numberOfTopResults": 5,
                "includeAudiobooks": false,
                "includeArtistHasConcertsField": false,
                "includePreReleases": false,
                "includeLocalConcertsField": false
            },
            "operationName": "searchDesktop",
            "extensions": {
                "persistedQuery": {
                    "version": 1,
                    "sha256Hash": SEARCH_DESKTOP_HASH
                }
            }
        });

        let mut req = self
            .http
            .post(PATHFINDER_URL)
            .bearer_auth(&token)
            .header("app-platform", "WebPlayer")
            .json(&gql_body);

        if let Some(ct) = client_token {
            req = req.header("client-token", ct);
        }

        let resp = req.send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("Pathfinder search error ({status}): {body}").into());
        }

        let json: Value = resp.json().await?;
        let results = parse_pathfinder_search_response(&json);
        if results.tracks.is_empty()
            && results.albums.is_empty()
            && results.artists.is_empty()
            && results.playlists.is_empty()
            && json.get("errors").is_some()
        {
            return Err(format!("Pathfinder GraphQL errors: {}", json["errors"]).into());
        }
        Ok(results)
    }

    async fn search_via_spclient_context(&self, query: &str) -> Result<SearchResults, BoxError> {
        let formatted_q = query.split_whitespace().collect::<Vec<_>>().join("+");
        let search_uri = format!("spotify:search:{formatted_q}");
        let ctx = self.session.spclient().get_context(&search_uri).await?;

        let mut track_uris = Vec::new();
        for page in &ctx.pages {
            for t in &page.tracks {
                if let Some(ref u) = t.uri
                    && u.starts_with("spotify:track:")
                    && let Ok(parsed) = SpotifyUri::from_uri(u)
                {
                    track_uris.push(parsed);
                }
            }
        }
        track_uris.truncate(20);
        let tracks = self.fetch_tracks_via_metadata(&track_uris, None).await?;
        Ok(SearchResults {
            tracks,
            ..Default::default()
        })
    }

    async fn search_via_web_api(&self, query: &str) -> Result<SearchResults, BoxError> {
        let token = self.get_access_token().await?;
        let encoded_q = urlencode_query(query);
        let url = format!(
            "{SPOTIFY_API_BASE}/search?q={encoded_q}&type=track,album,artist,playlist&limit=20"
        );
        let resp = self.http.get(&url).bearer_auth(&token).send().await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("Spotify search API error ({status}): {body}").into());
        }

        let json: Value = resp.json().await?;
        Ok(parse_search_response(&json))
    }

    pub async fn search(&self, query: &str) -> Result<SearchResults, BoxError> {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return Ok(SearchResults::default());
        }

        // 1. Primary: Spotify Partner Pathfinder GraphQL API (supports Tracks, Albums, Artists, Playlists without 429 rate-limits)
        if let Ok(results) = self.search_via_pathfinder(trimmed).await
            && (!results.tracks.is_empty()
                || !results.albums.is_empty()
                || !results.artists.is_empty()
                || !results.playlists.is_empty())
        {
            return Ok(results);
        }

        // 2. Secondary fallback: spclient context resolver for tracks
        if let Ok(results) = self.search_via_spclient_context(trimmed).await
            && !results.tracks.is_empty()
        {
            return Ok(results);
        }

        // 3. Final fallback: public Web API /v1/search
        self.search_via_web_api(trimmed).await
    }

    pub async fn fetch_collection_tracks(
        &self,
        kind: CollectionKind,
        uri: &str,
        title: &str,
    ) -> Result<Vec<TrackMetadata>, BoxError> {
        match kind {
            CollectionKind::Album => self.get_album_tracks(uri, title).await,
            CollectionKind::Artist => self.get_artist_tracks(uri).await,
            CollectionKind::Playlist => self.get_playlist_tracks(uri).await,
        }
    }

    pub async fn get_album_tracks(
        &self,
        album_uri: &str,
        album_name: &str,
    ) -> Result<Vec<TrackMetadata>, BoxError> {
        // 1. Primary: native librespot metadata over spclient (never hits 429)
        if let Ok(parsed_uri) = SpotifyUri::from_uri(album_uri)
            && let Ok(album) = Album::get(&self.session, &parsed_uri).await
        {
            let resolved_album_name = if album.name.is_empty() {
                album_name.to_string()
            } else {
                album.name.clone()
            };
            let track_uris: Vec<SpotifyUri> = album.tracks().take(50).cloned().collect();
            if let Ok(tracks) = self
                .fetch_tracks_via_metadata(&track_uris, Some(&resolved_album_name))
                .await
                && !tracks.is_empty()
            {
                return Ok(tracks);
            }
        }

        // 2. Fallback: Web API
        let id = extract_spotify_id(album_uri);
        let token = self.get_access_token().await?;
        let url = format!("{SPOTIFY_API_BASE}/albums/{id}/tracks?limit=50");
        let resp = self.http.get(&url).bearer_auth(&token).send().await?;
        if !resp.status().is_success() {
            return Err(format!("Failed to load album tracks ({})", resp.status()).into());
        }
        let json: Value = resp.json().await?;
        Ok(parse_album_tracks_response(&json, album_name))
    }

    pub async fn get_artist_tracks(&self, artist_uri: &str) -> Result<Vec<TrackMetadata>, BoxError> {
        // 1. Primary: native librespot metadata over spclient (never hits 429)
        if let Ok(parsed_uri) = SpotifyUri::from_uri(artist_uri)
            && let Ok(artist) = Artist::get(&self.session, &parsed_uri).await
        {
            let country = self.session.country();
            let mut track_uris: Vec<SpotifyUri> = artist.top_tracks.for_country(&country).0;
            if track_uris.is_empty() {
                track_uris = artist
                    .top_tracks
                    .iter()
                    .flat_map(|tt| tt.tracks.0.iter())
                    .take(20)
                    .cloned()
                    .collect();
            } else {
                track_uris.truncate(20);
            }
            track_uris.dedup();

            if let Ok(tracks) = self.fetch_tracks_via_metadata(&track_uris, None).await
                && !tracks.is_empty()
            {
                return Ok(tracks);
            }
        }

        // 2. Fallback: Web API
        let id = extract_spotify_id(artist_uri);
        let token = self.get_access_token().await?;
        let url = format!("{SPOTIFY_API_BASE}/artists/{id}/top-tracks");
        let resp = self.http.get(&url).bearer_auth(&token).send().await?;
        if !resp.status().is_success() {
            return Err(format!("Failed to load artist top tracks ({})", resp.status()).into());
        }
        let json: Value = resp.json().await?;
        Ok(parse_artist_top_tracks_response(&json))
    }

    pub async fn get_playlist_tracks(
        &self,
        playlist_uri: &str,
    ) -> Result<Vec<TrackMetadata>, BoxError> {
        // 1. Primary: native librespot metadata over spclient (never hits 429)
        if let Ok(parsed_uri) = SpotifyUri::from_uri(playlist_uri)
            && let Ok(playlist) = Playlist::get(&self.session, &parsed_uri).await
        {
            let track_uris: Vec<SpotifyUri> = playlist.tracks().take(50).cloned().collect();
            if let Ok(tracks) = self.fetch_tracks_via_metadata(&track_uris, None).await
                && !tracks.is_empty()
            {
                return Ok(tracks);
            }
        }

        // 2. Fallback: Web API
        let id = extract_spotify_id(playlist_uri);
        let token = self.get_access_token().await?;
        let url = format!("{SPOTIFY_API_BASE}/playlists/{id}/tracks?limit=50");
        let resp = self.http.get(&url).bearer_auth(&token).send().await?;
        if !resp.status().is_success() {
            return Err(format!("Failed to load playlist tracks ({})", resp.status()).into());
        }
        let json: Value = resp.json().await?;
        Ok(parse_playlist_tracks_response(&json))
    }

    /// Fetch multiple tracks concurrently (preserving order) via `librespot::metadata::Track::get`.
    async fn fetch_tracks_via_metadata(
        &self,
        uris: &[SpotifyUri],
        fallback_album: Option<&str>,
    ) -> Result<Vec<TrackMetadata>, BoxError> {
        let fallback_owned = fallback_album.map(str::to_string);
        let session = self.session.clone();

        let results: Vec<Option<TrackMetadata>> = stream::iter(uris.iter().cloned())
            .map(|uri| {
                let session = session.clone();
                let fallback_album = fallback_owned.clone();
                async move {
                    let track = Track::get(&session, &uri).await.ok()?;
                    let artist = track
                        .artists
                        .iter()
                        .map(|a| a.name.clone())
                        .collect::<Vec<_>>()
                        .join(", ");
                    let album = if track.album.name.is_empty() {
                        fallback_album.unwrap_or_else(|| "Unknown Album".to_string())
                    } else {
                        track.album.name
                    };
                    Some(TrackMetadata {
                        uri: uri.to_uri(),
                        title: track.name,
                        artist: if artist.is_empty() {
                            "Unknown Artist".to_string()
                        } else {
                            artist
                        },
                        album,
                        duration_ms: track.duration as u32,
                    })
                }
            })
            .buffered(16)
            .collect()
            .await;

        Ok(results.into_iter().flatten().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_extract_spotify_id() {
        assert_eq!(
            extract_spotify_id("spotify:album:4aawyAB9vmqN3uQ7FjRGTy"),
            "4aawyAB9vmqN3uQ7FjRGTy"
        );
        assert_eq!(
            extract_spotify_id("spotify:artist:0OdUWJ0sBjDrqHygGUXeCF"),
            "0OdUWJ0sBjDrqHygGUXeCF"
        );
        assert_eq!(
            extract_spotify_id("spotify:playlist:37i9dQZF1DXcBWIGoYBM5M"),
            "37i9dQZF1DXcBWIGoYBM5M"
        );
        assert_eq!(extract_spotify_id("rawId123"), "rawId123");
    }

    #[test]
    fn test_urlencode_query() {
        assert_eq!(urlencode_query("daft punk"), "daft%20punk");
        assert_eq!(
            urlencode_query("Океан Ельзи"),
            "%D0%9E%D0%BA%D0%B5%D0%B0%D0%BD%20%D0%95%D0%BB%D1%8C%D0%B7%D0%B8"
        );
        assert_eq!(urlencode_query("AC/DC & Friends"), "AC%2FDC%20%26%20Friends");
    }

    #[test]
    fn test_parse_pathfinder_search_response() {
        let sample = json!({
            "data": {
                "searchV2": {
                    "tracksV2": {
                        "items": [
                            {
                                "item": {
                                    "data": {
                                        "uri": "spotify:track:03sEzk1VyrUZSgyhoQR0LZ",
                                        "name": "Pneuma",
                                        "duration": { "totalMilliseconds": 713191 },
                                        "albumOfTrack": { "name": "Fear Inoculum" },
                                        "artists": {
                                            "items": [
                                                { "profile": { "name": "TOOL" } }
                                            ]
                                        }
                                    }
                                }
                            }
                        ]
                    },
                    "albumsV2": {
                        "items": [
                            {
                                "data": {
                                    "uri": "spotify:album:5l5m1hnH4punS1GQXgEi3T",
                                    "name": "Lateralus",
                                    "date": { "year": 2001 },
                                    "artists": {
                                        "items": [
                                            { "profile": { "name": "TOOL" } }
                                        ]
                                    }
                                }
                            }
                        ]
                    },
                    "artists": {
                        "items": [
                            {
                                "data": {
                                    "uri": "spotify:artist:2yEwvVSSSUkcLeSTNyHKh8",
                                    "profile": { "name": "TOOL" }
                                }
                            }
                        ]
                    },
                    "playlists": {
                        "items": [
                            {
                                "data": {
                                    "uri": "spotify:playlist:6yuewXyMJIg0oVeCwUKdxW",
                                    "name": "Tool Best of",
                                    "ownerV2": {
                                        "data": { "name": "Fat Neck McGee" }
                                    }
                                }
                            }
                        ]
                    }
                }
            }
        });

        let res = parse_search_response(&sample);
        assert_eq!(res.tracks.len(), 1);
        assert_eq!(res.tracks[0].title, "Pneuma");
        assert_eq!(res.tracks[0].artist, "TOOL");
        assert_eq!(res.tracks[0].album, "Fear Inoculum");
        assert_eq!(res.tracks[0].duration_ms, 713191);

        assert_eq!(res.albums.len(), 1);
        assert_eq!(res.albums[0].name, "Lateralus");
        assert_eq!(res.albums[0].artist, "TOOL");
        assert_eq!(res.albums[0].year, Some(2001));

        assert_eq!(res.artists.len(), 1);
        assert_eq!(res.artists[0].name, "TOOL");

        assert_eq!(res.playlists.len(), 1);
        assert_eq!(res.playlists[0].name, "Tool Best of");
        assert_eq!(res.playlists[0].owner, "Fat Neck McGee");
    }

    #[test]
    fn test_parse_search_response_handles_all_categories_and_nulls() {
        let sample = json!({
            "tracks": {
                "items": [
                    null,
                    {
                        "id": "t1",
                        "uri": "spotify:track:t1",
                        "name": "Starlight",
                        "duration_ms": 240000,
                        "artists": [{ "name": "Muse" }],
                        "album": { "name": "Black Holes and Revelations" }
                    }
                ]
            },
            "albums": {
                "items": [
                    {
                        "id": "al1",
                        "uri": "spotify:album:al1",
                        "name": "Absolution",
                        "total_tracks": 14,
                        "release_date": "2003-09-15",
                        "artists": [{ "name": "Muse" }]
                    },
                    null
                ]
            },
            "artists": {
                "items": [
                    {
                        "id": "ar1",
                        "uri": "spotify:artist:ar1",
                        "name": "Muse"
                    }
                ]
            },
            "playlists": {
                "items": [
                    null,
                    {
                        "id": "pl1",
                        "uri": "spotify:playlist:pl1",
                        "name": "Rock Classics",
                        "owner": { "display_name": "Spotify" },
                        "tracks": { "total": 100 }
                    }
                ]
            }
        });

        let results = parse_search_response(&sample);
        assert_eq!(results.tracks.len(), 1);
        assert_eq!(results.tracks[0].uri, "spotify:track:t1");
        assert_eq!(results.tracks[0].title, "Starlight");
        assert_eq!(results.tracks[0].artist, "Muse");
        assert_eq!(results.tracks[0].album, "Black Holes and Revelations");
        assert_eq!(results.tracks[0].duration_ms, 240000);

        assert_eq!(results.albums.len(), 1);
        assert_eq!(results.albums[0].id, "al1");
        assert_eq!(results.albums[0].name, "Absolution");
        assert_eq!(results.albums[0].artist, "Muse");
        assert_eq!(results.albums[0].total_tracks, 14);
        assert_eq!(results.albums[0].year, Some(2003));

        assert_eq!(results.artists.len(), 1);
        assert_eq!(results.artists[0].id, "ar1");
        assert_eq!(results.artists[0].name, "Muse");

        assert_eq!(results.playlists.len(), 1);
        assert_eq!(results.playlists[0].id, "pl1");
        assert_eq!(results.playlists[0].name, "Rock Classics");
        assert_eq!(results.playlists[0].owner, "Spotify");
        assert_eq!(results.playlists[0].total_tracks, 100);
    }

    #[test]
    fn test_parse_search_response_empty_json() {
        let empty = json!({});
        let results = parse_search_response(&empty);
        assert!(results.tracks.is_empty());
        assert!(results.albums.is_empty());
        assert!(results.artists.is_empty());
        assert!(results.playlists.is_empty());
    }

    #[test]
    fn test_parse_album_tracks_response() {
        let json = json!({
            "items": [
                {
                    "id": "trk1",
                    "uri": "spotify:track:trk1",
                    "name": "Intro",
                    "duration_ms": 22000,
                    "artists": [{ "name": "Muse" }]
                },
                {
                    "id": "trk2",
                    "uri": "spotify:track:trk2",
                    "name": "Apocalypse Please",
                    "duration_ms": 252000,
                    "artists": [{ "name": "Muse" }, { "name": "Guest" }]
                }
            ]
        });

        let tracks = parse_album_tracks_response(&json, "Absolution");
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].title, "Intro");
        assert_eq!(tracks[0].album, "Absolution");
        assert_eq!(tracks[1].artist, "Muse, Guest");
    }

    #[test]
    fn test_parse_artist_top_tracks_response() {
        let json = json!({
            "tracks": [
                {
                    "id": "top1",
                    "uri": "spotify:track:top1",
                    "name": "Uprising",
                    "duration_ms": 304000,
                    "artists": [{ "name": "Muse" }],
                    "album": { "name": "The Resistance" }
                }
            ]
        });

        let tracks = parse_artist_top_tracks_response(&json);
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].uri, "spotify:track:top1");
        assert_eq!(tracks[0].title, "Uprising");
        assert_eq!(tracks[0].album, "The Resistance");
    }

    #[test]
    fn test_parse_playlist_tracks_response() {
        let json = json!({
            "items": [
                null,
                { "track": null },
                {
                    "track": {
                        "id": "pl_t1",
                        "uri": "spotify:track:pl_t1",
                        "name": "Hysteria",
                        "duration_ms": 227000,
                        "artists": [{ "name": "Muse" }],
                        "album": { "name": "Absolution" }
                    }
                }
            ]
        });

        let tracks = parse_playlist_tracks_response(&json);
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].uri, "spotify:track:pl_t1");
        assert_eq!(tracks[0].title, "Hysteria");
    }

    #[tokio::test]
    #[ignore]
    async fn live_integration_search_and_collections_test() {
        use crate::spotify::auth::init_cache;
        use librespot::core::config::SessionConfig;

        let cache = init_cache().expect("cache");
        let creds = cache.credentials().expect("cached creds");
        let session = Session::new(SessionConfig::default(), Some(cache));
        session.connect(creds, true).await.expect("connect");

        let client = SpotifyClient::new(session);
        let results = client.search("Tool").await.expect("live search 'Tool' should succeed");
        println!(
            "Live Search 'Tool': {} tracks, {} albums, {} artists, {} playlists",
            results.tracks.len(),
            results.albums.len(),
            results.artists.len(),
            results.playlists.len()
        );
        assert!(!results.tracks.is_empty(), "Tracks should not be empty");
        assert!(!results.albums.is_empty(), "Albums should not be empty");
        assert!(!results.artists.is_empty(), "Artists should not be empty");
        assert!(!results.playlists.is_empty(), "Playlists should not be empty");

        // Test loading Album tracks
        let album = &results.albums[0];
        let album_tracks = client
            .get_album_tracks(&album.uri, &album.name)
            .await
            .expect("get_album_tracks should succeed");
        println!("Album '{}' loaded {} tracks (first: '{}')", album.name, album_tracks.len(), album_tracks[0].title);
        assert!(!album_tracks.is_empty());

        // Test loading Artist top tracks
        let artist = &results.artists[0];
        let artist_tracks = client
            .get_artist_tracks(&artist.uri)
            .await
            .expect("get_artist_tracks should succeed");
        println!("Artist '{}' loaded {} top tracks (first: '{}')", artist.name, artist_tracks.len(), artist_tracks[0].title);
        assert!(!artist_tracks.is_empty());

        // Test loading Playlist tracks
        let playlist = &results.playlists[0];
        let playlist_tracks = client
            .get_playlist_tracks(&playlist.uri)
            .await
            .expect("get_playlist_tracks should succeed");
        println!(
            "Playlist '{}' loaded {} tracks (first: '{}')",
            playlist.name,
            playlist_tracks.len(),
            playlist_tracks[0].title
        );
        assert!(!playlist_tracks.is_empty());

        // Edge case 1: Empty / whitespace query
        let empty_res = client.search("   ").await.expect("empty search should succeed");
        assert!(empty_res.tracks.is_empty());
        assert!(empty_res.albums.is_empty());
        assert!(empty_res.artists.is_empty());
        assert!(empty_res.playlists.is_empty());

        // Edge case 2: Cyrillic query ("Океан Ельзи")
        let cyrillic_res = client
            .search("Океан Ельзи")
            .await
            .expect("Cyrillic search 'Океан Ельзи' should succeed");
        println!(
            "Live Search 'Океан Ельзи': {} tracks, {} albums, {} artists, {} playlists",
            cyrillic_res.tracks.len(),
            cyrillic_res.albums.len(),
            cyrillic_res.artists.len(),
            cyrillic_res.playlists.len()
        );
        assert!(!cyrillic_res.tracks.is_empty());
        assert!(!cyrillic_res.albums.is_empty());
        assert!(!cyrillic_res.artists.is_empty());

        // Edge case 3: Special characters & rapid repeated queries (no 429 rate-limit)
        for q in ["AC/DC & Friends", "Pink Floyd", "Tool - Schism"] {
            let r = client
                .search(q)
                .await
                .unwrap_or_else(|e| panic!("Rapid search '{q}' failed: {e}"));
            println!(
                "Rapid Search '{q}': {} tracks, {} albums, {} artists, {} playlists",
                r.tracks.len(),
                r.albums.len(),
                r.artists.len(),
                r.playlists.len()
            );
            assert!(!r.tracks.is_empty(), "Tracks for '{q}' should not be empty");
        }
    }
}
