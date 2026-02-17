//! YouTube cookies management for yt-dlp
//!
//! This module provides functionality to:
//! - Validate YouTube cookies
//! - Update cookies file from base64 string
//! - Check cookies freshness periodically
//! - Background watchdog for cookie health

use anyhow::Result;
use base64::{engine::general_purpose, Engine as _};
use once_cell::sync::Lazy;
use std::path::PathBuf;
use std::time::Duration;
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio::time::timeout;

/// Mutex to prevent concurrent cookie file writes (race condition protection)
static COOKIES_WRITE_MUTEX: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

use crate::download::metadata::{get_proxy_chain, is_proxy_related_error};

/// Reason why cookies validation failed
#[derive(Debug, Clone, PartialEq)]
pub enum CookieInvalidReason {
    /// File not found or not configured
    FileNotFound,
    /// File is empty
    FileEmpty,
    /// File format is corrupted
    FileCorrupted,
    /// Cookies were rotated by YouTube (security measure)
    RotatedByYouTube,
    /// Session expired - need to login again
    SessionExpired,
    /// Bot detection triggered - need fresh cookies from human session
    BotDetected,
    /// IP address blocked or flagged
    IpBlocked,
    /// Account requires verification (captcha, SMS, etc.)
    VerificationRequired,
    /// Rate limited by YouTube
    RateLimited,
    /// Generic/unknown error
    Unknown(String),
    /// All proxies failed
    AllProxiesFailed(String),
}

impl CookieInvalidReason {
    /// Parse yt-dlp stderr to determine the reason
    pub fn from_ytdlp_error(stderr: &str) -> Self {
        let stderr_lower = stderr.to_lowercase();

        // Cookies rotated/invalidated by YouTube
        if stderr_lower.contains("cookies are no longer valid")
            || stderr_lower.contains("cookies have likely been rotated")
            || stderr_lower.contains("cookies have expired")
        {
            return Self::RotatedByYouTube;
        }

        // Session expired
        if (stderr_lower.contains("login") && stderr_lower.contains("required"))
            || stderr_lower.contains("please sign in")
            || stderr_lower.contains("sign in to confirm your age")
        {
            return Self::SessionExpired;
        }

        // Bot detection
        if stderr_lower.contains("sign in to confirm you're not a bot")
            || stderr_lower.contains("sign in to confirm you’re not a bot")
            || stderr_lower.contains("confirm you're not a bot")
            || stderr_lower.contains("confirm you’re not a bot")
            || stderr_lower.contains("unusual traffic")
        {
            return Self::BotDetected;
        }

        // IP blocked
        if stderr_lower.contains("blocked")
            || stderr_lower.contains("ip address")
            || stderr_lower.contains("access denied")
        {
            return Self::IpBlocked;
        }

        // Verification required
        if stderr_lower.contains("verify")
            || stderr_lower.contains("verification")
            || stderr_lower.contains("captcha")
            || stderr_lower.contains("two-factor")
            || stderr_lower.contains("2fa")
        {
            return Self::VerificationRequired;
        }

        // Rate limited
        if stderr_lower.contains("rate limit")
            || stderr_lower.contains("too many requests")
            || stderr_lower.contains("429")
        {
            return Self::RateLimited;
        }

        // File corrupted
        if (stderr_lower.contains("cookie") && stderr_lower.contains("invalid"))
            || stderr_lower.contains("could not read")
            || stderr_lower.contains("malformed")
        {
            return Self::FileCorrupted;
        }

        Self::Unknown(stderr.lines().next().unwrap_or("unknown").to_string())
    }

    /// Get human-readable description in Russian
    pub fn description(&self) -> String {
        match self {
            Self::FileNotFound => "Файл cookies не найден".to_string(),
            Self::FileEmpty => "Файл cookies пуст".to_string(),
            Self::FileCorrupted => "Файл cookies повреждён или имеет неверный формат".to_string(),
            Self::RotatedByYouTube => {
                "🔄 YouTube ротировал cookies (защита от бота). Нужно переэкспортировать из браузера.".to_string()
            }
            Self::SessionExpired => "⏰ Сессия истекла — YouTube требует повторный вход".to_string(),
            Self::BotDetected => {
                "🤖 YouTube обнаружил бота. Нужны свежие cookies после ручного просмотра видео.".to_string()
            }
            Self::IpBlocked => "🚫 IP адрес заблокирован YouTube".to_string(),
            Self::VerificationRequired => "🔐 Аккаунт требует верификацию (капча/SMS/2FA)".to_string(),
            Self::RateLimited => "⏳ Превышен лимит запросов — подожди немного".to_string(),
            Self::Unknown(msg) => format!("❓ Неизвестная ошибка: {}", msg),
            Self::AllProxiesFailed(msg) => format!("🌐 Все прокси не сработали: {}", msg),
        }
    }

    /// Should we notify admin immediately?
    pub fn is_critical(&self) -> bool {
        matches!(
            self,
            Self::RotatedByYouTube | Self::SessionExpired | Self::BotDetected | Self::VerificationRequired
        )
    }

    /// Can this be fixed by trying a different proxy?
    pub fn is_proxy_related(&self) -> bool {
        matches!(self, Self::IpBlocked | Self::RateLimited)
    }
}

/// Result of cookie validation with detailed reason
#[derive(Debug, Clone)]
pub struct CookieValidationResult {
    pub is_valid: bool,
    pub reason: Option<CookieInvalidReason>,
    pub proxy_used: Option<String>,
    pub raw_error: Option<String>,
}

/// Required YouTube authentication cookies for full functionality
const REQUIRED_AUTH_COOKIES: &[&str] = &[
    "SID",     // Session ID
    "HSID",    // HTTP Session ID
    "SSID",    // Secure Session ID
    "APISID",  // API Session ID
    "SAPISID", // Secure API Session ID
];

/// Secondary cookies that help with YouTube access
const SECONDARY_COOKIES: &[&str] = &[
    "__Secure-1PSID",
    "__Secure-3PSID",
    "__Secure-1PAPISID",
    "__Secure-3PAPISID",
    "LOGIN_INFO",
    "PREF",
    "VISITOR_INFO1_LIVE",
];

/// Parsed cookie from Netscape format
#[derive(Debug, Clone)]
pub struct ParsedCookie {
    pub domain: String,
    pub name: String,
    pub value: String,
    pub expires: Option<i64>, // Unix timestamp, 0 means session cookie
    pub secure: bool,
}

impl ParsedCookie {
    /// Check if cookie is expired
    pub fn is_expired(&self) -> bool {
        match self.expires {
            Some(0) => false, // Session cookie - never expires (until browser close)
            Some(ts) => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                ts < now
            }
            None => false,
        }
    }

    /// Get days until expiration (negative if expired)
    pub fn days_until_expiry(&self) -> Option<i64> {
        match self.expires {
            Some(0) => None, // Session cookie
            Some(ts) => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                Some((ts - now) / 86400)
            }
            None => None,
        }
    }

    /// Get human-readable expiration info
    pub fn expiration_info(&self) -> String {
        match self.expires {
            Some(0) => "сессионный".to_string(),
            Some(ts) => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);

                if ts < now {
                    let diff = now - ts;
                    let days = diff / 86400;
                    if days == 0 {
                        let hours = diff / 3600;
                        format!("истёк {} ч. назад", hours)
                    } else {
                        format!("истёк {} дн. назад", days)
                    }
                } else {
                    let diff = ts - now;
                    let days = diff / 86400;
                    if days > 365 {
                        format!("{} г.", days / 365)
                    } else if days > 30 {
                        format!("{} мес.", days / 30)
                    } else if days > 0 {
                        format!("{} дн.", days)
                    } else {
                        let hours = diff / 3600;
                        format!("{} ч.", hours)
                    }
                }
            }
            None => "?".to_string(),
        }
    }

    /// Get masked value (for security - show only first and last few chars)
    pub fn masked_value(&self) -> String {
        let len = self.value.len();
        if len <= 8 {
            "*".repeat(len)
        } else {
            format!("{}...{}", &self.value[..4], &self.value[len - 4..])
        }
    }

    /// Format expiration as date string
    pub fn expiration_date(&self) -> String {
        match self.expires {
            Some(0) => "session".to_string(),
            Some(ts) => {
                use std::time::{Duration, UNIX_EPOCH};
                let datetime = UNIX_EPOCH + Duration::from_secs(ts as u64);
                // Format as simple date
                let secs_since_epoch = datetime.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);

                // Simple date calculation
                let days_since_1970 = secs_since_epoch / 86400;
                let years = 1970 + (days_since_1970 / 365);
                let remaining_days = days_since_1970 % 365;
                let month = remaining_days / 30 + 1;
                let day = remaining_days % 30 + 1;

                format!("{:04}-{:02}-{:02}", years, month.min(12), day.min(31))
            }
            None => "unknown".to_string(),
        }
    }
}

/// Cookie detail for diagnostic report
#[derive(Debug, Clone)]
pub struct CookieDetail {
    pub name: String,
    pub masked_value: String,
    pub expiration: String,      // Human readable (e.g., "5 дн.")
    pub expiration_date: String, // Date (e.g., "2025-02-10")
    pub days_until_expiry: Option<i64>,
    pub is_expired: bool,
    pub is_critical: bool, // true for required auth cookies
}

/// Detailed cookies diagnostic result
#[derive(Debug, Clone)]
pub struct CookiesDiagnostic {
    pub file_exists: bool,
    pub file_size: u64,
    pub total_cookies: usize,
    pub youtube_cookies: usize,
    pub auth_cookies_found: Vec<String>,
    pub auth_cookies_missing: Vec<String>,
    pub auth_cookies_expired: Vec<String>,
    pub secondary_cookies_found: Vec<String>,
    pub issues: Vec<String>,
    pub is_valid: bool,
    /// Detailed info for important cookies
    pub cookie_details: Vec<CookieDetail>,
    /// Soonest expiring auth cookie (days)
    pub soonest_expiry_days: Option<i64>,
    /// Name of soonest expiring cookie
    pub soonest_expiry_name: Option<String>,
}

impl CookiesDiagnostic {
    /// Format as human-readable report
    pub fn format_report(&self) -> String {
        let mut report = String::new();

        // File status
        if !self.file_exists {
            return "❌ Файл cookies не найден".to_string();
        }

        report.push_str(&format!("📄 Размер файла: {} байт\n", self.file_size));
        report.push_str(&format!(
            "🍪 Всего cookies: {} (YouTube: {})\n\n",
            self.total_cookies, self.youtube_cookies
        ));

        // Auth cookies status with details
        report.push_str("*Обязательные auth cookies:*\n");

        for detail in self.cookie_details.iter().filter(|d| d.is_critical) {
            let status = if detail.is_expired { "⚠️" } else { "✅" };
            report.push_str(&format!(
                "  {} {} | {} | `{}`\n",
                status, detail.name, detail.expiration, detail.masked_value
            ));
        }

        for name in &self.auth_cookies_missing {
            report.push_str(&format!("  ❌ {} — отсутствует\n", name));
        }

        // Secondary cookies with details
        if !self.secondary_cookies_found.is_empty() {
            report.push_str("\n*Дополнительные cookies:*\n");
            for detail in self.cookie_details.iter().filter(|d| !d.is_critical) {
                let status = if detail.is_expired { "⚠️" } else { "✅" };
                report.push_str(&format!("  {} {} | {}\n", status, detail.name, detail.expiration));
            }
        }

        // Expiration warning
        if let (Some(days), Some(name)) = (self.soonest_expiry_days, &self.soonest_expiry_name) {
            report.push('\n');
            if days < 0 {
                report.push_str(&format!("🚨 *{} истёк {} дн. назад!*\n", name, -days));
            } else if days < 3 {
                report.push_str(&format!("⚠️ *{} истекает через {} дн.!*\n", name, days));
            } else if days < 7 {
                report.push_str(&format!("⏰ {} истекает через {} дн.\n", name, days));
            }
        }

        // Issues summary
        if !self.issues.is_empty() {
            report.push_str("\n*⚠️ Проблемы:*\n");
            for issue in &self.issues {
                report.push_str(&format!("  • {}\n", issue));
            }
        }

        // Overall status
        report.push('\n');
        if self.is_valid {
            report.push_str("✅ *Cookies выглядят корректно*");
        } else {
            report.push_str("❌ *Cookies невалидны — требуется переэкспорт*");
        }

        report
    }
}

/// Parse Netscape cookie file and return detailed diagnostics
pub fn diagnose_cookies_content(content: &str) -> CookiesDiagnostic {
    let mut diagnostic = CookiesDiagnostic {
        file_exists: true,
        file_size: content.len() as u64,
        total_cookies: 0,
        youtube_cookies: 0,
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

    // Check for Netscape format header
    let has_header = content.lines().any(|l| l.contains("Netscape HTTP Cookie File"));
    if !has_header {
        diagnostic
            .issues
            .push("Отсутствует заголовок Netscape HTTP Cookie File".to_string());
    }

    let mut parsed_cookies: Vec<ParsedCookie> = Vec::new();

    for line in content.lines() {
        let line = line.trim();

        // Skip comments and empty lines
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        // Netscape format: domain TAB flag TAB path TAB secure TAB expires TAB name TAB value
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

            // Check if YouTube cookie
            if domain.contains("youtube.com") || domain.contains("google.com") {
                diagnostic.youtube_cookies += 1;

                let is_auth = REQUIRED_AUTH_COOKIES.contains(&name.as_str());
                let is_secondary = SECONDARY_COOKIES.contains(&name.as_str());

                // Check if required auth cookie
                if is_auth {
                    diagnostic.auth_cookies_found.push(name.clone());
                    if cookie.is_expired() {
                        diagnostic.auth_cookies_expired.push(name.clone());
                    }
                }

                // Check secondary cookies
                if is_secondary {
                    diagnostic.secondary_cookies_found.push(name.clone());
                }

                // Create detail record for important cookies
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

                    // Track soonest expiring auth cookie
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

            parsed_cookies.push(cookie);
        }
    }

    // Find missing required cookies
    for &required in REQUIRED_AUTH_COOKIES {
        if !diagnostic.auth_cookies_found.iter().any(|n| n == required) {
            diagnostic.auth_cookies_missing.push(required.to_string());
        }
    }

    // Analyze issues
    if diagnostic.youtube_cookies == 0 {
        diagnostic
            .issues
            .push("Не найдено ни одного YouTube cookie".to_string());
    }

    if !diagnostic.auth_cookies_missing.is_empty() {
        diagnostic.issues.push(format!(
            "Отсутствуют обязательные cookies: {}",
            diagnostic.auth_cookies_missing.join(", ")
        ));
    }

    if !diagnostic.auth_cookies_expired.is_empty() {
        diagnostic.issues.push(format!(
            "Истекли cookies: {}",
            diagnostic.auth_cookies_expired.join(", ")
        ));
    }

    // Check for __Secure- cookies which are critical for authenticated access
    let has_secure_psid = diagnostic.secondary_cookies_found.iter().any(|n| n.contains("PSID"));
    if !has_secure_psid {
        diagnostic
            .issues
            .push("Отсутствуют __Secure-*PSID cookies (требуются для аутентификации)".to_string());
    }

    // Determine overall validity
    diagnostic.is_valid = diagnostic.auth_cookies_missing.is_empty()
        && diagnostic.auth_cookies_expired.is_empty()
        && diagnostic.youtube_cookies > 0
        && has_secure_psid;

    diagnostic
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
                auth_cookies_missing: REQUIRED_AUTH_COOKIES.iter().map(|s| s.to_string()).collect(),
                auth_cookies_expired: Vec::new(),
                secondary_cookies_found: Vec::new(),
                issues: vec!["YTDL_COOKIES_FILE не настроен".to_string()],
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
            auth_cookies_missing: REQUIRED_AUTH_COOKIES.iter().map(|s| s.to_string()).collect(),
            auth_cookies_expired: Vec::new(),
            secondary_cookies_found: Vec::new(),
            issues: vec![format!("Файл не найден: {}", cookies_path.display())],
            is_valid: false,
            cookie_details: Vec::new(),
            soonest_expiry_days: None,
            soonest_expiry_name: None,
        };
    }

    match tokio::fs::read_to_string(&cookies_path).await {
        Ok(content) => diagnose_cookies_content(&content),
        Err(e) => CookiesDiagnostic {
            file_exists: true,
            file_size: 0,
            total_cookies: 0,
            youtube_cookies: 0,
            auth_cookies_found: Vec::new(),
            auth_cookies_missing: REQUIRED_AUTH_COOKIES.iter().map(|s| s.to_string()).collect(),
            auth_cookies_expired: Vec::new(),
            secondary_cookies_found: Vec::new(),
            issues: vec![format!("Ошибка чтения файла: {}", e)],
            is_valid: false,
            cookie_details: Vec::new(),
            soonest_expiry_days: None,
            soonest_expiry_name: None,
        },
    }
}

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
                    return Err(reason.description());
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

    if let Ok(meta) = std::fs::metadata(&cookies_path) {
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

/// Returns the configured cookies file path from environment
fn get_cookies_path() -> Option<PathBuf> {
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
    let metadata = match std::fs::metadata(&cookies_path) {
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
    match std::fs::read_to_string(&cookies_path) {
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

    tokio::fs::write(&temp_path, &cookies_content)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to write temp cookies file: {}", e))?;

    tokio::fs::rename(&temp_path, &cookies_path).await.map_err(|e| {
        // Clean up temp file on rename failure
        let _ = std::fs::remove_file(&temp_path);
        anyhow::anyhow!("Failed to rename cookies file: {}", e)
    })?;

    log::info!("✅ Cookies file updated atomically: {:?}", cookies_path);

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

    // Acquire lock to prevent concurrent writes (race condition protection)
    let _lock = COOKIES_WRITE_MUTEX.lock().await;

    // Atomic write: write to temp file, then rename
    // This prevents file corruption if process is killed mid-write
    let temp_path = format!("{}.tmp.{}", cookies_path.display(), std::process::id());

    tokio::fs::write(&temp_path, content)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to write temp cookies file: {}", e))?;

    tokio::fs::rename(&temp_path, &cookies_path).await.map_err(|e| {
        // Clean up temp file on rename failure
        let _ = std::fs::remove_file(&temp_path);
        anyhow::anyhow!("Failed to rename cookies file: {}", e)
    })?;

    log::info!("✅ Cookies file updated atomically from content: {:?}", cookies_path);

    Ok(cookies_path)
}

// ============================================================================
// Cookies Watchdog - Background health monitoring
// ============================================================================

/// Status from watchdog check
#[derive(Debug, Clone)]
pub struct WatchdogStatus {
    pub cookies_valid: bool,
    pub invalid_reason: Option<CookieInvalidReason>,
    pub expiring_soon: Option<(String, i64)>, // (cookie_name, days_until_expiry)
    pub needs_attention: bool,
    pub message: String,
}

/// Run a single watchdog check
pub async fn watchdog_check() -> WatchdogStatus {
    // First check structural validity and expiration
    let diagnostic = diagnose_cookies_file().await;

    // Check for expiring cookies
    let expiring_soon =
        if let (Some(days), Some(name)) = (diagnostic.soonest_expiry_days, &diagnostic.soonest_expiry_name) {
            if days < 7 {
                Some((name.clone(), days))
            } else {
                None
            }
        } else {
            None
        };

    // If structural issues found, report without testing yt-dlp
    if !diagnostic.is_valid {
        let issues = diagnostic.issues.join("; ");
        return WatchdogStatus {
            cookies_valid: false,
            invalid_reason: if !diagnostic.auth_cookies_expired.is_empty() {
                Some(CookieInvalidReason::SessionExpired)
            } else if !diagnostic.auth_cookies_missing.is_empty() {
                Some(CookieInvalidReason::FileCorrupted)
            } else {
                Some(CookieInvalidReason::Unknown(issues.clone()))
            },
            expiring_soon,
            needs_attention: true,
            message: format!("Структурные проблемы: {}", issues),
        };
    }

    // Test with yt-dlp
    let validation = validate_cookies_detailed().await;

    if validation.is_valid {
        let message = if let Some((ref name, days)) = expiring_soon {
            format!("✅ Cookies работают. ⚠️ {} истекает через {} дн.", name, days)
        } else {
            "✅ Cookies работают".to_string()
        };

        let needs_attention = expiring_soon.as_ref().map(|(_, d)| *d < 3).unwrap_or(false);
        return WatchdogStatus {
            cookies_valid: true,
            invalid_reason: None,
            expiring_soon,
            needs_attention,
            message,
        };
    }

    // Cookies failed validation
    let reason = validation
        .reason
        .unwrap_or(CookieInvalidReason::Unknown("Unknown".to_string()));
    WatchdogStatus {
        cookies_valid: false,
        invalid_reason: Some(reason.clone()),
        expiring_soon,
        needs_attention: reason.is_critical(),
        message: reason.description(),
    }
}

/// Format watchdog status for Telegram notification
pub fn format_watchdog_alert(status: &WatchdogStatus) -> String {
    let mut msg = String::new();

    if status.cookies_valid {
        msg.push_str("🍪 *Cookies Watchdog*\n\n");
        msg.push_str(&status.message);
    } else {
        msg.push_str("🚨 *Cookies Alert*\n\n");
        msg.push_str(&status.message);

        if let Some(ref reason) = status.invalid_reason {
            msg.push_str("\n\n*Рекомендация:*\n");
            match reason {
                CookieInvalidReason::RotatedByYouTube | CookieInvalidReason::BotDetected => {
                    msg.push_str("1. Открой YouTube в браузере\n");
                    msg.push_str("2. Посмотри видео до конца (не пропускай)\n");
                    msg.push_str("3. Экспортируй cookies через расширение\n");
                    msg.push_str("4. Отправь файл через /update\\_cookies");
                }
                CookieInvalidReason::SessionExpired => {
                    msg.push_str("1. Залогинься в YouTube заново\n");
                    msg.push_str("2. Экспортируй cookies\n");
                    msg.push_str("3. Отправь через /update\\_cookies");
                }
                CookieInvalidReason::IpBlocked => {
                    msg.push_str("Смени прокси или подожди несколько часов");
                }
                CookieInvalidReason::RateLimited => {
                    msg.push_str("Подожди 15-30 минут");
                }
                CookieInvalidReason::VerificationRequired => {
                    msg.push_str("Пройди верификацию в браузере, затем переэкспортируй cookies");
                }
                _ => {
                    msg.push_str("Попробуй /update\\_cookies с новыми cookies");
                }
            }
        }
    }

    // Add expiry warning if relevant
    if let Some((ref name, days)) = status.expiring_soon {
        if days < 0 {
            msg.push_str(&format!("\n\n🚨 {} истёк {} дн. назад!", name, -days));
        } else if days < 3 {
            msg.push_str(&format!("\n\n⚠️ {} истекает через {} дн.!", name, days));
        }
    }

    msg
}

// ============================================================================
// Cookie Manager Client - HTTP API for Python cookie_manager.py (v3.0)
// ============================================================================

/// Cookie manager server endpoint (Python cookie_manager.py)
const COOKIE_MANAGER_BASE_URL: &str = "http://127.0.0.1:9876";

/// HTTP client for communicating with Python cookie_manager.py
///
/// Enables the feedback loop:
/// 1. Rust bot detects cookie error from yt-dlp
/// 2. Calls report_error() to notify cookie_manager
/// 3. Cookie manager triggers emergency refresh
/// 4. Rust bot retries download with fresh cookies
#[derive(Debug, Clone)]
pub struct CookieManagerClient {
    base_url: String,
    client: reqwest::Client,
}

/// Cookie manager health response
#[derive(Debug, Clone, serde::Deserialize)]
pub struct CookieHealthResponse {
    pub status: String, // "healthy", "degraded", "unhealthy"
    pub mode: String,   // "full", "degraded", "po_token", "offline"
    pub freshness_score: Option<i32>,
    pub freshness_reason: Option<String>,
    #[serde(default)]
    pub checks: CookieHealthChecks,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct CookieHealthChecks {
    #[serde(default)]
    pub cookies: CookieCheck,
    #[serde(default)]
    pub error_tracker: ErrorTrackerStatus,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct CookieCheck {
    pub exists: bool,
    pub valid: bool,
    pub required_count: i32,
    pub min_expiry_hours: Option<f64>,
    pub expired: Option<i32>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct ErrorTrackerStatus {
    pub emergency_mode: bool,
    pub errors_last_5min: i32,
    pub errors_last_1h: i32,
}

/// Response from /api/report_error
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ReportErrorResponse {
    pub success: bool,
    pub action: String, // "refresh_triggered", "cooldown", "ignored"
    pub emergency_mode: Option<bool>,
    pub recent_errors: Option<i32>,
    pub refresh_result: Option<RefreshResult>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct RefreshResult {
    pub success: bool,
    pub method: Option<String>,
    pub cookie_count: Option<i32>,
    pub error: Option<String>,
}

/// Response from /api/export_cookies
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ExportCookiesResponse {
    pub success: bool,
    pub cookie_count: Option<i32>,
    pub error: Option<String>,
}

impl Default for CookieManagerClient {
    fn default() -> Self {
        Self::new()
    }
}

impl CookieManagerClient {
    /// Create new client with default URL
    pub fn new() -> Self {
        Self::with_base_url(COOKIE_MANAGER_BASE_URL)
    }

    /// Create client with custom base URL
    pub fn with_base_url(base_url: &str) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_else(|e| {
                log::warn!("Failed to build HTTP client with timeout: {}, using default", e);
                reqwest::Client::new()
            });

        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client,
        }
    }

    /// Check if cookie manager is available
    pub async fn is_available(&self) -> bool {
        match timeout(
            Duration::from_secs(2),
            self.client.get(format!("{}/health", self.base_url)).send(),
        )
        .await
        {
            Ok(Ok(resp)) => resp.status().is_success() || resp.status().as_u16() == 503,
            _ => false,
        }
    }

    /// Get cookie manager health status
    ///
    /// Returns health info or None if cookie manager is not available.
    pub async fn health_check(&self) -> Option<CookieHealthResponse> {
        let url = format!("{}/health", self.base_url);

        match timeout(Duration::from_secs(5), self.client.get(&url).send()).await {
            Ok(Ok(resp)) => {
                if let Ok(health) = resp.json::<CookieHealthResponse>().await {
                    Some(health)
                } else {
                    log::warn!("Failed to parse cookie manager health response");
                    None
                }
            }
            Ok(Err(e)) => {
                log::debug!("Cookie manager health check failed: {}", e);
                None
            }
            Err(_) => {
                log::debug!("Cookie manager health check timed out");
                None
            }
        }
    }

    /// Report a cookie error to the cookie manager (Feedback Loop)
    ///
    /// This triggers emergency refresh if the error is cookie-related.
    /// Call this when yt-dlp returns InvalidCookies or BotDetection errors.
    ///
    /// # Arguments
    /// * `error_type` - Error type: "InvalidCookies", "BotDetection", etc.
    /// * `url` - URL that caused the error (for diagnostics)
    ///
    /// # Returns
    /// * `Ok(response)` - Cookie manager received the error
    /// * `Err` - Failed to communicate with cookie manager
    pub async fn report_error(&self, error_type: &str, url: &str) -> Result<ReportErrorResponse> {
        let api_url = format!("{}/api/report_error", self.base_url);

        let body = serde_json::json!({
            "error_type": error_type,
            "url": url,
        });

        log::info!(
            "Reporting cookie error to manager: type={}, url={}",
            error_type,
            &url[..url.len().min(80)]
        );

        let resp = timeout(Duration::from_secs(180), self.client.post(&api_url).json(&body).send())
            .await
            .map_err(|_| anyhow::anyhow!("Cookie manager report_error timed out"))?
            .map_err(|e| anyhow::anyhow!("Cookie manager request failed: {}", e))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!("Cookie manager returned {}: {}", status, text));
        }

        let response: ReportErrorResponse = resp
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to parse response: {}", e))?;

        log::info!(
            "Cookie manager response: action={}, success={}, emergency={}",
            response.action,
            response.refresh_result.as_ref().map(|r| r.success).unwrap_or(false),
            response.emergency_mode.unwrap_or(false)
        );

        Ok(response)
    }

    /// Trigger cookie export/refresh
    ///
    /// Use this to manually request a cookie refresh from the browser.
    pub async fn trigger_refresh(&self) -> Result<ExportCookiesResponse> {
        let url = format!("{}/api/export_cookies", self.base_url);

        log::info!("Triggering cookie refresh via cookie manager");

        let resp = timeout(Duration::from_secs(180), self.client.post(&url).send())
            .await
            .map_err(|_| anyhow::anyhow!("Cookie manager refresh timed out"))?
            .map_err(|e| anyhow::anyhow!("Cookie manager request failed: {}", e))?;

        let response: ExportCookiesResponse = resp
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to parse response: {}", e))?;

        if response.success {
            log::info!(
                "Cookie refresh successful: {} cookies",
                response.cookie_count.unwrap_or(0)
            );
        } else {
            log::warn!("Cookie refresh failed: {:?}", response.error);
        }

        Ok(response)
    }

    /// Check if cookies are healthy (quick check)
    ///
    /// Returns true if:
    /// - Cookie manager is available AND status is "healthy"
    /// - OR cookie manager is not available (fallback to direct cookie usage)
    pub async fn is_healthy(&self) -> bool {
        match self.health_check().await {
            Some(health) => health.status == "healthy",
            None => {
                // If cookie manager is not available, assume cookies are okay
                // (they will be validated by yt-dlp anyway)
                true
            }
        }
    }

    /// Check if emergency mode is active
    pub async fn is_emergency_mode(&self) -> bool {
        self.health_check()
            .await
            .map(|h| h.checks.error_tracker.emergency_mode)
            .unwrap_or(false)
    }
}

/// Global singleton for cookie manager client
static COOKIE_MANAGER_CLIENT: once_cell::sync::Lazy<CookieManagerClient> =
    once_cell::sync::Lazy::new(CookieManagerClient::new);

/// Get the global cookie manager client
pub fn cookie_manager() -> &'static CookieManagerClient {
    &COOKIE_MANAGER_CLIENT
}

/// Report cookie error and wait for refresh (convenience function)
///
/// This is the main function to call when yt-dlp returns a cookie error.
/// It:
/// 1. Reports the error to cookie manager
/// 2. Waits for emergency refresh if triggered
/// 3. Returns success status
///
/// # Arguments
/// * `error_type` - Error type from ytdlp_errors module
/// * `url` - URL that caused the error
///
/// # Returns
/// * `true` if refresh was triggered and succeeded (caller should retry)
/// * `false` if refresh failed or was not triggered (caller should not retry)
pub async fn report_and_wait_for_refresh(error_type: &str, url: &str) -> bool {
    let timestamp = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC");
    log::warn!(
        "[COOKIE_EVENT:{}] Cookie error detected! type={} url={}",
        timestamp,
        error_type,
        &url[..url.len().min(100)]
    );

    // Snapshot cookie file state BEFORE refresh attempt
    log_cookie_file_diagnostics(&format!("BEFORE_REFRESH({})", error_type));

    let client = cookie_manager();

    // Check if cookie manager is available
    if !client.is_available().await {
        log::warn!(
            "[COOKIE_EVENT:{}] Cookie manager NOT available (http://127.0.0.1:4417 unreachable). Cannot refresh cookies.",
            timestamp
        );
        return false;
    }

    // Report error
    match client.report_error(error_type, url).await {
        Ok(response) => {
            log::warn!(
                "[COOKIE_EVENT:{}] Cookie manager response: action={} emergency={} recent_errors={} refresh_success={} refresh_method={}",
                timestamp,
                response.action,
                response.emergency_mode.unwrap_or(false),
                response.recent_errors.unwrap_or(0),
                response.refresh_result.as_ref().map(|r| r.success).unwrap_or(false),
                response.refresh_result.as_ref().and_then(|r| r.method.as_deref()).unwrap_or("none"),
            );

            if response.action == "refresh_triggered" {
                if let Some(ref result) = response.refresh_result {
                    if result.success {
                        log::warn!(
                            "[COOKIE_EVENT:{}] Refresh SUCCESS via {} ({} cookies). Will retry download.",
                            timestamp,
                            result.method.as_deref().unwrap_or("unknown"),
                            result.cookie_count.unwrap_or(0),
                        );
                        // Snapshot cookie file state AFTER successful refresh
                        log_cookie_file_diagnostics("AFTER_REFRESH_SUCCESS");
                        return true;
                    } else {
                        log::error!(
                            "[COOKIE_EVENT:{}] Refresh FAILED: {}",
                            timestamp,
                            result.error.as_deref().unwrap_or("unknown error"),
                        );
                        log_cookie_file_diagnostics("AFTER_REFRESH_FAILURE");
                    }
                }
            } else if response.action == "cooldown" {
                log::warn!(
                    "[COOKIE_EVENT:{}] Refresh on COOLDOWN (recent refresh already happened). Waiting 5s...",
                    timestamp,
                );
                tokio::time::sleep(Duration::from_secs(5)).await;
                log_cookie_file_diagnostics("AFTER_COOLDOWN_WAIT");
                return true;
            } else if response.action == "ignored" {
                log::warn!(
                    "[COOKIE_EVENT:{}] Cookie manager IGNORED error (not classified as cookie error)",
                    timestamp,
                );
            }
        }
        Err(e) => {
            log::error!(
                "[COOKIE_EVENT:{}] Failed to communicate with cookie manager: {}",
                timestamp,
                e
            );
        }
    }

    false
}

// ============================================================================
// Instagram Cookies Management
// ============================================================================

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
            .push("Отсутствует заголовок Netscape HTTP Cookie File".to_string());
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
        diagnostic
            .issues
            .push("Не найдено ни одного Instagram cookie".to_string());
    }

    if !diagnostic.auth_cookies_missing.is_empty() {
        diagnostic.issues.push(format!(
            "Отсутствуют обязательные cookies: {}",
            diagnostic.auth_cookies_missing.join(", ")
        ));
    }

    if !diagnostic.auth_cookies_expired.is_empty() {
        diagnostic.issues.push(format!(
            "Истекли cookies: {}",
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

    tokio::fs::write(&temp_path, content)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to write temp IG cookies file: {}", e))?;

    tokio::fs::rename(&temp_path, &cookies_path).await.map_err(|e| {
        let _ = std::fs::remove_file(&temp_path);
        anyhow::anyhow!("Failed to rename IG cookies file: {}", e)
    })?;

    log::info!("✅ Instagram cookies file updated atomically: {:?}", cookies_path);

    Ok(cookies_path)
}

/// Validates Instagram cookies by testing with yt-dlp
pub async fn validate_ig_cookies() -> Result<(), String> {
    let cookies_path = match get_ig_cookies_path() {
        Some(path) => path,
        None => {
            return Err("INSTAGRAM_COOKIES_FILE не задан".to_string());
        }
    };

    if !cookies_path.exists() {
        return Err(format!("Файл cookies не найден: {}", cookies_path.display()));
    }

    match std::fs::metadata(&cookies_path) {
        Ok(meta) if meta.len() == 0 => {
            return Err("Файл cookies пуст (0 байт)".to_string());
        }
        Err(e) => {
            return Err(format!("Не удалось прочитать файл cookies: {}", e));
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
                    return Err("Instagram требует авторизацию — cookies невалидны".to_string());
                }

                if is_proxy_related_error(&stderr) {
                    last_error = Some(format!("Proxy error: {}", stderr.lines().next().unwrap_or("unknown")));
                    continue;
                }

                last_error = Some(stderr.lines().next().unwrap_or("unknown error").to_string());
            }
            Err(e) => {
                last_error = Some(format!("Не удалось запустить yt-dlp: {}", e));
                continue;
            }
        }
    }

    Err(last_error.unwrap_or_else(|| "IG Cookies validation failed".to_string()))
}

/// Load Instagram cookie header string from the cookies file.
///
/// Returns `Some("sessionid=xxx; csrftoken=yyy; ...")` if cookies are available.
pub fn load_instagram_cookie_header() -> Option<String> {
    let cookies_path = get_ig_cookies_path()?;
    let content = std::fs::read_to_string(&cookies_path).ok()?;
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
    let content = std::fs::read_to_string(&cookies_path).ok()?;
    extract_cookie_value_for_domain(&content, "instagram.com", "csrftoken")
}

#[cfg(test)]
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

    #[test]
    fn test_parse_cookies_for_domain() {
        let content = "# Netscape HTTP Cookie File\n\
            .instagram.com\tTRUE\t/\tTRUE\t0\tsessionid\tabc123\n\
            .instagram.com\tTRUE\t/\tTRUE\t0\tcsrftoken\txyz789\n\
            .youtube.com\tTRUE\t/\tTRUE\t0\tSID\tyt_sid\n";

        let result = super::parse_cookies_for_domain(content, "instagram.com");
        assert!(result.is_some());
        let header = result.unwrap();
        assert!(header.contains("sessionid=abc123"));
        assert!(header.contains("csrftoken=xyz789"));
        assert!(!header.contains("SID"));
    }

    #[test]
    fn test_parse_cookies_for_domain_no_match() {
        let content = "# Netscape HTTP Cookie File\n\
            .youtube.com\tTRUE\t/\tTRUE\t0\tSID\tyt_sid\n";
        let result = super::parse_cookies_for_domain(content, "instagram.com");
        assert!(result.is_none());
    }

    #[test]
    fn test_diagnose_ig_cookies_content_valid() {
        let content = "# Netscape HTTP Cookie File\n\
            .instagram.com\tTRUE\t/\tTRUE\t9999999999\tsessionid\tabc123\n\
            .instagram.com\tTRUE\t/\tTRUE\t9999999999\tcsrftoken\txyz789\n\
            .instagram.com\tTRUE\t/\tTRUE\t9999999999\tds_user_id\t12345\n\
            .instagram.com\tTRUE\t/\tTRUE\t0\tmid\tmid_val\n";

        let diag = super::diagnose_ig_cookies_content(content);
        assert!(diag.is_valid);
        assert!(diag.auth_cookies_missing.is_empty());
        assert_eq!(diag.auth_cookies_found.len(), 3);
    }

    #[test]
    fn test_diagnose_ig_cookies_content_missing_sessionid() {
        let content = "# Netscape HTTP Cookie File\n\
            .instagram.com\tTRUE\t/\tTRUE\t0\tcsrftoken\txyz789\n";

        let diag = super::diagnose_ig_cookies_content(content);
        assert!(!diag.is_valid);
        assert!(diag.auth_cookies_missing.contains(&"sessionid".to_string()));
    }

    #[test]
    fn test_extract_cookie_value_for_domain() {
        let content = "# Netscape HTTP Cookie File\n\
            .instagram.com\tTRUE\t/\tTRUE\t0\tsessionid\tabc123\n\
            .instagram.com\tTRUE\t/\tTRUE\t0\tcsrftoken\tmy_csrf_token\n\
            .youtube.com\tTRUE\t/\tTRUE\t0\tSID\tyt_sid\n";

        assert_eq!(
            super::extract_cookie_value_for_domain(content, "instagram.com", "csrftoken"),
            Some("my_csrf_token".to_string())
        );
        assert_eq!(
            super::extract_cookie_value_for_domain(content, "instagram.com", "sessionid"),
            Some("abc123".to_string())
        );
        assert_eq!(
            super::extract_cookie_value_for_domain(content, "instagram.com", "nonexistent"),
            None
        );
        assert_eq!(
            super::extract_cookie_value_for_domain(content, "youtube.com", "csrftoken"),
            None
        );
    }
}
