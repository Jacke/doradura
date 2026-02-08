//! Multi-backend download source abstraction layer.
//!
//! Provides the `DownloadSource` trait for implementing pluggable download backends
//! and a `SourceRegistry` for URL-based routing. New backends are added by implementing
//! `DownloadSource` and registering them in the registry.
//!
//! Built-in backends:
//! - `YtDlpSource` — 1000+ sites via yt-dlp (YouTube, SoundCloud, TikTok, etc.)
//! - `HttpSource` — direct file URLs (MP3, MP4, etc.) with chunked download + resume

pub mod http;
pub mod ytdlp;

use crate::core::error::AppError;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::mpsc;
use url::Url;

/// Progress information emitted during download.
#[derive(Debug, Clone)]
pub struct SourceProgress {
    /// Download progress percentage (0-100)
    pub percent: u8,
    /// Download speed in bytes per second
    pub speed_bytes_sec: Option<f64>,
    /// Estimated time remaining in seconds
    pub eta_seconds: Option<u64>,
    /// Bytes downloaded so far
    pub downloaded_bytes: Option<u64>,
    /// Total bytes expected
    pub total_bytes: Option<u64>,
}

impl SourceProgress {
    /// Convert to the existing ProgressInfo type used by the progress UI.
    pub fn to_progress_info(&self) -> crate::download::downloader::ProgressInfo {
        crate::download::downloader::ProgressInfo {
            percent: self.percent,
            speed_mbs: self.speed_bytes_sec.map(|b| b / (1024.0 * 1024.0)),
            eta_seconds: self.eta_seconds,
            current_size: self.downloaded_bytes,
            total_size: self.total_bytes,
        }
    }
}

/// Request parameters for a download operation.
#[derive(Debug, Clone)]
pub struct DownloadRequest {
    /// URL to download from
    pub url: Url,
    /// Local path to save the downloaded file
    pub output_path: String,
    /// Target format (e.g., "mp3", "mp4")
    pub format: String,
    /// Audio bitrate (e.g., "320k") - relevant for audio downloads
    pub audio_bitrate: Option<String>,
    /// Video quality (e.g., "720p") - relevant for video downloads
    pub video_quality: Option<String>,
    /// Maximum allowed file size in bytes
    pub max_file_size: Option<u64>,
}

/// Output from a successful download operation.
#[derive(Debug, Clone)]
pub struct DownloadOutput {
    /// Actual file path of the downloaded file (may differ from requested path)
    pub file_path: String,
    /// Duration in seconds (if media file)
    pub duration_secs: Option<u32>,
    /// File size in bytes
    pub file_size: u64,
    /// MIME type hint (e.g., "audio/mpeg", "video/mp4")
    pub mime_hint: Option<String>,
}

/// Trait for download source implementations.
///
/// Each source handles a specific type of URL (YouTube via yt-dlp, direct HTTP, etc.)
/// and provides metadata extraction, size estimation, and the actual download.
#[async_trait]
pub trait DownloadSource: Send + Sync {
    /// Human-readable name of this source (e.g., "yt-dlp", "http")
    fn name(&self) -> &str;

    /// Whether this source can handle the given URL.
    fn supports_url(&self, url: &Url) -> bool;

    /// Fetch metadata (title, artist) for the URL.
    async fn get_metadata(&self, url: &Url) -> Result<(String, String), AppError>;

    /// Estimate the file size in bytes before downloading.
    /// Returns None if estimation is not possible.
    async fn estimate_size(&self, url: &Url) -> Option<u64>;

    /// Check if the URL points to a livestream (not downloadable).
    async fn is_livestream(&self, url: &Url) -> bool;

    /// Execute the download, sending progress updates through the channel.
    async fn download(
        &self,
        request: &DownloadRequest,
        progress_tx: mpsc::UnboundedSender<SourceProgress>,
    ) -> Result<DownloadOutput, AppError>;
}

/// Registry that routes URLs to the appropriate download source.
///
/// Sources are tried in order; the first source that claims to support
/// the URL is used. This allows priority ordering (yt-dlp before HTTP).
pub struct SourceRegistry {
    sources: Vec<Arc<dyn DownloadSource>>,
}

impl SourceRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self { sources: Vec::new() }
    }

    /// Register a download source. Sources are tried in insertion order.
    pub fn register(&mut self, source: Arc<dyn DownloadSource>) {
        self.sources.push(source);
    }

    /// Find the first source that supports the given URL.
    pub fn resolve(&self, url: &Url) -> Option<Arc<dyn DownloadSource>> {
        self.sources.iter().find(|s| s.supports_url(url)).cloned()
    }

    /// Create the default registry with built-in sources.
    /// Add new sources by implementing `DownloadSource` and calling `register()`.
    pub fn default_registry() -> Self {
        let mut registry = Self::new();
        registry.register(Arc::new(ytdlp::YtDlpSource::new()));
        registry.register(Arc::new(http::HttpSource::new()));
        registry
    }
}

impl Default for SourceRegistry {
    fn default() -> Self {
        Self::default_registry()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_source_progress_to_progress_info() {
        let sp = SourceProgress {
            percent: 50,
            speed_bytes_sec: Some(1_048_576.0), // 1 MiB/s
            eta_seconds: Some(30),
            downloaded_bytes: Some(5_000_000),
            total_bytes: Some(10_000_000),
        };
        let pi = sp.to_progress_info();
        assert_eq!(pi.percent, 50);
        assert!((pi.speed_mbs.unwrap() - 1.0).abs() < 0.01);
        assert_eq!(pi.eta_seconds, Some(30));
        assert_eq!(pi.current_size, Some(5_000_000));
        assert_eq!(pi.total_size, Some(10_000_000));
    }

    #[test]
    fn test_registry_resolve_order() {
        let registry = SourceRegistry::default_registry();

        // YouTube URL should resolve to YtDlpSource
        let yt_url = Url::parse("https://www.youtube.com/watch?v=test123").unwrap();
        let source = registry.resolve(&yt_url);
        assert!(source.is_some());
        assert_eq!(source.unwrap().name(), "yt-dlp");

        // Direct MP3 URL should resolve to HttpSource
        let http_url = Url::parse("https://example.com/file.mp3").unwrap();
        let source = registry.resolve(&http_url);
        assert!(source.is_some());
        assert_eq!(source.unwrap().name(), "http");
    }
}
