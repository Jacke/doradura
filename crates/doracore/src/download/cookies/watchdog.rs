//! Admin watchdog status and format helper.

use super::file_ops::diagnose_cookies_file;
use super::probes::validate_cookies_detailed;
use super::types::CookieInvalidReason;

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
            message: format!("Structural issues: {}", issues),
        };
    }

    // Test with yt-dlp
    let validation = validate_cookies_detailed().await;

    if validation.is_valid {
        let message = if let Some((ref name, days)) = expiring_soon {
            format!("✅ Cookies are working. ⚠️ {} expires in {} days.", name, days)
        } else {
            "✅ Cookies are working".to_string()
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
            msg.push_str("\n\n*Recommendation:*\n");
            match reason {
                CookieInvalidReason::RotatedByYouTube | CookieInvalidReason::BotDetected => {
                    msg.push_str("1. Open YouTube in your browser\n");
                    msg.push_str("2. Watch a video to the end (do not skip)\n");
                    msg.push_str("3. Export cookies via the browser extension\n");
                    msg.push_str("4. Send the file via /update\\_cookies");
                }
                CookieInvalidReason::SessionExpired => {
                    msg.push_str("1. Log in to YouTube again\n");
                    msg.push_str("2. Export cookies\n");
                    msg.push_str("3. Send via /update\\_cookies");
                }
                CookieInvalidReason::IpBlocked => {
                    msg.push_str("Change the proxy or wait a few hours");
                }
                CookieInvalidReason::RateLimited => {
                    msg.push_str("Wait 15-30 minutes");
                }
                CookieInvalidReason::VerificationRequired => {
                    msg.push_str("Complete verification in your browser, then re-export cookies");
                }
                _ => {
                    msg.push_str("Try /update\\_cookies with new cookies");
                }
            }
        }
    }

    // Add expiry warning if relevant
    if let Some((ref name, days)) = status.expiring_soon {
        if days < 0 {
            msg.push_str(&format!("\n\n🚨 {} expired {} days ago!", name, -days));
        } else if days < 3 {
            msg.push_str(&format!("\n\n⚠️ {} expires in {} days!", name, days));
        }
    }

    msg
}
