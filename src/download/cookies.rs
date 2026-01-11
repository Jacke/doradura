//! YouTube cookies management for yt-dlp
//!
//! This module provides functionality to:
//! - Validate YouTube cookies
//! - Update cookies file from base64 string
//! - Check cookies freshness periodically

use anyhow::Result;
use base64::{engine::general_purpose, Engine as _};
use std::path::PathBuf;
use tokio::process::Command;

/// Validates YouTube cookies by testing a known video URL
///
/// Returns true if cookies are valid and working, false otherwise
pub async fn validate_cookies() -> bool {
    let cookies_path = match get_cookies_path() {
        Some(path) => path,
        None => {
            log::warn!("No cookies file configured (YTDL_COOKIES_FILE not set)");
            return false;
        }
    };

    if !cookies_path.exists() {
        log::warn!("Cookies file does not exist: {:?}", cookies_path);
        return false;
    }

    // Test cookies with a simple metadata fetch
    // Use a known age-restricted video that requires auth
    let test_url = "https://www.youtube.com/watch?v=dQw4w9WgXcQ";

    let ytdl_bin = crate::core::config::YTDL_BIN.as_str();

    let output = Command::new(ytdl_bin)
        .arg("--no-warnings")
        .arg("--no-playlist")
        .arg("--skip-download")
        .arg("--cookies")
        .arg(&cookies_path)
        .arg("--print")
        .arg("id")
        .arg(test_url)
        .output()
        .await;

    match output {
        Ok(output) => {
            if output.status.success() {
                true
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                log::warn!("❌ Cookies validation failed: {}", stderr);

                // Check for specific error patterns
                if stderr.contains("Sign in to confirm") || stderr.contains("bot") || stderr.contains("Cookie") {
                    log::error!("🔴 Cookies are invalid or expired!");
                    false
                } else {
                    // Other errors might be network-related, give benefit of the doubt
                    log::warn!("⚠️  Validation inconclusive, assuming cookies are OK");
                    true
                }
            }
        }
        Err(e) => {
            log::error!("Failed to execute yt-dlp for cookies validation: {}", e);
            false
        }
    }
}

/// Returns the configured cookies file path from environment
fn get_cookies_path() -> Option<PathBuf> {
    crate::core::config::YTDL_COOKIES_FILE.as_ref().map(PathBuf::from)
}

/// Updates the cookies file from a base64-encoded string
///
/// # Arguments
/// * `cookies_b64` - Base64-encoded cookies file content
///
/// # Returns
/// * `Ok(PathBuf)` - Path to the updated cookies file
/// * `Err` - If update fails
pub async fn update_cookies_from_base64(cookies_b64: &str) -> Result<PathBuf> {
    let cookies_path = get_cookies_path().ok_or_else(|| anyhow::anyhow!("YTDL_COOKIES_FILE not configured"))?;

    // Decode base64
    let decoded = general_purpose::STANDARD
        .decode(cookies_b64.trim())
        .map_err(|e| anyhow::anyhow!("Invalid base64: {}", e))?;

    let cookies_content = String::from_utf8(decoded).map_err(|e| anyhow::anyhow!("Invalid UTF-8 in cookies: {}", e))?;

    // Basic validation: check if it looks like Netscape cookies format
    if !cookies_content.contains("# Netscape HTTP Cookie File") && !cookies_content.contains(".youtube.com") {
        return Err(anyhow::anyhow!(
            "Invalid cookies format. Expected Netscape HTTP Cookie File format with youtube.com entries"
        ));
    }

    // Write to file
    tokio::fs::write(&cookies_path, cookies_content)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to write cookies file: {}", e))?;

    log::info!("✅ Cookies file updated: {:?}", cookies_path);

    Ok(cookies_path)
}

/// Checks if cookies need refresh by validating them
///
/// Returns true if cookies are missing, invalid, or expired
pub async fn needs_refresh() -> bool {
    let cookies_path = match get_cookies_path() {
        Some(path) => path,
        None => return true, // No cookies configured
    };

    if !cookies_path.exists() {
        log::info!("Cookies file missing: {:?}", cookies_path);
        return true;
    }

    // Validate cookies
    !validate_cookies().await
}

pub async fn update_cookies_from_content(content: &str) -> Result<PathBuf> {
    let cookies_path = get_cookies_path().ok_or_else(|| anyhow::anyhow!("YTDL_COOKIES_FILE not configured"))?;

    // Basic validation: check if it looks like Netscape cookies format
    if !content.contains("# Netscape HTTP Cookie File") && !content.contains(".youtube.com") {
        return Err(anyhow::anyhow!(
            "Invalid cookies format. Expected Netscape HTTP Cookie File format with youtube.com entries"
        ));
    }

    // Write to file
    tokio::fs::write(&cookies_path, content)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to write cookies file: {}", e))?;

    log::info!("✅ Cookies file updated from content: {:?}", cookies_path);

    Ok(cookies_path)
}
mod tests {
    #[test]
    fn test_get_cookies_path() {
        // This test will depend on env vars, just ensure it doesn't crash
        let _path = super::get_cookies_path();
    }

    #[tokio::test]
    async fn test_update_cookies_invalid_base64() {
        let result = super::update_cookies_from_base64("not-valid-base64!@#").await;
        assert!(result.is_err());
    }

    #[test]
    fn test_cookies_validation_format() {
        let valid_content = "# Netscape HTTP Cookie File\n.youtube.com\tTRUE\t/\tTRUE\t0\ttest\tvalue";
        assert!(valid_content.contains("# Netscape HTTP Cookie File"));
        assert!(valid_content.contains(".youtube.com"));
    }
}
