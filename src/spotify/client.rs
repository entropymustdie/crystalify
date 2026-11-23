use crate::spotify::models::{
    AlbumItem, ArtistItem, CollectionKind, LibraryData, PlaylistItem, SearchResults,
    SyncedLyricLine, TrackLyrics, TrackMetadata,
};
use futures_util::stream::{self, StreamExt};
use librespot::core::FileId;
use librespot::core::session::Session;
use librespot::core::spotify_id::SpotifyId;
use librespot::core::spotify_uri::SpotifyUri;
use librespot::metadata::audio::item::CoverImage;
use librespot::metadata::image::Images;
use librespot::metadata::{Album, Artist, Metadata, Playlist, Track};
use librespot::protocol::extended_metadata::{
    BatchedEntityRequest, BatchedExtensionResponse, EntityRequest, ExtensionQuery,
};
use librespot::protocol::extension_kind::ExtensionKind;
use librespot::protocol::metadata as proto_metadata;
use librespot::protocol::playlist4_external::SelectedListContent;
use protobuf::{CodedInputStream, CodedOutputStream, EnumOrUnknown, Message};
use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderMap, HeaderValue};
use reqwest::{Client, Method};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::env;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

const SPOTIFY_API_BASE: &str = "https://api.spotify.com/v1";
const PATHFINDER_URL: &str = "https://api-partner.spotify.com/pathfinder/v1/query";
const LRCLIB_API_BASE: &str = "https://lrclib.net/api";
const LRCLIB_USER_AGENT: &str = concat!(
    "crystalify/",
    env!("CARGO_PKG_VERSION"),
    " (https://github.com/entropymustdie/crystalify)"
);
const SEARCH_DESKTOP_HASH: &str =
    "fcad5a3e0d5af727fb76966f06971c19cfa2275e6ff7671196753e008611873c";
const METADATA_BATCH_SIZE: usize = 80;
const PLAYLIST_PAGE_SIZE: usize = 100;

pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Debug, Clone)]
pub struct CollectionBatchUpdate {
    pub tracks: Vec<TrackMetadata>,
    pub total_expected: Option<usize>,
    pub is_first: bool,
    pub is_done: bool,
}

#[derive(Default)]
struct MetadataCache {
    tracks: HashMap<String, TrackMetadata>,
    albums: HashMap<String, AlbumItem>,
    collections: HashMap<String, Vec<TrackMetadata>>,
}

#[derive(Clone)]
pub struct SpotifyClient {
    session: Session,
    http: Client,
    cache: Arc<RwLock<MetadataCache>>,
}

pub fn extract_spotify_id(uri_or_id: &str) -> &str {
    let trimmed = uri_or_id.trim();
    trimmed.rsplit(':').next().unwrap_or(trimmed)
}

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

pub fn select_best_image_url_from_array(arr: &[Value]) -> Option<String> {
    let mut best_ge_64: Option<(u64, &str)> = None;
    let mut fallback: Option<&str> = None;

    for img in arr {
        let Some(url) = img
            .get("url")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        if fallback.is_none() {
            fallback = Some(url);
        }
        let dim = img
            .get("width")
            .and_then(Value::as_u64)
            .or_else(|| img.get("height").and_then(Value::as_u64))
            .unwrap_or(0);
        if dim >= 64 {
            match best_ge_64 {
                None => best_ge_64 = Some((dim, url)),
                Some((best_dim, _)) if dim < best_dim => best_ge_64 = Some((dim, url)),
                _ => {}
            }
        }
    }

    best_ge_64
        .map(|(_, u)| u.to_string())
        .or_else(|| fallback.map(str::to_string))
}

pub fn select_cover_url_from_images(images: &Images) -> Option<String> {
    let mut best_ge_64: Option<(i32, String)> = None;
    let mut fallback: Option<String> = None;

    for img in images.iter() {
        let hex = img.id.to_base16();
        if hex.is_empty() {
            continue;
        }
        let url = format!("https://i.scdn.co/image/{hex}");
        if fallback.is_none() {
            fallback = Some(url.clone());
        }
        let dim = img.width.max(img.height);
        if dim >= 64 {
            match best_ge_64 {
                None => best_ge_64 = Some((dim, url)),
                Some((best_dim, _)) if dim < best_dim => best_ge_64 = Some((dim, url)),
                _ => {}
            }
        }
    }

    best_ge_64.map(|(_, u)| u).or(fallback)
}

pub fn select_cover_url_from_proto_images(images: &[proto_metadata::Image]) -> Option<String> {
    let mut best_ge_64: Option<(i32, String)> = None;
    let mut fallback: Option<String> = None;

    for img in images {
        if img.file_id().is_empty() {
            continue;
        }
        let hex = FileId::from(img).to_base16();
        if hex.is_empty() || hex.bytes().all(|b| b == b'0') {
            continue;
        }
        let url = format!("https://i.scdn.co/image/{hex}");
        if fallback.is_none() {
            fallback = Some(url.clone());
        }
        let dim = img.width().max(img.height());
        if dim >= 64 {
            match best_ge_64 {
                None => best_ge_64 = Some((dim, url)),
                Some((best_dim, _)) if dim < best_dim => best_ge_64 = Some((dim, url)),
                _ => {}
            }
        }
    }

    best_ge_64.map(|(_, u)| u).or(fallback)
}

pub fn track_metadata_from_proto(
    uri: &str,
    track: &proto_metadata::Track,
    fallback_album: Option<&str>,
) -> Option<TrackMetadata> {
    let title = track.name().trim();
    if title.is_empty() {
        return None;
    }
    let mut artist = track
        .artist
        .iter()
        .map(|a| a.name().trim())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(", ");
    if artist.is_empty() {
        artist = track
            .artist_with_role
            .iter()
            .map(|a| a.artist_name().trim())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(", ");
    }
    if artist.is_empty() {
        artist = "Unknown Artist".to_string();
    }

    let album_name = track
        .album
        .as_ref()
        .map(|a| a.name().trim())
        .filter(|s| !s.is_empty())
        .or_else(|| fallback_album.map(str::trim).filter(|s| !s.is_empty()))
        .unwrap_or("Unknown Album")
        .to_string();

    let cover_url = track.album.as_ref().and_then(|al| {
        select_cover_url_from_proto_images(&al.cover).or_else(|| {
            al.cover_group
                .as_ref()
                .and_then(|cg| select_cover_url_from_proto_images(&cg.image))
        })
    });

    let duration_ms = track.duration().max(0) as u32;

    Some(TrackMetadata {
        uri: uri.to_string(),
        title: title.to_string(),
        artist,
        album: album_name,
        duration_ms,
        cover_url,
    })
}

pub fn album_item_from_proto(uri: &str, album: &proto_metadata::Album) -> Option<AlbumItem> {
    let name = album.name().trim();
    if name.is_empty() {
        return None;
    }
    let artist = album
        .artist
        .iter()
        .map(|a| a.name().trim())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(", ");
    let artist_out = if artist.is_empty() {
        "Unknown Artist".to_string()
    } else {
        artist
    };
    let total_tracks: u32 = album.disc.iter().map(|d| d.track.len() as u32).sum();
    let year = album.date.as_ref().and_then(|d| {
        let y = d.year();
        if y > 0 { Some(y as u32) } else { None }
    });
    Some(AlbumItem {
        uri: uri.to_string(),
        id: extract_spotify_id(uri).to_string(),
        name: name.to_string(),
        artist: artist_out,
        total_tracks,
        year,
    })
}

pub fn parse_batched_tracks_response(
    resp: &BatchedExtensionResponse,
    ordered_uris: &[String],
    fallback_album: Option<&str>,
) -> Vec<TrackMetadata> {
    let mut map: HashMap<String, TrackMetadata> = HashMap::with_capacity(ordered_uris.len());
    for array in &resp.extended_metadata {
        for entry in &array.extension_data {
            let entity_uri = entry.entity_uri.trim();
            if entity_uri.is_empty() {
                continue;
            }
            let Some(any) = entry.extension_data.as_ref() else {
                continue;
            };
            if any.value.is_empty() {
                continue;
            }
            if let Ok(proto_track) = proto_metadata::Track::parse_from_bytes(&any.value)
                && let Some(meta) = track_metadata_from_proto(entity_uri, &proto_track, fallback_album)
            {
                map.insert(entity_uri.to_string(), meta);
            }
        }
    }

    ordered_uris
        .iter()
        .filter_map(|u| map.get(u).cloned())
        .collect()
}

pub fn parse_batched_albums_response(
    resp: &BatchedExtensionResponse,
    ordered_uris: &[String],
) -> Vec<AlbumItem> {
    let mut map: HashMap<String, AlbumItem> = HashMap::with_capacity(ordered_uris.len());
    for array in &resp.extended_metadata {
        for entry in &array.extension_data {
            let entity_uri = entry.entity_uri.trim();
            if entity_uri.is_empty() {
                continue;
            }
            let Some(any) = entry.extension_data.as_ref() else {
                continue;
            };
            if any.value.is_empty() {
                continue;
            }
            if let Ok(proto_album) = proto_metadata::Album::parse_from_bytes(&any.value)
                && let Some(item) = album_item_from_proto(entity_uri, &proto_album)
            {
                map.insert(entity_uri.to_string(), item);
            }
        }
    }

    ordered_uris
        .iter()
        .filter_map(|u| map.get(u).cloned())
        .collect()
}

pub fn parse_playlist_page_uris(
    page: &SelectedListContent,
) -> (Vec<SpotifyUri>, usize, usize, bool) {
    let total_length = page.length.unwrap_or(0).max(0) as usize;
    let Some(contents) = page.contents.as_ref() else {
        return (Vec::new(), 0, total_length, false);
    };
    let raw_items_len = contents.items.len();
    let is_truncated = contents.truncated();
    let mut uris = Vec::with_capacity(raw_items_len);
    for item in &contents.items {
        if let Some(raw_uri) = item.uri.as_deref() {
            let trimmed = raw_uri.trim();
            if trimmed.starts_with("spotify:track:")
                && let Ok(parsed) = SpotifyUri::from_uri(trimmed)
            {
                uris.push(parsed);
            }
        }
    }
    (uris, raw_items_len, total_length, is_truncated)
}

pub fn select_best_cover_url_from_audio_item(covers: &[CoverImage]) -> Option<String> {
    let mut best_ge_64: Option<(i32, String)> = None;
    let mut fallback: Option<String> = None;

    for img in covers {
        let url = img.url.trim();
        if url.is_empty() {
            continue;
        }
        if fallback.is_none() {
            fallback = Some(url.to_string());
        }
        let dim = img.width.max(img.height);
        if dim >= 64 {
            match best_ge_64 {
                None => best_ge_64 = Some((dim, url.to_string())),
                Some((best_dim, _)) if dim < best_dim => best_ge_64 = Some((dim, url.to_string())),
                _ => {}
            }
        }
    }

    best_ge_64.map(|(_, u)| u).or(fallback)
}

pub fn encode_collection_v2_page_request(
    username: &str,
    set_name: &str,
    pagination_token: &str,
    limit: i32,
) -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let mut cos = CodedOutputStream::vec(&mut buf);
        if !username.is_empty() {
            let _ = cos.write_string(1, username);
        }
        if !set_name.is_empty() {
            let _ = cos.write_string(2, set_name);
        }
        if !pagination_token.is_empty() {
            let _ = cos.write_string(3, pagination_token);
        }
        if limit > 0 {
            let _ = cos.write_int32(4, limit);
        }
        let _ = cos.flush();
    }
    buf
}

fn skip_proto_wire_value(cis: &mut CodedInputStream, wire_type: u32) -> Result<bool, BoxError> {
    match wire_type {
        0 => {
            let _ = cis.read_raw_varint64()?;
            Ok(true)
        }
        1 => {
            let _ = cis.read_fixed64()?;
            Ok(true)
        }
        2 => {
            let len = cis.read_raw_varint32()?;
            cis.skip_raw_bytes(len)?;
            Ok(true)
        }
        5 => {
            let _ = cis.read_fixed32()?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

fn parse_collection_v2_item_proto(bytes: &[u8]) -> Result<Option<(String, i32)>, BoxError> {
    let mut cis = CodedInputStream::from_bytes(bytes);
    let mut uri = String::new();
    let mut added_at: i32 = 0;
    let mut is_removed = false;

    while let Some(tag) = cis.read_raw_tag_or_eof()? {
        if tag == 0 {
            break;
        }
        let field_num = tag >> 3;
        let wire_type = tag & 0x07;
        match (field_num, wire_type) {
            (1, 2) => {
                uri = cis.read_string()?;
            }
            (2, 0) => {
                added_at = cis.read_int32()?;
            }
            (3, 0) => {
                is_removed = cis.read_bool()?;
            }
            _ => {
                if !skip_proto_wire_value(&mut cis, wire_type)? {
                    break;
                }
            }
        }
    }

    if is_removed || uri.trim().is_empty() {
        Ok(None)
    } else {
        Ok(Some((uri, added_at)))
    }
}

pub fn parse_collection_v2_proto(bytes: &[u8]) -> Result<(Vec<(String, i32)>, String), BoxError> {
    let mut cis = CodedInputStream::from_bytes(bytes);
    let mut items = Vec::new();
    let mut next_page_token = String::new();

    while let Some(tag) = cis.read_raw_tag_or_eof()? {
        if tag == 0 {
            break;
        }
        let field_num = tag >> 3;
        let wire_type = tag & 0x07;
        match (field_num, wire_type) {
            (1, 2) => {
                let item_bytes = cis.read_bytes()?;
                if let Some(parsed) = parse_collection_v2_item_proto(&item_bytes)? {
                    items.push(parsed);
                }
            }
            (2, 2) => {
                next_page_token = cis.read_string()?;
            }
            _ => {
                if !skip_proto_wire_value(&mut cis, wire_type)? {
                    break;
                }
            }
        }
    }

    Ok((items, next_page_token))
}

pub fn parse_collection_v2_json(json: &Value) -> Vec<(String, i32)> {
    let Some(arr) = json.get("items").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in arr {
        if entry.get("is_removed").and_then(Value::as_bool) == Some(true)
            || entry.get("isRemoved").and_then(Value::as_bool) == Some(true)
        {
            continue;
        }
        let Some(uri) = entry
            .get("uri")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
        else {
            continue;
        };
        let added_at = entry
            .get("added_at")
            .or_else(|| entry.get("addedAt"))
            .and_then(|v| {
                v.as_i64()
                    .or_else(|| v.as_str().and_then(|s| s.parse::<i64>().ok()))
            })
            .unwrap_or(0) as i32;
        out.push((uri.to_string(), added_at));
    }
    out
}

pub fn parse_rootlist_playlists(
    rootlist: &SelectedListContent,
) -> Vec<(String, Option<PlaylistItem>)> {
    let Some(contents) = rootlist.contents.as_ref() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (idx, item) in contents.items.iter().enumerate() {
        let Some(raw_uri) = item.uri.as_deref() else {
            continue;
        };
        let trimmed = raw_uri.trim();
        if trimmed.is_empty()
            || trimmed.starts_with("spotify:start-group:")
            || trimmed.starts_with("spotify:end-group:")
        {
            continue;
        }
        let id = extract_spotify_id(trimmed);
        let is_playlist = trimmed.starts_with("spotify:playlist:")
            || (trimmed.starts_with("spotify:user:") && trimmed.contains(":playlist:"));
        if !is_playlist || id.is_empty() {
            continue;
        }
        let normalized_uri = format!("spotify:playlist:{id}");
        let resolved_meta = contents.meta_items.get(idx).and_then(|meta| {
            let name = meta
                .attributes
                .as_ref()
                .and_then(|a| a.name.as_deref())
                .map(str::trim)
                .filter(|s| !s.is_empty())?;
            let owner = meta
                .owner_username
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or("Spotify")
                .to_string();
            let total_tracks = meta.length.unwrap_or(0).max(0) as u32;
            Some(PlaylistItem {
                uri: normalized_uri.clone(),
                id: id.to_string(),
                name: name.to_string(),
                owner,
                total_tracks,
            })
        });
        out.push((normalized_uri, resolved_meta));
    }
    out
}

pub fn parse_web_me_playlists(json: &Value) -> Vec<PlaylistItem> {
    let Some(items) = json.get("items").and_then(Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|pl| {
            if pl.is_null() {
                return None;
            }
            let id = pl.get("id").and_then(Value::as_str).unwrap_or_default();
            let uri = pl
                .get("uri")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| {
                    if !id.is_empty() {
                        format!("spotify:playlist:{id}")
                    } else {
                        String::new()
                    }
                });
            if uri.is_empty() {
                return None;
            }
            let resolved_id = if id.is_empty() {
                extract_spotify_id(&uri).to_string()
            } else {
                id.to_string()
            };
            let name = pl
                .get("name")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())?
                .to_string();
            let owner = pl
                .get("owner")
                .and_then(|o| {
                    o.get("display_name")
                        .and_then(Value::as_str)
                        .or_else(|| o.get("id").and_then(Value::as_str))
                })
                .unwrap_or("Spotify")
                .to_string();
            let total_tracks = pl
                .get("tracks")
                .or_else(|| pl.get("items"))
                .and_then(|t| t.get("total"))
                .and_then(Value::as_u64)
                .unwrap_or(0) as u32;
            Some(PlaylistItem {
                uri,
                id: resolved_id,
                name,
                owner,
                total_tracks,
            })
        })
        .collect()
}

pub fn parse_web_me_tracks(json: &Value) -> Vec<TrackMetadata> {
    let Some(items) = json.get("items").and_then(Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|entry| {
            let track_obj = entry.get("track").unwrap_or(entry);
            parse_track_item(track_obj, None)
        })
        .collect()
}

pub fn parse_web_me_albums(json: &Value) -> Vec<AlbumItem> {
    let Some(items) = json.get("items").and_then(Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|entry| {
            let al = entry.get("album").unwrap_or(entry);
            if al.is_null() {
                return None;
            }
            let id = al.get("id").and_then(Value::as_str).unwrap_or_default();
            let uri = al
                .get("uri")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| {
                    if !id.is_empty() {
                        format!("spotify:album:{id}")
                    } else {
                        String::new()
                    }
                });
            if uri.is_empty() {
                return None;
            }
            let resolved_id = if id.is_empty() {
                extract_spotify_id(&uri).to_string()
            } else {
                id.to_string()
            };
            let name = al
                .get("name")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())?
                .to_string();
            let artist = parse_artists_list(al);
            let total_tracks = al
                .get("total_tracks")
                .and_then(Value::as_u64)
                .unwrap_or(0) as u32;
            let year = al
                .get("release_date")
                .and_then(Value::as_str)
                .and_then(|s| s.get(0..4))
                .and_then(|y| y.parse::<u32>().ok());
            Some(AlbumItem {
                uri,
                id: resolved_id,
                name,
                artist,
                total_tracks,
                year,
            })
        })
        .collect()
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

    let cover_url = item
        .get("album")
        .and_then(|a| a.get("images"))
        .and_then(Value::as_array)
        .and_then(|arr| select_best_image_url_from_array(arr));

    Some(TrackMetadata {
        uri,
        title,
        artist,
        album,
        duration_ms,
        cover_url,
    })
}

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
                    let cover_url = data
                        .get("albumOfTrack")
                        .and_then(|a| a.get("coverArt"))
                        .and_then(|c| c.get("sources"))
                        .and_then(Value::as_array)
                        .and_then(|arr| select_best_image_url_from_array(arr));

                    Some(TrackMetadata {
                        uri,
                        title,
                        artist,
                        album,
                        duration_ms,
                        cover_url,
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
        crate::spotify::init_tls_provider();
        let http = Client::builder()
            .pool_max_idle_per_host(16)
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(15))
            .build()
            .unwrap_or_else(|_| Client::new());
        Self {
            session,
            http,
            cache: Arc::new(RwLock::new(MetadataCache::default())),
        }
    }

    pub fn clear_collection_cache(&self) {
        if let Ok(mut guard) = self.cache.write() {
            guard.collections.clear();
        }
    }

    pub fn get_cached_collection(&self, uri: &str) -> Option<Vec<TrackMetadata>> {
        if uri.is_empty() {
            return None;
        }
        self.cache
            .read()
            .ok()
            .and_then(|guard| guard.collections.get(uri).cloned())
    }

    pub fn cache_collection(&self, uri: String, tracks: Vec<TrackMetadata>) {
        if uri.is_empty() || tracks.is_empty() {
            return;
        }
        if let Ok(mut guard) = self.cache.write() {
            guard.collections.insert(uri, tracks);
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
        let tracks = self.fetch_tracks_batched(&track_uris, None).await?;
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

        if let Ok(results) = self.search_via_pathfinder(trimmed).await
            && (!results.tracks.is_empty()
                || !results.albums.is_empty()
                || !results.artists.is_empty()
                || !results.playlists.is_empty())
        {
            return Ok(results);
        }

        if let Ok(results) = self.search_via_spclient_context(trimmed).await
            && !results.tracks.is_empty()
        {
            return Ok(results);
        }

        self.search_via_web_api(trimmed).await
    }

    #[allow(dead_code)]
    pub async fn fetch_collection_tracks(
        &self,
        kind: CollectionKind,
        uri: &str,
        title: &str,
    ) -> Result<Vec<TrackMetadata>, BoxError> {
        if let Some(cached) = self.get_cached_collection(uri) {
            return Ok(cached);
        }
        let tracks = match kind {
            CollectionKind::Album => self.get_album_tracks(uri, title).await?,
            CollectionKind::Artist => self.get_artist_tracks(uri).await?,
            CollectionKind::Playlist => self.get_playlist_tracks(uri).await?,
        };
        self.cache_collection(uri.to_string(), tracks.clone());
        Ok(tracks)
    }

    async fn fetch_playlist_page(
        &self,
        playlist_id: &SpotifyId,
        from: usize,
        length: usize,
    ) -> Result<SelectedListContent, BoxError> {
        let endpoint = format!(
            "/playlist/v2/playlist/{}?from={from}&length={length}",
            playlist_id.to_base62()
        );
        match self
            .session
            .spclient()
            .request(&Method::GET, &endpoint, None, None)
            .await
        {
            Ok(bytes) => Ok(SelectedListContent::parse_from_bytes(&bytes)?),
            Err(e) => {
                if from == 0 {
                    let bytes = self.session.spclient().get_playlist(playlist_id).await?;
                    Ok(SelectedListContent::parse_from_bytes(&bytes)?)
                } else {
                    Err(e.into())
                }
            }
        }
    }

    pub async fn stream_collection_tracks<F>(
        &self,
        kind: CollectionKind,
        uri: &str,
        title: &str,
        cancel_token: Arc<AtomicU64>,
        expected_id: u64,
        on_batch: F,
    ) -> Result<(), BoxError>
    where
        F: Fn(CollectionBatchUpdate) + Send + Sync,
    {
        if cancel_token.load(Ordering::Relaxed) != expected_id {
            return Ok(());
        }
        if let Some(cached) = self.get_cached_collection(uri) {
            let total = cached.len();
            on_batch(CollectionBatchUpdate {
                tracks: cached,
                total_expected: Some(total),
                is_first: true,
                is_done: true,
            });
            return Ok(());
        }

        match kind {
            CollectionKind::Playlist => {
                self.stream_playlist_tracks(uri, cancel_token, expected_id, &on_batch)
                    .await
            }
            CollectionKind::Album => {
                self.stream_album_tracks(uri, title, cancel_token, expected_id, &on_batch)
                    .await
            }
            CollectionKind::Artist => {
                let tracks = self.get_artist_tracks(uri).await?;
                if cancel_token.load(Ordering::Relaxed) != expected_id {
                    return Ok(());
                }
                if tracks.is_empty() {
                    return Err("No playable tracks found".into());
                }
                let total = tracks.len();
                self.cache_collection(uri.to_string(), tracks.clone());
                on_batch(CollectionBatchUpdate {
                    tracks,
                    total_expected: Some(total),
                    is_first: true,
                    is_done: true,
                });
                Ok(())
            }
        }
    }

    async fn stream_album_tracks<F>(
        &self,
        album_uri: &str,
        album_name: &str,
        cancel_token: Arc<AtomicU64>,
        expected_id: u64,
        on_batch: &F,
    ) -> Result<(), BoxError>
    where
        F: Fn(CollectionBatchUpdate) + Send + Sync,
    {
        if let Ok(parsed_uri) = SpotifyUri::from_uri(album_uri)
            && let Ok(album) = Album::get(&self.session, &parsed_uri).await
        {
            if cancel_token.load(Ordering::Relaxed) != expected_id {
                return Ok(());
            }
            let resolved_album_name = if album.name.is_empty() {
                album_name.to_string()
            } else {
                album.name.clone()
            };
            let album_cover_url = select_cover_url_from_images(&album.covers)
                .or_else(|| select_cover_url_from_images(&album.cover_group));
            let track_uris: Vec<SpotifyUri> = album.tracks().cloned().collect();
            let total_uris = track_uris.len();

            if total_uris > 0 {
                let mut all_collected = Vec::with_capacity(total_uris);
                let mut is_first = true;
                let chunks: Vec<&[SpotifyUri]> = track_uris.chunks(80).collect();

                for (idx, chunk) in chunks.iter().enumerate() {
                    if cancel_token.load(Ordering::Relaxed) != expected_id {
                        return Ok(());
                    }
                    let mut batch_tracks = self
                        .fetch_tracks_batched(chunk, Some(&resolved_album_name))
                        .await
                        .unwrap_or_default();
                    if let Some(ref fallback_cover) = album_cover_url {
                        for t in &mut batch_tracks {
                            if t.cover_url.is_none() {
                                t.cover_url = Some(fallback_cover.clone());
                            }
                        }
                    }
                    let is_last = idx + 1 == chunks.len();
                    all_collected.extend(batch_tracks.clone());
                    if !batch_tracks.is_empty() || is_last {
                        on_batch(CollectionBatchUpdate {
                            tracks: batch_tracks,
                            total_expected: Some(total_uris),
                            is_first,
                            is_done: is_last,
                        });
                        is_first = false;
                    }
                }

                if !all_collected.is_empty() {
                    self.cache_collection(album_uri.to_string(), all_collected);
                    return Ok(());
                }
            }
        }

        let tracks = self.fetch_album_tracks_via_web_api(album_uri, album_name).await?;
        if cancel_token.load(Ordering::Relaxed) != expected_id {
            return Ok(());
        }
        if tracks.is_empty() {
            return Err("No playable tracks found in album".into());
        }
        let total = tracks.len();
        self.cache_collection(album_uri.to_string(), tracks.clone());
        on_batch(CollectionBatchUpdate {
            tracks,
            total_expected: Some(total),
            is_first: true,
            is_done: true,
        });
        Ok(())
    }

    async fn stream_playlist_tracks<F>(
        &self,
        playlist_uri: &str,
        cancel_token: Arc<AtomicU64>,
        expected_id: u64,
        on_batch: &F,
    ) -> Result<(), BoxError>
    where
        F: Fn(CollectionBatchUpdate) + Send + Sync,
    {
        let normalized_uri = if playlist_uri.starts_with("spotify:playlist:") {
            playlist_uri.to_string()
        } else {
            let id = extract_spotify_id(playlist_uri);
            if id.is_empty() {
                playlist_uri.to_string()
            } else {
                format!("spotify:playlist:{id}")
            }
        };

        if let Ok(SpotifyUri::Playlist { id: playlist_id, .. }) =
            SpotifyUri::from_uri(&normalized_uri)
            && let Ok(first_page) = self
                .fetch_playlist_page(&playlist_id, 0, PLAYLIST_PAGE_SIZE)
                .await
        {
            if cancel_token.load(Ordering::Relaxed) != expected_id {
                return Ok(());
            }
            let (page1_uris, raw_len, total_len, is_truncated) =
                parse_playlist_page_uris(&first_page);

            let has_more_pages = is_truncated && raw_len > 0 && (total_len == 0 || raw_len < total_len);
            let expected_total = if total_len > 0 {
                Some(total_len)
            } else if !has_more_pages {
                Some(page1_uris.len())
            } else {
                None
            };

            let mut all_collected: Vec<TrackMetadata> =
                Vec::with_capacity(expected_total.unwrap_or(page1_uris.len()));
            let mut is_first_emit = true;

            let first_split = page1_uris.len().min(50);
            if first_split > 0 {
                let first_chunk = &page1_uris[..first_split];
                if let Ok(tracks) = self.fetch_tracks_batched(first_chunk, None).await
                    && !tracks.is_empty()
                {
                    if cancel_token.load(Ordering::Relaxed) != expected_id {
                        return Ok(());
                    }
                    let is_done = first_split == page1_uris.len() && !has_more_pages;
                    all_collected.extend(tracks.clone());
                    on_batch(CollectionBatchUpdate {
                        tracks,
                        total_expected: expected_total,
                        is_first: true,
                        is_done,
                    });
                    is_first_emit = false;
                }
            }

            if page1_uris.len() > first_split {
                if cancel_token.load(Ordering::Relaxed) != expected_id {
                    return Ok(());
                }
                let second_chunk = &page1_uris[first_split..];
                if let Ok(tracks) = self.fetch_tracks_batched(second_chunk, None).await
                    && !tracks.is_empty()
                {
                    if cancel_token.load(Ordering::Relaxed) != expected_id {
                        return Ok(());
                    }
                    all_collected.extend(tracks.clone());
                    on_batch(CollectionBatchUpdate {
                        tracks,
                        total_expected: expected_total,
                        is_first: is_first_emit,
                        is_done: !has_more_pages,
                    });
                    is_first_emit = false;
                }
            }

            if has_more_pages {
                let first_page_first_uri = page1_uris.first().map(SpotifyUri::to_uri);
                let step = raw_len.max(PLAYLIST_PAGE_SIZE);

                if total_len > raw_len {
                    let max_total = total_len.min(20_000);
                    let offsets: Vec<usize> = (raw_len..max_total).step_by(step).collect();
                    let client_clone = self.clone();
                    let pid = playlist_id;
                    let cancel_for_stream = cancel_token.clone();

                    let mut page_stream = stream::iter(offsets)
                        .map(move |offset| {
                            let client = client_clone.clone();
                            let cancel = cancel_for_stream.clone();
                            async move {
                                if cancel.load(Ordering::Relaxed) != expected_id {
                                    return None;
                                }
                                let page = client
                                    .fetch_playlist_page(&pid, offset, step)
                                    .await
                                    .ok()?;
                                let (uris, _, _, _) = parse_playlist_page_uris(&page);
                                if uris.is_empty() {
                                    return None;
                                }
                                if cancel.load(Ordering::Relaxed) != expected_id {
                                    return None;
                                }
                                let tracks = client.fetch_tracks_batched(&uris, None).await.ok()?;
                                Some((uris, tracks))
                            }
                        })
                        .buffered(4);

                    while let Some(page_res) = page_stream.next().await {
                        if cancel_token.load(Ordering::Relaxed) != expected_id {
                            return Ok(());
                        }
                        if let Some((uris, tracks)) = page_res {
                            if let (Some(first_u), Some(page_first_u)) =
                                (&first_page_first_uri, uris.first().map(SpotifyUri::to_uri))
                                && first_u == &page_first_u
                                && uris.len() == page1_uris.len()
                            {
                                break;
                            }
                            if !tracks.is_empty() {
                                all_collected.extend(tracks.clone());
                                on_batch(CollectionBatchUpdate {
                                    tracks,
                                    total_expected: expected_total,
                                    is_first: is_first_emit,
                                    is_done: false,
                                });
                                is_first_emit = false;
                            }
                        }
                    }
                } else {
                    let mut from = raw_len;
                    while from < 20_000 {
                        if cancel_token.load(Ordering::Relaxed) != expected_id {
                            return Ok(());
                        }
                        let Ok(page) = self.fetch_playlist_page(&playlist_id, from, step).await
                        else {
                            break;
                        };
                        let (uris, page_raw_len, _, page_truncated) =
                            parse_playlist_page_uris(&page);
                        if page_raw_len == 0 || uris.is_empty() {
                            break;
                        }
                        if let (Some(first_u), Some(page_first_u)) =
                            (&first_page_first_uri, uris.first().map(SpotifyUri::to_uri))
                            && first_u == &page_first_u
                            && uris.len() == page1_uris.len()
                        {
                            break;
                        }
                        if let Ok(tracks) = self.fetch_tracks_batched(&uris, None).await
                            && !tracks.is_empty()
                        {
                            if cancel_token.load(Ordering::Relaxed) != expected_id {
                                return Ok(());
                            }
                            all_collected.extend(tracks.clone());
                            on_batch(CollectionBatchUpdate {
                                tracks,
                                total_expected: None,
                                is_first: is_first_emit,
                                is_done: !page_truncated,
                            });
                            is_first_emit = false;
                        }
                        from += page_raw_len;
                        if !page_truncated {
                            break;
                        }
                    }
                }

                if cancel_token.load(Ordering::Relaxed) != expected_id {
                    return Ok(());
                }
                if !all_collected.is_empty() {
                    let final_len = all_collected.len();
                    on_batch(CollectionBatchUpdate {
                        tracks: Vec::new(),
                        total_expected: Some(final_len),
                        is_first: is_first_emit,
                        is_done: true,
                    });
                }
            }

            if !all_collected.is_empty() {
                self.cache_collection(playlist_uri.to_string(), all_collected);
                return Ok(());
            }
        }

        let id = extract_spotify_id(playlist_uri);
        let token = self.get_access_token().await?;
        let mut next_url = Some(format!(
            "{SPOTIFY_API_BASE}/playlists/{id}/tracks?limit=100"
        ));
        let mut all_collected = Vec::new();
        let mut is_first_emit = true;

        while let Some(url) = next_url.take() {
            if cancel_token.load(Ordering::Relaxed) != expected_id {
                return Ok(());
            }
            let resp = self.http.get(&url).bearer_auth(&token).send().await?;
            if !resp.status().is_success() {
                if all_collected.is_empty() {
                    return Err(format!("Failed to load playlist tracks ({})", resp.status()).into());
                }
                break;
            }
            let json: Value = resp.json().await?;
            let total_expected = json
                .get("total")
                .and_then(Value::as_u64)
                .map(|t| t as usize);
            next_url = json
                .get("next")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            let tracks = parse_playlist_tracks_response(&json);
            let is_done = next_url.is_none();
            all_collected.extend(tracks.clone());
            if !tracks.is_empty() || is_done {
                on_batch(CollectionBatchUpdate {
                    tracks,
                    total_expected,
                    is_first: is_first_emit,
                    is_done,
                });
                is_first_emit = false;
            }
        }

        if !all_collected.is_empty() {
            self.cache_collection(playlist_uri.to_string(), all_collected);
        }
        Ok(())
    }

    async fn fetch_album_tracks_via_web_api(
        &self,
        album_uri: &str,
        album_name: &str,
    ) -> Result<Vec<TrackMetadata>, BoxError> {
        let id = extract_spotify_id(album_uri);
        let token = self.get_access_token().await?;
        let mut next_url = Some(format!("{SPOTIFY_API_BASE}/albums/{id}/tracks?limit=50"));
        let mut all_tracks = Vec::new();

        while let Some(url) = next_url.take() {
            let resp = self.http.get(&url).bearer_auth(&token).send().await?;
            if !resp.status().is_success() {
                if all_tracks.is_empty() {
                    return Err(format!("Failed to load album tracks ({})", resp.status()).into());
                }
                break;
            }
            let json: Value = resp.json().await?;
            next_url = json
                .get("next")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            all_tracks.extend(parse_album_tracks_response(&json, album_name));
        }

        Ok(all_tracks)
    }

    #[allow(dead_code)]
    pub async fn get_album_tracks(
        &self,
        album_uri: &str,
        album_name: &str,
    ) -> Result<Vec<TrackMetadata>, BoxError> {
        if let Some(cached) = self.get_cached_collection(album_uri) {
            return Ok(cached);
        }
        if let Ok(parsed_uri) = SpotifyUri::from_uri(album_uri)
            && let Ok(album) = Album::get(&self.session, &parsed_uri).await
        {
            let resolved_album_name = if album.name.is_empty() {
                album_name.to_string()
            } else {
                album.name.clone()
            };
            let album_cover_url = select_cover_url_from_images(&album.covers)
                .or_else(|| select_cover_url_from_images(&album.cover_group));
            let track_uris: Vec<SpotifyUri> = album.tracks().cloned().collect();
            if let Ok(mut tracks) = self
                .fetch_tracks_batched(&track_uris, Some(&resolved_album_name))
                .await
                && !tracks.is_empty()
            {
                if let Some(ref fallback_cover) = album_cover_url {
                    for t in &mut tracks {
                        if t.cover_url.is_none() {
                            t.cover_url = Some(fallback_cover.clone());
                        }
                    }
                }
                self.cache_collection(album_uri.to_string(), tracks.clone());
                return Ok(tracks);
            }
        }

        let tracks = self
            .fetch_album_tracks_via_web_api(album_uri, album_name)
            .await?;
        self.cache_collection(album_uri.to_string(), tracks.clone());
        Ok(tracks)
    }

    pub async fn fetch_image_bytes(&self, url: &str) -> Result<Vec<u8>, BoxError> {
        let resp = self.http.get(url).send().await?;
        if !resp.status().is_success() {
            return Err(format!("Failed to fetch cover image ({})", resp.status()).into());
        }
        let bytes = resp.bytes().await?;
        Ok(bytes.to_vec())
    }

    pub async fn get_artist_tracks(&self, artist_uri: &str) -> Result<Vec<TrackMetadata>, BoxError> {
        if let Some(cached) = self.get_cached_collection(artist_uri) {
            return Ok(cached);
        }
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

            if let Ok(tracks) = self.fetch_tracks_batched(&track_uris, None).await
                && !tracks.is_empty()
            {
                self.cache_collection(artist_uri.to_string(), tracks.clone());
                return Ok(tracks);
            }
        }

        let id = extract_spotify_id(artist_uri);
        let token = self.get_access_token().await?;
        let url = format!("{SPOTIFY_API_BASE}/artists/{id}/top-tracks");
        let resp = self.http.get(&url).bearer_auth(&token).send().await?;
        if !resp.status().is_success() {
            return Err(format!("Failed to load artist top tracks ({})", resp.status()).into());
        }
        let json: Value = resp.json().await?;
        let tracks = parse_artist_top_tracks_response(&json);
        self.cache_collection(artist_uri.to_string(), tracks.clone());
        Ok(tracks)
    }

    #[allow(dead_code)]
    pub async fn get_playlist_tracks(
        &self,
        playlist_uri: &str,
    ) -> Result<Vec<TrackMetadata>, BoxError> {
        if let Some(cached) = self.get_cached_collection(playlist_uri) {
            return Ok(cached);
        }
        let collected = Arc::new(RwLock::new(Vec::new()));
        let collected_ref = collected.clone();
        let cancel = Arc::new(AtomicU64::new(0));
        self.stream_playlist_tracks(playlist_uri, cancel, 0, &move |batch| {
            if !batch.tracks.is_empty()
                && let Ok(mut guard) = collected_ref.write()
            {
                guard.extend(batch.tracks);
            }
        })
        .await?;
        let tracks = collected
            .read()
            .map(|g| g.clone())
            .unwrap_or_default();
        Ok(tracks)
    }

    async fn fetch_track_chunk_extended(
        &self,
        chunk_uris: &[String],
        fallback_album: Option<&str>,
    ) -> Result<Vec<TrackMetadata>, BoxError> {
        let entity_request: Vec<EntityRequest> = chunk_uris
            .iter()
            .map(|uri| EntityRequest {
                entity_uri: uri.clone(),
                query: vec![ExtensionQuery {
                    extension_kind: EnumOrUnknown::new(ExtensionKind::TRACK_V4),
                    ..Default::default()
                }],
                ..Default::default()
            })
            .collect();

        let req = BatchedEntityRequest {
            entity_request,
            ..Default::default()
        };
        let resp = self
            .session
            .spclient()
            .get_extended_metadata(req)
            .await?;
        Ok(parse_batched_tracks_response(&resp, chunk_uris, fallback_album))
    }

    pub async fn fetch_tracks_batched(
        &self,
        uris: &[SpotifyUri],
        fallback_album: Option<&str>,
    ) -> Result<Vec<TrackMetadata>, BoxError> {
        if uris.is_empty() {
            return Ok(Vec::new());
        }

        let ordered_uri_strings: Vec<String> = uris.iter().map(SpotifyUri::to_uri).collect();
        let mut resolved_map: HashMap<String, TrackMetadata> =
            HashMap::with_capacity(ordered_uri_strings.len());
        let mut missing_uris: Vec<String> = Vec::new();
        let mut seen_missing: std::collections::HashSet<String> = std::collections::HashSet::new();

        if let Ok(guard) = self.cache.read() {
            for u in &ordered_uri_strings {
                if let Some(cached) = guard.tracks.get(u) {
                    resolved_map.insert(u.clone(), cached.clone());
                } else if seen_missing.insert(u.clone()) {
                    missing_uris.push(u.clone());
                }
            }
        } else {
            for u in &ordered_uri_strings {
                if seen_missing.insert(u.clone()) {
                    missing_uris.push(u.clone());
                }
            }
        }

        if !missing_uris.is_empty() {
            let chunks: Vec<Vec<String>> = missing_uris
                .chunks(METADATA_BATCH_SIZE)
                .map(|c| c.to_vec())
                .collect();
            let fallback_owned = fallback_album.map(str::to_string);
            let client_clone = self.clone();

            let chunk_results: Vec<Vec<TrackMetadata>> = stream::iter(chunks)
                .map(move |chunk| {
                    let client = client_clone.clone();
                    let fb = fallback_owned.clone();
                    async move {
                        if let Ok(batch) = client
                            .fetch_track_chunk_extended(&chunk, fb.as_deref())
                            .await
                            && !batch.is_empty()
                        {
                            return batch;
                        }
                        let mut recovered = Vec::new();
                        for sub in chunk.chunks(20) {
                            if let Ok(sub_batch) = client
                                .fetch_track_chunk_extended(sub, fb.as_deref())
                                .await
                                && !sub_batch.is_empty()
                            {
                                recovered.extend(sub_batch);
                            } else {
                                let sub_uris: Vec<SpotifyUri> = sub
                                    .iter()
                                    .filter_map(|s| SpotifyUri::from_uri(s).ok())
                                    .collect();
                                if let Ok(ind) = client
                                    .fetch_tracks_via_metadata(&sub_uris, fb.as_deref())
                                    .await
                                {
                                    recovered.extend(ind);
                                }
                            }
                        }
                        recovered
                    }
                })
                .buffered(4)
                .collect()
                .await;

            let mut newly_fetched = Vec::new();
            for batch in chunk_results {
                for track in batch {
                    resolved_map.insert(track.uri.clone(), track.clone());
                    newly_fetched.push(track);
                }
            }

            let still_missing: Vec<SpotifyUri> = missing_uris
                .iter()
                .filter(|u| !resolved_map.contains_key(*u))
                .filter_map(|u| SpotifyUri::from_uri(u).ok())
                .collect();
            if !still_missing.is_empty()
                && let Ok(fallback_tracks) = self
                    .fetch_tracks_via_metadata(&still_missing, fallback_album)
                    .await
            {
                for track in fallback_tracks {
                    resolved_map.insert(track.uri.clone(), track.clone());
                    newly_fetched.push(track);
                }
            }

            if !newly_fetched.is_empty()
                && let Ok(mut guard) = self.cache.write()
            {
                for track in newly_fetched {
                    guard.tracks.insert(track.uri.clone(), track);
                }
            }
        }

        let mut output = Vec::with_capacity(ordered_uri_strings.len());
        for u in &ordered_uri_strings {
            if let Some(mut meta) = resolved_map.get(u).cloned() {
                if let Some(fb_album) = fallback_album
                    && (meta.album.is_empty() || meta.album == "Unknown Album")
                    && !fb_album.trim().is_empty()
                {
                    meta.album = fb_album.to_string();
                }
                output.push(meta);
            }
        }

        Ok(output)
    }

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
                    let cover_url = select_cover_url_from_images(&track.album.covers)
                        .or_else(|| select_cover_url_from_images(&track.album.cover_group));
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
                        cover_url,
                    })
                }
            })
            .buffered(16)
            .collect()
            .await;

        Ok(results.into_iter().flatten().collect())
    }

    pub async fn fetch_lyrics(
        &self,
        title: &str,
        artist: &str,
        album: &str,
        duration_ms: u32,
    ) -> TrackLyrics {
        let trimmed_title = title.trim();
        if trimmed_title.is_empty() || trimmed_title == "No Track Playing" {
            return TrackLyrics::Unavailable;
        }
        let primary_artist = artist.split(',').next().unwrap_or(artist).trim();
        let duration_sec = duration_ms.saturating_add(500) / 1000;

        let mut get_url = format!(
            "{LRCLIB_API_BASE}/get?track_name={}&artist_name={}&album_name={}",
            urlencode_query(trimmed_title),
            urlencode_query(primary_artist),
            urlencode_query(album.trim())
        );
        if duration_sec > 0 {
            get_url.push_str(&format!("&duration={duration_sec}"));
        }

        if let Ok(resp) = self
            .http
            .get(&get_url)
            .header("User-Agent", LRCLIB_USER_AGENT)
            .send()
            .await
            && resp.status().is_success()
            && let Ok(val) = resp.json::<Value>().await
            && let Some(lyrics) = parse_lrclib_entry(&val)
        {
            return lyrics;
        }

        let cleaned_title = clean_track_title(trimmed_title);
        let search_query = if primary_artist.is_empty() {
            cleaned_title
        } else {
            format!("{primary_artist} {cleaned_title}")
        };
        let search_url = format!(
            "{LRCLIB_API_BASE}/search?q={}",
            urlencode_query(search_query.trim())
        );

        if let Ok(resp) = self
            .http
            .get(&search_url)
            .header("User-Agent", LRCLIB_USER_AGENT)
            .send()
            .await
            && resp.status().is_success()
            && let Ok(Value::Array(items)) = resp.json::<Value>().await
            && let Some(best) = select_best_lrclib_match(&items, duration_sec)
        {
            return best;
        }

        TrackLyrics::Unavailable
    }

    async fn fetch_collection_v2_items(
        &self,
        set_name: &str,
    ) -> Result<Vec<(String, i32)>, BoxError> {
        let username = self.session.username();
        if username.is_empty() {
            return Err("Session username is empty".into());
        }

        let mut all_items = Vec::new();
        let mut page_token = String::new();
        let mut proto_succeeded = false;

        for _ in 0..100 {
            let body = encode_collection_v2_page_request(&username, set_name, &page_token, 300);
            let mut headers = HeaderMap::new();
            headers.insert(
                CONTENT_TYPE,
                HeaderValue::from_static("application/x-protobuf"),
            );
            headers.insert(ACCEPT, HeaderValue::from_static("application/x-protobuf"));

            match self
                .session
                .spclient()
                .request(
                    &Method::POST,
                    "/collection/v2/paging",
                    Some(headers),
                    Some(&body),
                )
                .await
            {
                Ok(resp_bytes) => {
                    if let Ok((page_items, next_tok)) = parse_collection_v2_proto(&resp_bytes) {
                        proto_succeeded = true;
                        let got_items = !page_items.is_empty();
                        all_items.extend(page_items);
                        if next_tok.is_empty() || !got_items || next_tok == page_token {
                            break;
                        }
                        page_token = next_tok;
                    } else if let Ok(json_val) = serde_json::from_slice::<Value>(&resp_bytes) {
                        proto_succeeded = true;
                        all_items.extend(parse_collection_v2_json(&json_val));
                        break;
                    } else {
                        break;
                    }
                }
                Err(_) => break,
            }
        }

        if proto_succeeded {
            return Ok(all_items);
        }

        let json_body = json!({
            "username": username,
            "set": set_name,
            "pagination_token": "",
            "limit": 300
        })
        .to_string();
        let mut json_headers = HeaderMap::new();
        json_headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        let resp_bytes = self
            .session
            .spclient()
            .request_as_json(
                &Method::POST,
                "/collection/v2/paging",
                Some(json_headers),
                Some(&json_body),
            )
            .await?;

        if let Ok(json_val) = serde_json::from_slice::<Value>(&resp_bytes) {
            Ok(parse_collection_v2_json(&json_val))
        } else {
            let (items, _) = parse_collection_v2_proto(&resp_bytes)?;
            Ok(items)
        }
    }

    async fn fetch_album_chunk_extended(&self, chunk_uris: &[String]) -> Vec<AlbumItem> {
        let entity_request: Vec<EntityRequest> = chunk_uris
            .iter()
            .map(|uri| EntityRequest {
                entity_uri: uri.clone(),
                query: vec![ExtensionQuery {
                    extension_kind: EnumOrUnknown::new(ExtensionKind::ALBUM_V4),
                    ..Default::default()
                }],
                ..Default::default()
            })
            .collect();

        let req = BatchedEntityRequest {
            entity_request,
            ..Default::default()
        };
        match self.session.spclient().get_extended_metadata(req).await {
            Ok(resp) => parse_batched_albums_response(&resp, chunk_uris),
            Err(_) => Vec::new(),
        }
    }

    async fn fetch_albums_batched(&self, uris: &[SpotifyUri]) -> Vec<AlbumItem> {
        if uris.is_empty() {
            return Vec::new();
        }

        let ordered_uri_strings: Vec<String> = uris.iter().map(SpotifyUri::to_uri).collect();
        let mut resolved_map: HashMap<String, AlbumItem> =
            HashMap::with_capacity(ordered_uri_strings.len());
        let mut missing_uris: Vec<String> = Vec::new();
        let mut seen_missing: std::collections::HashSet<String> = std::collections::HashSet::new();

        if let Ok(guard) = self.cache.read() {
            for u in &ordered_uri_strings {
                if let Some(cached) = guard.albums.get(u) {
                    resolved_map.insert(u.clone(), cached.clone());
                } else if seen_missing.insert(u.clone()) {
                    missing_uris.push(u.clone());
                }
            }
        } else {
            for u in &ordered_uri_strings {
                if seen_missing.insert(u.clone()) {
                    missing_uris.push(u.clone());
                }
            }
        }

        if !missing_uris.is_empty() {
            let chunks: Vec<Vec<String>> = missing_uris
                .chunks(METADATA_BATCH_SIZE)
                .map(|c| c.to_vec())
                .collect();
            let client_clone = self.clone();

            let chunk_results: Vec<Vec<AlbumItem>> = stream::iter(chunks)
                .map(move |chunk| {
                    let client = client_clone.clone();
                    async move {
                        let batch = client.fetch_album_chunk_extended(&chunk).await;
                        if !batch.is_empty() {
                            return batch;
                        }
                        let chunk_uris: Vec<SpotifyUri> = chunk
                            .iter()
                            .filter_map(|s| SpotifyUri::from_uri(s).ok())
                            .collect();
                        client.fetch_albums_via_metadata(&chunk_uris).await
                    }
                })
                .buffered(4)
                .collect()
                .await;

            let mut newly_fetched = Vec::new();
            for batch in chunk_results {
                for album in batch {
                    resolved_map.insert(album.uri.clone(), album.clone());
                    newly_fetched.push(album);
                }
            }

            let still_missing: Vec<SpotifyUri> = missing_uris
                .iter()
                .filter(|u| !resolved_map.contains_key(*u))
                .filter_map(|u| SpotifyUri::from_uri(u).ok())
                .collect();
            if !still_missing.is_empty() {
                for album in self.fetch_albums_via_metadata(&still_missing).await {
                    resolved_map.insert(album.uri.clone(), album.clone());
                    newly_fetched.push(album);
                }
            }

            if !newly_fetched.is_empty()
                && let Ok(mut guard) = self.cache.write()
            {
                for album in newly_fetched {
                    guard.albums.insert(album.uri.clone(), album);
                }
            }
        }

        ordered_uri_strings
            .iter()
            .filter_map(|u| resolved_map.get(u).cloned())
            .collect()
    }

    async fn fetch_albums_via_metadata(&self, uris: &[SpotifyUri]) -> Vec<AlbumItem> {
        let session = self.session.clone();
        let results: Vec<Option<AlbumItem>> = stream::iter(uris.iter().cloned())
            .map(|uri| {
                let session = session.clone();
                async move {
                    let album = Album::get(&session, &uri).await.ok()?;
                    let artist = album
                        .artists
                        .iter()
                        .map(|a| a.name.clone())
                        .collect::<Vec<_>>()
                        .join(", ");
                    let total_tracks = album
                        .discs
                        .iter()
                        .map(|d| d.tracks.len() as u32)
                        .sum::<u32>();
                    let year = if album.date.year() > 0 {
                        Some(album.date.year() as u32)
                    } else {
                        None
                    };
                    let uri_str = uri.to_uri();
                    let id = extract_spotify_id(&uri_str).to_string();
                    Some(AlbumItem {
                        uri: uri_str,
                        id,
                        name: album.name,
                        artist: if artist.is_empty() {
                            "Unknown Artist".to_string()
                        } else {
                            artist
                        },
                        total_tracks,
                        year,
                    })
                }
            })
            .buffered(16)
            .collect()
            .await;

        results.into_iter().flatten().collect()
    }

    pub async fn get_user_playlists(&self) -> Result<Vec<PlaylistItem>, BoxError> {
        let mut all_entries: Vec<(String, Option<PlaylistItem>)> = Vec::new();
        let mut seen_uris: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut from: usize = 0;
        let page_len: usize = 120;

        while from < 2000 {
            let Ok(bytes) = self.session.spclient().get_rootlist(from, Some(page_len)).await else {
                break;
            };
            let Ok(rootlist) = SelectedListContent::parse_from_bytes(&bytes) else {
                break;
            };
            let raw_items = rootlist
                .contents
                .as_ref()
                .map(|c| c.items.len())
                .unwrap_or(0);
            let is_truncated = rootlist
                .contents
                .as_ref()
                .is_some_and(|c| c.truncated());

            for (uri, item) in parse_rootlist_playlists(&rootlist) {
                if seen_uris.insert(uri.clone()) {
                    all_entries.push((uri, item));
                }
            }

            if !is_truncated || raw_items == 0 {
                break;
            }
            from += raw_items;
        }

        if !all_entries.is_empty() {
            let session = self.session.clone();
            let resolved: Vec<Option<PlaylistItem>> = stream::iter(all_entries)
                .map(|(uri_str, maybe_item)| {
                    let session = session.clone();
                    async move {
                        if let Some(item) = maybe_item {
                            return Some(item);
                        }
                        let uri = SpotifyUri::from_uri(&uri_str).ok()?;
                        let pl = Playlist::get(&session, &uri).await.ok()?;
                        let name = pl.name().trim().to_string();
                        if name.is_empty() {
                            return None;
                        }
                        let owner = "Spotify".to_string();
                        let total_tracks = pl.length.max(0) as u32;
                        Some(PlaylistItem {
                            id: extract_spotify_id(&uri_str).to_string(),
                            uri: uri_str,
                            name,
                            owner,
                            total_tracks,
                        })
                    }
                })
                .buffered(16)
                .collect()
                .await;

            let playlists: Vec<PlaylistItem> = resolved.into_iter().flatten().collect();
            if !playlists.is_empty() {
                return Ok(playlists);
            }
        }

        let token = self.get_access_token().await?;
        let mut next_url = Some(format!("{SPOTIFY_API_BASE}/me/playlists?limit=50"));
        let mut playlists = Vec::new();
        while let Some(url) = next_url.take() {
            let resp = self.http.get(&url).bearer_auth(&token).send().await?;
            if !resp.status().is_success() {
                if playlists.is_empty() {
                    return Err(
                        format!("Failed to fetch user playlists ({})", resp.status()).into(),
                    );
                }
                break;
            }
            let body: Value = resp.json().await?;
            next_url = body
                .get("next")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            playlists.extend(parse_web_me_playlists(&body));
        }
        Ok(playlists)
    }

    fn extract_sorted_track_uris_from_items(items: &[(String, i32)]) -> Vec<SpotifyUri> {
        let mut track_entries: Vec<(&str, i32)> = items
            .iter()
            .filter_map(|(uri, ts)| {
                if uri.starts_with("spotify:track:") {
                    Some((uri.as_str(), *ts))
                } else {
                    None
                }
            })
            .collect();
        track_entries.sort_by(|a, b| b.1.cmp(&a.1));
        track_entries
            .into_iter()
            .filter_map(|(u, _)| SpotifyUri::from_uri(u).ok())
            .collect()
    }

    fn extract_sorted_album_uris_from_items(items: &[(String, i32)]) -> Vec<SpotifyUri> {
        let mut album_entries: Vec<(&str, i32)> = items
            .iter()
            .filter_map(|(uri, ts)| {
                if uri.starts_with("spotify:album:") {
                    Some((uri.as_str(), *ts))
                } else {
                    None
                }
            })
            .collect();
        album_entries.sort_by(|a, b| b.1.cmp(&a.1));
        album_entries
            .into_iter()
            .filter_map(|(u, _)| SpotifyUri::from_uri(u).ok())
            .collect()
    }

    pub async fn fetch_collection_uris_for_library(&self) -> (Vec<SpotifyUri>, Vec<SpotifyUri>) {
        let mut track_uris = Vec::new();
        let mut album_uris = Vec::new();

        if let Ok(items) = self.fetch_collection_v2_items("collection").await {
            track_uris = Self::extract_sorted_track_uris_from_items(&items);
            album_uris = Self::extract_sorted_album_uris_from_items(&items);
        }

        if album_uris.is_empty() {
            for set_name in ["albums", "album"] {
                if let Ok(items) = self.fetch_collection_v2_items(set_name).await {
                    album_uris = Self::extract_sorted_album_uris_from_items(&items);
                    if !album_uris.is_empty() {
                        break;
                    }
                }
            }
        }

        if track_uris.is_empty() {
            let user = self.session.username();
            if !user.is_empty() {
                let ctx_uri = format!("spotify:user:{user}:collection");
                if let Ok(ctx) = self.session.spclient().get_context(&ctx_uri).await {
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
                }
            }
        }

        (track_uris, album_uris)
    }

    pub async fn stream_liked_tracks<F>(
        &self,
        uris: Vec<SpotifyUri>,
        cancel_token: Arc<AtomicU64>,
        expected_id: u64,
        on_batch: F,
    ) -> Result<(), BoxError>
    where
        F: Fn(CollectionBatchUpdate) + Send + Sync,
    {
        if cancel_token.load(Ordering::Relaxed) != expected_id {
            return Ok(());
        }

        if !uris.is_empty() {
            let total = uris.len();
            let first_split = total.min(50);
            let mut is_first_emit = true;

            if first_split > 0 {
                let first_chunk = &uris[..first_split];
                if let Ok(tracks) = self.fetch_tracks_batched(first_chunk, None).await {
                    if cancel_token.load(Ordering::Relaxed) != expected_id {
                        return Ok(());
                    }
                    let is_done = first_split == total;
                    on_batch(CollectionBatchUpdate {
                        tracks,
                        total_expected: Some(total),
                        is_first: true,
                        is_done,
                    });
                    is_first_emit = false;
                }
            }

            if total > first_split {
                let remaining_chunks: Vec<Vec<SpotifyUri>> = uris[first_split..]
                    .chunks(100)
                    .map(|c| c.to_vec())
                    .collect();
                let num_chunks = remaining_chunks.len();
                let client_clone = self.clone();
                let cancel_for_stream = cancel_token.clone();

                let mut chunk_stream = stream::iter(remaining_chunks.into_iter().enumerate())
                    .map(move |(idx, chunk)| {
                        let client = client_clone.clone();
                        let cancel = cancel_for_stream.clone();
                        async move {
                            if cancel.load(Ordering::Relaxed) != expected_id {
                                return (idx, Vec::new());
                            }
                            let tracks = client
                                .fetch_tracks_batched(&chunk, None)
                                .await
                                .unwrap_or_default();
                            (idx, tracks)
                        }
                    })
                    .buffered(4);

                while let Some((idx, tracks)) = chunk_stream.next().await {
                    if cancel_token.load(Ordering::Relaxed) != expected_id {
                        return Ok(());
                    }
                    let is_done = idx + 1 == num_chunks;
                    on_batch(CollectionBatchUpdate {
                        tracks,
                        total_expected: Some(total),
                        is_first: is_first_emit,
                        is_done,
                    });
                    is_first_emit = false;
                }
            }

            return Ok(());
        }

        let tracks = self.get_liked_tracks().await.unwrap_or_default();
        if cancel_token.load(Ordering::Relaxed) != expected_id {
            return Ok(());
        }
        let total = tracks.len();
        on_batch(CollectionBatchUpdate {
            tracks,
            total_expected: Some(total),
            is_first: true,
            is_done: true,
        });
        Ok(())
    }

    pub async fn resolve_saved_albums(&self, uris: Vec<SpotifyUri>) -> Result<Vec<AlbumItem>, BoxError> {
        if !uris.is_empty() {
            return Ok(self.fetch_albums_batched(&uris).await);
        }
        Ok(Vec::new())
    }

    pub async fn get_liked_tracks(&self) -> Result<Vec<TrackMetadata>, BoxError> {
        let (track_uris, _) = self.fetch_collection_uris_for_library().await;
        if !track_uris.is_empty() {
            let tracks = self.fetch_tracks_batched(&track_uris, None).await?;
            if !tracks.is_empty() {
                return Ok(tracks);
            }
        }

        let token = self.get_access_token().await?;
        let mut next_url = Some(format!("{SPOTIFY_API_BASE}/me/tracks?limit=50"));
        let mut all_tracks = Vec::new();
        while let Some(url) = next_url.take() {
            let resp = self.http.get(&url).bearer_auth(&token).send().await?;
            if !resp.status().is_success() {
                if all_tracks.is_empty() {
                    return Err(format!("Failed to fetch liked tracks ({})", resp.status()).into());
                }
                break;
            }
            let body: Value = resp.json().await?;
            next_url = body
                .get("next")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            all_tracks.extend(parse_web_me_tracks(&body));
        }
        Ok(all_tracks)
    }

    pub async fn get_saved_albums(&self) -> Result<Vec<AlbumItem>, BoxError> {
        for set_name in ["collection", "albums", "album"] {
            if let Ok(items) = self.fetch_collection_v2_items(set_name).await {
                let uris = Self::extract_sorted_album_uris_from_items(&items);
                if !uris.is_empty() {
                    let albums = self.fetch_albums_batched(&uris).await;
                    if !albums.is_empty() {
                        return Ok(albums);
                    }
                }
            }
        }

        let token = self.get_access_token().await?;
        let mut next_url = Some(format!("{SPOTIFY_API_BASE}/me/albums?limit=50"));
        let mut all_albums = Vec::new();
        while let Some(url) = next_url.take() {
            let resp = self.http.get(&url).bearer_auth(&token).send().await?;
            if !resp.status().is_success() {
                if all_albums.is_empty() {
                    return Err(format!("Failed to fetch saved albums ({})", resp.status()).into());
                }
                break;
            }
            let body: Value = resp.json().await?;
            next_url = body
                .get("next")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            all_albums.extend(parse_web_me_albums(&body));
        }
        Ok(all_albums)
    }

    #[allow(dead_code)]
    pub async fn fetch_library(&self) -> Result<LibraryData, BoxError> {
        let playlists_fut = self.get_user_playlists();
        let uris_fut = self.fetch_collection_uris_for_library();
        let (playlists_res, (track_uris, album_uris)) = tokio::join!(playlists_fut, uris_fut);

        let tracks_fut = async {
            if !track_uris.is_empty() {
                self.fetch_tracks_batched(&track_uris, None).await
            } else {
                Ok(Vec::new())
            }
        };
        let albums_fut = async {
            if !album_uris.is_empty() {
                Ok::<Vec<AlbumItem>, BoxError>(self.fetch_albums_batched(&album_uris).await)
            } else {
                Ok(Vec::new())
            }
        };

        let (tracks_res, albums_res) = tokio::join!(tracks_fut, albums_fut);

        if playlists_res.is_err() && tracks_res.is_err() && albums_res.is_err() {
            return Err(playlists_res
                .err()
                .unwrap_or_else(|| "Failed to load user library".into()));
        }

        let liked_tracks = tracks_res.unwrap_or_default();
        let liked_len = liked_tracks.len();

        Ok(LibraryData {
            playlists: playlists_res.unwrap_or_default(),
            liked_tracks,
            saved_albums: albums_res.unwrap_or_default(),
            loaded: true,
            liked_total_expected: Some(liked_len),
            liked_loading_more: false,
        })
    }
}

fn parse_lrc_timestamp(tag: &str) -> Option<u32> {
    let (min_str, rest) = tag.trim().split_once(':')?;
    let minutes: u32 = min_str.parse().ok()?;
    let (sec_str, frac_opt) = if let Some((s, f)) = rest.split_once('.') {
        (s, Some(f))
    } else if let Some((s, f)) = rest.split_once(':') {
        (s, Some(f))
    } else {
        (rest, None)
    };
    let seconds: u32 = sec_str.parse().ok()?;
    let frac_ms: u32 = match frac_opt {
        Some(f) if f.len() == 1 => f.parse::<u32>().ok()? * 100,
        Some(f) if f.len() == 2 => f.parse::<u32>().ok()? * 10,
        Some(f) if f.len() >= 3 => f[..3].parse::<u32>().ok()?,
        Some(_) => 0,
        None => 0,
    };
    Some(
        minutes
            .saturating_mul(60_000)
            .saturating_add(seconds.saturating_mul(1_000))
            .saturating_add(frac_ms),
    )
}

pub fn parse_lrc(lrc: &str) -> Vec<SyncedLyricLine> {
    let mut lines = Vec::new();
    for raw_line in lrc.lines() {
        let mut rest = raw_line.trim();
        let mut timestamps = Vec::new();
        while rest.starts_with('[') {
            let Some(end_idx) = rest.find(']') else {
                break;
            };
            let tag = &rest[1..end_idx];
            if let Some(ms) = parse_lrc_timestamp(tag) {
                timestamps.push(ms);
                rest = &rest[end_idx + 1..];
            } else {
                break;
            }
        }
        if timestamps.is_empty() {
            continue;
        }
        let trimmed_text = rest.trim();
        let text = if trimmed_text.is_empty() {
            "♪".to_string()
        } else {
            trimmed_text.to_string()
        };
        for time_ms in timestamps {
            lines.push(SyncedLyricLine {
                time_ms,
                text: text.clone(),
            });
        }
    }
    lines.sort_by_key(|l| l.time_ms);
    lines
}

pub fn parse_lrclib_entry(json: &Value) -> Option<TrackLyrics> {
    if let Some(synced) = json.get("syncedLyrics").and_then(Value::as_str)
        && !synced.trim().is_empty()
    {
        let parsed = parse_lrc(synced);
        if !parsed.is_empty() {
            return Some(TrackLyrics::Synced(parsed));
        }
    }
    if let Some(plain) = json.get("plainLyrics").and_then(Value::as_str)
        && !plain.trim().is_empty()
    {
        let lines: Vec<String> = plain
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect();
        if !lines.is_empty() {
            return Some(TrackLyrics::Unsynced(lines));
        }
    }
    if json.get("instrumental").and_then(Value::as_bool) == Some(true) {
        return Some(TrackLyrics::Instrumental);
    }
    None
}

fn is_edition_suffix(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    const KEYWORDS: &[&str] = &[
        "remaster",
        "deluxe",
        "edition",
        "version",
        "anniversary",
        "live",
        "mono",
        "stereo",
        "bonus",
        "explicit",
        "clean",
        "feat.",
        "ft.",
        "radio edit",
        "single",
    ];
    KEYWORDS.iter().any(|k| lower.contains(k))
}

pub fn clean_track_title(title: &str) -> String {
    let mut current = title.trim();

    if let Some((before, after)) = current.split_once(" - ")
        && is_edition_suffix(after)
        && !before.trim().is_empty()
    {
        current = before.trim();
    }

    for (open, close) in [('(', ')'), ('[', ']')] {
        if current.ends_with(close)
            && let Some(idx) = current.rfind(open)
        {
            let inside = &current[idx + 1..current.len() - 1];
            let before = current[..idx].trim();
            if is_edition_suffix(inside) && !before.is_empty() {
                current = before;
            }
        }
    }

    current.to_string()
}

fn select_best_lrclib_match(items: &[Value], target_duration_sec: u32) -> Option<TrackLyrics> {
    let mut best: Option<(u8, u64, TrackLyrics)> = None;

    for item in items {
        let Some(lyrics) = parse_lrclib_entry(item) else {
            continue;
        };
        let tier: u8 = match lyrics {
            TrackLyrics::Synced(_) => 3,
            TrackLyrics::Unsynced(_) => 2,
            TrackLyrics::Instrumental => 1,
            _ => 0,
        };
        let dur_diff = item
            .get("duration")
            .and_then(Value::as_f64)
            .map(|d| (d.round() as i64 - target_duration_sec as i64).unsigned_abs())
            .unwrap_or(if target_duration_sec == 0 { 0 } else { u64::MAX });

        let replace = match &best {
            None => true,
            Some((best_tier, best_diff, _)) => {
                tier > *best_tier || (tier == *best_tier && dur_diff < *best_diff)
            }
        };
        if replace {
            best = Some((tier, dur_diff, lyrics));
        }
    }

    best.map(|(_, _, lyrics)| lyrics)
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
    fn test_select_best_image_url_and_cover_parsing() {
        let sources = vec![
            json!({ "url": "https://i.scdn.co/image/large640", "width": 640, "height": 640 }),
            json!({ "url": "https://i.scdn.co/image/small64", "width": 64, "height": 64 }),
            json!({ "url": "https://i.scdn.co/image/med300", "width": 300, "height": 300 }),
        ];
        assert_eq!(
            select_best_image_url_from_array(&sources),
            Some("https://i.scdn.co/image/small64".to_string())
        );

        let tiny_only = vec![json!({ "url": "https://i.scdn.co/image/tiny32", "width": 32, "height": 32 })];
        assert_eq!(
            select_best_image_url_from_array(&tiny_only),
            Some("https://i.scdn.co/image/tiny32".to_string())
        );

        assert_eq!(select_best_image_url_from_array(&[]), None);

        let pf_json = json!({
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
                                        "albumOfTrack": {
                                            "name": "Fear Inoculum",
                                            "coverArt": {
                                                "sources": [
                                                    { "url": "https://i.scdn.co/image/pf300", "width": 300, "height": 300 },
                                                    { "url": "https://i.scdn.co/image/pf64", "width": 64, "height": 64 }
                                                ]
                                            }
                                        },
                                        "artists": {
                                            "items": [{ "profile": { "name": "TOOL" } }]
                                        }
                                    }
                                }
                            }
                        ]
                    }
                }
            }
        });
        let pf_res = parse_pathfinder_search_response(&pf_json);
        assert_eq!(
            pf_res.tracks[0].cover_url,
            Some("https://i.scdn.co/image/pf64".to_string())
        );

        let web_json = json!({
            "tracks": {
                "items": [
                    {
                        "id": "t1",
                        "uri": "spotify:track:t1",
                        "name": "Starlight",
                        "duration_ms": 240000,
                        "artists": [{ "name": "Muse" }],
                        "album": {
                            "name": "Black Holes and Revelations",
                            "images": [
                                { "url": "https://i.scdn.co/image/web640", "width": 640, "height": 640 },
                                { "url": "https://i.scdn.co/image/web64", "width": 64, "height": 64 }
                            ]
                        }
                    }
                ]
            }
        });
        let web_res = parse_search_response(&web_json);
        assert_eq!(
            web_res.tracks[0].cover_url,
            Some("https://i.scdn.co/image/web64".to_string())
        );
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

        let pf_cover = results.tracks[0]
            .cover_url
            .as_ref()
            .expect("Pathfinder search track should have cover_url");
        assert!(pf_cover.starts_with("https://"));

        let cover_bytes = client
            .fetch_image_bytes(pf_cover)
            .await
            .expect("fetch_image_bytes should succeed for track cover_url");
        assert!(!cover_bytes.is_empty());
        let decoded_img = image::load_from_memory(&cover_bytes)
            .expect("Downloaded cover bytes should decode as valid image");
        println!(
            "Downloaded and decoded Pathfinder cover ({}): {}x{} px ({} bytes)",
            pf_cover,
            decoded_img.width(),
            decoded_img.height(),
            cover_bytes.len()
        );
        assert!(decoded_img.width() > 0 && decoded_img.width() <= 640);

        let album = &results.albums[0];
        let album_tracks = client
            .get_album_tracks(&album.uri, &album.name)
            .await
            .expect("get_album_tracks should succeed");
        println!("Album '{}' loaded {} tracks (first: '{}')", album.name, album_tracks.len(), album_tracks[0].title);
        assert!(!album_tracks.is_empty());
        let native_cover = album_tracks[0]
            .cover_url
            .as_ref()
            .expect("Native librespot album track should have cover_url");
        assert!(native_cover.starts_with("https://i.scdn.co/image/"));
        let native_bytes = client
            .fetch_image_bytes(native_cover)
            .await
            .expect("fetch_image_bytes should succeed for native cover_url");
        let native_decoded = image::load_from_memory(&native_bytes)
            .expect("Native cover bytes should decode as valid image");
        println!(
            "Downloaded and decoded librespot native cover ({}): {}x{} px",
            native_cover,
            native_decoded.width(),
            native_decoded.height()
        );
        assert!(native_decoded.width() > 0 && native_decoded.width() <= 640);

        assert!(
            client
                .fetch_image_bytes("https://i.scdn.co/image/0000000000000000000000000000000000000000")
                .await
                .is_err()
        );

        let artist = &results.artists[0];
        let artist_tracks = client
            .get_artist_tracks(&artist.uri)
            .await
            .expect("get_artist_tracks should succeed");
        println!("Artist '{}' loaded {} top tracks (first: '{}')", artist.name, artist_tracks.len(), artist_tracks[0].title);
        assert!(!artist_tracks.is_empty());

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

        let empty_res = client.search("   ").await.expect("empty search should succeed");
        assert!(empty_res.tracks.is_empty());
        assert!(empty_res.albums.is_empty());
        assert!(empty_res.artists.is_empty());
        assert!(empty_res.playlists.is_empty());

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

    #[test]
    fn test_parse_lrc_timestamps_multi_metadata_sorting_and_empty_text() {
        let lrc = "\
[ti:Schism]
[ar:TOOL]
[al:Lateralus]
[by:tester]
[length:06:47]
[01:20.500][00:12.05] I know the pieces fit
[00:05]
[00:08.50] 'Cause I watched them fall away
";
        let lines = parse_lrc(lrc);
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0].time_ms, 5_000);
        assert_eq!(lines[0].text, "♪");
        assert_eq!(lines[1].time_ms, 8_500);
        assert_eq!(lines[1].text, "'Cause I watched them fall away");
        assert_eq!(lines[2].time_ms, 12_050);
        assert_eq!(lines[2].text, "I know the pieces fit");
        assert_eq!(lines[3].time_ms, 80_500);
        assert_eq!(lines[3].text, "I know the pieces fit");
    }

    #[test]
    fn test_parse_lrclib_entry_and_clean_track_title() {
        let synced = json!({
            "syncedLyrics": "[00:01.00] First line\n[00:05.00] Second line",
            "plainLyrics": "First line\nSecond line",
            "instrumental": false
        });
        assert!(matches!(parse_lrclib_entry(&synced), Some(TrackLyrics::Synced(v)) if v.len() == 2));

        let unsynced = json!({
            "syncedLyrics": "",
            "plainLyrics": "  Line A \n\n Line B  ",
            "instrumental": false
        });
        assert_eq!(
            parse_lrclib_entry(&unsynced),
            Some(TrackLyrics::Unsynced(vec![
                "Line A".to_string(),
                "Line B".to_string()
            ]))
        );

        let instrumental = json!({
            "syncedLyrics": null,
            "plainLyrics": null,
            "instrumental": true
        });
        assert_eq!(
            parse_lrclib_entry(&instrumental),
            Some(TrackLyrics::Instrumental)
        );

        let empty = json!({
            "syncedLyrics": "",
            "plainLyrics": "   ",
            "instrumental": false
        });
        assert_eq!(parse_lrclib_entry(&empty), None);

        assert_eq!(clean_track_title("Time - 2011 Remastered Version"), "Time");
        assert_eq!(clean_track_title("Money (Remastered 2023)"), "Money");
        assert_eq!(clean_track_title("Schism (Deluxe Edition)"), "Schism");
        assert_eq!(clean_track_title("Schism"), "Schism");
    }

    #[tokio::test]
    #[ignore]
    async fn live_integration_lrclib_fetch_lyrics_test() {
        use crate::spotify::auth::init_cache;
        use librespot::core::config::SessionConfig;

        crate::spotify::init_tls_provider();
        let cache = init_cache().expect("cache");
        let session = Session::new(SessionConfig::default(), Some(cache));
        let client = SpotifyClient::new(session);

        let lyrics = client
            .fetch_lyrics("Schism", "TOOL", "Lateralus", 407_000)
            .await;
        match &lyrics {
            TrackLyrics::Synced(lines) => {
                println!("LRCLIB Synced lyrics fetched: {} lines (first: '{}')", lines.len(), lines[0].text);
                assert!(!lines.is_empty());
            }
            TrackLyrics::Unsynced(lines) => {
                println!("LRCLIB Unsynced lyrics fetched: {} lines (first: '{}')", lines.len(), lines[0]);
                assert!(!lines.is_empty());
            }
            other => panic!("Expected Synced or Unsynced lyrics from LRCLIB for TOOL - Schism, got {other:?}"),
        }
    }

    #[test]
    fn test_collection_v2_proto_encode_and_parse() {
        let req = encode_collection_v2_page_request("testuser", "collection", "tok_abc", 300);
        assert!(!req.is_empty());

        let mut item1 = Vec::new();
        {
            let mut cos = CodedOutputStream::vec(&mut item1);
            cos.write_string(1, "spotify:track:111").unwrap();
            cos.write_int32(2, 1700000100).unwrap();
            cos.write_bool(3, false).unwrap();
            cos.flush().unwrap();
        }
        let mut item2 = Vec::new();
        {
            let mut cos = CodedOutputStream::vec(&mut item2);
            cos.write_string(1, "spotify:track:222").unwrap();
            cos.write_int32(2, 1700000200).unwrap();
            cos.write_bool(3, true).unwrap();
            cos.flush().unwrap();
        }
        let mut resp = Vec::new();
        {
            let mut cos = CodedOutputStream::vec(&mut resp);
            cos.write_bytes(1, &item1).unwrap();
            cos.write_bytes(1, &item2).unwrap();
            cos.write_string(2, "next_tok_99").unwrap();
            cos.flush().unwrap();
        }

        let (items, next_tok) = parse_collection_v2_proto(&resp).expect("should parse proto");
        assert_eq!(items, vec![("spotify:track:111".to_string(), 1700000100)]);
        assert_eq!(next_tok, "next_tok_99");
    }

    #[test]
    fn test_collection_v2_json_rootlist_and_web_me_parsers() {
        use librespot::protocol::playlist4_external::{
            Item, ListAttributes, ListItems, MetaItem,
        };

        let col_json = json!({
            "items": [
                { "uri": "spotify:track:a", "added_at": 100, "is_removed": false },
                { "uri": "spotify:track:b", "addedAt": "200", "isRemoved": false },
                { "uri": "spotify:track:c", "added_at": 300, "is_removed": true },
                { "uri": "spotify:album:d", "addedAt": 400, "isRemoved": true }
            ]
        });
        assert_eq!(
            parse_collection_v2_json(&col_json),
            vec![
                ("spotify:track:a".to_string(), 100),
                ("spotify:track:b".to_string(), 200)
            ]
        );

        let mut rootlist = SelectedListContent::new();
        let mut list_items = ListItems::new();

        let mut group_start = Item::new();
        group_start.set_uri("spotify:start-group:abc:Folder".to_string());
        list_items.items.push(group_start);
        list_items.meta_items.push(MetaItem::new());

        let mut pl_item = Item::new();
        pl_item.set_uri("spotify:user:alice:playlist:pl123".to_string());
        list_items.items.push(pl_item);
        let mut pl_meta = MetaItem::new();
        let mut attrs = ListAttributes::new();
        attrs.set_name("My Mix".to_string());
        pl_meta.attributes = protobuf::MessageField::some(attrs);
        pl_meta.set_owner_username("alice".to_string());
        pl_meta.set_length(12);
        list_items.meta_items.push(pl_meta);

        let mut group_end = Item::new();
        group_end.set_uri("spotify:end-group:abc".to_string());
        list_items.items.push(group_end);
        list_items.meta_items.push(MetaItem::new());

        rootlist.contents = protobuf::MessageField::some(list_items);

        let parsed_rootlist = parse_rootlist_playlists(&rootlist);
        assert_eq!(parsed_rootlist.len(), 1);
        assert_eq!(parsed_rootlist[0].0, "spotify:playlist:pl123");
        let resolved = parsed_rootlist[0].1.as_ref().expect("meta should resolve");
        assert_eq!(resolved.name, "My Mix");
        assert_eq!(resolved.owner, "alice");
        assert_eq!(resolved.total_tracks, 12);

        let me_playlists = json!({
            "items": [
                {
                    "id": "p1",
                    "uri": "spotify:playlist:p1",
                    "name": "Chill",
                    "owner": { "display_name": "Bob" },
                    "tracks": { "total": 25 }
                }
            ]
        });
        let pls = parse_web_me_playlists(&me_playlists);
        assert_eq!(pls.len(), 1);
        assert_eq!(pls[0].name, "Chill");
        assert_eq!(pls[0].owner, "Bob");
        assert_eq!(pls[0].total_tracks, 25);

        let me_tracks = json!({
            "items": [
                {
                    "track": {
                        "uri": "spotify:track:t1",
                        "name": "Liked Song",
                        "duration_ms": 210000,
                        "artists": [{ "name": "Artist One" }],
                        "album": { "name": "Album One", "images": [] }
                    }
                }
            ]
        });
        let trks = parse_web_me_tracks(&me_tracks);
        assert_eq!(trks.len(), 1);
        assert_eq!(trks[0].title, "Liked Song");

        let me_albums = json!({
            "items": [
                {
                    "album": {
                        "id": "al1",
                        "uri": "spotify:album:al1",
                        "name": "Saved LP",
                        "artists": [{ "name": "Band X" }],
                        "total_tracks": 11,
                        "release_date": "2019-08-30"
                    }
                }
            ]
        });
        let albs = parse_web_me_albums(&me_albums);
        assert_eq!(albs.len(), 1);
        assert_eq!(albs[0].name, "Saved LP");
        assert_eq!(albs[0].year, Some(2019));

        let covers = vec![
            CoverImage {
                url: "https://i.scdn.co/image/large640".to_string(),
                size: librespot::metadata::image::ImageSize::LARGE,
                width: 640,
                height: 640,
            },
            CoverImage {
                url: "https://i.scdn.co/image/small64".to_string(),
                size: librespot::metadata::image::ImageSize::SMALL,
                width: 64,
                height: 64,
            },
        ];
        assert_eq!(
            select_best_cover_url_from_audio_item(&covers),
            Some("https://i.scdn.co/image/small64".to_string())
        );
    }

    #[tokio::test]
    #[ignore]
    async fn live_integration_fetch_library_test() {
        use crate::spotify::auth::{create_session, init_cache, wait_for_session_country};

        let cache = init_cache().expect("cache");
        let creds = cache.credentials().expect("cached creds");
        let session = create_session(Some(cache));
        session.connect(creds, true).await.expect("connect");
        wait_for_session_country(&session).await;

        let client = SpotifyClient::new(session);
        let lib = client.fetch_library().await.expect("fetch_library should succeed without 429");
        println!(
            "Live Library: {} playlists, {} liked tracks, {} saved albums",
            lib.playlists.len(),
            lib.liked_tracks.len(),
            lib.saved_albums.len()
        );
        if let Some(pl) = lib.playlists.first() {
            println!("First playlist: '{}' by {} ({} tracks)", pl.name, pl.owner, pl.total_tracks);
        }
        if let Some(tr) = lib.liked_tracks.first() {
            println!("First liked track: '{}' — {}", tr.title, tr.artist);
        }
        if let Some(al) = lib.saved_albums.first() {
            println!("First saved album: '{}' — {}", al.name, al.artist);
        }
        assert!(
            !lib.playlists.is_empty()
                || !lib.liked_tracks.is_empty()
                || !lib.saved_albums.is_empty(),
            "Expected at least one non-empty library section"
        );
    }

    #[test]
    fn test_parse_batched_tracks_and_albums_response() {
        use librespot::protocol::entity_extension_data::EntityExtensionData;
        use librespot::protocol::extended_metadata::EntityExtensionDataArray;
        use protobuf::well_known_types::any::Any;

        let mut img_large = proto_metadata::Image::new();
        img_large.set_file_id(vec![0xaa; 20]);
        img_large.set_width(640);
        img_large.set_height(640);

        let mut img_small = proto_metadata::Image::new();
        img_small.set_file_id(vec![0xbb; 20]);
        img_small.set_width(64);
        img_small.set_height(64);

        let mut img_zero = proto_metadata::Image::new();
        img_zero.set_file_id(vec![0x00; 20]);
        img_zero.set_width(64);
        img_zero.set_height(64);

        let mut artist1 = proto_metadata::Artist::new();
        artist1.set_name("TOOL".to_string());

        let mut album_proto = proto_metadata::Album::new();
        album_proto.set_name("Lateralus".to_string());
        album_proto.cover.push(img_zero);
        album_proto.cover.push(img_large);
        album_proto.cover.push(img_small);

        let mut track1 = proto_metadata::Track::new();
        track1.set_name("Schism".to_string());
        track1.artist.push(artist1);
        track1.album = protobuf::MessageField::some(album_proto);
        track1.set_duration(407_000);

        let mut cg_img = proto_metadata::Image::new();
        cg_img.set_file_id(vec![0xcc; 20]);
        cg_img.set_width(300);
        cg_img.set_height(300);

        let mut cg = proto_metadata::ImageGroup::new();
        cg.image.push(cg_img);

        let mut album2 = proto_metadata::Album::new();
        album2.cover_group = protobuf::MessageField::some(cg);

        let mut track2 = proto_metadata::Track::new();
        track2.set_name("Parabol".to_string());
        let mut awr = proto_metadata::ArtistWithRole::new();
        awr.set_artist_name("TOOL".to_string());
        track2.artist_with_role.push(awr);
        track2.album = protobuf::MessageField::some(album2);
        track2.set_duration(184_000);

        let mut entry1 = EntityExtensionData::new();
        entry1.entity_uri = "spotify:track:t1".to_string();
        let mut any1 = Any::new();
        any1.value = track1.write_to_bytes().unwrap();
        entry1.extension_data = protobuf::MessageField::some(any1);

        let mut entry2 = EntityExtensionData::new();
        entry2.entity_uri = "spotify:track:t2".to_string();
        let mut any2 = Any::new();
        any2.value = track2.write_to_bytes().unwrap();
        entry2.extension_data = protobuf::MessageField::some(any2);

        let mut arr = EntityExtensionDataArray::new();
        arr.extension_data.push(entry2);
        arr.extension_data.push(entry1);

        let mut resp = BatchedExtensionResponse::new();
        resp.extended_metadata.push(arr);

        let ordered = vec![
            "spotify:track:t1".to_string(),
            "spotify:track:t2".to_string(),
            "spotify:track:t1".to_string(),
            "spotify:track:missing".to_string(),
        ];

        let parsed = parse_batched_tracks_response(&resp, &ordered, Some("Fallback Album"));
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0].uri, "spotify:track:t1");
        assert_eq!(parsed[0].title, "Schism");
        assert_eq!(parsed[0].artist, "TOOL");
        assert_eq!(parsed[0].album, "Lateralus");
        assert_eq!(parsed[0].duration_ms, 407_000);
        assert_eq!(
            parsed[0].cover_url,
            Some(format!("https://i.scdn.co/image/{}", "bb".repeat(20)))
        );

        assert_eq!(parsed[1].uri, "spotify:track:t2");
        assert_eq!(parsed[1].title, "Parabol");
        assert_eq!(parsed[1].artist, "TOOL");
        assert_eq!(parsed[1].album, "Fallback Album");
        assert_eq!(
            parsed[1].cover_url,
            Some(format!("https://i.scdn.co/image/{}", "cc".repeat(20)))
        );

        assert_eq!(parsed[2].uri, "spotify:track:t1");
        assert_eq!(parsed[2].title, "Schism");

        let mut alb1 = proto_metadata::Album::new();
        alb1.set_name("Aenima".to_string());
        let mut a_art = proto_metadata::Artist::new();
        a_art.set_name("TOOL".to_string());
        alb1.artist.push(a_art);
        let mut disc1 = proto_metadata::Disc::new();
        disc1.track.push(proto_metadata::Track::new());
        disc1.track.push(proto_metadata::Track::new());
        let mut disc2 = proto_metadata::Disc::new();
        disc2.track.push(proto_metadata::Track::new());
        alb1.disc.push(disc1);
        alb1.disc.push(disc2);
        let mut dt = proto_metadata::Date::new();
        dt.set_year(1996);
        alb1.date = protobuf::MessageField::some(dt);

        let mut a_entry = EntityExtensionData::new();
        a_entry.entity_uri = "spotify:album:al96".to_string();
        let mut a_any = Any::new();
        a_any.value = alb1.write_to_bytes().unwrap();
        a_entry.extension_data = protobuf::MessageField::some(a_any);

        let mut a_arr = EntityExtensionDataArray::new();
        a_arr.extension_data.push(a_entry);
        let mut a_resp = BatchedExtensionResponse::new();
        a_resp.extended_metadata.push(a_arr);

        let parsed_albums = parse_batched_albums_response(
            &a_resp,
            &[
                "spotify:album:al96".to_string(),
                "spotify:album:nonexistent".to_string(),
            ],
        );
        assert_eq!(parsed_albums.len(), 1);
        assert_eq!(parsed_albums[0].id, "al96");
        assert_eq!(parsed_albums[0].name, "Aenima");
        assert_eq!(parsed_albums[0].artist, "TOOL");
        assert_eq!(parsed_albums[0].total_tracks, 3);
        assert_eq!(parsed_albums[0].year, Some(1996));
    }

    #[test]
    fn test_parse_playlist_page_uris() {
        use librespot::protocol::playlist4_external::{Item, ListItems};

        let mut page = SelectedListContent::new();
        page.set_length(250);
        let mut list_items = ListItems::new();
        list_items.set_truncated(true);

        let mut item1 = Item::new();
        item1.set_uri("spotify:track:55mJleti2WfWEFNFcBduhc".to_string());
        list_items.items.push(item1);

        let mut item_ep = Item::new();
        item_ep.set_uri("spotify:episode:1234567890123456789012".to_string());
        list_items.items.push(item_ep);

        let mut item2 = Item::new();
        item2.set_uri("spotify:track:7tvuLLroI0n6uYBWuFig5d".to_string());
        list_items.items.push(item2);

        page.contents = protobuf::MessageField::some(list_items);

        let (uris, raw_items_len, total_length, is_truncated) = parse_playlist_page_uris(&page);
        assert_eq!(uris.len(), 2);
        assert_eq!(raw_items_len, 3);
        assert_eq!(total_length, 250);
        assert!(is_truncated);
        assert_eq!(uris[0].to_uri(), "spotify:track:55mJleti2WfWEFNFcBduhc");
        assert_eq!(uris[1].to_uri(), "spotify:track:7tvuLLroI0n6uYBWuFig5d");

        let empty_page = SelectedListContent::new();
        let (e_uris, e_raw, e_total, e_trunc) = parse_playlist_page_uris(&empty_page);
        assert!(e_uris.is_empty());
        assert_eq!(e_raw, 0);
        assert_eq!(e_total, 0);
        assert!(!e_trunc);
    }

    #[tokio::test]
    #[ignore]
    async fn live_integration_large_playlist_and_speed_test() {
        use crate::spotify::auth::{create_session, init_cache, wait_for_session_country};
        use std::collections::HashSet;
        use std::sync::Mutex;
        use std::time::Instant;

        let cache = init_cache().expect("cache");
        let creds = cache.credentials().expect("cached creds");
        let session = create_session(Some(cache));
        session.connect(creds, true).await.expect("connect");
        wait_for_session_country(&session).await;

        let client = SpotifyClient::new(session);

        let t_lib = Instant::now();
        let lib = client
            .fetch_library()
            .await
            .expect("fetch_library should succeed");
        let lib_ms = t_lib.elapsed().as_millis();
        println!(
            "fetch_library() took {}ms: {} playlists, {} liked tracks, {} saved albums",
            lib_ms,
            lib.playlists.len(),
            lib.liked_tracks.len(),
            lib.saved_albums.len()
        );

        let largest_user_pl = lib
            .playlists
            .iter()
            .filter(|p| p.total_tracks > 50)
            .max_by_key(|p| p.total_tracks)
            .cloned();

        let target_pl = if let Some(ref pl) = largest_user_pl
            && pl.total_tracks > 100
        {
            println!(
                "Using largest user playlist (>100): '{}' ({} tracks, {})",
                pl.name, pl.total_tracks, pl.uri
            );
            pl.clone()
        } else {
            if let Some(ref pl) = largest_user_pl {
                println!(
                    "Largest user playlist has {} tracks (>50, <=100): '{}' ({}); testing it first",
                    pl.total_tracks, pl.name, pl.uri
                );
                let user_tracks = client
                    .get_playlist_tracks(&pl.uri)
                    .await
                    .expect("user >50 playlist load");
                println!(
                    "User playlist '{}' loaded {} tracks",
                    pl.name,
                    user_tracks.len()
                );
                assert!(
                    user_tracks.len() > 50,
                    "Expected >50 tracks for user playlist '{}', got {}",
                    pl.name,
                    user_tracks.len()
                );
            }
            let mut found_big: Option<PlaylistItem> = None;
            for q in ["Rock Classics", "All Out 2000s", "Top Hits", "Workout"] {
                if let Ok(res) = client.search(q).await {
                    for pl in res.playlists {
                        if pl.total_tracks > 100
                            && found_big
                                .as_ref()
                                .is_none_or(|b| pl.total_tracks > b.total_tracks)
                        {
                            found_big = Some(pl);
                        }
                    }
                }
                if found_big.is_some() {
                    break;
                }
            }
            let big = found_big.unwrap_or_else(|| {
                PlaylistItem {
                    uri: "spotify:playlist:37i9dQZF1DXcBWIGoYBM5M".to_string(),
                    id: "37i9dQZF1DXcBWIGoYBM5M".to_string(),
                    name: "Fallback Large Playlist".to_string(),
                    owner: "Spotify".to_string(),
                    total_tracks: 0,
                }
            });
            println!(
                "Using public playlist (>100): '{}' ({} expected tracks, {})",
                big.name, big.total_tracks, big.uri
            );
            big
        };

        client.clear_collection_cache();
        let cancel = Arc::new(AtomicU64::new(1));
        let start = Instant::now();
        let first_batch_ms: Arc<Mutex<Option<u128>>> = Arc::new(Mutex::new(None));
        let batch_count: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));
        let collected: Arc<Mutex<Vec<TrackMetadata>>> = Arc::new(Mutex::new(Vec::new()));

        let fb_ref = first_batch_ms.clone();
        let bc_ref = batch_count.clone();
        let col_ref = collected.clone();

        client
            .stream_collection_tracks(
                CollectionKind::Playlist,
                &target_pl.uri,
                &target_pl.name,
                cancel.clone(),
                1,
                move |batch| {
                    let mut fb = fb_ref.lock().unwrap();
                    if fb.is_none() && !batch.tracks.is_empty() {
                        *fb = Some(start.elapsed().as_millis());
                    }
                    *bc_ref.lock().unwrap() += 1;
                    col_ref.lock().unwrap().extend(batch.tracks);
                },
            )
            .await
            .expect("stream_collection_tracks should succeed");

        let total_ms = start.elapsed().as_millis();
        let first_ms = first_batch_ms.lock().unwrap().unwrap_or(0);
        let batches = *batch_count.lock().unwrap();
        let tracks = collected.lock().unwrap().clone();
        let unique_uris: HashSet<&str> = tracks.iter().map(|t| t.uri.as_str()).collect();

        println!(
            "stream_collection_tracks('{}'): {} tracks ({} unique) in {}ms (1st batch: {}ms, batches: {})",
            target_pl.name,
            tracks.len(),
            unique_uris.len(),
            total_ms,
            first_ms,
            batches
        );

        assert!(
            tracks.len() > 50,
            "Expected >50 tracks loaded, got {}",
            tracks.len()
        );
        if target_pl.total_tracks > 100 {
            assert!(
                tracks.len() > 100,
                "Expected >100 tracks loaded for playlist with {} tracks, got {}",
                target_pl.total_tracks,
                tracks.len()
            );
            assert!(
                unique_uris.len() > 100,
                "Expected >100 unique track URIs (no page-1 repetition), got {}",
                unique_uris.len()
            );
        }

        let t_cache = Instant::now();
        let cached_collected: Arc<Mutex<Vec<TrackMetadata>>> = Arc::new(Mutex::new(Vec::new()));
        let cc_ref = cached_collected.clone();
        client
            .stream_collection_tracks(
                CollectionKind::Playlist,
                &target_pl.uri,
                &target_pl.name,
                cancel,
                1,
                move |batch| {
                    cc_ref.lock().unwrap().extend(batch.tracks);
                },
            )
            .await
            .expect("cached stream_collection_tracks should succeed");
        let cache_ms = t_cache.elapsed().as_millis();
        let cached_tracks = cached_collected.lock().unwrap().clone();
        println!(
            "Cached stream_collection_tracks('{}'): {} tracks in {}ms",
            target_pl.name,
            cached_tracks.len(),
            cache_ms
        );
        assert_eq!(cached_tracks.len(), tracks.len());
        assert!(
            cache_ms < 50,
            "Expected cached load < 50ms, took {}ms",
            cache_ms
        );
    }
}
