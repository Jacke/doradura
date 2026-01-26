//! Video conversion utilities
//!
//! Provides conversions:
//! - Video to Video Note (circle) - 640x640, max 60 seconds
//! - Video to Audio (MP3)
//! - Video to GIF
//! - Video compression

use super::{temp_output_path, ConversionError, ConversionResult};
use std::path::Path;
use tokio::process::Command;

/// Maximum duration for video notes in seconds
pub const VIDEO_NOTE_MAX_DURATION: u64 = 60;

/// Video note dimensions (square)
pub const VIDEO_NOTE_SIZE: u32 = 640;

/// Options for video note conversion
#[derive(Debug, Clone, Default)]
pub struct VideoNoteOptions {
    /// Duration to cut (in seconds), None for full video up to 60s
    pub duration: Option<u64>,
    /// Start time in seconds, None for start from beginning
    pub start_time: Option<f64>,
    /// Speed multiplier (e.g., 1.5 for 1.5x speed)
    pub speed: Option<f64>,
}

/// Convert video to video note (circle format)
///
/// Creates a 640x640 square video suitable for Telegram video notes.
/// Maximum duration is 60 seconds.
///
/// # Arguments
/// * `input_path` - Path to input video file
/// * `options` - Conversion options (duration, start time, speed)
///
/// # Returns
/// Path to the converted video note file
pub async fn to_video_note<P: AsRef<Path>>(
    input_path: P,
    options: VideoNoteOptions,
) -> ConversionResult<std::path::PathBuf> {
    let input = input_path.as_ref();

    if !input.exists() {
        return Err(ConversionError::InputNotFound(input.display().to_string()));
    }

    let output_path = temp_output_path("video_note", "mp4");

    // Build the filter for video note: scale to 640x640, crop to square, yuv420p format
    let video_filter = "scale=640:640:force_original_aspect_ratio=increase,crop=640:640,format=yuv420p";

    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-hide_banner").arg("-loglevel").arg("error").arg("-y");

    // Add start time if specified
    if let Some(ss) = options.start_time {
        cmd.arg("-ss").arg(format!("{}", ss));
    }

    cmd.arg("-i").arg(input);

    // Add duration limit (max 60 seconds for video notes)
    let duration = options
        .duration
        .unwrap_or(VIDEO_NOTE_MAX_DURATION)
        .min(VIDEO_NOTE_MAX_DURATION);
    cmd.arg("-t").arg(format!("{}", duration));

    // Build filter complex based on speed option
    let filter_complex = if let Some(spd) = options.speed {
        let setpts_factor = 1.0 / spd;
        let atempo_filter = build_atempo_filter(spd);
        format!(
            "[0:v]{}setpts={}*PTS[vout];[0:a]{}[aout]",
            video_filter.to_owned() + ",",
            setpts_factor,
            atempo_filter
        )
    } else {
        format!("[0:v]{}[vout]", video_filter)
    };

    cmd.arg("-filter_complex").arg(&filter_complex);

    // Map video output
    cmd.arg("-map").arg("[vout]");

    // Map audio output
    if options.speed.is_some() {
        cmd.arg("-map").arg("[aout]");
    } else {
        cmd.arg("-map").arg("0:a?");
    }

    // Video codec settings optimized for video notes
    cmd.arg("-c:v")
        .arg("libx264")
        .arg("-preset")
        .arg("fast")
        .arg("-crf")
        .arg("28");

    // Audio codec settings
    cmd.arg("-c:a").arg("aac").arg("-b:a").arg("192k");

    // Output format
    cmd.arg("-movflags").arg("+faststart");
    cmd.arg(&output_path);

    let output = cmd.output().await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        log::error!("FFmpeg video note error: {}", stderr);
        return Err(ConversionError::FfmpegError(stderr.to_string()));
    }

    Ok(output_path)
}

/// Extract audio from video as MP3
///
/// # Arguments
/// * `input_path` - Path to input video file
/// * `bitrate` - Audio bitrate (e.g., "320k", "192k", "128k")
///
/// # Returns
/// Path to the extracted MP3 file
pub async fn extract_audio<P: AsRef<Path>>(input_path: P, bitrate: &str) -> ConversionResult<std::path::PathBuf> {
    let input = input_path.as_ref();

    if !input.exists() {
        return Err(ConversionError::InputNotFound(input.display().to_string()));
    }

    let output_path = temp_output_path("audio", "mp3");

    let output = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(input)
        .args(["-vn", "-acodec", "libmp3lame", "-b:a", bitrate])
        .arg(&output_path)
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        log::error!("FFmpeg audio extraction error: {}", stderr);
        return Err(ConversionError::FfmpegError(stderr.to_string()));
    }

    Ok(output_path)
}

/// Options for GIF conversion
#[derive(Debug, Clone)]
pub struct GifOptions {
    /// Duration in seconds (default: 10)
    pub duration: Option<u64>,
    /// Start time in seconds
    pub start_time: Option<f64>,
    /// Output width (height auto-calculated to maintain aspect ratio)
    pub width: Option<u32>,
    /// Frames per second (default: 15)
    pub fps: Option<u8>,
}

impl Default for GifOptions {
    fn default() -> Self {
        Self {
            duration: Some(10),
            start_time: None,
            width: Some(480),
            fps: Some(15),
        }
    }
}

/// Convert video to GIF using two-pass method for better quality
///
/// # Arguments
/// * `input_path` - Path to input video file
/// * `options` - GIF conversion options
///
/// # Returns
/// Path to the created GIF file
pub async fn to_gif<P: AsRef<Path>>(input_path: P, options: GifOptions) -> ConversionResult<std::path::PathBuf> {
    let input = input_path.as_ref();

    if !input.exists() {
        return Err(ConversionError::InputNotFound(input.display().to_string()));
    }

    let output_path = temp_output_path("gif", "gif");
    let palette_path = temp_output_path("palette", "png");

    let fps = options.fps.unwrap_or(15);
    let width = options.width.unwrap_or(480);
    let duration = options.duration.unwrap_or(10);

    // First pass: generate palette
    let palette_filter = format!("fps={},scale={}:-1:flags=lanczos,palettegen", fps, width);

    let mut cmd1 = Command::new("ffmpeg");
    cmd1.args(["-hide_banner", "-loglevel", "error", "-y"]);

    if let Some(ss) = options.start_time {
        cmd1.arg("-ss").arg(format!("{}", ss));
    }

    cmd1.arg("-i")
        .arg(input)
        .arg("-t")
        .arg(format!("{}", duration))
        .arg("-vf")
        .arg(&palette_filter)
        .arg(&palette_path);

    let output1 = cmd1.output().await?;

    if !output1.status.success() {
        let stderr = String::from_utf8_lossy(&output1.stderr);
        log::error!("FFmpeg palette generation error: {}", stderr);
        return Err(ConversionError::FfmpegError(stderr.to_string()));
    }

    // Second pass: create GIF using palette
    let gif_filter = format!("fps={},scale={}:-1:flags=lanczos[x];[x][1:v]paletteuse", fps, width);

    let mut cmd2 = Command::new("ffmpeg");
    cmd2.args(["-hide_banner", "-loglevel", "error", "-y"]);

    if let Some(ss) = options.start_time {
        cmd2.arg("-ss").arg(format!("{}", ss));
    }

    cmd2.arg("-i")
        .arg(input)
        .arg("-i")
        .arg(&palette_path)
        .arg("-t")
        .arg(format!("{}", duration))
        .arg("-lavfi")
        .arg(&gif_filter)
        .arg(&output_path);

    let output2 = cmd2.output().await?;

    // Clean up palette file
    let _ = tokio::fs::remove_file(&palette_path).await;

    if !output2.status.success() {
        let stderr = String::from_utf8_lossy(&output2.stderr);
        log::error!("FFmpeg GIF creation error: {}", stderr);
        return Err(ConversionError::FfmpegError(stderr.to_string()));
    }

    Ok(output_path)
}

/// Options for video compression
#[derive(Debug, Clone)]
pub struct CompressionOptions {
    /// Target CRF value (18-28, higher = more compression, lower quality)
    pub crf: Option<u8>,
    /// Audio bitrate (e.g., "128k")
    pub audio_bitrate: Option<String>,
    /// Video preset (ultrafast, superfast, veryfast, faster, fast, medium, slow, slower, veryslow)
    pub preset: Option<String>,
}

impl Default for CompressionOptions {
    fn default() -> Self {
        Self {
            crf: Some(28),
            audio_bitrate: Some("128k".to_string()),
            preset: Some("slow".to_string()),
        }
    }
}

/// Compress video to reduce file size
///
/// # Arguments
/// * `input_path` - Path to input video file
/// * `options` - Compression options
///
/// # Returns
/// Path to the compressed video file
pub async fn compress<P: AsRef<Path>>(
    input_path: P,
    options: CompressionOptions,
) -> ConversionResult<std::path::PathBuf> {
    let input = input_path.as_ref();

    if !input.exists() {
        return Err(ConversionError::InputNotFound(input.display().to_string()));
    }

    let output_path = temp_output_path("compressed", "mp4");

    let crf = options.crf.unwrap_or(28).to_string();
    let audio_bitrate = options.audio_bitrate.unwrap_or_else(|| "128k".to_string());
    let preset = options.preset.unwrap_or_else(|| "slow".to_string());

    let output = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(input)
        .args([
            "-c:v",
            "libx264",
            "-preset",
            &preset,
            "-crf",
            &crf,
            "-c:a",
            "aac",
            "-b:a",
            &audio_bitrate,
            "-movflags",
            "+faststart",
        ])
        .arg(&output_path)
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        log::error!("FFmpeg compression error: {}", stderr);
        return Err(ConversionError::FfmpegError(stderr.to_string()));
    }

    Ok(output_path)
}

/// Build atempo filter chain for speed changes
/// atempo filter has range 0.5-2.0, so we chain filters for extreme values
fn build_atempo_filter(speed: f64) -> String {
    if speed > 2.0 {
        format!("atempo=2.0,atempo={}", speed / 2.0)
    } else if speed < 0.5 {
        format!("atempo=0.5,atempo={}", speed / 0.5)
    } else {
        format!("atempo={}", speed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_atempo_filter() {
        assert_eq!(build_atempo_filter(1.0), "atempo=1");
        assert_eq!(build_atempo_filter(1.5), "atempo=1.5");
        assert_eq!(build_atempo_filter(2.5), "atempo=2.0,atempo=1.25");
        assert_eq!(build_atempo_filter(0.25), "atempo=0.5,atempo=0.5");
    }

    #[test]
    fn test_video_note_options_default() {
        let opts = VideoNoteOptions::default();
        assert!(opts.duration.is_none());
        assert!(opts.start_time.is_none());
        assert!(opts.speed.is_none());
    }

    #[test]
    fn test_gif_options_default() {
        let opts = GifOptions::default();
        assert_eq!(opts.duration, Some(10));
        assert_eq!(opts.width, Some(480));
        assert_eq!(opts.fps, Some(15));
    }

    #[test]
    fn test_compression_options_default() {
        let opts = CompressionOptions::default();
        assert_eq!(opts.crf, Some(28));
        assert_eq!(opts.audio_bitrate, Some("128k".to_string()));
        assert_eq!(opts.preset, Some("slow".to_string()));
    }
}
