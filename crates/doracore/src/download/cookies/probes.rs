//! Validation probes — yt-dlp-based health checks for YouTube cookies.

use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;

use crate::download::metadata::{get_proxy_chain, is_proxy_related_error};

use super::file_ops::get_cookies_path;
use super::types::{CookieInvalidReason, CookieValidationResult};

/// Validates YouTube cookies by testing video URLs that require authentication
///
/// Uses proxy chain (WARP → Residential → Direct) for validation to avoid
/// false negatives from datacenter IP blocks.
///
/// Returns `Ok(())` if cookies are valid, or `Err(reason)` with a human-readable failure reason.
pub async fn validate_cookies() -> anyhow::Result<()> {
    let cookies_path = match get_cookies_path() {
        Some(path) => path,
        None => {
            log::warn!("No cookies file configured (YTDL_COOKIES_FILE not set)");
            anyhow::bail!("YTDL_COOKIES_FILE is not set — cookies path is not configured");
        }
    };

    if !cookies_path.exists() {
        log::warn!("Cookies file does not exist: {:?}", cookies_path);
        anyhow::bail!("Cookies file not found: {}", cookies_path.display());
    }

    // Check file is not empty
    match fs_err::metadata(&cookies_path) {
        Ok(meta) if meta.len() == 0 => {
            anyhow::bail!("Cookies file is empty (0 bytes)");
        }
        Err(e) => {
            anyhow::bail!("Failed to read cookies file: {}", e);
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
            .arg("120");

        // Add proxy if configured
        if let Some(ref proxy_config) = proxy_option {
            log::debug!("Using proxy: {}", proxy_config.masked_url());
            cmd.arg("--proxy").arg(&proxy_config.url);
        }

        cmd.arg("--cookies")
            .arg(&cookies_path)
            // Use web_music client (best for premium formats with cookies)
            .arg("--extractor-args")
            .arg("youtube:player_client=android_vr,web_safari;formats=missing_pot")
            .arg("--js-runtimes")
            .arg("deno")
            .arg("--print")
            .arg("%(id)s %(title)s")
            .arg(test_url);

        // Use 60 second timeout for validation (shorter than download timeout)
        let output = match timeout(Duration::from_secs(180), cmd.output()).await {
            Ok(result) => result,
            Err(_) => {
                log::warn!(
                    "🔄 Cookies validation timed out with [{}], trying next proxy",
                    proxy_name
                );
                last_error = Some("Validation timed out".to_string());
                continue;
            }
        };

        match output {
            Ok(output) => {
                if output.status.success() {
                    log::info!("✅ Cookies validation passed using [{}]", proxy_name);
                    return Ok(());
                }

                let stderr = String::from_utf8_lossy(&output.stderr);
                let reason = CookieInvalidReason::from_ytdlp_error(&stderr);

                // Critical cookie problems: stop immediately (proxy won't help)
                if reason.is_critical() || matches!(reason, CookieInvalidReason::FileCorrupted) {
                    log::error!("🔴 Cookies validation failed: {}", stderr);
                    anyhow::bail!("{}", reason.description());
                }

                // Check for proxy-related errors that should trigger fallback
                if reason.is_proxy_related() || is_proxy_related_error(&stderr) {
                    log::warn!(
                        "🔄 Proxy-related error with [{}], trying next proxy: {}",
                        proxy_name,
                        stderr.lines().next().unwrap_or("unknown")
                    );
                    last_error = Some(format!("Proxy error: {}", stderr.lines().next().unwrap_or("unknown")));
                    continue;
                }

                // Non-proxy error - might still be worth trying next proxy
                let stderr_short = stderr.lines().next().unwrap_or("unknown error");
                log::warn!("❌ Cookies validation failed with [{}]: {}", proxy_name, stderr_short);
                last_error = Some(stderr_short.to_string());
            }
            Err(e) => {
                log::error!("Failed to execute yt-dlp with [{}]: {}", proxy_name, e);
                last_error = Some(format!("Failed to run yt-dlp: {}", e));
                continue;
            }
        }
    }

    // All proxies failed
    log::error!("❌ Cookies validation failed with all {} proxies", total_proxies);
    Err(anyhow::anyhow!(
        last_error.unwrap_or_else(|| "Cookies validation failed".to_string())
    ))
}

/// Validates YouTube cookies (bool wrapper for backward compatibility)
pub async fn validate_cookies_ok() -> bool {
    validate_cookies().await.is_ok()
}

/// Validates that cookies carry **age-verified** authentication.
///
/// YouTube gates some videos behind "Sign in to confirm your age". Regular
/// `validate_cookies` probes a non-age-gated video ("Me at the zoo") — that
/// passes for any logged-in account, even one without a verified DOB, so it
/// cannot detect the loss of age-verification state (which invalidates
/// 18+ video downloads without otherwise affecting the session).
///
/// This probe downloads metadata for an age-restricted classic (Rammstein
/// "Sonne") through the full proxy chain. `Ok(())` means age-gated content
/// is accessible; an `Err` typically means the cookies were re-exported from
/// a browser session that never completed the age-verification step.
pub async fn validate_age_gated_cookies() -> anyhow::Result<()> {
    let cookies_path = match get_cookies_path() {
        Some(path) => path,
        None => {
            anyhow::bail!("YTDL_COOKIES_FILE is not set — cookies path is not configured");
        }
    };

    if !cookies_path.exists() {
        anyhow::bail!("Cookies file not found: {}", cookies_path.display());
    }

    // Age-restricted probe: Rammstein "Sonne" — long-standing 18+ gate on YouTube.
    let test_url = "https://www.youtube.com/watch?v=PmAI3GvuRkA";
    let ytdl_bin = crate::core::config::YTDL_BIN.as_str();

    let proxy_chain = get_proxy_chain();
    let total_proxies = proxy_chain.len();
    let mut last_error: Option<String> = None;

    for (attempt, proxy_option) in proxy_chain.into_iter().enumerate() {
        let proxy_name = proxy_option
            .as_ref()
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "Direct (no proxy)".to_string());

        log::debug!(
            "🔞 Age-gated probe attempt {}/{} using [{}]",
            attempt + 1,
            total_proxies,
            proxy_name
        );

        let mut cmd = Command::new(ytdl_bin);
        cmd.arg("--no-warnings")
            .arg("--no-playlist")
            .arg("--skip-download")
            .arg("--socket-timeout")
            .arg("120");

        if let Some(ref proxy_config) = proxy_option {
            cmd.arg("--proxy").arg(&proxy_config.url);
        }

        cmd.arg("--cookies")
            .arg(&cookies_path)
            .arg("--extractor-args")
            .arg("youtube:player_client=android_vr,web_safari;formats=missing_pot")
            .arg("--js-runtimes")
            .arg("deno")
            .arg("--print")
            .arg("%(id)s %(title)s")
            .arg(test_url);

        let output = match timeout(Duration::from_secs(180), cmd.output()).await {
            Ok(result) => result,
            Err(_) => {
                last_error = Some("Age-gated probe timed out".to_string());
                continue;
            }
        };

        match output {
            Ok(output) => {
                if output.status.success() {
                    log::debug!("✅ Age-gated probe passed using [{}]", proxy_name);
                    return Ok(());
                }

                let stderr = String::from_utf8_lossy(&output.stderr);
                let stderr_lower = stderr.to_lowercase();

                // Age-gate: cookies logged-in but not age-verified. Proxy won't help.
                if stderr_lower.contains("sign in to confirm your age")
                    || stderr_lower.contains("inappropriate for some users")
                {
                    anyhow::bail!("Cookies are not age-verified (YouTube requires age confirmation)");
                }

                // Proxy-level errors: try the next tier.
                if is_proxy_related_error(&stderr) {
                    last_error = Some(format!("Proxy error: {}", stderr.lines().next().unwrap_or("unknown")));
                    continue;
                }

                let stderr_short = stderr.lines().next().unwrap_or("unknown error");
                last_error = Some(stderr_short.to_string());
            }
            Err(e) => {
                last_error = Some(format!("Failed to run yt-dlp: {}", e));
                continue;
            }
        }
    }

    Err(anyhow::anyhow!(
        last_error.unwrap_or_else(|| "Age-gated probe failed on every proxy".to_string())
    ))
}

/// Bool wrapper for `validate_age_gated_cookies` (parallels `validate_cookies_ok`).
pub async fn validate_age_gated_cookies_ok() -> bool {
    validate_age_gated_cookies().await.is_ok()
}

/// Detailed validation that returns structured result with reason
pub async fn validate_cookies_detailed() -> CookieValidationResult {
    let cookies_path = match get_cookies_path() {
        Some(path) => path,
        None => {
            return CookieValidationResult {
                is_valid: false,
                reason: Some(CookieInvalidReason::FileNotFound),
                proxy_used: None,
                raw_error: Some("YTDL_COOKIES_FILE not set".to_string()),
            };
        }
    };

    if !cookies_path.exists() {
        return CookieValidationResult {
            is_valid: false,
            reason: Some(CookieInvalidReason::FileNotFound),
            proxy_used: None,
            raw_error: Some(format!("File not found: {}", cookies_path.display())),
        };
    }

    if let Ok(meta) = fs_err::metadata(&cookies_path) {
        if meta.len() == 0 {
            return CookieValidationResult {
                is_valid: false,
                reason: Some(CookieInvalidReason::FileEmpty),
                proxy_used: None,
                raw_error: None,
            };
        }
    }

    let test_url = "https://www.youtube.com/watch?v=jNQXAC9IVRw";
    let ytdl_bin = crate::core::config::YTDL_BIN.as_str();
    let proxy_chain = get_proxy_chain();
    let mut last_reason: Option<CookieInvalidReason> = None;
    let mut last_error: Option<String> = None;

    for proxy_option in proxy_chain {
        let proxy_name = proxy_option
            .as_ref()
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "Direct".to_string());

        let mut cmd = Command::new(ytdl_bin);
        cmd.arg("--no-warnings")
            .arg("--no-playlist")
            .arg("--skip-download")
            .arg("--socket-timeout")
            .arg("120");

        if let Some(ref proxy_config) = proxy_option {
            cmd.arg("--proxy").arg(&proxy_config.url);
        }

        cmd.arg("--cookies")
            .arg(&cookies_path)
            // Use web_music client (best for premium formats with cookies)
            .arg("--extractor-args")
            .arg("youtube:player_client=android_vr,web_safari;formats=missing_pot")
            .arg("--js-runtimes")
            .arg("deno")
            .arg("--print")
            .arg("%(id)s %(title)s")
            .arg(test_url);

        let output = match timeout(Duration::from_secs(180), cmd.output()).await {
            Ok(Ok(output)) => output,
            Ok(Err(e)) => {
                last_error = Some(e.to_string());
                continue;
            }
            Err(_) => {
                last_error = Some("Timeout".to_string());
                continue;
            }
        };

        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if output.status.success() {
            return CookieValidationResult {
                is_valid: true,
                reason: None,
                proxy_used: Some(proxy_name),
                raw_error: None,
            };
        }

        // Parse the reason
        let reason = CookieInvalidReason::from_ytdlp_error(&stderr);

        // If it's a critical cookie error (not proxy-related), return immediately
        if reason.is_critical() {
            return CookieValidationResult {
                is_valid: false,
                reason: Some(reason),
                proxy_used: Some(proxy_name),
                raw_error: Some(stderr),
            };
        }

        last_reason = Some(reason);
        last_error = Some(stderr);
    }

    CookieValidationResult {
        is_valid: false,
        reason: Some(last_reason.unwrap_or_else(|| {
            CookieInvalidReason::AllProxiesFailed(last_error.clone().unwrap_or_else(|| "Unknown".to_string()))
        })),
        proxy_used: None,
        raw_error: last_error,
    }
}

/// Checks if cookies need refresh by validating them
///
/// Returns `None` if cookies are valid, or `Some(reason)` with a human-readable failure reason.
pub async fn needs_refresh() -> Option<String> {
    validate_cookies().await.err().map(|e| e.to_string())
}
