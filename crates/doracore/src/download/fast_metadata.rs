//! Fast YouTube metadata via HTML scraping of `ytInitialPlayerResponse`.
//!
//! YouTube server-side renders the full player response into the HTML page.
//! Extracting it is ~3-5x faster than yt-dlp (1-2s vs 5-15s) because it
//! skips JS challenge solving, cipher decryption, and PO token generation.
//!
//! Used in experimental mode for preview metadata.

use lazy_regex::{Lazy, Regex, lazy_regex};
use serde::Deserialize;
use std::time::Duration;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

static YT_INITIAL_PLAYER_RE: Lazy<Regex> =
    lazy_regex!(r"(?s)var ytInitialPlayerResponse\s*=\s*(\{.+?\});\s*(?:var|</script>)");

/// Video format info extracted from YouTube HTML.
#[derive(Debug, Clone)]
pub struct YtFastFormat {
    pub quality_label: String,
    pub mime_type: String,
    pub content_length: Option<u64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub bitrate: Option<u64>,
    pub fps: Option<u32>,
}

/// Full metadata from YouTube HTML scraping.
#[derive(Debug, Clone)]
pub struct YtPageMetadata {
    pub title: String,
    pub author: Option<String>,
    pub duration_secs: Option<u32>,
    pub thumbnail_url: Option<String>,
    pub video_formats: Vec<YtFastFormat>,
}

/// Extract YouTube video ID from a URL string.
pub fn extract_youtube_id(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    let host = parsed.host_str()?.to_lowercase();

    if host == "youtu.be" {
        return parsed
            .path_segments()?
            .next()
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty());
    }

    if host.contains("youtube.com") {
        let path = parsed.path();
        if path.starts_with("/watch") {
            return parsed
                .query_pairs()
                .find(|(k, _)| k == "v")
                .map(|(_, v)| v.into_owned());
        }
        for prefix in &["/shorts/", "/live/", "/embed/"] {
            if let Some(rest) = path.strip_prefix(prefix) {
                let id = rest.split('/').next().unwrap_or("");
                if !id.is_empty() {
                    return Some(id.to_string());
                }
            }
        }
    }
    None
}

/// Scrape YouTube page for format metadata.
///
/// Returns `None` if the URL is not YouTube, the page can't be fetched,
/// or the player response can't be parsed.
pub async fn scrape_youtube_metadata(url: &str) -> Option<YtPageMetadata> {
    let video_id = extract_youtube_id(url)?;
    let start = std::time::Instant::now();

    let result = scrape_inner(&video_id).await;

    let elapsed = start.elapsed();
    match &result {
        Some(meta) => log::info!(
            "⚡ YouTube HTML scrape OK in {:.1}s: {} ({} video formats)",
            elapsed.as_secs_f64(),
            meta.title,
            meta.video_formats.len()
        ),
        None => log::warn!(
            "⚡ YouTube HTML scrape FAILED in {:.1}s for {}",
            elapsed.as_secs_f64(),
            video_id
        ),
    }

    result
}

async fn scrape_inner(video_id: &str) -> Option<YtPageMetadata> {
    let client = reqwest::Client::builder().timeout(REQUEST_TIMEOUT).build().ok()?;

    let page_url = format!("https://www.youtube.com/watch?v={}", video_id);
    let resp = client
        .get(&page_url)
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36")
        .header("Accept-Language", "en-US,en;q=0.9")
        .send()
        .await
        .ok()?;

    if !resp.status().is_success() {
        log::debug!("YouTube page returned {}", resp.status());
        return None;
    }

    let html = resp.text().await.ok()?;
    let json_str = YT_INITIAL_PLAYER_RE.captures(&html)?.get(1)?.as_str();

    let player: PlayerResponse = serde_json::from_str(json_str).ok()?;

    // Check playability
    if let Some(ref status) = player.playability_status
        && status.status.as_deref() != Some("OK")
    {
        log::debug!("YouTube playability: {:?} — {:?}", status.status, status.reason);
        return None;
    }

    let details = player.video_details?;
    let title = details.title.unwrap_or_default();
    if title.is_empty() {
        return None;
    }

    // Extract video formats from adaptiveFormats
    let video_formats = player
        .streaming_data
        .as_ref()
        .and_then(|sd| sd.adaptive_formats.as_ref())
        .map(|formats| {
            formats
                .iter()
                .filter(|f| f.mime_type.as_deref().is_some_and(|m| m.starts_with("video/")))
                .filter_map(|f| {
                    let label = f.quality_label.as_ref()?;
                    if label.is_empty() || label == "unknown" {
                        return None;
                    }
                    Some(YtFastFormat {
                        quality_label: label.clone(),
                        mime_type: f.mime_type.clone().unwrap_or_default(),
                        content_length: f.content_length.as_ref().and_then(|s| s.parse::<u64>().ok()),
                        width: f.width,
                        height: f.height,
                        bitrate: f.bitrate,
                        fps: f.fps,
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    // Thumbnail: use highest quality available
    let thumbnail_url = details
        .thumbnail
        .and_then(|t| t.thumbnails)
        .and_then(|thumbs| thumbs.into_iter().max_by_key(|t| t.width.unwrap_or(0)))
        .and_then(|t| t.url)
        .or_else(|| Some(format!("https://i.ytimg.com/vi/{}/hqdefault.jpg", video_id)));

    Some(YtPageMetadata {
        title,
        author: details.author,
        duration_secs: details.length_seconds.as_ref().and_then(|s| s.parse::<u32>().ok()),
        thumbnail_url,
        video_formats,
    })
}

// ── JSON structures for ytInitialPlayerResponse ─────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlayerResponse {
    #[serde(default)]
    video_details: Option<VideoDetails>,
    #[serde(default)]
    streaming_data: Option<StreamingData>,
    #[serde(default)]
    playability_status: Option<PlayabilityStatus>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlayabilityStatus {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VideoDetails {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    length_seconds: Option<String>,
    #[serde(default)]
    thumbnail: Option<ThumbnailContainer>,
}

#[derive(Debug, Deserialize)]
struct ThumbnailContainer {
    #[serde(default)]
    thumbnails: Option<Vec<Thumbnail>>,
}

#[derive(Debug, Deserialize)]
struct Thumbnail {
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    width: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StreamingData {
    #[serde(default)]
    adaptive_formats: Option<Vec<AdaptiveFormat>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AdaptiveFormat {
    #[serde(default)]
    quality_label: Option<String>,
    #[serde(default)]
    mime_type: Option<String>,
    #[serde(default)]
    content_length: Option<String>,
    #[serde(default)]
    width: Option<u32>,
    #[serde(default)]
    height: Option<u32>,
    #[serde(default)]
    bitrate: Option<u64>,
    #[serde(default)]
    fps: Option<u32>,
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_id_standard() {
        assert_eq!(
            extract_youtube_id("https://www.youtube.com/watch?v=dQw4w9WgXcQ"),
            Some("dQw4w9WgXcQ".to_string())
        );
    }

    #[test]
    fn extract_id_short() {
        assert_eq!(
            extract_youtube_id("https://youtu.be/dQw4w9WgXcQ?si=abc"),
            Some("dQw4w9WgXcQ".to_string())
        );
    }

    #[test]
    fn extract_id_shorts() {
        assert_eq!(
            extract_youtube_id("https://www.youtube.com/shorts/dQw4w9WgXcQ"),
            Some("dQw4w9WgXcQ".to_string())
        );
    }

    #[test]
    fn extract_id_music() {
        assert_eq!(
            extract_youtube_id("https://music.youtube.com/watch?v=dQw4w9WgXcQ"),
            Some("dQw4w9WgXcQ".to_string())
        );
    }

    #[test]
    fn extract_id_non_youtube() {
        assert_eq!(extract_youtube_id("https://soundcloud.com/x/y"), None);
    }

    #[tokio::test]
    #[ignore] // Run with: cargo test -p doracore -- --ignored test_scrape
    async fn test_scrape_real_video() {
        // "Me at the zoo" — first YouTube video, always available
        let meta = scrape_youtube_metadata("https://www.youtube.com/watch?v=jNQXAC9IVRw").await;
        assert!(meta.is_some(), "Should get metadata for 'Me at the zoo'");
        let meta = meta.unwrap();
        assert!(
            meta.title.to_lowercase().contains("zoo"),
            "Title should contain 'zoo': {}",
            meta.title
        );
        assert!(meta.duration_secs.is_some());
        println!(
            "Title: {}, Author: {:?}, Duration: {:?}s",
            meta.title, meta.author, meta.duration_secs
        );
        println!("Formats: {}", meta.video_formats.len());
        for f in &meta.video_formats {
            println!(
                "  {} ({}x{}) size={:?} bitrate={:?}",
                f.quality_label,
                f.width.unwrap_or(0),
                f.height.unwrap_or(0),
                f.content_length,
                f.bitrate
            );
        }
    }

    #[tokio::test]
    #[ignore] // Run with: cargo test -p doracore -- --ignored test_scrape
    async fn test_scrape_target_video() {
        // The video that was failing: "Два слова" — non-standard resolutions
        let meta = scrape_youtube_metadata("https://youtu.be/IM7TBEr72nE").await;
        assert!(meta.is_some(), "Should get metadata");
        let meta = meta.unwrap();
        println!(
            "Title: {}, Duration: {:?}s, Formats: {}",
            meta.title,
            meta.duration_secs,
            meta.video_formats.len()
        );
        for f in &meta.video_formats {
            println!(
                "  {} ({}x{}) size={:?}",
                f.quality_label,
                f.width.unwrap_or(0),
                f.height.unwrap_or(0),
                f.content_length
            );
        }
        assert!(!meta.video_formats.is_empty(), "Should have video formats");
    }

    #[tokio::test]
    #[ignore]
    async fn test_scrape_4k_video() {
        // Rick Astley 4K remaster
        let meta = scrape_youtube_metadata("https://www.youtube.com/watch?v=dQw4w9WgXcQ").await;
        assert!(meta.is_some());
        let meta = meta.unwrap();
        let has_1080 = meta.video_formats.iter().any(|f| f.quality_label.contains("1080"));
        println!(
            "Formats: {:?}",
            meta.video_formats.iter().map(|f| &f.quality_label).collect::<Vec<_>>()
        );
        assert!(has_1080, "Rick Astley should have at least 1080p");
    }
}
