//! Admin functionality for the Telegram bot
//!
//! This module contains all admin-related commands and utilities:
//! - User management (/users, /setplan, /admin)
//! - System diagnostics (/version, /botapi_speed, /transactions, /backup)
//! - Cookie management (/update_cookies, /diagnose_cookies)
//! - Browser automation (/browser_login, /browser_status)
//! - Proxy management (/proxy_stats, /proxy_reset)
//! - Broadcast (/send, /broadcast)
//! - File download helpers

pub mod broadcast;
pub mod browser;
pub mod cookies;
pub mod download_helpers;
pub mod system;
pub mod users;

// Re-export all public items for backward compatibility
pub use broadcast::*;
pub use browser::{
    handle_browser_callback, handle_browser_login_command, handle_browser_status_command, handle_proxy_reset_command,
    handle_proxy_stats_command,
};
pub use cookies::*;
pub use download_helpers::{download_file_from_telegram, download_file_with_fallback};
pub use system::*;
pub use users::*;

use crate::core::config::admin::{ADMIN_IDS, ADMIN_USER_ID};

// Re-export escape_markdown for backward compatibility (other modules import from here)
pub use crate::core::escape_markdown;

/// Maximum message length for Telegram (with margin) - for backward compatibility
pub const MAX_MESSAGE_LENGTH: usize = crate::core::TELEGRAM_MESSAGE_LIMIT;

/// Check if user is admin
pub fn is_admin(user_id: i64) -> bool {
    if !ADMIN_IDS.is_empty() {
        return ADMIN_IDS.contains(&user_id);
    }
    if *ADMIN_USER_ID != 0 {
        return *ADMIN_USER_ID == user_id;
    }
    false
}

/// Truncate message for Telegram - for backward compatibility with original behavior
fn truncate_message(text: &str) -> String {
    if text.len() <= MAX_MESSAGE_LENGTH {
        return text.to_string();
    }
    // Original behavior: trim to MAX_MESSAGE_LENGTH - 20 and add "\n... (truncated)"
    let mut trimmed = text.chars().take(MAX_MESSAGE_LENGTH - 20).collect::<String>();
    trimmed.push_str("\n... (truncated)");
    trimmed
}

fn indent_lines(text: &str, indent: &str) -> String {
    text.lines()
        .map(|line| format!("{}{}", indent, line))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_escape_markdown_basic() {
        assert_eq!(super::escape_markdown("hello"), "hello");
        assert_eq!(super::escape_markdown("hello_world"), "hello\\_world");
        assert_eq!(super::escape_markdown("hello*world"), "hello\\*world");
    }

    #[test]
    fn test_escape_markdown_complex() {
        let input = "Test: [link](url) *bold* _italic_ `code`";
        let expected = "Test: \\[link\\]\\(url\\) \\*bold\\* \\_italic\\_ \\`code\\`";
        assert_eq!(super::escape_markdown(input), expected);
    }

    #[test]
    fn test_escape_markdown_all_special_chars() {
        let input = r"\*[]()~`>#+-=|{}.!";
        let expected = r"\\\*\[\]\(\)\~\`\>\#\+\-\=\|\{\}\.\!";
        assert_eq!(super::escape_markdown(input), expected);
    }

    #[test]
    fn test_is_admin() {
        if !super::ADMIN_IDS.is_empty() {
            let admin_id = super::ADMIN_IDS[0];
            let non_admin_id = super::ADMIN_IDS.iter().max().copied().unwrap_or(0) + 1;
            assert!(super::is_admin(admin_id));
            assert!(!super::is_admin(non_admin_id));
        } else if *super::ADMIN_USER_ID != 0 {
            let admin_id = *super::ADMIN_USER_ID;
            assert!(super::is_admin(admin_id));
            assert!(!super::is_admin(admin_id + 1));
        } else {
            assert!(!super::is_admin(0));
        }
    }

    // ==================== truncate_message Tests ====================

    #[test]
    fn test_truncate_message_short() {
        let text = "Hello, World!";
        assert_eq!(super::truncate_message(text), text);
    }

    #[test]
    fn test_truncate_message_at_limit() {
        let text = "a".repeat(super::MAX_MESSAGE_LENGTH);
        assert_eq!(super::truncate_message(&text), text);
    }

    #[test]
    fn test_truncate_message_over_limit() {
        let text = "a".repeat(super::MAX_MESSAGE_LENGTH + 100);
        let result = super::truncate_message(&text);
        assert!(result.len() < super::MAX_MESSAGE_LENGTH);
        assert!(result.ends_with("... (truncated)"));
    }

    #[test]
    fn test_truncate_message_empty() {
        assert_eq!(super::truncate_message(""), "");
    }

    // ==================== indent_lines Tests ====================

    #[test]
    fn test_indent_lines_single_line() {
        assert_eq!(super::indent_lines("hello", "  "), "  hello");
    }

    #[test]
    fn test_indent_lines_multiple_lines() {
        let input = "line1\nline2\nline3";
        let expected = "  line1\n  line2\n  line3";
        assert_eq!(super::indent_lines(input, "  "), expected);
    }

    #[test]
    fn test_indent_lines_empty() {
        assert_eq!(super::indent_lines("", "  "), "");
    }

    #[test]
    fn test_indent_lines_with_tabs() {
        assert_eq!(super::indent_lines("hello", "\t"), "\thello");
    }

    // ==================== format_subscription_period_for_log Tests ====================

    #[test]
    fn test_format_subscription_period_for_log_one_day() {
        use teloxide::types::Seconds;
        let period = Seconds::from_seconds(86400);
        let result = super::system::format_subscription_period_for_log(&period);
        assert!(result.contains("86400 seconds"));
        assert!(result.contains("~1.00 days"));
    }

    #[test]
    fn test_format_subscription_period_for_log_one_month() {
        use teloxide::types::Seconds;
        let period = Seconds::from_seconds(86400 * 30);
        let result = super::system::format_subscription_period_for_log(&period);
        assert!(result.contains("~30.00 days"));
        assert!(result.contains("~1.00 months"));
    }

    #[test]
    fn test_format_subscription_period_for_log_zero() {
        use teloxide::types::Seconds;
        let period = Seconds::from_seconds(0);
        let result = super::system::format_subscription_period_for_log(&period);
        assert!(result.contains("0 seconds"));
    }

    // ==================== is_local_bot_api Tests ====================

    #[test]
    fn test_is_local_bot_api_official() {
        assert!(!crate::core::config::bot_api::is_local_url(
            "https://api.telegram.org/bot12345"
        ));
    }

    #[test]
    fn test_is_local_bot_api_local() {
        assert!(crate::core::config::bot_api::is_local_url(
            "http://localhost:8081/bot12345"
        ));
        assert!(crate::core::config::bot_api::is_local_url(
            "http://127.0.0.1:8081/bot12345"
        ));
        assert!(crate::core::config::bot_api::is_local_url(
            "http://my-bot-api.local/bot12345"
        ));
    }

    // ==================== read_log_tail Tests ====================

    #[test]
    fn test_read_log_tail_nonexistent() {
        let result = super::system::read_log_tail(&std::path::PathBuf::from("/nonexistent/file.log"), 1024);
        assert!(result.is_err());
    }

    #[test]
    fn test_read_log_tail_small_file() {
        use std::io::Write;
        let temp_dir = std::env::temp_dir();
        let temp_file = temp_dir.join(format!("admin_test_log_{}.txt", std::process::id()));

        let mut file = std::fs::File::create(&temp_file).unwrap();
        writeln!(file, "Line 1").unwrap();
        writeln!(file, "Line 2").unwrap();
        drop(file);

        let result = super::system::read_log_tail(&temp_file, 1024).unwrap();
        let _ = std::fs::remove_file(&temp_file);

        assert!(result.contains("Line 1"));
        assert!(result.contains("Line 2"));
    }

    // ==================== MAX_MESSAGE_LENGTH constant test ====================

    #[test]
    fn test_max_message_length_value() {
        // Telegram limit is 4096, we use 4000 to have margin
        assert_eq!(super::MAX_MESSAGE_LENGTH, 4000);
    }
}
