pub mod callbacks;
mod categories;
mod cb_helpers;
mod clipping;
mod cover;
mod send;
mod speed;
pub mod subtitles;
mod voice_lyrics;

pub use callbacks::*;
pub use subtitles::*;

use crate::core::escape_markdown;
use crate::downsub::DownsubGateway;
use crate::storage::{DbPool, SharedStorage, SubtitleCache};
use crate::telegram::Bot;
use crate::timestamps::{format_timestamp, select_best_timestamps, VideoTimestamp};
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup, ParseMode};

/// Shared context for download callback handlers, avoiding 9-parameter signatures.
pub(crate) struct CallbackCtx {
    pub bot: Bot,
    pub chat_id: ChatId,
    pub message_id: teloxide::types::MessageId,
    pub db_pool: Arc<DbPool>,
    pub shared_storage: Arc<SharedStorage>,
    pub username: Option<String>,
    pub downsub_gateway: Arc<DownsubGateway>,
    pub subtitle_cache: Arc<SubtitleCache>,
}

const ITEMS_PER_PAGE: usize = 5;

fn is_youtube_url(url: &str) -> bool {
    // Check for the domain after the scheme to avoid false positives like "notyoutube.com"
    url.contains("://youtube.com/")
        || url.contains("://www.youtube.com/")
        || url.contains("://m.youtube.com/")
        || url.contains("://music.youtube.com/")
        || url.contains("://youtu.be/")
}

/// Re-export of the shared byte formatter under the local name so call
/// sites in this file don't need to change.
use doracore::core::format_bytes_i64 as format_file_size;

/// Format duration for display
fn format_duration(seconds: i64) -> String {
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let secs = seconds % 60;

    if hours > 0 {
        format!("{}:{:02}:{:02}", hours, minutes, secs)
    } else {
        format!("{}:{:02}", minutes, secs)
    }
}

/// Build duration selection buttons for circle creation
/// Returns rows of buttons with time ranges (first/last/middle/full)
fn build_duration_buttons(download_id: i64, lang: &unic_langid::LanguageIdentifier) -> Vec<Vec<InlineKeyboardButton>> {
    let durations = [15, 30, 60];

    // Row 1: First N seconds (from beginning)
    let first_row: Vec<InlineKeyboardButton> = durations
        .iter()
        .map(|&dur| {
            let label = format!("▶ 0:00–{}", format_duration_short(dur));
            crate::telegram::cb(label, format!("downloads:dur:first:{}:{}", download_id, dur))
        })
        .collect();

    // Row 2: Last N seconds (from end)
    let last_row: Vec<InlineKeyboardButton> = durations
        .iter()
        .map(|&dur| {
            let label = format!("◀ ...–{}", format_duration_short(dur));
            crate::telegram::cb(label, format!("downloads:dur:last:{}:{}", download_id, dur))
        })
        .collect();

    // Row 3: Middle and Full (localized)
    let btn_middle = crate::i18n::t(lang, "video_circle.btn_middle");
    let btn_full = crate::i18n::t(lang, "video_circle.btn_full");
    let special_row = vec![
        crate::telegram::cb(btn_middle, format!("downloads:dur:middle:{}:30", download_id)),
        crate::telegram::cb(btn_full, format!("downloads:dur:full:{}", download_id)),
    ];

    vec![first_row, last_row, special_row]
}

/// Format duration as short string (0:15, 0:30, 1:00)
pub(super) fn format_duration_short(seconds: i64) -> String {
    let mins = seconds / 60;
    let secs = seconds % 60;
    format!("{}:{:02}", mins, secs)
}

/// Build timestamp buttons for clip/circle creation
/// Returns (buttons_rows, text_list) where buttons_rows contains up to 6 buttons
/// and text_list contains all timestamps as formatted text
fn build_timestamp_ui(
    timestamps: &[VideoTimestamp],
    output_kind: &str,
    download_id: i64,
) -> (Vec<Vec<InlineKeyboardButton>>, String) {
    if timestamps.is_empty() {
        return (vec![], String::new());
    }

    // Select best timestamps for buttons (max 6)
    let best = select_best_timestamps(timestamps, 6);

    // Build buttons (2 per row)
    let mut button_rows: Vec<Vec<InlineKeyboardButton>> = vec![];
    let mut current_row: Vec<InlineKeyboardButton> = vec![];

    for ts in &best {
        let time_str = ts.format_time();
        let label = ts.display_label(10);
        let button_text = format!("{} {}", time_str, label);

        // Callback format: downloads:ts:{output_kind}:{download_id}:{time_seconds}
        let callback = format!("downloads:ts:{}:{}:{}", output_kind, download_id, ts.time_seconds);

        current_row.push(crate::telegram::cb(button_text, callback));

        if current_row.len() == 2 {
            button_rows.push(current_row);
            current_row = vec![];
        }
    }

    // Add remaining button if any
    if !current_row.is_empty() {
        button_rows.push(current_row);
    }

    // Build text list for all timestamps
    let mut text_lines: Vec<String> = vec![];
    for ts in timestamps {
        let time_str = ts.format_time();
        let label = ts.label.as_deref().unwrap_or("");
        if label.is_empty() {
            text_lines.push(format!("• {}", escape_markdown(&time_str)));
        } else {
            text_lines.push(format!(
                "• {} \\- {}",
                escape_markdown(&time_str),
                escape_markdown(label)
            ));
        }
    }

    let text = if !text_lines.is_empty() {
        format!("\n\n📍 *Saved timestamps:*\n{}", text_lines.join("\n"))
    } else {
        String::new()
    };

    (button_rows, text)
}

/// Show downloads page
pub async fn show_downloads_page(
    bot: &Bot,
    chat_id: ChatId,
    _db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
    page: usize,
    file_type_filter: Option<String>,
    search_text: Option<String>,
    category_filter: Option<String>,
) -> ResponseResult<Message> {
    // Get filtered downloads
    let all_downloads = if file_type_filter.as_deref() == Some("edit") {
        shared_storage
            .get_cuts_history_filtered(chat_id.0, search_text.as_deref())
            .await
            .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
    } else {
        shared_storage
            .get_download_history_filtered(
                chat_id.0,
                file_type_filter.as_deref(),
                search_text.as_deref(),
                category_filter.as_deref(),
            )
            .await
            .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
    };

    if all_downloads.is_empty() {
        let empty_msg = if file_type_filter.is_some() || search_text.is_some() || category_filter.is_some() {
            "📭 Nothing found.\n\nTry changing the filters."
        } else {
            "📭 You have no downloaded files yet.\n\nDownload something and it will appear here!"
        };
        return bot.send_message(chat_id, empty_msg).await;
    }

    let total_items = all_downloads.len();
    let total_pages = total_items.div_ceil(ITEMS_PER_PAGE);
    let current_page = page.min(total_pages.saturating_sub(1));

    let start_idx = current_page * ITEMS_PER_PAGE;
    let end_idx = (start_idx + ITEMS_PER_PAGE).min(total_items);
    let page_downloads = &all_downloads[start_idx..end_idx];

    // Build message text
    let mut text = String::from("📥 *Your downloads*\n\n");

    // Show active filters
    if let Some(ref ft) = file_type_filter {
        let icon = match ft.as_str() {
            "mp3" => "🎵",
            "mp4" => "🎬",
            "edit" => "✂️",
            _ => "📄",
        };
        let filter_name = if ft == "edit" {
            "Clips".to_string()
        } else {
            ft.to_uppercase()
        };
        text.push_str(&format!("Filter: {} {}\n\n", icon, filter_name));
    }
    if let Some(ref search) = search_text {
        text.push_str(&format!("🔍 Search: \"{}\"\n\n", search));
    }
    if let Some(ref cat) = category_filter {
        text.push_str(&format!("🏷 Category: {}\n\n", escape_markdown(cat)));
    }

    // List downloads
    for download in page_downloads {
        let icon = match download.format.as_str() {
            "mp3" => "🎵",
            "mp4" => "🎬",
            "edit" => "✂️",
            _ => "📄",
        };
        let title = if let Some(ref author) = download.author {
            format!("{} - {}", author, download.title)
        } else {
            download.title.clone()
        };
        let cat_badge = download
            .category
            .as_deref()
            .map(|c| format!(" _({})", escape_markdown(c)))
            .unwrap_or_default();

        text.push_str(&format!("{} *{}*{}\n", icon, escape_markdown(&title), cat_badge));

        // Format metadata
        let mut metadata_parts = Vec::new();

        if let Some(size) = download.file_size {
            metadata_parts.push(format_file_size(size));
        }

        if let Some(dur) = download.duration {
            metadata_parts.push(format_duration(dur));
        }

        if let Some(ref quality) = download.video_quality {
            metadata_parts.push(quality.clone());
        }

        if let Some(ref bitrate) = download.audio_bitrate {
            metadata_parts.push(bitrate.clone());
        }

        if let Some(spd) = download.speed {
            // Format as "2x" for whole numbers, "1.5x" otherwise
            let formatted = if spd.fract() == 0.0 {
                format!("{}x", spd as i32)
            } else {
                format!("{}x", spd)
            };
            metadata_parts.push(formatted);
        }

        if !metadata_parts.is_empty() {
            let date_only: String = download.downloaded_at.chars().take(10).collect();
            let metadata_str = escape_markdown(&metadata_parts.join(" · "));
            text.push_str(&format!("└ {} · {}\n\n", metadata_str, escape_markdown(&date_only)));
        } else {
            let date_only: String = download.downloaded_at.chars().take(10).collect();
            text.push_str(&format!("└ {}\n\n", escape_markdown(&date_only)));
        }
    }

    // Page counter
    if total_pages > 1 {
        text.push_str(&format!("\n_Page {}/{}_", current_page + 1, total_pages));
    }

    // Truncate search_text to 20 chars before embedding in callback_data to
    // prevent Telegram's 64-byte callback_data overflow (CRIT-10).
    let search_short: String = search_text.as_deref().unwrap_or("").chars().take(20).collect();

    // Build keyboard
    let mut keyboard_rows = Vec::new();

    // Each download gets a button to resend
    for download in page_downloads {
        let button_text = format!(
            "📤 {}",
            if download.title.chars().count() > 30 {
                let truncated: String = download.title.chars().take(27).collect();
                format!("{}...", truncated)
            } else {
                download.title.clone()
            }
        );
        keyboard_rows.push(vec![crate::telegram::cb(
            button_text,
            if download.format == "edit" {
                format!("downloads:resend_cut:{}", download.id)
            } else {
                format!("downloads:resend:{}", download.id)
            },
        )]);
    }

    // Navigation row
    let mut nav_buttons = Vec::new();

    if current_page > 0 {
        nav_buttons.push(crate::telegram::cb(
            "⬅️".to_string(),
            format!(
                "downloads:page:{}:{}:{}",
                current_page - 1,
                file_type_filter.as_deref().unwrap_or("all"),
                search_short
            ),
        ));
    }

    if total_pages > 1 {
        nav_buttons.push(crate::telegram::cb(
            format!("{}/{}", current_page + 1, total_pages),
            format!(
                "downloads:page:{}:{}:{}",
                current_page,
                file_type_filter.as_deref().unwrap_or("all"),
                search_short
            ),
        ));
    }

    if current_page < total_pages - 1 {
        nav_buttons.push(crate::telegram::cb(
            "➡️".to_string(),
            format!(
                "downloads:page:{}:{}:{}",
                current_page + 1,
                file_type_filter.as_deref().unwrap_or("all"),
                search_short
            ),
        ));
    }

    if !nav_buttons.is_empty() {
        keyboard_rows.push(nav_buttons);
    }

    // Format filter buttons row
    let mut filter_row = Vec::new();

    if file_type_filter.as_deref() != Some("mp3") {
        filter_row.push(crate::telegram::cb(
            "🎵 MP3".to_string(),
            format!("downloads:filter:mp3:{}", search_short),
        ));
    }

    if file_type_filter.as_deref() != Some("mp4") {
        filter_row.push(crate::telegram::cb(
            "🎬 MP4".to_string(),
            format!("downloads:filter:mp4:{}", search_short),
        ));
    }

    if file_type_filter.as_deref() != Some("edit") {
        filter_row.push(crate::telegram::cb(
            "✂️ Clips".to_string(),
            format!("downloads:filter:edit:{}", search_short),
        ));
    }

    if file_type_filter.is_some() {
        filter_row.push(crate::telegram::cb(
            "🔄 All".to_string(),
            format!("downloads:filter:all:{}", search_short),
        ));
    }

    if !filter_row.is_empty() {
        keyboard_rows.push(filter_row);
    }

    // Category filter buttons row
    let user_cats = shared_storage.get_user_categories(chat_id.0).await.unwrap_or_default();
    if !user_cats.is_empty() {
        let ft_str = file_type_filter.as_deref().unwrap_or("");
        let mut cat_row: Vec<InlineKeyboardButton> = Vec::new();
        for cat in &user_cats {
            let active = category_filter.as_deref() == Some(cat.as_str());
            let label = if active { format!("• {}", cat) } else { cat.clone() };
            // Truncate label to fit Telegram button limits
            let label = if label.chars().count() > 20 {
                let t: String = label.chars().take(18).collect();
                format!("{}…", t)
            } else {
                label
            };
            cat_row.push(crate::telegram::cb(
                label,
                format!(
                    "downloads:catfilter:{}:{}:{}",
                    urlencoding::encode(cat),
                    ft_str,
                    search_short
                ),
            ));
            if cat_row.len() == 3 {
                keyboard_rows.push(std::mem::take(&mut cat_row));
            }
        }
        if !cat_row.is_empty() {
            keyboard_rows.push(cat_row);
        }
        if category_filter.is_some() {
            keyboard_rows.push(vec![crate::telegram::cb(
                "📂 All".to_string(),
                format!("downloads:catfilter::{}:{}", ft_str, search_short),
            )]);
        }
    }

    // Archive button
    keyboard_rows.push(vec![crate::telegram::cb(
        "📦 Create Archive".to_string(),
        "arc:new".to_string(),
    )]);

    // Close button
    keyboard_rows.push(vec![crate::telegram::cb(
        "❌ Close".to_string(),
        "downloads:close".to_string(),
    )]);

    let keyboard = InlineKeyboardMarkup::new(keyboard_rows);

    crate::telegram::styled::send_message_styled_or_fallback(
        bot,
        chat_id,
        &text,
        &keyboard,
        Some(ParseMode::MarkdownV2),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    // ==================== format_file_size tests ====================

    #[test]
    fn test_format_file_size_bytes() {
        assert_eq!(format_file_size(0), "0 B");
        assert_eq!(format_file_size(1), "1 B");
        assert_eq!(format_file_size(500), "500 B");
        assert_eq!(format_file_size(1023), "1023 B");
    }

    #[test]
    fn test_format_file_size_kilobytes() {
        assert_eq!(format_file_size(1024), "1.0 KB");
        assert_eq!(format_file_size(1536), "1.5 KB");
        assert_eq!(format_file_size(102400), "100.0 KB");
    }

    #[test]
    fn test_format_file_size_megabytes() {
        assert_eq!(format_file_size(1024 * 1024), "1.0 MB");
        assert_eq!(format_file_size(1024 * 1024 * 50), "50.0 MB");
        assert_eq!(format_file_size(1024 * 1024 * 512), "512.0 MB");
    }

    #[test]
    fn test_format_file_size_gigabytes() {
        assert_eq!(format_file_size(1024 * 1024 * 1024), "1.00 GB");
        assert_eq!(format_file_size(1024 * 1024 * 1024 * 2), "2.00 GB");
    }

    // ==================== format_duration tests ====================

    #[test]
    fn test_format_duration_seconds_only() {
        assert_eq!(format_duration(0), "0:00");
        assert_eq!(format_duration(5), "0:05");
        assert_eq!(format_duration(30), "0:30");
        assert_eq!(format_duration(59), "0:59");
    }

    #[test]
    fn test_format_duration_minutes() {
        assert_eq!(format_duration(60), "1:00");
        assert_eq!(format_duration(90), "1:30");
        assert_eq!(format_duration(600), "10:00");
        assert_eq!(format_duration(3599), "59:59");
    }

    #[test]
    fn test_format_duration_hours() {
        assert_eq!(format_duration(3600), "1:00:00");
        assert_eq!(format_duration(3661), "1:01:01");
        assert_eq!(format_duration(7200), "2:00:00");
        assert_eq!(format_duration(86399), "23:59:59");
    }

    // ==================== ITEMS_PER_PAGE constant tests ====================

    #[test]
    fn test_items_per_page_value() {
        assert_eq!(ITEMS_PER_PAGE, 5);
    }

    // ==================== is_youtube_url tests ====================

    #[test]
    fn test_is_youtube_url_standard() {
        assert!(is_youtube_url("https://www.youtube.com/watch?v=dQw4w9WgXcQ"));
    }

    #[test]
    fn test_is_youtube_url_no_www() {
        assert!(is_youtube_url("https://youtube.com/watch?v=dQw4w9WgXcQ"));
    }

    #[test]
    fn test_is_youtube_url_mobile() {
        assert!(is_youtube_url("https://m.youtube.com/watch?v=dQw4w9WgXcQ"));
    }

    #[test]
    fn test_is_youtube_url_music() {
        assert!(is_youtube_url("https://music.youtube.com/watch?v=dQw4w9WgXcQ"));
    }

    #[test]
    fn test_is_youtube_url_short_link() {
        assert!(is_youtube_url("https://youtu.be/dQw4w9WgXcQ"));
    }

    #[test]
    fn test_is_youtube_url_shorts() {
        assert!(is_youtube_url("https://www.youtube.com/shorts/dQw4w9WgXcQ"));
    }

    #[test]
    fn test_is_youtube_url_not_youtube_vimeo() {
        assert!(!is_youtube_url("https://vimeo.com/12345678"));
    }

    #[test]
    fn test_is_youtube_url_not_youtube_instagram() {
        assert!(!is_youtube_url("https://www.instagram.com/p/abc123/"));
    }

    #[test]
    fn test_is_youtube_url_false_positive_prevention() {
        // "notyoutube.com" contains "youtube.com" as a substring -- must NOT match
        assert!(!is_youtube_url("https://notyoutube.com/watch?v=abc"));
    }

    #[test]
    fn test_is_youtube_url_false_positive_youtu_be_alike() {
        // domain ending in "youtu.be" but different
        assert!(!is_youtube_url("https://notyoutu.be/abc"));
    }

    #[test]
    fn test_is_youtube_url_empty_string() {
        assert!(!is_youtube_url(""));
    }
}
