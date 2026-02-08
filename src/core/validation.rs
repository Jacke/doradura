//! URL and path validation utilities
//!
//! Provides security-focused validation for user inputs:
//! - YouTube URL validation (whitelist-based)
//! - Path sanitization (prevent directory traversal)
//! - Filename sanitization (remove filesystem-unsafe characters)
//!
//! Inspired by boul2gom/yt-dlp validation patterns, adapted for doradura.

use std::path::{Component, Path};
use thiserror::Error;
use url::Url;

/// Validation errors
#[derive(Debug, Error)]
pub enum ValidationError {
    /// Invalid URL format or non-YouTube domain
    #[error("Invalid YouTube URL: {0}")]
    InvalidUrl(String),

    /// Path validation failed (traversal attempt, absolute path, etc.)
    #[error("Invalid path '{path}': {reason}")]
    InvalidPath { path: String, reason: String },

    /// Empty result after sanitization
    #[error("Path '{0}' is empty after sanitization")]
    EmptyPath(String),
}

/// Validates that a URL is a valid YouTube URL.
///
/// # Security
/// Uses whitelist approach:
/// - Only HTTP/HTTPS schemes allowed
/// - Only youtube.com, youtu.be, youtube-nocookie.com domains (+ subdomains)
///
/// # Arguments
/// * `url` - The URL string to validate
///
/// # Returns
/// * `Ok(())` if URL is valid YouTube URL
/// * `Err(ValidationError)` if invalid
///
/// # Examples
/// ```
/// use doradura::core::validation::validate_youtube_url;
///
/// // Valid URLs
/// assert!(validate_youtube_url("https://youtube.com/watch?v=dQw4w9WgXcQ").is_ok());
/// assert!(validate_youtube_url("https://youtu.be/dQw4w9WgXcQ").is_ok());
/// assert!(validate_youtube_url("https://www.youtube.com/watch?v=dQw4w9WgXcQ").is_ok());
/// assert!(validate_youtube_url("https://m.youtube.com/watch?v=dQw4w9WgXcQ").is_ok());
///
/// // Invalid URLs
/// assert!(validate_youtube_url("https://evil.com/watch?v=dQw4w9WgXcQ").is_err());
/// assert!(validate_youtube_url("ftp://youtube.com/video").is_err());
/// assert!(validate_youtube_url("not a url").is_err());
/// ```
pub fn validate_youtube_url(url: &str) -> Result<(), ValidationError> {
    // Parse URL
    let parsed = Url::parse(url).map_err(|_| ValidationError::InvalidUrl(url.to_string()))?;

    // Only HTTP and HTTPS are allowed
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(ValidationError::InvalidUrl(format!(
            "{} (invalid scheme: {})",
            url,
            parsed.scheme()
        )));
    }

    // Check host is YouTube domain
    let host = parsed
        .host_str()
        .ok_or_else(|| ValidationError::InvalidUrl(format!("{} (no host)", url)))?;

    let is_youtube = host == "youtube.com"
        || host.ends_with(".youtube.com")
        || host == "youtu.be"
        || host.ends_with(".youtube-nocookie.com");

    if !is_youtube {
        return Err(ValidationError::InvalidUrl(format!(
            "{} (not a YouTube domain: {})",
            url, host
        )));
    }

    Ok(())
}

/// Sanitizes a file path by removing dangerous components.
///
/// # Security
/// Prevents directory traversal attacks by:
/// - Rejecting absolute paths
/// - Removing parent directory references (`..`)
/// - Removing root references
/// - Windows path prefixes are removed
///
/// # Arguments
/// * `path` - The path string to sanitize
///
/// # Returns
/// * `Ok(String)` with sanitized path
/// * `Err(ValidationError)` if path is invalid or empty after sanitization
///
/// # Examples
/// ```
/// use doradura::core::validation::sanitize_path;
///
/// // Valid paths
/// assert_eq!(sanitize_path("video.mp4").unwrap(), "video.mp4");
/// assert_eq!(sanitize_path("folder/video.mp4").unwrap(), "folder/video.mp4");
///
/// // Invalid paths (traversal attempts)
/// assert!(sanitize_path("/etc/passwd").is_err()); // absolute
///
/// // Paths with parent dirs are sanitized (.. removed)
/// assert_eq!(sanitize_path("../etc/passwd").unwrap(), "etc/passwd");
/// assert_eq!(sanitize_path("folder/../etc/passwd").unwrap(), "folder/etc/passwd");
/// ```
pub fn sanitize_path(path: &str) -> Result<String, ValidationError> {
    // Reject absolute paths
    if Path::new(path).is_absolute() {
        return Err(ValidationError::InvalidPath {
            path: path.to_string(),
            reason: "absolute paths not allowed".to_string(),
        });
    }

    // Filter out dangerous components
    let components: Vec<_> = Path::new(path)
        .components()
        .filter_map(|c| match c {
            // Remove parent directory and root references
            Component::ParentDir | Component::RootDir => None,
            // Accept normal path components
            Component::Normal(name) => Some(name.to_string_lossy().to_string()),
            Component::CurDir => Some(".".to_string()),
            Component::Prefix(_) => None, // Windows path prefixes
        })
        .collect();

    if components.is_empty() {
        return Err(ValidationError::EmptyPath(path.to_string()));
    }

    // Reconstruct path from safe components
    Ok(components.join("/"))
}

/// Sanitizes a filename by removing filesystem-unsafe characters.
///
/// # Security
/// Removes characters that could cause issues on various filesystems:
/// - Path separators: `/`, `\`
/// - Reserved characters: `:`, `*`, `?`, `"`, `<`, `>`, `|`
/// - Control characters (ASCII 0-31, 127)
///
/// # Arguments
/// * `name` - The filename to sanitize
///
/// # Returns
/// * Sanitized filename string
///
/// # Examples
/// ```
/// use doradura::core::validation::sanitize_filename;
///
/// assert_eq!(sanitize_filename("video.mp4"), "video.mp4");
/// assert_eq!(sanitize_filename("video:file.mp4"), "videofile.mp4");
/// assert_eq!(sanitize_filename("path/to/file.mp4"), "pathtofile.mp4");
/// assert_eq!(sanitize_filename("file*.mp4"), "file.mp4");
/// ```
pub fn sanitize_filename(name: &str) -> String {
    name.chars()
        // Remove filesystem-unsafe characters
        .filter(|c| !['/', '\\', ':', '*', '?', '"', '<', '>', '|'].contains(c))
        // Remove control characters
        .filter(|c| !c.is_control())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ==================== validate_youtube_url Tests ====================

    #[test]
    fn test_validate_youtube_url_valid() {
        let valid_urls = vec![
            "https://youtube.com/watch?v=dQw4w9WgXcQ",
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            "https://m.youtube.com/watch?v=dQw4w9WgXcQ",
            "https://youtu.be/dQw4w9WgXcQ",
            "http://youtube.com/watch?v=dQw4w9WgXcQ", // http ok
            "https://music.youtube.com/watch?v=dQw4w9WgXcQ",
            "https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ",
        ];

        for url in valid_urls {
            assert!(validate_youtube_url(url).is_ok(), "Failed for: {}", url);
        }
    }

    #[test]
    fn test_validate_youtube_url_invalid_scheme() {
        let invalid_urls = vec![
            "ftp://youtube.com/watch?v=abc",
            "file:///youtube.com/watch?v=abc",
            "javascript:alert('xss')",
        ];

        for url in invalid_urls {
            assert!(validate_youtube_url(url).is_err(), "Should fail for: {}", url);
        }
    }

    #[test]
    fn test_validate_youtube_url_invalid_domain() {
        let invalid_urls = vec![
            "https://evil.com/watch?v=dQw4w9WgXcQ",
            "https://youtube.evil.com/watch?v=dQw4w9WgXcQ", // subdomain of evil.com
            "https://notyoutube.com/watch?v=dQw4w9WgXcQ",
            "https://youtubecom.malware.org/watch?v=abc",
        ];

        for url in invalid_urls {
            assert!(validate_youtube_url(url).is_err(), "Should fail for: {}", url);
        }
    }

    #[test]
    fn test_validate_youtube_url_malformed() {
        let invalid_urls = vec!["not a url", "htt://broken", "youtube.com", ""];

        for url in invalid_urls {
            assert!(validate_youtube_url(url).is_err(), "Should fail for: {}", url);
        }
    }

    // ==================== sanitize_path Tests ====================

    #[test]
    fn test_sanitize_path_valid() {
        let cases = vec![
            ("video.mp4", "video.mp4"),
            ("folder/video.mp4", "folder/video.mp4"),
            ("a/b/c/video.mp4", "a/b/c/video.mp4"),
            ("./video.mp4", "./video.mp4"),
        ];

        for (input, expected) in cases {
            assert_eq!(sanitize_path(input).unwrap(), expected, "Failed for: {}", input);
        }
    }

    #[test]
    fn test_sanitize_path_removes_parent_dirs() {
        let cases = vec![
            ("../etc/passwd", "etc/passwd"),
            ("folder/../etc/passwd", "folder/etc/passwd"),
            ("../../etc/passwd", "etc/passwd"),
            ("a/b/../../../etc/passwd", "a/b/etc/passwd"),
        ];

        for (input, expected) in cases {
            let sanitized = sanitize_path(input).unwrap();
            assert_eq!(sanitized, expected, "Failed for: {}", input);
            assert!(!sanitized.contains(".."), "Failed to remove .. from: {}", input);
        }
    }

    #[test]
    fn test_sanitize_path_rejects_absolute() {
        let cases = vec!["/etc/passwd", "/tmp/file", "/var/log/app.log"];

        for input in cases {
            assert!(sanitize_path(input).is_err(), "Should reject absolute path: {}", input);
        }
    }

    #[test]
    fn test_sanitize_path_allows_double_dots_in_filenames() {
        // Double dots in filenames are OK (not path traversal)
        let cases = vec![
            ("file..name", "file..name"),
            ("fold..er/file", "fold..er/file"),
            ("a/b/c..d", "a/b/c..d"),
        ];

        for (input, expected) in cases {
            assert_eq!(
                sanitize_path(input).unwrap(),
                expected,
                "Should allow .. in filename: {}",
                input
            );
        }
    }

    #[test]
    fn test_sanitize_path_empty_after_sanitization() {
        let cases = vec!["../../../", "..", "/..", "/"];

        for input in cases {
            assert!(
                sanitize_path(input).is_err(),
                "Should error for empty result: {}",
                input
            );
        }
    }

    // ==================== sanitize_filename Tests ====================

    #[test]
    fn test_sanitize_filename_valid() {
        let cases = vec![
            ("video.mp4", "video.mp4"),
            ("my-video_2024.mp4", "my-video_2024.mp4"),
            ("video (1).mp4", "video (1).mp4"),
            ("Видео на русском.mp4", "Видео на русском.mp4"),
        ];

        for (input, expected) in cases {
            assert_eq!(sanitize_filename(input), expected, "Failed for: {}", input);
        }
    }

    #[test]
    fn test_sanitize_filename_removes_unsafe_chars() {
        let cases = vec![
            ("video:file.mp4", "videofile.mp4"),
            ("path/to/file.mp4", "pathtofile.mp4"),
            ("file*.mp4", "file.mp4"),
            ("file?.mp4", "file.mp4"),
            ("file<>|.mp4", "file.mp4"),
            ("file\"name.mp4", "filename.mp4"),
            ("video\\file.mp4", "videofile.mp4"),
        ];

        for (input, expected) in cases {
            assert_eq!(sanitize_filename(input), expected, "Failed for: {}", input);
        }
    }

    #[test]
    fn test_sanitize_filename_removes_control_chars() {
        // ASCII control characters (0-31, 127)
        let input = "file\x00\x01\x1f\x7fname.mp4";
        let sanitized = sanitize_filename(input);
        assert_eq!(sanitized, "filename.mp4");
    }

    #[test]
    fn test_sanitize_filename_empty_result() {
        // All characters removed
        let input = "/:*?\"<>|";
        let sanitized = sanitize_filename(input);
        assert_eq!(sanitized, "");
    }

    // ==================== Integration Tests ====================

    #[test]
    fn test_full_validation_flow() {
        // Simulate validating user input for a download
        let url = "https://youtube.com/watch?v=dQw4w9WgXcQ";
        let path = "downloads/video.mp4";
        let filename = "My Video: Title*.mp4";

        // Validate URL
        assert!(validate_youtube_url(url).is_ok());

        // Sanitize path
        let safe_path = sanitize_path(path).unwrap();
        assert_eq!(safe_path, "downloads/video.mp4");

        // Sanitize filename
        let safe_filename = sanitize_filename(filename);
        assert_eq!(safe_filename, "My Video Title.mp4");
    }

    #[test]
    fn test_validation_error_messages() {
        // URL validation error
        let err = validate_youtube_url("https://evil.com").unwrap_err();
        assert!(err.to_string().contains("Invalid YouTube URL"));

        // Path validation error
        let err = sanitize_path("/etc/passwd").unwrap_err();
        assert!(err.to_string().contains("Invalid path"));

        // Empty path error
        let err = sanitize_path("../../../").unwrap_err();
        assert!(err.to_string().contains("empty after sanitization"));
    }
}
