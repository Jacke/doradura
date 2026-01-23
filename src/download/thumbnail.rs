//! Thumbnail processing utilities for video and image downloads.
//!
//! This module provides functions for:
//! - Detecting image formats from magic bytes (JPEG, PNG, WebP)
//! - Converting WebP images to JPEG format
//! - Compressing JPEG thumbnails to meet Telegram size limits
//! - Generating thumbnails from video files using ffmpeg

use crate::core::error::AppError;
use std::fs;
use std::process::Command;

/// Image format detected by magic bytes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImageFormat {
    Jpeg,
    Png,
    WebP,
    Unknown,
}

/// Detects image format from the first bytes of a file (magic bytes)
///
/// # Arguments
///
/// * `bytes` - The first bytes of the image file (at least 12 bytes recommended)
///
/// # Returns
///
/// The detected `ImageFormat` or `ImageFormat::Unknown` if the format cannot be determined
pub(crate) fn detect_image_format(bytes: &[u8]) -> ImageFormat {
    if bytes.len() < 4 {
        return ImageFormat::Unknown;
    }

    // JPEG: FF D8 FF
    if bytes.len() >= 3 && bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes[2] == 0xFF {
        return ImageFormat::Jpeg;
    }

    // PNG: 89 50 4E 47
    if bytes.len() >= 4 && bytes[0] == 0x89 && bytes[1] == 0x50 && bytes[2] == 0x4E && bytes[3] == 0x47 {
        return ImageFormat::Png;
    }

    // WebP: RIFF...WEBP
    if bytes.len() >= 12
        && bytes[0] == 0x52
        && bytes[1] == 0x49
        && bytes[2] == 0x46
        && bytes[3] == 0x46
        && bytes[8] == 0x57
        && bytes[9] == 0x45
        && bytes[10] == 0x42
        && bytes[11] == 0x50
    {
        return ImageFormat::WebP;
    }

    ImageFormat::Unknown
}

/// Converts a WebP image to JPEG format using ffmpeg
///
/// # Arguments
///
/// * `webp_bytes` - The raw bytes of the WebP image
///
/// # Returns
///
/// The JPEG image bytes on success, or an error if conversion fails
pub(crate) fn convert_webp_to_jpeg(webp_bytes: &[u8]) -> Result<Vec<u8>, AppError> {
    // Create temporary file for WebP
    let temp_dir = std::env::temp_dir();
    let temp_webp = temp_dir.join(format!(
        "temp_webp_{}.webp",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    ));
    let temp_jpeg = temp_dir.join(format!(
        "temp_jpeg_{}.jpg",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    ));

    // Save WebP to temporary file
    fs::write(&temp_webp, webp_bytes)
        .map_err(|e| AppError::Download(format!("Failed to write WebP temp file: {}", e)))?;

    // Convert WebP to JPEG using ffmpeg
    let output = Command::new("ffmpeg")
        .args([
            "-i",
            temp_webp.to_str().unwrap_or(""),
            "-q:v",
            "2",  // High quality
            "-y", // Overwrite output file
            temp_jpeg.to_str().unwrap_or(""),
        ])
        .output();

    let _ = fs::remove_file(&temp_webp);

    match output {
        Ok(result) => {
            if result.status.success() {
                match fs::read(&temp_jpeg) {
                    Ok(jpeg_bytes) => {
                        let _ = fs::remove_file(&temp_jpeg);
                        Ok(jpeg_bytes)
                    }
                    Err(e) => {
                        let _ = fs::remove_file(&temp_jpeg);
                        Err(AppError::Download(format!("Failed to read converted JPEG: {}", e)))
                    }
                }
            } else {
                let stderr = String::from_utf8_lossy(&result.stderr);
                let _ = fs::remove_file(&temp_jpeg);
                Err(AppError::Download(format!("ffmpeg conversion failed: {}", stderr)))
            }
        }
        Err(e) => {
            let _ = fs::remove_file(&temp_jpeg);
            Err(AppError::Download(format!("Failed to run ffmpeg: {}", e)))
        }
    }
}

/// Compresses a JPEG thumbnail to meet Telegram's 200KB size limit
///
/// Uses ffmpeg to resize and compress the image. Returns `None` if compression fails
/// or if the resulting image is still larger than 200KB.
///
/// # Arguments
///
/// * `jpeg_bytes` - The raw bytes of the JPEG image
///
/// # Returns
///
/// The compressed JPEG bytes if successful and under 200KB, or `None` otherwise
pub(crate) fn compress_thumbnail_jpeg(jpeg_bytes: &[u8]) -> Option<Vec<u8>> {
    // Create temporary files
    let temp_dir = std::env::temp_dir();
    let temp_input = temp_dir.join(format!(
        "thumb_in_{}.jpg",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    ));
    let temp_output = temp_dir.join(format!(
        "thumb_out_{}.jpg",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    ));

    if fs::write(&temp_input, jpeg_bytes).is_err() {
        return None;
    }

    // Compress using ffmpeg with reduced quality and size
    let output = Command::new("ffmpeg")
        .args([
            "-i",
            temp_input.to_str().unwrap_or(""),
            "-vf",
            "scale=320:320:force_original_aspect_ratio=decrease",
            "-q:v",
            "5", // Medium quality for size reduction
            "-y",
            temp_output.to_str().unwrap_or(""),
        ])
        .output();

    let _ = fs::remove_file(&temp_input);

    match output {
        Ok(result) => {
            if result.status.success() {
                if let Ok(compressed) = fs::read(&temp_output) {
                    let _ = fs::remove_file(&temp_output);
                    if compressed.len() <= 200 * 1024 {
                        Some(compressed)
                    } else {
                        // If still too large, could try lower quality but return None for now
                        None
                    }
                } else {
                    let _ = fs::remove_file(&temp_output);
                    None
                }
            } else {
                let _ = fs::remove_file(&temp_output);
                None
            }
        }
        Err(_) => {
            let _ = fs::remove_file(&temp_output);
            None
        }
    }
}

/// Generates a thumbnail from a video file using ffmpeg
///
/// Extracts the first frame of the video and saves it as a JPEG image,
/// scaled to fit within 320x320 pixels (Telegram's recommended thumbnail size).
///
/// # Arguments
///
/// * `video_path` - Path to the video file
///
/// # Returns
///
/// The JPEG thumbnail bytes if successful, or `None` if extraction fails
pub(crate) fn generate_thumbnail_from_video(video_path: &str) -> Option<Vec<u8>> {
    log::info!("[THUMBNAIL] Generating thumbnail from video file: {}", video_path);

    // Create temporary file for thumbnail
    let temp_dir = std::env::temp_dir();
    let temp_thumbnail_path = temp_dir.join(format!(
        "thumb_{}.jpg",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    ));

    // Extract first frame using ffmpeg
    // vframes=1 gets a single frame
    // scale limits size to 320x320 max for Telegram
    let output = Command::new("ffmpeg")
        .args([
            "-i",
            video_path,
            "-vframes",
            "1",
            "-vf",
            "scale=320:320:force_original_aspect_ratio=decrease",
            "-q:v",
            "2", // High quality JPEG (2 = high, 31 = low)
            "-f",
            "image2",
            temp_thumbnail_path.to_str().unwrap_or(""),
        ])
        .output();

    match output {
        Ok(result) => {
            if result.status.success() {
                // Read the generated thumbnail
                match fs::read(&temp_thumbnail_path) {
                    Ok(bytes) => {
                        log::info!(
                            "[THUMBNAIL] Successfully generated thumbnail from video: {} bytes ({} KB)",
                            bytes.len(),
                            bytes.len() as f64 / 1024.0
                        );

                        // Remove temporary file
                        let _ = fs::remove_file(&temp_thumbnail_path);

                        // Check size (Telegram requires <= 200 KB)
                        if bytes.len() > 200 * 1024 {
                            log::warn!(
                                "[THUMBNAIL] Generated thumbnail size ({} KB) exceeds Telegram limit (200 KB). Will try to compress.",
                                bytes.len() as f64 / 1024.0
                            );
                            // Telegram may accept files > 200KB but might not display preview
                        }

                        Some(bytes)
                    }
                    Err(e) => {
                        log::warn!("[THUMBNAIL] Failed to read generated thumbnail: {}", e);
                        let _ = fs::remove_file(&temp_thumbnail_path);
                        None
                    }
                }
            } else {
                let stderr = String::from_utf8_lossy(&result.stderr);
                log::warn!("[THUMBNAIL] ffmpeg failed to generate thumbnail: {}", stderr);
                let _ = fs::remove_file(&temp_thumbnail_path);
                None
            }
        }
        Err(e) => {
            log::warn!("[THUMBNAIL] Failed to run ffmpeg to generate thumbnail: {}", e);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ==================== ImageFormat Tests ====================

    #[test]
    fn test_image_format_debug() {
        assert_eq!(format!("{:?}", ImageFormat::Jpeg), "Jpeg");
        assert_eq!(format!("{:?}", ImageFormat::Png), "Png");
        assert_eq!(format!("{:?}", ImageFormat::WebP), "WebP");
        assert_eq!(format!("{:?}", ImageFormat::Unknown), "Unknown");
    }

    #[test]
    fn test_image_format_clone() {
        let format = ImageFormat::Jpeg;
        let cloned = format;
        assert_eq!(format, cloned);
    }

    #[test]
    fn test_image_format_copy() {
        let format = ImageFormat::Png;
        let copied: ImageFormat = format;
        assert_eq!(format, copied);
    }

    // ==================== detect_image_format Tests ====================

    #[test]
    fn test_detect_image_format_jpeg() {
        // JPEG magic bytes: FF D8 FF
        let jpeg_bytes = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
        assert_eq!(detect_image_format(&jpeg_bytes), ImageFormat::Jpeg);
    }

    #[test]
    fn test_detect_image_format_png() {
        // PNG magic bytes: 89 50 4E 47 (0x89 PNG)
        let png_bytes = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        assert_eq!(detect_image_format(&png_bytes), ImageFormat::Png);
    }

    #[test]
    fn test_detect_image_format_webp() {
        // WebP magic bytes: RIFF....WEBP
        let webp_bytes = vec![
            0x52, 0x49, 0x46, 0x46, // RIFF
            0x00, 0x00, 0x00, 0x00, // size placeholder
            0x57, 0x45, 0x42, 0x50, // WEBP
        ];
        assert_eq!(detect_image_format(&webp_bytes), ImageFormat::WebP);
    }

    #[test]
    fn test_detect_image_format_unknown() {
        let random_bytes = vec![0x00, 0x01, 0x02, 0x03];
        assert_eq!(detect_image_format(&random_bytes), ImageFormat::Unknown);
    }

    #[test]
    fn test_detect_image_format_too_short() {
        let short_bytes = vec![0xFF, 0xD8];
        assert_eq!(detect_image_format(&short_bytes), ImageFormat::Unknown);

        let empty_bytes: Vec<u8> = vec![];
        assert_eq!(detect_image_format(&empty_bytes), ImageFormat::Unknown);
    }
}
