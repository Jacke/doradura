//! HttpSource — direct HTTP download source with chunked transfer and resume support.
//!
//! Handles direct file URLs (e.g., `https://example.com/file.mp3`).
//! Features:
//! - Chunked download with progress tracking via reqwest
//! - Resume via HTTP Range headers (if server supports it)
//! - Content-Disposition parsing for filename
//! - HEAD request for size estimation
//! - Fallback source for any http/https URL not handled by YtDlpSource

use crate::core::error::AppError;
use crate::download::source::{DownloadOutput, DownloadRequest, DownloadSource, SourceProgress};
use async_trait::async_trait;
use reqwest::Client;
use std::io::Write;
use tokio::sync::mpsc;
use url::Url;

/// Known direct-file extensions this source handles.
const DIRECT_FILE_EXTENSIONS: &[&str] = &[
    "mp3", "mp4", "wav", "flac", "ogg", "m4a", "webm", "avi", "mkv", "aac", "opus",
];

/// Download source for direct HTTP file downloads.
pub struct HttpSource {
    client: Client,
}

impl Default for HttpSource {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpSource {
    pub fn new() -> Self {
        let client = Client::builder()
            .user_agent("Mozilla/5.0 (compatible; doradura/0.2)")
            .timeout(std::time::Duration::from_secs(600))
            .connect_timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("HTTP client build failed: user_agent + timeout config should always succeed");

        Self { client }
    }

    /// Extract filename from Content-Disposition header or URL path.
    fn extract_filename(response: &reqwest::Response, url: &Url) -> String {
        // Try Content-Disposition header first
        if let Some(cd) = response.headers().get("content-disposition") {
            if let Ok(cd_str) = cd.to_str() {
                // Parse: attachment; filename="file.mp3" or filename*=UTF-8''file.mp3
                if let Some(start) = cd_str.find("filename=") {
                    let value = &cd_str[start + 9..];
                    let filename = value.trim_start_matches('"').split('"').next().unwrap_or("download");
                    if !filename.is_empty() {
                        return filename.to_string();
                    }
                }
            }
        }

        // Fallback: extract from URL path
        url.path_segments()
            .and_then(|mut segments| segments.next_back())
            .filter(|s| !s.is_empty())
            .map(|s| {
                // URL-decode the filename
                urlencoding::decode(s).unwrap_or_else(|_| s.into()).to_string()
            })
            .unwrap_or_else(|| "download".to_string())
    }

    /// Guess MIME type from file extension.
    fn mime_from_extension(path: &str) -> Option<String> {
        let ext = path.rsplit('.').next()?.to_lowercase();
        match ext.as_str() {
            "mp3" => Some("audio/mpeg".to_string()),
            "mp4" => Some("video/mp4".to_string()),
            "wav" => Some("audio/wav".to_string()),
            "flac" => Some("audio/flac".to_string()),
            "ogg" => Some("audio/ogg".to_string()),
            "m4a" => Some("audio/mp4".to_string()),
            "webm" => Some("video/webm".to_string()),
            "avi" => Some("video/x-msvideo".to_string()),
            "mkv" => Some("video/x-matroska".to_string()),
            "aac" => Some("audio/aac".to_string()),
            "opus" => Some("audio/opus".to_string()),
            _ => None,
        }
    }
}

#[async_trait]
impl DownloadSource for HttpSource {
    fn name(&self) -> &str {
        "http"
    }

    fn supports_url(&self, url: &Url) -> bool {
        let scheme = url.scheme();
        if scheme != "http" && scheme != "https" {
            return false;
        }

        // Check if URL path ends with a known file extension
        let path = url.path().to_lowercase();
        DIRECT_FILE_EXTENSIONS
            .iter()
            .any(|ext| path.ends_with(&format!(".{}", ext)))
    }

    async fn get_metadata(&self, url: &Url) -> Result<crate::download::source::MediaMetadata, AppError> {
        // For direct HTTP files, title is the filename and artist is empty
        let filename = url
            .path_segments()
            .and_then(|mut segments| segments.next_back())
            .filter(|s| !s.is_empty())
            .map(|s| urlencoding::decode(s).unwrap_or_else(|_| s.into()).to_string())
            .unwrap_or_else(|| "Download".to_string());

        // Strip extension from title
        let title = if let Some(dot_pos) = filename.rfind('.') {
            filename[..dot_pos].to_string()
        } else {
            filename
        };

        Ok(crate::download::source::MediaMetadata {
            title,
            artist: String::new(),
        })
    }

    async fn estimate_size(&self, url: &Url) -> Option<u64> {
        let response = self.client.head(url.as_str()).send().await.ok()?;
        response.content_length()
    }

    async fn is_livestream(&self, _url: &Url) -> bool {
        false // Direct HTTP files are never livestreams
    }

    async fn download(
        &self,
        request: &DownloadRequest,
        progress_tx: mpsc::UnboundedSender<SourceProgress>,
    ) -> Result<DownloadOutput, AppError> {
        log::info!("📥 HTTP direct download: {}", request.url);

        // Check if we can resume (file already partially downloaded)
        let existing_size = std::fs::metadata(&request.output_path).map(|m| m.len()).unwrap_or(0);

        let mut req = self.client.get(request.url.as_str());

        if existing_size > 0 {
            log::info!("Resuming download from byte {}: {}", existing_size, request.output_path);
            req = req.header("Range", format!("bytes={}-", existing_size));
        }

        let response = req
            .send()
            .await
            .map_err(|e| AppError::Download(format!("HTTP request failed: {}", e)))?;

        if !response.status().is_success() && response.status().as_u16() != 206 {
            return Err(AppError::Download(format!(
                "HTTP {} for {}",
                response.status(),
                request.url
            )));
        }

        let is_partial = response.status().as_u16() == 206;
        let total_size = if is_partial {
            // Parse Content-Range header for total size
            response
                .headers()
                .get("content-range")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.rsplit('/').next())
                .and_then(|s| s.parse::<u64>().ok())
        } else {
            response.content_length()
        };

        let _filename = Self::extract_filename(&response, &request.url);
        let mime_hint = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
            .or_else(|| Self::mime_from_extension(&request.output_path));

        // Open file for writing (append if resuming)
        let mut file = if is_partial && existing_size > 0 {
            std::fs::OpenOptions::new()
                .append(true)
                .open(&request.output_path)
                .map_err(|e| AppError::Download(format!("Failed to open file for resume: {}", e)))?
        } else {
            std::fs::File::create(&request.output_path)
                .map_err(|e| AppError::Download(format!("Failed to create file: {}", e)))?
        };

        let mut downloaded: u64 = if is_partial { existing_size } else { 0 };
        let mut last_progress_percent = 0u8;

        // Stream response body in chunks
        let mut stream = response.bytes_stream();
        use futures_util::StreamExt;

        while let Some(chunk_result) = stream.next().await {
            let chunk = chunk_result.map_err(|e| AppError::Download(format!("Error reading chunk: {}", e)))?;

            file.write_all(&chunk)
                .map_err(|e| AppError::Download(format!("Error writing to file: {}", e)))?;

            downloaded += chunk.len() as u64;

            // Check max file size
            if let Some(max_size) = request.max_file_size {
                if downloaded > max_size {
                    let _ = std::fs::remove_file(&request.output_path);
                    return Err(AppError::Validation(format!(
                        "File exceeds maximum size: {} bytes > {} bytes",
                        downloaded, max_size
                    )));
                }
            }

            // Send progress
            let percent = total_size
                .map(|total| {
                    if total > 0 {
                        ((downloaded as f64 / total as f64) * 100.0) as u8
                    } else {
                        0
                    }
                })
                .unwrap_or(0);

            if percent >= last_progress_percent + 5 || percent == 100 {
                last_progress_percent = percent;
                let _ = progress_tx.send(SourceProgress {
                    percent,
                    speed_bytes_sec: None, // Could add rate calculation
                    eta_seconds: None,
                    downloaded_bytes: Some(downloaded),
                    total_bytes: total_size,
                });
            }
        }

        file.flush()
            .map_err(|e| AppError::Download(format!("Failed to flush file: {}", e)))?;

        let file_size = std::fs::metadata(&request.output_path)
            .map(|m| m.len())
            .unwrap_or(downloaded);

        log::info!(
            "✅ HTTP download complete: {} ({:.2} MB)",
            request.output_path,
            file_size as f64 / (1024.0 * 1024.0)
        );

        // Probe duration if it's a media file
        let duration_secs = crate::download::metadata::probe_duration_seconds(&request.output_path);

        Ok(DownloadOutput {
            file_path: request.output_path.clone(),
            duration_secs,
            file_size,
            mime_hint,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_supports_url_mp3() {
        let source = HttpSource::new();
        let url = Url::parse("https://example.com/music/file.mp3").unwrap();
        assert!(source.supports_url(&url));
    }

    #[test]
    fn test_supports_url_mp4() {
        let source = HttpSource::new();
        let url = Url::parse("https://cdn.example.com/video.mp4").unwrap();
        assert!(source.supports_url(&url));
    }

    #[test]
    fn test_rejects_html_page() {
        let source = HttpSource::new();
        let url = Url::parse("https://example.com/page").unwrap();
        assert!(!source.supports_url(&url));
    }

    #[test]
    fn test_rejects_youtube() {
        let source = HttpSource::new();
        let url = Url::parse("https://www.youtube.com/watch?v=abc").unwrap();
        assert!(!source.supports_url(&url));
    }

    #[test]
    fn test_supports_flac() {
        let source = HttpSource::new();
        let url = Url::parse("https://example.com/audio.flac").unwrap();
        assert!(source.supports_url(&url));
    }

    #[test]
    fn test_mime_from_extension() {
        assert_eq!(
            HttpSource::mime_from_extension("file.mp3"),
            Some("audio/mpeg".to_string())
        );
        assert_eq!(
            HttpSource::mime_from_extension("video.mp4"),
            Some("video/mp4".to_string())
        );
        assert_eq!(HttpSource::mime_from_extension("file.xyz"), None);
    }
}
