//! Audio format conversion utilities
//!
//! Converts between audio formats using FFmpeg:
//! - WAV, FLAC, OGG, M4A, Opus, AAC <-> MP3
//! - Configurable bitrate for lossy formats

use super::{temp_output_path, ConversionError, ConversionResult};
use std::path::Path;
use tokio::process::Command;

/// Supported audio formats for conversion.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AudioFormat {
    Mp3,
    Wav,
    Flac,
    Ogg,
    M4a,
    Opus,
    Aac,
}

impl AudioFormat {
    /// Parse from file extension string.
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext.to_lowercase().as_str() {
            "mp3" => Some(Self::Mp3),
            "wav" => Some(Self::Wav),
            "flac" => Some(Self::Flac),
            "ogg" | "oga" => Some(Self::Ogg),
            "m4a" => Some(Self::M4a),
            "opus" => Some(Self::Opus),
            "aac" => Some(Self::Aac),
            _ => None,
        }
    }

    /// Get the file extension for this format.
    pub fn extension(&self) -> &str {
        match self {
            Self::Mp3 => "mp3",
            Self::Wav => "wav",
            Self::Flac => "flac",
            Self::Ogg => "ogg",
            Self::M4a => "m4a",
            Self::Opus => "opus",
            Self::Aac => "aac",
        }
    }

    /// Get the FFmpeg codec name.
    fn codec(&self) -> &str {
        match self {
            Self::Mp3 => "libmp3lame",
            Self::Wav => "pcm_s16le",
            Self::Flac => "flac",
            Self::Ogg => "libvorbis",
            Self::M4a => "aac",
            Self::Opus => "libopus",
            Self::Aac => "aac",
        }
    }

    /// Whether this format supports bitrate setting.
    pub fn supports_bitrate(&self) -> bool {
        matches!(self, Self::Mp3 | Self::Ogg | Self::M4a | Self::Opus | Self::Aac)
    }

    /// Human-readable display name.
    pub fn display_name(&self) -> &str {
        match self {
            Self::Mp3 => "MP3",
            Self::Wav => "WAV",
            Self::Flac => "FLAC",
            Self::Ogg => "OGG",
            Self::M4a => "M4A",
            Self::Opus => "OPUS",
            Self::Aac => "AAC",
        }
    }
}

/// Convert an audio file to a target format.
///
/// # Arguments
/// * `input_path` - Path to the input audio file
/// * `target` - Target audio format
/// * `bitrate` - Optional bitrate (e.g., "320k", "192k"). Only used for lossy formats.
///
/// # Returns
/// Path to the converted audio file
pub async fn convert_audio<P: AsRef<Path>>(
    input_path: P,
    target: AudioFormat,
    bitrate: Option<&str>,
) -> ConversionResult<std::path::PathBuf> {
    let input = input_path.as_ref();

    if !input.exists() {
        return Err(ConversionError::InputNotFound(input.display().to_string()));
    }

    let output_path = temp_output_path("audio_convert", target.extension());

    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-y")
        .arg("-i")
        .arg(input);

    // Set codec
    cmd.arg("-acodec").arg(target.codec());

    // Set bitrate for lossy formats
    if target.supports_bitrate() {
        let br = bitrate.unwrap_or("320k");
        cmd.arg("-b:a").arg(br);
    }

    // For M4A/AAC, set container format
    if matches!(target, AudioFormat::M4a) {
        cmd.arg("-f").arg("ipod");
    }

    cmd.arg(&output_path);

    let output = cmd.output().await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        log::error!("FFmpeg audio conversion error: {}", stderr);
        return Err(ConversionError::FfmpegError(stderr.to_string()));
    }

    Ok(output_path)
}

/// Check if a file is a supported audio format for conversion.
pub fn is_convertible_audio<P: AsRef<Path>>(path: P) -> bool {
    path.as_ref()
        .extension()
        .and_then(|e| e.to_str())
        .and_then(AudioFormat::from_extension)
        .is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audio_format_from_extension() {
        assert_eq!(AudioFormat::from_extension("mp3"), Some(AudioFormat::Mp3));
        assert_eq!(AudioFormat::from_extension("MP3"), Some(AudioFormat::Mp3));
        assert_eq!(AudioFormat::from_extension("wav"), Some(AudioFormat::Wav));
        assert_eq!(AudioFormat::from_extension("flac"), Some(AudioFormat::Flac));
        assert_eq!(AudioFormat::from_extension("ogg"), Some(AudioFormat::Ogg));
        assert_eq!(AudioFormat::from_extension("oga"), Some(AudioFormat::Ogg));
        assert_eq!(AudioFormat::from_extension("m4a"), Some(AudioFormat::M4a));
        assert_eq!(AudioFormat::from_extension("opus"), Some(AudioFormat::Opus));
        assert_eq!(AudioFormat::from_extension("aac"), Some(AudioFormat::Aac));
        assert_eq!(AudioFormat::from_extension("xyz"), None);
        assert_eq!(AudioFormat::from_extension("pdf"), None);
    }

    #[test]
    fn test_audio_format_from_extension_case_insensitive() {
        assert_eq!(AudioFormat::from_extension("WAV"), Some(AudioFormat::Wav));
        assert_eq!(AudioFormat::from_extension("Flac"), Some(AudioFormat::Flac));
        assert_eq!(AudioFormat::from_extension("OGG"), Some(AudioFormat::Ogg));
        assert_eq!(AudioFormat::from_extension("OGA"), Some(AudioFormat::Ogg));
        assert_eq!(AudioFormat::from_extension("M4A"), Some(AudioFormat::M4a));
        assert_eq!(AudioFormat::from_extension("OPUS"), Some(AudioFormat::Opus));
        assert_eq!(AudioFormat::from_extension("AAC"), Some(AudioFormat::Aac));
    }

    #[test]
    fn test_audio_format_extension() {
        assert_eq!(AudioFormat::Mp3.extension(), "mp3");
        assert_eq!(AudioFormat::Wav.extension(), "wav");
        assert_eq!(AudioFormat::Flac.extension(), "flac");
        assert_eq!(AudioFormat::Ogg.extension(), "ogg");
        assert_eq!(AudioFormat::M4a.extension(), "m4a");
        assert_eq!(AudioFormat::Opus.extension(), "opus");
        assert_eq!(AudioFormat::Aac.extension(), "aac");
    }

    #[test]
    fn test_audio_format_supports_bitrate() {
        assert!(AudioFormat::Mp3.supports_bitrate());
        assert!(AudioFormat::Ogg.supports_bitrate());
        assert!(AudioFormat::M4a.supports_bitrate());
        assert!(AudioFormat::Opus.supports_bitrate());
        assert!(AudioFormat::Aac.supports_bitrate());
        assert!(!AudioFormat::Wav.supports_bitrate());
        assert!(!AudioFormat::Flac.supports_bitrate());
    }

    #[test]
    fn test_audio_format_display_name() {
        assert_eq!(AudioFormat::Mp3.display_name(), "MP3");
        assert_eq!(AudioFormat::Wav.display_name(), "WAV");
        assert_eq!(AudioFormat::Flac.display_name(), "FLAC");
        assert_eq!(AudioFormat::Ogg.display_name(), "OGG");
        assert_eq!(AudioFormat::M4a.display_name(), "M4A");
        assert_eq!(AudioFormat::Opus.display_name(), "OPUS");
        assert_eq!(AudioFormat::Aac.display_name(), "AAC");
    }

    #[test]
    fn test_audio_format_codec() {
        assert_eq!(AudioFormat::Mp3.codec(), "libmp3lame");
        assert_eq!(AudioFormat::Wav.codec(), "pcm_s16le");
        assert_eq!(AudioFormat::Flac.codec(), "flac");
        assert_eq!(AudioFormat::Ogg.codec(), "libvorbis");
        assert_eq!(AudioFormat::M4a.codec(), "aac");
        assert_eq!(AudioFormat::Opus.codec(), "libopus");
        assert_eq!(AudioFormat::Aac.codec(), "aac");
    }

    #[test]
    fn test_is_convertible_audio() {
        assert!(is_convertible_audio("/tmp/test.mp3"));
        assert!(is_convertible_audio("/tmp/test.wav"));
        assert!(is_convertible_audio("/tmp/test.flac"));
        assert!(is_convertible_audio("/tmp/test.ogg"));
        assert!(is_convertible_audio("/tmp/test.m4a"));
        assert!(is_convertible_audio("/tmp/test.opus"));
        assert!(is_convertible_audio("/tmp/test.aac"));
        assert!(is_convertible_audio("/tmp/test.oga"));
        assert!(!is_convertible_audio("/tmp/test.pdf"));
        assert!(!is_convertible_audio("/tmp/test.mp4"));
        assert!(!is_convertible_audio("/tmp/test.docx"));
        assert!(!is_convertible_audio("/tmp/test"));
    }

    #[test]
    fn test_audio_format_roundtrip() {
        // extension() → from_extension() should roundtrip
        let formats = [
            AudioFormat::Mp3,
            AudioFormat::Wav,
            AudioFormat::Flac,
            AudioFormat::Ogg,
            AudioFormat::M4a,
            AudioFormat::Opus,
            AudioFormat::Aac,
        ];
        for fmt in &formats {
            let ext = fmt.extension();
            let parsed = AudioFormat::from_extension(ext);
            assert_eq!(parsed, Some(*fmt), "Roundtrip failed for {:?}", fmt);
        }
    }

    #[tokio::test]
    async fn test_convert_audio_input_not_found() {
        let result = convert_audio("/tmp/nonexistent_audio_file_12345.mp3", AudioFormat::Wav, None).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ConversionError::InputNotFound(path) => {
                assert!(path.contains("nonexistent_audio_file_12345"));
            }
            other => panic!("Expected InputNotFound, got: {:?}", other),
        }
    }
}
