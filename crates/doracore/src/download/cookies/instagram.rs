//! Instagram-specific cookies management.

use anyhow::Result;
use std::path::PathBuf;

use crate::download::metadata::{get_proxy_chain, is_proxy_related_error};

use super::file_ops::COOKIES_WRITE_MUTEX;
use super::types::{CookieDetail, CookiesDiagnostic, ParsedCookie};

use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;

/// Required Instagram authentication cookies
const REQUIRED_IG_AUTH_COOKIES: &[&str] = &[
    "sessionid",  // Main session (REQUIRED)
    "csrftoken",  // CSRF token
    "ds_user_id", // User ID
];

/// Secondary Instagram cookies that help with access
const SECONDARY_IG_COOKIES: &[&str] = &[
    "mid",     // Machine ID
    "ig_did",  // Device ID
    "rur",     // Region hint
    "ig_nrcb", // Browser cookie
];

/// Returns the configured Instagram cookies file path from environment
pub fn get_ig_cookies_path() -> Option<PathBuf> {
    crate::core::config::INSTAGRAM_COOKIES_FILE.as_ref().map(PathBuf::from)
}

/// Parse Netscape cookie file and return cookies for a specific domain as HTTP header string.
///
/// Reads the file, filters cookies matching the domain (e.g., `.instagram.com`),
/// and returns a `Cookie:` header value like `sessionid=xxx; csrftoken=yyy`.
pub fn parse_cookies_for_domain(content: &str, domain: &str) -> Option<String> {
    let mut cookies = Vec::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 7 {
            let cookie_domain = parts[0];
            let name = parts[5];
            let value = parts[6];

            // Match domain: ".instagram.com" matches "instagram.com" and subdomains
            if cookie_domain.contains(domain) {
                cookies.push(format!("{}={}", name, value));
            }
        }
    }

    if cookies.is_empty() {
        None
    } else {
        Some(cookies.join("; "))
    }
}

/// Parse Instagram cookies file and return detailed diagnostics
pub fn diagnose_ig_cookies_content(content: &str) -> CookiesDiagnostic {
    let mut diagnostic = CookiesDiagnostic {
        file_exists: true,
        file_size: content.len() as u64,
        total_cookies: 0,
        youtube_cookies: 0, // reused field, counts IG cookies here
        auth_cookies_found: Vec::new(),
        auth_cookies_missing: Vec::new(),
        auth_cookies_expired: Vec::new(),
        secondary_cookies_found: Vec::new(),
        issues: Vec::new(),
        is_valid: false,
        cookie_details: Vec::new(),
        soonest_expiry_days: None,
        soonest_expiry_name: None,
    };

    let has_header = content.lines().any(|l| l.contains("Netscape HTTP Cookie File"));
    if !has_header {
        diagnostic
            .issues
            .push("Missing Netscape HTTP Cookie File header".to_string());
    }

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 7 {
            diagnostic.total_cookies += 1;

            let domain = parts[0].to_string();
            let secure = parts[3] == "TRUE";
            let expires: Option<i64> = parts[4].parse().ok();
            let name = parts[5].to_string();
            let value = parts[6].to_string();

            let cookie = ParsedCookie {
                domain: domain.clone(),
                name: name.clone(),
                value,
                expires,
                secure,
            };

            if domain.contains("instagram.com") {
                diagnostic.youtube_cookies += 1; // reused field for IG count

                let is_auth = REQUIRED_IG_AUTH_COOKIES.contains(&name.as_str());
                let is_secondary = SECONDARY_IG_COOKIES.contains(&name.as_str());

                if is_auth {
                    diagnostic.auth_cookies_found.push(name.clone());
                    if cookie.is_expired() {
                        diagnostic.auth_cookies_expired.push(name.clone());
                    }
                }

                if is_secondary {
                    diagnostic.secondary_cookies_found.push(name.clone());
                }

                if is_auth || is_secondary {
                    let detail = CookieDetail {
                        name: name.clone(),
                        masked_value: cookie.masked_value(),
                        expiration: cookie.expiration_info(),
                        expiration_date: cookie.expiration_date(),
                        days_until_expiry: cookie.days_until_expiry(),
                        is_expired: cookie.is_expired(),
                        is_critical: is_auth,
                    };

                    if is_auth {
                        if let Some(days) = detail.days_until_expiry {
                            match diagnostic.soonest_expiry_days {
                                None => {
                                    diagnostic.soonest_expiry_days = Some(days);
                                    diagnostic.soonest_expiry_name = Some(name.clone());
                                }
                                Some(current) if days < current => {
                                    diagnostic.soonest_expiry_days = Some(days);
                                    diagnostic.soonest_expiry_name = Some(name.clone());
                                }
                                _ => {}
                            }
                        }
                    }

                    diagnostic.cookie_details.push(detail);
                }
            }
        }
    }

    // Find missing required cookies
    for &required in REQUIRED_IG_AUTH_COOKIES {
        if !diagnostic.auth_cookies_found.iter().any(|n| n == required) {
            diagnostic.auth_cookies_missing.push(required.to_string());
        }
    }

    if diagnostic.youtube_cookies == 0 {
        diagnostic.issues.push("No Instagram cookies found".to_string());
    }

    if !diagnostic.auth_cookies_missing.is_empty() {
        diagnostic.issues.push(format!(
            "Missing required cookies: {}",
            diagnostic.auth_cookies_missing.join(", ")
        ));
    }

    if !diagnostic.auth_cookies_expired.is_empty() {
        diagnostic.issues.push(format!(
            "Expired cookies: {}",
            diagnostic.auth_cookies_expired.join(", ")
        ));
    }

    // sessionid is the most critical
    let has_sessionid = diagnostic.auth_cookies_found.iter().any(|n| n == "sessionid");

    diagnostic.is_valid = has_sessionid && diagnostic.auth_cookies_expired.is_empty() && diagnostic.youtube_cookies > 0;

    diagnostic
}

/// Updates the Instagram cookies file from content string
pub async fn update_ig_cookies_from_content(content: &str) -> Result<PathBuf> {
    let cookies_path = get_ig_cookies_path().ok_or_else(|| anyhow::anyhow!("INSTAGRAM_COOKIES_FILE not configured"))?;

    // Basic validation: check if it looks like Netscape cookies format with Instagram entries
    if !content.contains("# Netscape HTTP Cookie File") && !content.contains(".instagram.com") {
        return Err(anyhow::anyhow!(
            "Invalid cookies format. Expected Netscape HTTP Cookie File format with instagram.com entries"
        ));
    }

    let _lock = COOKIES_WRITE_MUTEX.lock().await;

    let temp_path = format!("{}.tmp.{}", cookies_path.display(), std::process::id());

    fs_err::tokio::write(&temp_path, content)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to write temp IG cookies file: {}", e))?;

    fs_err::tokio::rename(&temp_path, &cookies_path).await.map_err(|e| {
        let _ = fs_err::remove_file(&temp_path);
        anyhow::anyhow!("Failed to rename IG cookies file: {}", e)
    })?;

    log::info!("✅ Instagram cookies file updated atomically: {:?}", cookies_path);

    Ok(cookies_path)
}

/// Validates Instagram cookies by testing with yt-dlp
pub async fn validate_ig_cookies() -> anyhow::Result<()> {
    let cookies_path = match get_ig_cookies_path() {
        Some(path) => path,
        None => {
            anyhow::bail!("INSTAGRAM_COOKIES_FILE is not set");
        }
    };

    if !cookies_path.exists() {
        anyhow::bail!("Cookies file not found: {}", cookies_path.display());
    }

    match fs_err::metadata(&cookies_path) {
        Ok(meta) if meta.len() == 0 => {
            anyhow::bail!("Cookies file is empty (0 bytes)");
        }
        Err(e) => {
            anyhow::bail!("Failed to read cookies file: {}", e);
        }
        _ => {}
    }

    // Test with yt-dlp using a known Instagram reel
    let test_url = "https://www.instagram.com/reel/C1234567890/";
    let ytdl_bin = crate::core::config::YTDL_BIN.as_str();

    let proxy_chain = get_proxy_chain();
    let total_proxies = proxy_chain.len();
    let mut last_error: Option<String> = None;

    for (attempt, proxy_option) in proxy_chain.into_iter().enumerate() {
        let proxy_name = proxy_option
            .as_ref()
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "Direct".to_string());

        log::info!(
            "🍪 IG Cookies validation attempt {}/{} using [{}]",
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

        if let Some(ref proxy_config) = proxy_option {
            cmd.arg("--proxy").arg(&proxy_config.url);
        }

        cmd.arg("--cookies")
            .arg(&cookies_path)
            .arg("--print")
            .arg("%(id)s")
            .arg(test_url);

        let output = match timeout(Duration::from_secs(60), cmd.output()).await {
            Ok(result) => result,
            Err(_) => {
                last_error = Some("Validation timed out".to_string());
                continue;
            }
        };

        match output {
            Ok(output) => {
                if output.status.success() {
                    log::info!("✅ IG Cookies validation passed using [{}]", proxy_name);
                    return Ok(());
                }

                let stderr = String::from_utf8_lossy(&output.stderr);
                // For IG validation, any successful skip-download is enough
                // Some test URLs may not exist, so we accept 404 as "cookies work"
                if stderr.contains("404") || stderr.contains("not found") || stderr.contains("Unsupported URL") {
                    log::info!(
                        "✅ IG Cookies validation passed (test URL returned 404, cookies accepted) using [{}]",
                        proxy_name
                    );
                    return Ok(());
                }

                if stderr.contains("login") || stderr.contains("Login") || stderr.contains("authentication") {
                    anyhow::bail!("Instagram requires authentication — cookies are invalid");
                }

                if is_proxy_related_error(&stderr) {
                    last_error = Some(format!("Proxy error: {}", stderr.lines().next().unwrap_or("unknown")));
                    continue;
                }

                last_error = Some(stderr.lines().next().unwrap_or("unknown error").to_string());
            }
            Err(e) => {
                last_error = Some(format!("Failed to run yt-dlp: {}", e));
                continue;
            }
        }
    }

    Err(anyhow::anyhow!(
        last_error.unwrap_or_else(|| "IG Cookies validation failed".to_string())
    ))
}

/// Load Instagram cookie header string from the cookies file.
///
/// Returns `Some("sessionid=xxx; csrftoken=yyy; ...")` if cookies are available.
pub fn load_instagram_cookie_header() -> Option<String> {
    let cookies_path = get_ig_cookies_path()?;
    let content = fs_err::read_to_string(&cookies_path).ok()?;
    parse_cookies_for_domain(&content, "instagram.com")
}

/// Extract a specific cookie value by name from Netscape format file content.
pub fn extract_cookie_value_for_domain(content: &str, domain: &str, name: &str) -> Option<String> {
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 7 && parts[0].contains(domain) && parts[5] == name {
            return Some(parts[6].to_string());
        }
    }
    None
}

/// Get Instagram csrftoken from cookies file.
pub fn load_ig_csrf_token() -> Option<String> {
    let cookies_path = get_ig_cookies_path()?;
    let content = fs_err::read_to_string(&cookies_path).ok()?;
    extract_cookie_value_for_domain(&content, "instagram.com", "csrftoken")
}
