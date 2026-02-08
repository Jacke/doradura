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

/// Generate a temporary output path with given extension
pub fn temp_output_path(prefix: &str, extension: &str) -> std::path::PathBuf {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let rand: u32 = rand::random();
    std::path::PathBuf::from(format!("/tmp/{}_{:x}_{:x}.{}", prefix, timestamp, rand, extension))
}
