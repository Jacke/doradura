//! Odesli API client for fetching streaming links.
//!
//! Calls https://api.song.link to get cross-platform streaming links for a given URL.
//! Free API, no key required. Rate limit: 10 req/sec.

use serde::Deserialize;
use std::collections::HashMap;
use std::time::Duration;

/// Streaming platform links for a track.
#[derive(Debug, Clone, Default)]
pub struct StreamingLinks {
    pub spotify: Option<String>,
    pub apple_music: Option<String>,
    pub youtube_music: Option<String>,
    pub deezer: Option<String>,
    pub tidal: Option<String>,
    pub amazon_music: Option<String>,
}

impl StreamingLinks {
    /// Returns true if at least one streaming link is available.
    pub fn has_any(&self) -> bool {
        self.spotify.is_some()
            || self.apple_music.is_some()
            || self.youtube_music.is_some()
            || self.deezer.is_some()
            || self.tidal.is_some()
            || self.amazon_music.is_some()
    }
}

#[derive(Deserialize)]
struct OdesliResponse {
    #[serde(rename = "linksByPlatform")]
    links_by_platform: Option<HashMap<String, OdesliPlatformEntry>>,
    #[serde(rename = "entityUniqueId")]
    entity_unique_id: Option<String>,
    #[serde(rename = "entitiesByUniqueId")]
    entities_by_unique_id: Option<HashMap<String, OdesliEntity>>,
}

#[derive(Deserialize)]
struct OdesliPlatformEntry {
    url: String,
}

#[derive(Deserialize)]
struct OdesliEntity {
    title: Option<String>,
    #[serde(rename = "artistName")]
    artist_name: Option<String>,
}

/// A track resolved through Odesli: a direct YouTube link when one exists, plus
/// title/artist (always present) so callers can fall back to a YouTube search
/// for tracks Odesli has no YouTube entry for.
#[derive(Debug, Clone, Default)]
pub struct OdesliTrack {
    /// `youtube` or `youtubeMusic` link, if Odesli has one.
    pub youtube: Option<String>,
    pub title: Option<String>,
    pub artist: Option<String>,
}

/// Resolve any streaming URL (Spotify/Apple/Deezer track, …) to a YouTube link
/// and/or its title+artist. Returns `None` only when Odesli is unreachable or
/// has no data. The YouTube link may be `None` even on success (no YT match) —
/// callers should then search by `title`/`artist`.
pub async fn fetch_track(source_url: &str) -> Option<OdesliTrack> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
        .ok()?;
    let api_url = format!(
        "https://api.song.link/v1-alpha.1/links?url={}&userCountry=US",
        urlencoding::encode(source_url)
    );
    let response = client.get(&api_url).send().await.ok()?;
    if !response.status().is_success() {
        log::debug!("Odesli fetch_track status {} for {}", response.status(), source_url);
        return None;
    }
    let data: OdesliResponse = response.json().await.ok()?;
    let platforms = data.links_by_platform.unwrap_or_default();
    let youtube = platforms
        .get("youtube")
        .or_else(|| platforms.get("youtubeMusic"))
        .map(|e| e.url.clone());

    let (title, artist) = data
        .entity_unique_id
        .as_ref()
        .and_then(|id| data.entities_by_unique_id.as_ref()?.get(id))
        .map(|e| (e.title.clone(), e.artist_name.clone()))
        .unwrap_or((None, None));

    let track = OdesliTrack { youtube, title, artist };
    if track.youtube.is_some() || track.title.is_some() {
        Some(track)
    } else {
        None
    }
}

/// Fetches streaming links from Odesli API for a given URL.
///
/// Returns `None` silently on any error — the bot should not fail if Odesli is down.
pub async fn fetch_streaming_links(source_url: &str) -> Option<StreamingLinks> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
        .ok()?;

    let api_url = format!(
        "https://api.song.link/v1-alpha.1/links?url={}&userCountry=US",
        urlencoding::encode(source_url)
    );

    let response = client.get(&api_url).send().await.ok()?;

    if !response.status().is_success() {
        log::debug!("Odesli API returned status {} for {}", response.status(), source_url);
        return None;
    }

    let data: OdesliResponse = response.json().await.ok()?;
    let platforms = data.links_by_platform?;

    let get = |key: &str| platforms.get(key).map(|e| e.url.clone());

    let links = StreamingLinks {
        spotify: get("spotify"),
        apple_music: get("appleMusic"),
        youtube_music: get("youtubeMusic"),
        deezer: get("deezer"),
        tidal: get("tidal"),
        amazon_music: get("amazonMusic"),
    };

    if links.has_any() { Some(links) } else { None }
}
