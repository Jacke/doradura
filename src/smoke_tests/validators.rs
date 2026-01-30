//! File validation utilities for smoke tests.
//!
//! Validates downloaded audio and video files using ffprobe.

use std::fs;
use std::path::Path;
use std::process::Command;

/// Result of audio file validation
#[derive(Debug, Clone)]
pub struct AudioFileValidation {
    /// File size in bytes
    pub size: u64,
    /// Duration in seconds (from ffprobe)
    pub duration: Option<u32>,
    /// Whether the file is valid
    pub is_valid: bool,
    /// Error message if invalid
    pub error: Option<String>,
}

/// Result of video file validation
#[derive(Debug, Clone)]
pub struct VideoFileValidation {
    /// File size in bytes
    pub size: u64,
    /// Duration in seconds
    pub duration: Option<u32>,
    /// Video width in pixels
    pub width: Option<u32>,
    /// Video height in pixels
    pub height: Option<u32>,
    /// Whether the file has a video stream
    pub has_video_stream: bool,
    /// Whether the file has an audio stream
    pub has_audio_stream: bool,
    /// Whether the file is valid
    pub is_valid: bool,
    /// Error message if invalid
    pub error: Option<String>,
}

/// Probe duration of a media file using ffprobe
fn probe_duration(path: &str) -> Option<u32> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
            path,
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<f64>()
        .ok()
        .map(|d| d as u32)
}

/// Probe video dimensions using ffprobe
fn probe_dimensions(path: &str) -> Option<(u32, u32)> {
    let width_output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
            path,
        ])
        .output()
        .ok()?;

    let height_output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=height",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
            path,
        ])
        .output()
        .ok()?;

    if !width_output.status.success() || !height_output.status.success() {
        return None;
    }

    let width = String::from_utf8_lossy(&width_output.stdout)
        .trim()
        .parse::<u32>()
        .ok()?;

    let height = String::from_utf8_lossy(&height_output.stdout)
        .trim()
        .parse::<u32>()
        .ok()?;

    Some((width, height))
}

/// Check if file has a video stream
fn has_video_stream(path: &str) -> bool {
    Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=codec_type",
            path,
        ])
        .output()
        .map(|o| o.status.success() && !o.stdout.is_empty())
        .unwrap_or(false)
}

/// Check if file has an audio stream
fn has_audio_stream(path: &str) -> bool {
    Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "a:0",
            "-show_entries",
            "stream=codec_type",
            path,
        ])
        .output()
        .map(|o| o.status.success() && !o.stdout.is_empty())
        .unwrap_or(false)
}

/// Validates an MP3 audio file
pub fn validate_audio_file(path: &Path) -> AudioFileValidation {
    let mut validation = AudioFileValidation {
        size: 0,
        duration: None,
        is_valid: false,
        error: None,
    };

    // Check file exists
    if !path.exists() {
        validation.error = Some("File does not exist".to_string());
        return validation;
    }

    // Get file size
    match fs::metadata(path) {
        Ok(meta) => {
            validation.size = meta.len();
            if validation.size == 0 {
                validation.error = Some("File is empty (0 bytes)".to_string());
                return validation;
            }
            // MP3 files should be at least 10KB for a short clip
            if validation.size < 10_000 {
                validation.error = Some(format!("File too small ({} bytes), likely corrupted", validation.size));
                return validation;
            }
        }
        Err(e) => {
            validation.error = Some(format!("Failed to read file metadata: {}", e));
            return validation;
        }
    }

    // Get duration via ffprobe
    let path_str = path.to_str().unwrap_or_default();
    validation.duration = probe_duration(path_str);

    if validation.duration.is_none() {
        validation.error = Some("Failed to probe audio duration (file may be corrupted)".to_string());
        return validation;
    }

    // Verify duration is reasonable (at least 1 second)
    if let Some(duration) = validation.duration {
        if duration == 0 {
            validation.error = Some("Audio duration is 0 seconds".to_string());
            return validation;
        }
    }

    validation.is_valid = true;
    validation
}

/// Validates an MP4 video file
pub fn validate_video_file(path: &Path) -> VideoFileValidation {
    let mut validation = VideoFileValidation {
        size: 0,
        duration: None,
        width: None,
        height: None,
        has_video_stream: false,
        has_audio_stream: false,
        is_valid: false,
        error: None,
    };

    // Check file exists
    if !path.exists() {
        validation.error = Some("File does not exist".to_string());
        return validation;
    }

    // Get file size
    match fs::metadata(path) {
        Ok(meta) => {
            validation.size = meta.len();
            if validation.size == 0 {
                validation.error = Some("File is empty (0 bytes)".to_string());
                return validation;
            }
        }
        Err(e) => {
            validation.error = Some(format!("Failed to read file metadata: {}", e));
            return validation;
        }
    }

    let path_str = path.to_str().unwrap_or_default();

    // Get duration
    validation.duration = probe_duration(path_str);
    if validation.duration.is_none() {
        validation.error = Some("Failed to probe video duration".to_string());
        return validation;
    }

    // Get dimensions
    if let Some((width, height)) = probe_dimensions(path_str) {
        validation.width = Some(width);
        validation.height = Some(height);
    }

    // Check for video stream
    validation.has_video_stream = has_video_stream(path_str);
    if !validation.has_video_stream {
        validation.error = Some("Video file has no video stream".to_string());
        return validation;
    }

    // Check for audio stream
    validation.has_audio_stream = has_audio_stream(path_str);
    if !validation.has_audio_stream {
        validation.error = Some("Video file has no audio stream (will show black screen in Telegram)".to_string());
        return validation;
    }

    validation.is_valid = true;
    validation
}

/// Checks if ffmpeg is available
pub fn is_ffmpeg_available() -> bool {
    Command::new("ffmpeg")
        .arg("-version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Checks if ffprobe is available
pub fn is_ffprobe_available() -> bool {
    Command::new("ffprobe")
        .arg("-version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Checks if yt-dlp is available
pub fn is_ytdlp_available() -> bool {
    // Try yt-dlp first, then youtube-dl
    Command::new("yt-dlp")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or_else(|_| {
            Command::new("youtube-dl")
                .arg("--version")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        })
}
