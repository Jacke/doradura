//! Fast YouTube metadata via external APIs (Piped, oEmbed).
//!
//! Fallback chain: Piped (~1-2s, has formats) → oEmbed (~200ms, title only).
//! Both bypass yt-dlp's JS challenge solving which takes 5-15s.

use serde::Deserialize;
use std::time::Duration;

/// Metadata retrieved from fast external APIs.
#[derive(Debug, Clone)]
pub struct FastMetadata {
    pub title: String,
    pub author: Option<String>,
    pub duration_secs: Option<u32>,
    pub thumbnail_url: Option<String>,
    /// Video streams with quality info (only from Piped).
    pub video_formats: Vec<FastVideoFormat>,
    /// Which source provided the data.
    pub source: FastMetadataSource,
}

#[derive(Debug, Clone)]
pub struct FastVideoFormat {
    pub quality: String, // "1080p", "720p", etc.
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fps: Option<u32>,
    pub bitrate: Option<u64>,
    pub content_length: Option<u64>,
    pub codec: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FastMetadataSource {
    Piped,
    OEmbed,
}

/// Default Piped API instances to try (in order).
const PIPED_INSTANCES: &[&str] = &[
    "https://pipedapi.kavin.rocks",
    "https://pipedapi.adminforge.de",
    "https://pipedapi.darkness.services",
];

const REQUEST_TIMEOUT: Duration = Duration::from_secs(4);

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

        // /watch?v=ID
        if path.starts_with("/watch") {
            return parsed
                .query_pairs()
                .find(|(k, _)| k == "v")
                .map(|(_, v)| v.into_owned());
        }

        // /shorts/ID, /live/ID, /embed/ID
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

/// Try to get fast metadata for a YouTube URL.
///
/// Fallback chain: Piped → oEmbed.
/// Returns `None` if all sources fail or URL is not YouTube.
pub async fn get_fast_youtube_metadata(url: &str) -> Option<FastMetadata> {
    let video_id = extract_youtube_id(url)?;

    // Try Piped first (has formats + duration)
    match try_piped(&video_id).await {
        Ok(meta) => {
            log::info!(
                "⚡ Fast metadata from Piped for {} ({} formats)",
                video_id,
                meta.video_formats.len()
            );
            return Some(meta);
        }
        Err(e) => {
            log::debug!("Piped failed for {}: {}", video_id, e);
        }
    }

    // Fallback: oEmbed (title + author only, but very fast)
    match try_oembed(&video_id).await {
        Ok(meta) => {
            log::info!("⚡ Fast metadata from oEmbed for {}", video_id);
            return Some(meta);
        }
        Err(e) => {
            log::debug!("oEmbed failed for {}: {}", video_id, e);
        }
    }

    log::warn!("All fast metadata sources failed for {}", video_id);
    None
}

// ── Piped ───────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct PipedResponse {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    uploader: Option<String>,
    #[serde(default)]
    duration: Option<f64>,
    #[serde(default, alias = "thumbnailUrl")]
    thumbnail_url: Option<String>,
    #[serde(default, alias = "videoStreams")]
    video_streams: Vec<PipedVideoStream>,
}

#[derive(Debug, Deserialize)]
struct PipedVideoStream {
    #[serde(default)]
    quality: Option<String>,
    #[serde(default)]
    width: Option<u32>,
    #[serde(default)]
    height: Option<u32>,
    #[serde(default)]
    fps: Option<u32>,
    #[serde(default)]
    bitrate: Option<u64>,
    #[serde(default, alias = "contentLength")]
    content_length: Option<u64>,
    #[serde(default)]
    codec: Option<String>,
}

async fn try_piped(video_id: &str) -> Result<FastMetadata, String> {
    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|e| format!("HTTP client error: {}", e))?;

    for instance in PIPED_INSTANCES {
        let url = format!("{}/streams/{}", instance, video_id);
        match client.get(&url).send().await {
            Ok(resp) if resp.status().is_success() => {
                match resp.json::<PipedResponse>().await {
                    Ok(data) => {
                        let title = data.title.unwrap_or_default();
                        if title.is_empty() {
                            continue; // Bad response
                        }

                        let video_formats: Vec<FastVideoFormat> = data
                            .video_streams
                            .into_iter()
                            .filter_map(|s| {
                                let quality = s.quality?;
                                if quality == "unknown" {
                                    return None;
                                }
                                Some(FastVideoFormat {
                                    quality,
                                    width: s.width,
                                    height: s.height,
                                    fps: s.fps,
                                    bitrate: s.bitrate,
                                    content_length: s.content_length,
                                    codec: s.codec,
                                })
                            })
                            .collect();

                        return Ok(FastMetadata {
                            title,
                            author: data.uploader,
                            duration_secs: data.duration.map(|d| d as u32),
                            thumbnail_url: data.thumbnail_url,
                            video_formats,
                            source: FastMetadataSource::Piped,
                        });
                    }
                    Err(e) => {
                        log::debug!("Piped {} JSON parse error: {}", instance, e);
                        continue;
                    }
                }
            }
            Ok(resp) => {
                log::debug!("Piped {} returned {}", instance, resp.status());
                continue;
            }
            Err(e) => {
                log::debug!("Piped {} request error: {}", instance, e);
                continue;
            }
        }
    }

    Err("All Piped instances failed".to_string())
}

// ── oEmbed ──────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct OEmbedResponse {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    author_name: Option<String>,
    #[serde(default)]
    thumbnail_url: Option<String>,
}

async fn try_oembed(video_id: &str) -> Result<FastMetadata, String> {
    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|e| format!("HTTP client error: {}", e))?;

    let video_url = format!("https://www.youtube.com/watch?v={}", video_id);
    let oembed_url = format!(
        "https://www.youtube.com/oembed?url={}&format=json",
        urlencoding::encode(&video_url)
    );

    let resp = client
        .get(&oembed_url)
        .send()
        .await
        .map_err(|e| format!("oEmbed request error: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("oEmbed returned {}", resp.status()));
    }

    let data: OEmbedResponse = resp
        .json()
        .await
        .map_err(|e| format!("oEmbed JSON parse error: {}", e))?;

    let title = data.title.unwrap_or_default();
    if title.is_empty() {
        return Err("oEmbed returned empty title".to_string());
    }

    // Standard YouTube thumbnail (predictable from video ID)
    let thumbnail = data
        .thumbnail_url
        .or_else(|| Some(format!("https://i.ytimg.com/vi/{}/hqdefault.jpg", video_id)));

    Ok(FastMetadata {
        title,
        author: data.author_name,
        duration_secs: None, // oEmbed doesn't provide duration
        thumbnail_url: thumbnail,
        video_formats: vec![], // oEmbed doesn't provide formats
        source: FastMetadataSource::OEmbed,
    })
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── extract_youtube_id ──────────────────────────────────────────

    #[test]
    fn extract_id_standard() {
        assert_eq!(
            extract_youtube_id("https://www.youtube.com/watch?v=dQw4w9WgXcQ"),
            Some("dQw4w9WgXcQ".to_string())
        );
    }

    #[test]
    fn extract_id_short_link() {
        assert_eq!(
            extract_youtube_id("https://youtu.be/dQw4w9WgXcQ"),
            Some("dQw4w9WgXcQ".to_string())
        );
    }

    #[test]
    fn extract_id_short_link_with_params() {
        assert_eq!(
            extract_youtube_id("https://youtu.be/dQw4w9WgXcQ?si=abc&t=42"),
            Some("dQw4w9WgXcQ".to_string())
        );
    }

    #[test]
    fn extract_id_music() {
        assert_eq!(
            extract_youtube_id("https://music.youtube.com/watch?v=dQw4w9WgXcQ&si=abc"),
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
    fn extract_id_live() {
        assert_eq!(
            extract_youtube_id("https://www.youtube.com/live/dQw4w9WgXcQ"),
            Some("dQw4w9WgXcQ".to_string())
        );
    }

    #[test]
    fn extract_id_embed() {
        assert_eq!(
            extract_youtube_id("https://www.youtube.com/embed/dQw4w9WgXcQ"),
            Some("dQw4w9WgXcQ".to_string())
        );
    }

    #[test]
    fn extract_id_mobile() {
        assert_eq!(
            extract_youtube_id("https://m.youtube.com/watch?v=dQw4w9WgXcQ"),
            Some("dQw4w9WgXcQ".to_string())
        );
    }

    #[test]
    fn extract_id_non_youtube() {
        assert_eq!(extract_youtube_id("https://soundcloud.com/artist/track"), None);
    }

    #[test]
    fn extract_id_invalid() {
        assert_eq!(extract_youtube_id("not-a-url"), None);
    }

    // ── Integration tests (require network) ─────────────────────────

    #[tokio::test]
    #[ignore] // Run with: cargo test -p doracore -- --ignored fast_metadata
    async fn test_piped_real() {
        // "Me at the zoo" — first YouTube video ever, always available
        let result = try_piped("jNQXAC9IVRw").await;
        match result {
            Ok(meta) => {
                assert!(meta.title.contains("zoo") || meta.title.contains("Zoo"));
                assert!(meta.duration_secs.is_some());
                println!(
                    "Piped: {} by {:?} ({}s)",
                    meta.title,
                    meta.author,
                    meta.duration_secs.unwrap_or(0)
                );
                println!("  {} video formats", meta.video_formats.len());
                for f in &meta.video_formats {
                    println!("  - {} ({}x{})", f.quality, f.width.unwrap_or(0), f.height.unwrap_or(0));
                }
            }
            Err(e) => {
                println!("Piped unavailable (expected in CI): {}", e);
            }
        }
    }

    #[tokio::test]
    #[ignore] // Run with: cargo test -p doracore -- --ignored fast_metadata
    async fn test_oembed_real() {
        let result = try_oembed("jNQXAC9IVRw").await;
        match result {
            Ok(meta) => {
                assert!(meta.title.contains("zoo") || meta.title.contains("Zoo"));
                assert!(meta.author.is_some());
                assert_eq!(meta.source, FastMetadataSource::OEmbed);
                println!("oEmbed: {} by {:?}", meta.title, meta.author);
            }
            Err(e) => {
                panic!("oEmbed should work: {}", e);
            }
        }
    }

    #[tokio::test]
    #[ignore] // Run with: cargo test -p doracore -- --ignored fast_metadata
    async fn test_full_chain() {
        let result = get_fast_youtube_metadata("https://www.youtube.com/watch?v=jNQXAC9IVRw").await;
        assert!(result.is_some(), "At least one source should work");
        let meta = result.unwrap();
        println!(
            "Fast metadata: {} (source: {:?}, formats: {})",
            meta.title,
            meta.source,
            meta.video_formats.len()
        );
    }
}
