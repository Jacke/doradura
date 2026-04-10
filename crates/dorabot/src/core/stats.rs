use crate::core::escape_markdown;
use crate::storage::{DbPool, SharedStorage};
use crate::telegram::Bot;
use std::sync::Arc;
use teloxide::prelude::*;

/// Format a byte size for MarkdownV2 output — wraps the shared
/// `doracore::core::format_bytes_i64` and escapes the dot for MDv2.
fn format_size(bytes: i64) -> String {
    doracore::core::format_bytes_i64(bytes).replace('.', "\\.")
}

// truncate_string_safe is now imported from crate::core

/// Creates an ASCII activity chart
fn create_activity_chart(activity_by_day: &[(String, i64)]) -> String {
    if activity_by_day.is_empty() {
        return "No data".to_string();
    }

    let max_count = activity_by_day.iter().map(|(_, count)| *count).max().unwrap_or(1);
    let max_bars = 10;

    let mut chart = String::new();
    for (day, count) in activity_by_day.iter().take(7) {
        let bars = if max_count > 0 {
            (count * max_bars as i64 / max_count) as usize
        } else {
            0
        };
        let bar_string = "█".repeat(bars) + &"░".repeat(max_bars - bars);

        // Format date from "YYYY-MM-DD" to short format
        let day_short = if day.len() >= 10 {
            let parts: Vec<&str> = day.split('-').collect();
            if parts.len() >= 3 {
                format!("{}.{}", parts[2], parts[1])
            } else {
                day.clone()
            }
        } else {
            day.clone()
        };

        chart.push_str(&format!("{}: {} {}\n", day_short, bar_string, count));
    }
    chart
}

/// Shows the user's download statistics
pub async fn show_user_stats(
    bot: &Bot,
    chat_id: ChatId,
    _db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
) -> ResponseResult<Message> {
    log::info!("show_user_stats called for chat_id: {}", chat_id.0);

    log::debug!("Fetching shared stats for user {}", chat_id.0);

    let stats = match shared_storage.get_user_stats(chat_id.0).await {
        Ok(stats) => {
            log::info!(
                "Stats fetched: downloads={}, size={}, days={}",
                stats.total_downloads,
                stats.total_size,
                stats.active_days
            );
            stats
        }
        Err(e) => {
            log::error!("Failed to get user stats from DB: {}", e);
            return bot
                .send_message(chat_id, "Failed to load statistics 😢 Please try again later\\.")
                .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                .await;
        }
    };

    log::debug!("Building stats text message");

    let mut text = "📊 *Your Statistics*\n\n".to_string();

    text.push_str(&format!("🎵 Total downloads: {}\n", stats.total_downloads));
    text.push_str(&format!("📅 Active days: {}\n", stats.active_days));
    text.push_str(&format!("💾 Total size: {}\n\n", format_size(stats.total_size)));

    if !stats.top_artists.is_empty() {
        text.push_str("🏆 *Top artists:*\n");
        for (idx, (artist, count)) in stats.top_artists.iter().enumerate() {
            text.push_str(&format!(
                "{}\\. {} \\- {} tracks\n",
                idx + 1,
                escape_markdown(artist),
                count
            ));
        }
        text.push('\n');
    }

    if !stats.top_formats.is_empty() {
        text.push_str("📦 *Formats:*\n");
        for (format, count) in stats.top_formats.iter() {
            let format_emoji = match format.as_str() {
                "mp3" => "🎵",
                "mp4" => "🎬",
                "srt" => "📝",
                "txt" => "📄",
                _ => "📦",
            };
            text.push_str(&format!("{} {}: {}\n", format_emoji, format.to_uppercase(), count));
        }
        text.push('\n');
    }

    if !stats.activity_by_day.is_empty() {
        text.push_str("📈 *Activity \\(last 7 days\\):*\n");
        text.push_str("```\n");
        text.push_str(&create_activity_chart(&stats.activity_by_day));
        text.push_str("```\n");
    }

    if stats.total_downloads == 0 {
        text = "📊 *Your Statistics*\n\nYou have no downloads yet\\. Send me a link to a track or video\\!".to_string();
    }

    log::debug!("Sending stats message, length: {}", text.len());

    bot.send_message(chat_id, text)
        .parse_mode(teloxide::types::ParseMode::MarkdownV2)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::truncate_string_safe;

    // ==================== format_size Tests ====================

    #[test]
    fn test_format_size_escapes_dots() {
        // Test GB format (has decimal point)
        let gb_size = 2_500_000_000; // ~2.33 GB
        let result = format_size(gb_size);
        assert!(result.contains("\\."), "GB format should escape dot: {}", result);
        assert!(!result.contains(" . "), "Should not have unescaped dots");

        // Test MB format (has decimal point)
        let mb_size = 12_500_000; // ~11.9 MB
        let result = format_size(mb_size);
        assert!(result.contains("\\."), "MB format should escape dot: {}", result);

        // Test KB format (has decimal point)
        let kb_size = 2_500; // ~2.4 KB
        let result = format_size(kb_size);
        assert!(result.contains("\\."), "KB format should escape dot: {}", result);

        // Test B format (no decimal point)
        let b_size = 500;
        let result = format_size(b_size);
        assert!(!result.contains("."), "B format should have no dots: {}", result);
        assert_eq!(result, "500 B");
    }

    #[test]
    fn test_format_size_boundary_values() {
        // Exactly 1 GB
        let one_gb = 1024 * 1024 * 1024;
        let result = format_size(one_gb);
        assert!(result.contains("GB"));

        // Just under 1 GB
        let under_gb = 1024 * 1024 * 1024 - 1;
        let result = format_size(under_gb);
        assert!(result.contains("MB"));

        // Exactly 1 MB
        let one_mb = 1024 * 1024;
        let result = format_size(one_mb);
        assert!(result.contains("MB"));

        // Exactly 1 KB
        let one_kb = 1024;
        let result = format_size(one_kb);
        assert!(result.contains("KB"));
    }

    #[test]
    fn test_format_size_zero() {
        let result = format_size(0);
        assert_eq!(result, "0 B");
    }

    // ==================== truncate_string_safe Tests ====================

    #[test]
    fn test_truncate_string_safe_short_text() {
        let result = truncate_string_safe("Hello", 10);
        assert_eq!(result, "Hello");
    }

    #[test]
    fn test_truncate_string_safe_exact_length() {
        let result = truncate_string_safe("Hello", 5);
        assert_eq!(result, "Hello");
    }

    #[test]
    fn test_truncate_string_safe_needs_truncation() {
        let result = truncate_string_safe("Hello World", 8);
        assert_eq!(result, "Hello...");
    }

    #[test]
    fn test_truncate_string_safe_empty() {
        let result = truncate_string_safe("", 10);
        assert_eq!(result, "");
    }

    #[test]
    fn test_truncate_string_safe_multibyte() {
        // Test with multi-byte UTF-8 characters (emoji are 4 bytes each)
        let result = truncate_string_safe("Hello 🌍 world 🎵", 8);
        assert!(result.ends_with("..."));
        // Should not panic with multi-byte characters
    }

    #[test]
    fn test_truncate_string_safe_very_short_max() {
        // Max length less than 3 (length of "...")
        let result = truncate_string_safe("Hello", 2);
        assert_eq!(result, "...");
    }

    // ==================== create_activity_chart Tests ====================

    #[test]
    fn test_create_activity_chart_empty() {
        let result = create_activity_chart(&[]);
        assert_eq!(result, "No data");
    }

    #[test]
    fn test_create_activity_chart_single_day() {
        let data = vec![("2024-01-15".to_string(), 5)];
        let result = create_activity_chart(&data);
        assert!(result.contains("15.01"));
        assert!(result.contains("5"));
    }

    #[test]
    fn test_create_activity_chart_multiple_days() {
        let data = vec![
            ("2024-01-15".to_string(), 10),
            ("2024-01-14".to_string(), 5),
            ("2024-01-13".to_string(), 2),
        ];
        let result = create_activity_chart(&data);
        assert!(result.contains("15.01"));
        assert!(result.contains("14.01"));
        assert!(result.contains("13.01"));
    }

    #[test]
    fn test_create_activity_chart_max_7_days() {
        let data: Vec<(String, i64)> = (0..10).map(|i| (format!("2024-01-{:02}", 10 + i), i as i64)).collect();
        let result = create_activity_chart(&data);
        // Should only include 7 days
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 7);
    }

    #[test]
    fn test_create_activity_chart_zero_count() {
        let data = vec![("2024-01-15".to_string(), 0)];
        let result = create_activity_chart(&data);
        assert!(result.contains("0"));
        assert!(result.contains("░░░░░░░░░░")); // All empty bars
    }

    // ==================== escape_markdown Tests ====================

    #[test]
    fn test_escape_markdown_escapes_dots() {
        let text = "Hello. World. Test.";
        let result = escape_markdown(text);
        assert_eq!(result, "Hello\\. World\\. Test\\.");
    }

    #[test]
    fn test_escape_markdown_all_special_chars() {
        let input = r"_*[]()~`>#+-=|{}.!";
        let expected = r"\_\*\[\]\(\)\~\`\>\#\+\-\=\|\{\}\.\!";
        assert_eq!(escape_markdown(input), expected);
    }

    #[test]
    fn test_escape_markdown_backslash() {
        assert_eq!(escape_markdown("a\\b"), "a\\\\b");
    }

    #[test]
    fn test_escape_markdown_empty() {
        assert_eq!(escape_markdown(""), "");
    }

    #[test]
    fn test_escape_markdown_exclamation() {
        let text = "Hello world!";
        let result = escape_markdown(text);
        assert_eq!(result, "Hello world\\!");
    }
}
