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
}

#[derive(Deserialize)]
struct OdesliPlatformEntry {
    url: String,
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

    if links.has_any() {
        Some(links)
    } else {
        None
    }
}
