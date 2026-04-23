//! Shared types and YouTube cookie parsing/diagnosis.

/// Legacy YouTube authentication cookies (2019-era).
///
/// Modern Chrome (and every cookie-export tool based on it) stopped writing
/// these years ago — YouTube now ships the `__Secure-*` equivalents. Kept in
/// the checker as a **fallback path**: if a user happens to have a legacy
/// export, we still accept it. But their absence is NOT an error.
pub(super) const LEGACY_AUTH_COOKIES: &[&str] = &[
    "SID",     // Session ID
    "HSID",    // HTTP Session ID
    "SSID",    // Secure Session ID
    "APISID",  // API Session ID
    "SAPISID", // Secure API Session ID
];

/// Modern YouTube authentication cookies (2024+).
///
/// These are what `__Secure-*PSID` / `__Secure-*PAPISID` / `LOGIN_INFO` look
/// like in a fresh Chrome cookie export. Having `__Secure-3PSID` (or
/// `__Secure-1PSID`) is the modern equivalent of the full legacy set.
pub(super) const MODERN_AUTH_COOKIES: &[&str] = &[
    "__Secure-3PSID",
    "__Secure-1PSID",
    "__Secure-3PAPISID",
    "__Secure-1PAPISID",
    "LOGIN_INFO",
];

/// Secondary cookies that help with YouTube access but aren't sufficient
/// for authentication on their own.
pub(super) const SECONDARY_COOKIES: &[&str] = &["PREF", "VISITOR_INFO1_LIVE"];

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
            || stderr_lower.contains("sign in to confirm you're not a bot")
            || stderr_lower.contains("confirm you're not a bot")
            || stderr_lower.contains("confirm you're not a bot")
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

    /// Get human-readable description in English
    pub fn description(&self) -> String {
        match self {
            Self::FileNotFound => "Cookies file not found".to_string(),
            Self::FileEmpty => "Cookies file is empty".to_string(),
            Self::FileCorrupted => "Cookies file is corrupted or has an invalid format".to_string(),
            Self::RotatedByYouTube => {
                "🔄 YouTube rotated cookies (bot protection). Please re-export them from your browser.".to_string()
            }
            Self::SessionExpired => "⏰ Session expired — YouTube requires you to log in again".to_string(),
            Self::BotDetected => {
                "🤖 YouTube detected a bot. Fresh cookies are needed after manually watching a video.".to_string()
            }
            Self::IpBlocked => "🚫 IP address is blocked by YouTube".to_string(),
            Self::VerificationRequired => "🔐 Account requires verification (captcha/SMS/2FA)".to_string(),
            Self::RateLimited => "⏳ Request rate limit exceeded — please wait a moment".to_string(),
            Self::Unknown(msg) => format!("❓ Unknown error: {}", msg),
            Self::AllProxiesFailed(msg) => format!("🌐 All proxies failed: {}", msg),
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
            Some(0) => "session".to_string(),
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
                        format!("expired {}h ago", hours)
                    } else {
                        format!("expired {}d ago", days)
                    }
                } else {
                    let diff = ts - now;
                    let days = diff / 86400;
                    if days > 365 {
                        format!("{}y", days / 365)
                    } else if days > 30 {
                        format!("{}mo", days / 30)
                    } else if days > 0 {
                        format!("{}d", days)
                    } else {
                        let hours = diff / 3600;
                        format!("{}h", hours)
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
            Some(ts) => match chrono::DateTime::from_timestamp(ts, 0) {
                Some(dt) => dt.format("%Y-%m-%d").to_string(),
                None => "Unknown".to_string(),
            },
            None => "unknown".to_string(),
        }
    }
}

/// Cookie detail for diagnostic report
#[derive(Debug, Clone)]
pub struct CookieDetail {
    pub name: String,
    pub masked_value: String,
    pub expiration: String,      // Human readable (e.g., "5d")
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
            return "❌ Cookies file not found".to_string();
        }

        report.push_str(&format!("📄 File size: {} bytes\n", self.file_size));
        report.push_str(&format!(
            "🍪 Total cookies: {} (YouTube: {})\n\n",
            self.total_cookies, self.youtube_cookies
        ));

        // Auth cookies status. YouTube accepts either the modern `__Secure-*`
        // set or the legacy `SID/HSID/SSID/APISID/SAPISID` set — we print
        // whichever the user actually has, and only show missing entries
        // when they belong to the scheme the user is using.
        let has_any_auth_detail = self.cookie_details.iter().any(|d| d.is_critical);
        if has_any_auth_detail || !self.auth_cookies_missing.is_empty() {
            report.push_str("*Authentication cookies:*\n");

            for detail in self.cookie_details.iter().filter(|d| d.is_critical) {
                let status = if detail.is_expired { "⚠️" } else { "✅" };
                report.push_str(&format!(
                    "  {} {} | {} | `{}`\n",
                    status, detail.name, detail.expiration, detail.masked_value
                ));
            }

            // Only shown for truly missing cookies in the scheme the user
            // is using — a modern export won't trigger legacy ❌ lines.
            for name in &self.auth_cookies_missing {
                report.push_str(&format!("  ❌ {} — missing\n", name));
            }
        }

        // Secondary / helper cookies (PREF, VISITOR_INFO1_LIVE, etc.)
        let has_any_secondary_detail = self.cookie_details.iter().any(|d| !d.is_critical);
        if has_any_secondary_detail {
            report.push_str("\n*Additional cookies:*\n");
            for detail in self.cookie_details.iter().filter(|d| !d.is_critical) {
                let status = if detail.is_expired { "⚠️" } else { "✅" };
                report.push_str(&format!("  {} {} | {}\n", status, detail.name, detail.expiration));
            }
        }

        // Expiration warning
        if let (Some(days), Some(name)) = (self.soonest_expiry_days, &self.soonest_expiry_name) {
            report.push('\n');
            if days < 0 {
                report.push_str(&format!("🚨 *{} expired {} days ago!*\n", name, -days));
            } else if days < 3 {
                report.push_str(&format!("⚠️ *{} expires in {} days!*\n", name, days));
            } else if days < 7 {
                report.push_str(&format!("⏰ {} expires in {} days\n", name, days));
            }
        }

        // Issues summary
        if !self.issues.is_empty() {
            report.push_str("\n*⚠️ Issues:*\n");
            for issue in &self.issues {
                report.push_str(&format!("  • {}\n", issue));
            }
        }

        // Overall status
        report.push('\n');
        if self.is_valid {
            report.push_str("✅ *Cookies look valid*");
        } else {
            report.push_str("❌ *Cookies are invalid — re-export required*");
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
            .push("Missing Netscape HTTP Cookie File header".to_string());
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

                // An auth cookie is EITHER the legacy set OR the modern set —
                // whichever the user's cookie-export tool happened to write.
                // YouTube 2024+ has completely stopped shipping the legacy
                // names, so the modern set is the common case.
                let is_legacy_auth = LEGACY_AUTH_COOKIES.contains(&name.as_str());
                let is_modern_auth = MODERN_AUTH_COOKIES.contains(&name.as_str());
                let is_auth = is_legacy_auth || is_modern_auth;
                let is_secondary = SECONDARY_COOKIES.contains(&name.as_str());

                if is_auth {
                    diagnostic.auth_cookies_found.push(name.clone());
                    if cookie.is_expired() {
                        diagnostic.auth_cookies_expired.push(name.clone());
                    }
                }

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

    // Determine which auth scheme the user's cookies are using:
    // - modern: has any of __Secure-3PSID / __Secure-1PSID / __Secure-*PAPISID / LOGIN_INFO
    // - legacy: has the full old-style set (SID/HSID/SSID/APISID/SAPISID)
    //
    // Only report a cookie as "missing" if it's missing from the scheme the
    // user is ACTUALLY using. Marking legacy cookies as ❌ on a modern export
    // produced a confusing "all red + validation passed" report.
    let has_any_modern = diagnostic
        .auth_cookies_found
        .iter()
        .any(|n| MODERN_AUTH_COOKIES.contains(&n.as_str()));
    let has_all_legacy = LEGACY_AUTH_COOKIES
        .iter()
        .all(|&n| diagnostic.auth_cookies_found.iter().any(|f| f == n));

    if has_any_modern {
        // User has modern cookies — only report modern ones as "required".
        // Missing entries in the legacy set are expected and not a problem.
        for &required in MODERN_AUTH_COOKIES {
            if !diagnostic.auth_cookies_found.iter().any(|n| n == required) {
                // __Secure-3PSID is the primary one — the others are nice-to-have.
                // Only mark __Secure-3PSID as missing when it's actually absent.
                if required == "__Secure-3PSID"
                    && !diagnostic
                        .auth_cookies_found
                        .iter()
                        .any(|n| n == "__Secure-3PSID" || n == "__Secure-1PSID")
                {
                    diagnostic.auth_cookies_missing.push(required.to_string());
                }
            }
        }
    } else {
        // No modern cookies — fall back to legacy checking.
        for &required in LEGACY_AUTH_COOKIES {
            if !diagnostic.auth_cookies_found.iter().any(|n| n == required) {
                diagnostic.auth_cookies_missing.push(required.to_string());
            }
        }
    }

    // Analyze issues
    if diagnostic.youtube_cookies == 0 {
        diagnostic.issues.push("No YouTube cookies found".to_string());
    }

    // Only warn about missing cookies when BOTH schemes are incomplete.
    if !diagnostic.auth_cookies_missing.is_empty() && !has_any_modern {
        diagnostic.issues.push(format!(
            "Missing auth cookies: {}",
            diagnostic.auth_cookies_missing.join(", ")
        ));
    }

    if !diagnostic.auth_cookies_expired.is_empty() {
        diagnostic.issues.push(format!(
            "Expired cookies: {}",
            diagnostic.auth_cookies_expired.join(", ")
        ));
    }

    if !has_any_modern && !has_all_legacy {
        diagnostic
            .issues
            .push("Missing authentication cookies (need either __Secure-*PSID or legacy SID set)".to_string());
    }

    // Valid if EITHER auth scheme is present, no expired cookies, and we actually
    // saw some YouTube cookies.
    diagnostic.is_valid = (has_any_modern || has_all_legacy)
        && diagnostic.auth_cookies_expired.is_empty()
        && diagnostic.youtube_cookies > 0;

    diagnostic
}
