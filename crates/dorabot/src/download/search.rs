//! Music search engine using yt-dlp for YouTube and SoundCloud.

use crate::core::config;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tokio::process::Command as TokioCommand;
use tokio::time::timeout;

use crate::storage::db::{self, DbPool};

/// A single search result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub title: String,
    pub artist: String,
    pub url: String,
    pub duration_secs: Option<u32>,
    pub thumbnail: Option<String>,
}

/// Search source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SearchSource {
    YouTube,
    SoundCloud,
}

impl SearchSource {
    pub fn prefix(&self) -> &'static str {
        match self {
            SearchSource::YouTube => "ytsearch",
            SearchSource::SoundCloud => "scsearch",
        }
    }

    pub fn cache_key(&self) -> &'static str {
        match self {
            SearchSource::YouTube => "yt",
            SearchSource::SoundCloud => "sc",
        }
    }

    /// Full source name for storage in playlist_items.source column.
    pub fn source_name(&self) -> &'static str {
        match self {
            SearchSource::YouTube => "youtube",
            SearchSource::SoundCloud => "soundcloud",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            SearchSource::YouTube => "YouTube",
            SearchSource::SoundCloud => "SoundCloud",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "y" => Some(SearchSource::YouTube),
            "s" => Some(SearchSource::SoundCloud),
            _ => None,
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            SearchSource::YouTube => "y",
            SearchSource::SoundCloud => "s",
        }
    }
}

/// Append WARP proxy args to a yt-dlp command if configured.
pub fn append_proxy_args(args: &mut Vec<String>) {
    if let Some(ref proxy) = *config::proxy::WARP_PROXY {
        let proxy_url = proxy.trim();
        if !proxy_url.is_empty() && proxy_url != "none" && proxy_url != "disabled" {
            args.push("--proxy".to_string());
            args.push(proxy_url.to_string());
        }
    }
}

/// Search timeout in seconds.
const SEARCH_TIMEOUT_SECS: u64 = 30;

/// Cache TTL in minutes.
const CACHE_TTL_MINUTES: i64 = 15;

/// JSON structure from yt-dlp --flat-playlist --dump-json.
/// Shared by search and playlist import.
///
/// Note: yt-dlp outputs BOTH `uploader` and `channel` fields simultaneously,
/// so we cannot use `#[serde(alias)]` — serde treats both as duplicates and fails.
/// Instead we read both and pick the best one via `artist()`.
#[derive(Debug, Deserialize)]
pub struct YtdlpFlatEntry {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub uploader: Option<String>,
    #[serde(default)]
    pub channel: Option<String>,
    #[serde(default)]
    pub webpage_url: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub duration: Option<f64>,
    #[serde(default)]
    pub thumbnail: Option<String>,
}

impl YtdlpFlatEntry {
    /// Get artist name: prefer `uploader`, fall back to `channel`.
    /// Filters out yt-dlp "NA" placeholder values.
    pub fn artist(&self) -> Option<&str> {
        self.uploader
            .as_deref()
            .filter(|s| !s.is_empty() && *s != "NA")
            .or_else(|| self.channel.as_deref().filter(|s| !s.is_empty() && *s != "NA"))
    }
}

/// Run a music search via yt-dlp.
pub async fn search(
    source: SearchSource,
    query: &str,
    limit: u8,
    db_pool: Option<&Arc<DbPool>>,
) -> anyhow::Result<Vec<SearchResult>> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(vec![]);
    }

    let cache_key = format!("{}:{}", source.cache_key(), query.to_lowercase());

    // Check cache
    if let Some(pool) = db_pool {
        if let Ok(conn) = db::get_connection(pool) {
            if let Ok(Some(cached)) = db::get_cached_search(&conn, &cache_key, CACHE_TTL_MINUTES) {
                if let Ok(results) = serde_json::from_str::<Vec<SearchResult>>(&cached) {
                    log::debug!("Search cache hit for '{}'", cache_key);
                    return Ok(results);
                }
            }
        }
    }

    let search_query = format!("{}{}:{}", source.prefix(), limit, query);
    let ytdl_bin = &*config::YTDL_BIN;

    let mut args: Vec<String> = vec![
        "--flat-playlist".to_string(),
        "--dump-json".to_string(),
        "--no-warnings".to_string(),
        "--no-check-certificate".to_string(),
    ];

    append_proxy_args(&mut args);

    args.push(search_query);

    log::info!(
        "Search: {} '{}' (limit={}) args={:?}",
        source.label(),
        query,
        limit,
        args
    );

    let output = timeout(
        Duration::from_secs(SEARCH_TIMEOUT_SECS),
        TokioCommand::new(ytdl_bin).args(&args).output(),
    )
    .await
    .map_err(|_| anyhow::anyhow!("Search timed out"))?
    .with_context(|| "Failed to execute yt-dlp")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        log::warn!("yt-dlp search failed: {}", stderr);
        anyhow::bail!("Search failed: {}", stderr.lines().next().unwrap_or("unknown error"));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !stderr.is_empty() {
        log::debug!("yt-dlp search stderr: {}", stderr);
    }

    let results: Vec<SearchResult> = stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| {
            let json: YtdlpFlatEntry = match serde_json::from_str(line) {
                Ok(j) => j,
                Err(e) => {
                    log::warn!("Search JSON parse error: {} — line: {:.200}", e, line);
                    return None;
                }
            };
            let artist = json.artist().unwrap_or_default().to_string();
            let title = json.title.unwrap_or_default();
            if title.is_empty() {
                return None;
            }
            let url = json.webpage_url.or(json.url).unwrap_or_default();
            if url.is_empty() {
                return None;
            }
            // Skip non-video results (channels, playlists) — they cause download hangs
            if url.contains("/channel/") || url.contains("/playlist?") || url.contains("/user/") || url.contains("/@") {
                log::debug!("Search: skipping non-video URL: {:.100}", url);
                return None;
            }
            Some(SearchResult {
                title,
                artist,
                url,
                duration_secs: json.duration.map(|d| d as u32),
                thumbnail: json.thumbnail,
            })
        })
        .collect();

    if results.is_empty() && !stdout.is_empty() {
        log::warn!("Search parsed 0 results from {} bytes stdout", stdout.len());
        log::debug!("Search raw stdout (first 500 chars): {:.500}", stdout);
    }

    // Cache results and periodically clean up stale entries
    if let Some(pool) = db_pool {
        if let Ok(conn) = db::get_connection(pool) {
            if let Ok(json) = serde_json::to_string(&results) {
                let _ = db::cache_search_results(&conn, &cache_key, &json);
            }
            // Cleanup stale cache entries ~5% of the time
            if rand::random::<u8>() < 13 {
                let _ = db::cleanup_search_cache(&conn, CACHE_TTL_MINUTES);
            }
        }
    }

    log::info!("Search returned {} results for '{}'", results.len(), query);
    Ok(results)
}

/// Detect source name from URL for storage in playlist_items.source column.
pub fn source_name_from_url(url: &str) -> &'static str {
    if url.contains("spotify.com") {
        "spotify"
    } else if url.contains("soundcloud.com") {
        "soundcloud"
    } else {
        "youtube"
    }
}

/// Format duration as mm:ss.
pub fn format_duration(secs: Option<u32>) -> String {
    match secs {
        Some(s) => format!("{}:{:02}", s / 60, s % 60),
        None => "?:??".to_string(),
    }
}
