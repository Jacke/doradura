# Extending Download Sources

Doradura uses a **trait-based plugin architecture** for download backends. Adding a new source (e.g., Spotify API, cloud storage, torrent) requires implementing one trait and registering it — no changes to the pipeline, UI, or Telegram handlers.

## Architecture Overview

```
                    User sends URL
                         │
                         ▼
              ┌─── SourceRegistry ───┐
              │  resolve(url) → src  │   Tries sources in order,
              │                      │   first match wins
              └──────────┬───────────┘
                         │
           ┌─────────────┼─────────────┐
           ▼             ▼             ▼
      YtDlpSource   HttpSource   YourSource
      (1000+ sites)  (direct URLs)  (custom)
           │             │             │
           └─────────────┼─────────────┘
                         ▼
              Pipeline (execute / download_phase)
                         │
                    ┌────┴────┐
                    │ audio   │ video
                    │ .rs     │ .rs
                    └─────────┘
```

Two independent systems work together:

| System | Purpose | File |
|--------|---------|------|
| **`DownloadSource`** trait | URL routing + download logic | `src/download/source/mod.rs` |
| **`BotExtension`** trait | UI metadata (icon, name, capabilities) | `src/extension/mod.rs` |

You implement both for a complete integration, but they're decoupled — a source works without an extension (just no UI listing).

## Step 1: Implement `DownloadSource`

The core trait lives in [`src/download/source/mod.rs`](../src/download/source/mod.rs):

```rust
#[async_trait]
pub trait DownloadSource: Send + Sync {
    /// Human-readable name (e.g., "spotify", "s3")
    fn name(&self) -> &str;

    /// Whether this source can handle the given URL.
    /// Called for every URL — keep it fast (no network calls).
    fn supports_url(&self, url: &Url) -> bool;

    /// Fetch metadata (title, artist) for the URL.
    async fn get_metadata(&self, url: &Url) -> Result<(String, String), AppError>;

    /// Estimate file size in bytes (HEAD request, API call, etc.)
    /// Return None if unknown.
    async fn estimate_size(&self, url: &Url) -> Option<u64>;

    /// Check if URL is a livestream (not downloadable).
    async fn is_livestream(&self, url: &Url) -> bool;

    /// Download the file, reporting progress via the channel.
    async fn download(
        &self,
        request: &DownloadRequest,
        progress_tx: mpsc::UnboundedSender<SourceProgress>,
    ) -> Result<DownloadOutput, AppError>;
}
```

### Example: S3 Download Source

```rust
// src/download/source/s3.rs

use crate::core::error::AppError;
use crate::download::source::*;
use async_trait::async_trait;
use tokio::sync::mpsc;
use url::Url;

pub struct S3Source {
    client: aws_sdk_s3::Client,
}

impl S3Source {
    pub async fn new() -> Self {
        let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        Self {
            client: aws_sdk_s3::Client::new(&config),
        }
    }
}

#[async_trait]
impl DownloadSource for S3Source {
    fn name(&self) -> &str {
        "s3"
    }

    fn supports_url(&self, url: &Url) -> bool {
        // Match s3:// URLs or *.s3.amazonaws.com
        url.scheme() == "s3"
            || url.host_str()
                .map(|h| h.ends_with(".s3.amazonaws.com"))
                .unwrap_or(false)
    }

    async fn get_metadata(&self, url: &Url) -> Result<(String, String), AppError> {
        // Extract bucket/key from URL, use HeadObject for metadata
        let key = url.path().trim_start_matches('/');
        let title = key.rsplit('/').next().unwrap_or("download").to_string();
        Ok((title, String::new()))
    }

    async fn estimate_size(&self, url: &Url) -> Option<u64> {
        // Use HeadObject to get ContentLength
        None // simplified
    }

    async fn is_livestream(&self, _url: &Url) -> bool {
        false
    }

    async fn download(
        &self,
        request: &DownloadRequest,
        progress_tx: mpsc::UnboundedSender<SourceProgress>,
    ) -> Result<DownloadOutput, AppError> {
        // ... download from S3, write to request.output_path,
        // send SourceProgress updates via progress_tx ...

        Ok(DownloadOutput {
            file_path: request.output_path.clone(),
            duration_secs: None,
            file_size: 0, // actual size after download
            mime_hint: Some("audio/mpeg".to_string()),
        })
    }
}
```

### Key types your download receives and returns

```rust
/// What the pipeline gives you
pub struct DownloadRequest {
    pub url: Url,
    pub output_path: String,        // Where to save the file
    pub format: String,             // "mp3", "mp4", etc.
    pub audio_bitrate: Option<String>,
    pub video_quality: Option<String>,
    pub max_file_size: Option<u64>,
    pub time_range: Option<(String, String)>,
}

/// What you return on success
pub struct DownloadOutput {
    pub file_path: String,          // Actual path (may differ from requested)
    pub duration_secs: Option<u32>,
    pub file_size: u64,
    pub mime_hint: Option<String>,
}

/// Progress updates during download
pub struct SourceProgress {
    pub percent: u8,                // 0-100
    pub speed_bytes_sec: Option<f64>,
    pub eta_seconds: Option<u64>,
    pub downloaded_bytes: Option<u64>,
    pub total_bytes: Option<u64>,
}
```

## Step 2: Register in `SourceRegistry`

Edit [`src/download/source/mod.rs`](../src/download/source/mod.rs):

```rust
// In SourceRegistry::default_registry()
pub fn default_registry() -> Self {
    let mut registry = Self::new();
    registry.register(Arc::new(ytdlp::YtDlpSource::new()));
    registry.register(Arc::new(s3::S3Source::new()));  // ← add yours
    registry.register(Arc::new(http::HttpSource::new())); // HTTP last (fallback)
    registry
}
```

**Order matters.** Sources are tried in insertion order. Put specific sources before generic ones — `HttpSource` should be last since it matches any URL with a file extension.

## Step 3 (optional): Add a `BotExtension` for UI

This makes your source visible in the `/services` menu with icon, localized name, and capabilities.

```rust
// src/extension/s3_downloader.rs

use super::{BotExtension, Capability, ExtensionCategory};

pub struct S3Extension;

impl BotExtension for S3Extension {
    fn id(&self) -> &str { "s3" }
    fn locale_key(&self) -> &str { "ext_s3" }
    fn icon(&self) -> &str { "☁️" }

    fn capabilities(&self) -> Vec<Capability> {
        vec![
            Capability {
                name: "S3 Buckets".into(),
                description: "Download from AWS S3".into(),
            },
        ]
    }

    fn is_available(&self) -> bool { true }
    fn category(&self) -> ExtensionCategory { ExtensionCategory::Downloader }
}
```

Register in [`src/extension/mod.rs`](../src/extension/mod.rs):

```rust
pub fn default_registry() -> Self {
    let extensions: Vec<Box<dyn BotExtension>> = vec![
        Box::new(ytdlp_downloader::YtDlpExtension),
        Box::new(http_downloader::HttpExtension),
        Box::new(s3_downloader::S3Extension),    // ← add yours
        Box::new(converter::ConverterExtension),
        Box::new(audio_effects::AudioEffectsExtension),
    ];
    Self { extensions }
}
```

Add locale keys in `locales/{en,ru,fr,de}/main.ftl`:

```ftl
ext_s3-name = S3 Downloads
ext_s3-description = Download files from AWS S3 buckets
```

## Built-in Sources Reference

### YtDlpSource (`src/download/source/ytdlp.rs`)

- **Matches:** YouTube, SoundCloud, TikTok, Instagram, VK, Twitch, and 1000+ more sites
- **Features:** 3-tier fallback chain (no cookies → cookies + PO token → fixup never), proxy chain failover, progress parsing from yt-dlp stderr
- **Priority:** First (most URLs go through yt-dlp)

### HttpSource (`src/download/source/http.rs`)

- **Matches:** Any `http://` / `https://` URL ending in a media file extension (`.mp3`, `.mp4`, `.wav`, `.flac`, etc.)
- **Features:** Chunked download with resume via HTTP Range headers, Content-Disposition filename parsing, progress tracking
- **Priority:** Last (fallback for direct file links)

## Testing Your Source

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_supports_url() {
        let source = S3Source::new();
        assert!(source.supports_url(&Url::parse("s3://bucket/key.mp3").unwrap()));
        assert!(!source.supports_url(&Url::parse("https://youtube.com/watch?v=x").unwrap()));
    }

    #[tokio::test]
    async fn test_get_metadata() {
        let source = S3Source::new();
        let (title, _) = source.get_metadata(&Url::parse("s3://bucket/song.mp3").unwrap()).await.unwrap();
        assert_eq!(title, "song.mp3");
    }
}
```

The pipeline handles everything else — progress UI, Telegram sending, error messages, cleanup, history tracking. Your source just downloads the file.
