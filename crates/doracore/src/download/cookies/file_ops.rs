//! File I/O and atomic cookie-file updates.

use anyhow::Result;
use base64::{engine::general_purpose, Engine as _};
use std::path::PathBuf;
use std::sync::LazyLock;
use tokio::sync::Mutex;

use super::types::{diagnose_cookies_content, CookiesDiagnostic};

/// Mutex to prevent concurrent cookie file writes (race condition protection)
pub(super) static COOKIES_WRITE_MUTEX: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// Returns the configured cookies file path from environment
pub(super) fn get_cookies_path() -> Option<PathBuf> {
    crate::core::config::YTDL_COOKIES_FILE.as_ref().map(PathBuf::from)
}

/// Log detailed cookie file diagnostics for debugging cookie invalidation.
///
/// Call this at key moments (before/after Tier 2 attempts, on cookie errors)
/// to get a full snapshot of cookie file state in the logs.
///
/// # Arguments
/// * `context` - Human-readable label for when/why this diagnostic was taken
///   (e.g., "TIER2_BEFORE_ATTEMPT", "TIER2_AFTER_FAILURE", "COOKIE_REFRESH_TRIGGERED")
pub fn log_cookie_file_diagnostics(context: &str) {
    let cookies_path = match get_cookies_path() {
        Some(path) => path,
        None => {
            log::warn!(
                "[COOKIE_DIAG:{}] No cookies file configured (YTDL_COOKIES_FILE not set)",
                context
            );
            return;
        }
    };

    // File metadata
    let metadata = match fs_err::metadata(&cookies_path) {
        Ok(m) => m,
        Err(e) => {
            log::warn!(
                "[COOKIE_DIAG:{}] Cannot read cookie file {:?}: {}",
                context,
                cookies_path,
                e
            );
            return;
        }
    };

    let file_size = metadata.len();
    let file_age_secs = metadata
        .modified()
        .ok()
        .and_then(|t| t.elapsed().ok())
        .map(|d| d.as_secs());

    let age_str = match file_age_secs {
        Some(s) if s < 60 => format!("{}s ago", s),
        Some(s) if s < 3600 => format!("{}m {}s ago", s / 60, s % 60),
        Some(s) => format!("{}h {}m ago", s / 3600, (s % 3600) / 60),
        None => "unknown age".to_string(),
    };

    // Read and parse content for diagnostics
    match fs_err::read_to_string(&cookies_path) {
        Ok(content) => {
            let diag = diagnose_cookies_content(&content);
            log::warn!(
                "[COOKIE_DIAG:{}] file={:?} size={}B age={} | cookies: total={} yt={} auth_found=[{}] auth_missing=[{}] auth_expired=[{}] secondary=[{}] valid={} | soonest_expiry={}",
                context,
                cookies_path,
                file_size,
                age_str,
                diag.total_cookies,
                diag.youtube_cookies,
                diag.auth_cookies_found.join(","),
                diag.auth_cookies_missing.join(","),
                diag.auth_cookies_expired.join(","),
                diag.secondary_cookies_found.join(","),
                diag.is_valid,
                diag.soonest_expiry_days
                    .map(|d| format!("{}d ({})", d, diag.soonest_expiry_name.as_deref().unwrap_or("?")))
                    .unwrap_or_else(|| "N/A".to_string()),
            );

            if !diag.issues.is_empty() {
                log::warn!("[COOKIE_DIAG:{}] Issues: {}", context, diag.issues.join("; "));
            }
        }
        Err(e) => {
            log::warn!(
                "[COOKIE_DIAG:{}] file={:?} size={}B age={} | CANNOT READ: {}",
                context,
                cookies_path,
                file_size,
                age_str,
                e
            );
        }
    }
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

    // Acquire lock to prevent concurrent writes (race condition protection)
    let _lock = COOKIES_WRITE_MUTEX.lock().await;

    // Atomic write: write to temp file, then rename
    // This prevents file corruption if process is killed mid-write
    let temp_path = format!("{}.tmp.{}", cookies_path.display(), std::process::id());

    fs_err::tokio::write(&temp_path, &cookies_content)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to write temp cookies file: {}", e))?;

    fs_err::tokio::rename(&temp_path, &cookies_path).await.map_err(|e| {
        // Clean up temp file on rename failure
        let _ = fs_err::remove_file(&temp_path);
        anyhow::anyhow!("Failed to rename cookies file: {}", e)
    })?;

    log::info!("✅ Cookies file updated atomically: {:?}", cookies_path);

    Ok(cookies_path)
}

pub async fn update_cookies_from_content(content: &str) -> Result<PathBuf> {
    let cookies_path = get_cookies_path().ok_or_else(|| anyhow::anyhow!("YTDL_COOKIES_FILE not configured"))?;

    // Basic validation: check if it looks like Netscape cookies format
    if !content.contains("# Netscape HTTP Cookie File") && !content.contains(".youtube.com") {
        return Err(anyhow::anyhow!(
            "Invalid cookies format. Expected Netscape HTTP Cookie File format with youtube.com entries"
        ));
    }

    // Acquire lock to prevent concurrent writes (race condition protection)
    let _lock = COOKIES_WRITE_MUTEX.lock().await;

    // Atomic write: write to temp file, then rename
    // This prevents file corruption if process is killed mid-write
    let temp_path = format!("{}.tmp.{}", cookies_path.display(), std::process::id());

    fs_err::tokio::write(&temp_path, content)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to write temp cookies file: {}", e))?;

    fs_err::tokio::rename(&temp_path, &cookies_path).await.map_err(|e| {
        // Clean up temp file on rename failure
        let _ = fs_err::remove_file(&temp_path);
        anyhow::anyhow!("Failed to rename cookies file: {}", e)
    })?;

    log::info!("✅ Cookies file updated atomically from content: {:?}", cookies_path);

    Ok(cookies_path)
}

/// Diagnose cookies from file path
pub async fn diagnose_cookies_file() -> CookiesDiagnostic {
    let cookies_path = match get_cookies_path() {
        Some(path) => path,
        None => {
            return CookiesDiagnostic {
                file_exists: false,
                file_size: 0,
                total_cookies: 0,
                youtube_cookies: 0,
                auth_cookies_found: Vec::new(),
                auth_cookies_missing: Vec::new(),
                auth_cookies_expired: Vec::new(),
                secondary_cookies_found: Vec::new(),
                issues: vec!["YTDL_COOKIES_FILE is not configured".to_string()],
                is_valid: false,
                cookie_details: Vec::new(),
                soonest_expiry_days: None,
                soonest_expiry_name: None,
            };
        }
    };

    if !cookies_path.exists() {
        return CookiesDiagnostic {
            file_exists: false,
            file_size: 0,
            total_cookies: 0,
            youtube_cookies: 0,
            auth_cookies_found: Vec::new(),
            auth_cookies_missing: Vec::new(),
            auth_cookies_expired: Vec::new(),
            secondary_cookies_found: Vec::new(),
            issues: vec![format!("File not found: {}", cookies_path.display())],
            is_valid: false,
            cookie_details: Vec::new(),
            soonest_expiry_days: None,
            soonest_expiry_name: None,
        };
    }

    match fs_err::tokio::read_to_string(&cookies_path).await {
        Ok(content) => diagnose_cookies_content(&content),
        Err(e) => CookiesDiagnostic {
            file_exists: true,
            file_size: 0,
            total_cookies: 0,
            youtube_cookies: 0,
            auth_cookies_found: Vec::new(),
            auth_cookies_missing: Vec::new(),
            auth_cookies_expired: Vec::new(),
            secondary_cookies_found: Vec::new(),
            issues: vec![format!("Error reading file: {}", e)],
            is_valid: false,
            cookie_details: Vec::new(),
            soonest_expiry_days: None,
            soonest_expiry_name: None,
        },
    }
}
