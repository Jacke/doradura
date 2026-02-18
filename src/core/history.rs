use crate::core::escape_markdown;
use crate::core::types::Plan;
use crate::storage::db::{self, DbPool};
use crate::telegram::Bot;
use chrono::NaiveDateTime;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{CallbackQueryId, ChatId, InlineKeyboardMarkup, MessageId};
use teloxide::RequestError;
use url::Url;

/// Formats a date for display
fn format_date(date_str: &str) -> String {
    // Parse date from SQLite format (YYYY-MM-DD HH:MM:SS)
    if let Ok(dt) = NaiveDateTime::parse_from_str(date_str, "%Y-%m-%d %H:%M:%S") {
        use chrono::Datelike;
        use chrono::Timelike;
        // Format using short month abbreviations
        format!(
            "{} {}, {:02}:{:02}",
            match dt.month() {
                1 => "Jan",
                2 => "Feb",
                3 => "Mar",
                4 => "Apr",
                5 => "May",
                6 => "Jun",
                7 => "Jul",
                8 => "Aug",
                9 => "Sep",
                10 => "Oct",
                11 => "Nov",
                12 => "Dec",
                _ => "???",
            },
            dt.day(),
            dt.hour(),
            dt.minute()
        )
    } else {
        date_str.to_string()
    }
}

/// Number of entries per history page
const ITEMS_PER_PAGE: usize = 5;

/// Shows the user's download history with pagination
pub async fn show_history(bot: &Bot, chat_id: ChatId, db_pool: Arc<DbPool>) -> ResponseResult<Message> {
    show_history_page(bot, chat_id, db_pool, 0).await
}

/// Shows a specific page of the download history
pub async fn show_history_page(
    bot: &Bot,
    chat_id: ChatId,
    db_pool: Arc<DbPool>,
    page: usize,
) -> ResponseResult<Message> {
    let lang = crate::i18n::user_lang_from_pool(&db_pool, chat_id.0);

    let conn = db::get_connection(&db_pool)
        .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

    // Fetch all history entries to count pages
    let all_entries = match db::get_download_history(&conn, chat_id.0, None) {
        Ok(entries) => entries,
        Err(e) => {
            log::error!("Failed to get download history: {}", e);
            return bot
                .send_message(chat_id, crate::i18n::t(&lang, "history.load_failed"))
                .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                .await;
        }
    };

    if all_entries.is_empty() {
        return bot
            .send_message(
                chat_id,
                "📚 *Download History*\n\nYou have no downloads yet\\. Send me a link to a track or video\\!",
            )
            .parse_mode(teloxide::types::ParseMode::MarkdownV2)
            .await;
    }

    let total_items = all_entries.len();
    let total_pages = total_items.div_ceil(ITEMS_PER_PAGE);
    let current_page = page.min(total_pages - 1);

    let start_idx = current_page * ITEMS_PER_PAGE;
    let end_idx = (start_idx + ITEMS_PER_PAGE).min(total_items);
    let entries = &all_entries[start_idx..end_idx];

    let mut text = format!(
        "📚 *Download History*\n_Page {} of {}_\n\n",
        current_page + 1,
        total_pages
    );
    let mut keyboard_rows = Vec::new();

    for (idx, entry) in entries.iter().enumerate() {
        let format_emoji = match entry.format.as_str() {
            "mp3" => "🎵",
            "mp4" => "🎬",
            "srt" => "📝",
            "txt" => "📄",
            _ => "📦",
        };

        let escaped_title = escape_markdown(&entry.title);
        let formatted_date = format_date(&entry.downloaded_at);
        let escaped_date = escape_markdown(&formatted_date);

        text.push_str(&format!(
            "*{}*\\. {} {}\n📅 {}\n",
            start_idx + idx + 1,
            format_emoji,
            escaped_title,
            escaped_date
        ));

        // Store URL in cache and get a short ID
        let url_id = crate::storage::cache::store_url(&db_pool, &entry.url).await;
        let callback_data = format!("history:repeat:{}:{}", entry.id, url_id);
        let delete_callback = format!("history:delete:{}", entry.id);

        // Shortened track title (first 20 characters)
        let short_title = if entry.title.len() > 20 {
            format!("{}...", &entry.title.chars().take(20).collect::<String>())
        } else {
            entry.title.clone()
        };

        // One row with two buttons: repeat and delete
        keyboard_rows.push(vec![
            crate::telegram::cb(format!("🔄 {}", short_title), callback_data),
            crate::telegram::cb("🗑️".to_string(), delete_callback),
        ]);

        // Add a visual separator between entries (except after the last one)
        if idx < entries.len() - 1 {
            text.push_str("───────────────\n");
        } else {
            text.push('\n');
        }
    }

    // Navigation buttons
    let mut nav_buttons = Vec::new();

    if current_page > 0 {
        nav_buttons.push(crate::telegram::cb(
            "⬅️".to_string(),
            format!("history:page:{}", current_page - 1),
        ));
    }

    // Show page number as an inactive button (callback will not be handled)
    if total_pages > 1 {
        nav_buttons.push(crate::telegram::cb(
            format!("{}/{}", current_page + 1, total_pages),
            format!("history:page:{}", current_page), // Clicking current page does nothing
        ));
    }

    if current_page < total_pages - 1 {
        nav_buttons.push(crate::telegram::cb(
            "➡️".to_string(),
            format!("history:page:{}", current_page + 1),
        ));
    }

    if !nav_buttons.is_empty() {
        keyboard_rows.push(nav_buttons);
    }

    keyboard_rows.push(vec![crate::telegram::cb("🔙 Main menu".to_string(), "back:start")]);

    let keyboard = InlineKeyboardMarkup::new(keyboard_rows);

    bot.send_message(chat_id, text)
        .parse_mode(teloxide::types::ParseMode::MarkdownV2)
        .reply_markup(keyboard)
        .await
}

/// Handles callbacks for the download history
pub async fn handle_history_callback(
    bot: &Bot,
    callback_id: CallbackQueryId,
    chat_id: ChatId,
    message_id: MessageId,
    data: &str,
    db_pool: Arc<DbPool>,
    download_queue: Arc<crate::download::queue::DownloadQueue>,
    rate_limiter: Arc<crate::core::rate_limiter::RateLimiter>,
) -> ResponseResult<()> {
    let lang = crate::i18n::user_lang_from_pool(&db_pool, chat_id.0);

    let parts: Vec<&str> = data.splitn(3, ':').collect();
    if parts.len() < 3 {
        bot.answer_callback_query(callback_id)
            .text(crate::i18n::t(&lang, "history.invalid_format"))
            .await?;
        return Ok(());
    }

    let action = parts[1];

    match action {
        "page" => {
            // Format: history:page:page_number
            let page_str = parts[2];

            match page_str.parse::<usize>() {
                Ok(page) => {
                    // Get current page from message to check
                    // If same page, just acknowledge the callback
                    bot.answer_callback_query(callback_id.clone()).await?;

                    // Delete the current message
                    if let Err(e) = bot.delete_message(chat_id, message_id).await {
                        log::warn!("Failed to delete history message: {:?}", e);
                    }

                    // Show the new page
                    show_history_page(bot, chat_id, db_pool, page).await?;
                }
                Err(e) => {
                    log::error!("Failed to parse page number: {}", e);
                    bot.answer_callback_query(callback_id)
                        .text(crate::i18n::t(&lang, "history.invalid_page"))
                        .await?;
                }
            }
        }
        "repeat" => {
            // Format: history:repeat:entry_id:url_id
            let entry_id_str = parts[2].split(':').next().unwrap_or("");
            let url_id = parts[2].split_once(':').map(|x| x.1).unwrap_or("");

            // First try to resend by file_id if available
            let mut file_sent = false;
            if let Ok(entry_id) = entry_id_str.parse::<i64>() {
                if let Ok(conn) = db::get_connection(&db_pool) {
                    if let Ok(Some(entry)) = db::get_download_history_entry(&conn, chat_id.0, entry_id) {
                        if let Some(file_id) = entry.file_id {
                            log::info!("Found file_id for history entry {}: {}", entry_id, file_id);

                            let result = match entry.format.as_str() {
                                "mp3" => {
                                    bot.send_audio(
                                        chat_id,
                                        teloxide::types::InputFile::file_id(teloxide::types::FileId(file_id.clone())),
                                    )
                                    .await
                                }
                                "mp4" => {
                                    bot.send_video(
                                        chat_id,
                                        teloxide::types::InputFile::file_id(teloxide::types::FileId(file_id.clone())),
                                    )
                                    .await
                                }
                                _ => {
                                    bot.send_document(
                                        chat_id,
                                        teloxide::types::InputFile::file_id(teloxide::types::FileId(file_id)),
                                    )
                                    .await
                                }
                            };

                            match result {
                                Ok(_) => {
                                    log::info!("Successfully resent file using file_id for entry {}", entry_id);
                                    bot.answer_callback_query(callback_id.clone())
                                        .text(crate::i18n::t(&lang, "history.file_sent"))
                                        .await?;
                                    file_sent = true;

                                    // Delete the history message
                                    if let Err(e) = bot.delete_message(chat_id, message_id).await {
                                        log::warn!("Failed to delete history message: {:?}", e);
                                    }
                                }
                                Err(e) => {
                                    log::warn!(
                                        "Failed to resend file using file_id: {}. Falling back to re-download.",
                                        e
                                    );
                                }
                            }
                        }
                    }
                }
            }

            if file_sent {
                return Ok(());
            }

            // Get URL from cache (fallback)
            match crate::storage::cache::get_url(&db_pool, url_id).await {
                Some(url_str) => {
                    // URL found in cache
                    match Url::parse(&url_str) {
                        Ok(url) => {
                            // Get user plan for rate limiting
                            let conn = db::get_connection(&db_pool).map_err(|e| {
                                RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string())))
                            })?;
                            let plan = match db::get_user(&conn, chat_id.0) {
                                Ok(Some(ref user)) => user.plan,
                                _ => Plan::default(),
                            };

                            // Check rate limit
                            if rate_limiter.is_rate_limited(chat_id, plan.as_str()).await {
                                if let Some(remaining_time) = rate_limiter.get_remaining_time(chat_id).await {
                                    let remaining_seconds = remaining_time.as_secs();
                                    let mut args = fluent_templates::fluent_bundle::FluentArgs::new();
                                    args.set("seconds", remaining_seconds);
                                    bot.answer_callback_query(callback_id)
                                        .text(crate::i18n::t_args(&lang, "commands.wait_seconds", &args))
                                        .await?;
                                } else {
                                    bot.answer_callback_query(callback_id)
                                        .text(crate::i18n::t(&lang, "commands.wait"))
                                        .await?;
                                }
                                return Ok(());
                            }

                            bot.answer_callback_query(callback_id.clone()).await?;

                            // Get format from history entry
                            let format = match entry_id_str.parse::<i64>() {
                                Ok(id) => match db::get_download_history_entry(&conn, chat_id.0, id) {
                                    Ok(Some(entry)) => entry.format,
                                    _ => "mp3".to_string(),
                                },
                                Err(_) => "mp3".to_string(),
                            };

                            rate_limiter.update_rate_limit(chat_id, plan.as_str()).await;

                            // Get user preferences for quality/bitrate
                            let video_quality = if format == "mp4" {
                                match db::get_user_video_quality(&conn, chat_id.0) {
                                    Ok(q) => Some(q),
                                    Err(_) => Some("best".to_string()),
                                }
                            } else {
                                None
                            };
                            let audio_bitrate = if format == "mp3" {
                                match db::get_user_audio_bitrate(&conn, chat_id.0) {
                                    Ok(b) => Some(b),
                                    Err(_) => Some("320k".to_string()),
                                }
                            } else {
                                None
                            };

                            // Add task to the download queue
                            let is_video = format == "mp4";
                            let task = crate::download::queue::DownloadTask::from_plan(
                                url.as_str().to_string(),
                                chat_id,
                                None, // Callback doesn't have original user message
                                is_video,
                                format.clone(),
                                video_quality,
                                audio_bitrate,
                                plan.as_str(),
                            );
                            download_queue.add_task(task, Some(Arc::clone(&db_pool))).await;

                            // Delete the history message
                            if let Err(e) = bot.delete_message(chat_id, message_id).await {
                                log::warn!("Failed to delete history message: {:?}", e);
                            }
                        }
                        Err(e) => {
                            log::error!("Failed to parse URL: {}", e);
                            bot.answer_callback_query(callback_id)
                                .text(crate::i18n::t(&lang, "commands.invalid_url"))
                                .await?;
                        }
                    }
                }
                None => {
                    log::warn!("URL not found in cache for id: {} (expired or invalid)", url_id);
                    bot.answer_callback_query(callback_id)
                        .text(crate::i18n::t(&lang, "commands.link_expired"))
                        .await?;
                }
            }
        }
        "delete" => {
            // Format: history:delete:entry_id
            let entry_id_str = parts[2];

            match entry_id_str.parse::<i64>() {
                Ok(entry_id) => {
                    let conn = db::get_connection(&db_pool)
                        .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

                    match db::delete_download_history_entry(&conn, chat_id.0, entry_id) {
                        Ok(true) => {
                            bot.answer_callback_query(callback_id.clone()).await?;

                            // Refresh the history message
                            show_history(bot, chat_id, db_pool).await?;

                            // Delete the old message
                            if let Err(e) = bot.delete_message(chat_id, message_id).await {
                                log::warn!("Failed to delete old history message: {:?}", e);
                            }
                        }
                        Ok(false) => {
                            bot.answer_callback_query(callback_id)
                                .text(crate::i18n::t(&lang, "history.entry_not_found"))
                                .await?;
                        }
                        Err(e) => {
                            log::error!("Failed to delete history entry: {}", e);
                            bot.answer_callback_query(callback_id)
                                .text(crate::i18n::t(&lang, "history.delete_failed"))
                                .await?;
                        }
                    }
                }
                Err(e) => {
                    log::error!("Failed to parse entry ID: {}", e);
                    bot.answer_callback_query(callback_id)
                        .text(crate::i18n::t(&lang, "history.invalid_id"))
                        .await?;
                }
            }
        }
        _ => {
            bot.answer_callback_query(callback_id)
                .text(crate::i18n::t(&lang, "history.unknown_action"))
                .await?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escape_markdown_special_chars() {
        assert_eq!(escape_markdown("test_underscore"), "test\\_underscore");
        assert_eq!(escape_markdown("bold*text"), "bold\\*text");
        assert_eq!(escape_markdown("[link]"), "\\[link\\]");
        assert_eq!(escape_markdown("(parens)"), "\\(parens\\)");
        assert_eq!(escape_markdown("~strike~"), "\\~strike\\~");
        assert_eq!(escape_markdown("`code`"), "\\`code\\`");
        assert_eq!(escape_markdown(">quote"), "\\>quote");
        assert_eq!(escape_markdown("#hash"), "\\#hash");
        assert_eq!(escape_markdown("+plus"), "\\+plus");
        assert_eq!(escape_markdown("-dash"), "\\-dash");
        assert_eq!(escape_markdown("=equals"), "\\=equals");
        assert_eq!(escape_markdown("|pipe"), "\\|pipe");
        assert_eq!(escape_markdown("{brace}"), "\\{brace\\}");
        assert_eq!(escape_markdown("period."), "period\\.");
        assert_eq!(escape_markdown("exclaim!"), "exclaim\\!");
    }

    #[test]
    fn test_escape_markdown_backslash() {
        assert_eq!(escape_markdown("back\\slash"), "back\\\\slash");
    }

    #[test]
    fn test_escape_markdown_normal_text() {
        assert_eq!(escape_markdown("normal text"), "normal text");
        assert_eq!(escape_markdown("Hello World"), "Hello World");
        assert_eq!(escape_markdown("12345"), "12345");
    }

    #[test]
    fn test_escape_markdown_empty_string() {
        assert_eq!(escape_markdown(""), "");
    }

    #[test]
    fn test_escape_markdown_multiple_special_chars() {
        assert_eq!(
            escape_markdown("Hello [World]! How are you?"),
            "Hello \\[World\\]\\! How are you?"
        );
    }

    #[test]
    fn test_escape_markdown_unicode() {
        assert_eq!(escape_markdown("Привет мир"), "Привет мир");
        assert_eq!(escape_markdown("🎵 Music"), "🎵 Music");
    }

    #[test]
    fn test_format_date_valid() {
        assert_eq!(format_date("2024-01-15 10:30:00"), "Jan 15, 10:30");
        assert_eq!(format_date("2024-06-01 00:00:00"), "Jun 1, 00:00");
        assert_eq!(format_date("2024-12-31 23:59:00"), "Dec 31, 23:59");
    }

    #[test]
    fn test_format_date_all_months() {
        assert!(format_date("2024-01-01 12:00:00").starts_with("Jan"));
        assert!(format_date("2024-02-01 12:00:00").starts_with("Feb"));
        assert!(format_date("2024-03-01 12:00:00").starts_with("Mar"));
        assert!(format_date("2024-04-01 12:00:00").starts_with("Apr"));
        assert!(format_date("2024-05-01 12:00:00").starts_with("May"));
        assert!(format_date("2024-06-01 12:00:00").starts_with("Jun"));
        assert!(format_date("2024-07-01 12:00:00").starts_with("Jul"));
        assert!(format_date("2024-08-01 12:00:00").starts_with("Aug"));
        assert!(format_date("2024-09-01 12:00:00").starts_with("Sep"));
        assert!(format_date("2024-10-01 12:00:00").starts_with("Oct"));
        assert!(format_date("2024-11-01 12:00:00").starts_with("Nov"));
        assert!(format_date("2024-12-01 12:00:00").starts_with("Dec"));
    }

    #[test]
    fn test_format_date_invalid() {
        assert_eq!(format_date("not a date"), "not a date");
        assert_eq!(format_date(""), "");
        assert_eq!(format_date("2024-13-01 12:00:00"), "2024-13-01 12:00:00");
    }

    #[test]
    fn test_format_date_midnight() {
        assert_eq!(format_date("2024-01-01 00:00:00"), "Jan 1, 00:00");
    }

    #[test]
    fn test_format_date_end_of_day() {
        assert_eq!(format_date("2024-01-01 23:59:00"), "Jan 1, 23:59");
    }

    #[test]
    fn test_items_per_page_constant() {
        assert_eq!(ITEMS_PER_PAGE, 5);
    }
}
