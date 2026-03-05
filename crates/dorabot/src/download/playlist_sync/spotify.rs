//! Spotify playlist resolver using Spotify Web API + YouTube search for track matching.

use async_trait::async_trait;
use serde::Deserialize;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

use crate::core::config;
use crate::download::search::{self, SearchSource};
use crate::storage::db::DbPool;

use super::resolver::{ImportTrack, Platform, PlaylistResolver, ProgressFn, ResolvedPlaylist, TrackStatus};

/// Cached Spotify access token.
static TOKEN_CACHE: once_cell::sync::Lazy<RwLock<Option<(String, Instant)>>> =
    once_cell::sync::Lazy::new(|| RwLock::new(None));

pub struct SpotifyResolver {
    db_pool: Arc<DbPool>,
}

impl SpotifyResolver {
    pub fn new(db_pool: Arc<DbPool>) -> Self {
        Self { db_pool }
    }
}

#[async_trait]
impl PlaylistResolver for SpotifyResolver {
    fn platform(&self) -> Platform {
        Platform::Spotify
    }

    fn supports_url(&self, url: &str) -> bool {
        let lower = url.to_lowercase();
        lower.contains("open.spotify.com/playlist/") || lower.contains("open.spotify.com/album/")
    }

    async fn resolve(&self, url: &str, progress: Option<ProgressFn>) -> Result<ResolvedPlaylist, String> {
        let client_id = config::spotify::CLIENT_ID
            .as_deref()
            .ok_or("Spotify import unavailable. Admin: set SPOTIFY_CLIENT_ID")?;
        let client_secret = config::spotify::CLIENT_SECRET
            .as_deref()
            .ok_or("Spotify import unavailable. Admin: set SPOTIFY_CLIENT_SECRET")?;

        let token = get_token(client_id, client_secret).await?;
        let (kind, spotify_id) = parse_spotify_url(url)?;

        let client = reqwest::Client::new();

        let (name, description, raw_tracks) = match kind.as_str() {
            "playlist" => fetch_playlist_tracks(&client, &token, &spotify_id).await?,
            "album" => fetch_album_tracks(&client, &token, &spotify_id).await?,
            _ => return Err(format!("Unsupported Spotify URL type: {}", kind)),
        };

        let total = raw_tracks.len();
        let mut tracks = Vec::with_capacity(total);

        for (i, raw) in raw_tracks.into_iter().enumerate() {
            let search_query = format!("{} - {}", raw.artist, raw.title);

            if let Some(ref cb) = progress {
                cb(i + 1, total, &search_query);
            }

            let resolved_url = match search::search(SearchSource::YouTube, &search_query, 1, Some(&self.db_pool)).await
            {
                Ok(results) if !results.is_empty() => Some(results[0].url.clone()),
                _ => None,
            };

            let status = if resolved_url.is_some() {
                TrackStatus::Matched
            } else {
                TrackStatus::NotFound
            };

            tracks.push(ImportTrack {
                title: raw.title,
                artist: Some(raw.artist),
                duration_secs: Some(raw.duration_ms / 1000),
                external_id: Some(format!("spotify:track:{}", raw.id)),
                source_url: Some(format!("https://open.spotify.com/track/{}", raw.id)),
                resolved_url,
                status,
            });
        }

        Ok(ResolvedPlaylist {
            name,
            description,
            tracks,
            platform: Platform::Spotify,
        })
    }
}

// ==================== Spotify API Types ====================

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[allow(dead_code)]
    expires_in: u64,
}

#[derive(Debug)]
struct RawSpotifyTrack {
    id: String,
    title: String,
    artist: String,
    duration_ms: i32,
}

#[derive(Debug, Deserialize)]
struct SpotifyPlaylistResponse {
    name: String,
    description: Option<String>,
    tracks: SpotifyPaginatedTracks,
}

#[derive(Debug, Deserialize)]
struct SpotifyPaginatedTracks {
    items: Vec<SpotifyPlaylistItem>,
    next: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SpotifyPlaylistItem {
    track: Option<SpotifyTrack>,
}

#[derive(Debug, Deserialize)]
struct SpotifyAlbumResponse {
    name: String,
    artists: Vec<SpotifyArtist>,
    tracks: SpotifyAlbumTracks,
}

#[derive(Debug, Deserialize)]
struct SpotifyAlbumTracks {
    items: Vec<SpotifyTrack>,
    next: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SpotifyTrack {
    id: Option<String>,
    name: String,
    artists: Vec<SpotifyArtist>,
    duration_ms: i32,
}

#[derive(Debug, Deserialize)]
struct SpotifyArtist {
    name: String,
}

// ==================== Token Management ====================

async fn get_token(client_id: &str, client_secret: &str) -> Result<String, String> {
    // Check cache — reuse if created within ~55 min (tokens expire in 1h)
    {
        let cache = TOKEN_CACHE.read().await;
        if let Some((ref token, ref created_at)) = *cache {
            if created_at.elapsed().as_secs() < 3300 {
                return Ok(token.clone());
            }
        }
    }

    log::info!("Fetching new Spotify access token");

    let client = reqwest::Client::new();
    let resp = client
        .post("https://accounts.spotify.com/api/token")
        .basic_auth(client_id, Some(client_secret))
        .form(&[("grant_type", "client_credentials")])
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("Spotify auth failed: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("Spotify auth error {}: {}", status, body));
    }

    let token_resp: TokenResponse = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse Spotify token: {}", e))?;

    let token = token_resp.access_token.clone();

    // Cache token
    {
        let mut cache = TOKEN_CACHE.write().await;
        *cache = Some((token_resp.access_token, Instant::now()));
    }

    Ok(token)
}

// ==================== API Helpers ====================

fn parse_spotify_url(url: &str) -> Result<(String, String), String> {
    // https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M?si=...
    // https://open.spotify.com/album/1DFixLWuPkv3KT3TnV35m3
    let parts: Vec<&str> = url.split('/').collect();
    for (i, part) in parts.iter().enumerate() {
        if (*part == "playlist" || *part == "album") && i + 1 < parts.len() {
            let id = parts[i + 1].split('?').next().unwrap_or(parts[i + 1]);
            return Ok((part.to_string(), id.to_string()));
        }
    }
    Err("Invalid Spotify URL. Expected /playlist/ or /album/ URL".to_string())
}

async fn fetch_playlist_tracks(
    client: &reqwest::Client,
    token: &str,
    playlist_id: &str,
) -> Result<(String, Option<String>, Vec<RawSpotifyTrack>), String> {
    let url = format!("https://api.spotify.com/v1/playlists/{}?fields=name,description,tracks(items(track(id,name,artists,duration_ms)),next,total)", playlist_id);

    let resp = spotify_get(client, token, &url).await?;
    let playlist: SpotifyPlaylistResponse = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse Spotify playlist: {}", e))?;

    let name = playlist.name;
    let description = playlist.description.filter(|d| !d.is_empty());
    let mut tracks = extract_playlist_tracks(playlist.tracks.items);
    let mut next_url = playlist.tracks.next;

    // Paginate
    while let Some(ref next) = next_url {
        let resp = spotify_get(client, token, next).await?;
        let page: SpotifyPaginatedTracks = resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse Spotify page: {}", e))?;
        tracks.extend(extract_playlist_tracks(page.items));
        next_url = page.next;
    }

    Ok((name, description, tracks))
}

async fn fetch_album_tracks(
    client: &reqwest::Client,
    token: &str,
    album_id: &str,
) -> Result<(String, Option<String>, Vec<RawSpotifyTrack>), String> {
    let url = format!("https://api.spotify.com/v1/albums/{}", album_id);

    let resp = spotify_get(client, token, &url).await?;
    let album: SpotifyAlbumResponse = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse Spotify album: {}", e))?;

    let name = album.name;
    let album_artist = album.artists.first().map(|a| a.name.clone()).unwrap_or_default();
    let mut tracks: Vec<RawSpotifyTrack> = album
        .tracks
        .items
        .iter()
        .filter_map(|t| {
            let id = t.id.as_ref()?.clone();
            let artist = t
                .artists
                .first()
                .map(|a| a.name.clone())
                .unwrap_or_else(|| album_artist.clone());
            Some(RawSpotifyTrack {
                id,
                title: t.name.clone(),
                artist,
                duration_ms: t.duration_ms,
            })
        })
        .collect();

    let mut next_url = album.tracks.next;
    while let Some(ref next) = next_url {
        let resp = spotify_get(client, token, next).await?;
        let page: SpotifyAlbumTracks = resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse Spotify album page: {}", e))?;
        for t in &page.items {
            if let Some(ref id) = t.id {
                let artist = t
                    .artists
                    .first()
                    .map(|a| a.name.clone())
                    .unwrap_or_else(|| album_artist.clone());
                tracks.push(RawSpotifyTrack {
                    id: id.clone(),
                    title: t.name.clone(),
                    artist,
                    duration_ms: t.duration_ms,
                });
            }
        }
        next_url = page.next;
    }

    Ok((name, None, tracks))
}

fn extract_playlist_tracks(items: Vec<SpotifyPlaylistItem>) -> Vec<RawSpotifyTrack> {
    items
        .into_iter()
        .filter_map(|item| {
            let track = item.track?;
            let id = track.id?;
            let artist = track
                .artists
                .first()
                .map(|a| a.name.clone())
                .unwrap_or_else(|| "Unknown".to_string());
            Some(RawSpotifyTrack {
                id,
                title: track.name,
                artist,
                duration_ms: track.duration_ms,
            })
        })
        .collect()
}

async fn spotify_get(client: &reqwest::Client, token: &str, url: &str) -> Result<reqwest::Response, String> {
    let resp = client
        .get(url)
        .bearer_auth(token)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| format!("Spotify API request failed: {}", e))?;

    if resp.status().as_u16() == 429 {
        // Rate limited — get retry-after header
        let retry_after = resp
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(5);
        log::warn!("Spotify rate limited, waiting {}s", retry_after);
        tokio::time::sleep(Duration::from_secs(retry_after)).await;
        // Retry once
        return client
            .get(url)
            .bearer_auth(token)
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .map_err(|e| format!("Spotify API retry failed: {}", e));
    }

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("Spotify API error {}: {}", status, body));
    }

    Ok(resp)
}
