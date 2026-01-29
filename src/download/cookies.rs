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

use crate::download::metadata::{get_proxy_chain, is_proxy_related_error};

/// Validates YouTube cookies by testing video URLs that require authentication
///
/// Uses proxy chain (WARP → Residential → Direct) for validation to avoid
/// false negatives from datacenter IP blocks.
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

    // Test URL - use a simple video that requires auth
    let test_url = "https://www.youtube.com/watch?v=jNQXAC9IVRw"; // "Me at the zoo" - first YouTube video
    let ytdl_bin = crate::core::config::YTDL_BIN.as_str();

    // Get proxy chain and try each proxy
    let proxy_chain = get_proxy_chain();
    let total_proxies = proxy_chain.len();
    let mut last_error: Option<String> = None;

    for (attempt, proxy_option) in proxy_chain.into_iter().enumerate() {
        let proxy_name = proxy_option
            .as_ref()
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "Direct (no proxy)".to_string());

        log::info!(
            "🍪 Cookies validation attempt {}/{} using [{}]",
            attempt + 1,
            total_proxies,
            proxy_name
        );

        let mut cmd = Command::new(ytdl_bin);
        cmd.arg("--no-warnings")
            .arg("--no-playlist")
            .arg("--skip-download")
            .arg("--socket-timeout")
            .arg("30");

        // Add proxy if configured
        if let Some(ref proxy_config) = proxy_option {
            log::debug!("Using proxy: {}", proxy_config.masked_url());
            cmd.arg("--proxy").arg(&proxy_config.url);
        }

        cmd.arg("--cookies")
            .arg(&cookies_path)
            // PO Token provider for YouTube bot detection bypass
            .arg("--extractor-args")
            .arg("youtubepot-bgutilhttp:base_url=http://127.0.0.1:4416")
            .arg("--js-runtimes")
            .arg("node")
            .arg("--print")
            .arg("%(id)s %(title)s")
            .arg(test_url);

        let output = cmd.output().await;

        match output {
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);

                // Check for actual cookies problems (not proxy-related)
                if stderr.contains("Cookie") && stderr.contains("invalid") {
                    log::error!("🔴 Cookies validation failed: {}", stderr);
                    return Err("Ошибка чтения cookies — файл повреждён или имеет неверный формат".to_string());
                }

                if stderr.contains("login") && stderr.contains("required") {
                    log::error!("🔴 Cookies validation failed: {}", stderr);
                    return Err("YouTube требует повторный вход — сессия истекла".to_string());
                }

                // Check for proxy-related errors that should trigger fallback
                if is_proxy_related_error(&stderr) {
                    log::warn!(
                        "🔄 Proxy-related error with [{}], trying next proxy: {}",
                        proxy_name,
                        stderr.lines().next().unwrap_or("unknown")
                    );
                    last_error = Some(format!("Proxy error: {}", stderr.lines().next().unwrap_or("unknown")));
                    continue;
                }

                if output.status.success() {
                    log::info!("✅ Cookies validation passed using [{}]", proxy_name);
                    return Ok(());
                }

                // Non-proxy error - might still be worth trying next proxy
                let stderr_short = stderr.lines().next().unwrap_or("unknown error");
                log::warn!("❌ Cookies validation failed with [{}]: {}", proxy_name, stderr_short);
                last_error = Some(stderr_short.to_string());

                // If it's a definitive cookies error, don't try other proxies
                if stderr.contains("Sign in to confirm") || stderr.contains("not a bot") {
                    // This might be a proxy issue, try next
                    continue;
                }
            }
            Err(e) => {
                log::error!("Failed to execute yt-dlp with [{}]: {}", proxy_name, e);
                last_error = Some(format!("Не удалось запустить yt-dlp: {}", e));
                continue;
            }
        }
    }

    // All proxies failed
    log::error!("❌ Cookies validation failed with all {} proxies", total_proxies);
    Err(last_error.unwrap_or_else(|| "Cookies validation failed".to_string()))
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
