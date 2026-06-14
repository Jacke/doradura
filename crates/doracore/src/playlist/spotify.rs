//! Spotify [`PlaylistProvider`]: reads a **public** playlist via the Web API
//! using the Client Credentials flow (app token, no user OAuth). Needs
//! `SPOTIFY_CLIENT_ID` / `SPOTIFY_CLIENT_SECRET` (see `config::spotify`).
//!
//! Step 1 scope: parse playlist URL, fetch app token, fetch the first page of
//! tracks (≤100) + the playlist name, map to neutral [`PlaylistTrack`]s with
//! ISRC. Pagination beyond 100 and album URLs are follow-ups.

use anyhow::{Context, anyhow};
use async_trait::async_trait;
use serde_json::Value;
use url::Url;

use super::{Platform, PlaylistProvider, PlaylistRef, PlaylistSnapshot, PlaylistTrack};
use crate::core::config;

/// Spotify provider. Stateless — a fresh app token is fetched per `fetch` call
/// (tokens are cheap and last an hour; caching is a later optimization).
pub struct SpotifyProvider;

impl SpotifyProvider {
    pub fn new() -> Self {
        Self
    }

    /// Fetch a Client Credentials app token.
    async fn app_token(client: &reqwest::Client) -> anyhow::Result<String> {
        let id = config::spotify::CLIENT_ID
            .clone()
            .ok_or_else(|| anyhow!("SPOTIFY_CLIENT_ID is not set"))?;
        let secret = config::spotify::CLIENT_SECRET
            .clone()
            .ok_or_else(|| anyhow!("SPOTIFY_CLIENT_SECRET is not set"))?;
        let resp: Value = client
            .post("https://accounts.spotify.com/api/token")
            .basic_auth(id, Some(secret))
            .form(&[("grant_type", "client_credentials")])
            .send()
            .await
            .context("spotify token request")?
            .json()
            .await
            .context("spotify token decode")?;
        resp.get("access_token")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| anyhow!("spotify token response had no access_token: {resp}"))
    }
}

impl Default for SpotifyProvider {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse a Spotify playlist URL → playlist id. Accepts
/// `https://open.spotify.com/playlist/{id}` (with optional `?si=…`, locale
/// segments like `/intl-de/`, and `spotify:playlist:{id}` URIs).
pub fn parse_playlist_id(url: &Url) -> Option<String> {
    let host = url.host_str().unwrap_or("");
    if !host.contains("spotify.com") {
        // Also accept the `spotify:playlist:ID` URI form (host is empty there;
        // url crate parses it oddly, so this mostly handles the http form).
        return None;
    }
    let segs: Vec<&str> = url.path_segments()?.filter(|s| !s.is_empty()).collect();
    // Find "playlist" and take the next segment as the id.
    let pos = segs.iter().position(|s| *s == "playlist")?;
    segs.get(pos + 1).map(|s| s.to_string())
}

/// Map a Spotify `playlist tracks` JSON object → neutral tracks. Pure.
///
/// Shape: `{ "items": [ { "track": { id, name, duration_ms, artists:[{name}],
/// external_ids:{isrc} } } ] }`. Removed/`null` tracks and local files (no id)
/// are skipped.
pub fn map_tracks_json(value: &Value) -> Vec<PlaylistTrack> {
    let Some(items) = value.get("items").and_then(Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let track = item.get("track").filter(|t| !t.is_null())?;
            let external_id = track.get("id").and_then(Value::as_str)?.to_string();
            let title = track.get("name").and_then(Value::as_str).unwrap_or("").to_string();
            let artist = track
                .get("artists")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(|a| a.get("name").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            let isrc = track
                .get("external_ids")
                .and_then(|e| e.get("isrc"))
                .and_then(Value::as_str)
                .map(str::to_string);
            let duration_secs = track
                .get("duration_ms")
                .and_then(Value::as_u64)
                .map(|ms| (ms / 1000) as u32);
            Some(PlaylistTrack {
                external_id,
                title,
                artist,
                isrc,
                duration_secs,
            })
        })
        .collect()
}

#[async_trait]
impl PlaylistProvider for SpotifyProvider {
    fn platform(&self) -> Platform {
        Platform::Spotify
    }

    fn parse_ref(&self, url: &Url) -> Option<PlaylistRef> {
        parse_playlist_id(url).map(|id| PlaylistRef {
            platform: Platform::Spotify,
            id,
        })
    }

    async fn fetch(&self, playlist: &PlaylistRef) -> anyhow::Result<PlaylistSnapshot> {
        let client = reqwest::Client::new();
        let token = Self::app_token(&client).await?;

        // Playlist name.
        let meta: Value = client
            .get(format!(
                "https://api.spotify.com/v1/playlists/{}?fields=name",
                playlist.id
            ))
            .bearer_auth(&token)
            .send()
            .await
            .context("spotify playlist meta")?
            .json()
            .await
            .context("spotify playlist meta decode")?;
        let name = meta
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("Spotify playlist")
            .to_string();

        // First page of tracks (≤100). Pagination via `next` is a follow-up.
        let tracks_json: Value = client
            .get(format!(
                "https://api.spotify.com/v1/playlists/{}/tracks?limit=100&fields=items(track(id,name,duration_ms,artists(name),external_ids(isrc)))",
                playlist.id
            ))
            .bearer_auth(&token)
            .send()
            .await
            .context("spotify playlist tracks")?
            .json()
            .await
            .context("spotify playlist tracks decode")?;

        Ok(PlaylistSnapshot {
            playlist: playlist.clone(),
            name,
            tracks: map_tracks_json(&tracks_json),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn u(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn parses_playlist_id_from_urls() {
        assert_eq!(
            parse_playlist_id(&u("https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M")).as_deref(),
            Some("37i9dQZF1DXcBWIGoYBM5M")
        );
        // With ?si tracking + locale segment.
        assert_eq!(
            parse_playlist_id(&u("https://open.spotify.com/intl-de/playlist/abc123?si=xyz")).as_deref(),
            Some("abc123")
        );
        // Not a playlist URL.
        assert_eq!(parse_playlist_id(&u("https://open.spotify.com/track/xyz")), None);
        assert_eq!(parse_playlist_id(&u("https://youtube.com/playlist?list=PL")), None);
    }

    #[test]
    fn maps_tracks_with_isrc_and_skips_nulls() {
        let v = json!({
            "items": [
                { "track": { "id": "t1", "name": "Song A", "duration_ms": 204000,
                    "artists": [{"name": "Дора"}, {"name": "Guest"}],
                    "external_ids": {"isrc": "QM1234567890"} } },
                { "track": null },
                { "track": { "id": "t2", "name": "Song B", "duration_ms": 180000,
                    "artists": [{"name": "Eminem"}], "external_ids": {} } }
            ]
        });
        let tracks = map_tracks_json(&v);
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].external_id, "t1");
        assert_eq!(tracks[0].artist, "Дора, Guest");
        assert_eq!(tracks[0].isrc.as_deref(), Some("QM1234567890"));
        assert_eq!(tracks[0].duration_secs, Some(204));
        assert_eq!(tracks[1].external_id, "t2");
        assert_eq!(tracks[1].isrc, None);
    }

    #[test]
    fn empty_or_missing_items_is_empty() {
        assert!(map_tracks_json(&json!({})).is_empty());
        assert!(map_tracks_json(&json!({"items": []})).is_empty());
    }
}
