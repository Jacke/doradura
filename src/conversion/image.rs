//! Image conversion utilities
//!
//! Provides conversions:
//! - Resize images
//! - Format conversion (JPG, PNG, WebP)

use super::{temp_output_path, ConversionError, ConversionResult};
use std::path::Path;
use tokio::process::Command;

/// Options for image resizing
#[derive(Debug, Clone)]
pub struct ResizeOptions {
    /// Target width (None to auto-calculate from height)
    pub width: Option<u32>,
    /// Target height (None to auto-calculate from width)
    pub height: Option<u32>,
    /// Maintain aspect ratio (default: true)
    pub maintain_aspect: bool,
    /// Quality for JPEG output (1-100, default: 90)
    pub quality: Option<u8>,
}

impl Default for ResizeOptions {
    fn default() -> Self {
        Self {
            width: None,
            height: None,
            maintain_aspect: true,
            quality: Some(90),
        }
    }
}

/// Supported output formats
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ImageFormat {
    Jpeg,
    Png,
    WebP,
}

impl ImageFormat {
    pub fn extension(&self) -> &'static str {
        match self {
            ImageFormat::Jpeg => "jpg",
            ImageFormat::Png => "png",
            ImageFormat::WebP => "webp",
        }
    }

    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext.to_lowercase().as_str() {
            "jpg" | "jpeg" => Some(ImageFormat::Jpeg),
            "png" => Some(ImageFormat::Png),
            "webp" => Some(ImageFormat::WebP),
            _ => None,
        }
    }
}

/// Resize an image
///
/// # Arguments
/// * `input_path` - Path to input image file
/// * `options` - Resize options
/// * `output_format` - Output format (optional, defaults to input format)
///
/// # Returns
/// Path to the resized image file
pub async fn resize<P: AsRef<Path>>(
    input_path: P,
    options: ResizeOptions,
    output_format: Option<ImageFormat>,
) -> ConversionResult<std::path::PathBuf> {
    let input = input_path.as_ref();

    if !input.exists() {
        return Err(ConversionError::InputNotFound(input.display().to_string()));
    }

    // Determine output format from input if not specified
    let format = output_format.unwrap_or_else(|| {
        input
            .extension()
            .and_then(|e| e.to_str())
            .and_then(ImageFormat::from_extension)
            .unwrap_or(ImageFormat::Jpeg)
    });

    let output_path = temp_output_path("resized", format.extension());

    // Build scale filter
    let scale = match (options.width, options.height, options.maintain_aspect) {
        (Some(w), Some(h), false) => format!("scale={}:{}", w, h),
        (Some(w), Some(h), true) => format!("scale={}:{}:force_original_aspect_ratio=decrease", w, h),
        (Some(w), None, _) => format!("scale={}:-1", w),
        (None, Some(h), _) => format!("scale=-1:{}", h),
        (None, None, _) => "scale=iw:ih".to_string(), // No resize, just format conversion
    };

    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(input)
        .arg("-vf")
        .arg(&scale);

    // Add quality settings for JPEG
    if format == ImageFormat::Jpeg {
        let quality = options.quality.unwrap_or(90);
        cmd.arg("-qscale:v").arg(format!("{}", (100 - quality) / 3)); // FFmpeg uses 2-31 scale
    }

    // Add quality settings for WebP
    if format == ImageFormat::WebP {
        let quality = options.quality.unwrap_or(90);
        cmd.arg("-quality").arg(format!("{}", quality));
    }

    cmd.arg(&output_path);

    let output = cmd.output().await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        log::error!("FFmpeg image resize error: {}", stderr);
        return Err(ConversionError::FfmpegError(stderr.to_string()));
    }

    Ok(output_path)
}

/// Convert image to different format
///
/// # Arguments
/// * `input_path` - Path to input image file
/// * `format` - Target format
/// * `quality` - Quality (1-100, for JPEG/WebP)
///
/// # Returns
/// Path to the converted image file
pub async fn convert_format<P: AsRef<Path>>(
    input_path: P,
    format: ImageFormat,
    quality: Option<u8>,
) -> ConversionResult<std::path::PathBuf> {
    resize(
        input_path,
        ResizeOptions {
            width: None,
            height: None,
            maintain_aspect: true,
            quality,
        },
        Some(format),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_image_format_extension() {
        assert_eq!(ImageFormat::Jpeg.extension(), "jpg");
        assert_eq!(ImageFormat::Png.extension(), "png");
        assert_eq!(ImageFormat::WebP.extension(), "webp");
    }

    #[test]
    fn test_image_format_from_extension() {
        assert_eq!(ImageFormat::from_extension("jpg"), Some(ImageFormat::Jpeg));
        assert_eq!(ImageFormat::from_extension("jpeg"), Some(ImageFormat::Jpeg));
        assert_eq!(ImageFormat::from_extension("PNG"), Some(ImageFormat::Png));
        assert_eq!(ImageFormat::from_extension("webp"), Some(ImageFormat::WebP));
        assert_eq!(ImageFormat::from_extension("bmp"), None);
    }

    #[test]
    fn test_resize_options_default() {
        let opts = ResizeOptions::default();
        assert!(opts.width.is_none());
        assert!(opts.height.is_none());
        assert!(opts.maintain_aspect);
        assert_eq!(opts.quality, Some(90));
    }
}
