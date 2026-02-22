//! Share page creation for streaming links.
//!
//! After a successful YouTube download, creates a public web page with
//! Odesli streaming links and an ambilight UI.

use crate::core::odesli::{self, StreamingLinks};
use crate::storage::db::DbPool;
use crate::storage::get_connection;
use std::sync::Arc;

/// Data stored and returned for a share page.
pub struct SharePageData {
    pub id: String,
    pub title: String,
    pub artist: Option<String>,
    pub thumbnail_url: Option<String>,
    pub duration_secs: Option<u64>,
    pub streaming_links: Option<StreamingLinks>,
}

/// Creates a share page record in the database and returns the full public URL.
///
/// Returns `None` silently if:
/// - `WEB_BASE_URL` is not configured (opt-out)
/// - The URL is not a YouTube URL
/// - DB insertion fails
pub async fn create_share_page(
    db: &Arc<DbPool>,
    youtube_url: &str,
    title: &str,
    artist: Option<&str>,
    thumbnail_url: Option<&str>,
    duration_secs: Option<u64>,
) -> Option<(String, Option<StreamingLinks>)> {
    let base_url = std::env::var("WEB_BASE_URL").ok()?;

    // Generate 8-char ID from UUID hex
    let id = {
        let full = uuid::Uuid::new_v4().simple().to_string();
        full[..8].to_string()
    };

    // Fetch streaming links concurrently (don't block on failure)
    let streaming_links = odesli::fetch_streaming_links(youtube_url).await;

    // Serialize streaming links to JSON
    let links_json = streaming_links.as_ref().map(|links| {
        let mut map = serde_json::Map::new();
        if let Some(ref s) = links.spotify {
            map.insert("spotify".into(), serde_json::Value::String(s.clone()));
        }
        if let Some(ref s) = links.apple_music {
            map.insert("appleMusic".into(), serde_json::Value::String(s.clone()));
        }
        if let Some(ref s) = links.youtube_music {
            map.insert("youtubeMusic".into(), serde_json::Value::String(s.clone()));
        }
        if let Some(ref s) = links.deezer {
            map.insert("deezer".into(), serde_json::Value::String(s.clone()));
        }
        if let Some(ref s) = links.tidal {
            map.insert("tidal".into(), serde_json::Value::String(s.clone()));
        }
        if let Some(ref s) = links.amazon_music {
            map.insert("amazonMusic".into(), serde_json::Value::String(s.clone()));
        }
        serde_json::Value::Object(map).to_string()
    });

    // Insert into DB
    let conn = get_connection(db).ok()?;
    let result = conn.execute(
        "INSERT INTO share_pages (id, youtube_url, title, artist, thumbnail_url, duration_secs, streaming_links) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![
            id,
            youtube_url,
            title,
            artist,
            thumbnail_url,
            duration_secs.map(|d| d as i64),
            links_json,
        ],
    );

    if let Err(e) = result {
        log::warn!("Failed to insert share page into DB: {}", e);
        return None;
    }

    let share_url = format!("{}/s/{}", base_url.trim_end_matches('/'), id);
    log::info!("Created share page: {}", share_url);

    Some((share_url, streaming_links))
}

/// Extracts YouTube video ID from a URL and returns thumbnail URL.
///
/// Supports:
/// - `https://www.youtube.com/watch?v=VIDEO_ID`
/// - `https://youtu.be/VIDEO_ID`
/// - `https://m.youtube.com/watch?v=VIDEO_ID`
pub fn youtube_thumbnail_url(url: &str) -> Option<String> {
    let video_id = extract_youtube_video_id(url)?;
    Some(format!("https://img.youtube.com/vi/{}/maxresdefault.jpg", video_id))
}

fn extract_youtube_video_id(url: &str) -> Option<String> {
    // youtu.be/VIDEO_ID
    if url.contains("youtu.be/") {
        let parts: Vec<&str> = url.splitn(2, "youtu.be/").collect();
        if let Some(after) = parts.get(1) {
            let id = after.split(['?', '#', '&']).next()?;
            if !id.is_empty() {
                return Some(id.to_string());
            }
        }
    }

    // youtube.com/watch?v=VIDEO_ID
    if url.contains("youtube.com/") {
        if let Ok(parsed) = url::Url::parse(url) {
            for (key, val) in parsed.query_pairs() {
                if key == "v" && !val.is_empty() {
                    return Some(val.into_owned());
                }
            }
        }
    }

    None
}

/// Returns true if the URL is a YouTube URL.
pub fn is_youtube_url(url: &str) -> bool {
    url.contains("youtube.com/") || url.contains("youtu.be/")
}
