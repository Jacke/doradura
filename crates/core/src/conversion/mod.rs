//! Media conversion engine — a core feature of Doradura.
//!
//! Converts between media formats without requiring a URL download.
//! Users send a file directly and receive the converted result.
//!
//! Supported conversions:
//! - Video: to video note (circle), audio extraction, GIF, compression
//! - Image: resize, format conversion (PNG, JPEG, WebP, etc.)
//! - Document: DOCX/ODT to PDF via LibreOffice
//! - Audio: effects (pitch, tempo, bass boost), ringtone creation

pub mod audio;
pub mod document;
pub mod image;
pub mod video;

use std::path::Path;
use thiserror::Error;

/// Errors that can occur during conversion
#[derive(Error, Debug)]
pub enum ConversionError {
    #[error("FFmpeg error: {0}")]
    FfmpegError(String),

    #[error("Input file not found: {0}")]
    InputNotFound(String),

    #[error("Output creation failed: {0}")]
    OutputFailed(String),

    #[error("Unsupported format: {0}")]
    UnsupportedFormat(String),

    #[error("LibreOffice error: {0}")]
    LibreOfficeError(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Duration exceeds limit: {actual}s > {limit}s")]
    DurationExceeded { actual: u64, limit: u64 },

    #[error("File size exceeds limit: {actual} > {limit}")]
    SizeExceeded { actual: u64, limit: u64 },

    #[error("Invalid input: {0}")]
    InvalidInput(String),
}

pub type ConversionResult<T> = Result<T, ConversionError>;

/// Check if ffmpeg is available
pub async fn check_ffmpeg() -> bool {
    tokio::process::Command::new("ffmpeg")
        .arg("-version")
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Check if libreoffice is available
pub async fn check_libreoffice() -> bool {
    tokio::process::Command::new("libreoffice")
        .arg("--version")
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Get video duration using ffprobe
pub async fn get_video_duration<P: AsRef<Path>>(path: P) -> ConversionResult<f64> {
    let output = tokio::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(path.as_ref())
        .output()
        .await?;

    if !output.status.success() {
        return Err(ConversionError::FfmpegError(
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
    }

    let duration_str = String::from_utf8_lossy(&output.stdout);
    duration_str
        .trim()
        .parse::<f64>()
        .map_err(|_| ConversionError::FfmpegError("Failed to parse duration".to_string()))
}

/// Get file size in bytes
pub async fn get_file_size<P: AsRef<Path>>(path: P) -> ConversionResult<u64> {
    let metadata = tokio::fs::metadata(path).await?;
    Ok(metadata.len())
}

/// Generate a temporary output path with given extension.
/// Uses `std::env::temp_dir()` for portability and `u64` random for higher entropy.
pub fn temp_output_path(prefix: &str, extension: &str) -> std::path::PathBuf {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let rand: u64 = rand::random();
    std::env::temp_dir().join(format!("{}_{:x}_{:016x}.{}", prefix, timestamp, rand, extension))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_temp_output_path_has_correct_extension() {
        let path = temp_output_path("test", "mp3");
        assert_eq!(path.extension().and_then(|e| e.to_str()), Some("mp3"));
    }

    #[test]
    fn test_temp_output_path_has_prefix() {
        let path = temp_output_path("myprefix", "wav");
        let name = path.file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with("myprefix_"), "Path should start with prefix: {}", name);
    }

    #[test]
    fn test_temp_output_path_uniqueness() {
        let path1 = temp_output_path("test", "mp3");
        let path2 = temp_output_path("test", "mp3");
        assert_ne!(path1, path2, "Two calls should produce unique paths");
    }

    #[test]
    fn test_temp_output_path_in_tmp_dir() {
        let path = temp_output_path("test", "flac");
        assert!(
            path.starts_with(std::env::temp_dir()),
            "Path should be in temp dir: {:?}",
            path
        );
    }

    #[tokio::test]
    async fn test_check_ffmpeg() {
        // ffmpeg should be available on dev machines
        let available = check_ffmpeg().await;
        // Don't assert true since CI may not have it, just check it runs
        let _ = available;
    }

    #[tokio::test]
    async fn test_get_file_size() {
        let path = "/tmp/test_filesize_check.txt";
        tokio::fs::write(path, "hello world").await.unwrap();

        let size = get_file_size(path).await.unwrap();
        assert_eq!(size, 11); // "hello world" = 11 bytes

        let _ = tokio::fs::remove_file(path).await;
    }

    #[tokio::test]
    async fn test_get_file_size_not_found() {
        let result = get_file_size("/tmp/nonexistent_filesize_12345.txt").await;
        assert!(result.is_err());
    }

    #[test]
    fn test_conversion_error_display() {
        let err = ConversionError::InputNotFound("/tmp/test.mp3".to_string());
        assert!(err.to_string().contains("/tmp/test.mp3"));

        let err = ConversionError::FfmpegError("codec not found".to_string());
        assert!(err.to_string().contains("codec not found"));

        let err = ConversionError::UnsupportedFormat("xyz".to_string());
        assert!(err.to_string().contains("xyz"));

        let err = ConversionError::DurationExceeded {
            actual: 600,
            limit: 300,
        };
        assert!(err.to_string().contains("600"));
        assert!(err.to_string().contains("300"));

        let err = ConversionError::SizeExceeded { actual: 100, limit: 50 };
        assert!(err.to_string().contains("100"));
        assert!(err.to_string().contains("50"));
    }
}
