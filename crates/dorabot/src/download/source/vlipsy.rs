//! VlipsySource — download source for vlipsy.com video reaction clips.
//!
//! Handles URLs like `https://vlipsy.com/clips/sus-dog-original-z2wRJ7aR`.
//! Scrapes clip metadata from og: meta tags (no API key required),
//! then downloads the MP4 with chunked transfer and progress tracking.

use crate::core::error::AppError;
use crate::download::error::DownloadError;
use crate::download::source::{DownloadOutput, DownloadRequest, DownloadSource, MediaMetadata, SourceProgress};
use async_trait::async_trait;
use regex::Regex;
use reqwest::Client;
use std::sync::LazyLock;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;
use url::Url;

// HIGH-13: Compile fixed-pattern regexes once at startup rather than on every
// call to extract_duration. The patterns in extract_meta are NOT hoisted here
// because they incorporate the `property` argument via format! and therefore
// differ per call.

/// Matches JSON-LD duration values like `"duration":5.39`.
static DUR_JSON_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""duration"\s*:\s*([0-9]+(?:\.[0-9]+)?)"#).expect("valid regex"));

/// Matches description duration hints like `(6s)` or `(12s)`.
static DUR_DESC_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\((\d+)s\)").expect("valid regex"));

/// Download source for Vlipsy video clips (works without API key).
pub struct VlipsySource {
    http: Client,
}

impl Default for VlipsySource {
    fn default() -> Self {
        Self::new()
    }
}

impl VlipsySource {
    pub fn new() -> Self {
        let http = Client::builder()
            .user_agent("doradura/0.14")
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .expect("Failed to create HTTP client");
        Self { http }
    }
}

/// Extract the clip slug from a Vlipsy URL path.
///
/// Handles paths like `/clips/sus-dog-original-z2wRJ7aR` or `/vlip/abcXYZ`.
fn extract_clip_slug(url: &Url) -> Option<String> {
    let path = url.path();
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

    match segments.as_slice() {
        ["clips", slug] | ["vlip", slug] => Some(slug.to_string()),
        _ => None,
    }
}

/// Metadata scraped from a Vlipsy clip HTML page.
pub struct ScrapedClipInfo {
    pub title: String,
    pub mp4_url: String,
    pub duration_secs: Option<u32>,
    pub thumbnail_url: Option<String>,
}

/// Extract a meta tag content by property or name attribute.
fn extract_meta(html: &str, property: &str) -> Option<String> {
    // Match name="og:video" content="..." (Vlipsy uses `name` for og:video)
    let pat1 = format!(
        r#"<meta\s+name="{prop}"[^>]*\scontent="([^"]*)"#,
        prop = regex::escape(property)
    );
    if let Some(caps) = Regex::new(&pat1).ok().and_then(|re| re.captures(html)) {
        return Some(html_decode(&caps[1]));
    }

    // Match property="og:title" content="..." (Vlipsy uses `property` for og:title)
    let pat2 = format!(
        r#"<meta\s+property="{prop}"[^>]*\scontent="([^"]*)"#,
        prop = regex::escape(property)
    );
    if let Some(caps) = Regex::new(&pat2).ok().and_then(|re| re.captures(html)) {
        return Some(html_decode(&caps[1]));
    }

    None
}

/// Decode basic HTML entities.
fn html_decode(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

/// Extract duration in seconds from the description meta tag.
///
/// Matches patterns like "(6s)" or "(12s)".
fn extract_duration(html: &str) -> Option<u32> {
    // Try JSON-LD duration first: "duration":5.39
    // DUR_JSON_RE is a module-level LazyLock — compiled exactly once.
    if let Some(caps) = DUR_JSON_RE.captures(html) {
        if let Ok(secs) = caps[1].parse::<f64>() {
            return Some(secs.ceil() as u32);
        }
    }

    // Fallback: description "(Xs)"
    // DUR_DESC_RE is a module-level LazyLock — compiled exactly once.
    if let Some(caps) = DUR_DESC_RE.captures(html) {
        if let Ok(secs) = caps[1].parse::<u32>() {
            return Some(secs);
        }
    }

    None
}

/// Scrape clip info from a Vlipsy HTML page.
pub async fn scrape_clip_page(http: &Client, page_url: &Url) -> Result<ScrapedClipInfo, AppError> {
    let resp = http
        .get(page_url.as_str())
        .send()
        .await
        .map_err(|e| AppError::Download(DownloadError::Vlipsy(format!("Failed to fetch page: {}", e))))?;

    if !resp.status().is_success() {
        return Err(AppError::Download(DownloadError::Vlipsy(format!(
            "HTTP {} fetching clip page",
            resp.status()
        ))));
    }

    let html = resp
        .text()
        .await
        .map_err(|e| AppError::Download(DownloadError::Vlipsy(format!("Failed to read page: {}", e))))?;

    let mp4_url = extract_meta(&html, "og:video")
        .ok_or_else(|| AppError::Download(DownloadError::Vlipsy("No og:video found on page".into())))?;

    let title = extract_meta(&html, "og:title")
        .unwrap_or_else(|| "Vlipsy Clip".to_string())
        .trim_end_matches(" | Vlipsy")
        .to_string();

    let duration_secs = extract_duration(&html);
    let thumbnail_url = extract_meta(&html, "og:image");

    // Try non-watermarked URL (480p.mp4 instead of 480p-watermark.mp4)
    let mp4_url = mp4_url.replace("480p-watermark.mp4", "480p.mp4");

    Ok(ScrapedClipInfo {
        title,
        mp4_url,
        duration_secs,
        thumbnail_url,
    })
}

#[async_trait]
impl DownloadSource for VlipsySource {
    fn name(&self) -> &str {
        "vlipsy"
    }

    fn supports_url(&self, url: &Url) -> bool {
        let host = url.host_str().unwrap_or("");
        (host == "vlipsy.com" || host == "www.vlipsy.com") && extract_clip_slug(url).is_some()
    }

    async fn get_metadata(&self, url: &Url) -> Result<MediaMetadata, AppError> {
        let info = scrape_clip_page(&self.http, url).await?;
        Ok(MediaMetadata {
            title: info.title,
            artist: String::new(),
        })
    }

    async fn estimate_size(&self, _url: &Url) -> Option<u64> {
        None
    }

    async fn is_livestream(&self, _url: &Url) -> bool {
        false
    }

    async fn download(
        &self,
        request: &DownloadRequest,
        progress_tx: mpsc::UnboundedSender<SourceProgress>,
    ) -> Result<DownloadOutput, AppError> {
        log::info!("📥 Vlipsy download: {}", request.url);

        let info = scrape_clip_page(&self.http, &request.url).await?;

        // Download MP4 via HTTP
        let response = self
            .http
            .get(&info.mp4_url)
            .send()
            .await
            .map_err(|e| AppError::Download(DownloadError::Vlipsy(format!("Failed to download MP4: {}", e))))?;

        if !response.status().is_success() {
            return Err(AppError::Download(DownloadError::Vlipsy(format!(
                "HTTP {} downloading MP4",
                response.status()
            ))));
        }

        let total_size = response.content_length();

        // Ensure parent directory exists (DOWNLOAD_FOLDER may not exist yet)
        if let Some(parent) = std::path::Path::new(&request.output_path).parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| AppError::Download(DownloadError::Vlipsy(format!("Failed to create directory: {}", e))))?;
        }

        // LOW-05: use async I/O so the tokio executor thread is not blocked during writes.
        let mut file = tokio::fs::File::create(&request.output_path)
            .await
            .map_err(|e| AppError::Download(DownloadError::Vlipsy(format!("Failed to create file: {}", e))))?;

        let mut downloaded: u64 = 0;
        let mut last_progress_percent = 0u8;

        let mut stream = response.bytes_stream();
        use futures_util::StreamExt;

        while let Some(chunk_result) = stream.next().await {
            let chunk = chunk_result
                .map_err(|e| AppError::Download(DownloadError::Vlipsy(format!("Error reading chunk: {}", e))))?;

            file.write_all(&chunk)
                .await
                .map_err(|e| AppError::Download(DownloadError::Vlipsy(format!("Error writing file: {}", e))))?;

            downloaded += chunk.len() as u64;

            let percent = total_size
                .map(|total| {
                    if total > 0 {
                        ((downloaded as f64 / total as f64) * 100.0) as u8
                    } else {
                        0
                    }
                })
                .unwrap_or(0);

            if percent >= last_progress_percent + 10 || percent == 100 {
                last_progress_percent = percent;
                let _ = progress_tx.send(SourceProgress {
                    percent,
                    speed_bytes_sec: None,
                    eta_seconds: None,
                    downloaded_bytes: Some(downloaded),
                    total_bytes: total_size,
                });
            }
        }

        file.flush()
            .await
            .map_err(|e| AppError::Download(DownloadError::Vlipsy(format!("Failed to flush file: {}", e))))?;

        let file_size = std::fs::metadata(&request.output_path)
            .map(|m| m.len())
            .unwrap_or(downloaded);

        log::info!(
            "✅ Vlipsy download complete: {} ({:.2} MB)",
            request.output_path,
            file_size as f64 / (1024.0 * 1024.0)
        );

        Ok(DownloadOutput {
            file_path: request.output_path.clone(),
            duration_secs: info.duration_secs,
            file_size,
            mime_hint: Some("video/mp4".into()),
            additional_files: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_clip_slug_clips() {
        let url = Url::parse("https://vlipsy.com/clips/sus-dog-original-z2wRJ7aR").unwrap();
        assert_eq!(extract_clip_slug(&url), Some("sus-dog-original-z2wRJ7aR".into()));
    }

    #[test]
    fn test_extract_clip_slug_vlip() {
        let url = Url::parse("https://vlipsy.com/vlip/abc123").unwrap();
        assert_eq!(extract_clip_slug(&url), Some("abc123".into()));
    }

    #[test]
    fn test_extract_clip_slug_www() {
        let url = Url::parse("https://www.vlipsy.com/clips/test-clip-xyz").unwrap();
        assert_eq!(extract_clip_slug(&url), Some("test-clip-xyz".into()));
    }

    #[test]
    fn test_rejects_other_paths() {
        let urls = [
            "https://youtube.com/watch?v=abc",
            "https://vlipsy.com/",
            "https://vlipsy.com/about",
            "https://vlipsy.com/search?q=funny",
        ];
        for u in &urls {
            let url = Url::parse(u).unwrap();
            assert!(extract_clip_slug(&url).is_none(), "Should reject: {}", u);
        }
    }

    #[test]
    fn test_extract_clip_slug_no_path() {
        let url = Url::parse("https://vlipsy.com").unwrap();
        assert!(extract_clip_slug(&url).is_none());
    }

    #[test]
    fn test_supports_url() {
        let source = VlipsySource::new();
        assert!(source.supports_url(&Url::parse("https://vlipsy.com/clips/sus-dog-original-z2wRJ7aR").unwrap()));
        assert!(source.supports_url(&Url::parse("https://www.vlipsy.com/clips/test-xyz").unwrap()));
        assert!(!source.supports_url(&Url::parse("https://youtube.com/watch?v=abc").unwrap()));
        assert!(!source.supports_url(&Url::parse("https://vlipsy.com/about").unwrap()));
    }

    #[test]
    fn test_extract_meta_og_video() {
        let html = r#"<meta name="og:video" content="https://cdn.vlipsy.com/clips/meta/z2wRJ7aR/480p-watermark.mp4"/>"#;
        assert_eq!(
            extract_meta(html, "og:video"),
            Some("https://cdn.vlipsy.com/clips/meta/z2wRJ7aR/480p-watermark.mp4".into())
        );
    }

    #[test]
    fn test_extract_meta_og_title() {
        let html = r#"<meta property="og:title" content="Sus dog (original) | Vlipsy"/>"#;
        assert_eq!(
            extract_meta(html, "og:title"),
            Some("Sus dog (original) | Vlipsy".into())
        );
    }

    #[test]
    fn test_extract_meta_html_entities() {
        let html = r#"<meta property="og:title" content="It&#x27;s a &amp; test"/>"#;
        assert_eq!(extract_meta(html, "og:title"), Some("It's a & test".into()));
    }

    #[test]
    fn test_extract_meta_missing() {
        let html = r#"<meta property="og:image" content="pic.jpg"/>"#;
        assert_eq!(extract_meta(html, "og:video"), None);
    }

    #[test]
    fn test_extract_duration_json_ld() {
        let html = r#"something "duration":5.39 something"#;
        assert_eq!(extract_duration(html), Some(6)); // ceil(5.39) = 6
    }

    #[test]
    fn test_extract_duration_description() {
        let html = r#"HD video clip (6s) perfect for reactions"#;
        assert_eq!(extract_duration(html), Some(6));
    }

    #[test]
    fn test_extract_duration_none() {
        let html = r#"no duration info here"#;
        assert_eq!(extract_duration(html), None);
    }

    #[test]
    fn test_html_decode() {
        assert_eq!(html_decode("it&#x27;s &amp; fun"), "it's & fun");
        assert_eq!(html_decode("&lt;b&gt;bold&lt;/b&gt;"), "<b>bold</b>");
    }

    // ── Real API tests (require network) ──

    #[tokio::test]
    #[ignore = "requires external vlipsy.com network access"]
    async fn test_real_scrape_clip_page() {
        let http = Client::builder()
            .user_agent("doradura-test/0.14")
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .unwrap();

        let url = Url::parse("https://vlipsy.com/clips/sus-dog-original-z2wRJ7aR").unwrap();
        let info = scrape_clip_page(&http, &url).await.expect("Failed to scrape clip page");

        assert_eq!(info.title, "Sus dog (original)");
        assert!(
            info.mp4_url.contains("cdn.vlipsy.com"),
            "MP4 URL should be from CDN: {}",
            info.mp4_url
        );
        assert!(info.mp4_url.ends_with(".mp4"), "Should be MP4: {}", info.mp4_url);
        assert!(info.duration_secs.is_some(), "Should have duration");
        assert!(info.duration_secs.unwrap() > 0, "Duration should be > 0");
    }

    #[tokio::test]
    #[ignore = "requires external vlipsy.com network access"]
    async fn test_real_scrape_another_clip() {
        let http = Client::builder()
            .user_agent("doradura-test/0.14")
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .unwrap();

        let url = Url::parse("https://vlipsy.com/clips/mr-bean-magic-sVUWucuf").unwrap();
        let info = scrape_clip_page(&http, &url).await.expect("Failed to scrape clip page");

        assert_eq!(info.title, "Mr Bean - Magic");
        assert!(info.mp4_url.contains("cdn.vlipsy.com"));
        assert!(info.mp4_url.contains("sVUWucuf"));
    }

    #[tokio::test]
    #[ignore = "requires external vlipsy.com network access"]
    async fn test_real_mp4_accessible() {
        let http = Client::builder()
            .user_agent("doradura-test/0.14")
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .unwrap();

        let url = Url::parse("https://vlipsy.com/clips/sus-dog-original-z2wRJ7aR").unwrap();
        let info = scrape_clip_page(&http, &url).await.unwrap();

        // Verify the MP4 URL is actually downloadable
        let resp = http.head(&info.mp4_url).send().await.expect("HEAD request failed");
        assert!(
            resp.status().is_success(),
            "MP4 should be accessible: HTTP {}",
            resp.status()
        );

        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(
            content_type.contains("video") || content_type.contains("mp4"),
            "Content-Type should be video: {}",
            content_type
        );
    }

    #[tokio::test]
    #[ignore = "requires external vlipsy.com network access"]
    async fn test_real_get_metadata() {
        let source = VlipsySource::new();
        let url = Url::parse("https://vlipsy.com/clips/sus-dog-original-z2wRJ7aR").unwrap();

        let meta = source.get_metadata(&url).await.expect("get_metadata failed");
        assert_eq!(meta.title, "Sus dog (original)");
    }

    #[tokio::test]
    #[ignore = "requires external vlipsy.com network access"]
    async fn test_real_download() {
        let source = VlipsySource::new();
        let url = Url::parse("https://vlipsy.com/clips/sus-dog-original-z2wRJ7aR").unwrap();

        let tmp = tempfile::NamedTempFile::new().unwrap();
        let output_path = tmp.path().to_string_lossy().to_string();

        let request = DownloadRequest {
            url: url.clone(),
            output_path: output_path.clone(),
            format: "mp4".to_string(),
            audio_bitrate: None,
            video_quality: None,
            max_file_size: None,
            time_range: None,
            carousel_mask: None,
            concurrent_fragments: 1,
        };

        let (progress_tx, mut progress_rx) = mpsc::unbounded_channel();

        let result = source.download(&request, progress_tx).await.expect("download failed");

        assert_eq!(result.file_path, output_path);
        assert!(result.file_size > 0, "File should not be empty");
        assert_eq!(result.mime_hint.as_deref(), Some("video/mp4"));
        assert!(result.duration_secs.is_some(), "Should have duration");

        // Check we got progress updates
        let mut got_progress = false;
        while let Ok(p) = progress_rx.try_recv() {
            got_progress = true;
            assert!(p.percent <= 100);
        }
        assert!(got_progress, "Should have received progress updates");
    }
}
