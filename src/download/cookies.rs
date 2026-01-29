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

/// Validates YouTube cookies by testing video URLs that require authentication
///
/// Returns `Ok(())` if cookies are valid, or `Err(reason)` with a human-readable failure reason.
pub async fn validate_cookies() -> Result<(), String> {
    let cookies_path = match get_cookies_path() {
        Some(path) => path,
        None => {
            log::warn!("No cookies file configured (YTDL_COOKIES_FILE not set)");
            return Err("YTDL_COOKIES_FILE не задан — путь к cookies не настроен".to_string());
        }
    };

    if !cookies_path.exists() {
        log::warn!("Cookies file does not exist: {:?}", cookies_path);
        return Err(format!("Файл cookies не найден: {}", cookies_path.display()));
    }

    // Check file is not empty
    match std::fs::metadata(&cookies_path) {
        Ok(meta) if meta.len() == 0 => {
            return Err("Файл cookies пуст (0 байт)".to_string());
        }
        Err(e) => {
            return Err(format!("Не удалось прочитать файл cookies: {}", e));
        }
        _ => {}
    }

    // Test with multiple videos - some require auth more strictly
    let test_urls = [
        "https://www.youtube.com/watch?v=jNQXAC9IVRw", // "Me at the zoo" - first YouTube video
        "https://www.youtube.com/watch?v=dQw4w9WgXcQ", // Rick Astley
    ];

    let ytdl_bin = crate::core::config::YTDL_BIN.as_str();

    for test_url in &test_urls {
        let output = Command::new(ytdl_bin)
            .arg("--no-warnings")
            .arg("--no-playlist")
            .arg("--skip-download")
            .arg("--cookies")
            .arg(&cookies_path)
            // PO Token provider for YouTube bot detection bypass
            .arg("--extractor-args")
            .arg("youtubepot-bgutilhttp:base_url=http://127.0.0.1:4416")
            .arg("--js-runtimes")
            .arg("node")
            .arg("--print")
            .arg("%(id)s %(title)s")
            .arg(test_url)
            .output()
            .await;

        match output {
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);

                if stderr.contains("Sign in to confirm") || stderr.contains("not a bot") {
                    log::error!("🔴 Cookies validation failed for {}: {}", test_url, stderr);
                    return Err("YouTube требует авторизацию — cookies истекли или сессия недействительна".to_string());
                }

                if stderr.contains("Cookie") || stderr.contains("cookies") {
                    log::error!("🔴 Cookies validation failed for {}: {}", test_url, stderr);
                    return Err("Ошибка чтения cookies — файл повреждён или имеет неверный формат".to_string());
                }

                if stderr.contains("login") || stderr.contains("authentication") {
                    log::error!("🔴 Cookies validation failed for {}: {}", test_url, stderr);
                    return Err("YouTube требует повторный вход — сессия истекла".to_string());
                }

                if !output.status.success() {
                    let stderr_short = stderr.lines().next().unwrap_or("unknown error");
                    log::warn!("❌ Cookies validation failed for {}: {}", test_url, stderr);
                    return Err(format!("yt-dlp завершился с ошибкой: {}", stderr_short));
                }
            }
            Err(e) => {
                log::error!("Failed to execute yt-dlp for cookies validation: {}", e);
                return Err(format!("Не удалось запустить yt-dlp: {}", e));
            }
        }
    }

    log::debug!("✅ Cookies validation passed");
    Ok(())
}

/// Validates YouTube cookies (bool wrapper for backward compatibility)
pub async fn validate_cookies_ok() -> bool {
    validate_cookies().await.is_ok()
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
/// Returns `None` if cookies are valid, or `Some(reason)` with a human-readable failure reason.
pub async fn needs_refresh() -> Option<String> {
    validate_cookies().await.err()
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
