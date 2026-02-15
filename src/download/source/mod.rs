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
pub mod instagram;
pub mod ytdlp;

use crate::core::error::AppError;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::mpsc;
use url::Url;

/// Metadata for a media URL (title and artist).
#[derive(Debug, Clone)]
pub struct MediaMetadata {
    pub title: String,
    pub artist: String,
}

/// Progress information emitted during download.
#[derive(Debug, Clone, Default)]
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
    /// Time range to download (start, end) e.g. ("00:01:00", "00:02:30").
    /// Uses yt-dlp --download-sections under the hood.
    pub time_range: Option<(String, String)>,
}

/// An additional media file from a multi-item post (e.g., Instagram carousel).
#[derive(Debug, Clone)]
pub struct AdditionalFile {
    /// Local path to the downloaded file
    pub file_path: String,
    /// MIME type (e.g., "image/jpeg", "video/mp4")
    pub mime_type: String,
    /// Duration in seconds (for video items)
    pub duration_secs: Option<u32>,
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
    /// Additional files from multi-item posts (e.g., Instagram carousel).
    /// None for single-item downloads.
    pub additional_files: Option<Vec<AdditionalFile>>,
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
    async fn get_metadata(&self, url: &Url) -> Result<MediaMetadata, AppError>;

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
        registry.register(Arc::new(instagram::InstagramSource::new()));
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

static DEFAULT_REGISTRY: std::sync::LazyLock<SourceRegistry> =
    std::sync::LazyLock::new(SourceRegistry::default_registry);

impl SourceRegistry {
    /// Get a reference to the shared default registry singleton.
    ///
    /// Both `YtDlpSource` and `HttpSource` are stateless, so a single
    /// shared instance avoids re-allocating on every download.
    pub fn global() -> &'static SourceRegistry {
        &DEFAULT_REGISTRY
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A configurable mock download source for unit tests.
    pub struct MockSource {
        pub name: String,
        pub urls: Vec<String>,
        pub metadata: MediaMetadata,
        pub fail_download: bool,
    }

    #[async_trait]
    impl DownloadSource for MockSource {
        fn name(&self) -> &str {
            &self.name
        }

        fn supports_url(&self, url: &Url) -> bool {
            self.urls.iter().any(|u| url.as_str().contains(u))
        }

        async fn get_metadata(&self, _url: &Url) -> Result<MediaMetadata, AppError> {
            Ok(self.metadata.clone())
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
            tx: mpsc::UnboundedSender<SourceProgress>,
        ) -> Result<DownloadOutput, AppError> {
            for p in [25, 50, 75, 100] {
                let _ = tx.send(SourceProgress {
                    percent: p,
                    ..Default::default()
                });
            }
            if self.fail_download {
                return Err(AppError::Download("mock failure".into()));
            }
            std::fs::write(&request.output_path, b"mock audio data")
                .map_err(|e| AppError::Download(crate::download::error::DownloadError::Other(e.to_string())))?;
            Ok(DownloadOutput {
                file_path: request.output_path.clone(),
                duration_secs: Some(180),
                file_size: 15,
                mime_hint: None,
                additional_files: None,
            })
        }
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

    #[test]
    fn test_registry_with_mock_source() {
        let mut registry = SourceRegistry::new();
        registry.register(Arc::new(MockSource {
            name: "mock".to_string(),
            urls: vec!["mock.example.com".to_string()],
            metadata: MediaMetadata {
                title: "Test".to_string(),
                artist: "Artist".to_string(),
            },
            fail_download: false,
        }));

        let url = Url::parse("https://mock.example.com/track/1").unwrap();
        let source = registry.resolve(&url);
        assert!(source.is_some());
        assert_eq!(source.unwrap().name(), "mock");

        // Unregistered URL returns None
        let unknown = Url::parse("https://unknown.example.com/track/1").unwrap();
        assert!(registry.resolve(&unknown).is_none());
    }

    #[tokio::test]
    async fn test_mock_source_metadata() {
        let source = MockSource {
            name: "mock".to_string(),
            urls: vec!["example.com".to_string()],
            metadata: MediaMetadata {
                title: "Song Title".to_string(),
                artist: "Artist Name".to_string(),
            },
            fail_download: false,
        };
        let url = Url::parse("https://example.com/track").unwrap();
        let meta = source.get_metadata(&url).await.unwrap();
        assert_eq!(meta.title, "Song Title");
        assert_eq!(meta.artist, "Artist Name");
    }

    #[tokio::test]
    async fn test_mock_source_download_progress() {
        let source = MockSource {
            name: "mock".to_string(),
            urls: vec!["example.com".to_string()],
            metadata: MediaMetadata {
                title: "T".to_string(),
                artist: "A".to_string(),
            },
            fail_download: false,
        };

        let tmp = std::env::temp_dir().join(format!("mock_dl_{}", std::process::id()));
        let request = DownloadRequest {
            url: Url::parse("https://example.com/track").unwrap(),
            output_path: tmp.to_string_lossy().to_string(),
            format: "mp3".to_string(),
            audio_bitrate: None,
            video_quality: None,
            max_file_size: None,
            time_range: None,
        };

        let (tx, mut rx) = mpsc::unbounded_channel();
        let result = source.download(&request, tx).await;
        assert!(result.is_ok());

        // Collect progress updates
        let mut percents = Vec::new();
        while let Ok(sp) = rx.try_recv() {
            percents.push(sp.percent);
        }
        assert_eq!(percents, vec![25, 50, 75, 100]);

        // Clean up
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn test_registry_priority_ordering() {
        // Mock that claims YouTube URLs should override the built-in yt-dlp source
        let mut registry = SourceRegistry::new();
        registry.register(Arc::new(MockSource {
            name: "custom-yt".to_string(),
            urls: vec!["youtube.com".to_string()],
            metadata: MediaMetadata {
                title: "T".to_string(),
                artist: "A".to_string(),
            },
            fail_download: false,
        }));
        registry.register(Arc::new(ytdlp::YtDlpSource::new()));
        registry.register(Arc::new(http::HttpSource::new()));

        let yt_url = Url::parse("https://www.youtube.com/watch?v=abc").unwrap();
        let source = registry.resolve(&yt_url).unwrap();
        // First registered source (custom-yt) should win over yt-dlp
        assert_eq!(source.name(), "custom-yt");
    }

    #[test]
    fn test_source_progress_default() {
        let sp = SourceProgress::default();
        assert_eq!(sp.percent, 0);
        assert!(sp.speed_bytes_sec.is_none());
        assert!(sp.eta_seconds.is_none());
        assert!(sp.downloaded_bytes.is_none());
        assert!(sp.total_bytes.is_none());
    }
}
