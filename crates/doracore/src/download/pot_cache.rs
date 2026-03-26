//! PO Token cache for YouTube downloads.
//!
//! Caches tokens from the bgutil HTTP server to avoid regenerating them
//! on every yt-dlp invocation (~6.5s each). Tokens are valid for 6 hours.
//! When a cached token is available, yt-dlp receives it via
//! `--extractor-args youtube:po_token=web+TOKEN` instead of calling bgutil.

use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};
use std::time::{Duration, Instant};

/// Cached PO token entry.
struct PotEntry {
    /// Formatted extractor-arg: `youtube:po_token=web+TOKEN`
    extractor_arg: String,
    /// When this token was fetched.
    fetched_at: Instant,
}

/// Global PO Token cache keyed by video ID.
static POT_CACHE: LazyLock<RwLock<HashMap<String, PotEntry>>> = LazyLock::new(|| RwLock::new(HashMap::new()));

/// TTL for cached tokens (6 hours, matching bgutil TOKEN_TTL).
const POT_TTL: Duration = Duration::from_secs(6 * 3600);

/// Maximum cache entries before eviction.
const MAX_ENTRIES: usize = 1000;

/// bgutil HTTP server endpoint.
const BGUTIL_URL: &str = "http://127.0.0.1:4416/get_pot";

/// Timeout for bgutil HTTP request.
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);

/// Get a cached PO token extractor-arg for the given video ID.
fn get_cached(video_id: &str) -> Option<String> {
    let cache = POT_CACHE.read().ok()?;
    let entry = cache.get(video_id)?;
    if entry.fetched_at.elapsed() < POT_TTL {
        Some(entry.extractor_arg.clone())
    } else {
        None
    }
}

/// Store a PO token in the cache.
fn store(video_id: String, extractor_arg: String) {
    let Ok(mut cache) = POT_CACHE.write() else {
        return;
    };
    if cache.len() >= MAX_ENTRIES {
        let keys: Vec<String> = cache.keys().take(MAX_ENTRIES / 2).cloned().collect();
        for key in keys {
            cache.remove(&key);
        }
    }
    cache.insert(
        video_id,
        PotEntry {
            extractor_arg,
            fetched_at: Instant::now(),
        },
    );
}

/// Fetch a fresh PO token from bgutil HTTP server (async).
async fn fetch_from_bgutil(video_id: &str) -> Option<String> {
    let url = format!("https://www.youtube.com/watch?v={}", video_id);
    let body = serde_json::json!({ "url": url });

    let client = reqwest::Client::builder().timeout(FETCH_TIMEOUT).build().ok()?;

    let resp = client
        .post(BGUTIL_URL)
        .json(&body)
        .send()
        .await
        .map_err(|e| {
            log::warn!("[POT_CACHE] bgutil request failed: {}", e);
            e
        })
        .ok()?;

    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| {
            log::warn!("[POT_CACHE] bgutil response parse failed: {}", e);
            e
        })
        .ok()?;

    let token = json.get("poToken")?.as_str()?;
    if token.is_empty() {
        log::warn!("[POT_CACHE] bgutil returned empty poToken");
        return None;
    }

    let extractor_arg = format!("youtube:po_token=web+{},fetch_pot=never", token);
    log::info!(
        "[POT_CACHE] Fetched PO token for {} ({}... len={})",
        video_id,
        &token[..token.len().min(20)],
        token.len()
    );
    Some(extractor_arg)
}

/// Get a PO token extractor-arg, using cache when possible.
///
/// Returns the formatted string for `--extractor-args`, e.g.
/// `"youtube:po_token=web+TOKEN"`.
///
/// Must be called from async context (before `spawn_blocking`).
/// Returns `None` if bgutil is unavailable — caller should fall back
/// to the bgutil yt-dlp plugin.
pub async fn get_or_fetch(video_id: &str) -> Option<String> {
    if let Some(cached) = get_cached(video_id) {
        log::info!("[POT_CACHE] Cache hit for {}", video_id);
        return Some(cached);
    }

    let extractor_arg = fetch_from_bgutil(video_id).await?;
    store(video_id.to_string(), extractor_arg.clone());
    Some(extractor_arg)
}
