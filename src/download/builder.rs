//! Builder pattern for download configuration.
//!
//! Provides a fluent API for constructing `DownloadRequest` instances
//! with sensible defaults.

use crate::core::config;
use crate::core::utils::escape_filename;
use crate::download::downloader::generate_file_name_with_ext;
use crate::download::source::DownloadRequest;
use url::Url;

/// Builder for constructing download requests.
///
/// # Example
///
/// ```ignore
/// let request = DownloadConfigBuilder::new(url)
///     .format("mp3")
///     .audio_bitrate("320k")
///     .max_file_size(49 * 1024 * 1024)
///     .build(&title, &artist);
/// ```
pub struct DownloadConfigBuilder {
    url: Url,
    format: String,
    audio_bitrate: Option<String>,
    video_quality: Option<String>,
    max_file_size: Option<u64>,
    custom_output_path: Option<String>,
    time_range: Option<(String, String)>,
}

impl DownloadConfigBuilder {
    /// Create a new builder for the given URL.
    pub fn new(url: Url) -> Self {
        Self {
            url,
            format: "mp3".to_string(),
            audio_bitrate: None,
            video_quality: None,
            max_file_size: None,
            custom_output_path: None,
            time_range: None,
        }
    }

    /// Set the target format (e.g., "mp3", "mp4").
    pub fn format(mut self, format: &str) -> Self {
        self.format = format.to_string();
        self
    }

    /// Set the audio bitrate (e.g., "128k", "320k").
    pub fn audio_bitrate(mut self, bitrate: &str) -> Self {
        self.audio_bitrate = Some(bitrate.to_string());
        self
    }

    /// Set the video quality (e.g., "720p", "1080p").
    pub fn video_quality(mut self, quality: &str) -> Self {
        self.video_quality = Some(quality.to_string());
        self
    }

    /// Set maximum allowed file size in bytes.
    pub fn max_file_size(mut self, size: u64) -> Self {
        self.max_file_size = Some(size);
        self
    }

    /// Override the output path instead of auto-generating from title/artist.
    pub fn output_path(mut self, path: &str) -> Self {
        self.custom_output_path = Some(path.to_string());
        self
    }

    /// Set a time range to download only a segment (e.g., "00:01:00", "00:02:30").
    pub fn time_range(mut self, start: &str, end: &str) -> Self {
        self.time_range = Some((start.to_string(), end.to_string()));
        self
    }

    /// Build the `DownloadRequest`, generating the output path from title and artist.
    ///
    /// Adds a timestamp to the filename to prevent race conditions with concurrent downloads.
    pub fn build(self, title: &str, artist: &str) -> DownloadRequest {
        let output_path = if let Some(path) = self.custom_output_path {
            path
        } else {
            Self::generate_output_path(title, artist, &self.format)
        };

        DownloadRequest {
            url: self.url,
            output_path,
            format: self.format,
            audio_bitrate: self.audio_bitrate,
            video_quality: self.video_quality,
            max_file_size: self.max_file_size,
            time_range: self.time_range,
        }
    }

    /// Generate a unique output path with timestamp to avoid filename collisions.
    fn generate_output_path(title: &str, artist: &str, format: &str) -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);

        let ext = format;

        let base_file_name = generate_file_name_with_ext(title, artist, ext);

        // Add timestamp before extension to ensure uniqueness
        let file_name = if let Some(dot_pos) = base_file_name.rfind('.') {
            format!(
                "{}_{}.{}",
                &base_file_name[..dot_pos],
                timestamp,
                &base_file_name[dot_pos + 1..]
            )
        } else {
            format!("{}_{}", base_file_name, timestamp)
        };

        let safe_filename = escape_filename(&file_name);
        let full_path = format!("{}/{}", &*config::DOWNLOAD_FOLDER, safe_filename);
        shellexpand::tilde(&full_path).into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builder_default_format() {
        let url = Url::parse("https://youtube.com/watch?v=test").unwrap();
        let request = DownloadConfigBuilder::new(url.clone()).build("Song", "Artist");
        assert_eq!(request.format, "mp3");
        assert_eq!(request.url, url);
        assert!(request.audio_bitrate.is_none());
        assert!(request.video_quality.is_none());
    }

    #[test]
    fn test_builder_audio_config() {
        let url = Url::parse("https://youtube.com/watch?v=test").unwrap();
        let request = DownloadConfigBuilder::new(url)
            .format("mp3")
            .audio_bitrate("320k")
            .max_file_size(49 * 1024 * 1024)
            .build("Song Title", "Artist Name");

        assert_eq!(request.format, "mp3");
        assert_eq!(request.audio_bitrate.as_deref(), Some("320k"));
        assert_eq!(request.max_file_size, Some(49 * 1024 * 1024));
        assert!(request.output_path.contains("Artist_Name"));
    }

    #[test]
    fn test_builder_video_config() {
        let url = Url::parse("https://youtube.com/watch?v=test").unwrap();
        let request = DownloadConfigBuilder::new(url)
            .format("mp4")
            .video_quality("720p")
            .build("Video Title", "Channel");

        assert_eq!(request.format, "mp4");
        assert_eq!(request.video_quality.as_deref(), Some("720p"));
    }

    #[test]
    fn test_builder_custom_output_path() {
        let url = Url::parse("https://example.com/file.mp3").unwrap();
        let request = DownloadConfigBuilder::new(url)
            .output_path("/tmp/custom.mp3")
            .build("Title", "Artist");

        assert_eq!(request.output_path, "/tmp/custom.mp3");
    }

    #[test]
    fn test_builder_time_range() {
        let url = Url::parse("https://youtube.com/watch?v=test").unwrap();
        let request = DownloadConfigBuilder::new(url)
            .format("mp4")
            .time_range("00:01:00", "00:02:30")
            .build("Video", "Channel");

        assert_eq!(
            request.time_range,
            Some(("00:01:00".to_string(), "00:02:30".to_string()))
        );
    }

    #[test]
    fn test_builder_no_time_range_by_default() {
        let url = Url::parse("https://youtube.com/watch?v=test").unwrap();
        let request = DownloadConfigBuilder::new(url).build("Song", "Artist");
        assert!(request.time_range.is_none());
    }
}
