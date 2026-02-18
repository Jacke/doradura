//! User uploads management - /videos command
//!
//! Handles displaying and managing user-uploaded media files with conversion options.

use crate::conversion::video::{
    calculate_video_note_split, compress, extract_audio, is_too_long_for_split, to_gif, to_video_note,
    to_video_notes_split, CompressionOptions, GifOptions, VideoNoteOptions, VIDEO_NOTE_MAX_DURATION,
};
use crate::core::escape_markdown;
use crate::storage::uploads::{delete_upload, get_upload_by_id, get_uploads_filtered, UploadEntry};
use crate::storage::{db, DbPool};
use crate::telegram::{download_file_from_telegram, Bot};
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{CallbackQueryId, InlineKeyboardButton, InlineKeyboardMarkup, InputFile, MessageId, ParseMode};

const ITEMS_PER_PAGE: usize = 5;

/// Format file size for display
fn format_file_size(bytes: i64) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.2} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

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

/// Get media type icon
fn get_media_icon(media_type: &str) -> &'static str {
    match media_type {
        "photo" => "📷",
        "video" => "🎬",
        "audio" => "🎵",
        "document" => "📄",
        _ => "📎",
    }
}

/// Get media type name in English
fn get_media_type_name(media_type: &str) -> &'static str {
    match media_type {
        "photo" => "Photos",
        "video" => "Videos",
        "audio" => "Audio",
        "document" => "Documents",
        _ => "All",
    }
}

/// Show uploads page for /videos command
pub async fn show_videos_page(
    bot: &Bot,
    chat_id: ChatId,
    db_pool: Arc<DbPool>,
    page: usize,
    media_type_filter: Option<String>,
    search_text: Option<String>,
) -> ResponseResult<Message> {
    let conn = db::get_connection(&db_pool)
        .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

    // Get filtered uploads
    let all_uploads = get_uploads_filtered(&conn, chat_id.0, media_type_filter.as_deref(), search_text.as_deref())
        .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

    if all_uploads.is_empty() {
        let empty_msg = if media_type_filter.is_some() || search_text.is_some() {
            "📭 Nothing found.\n\nTry changing the filters."
        } else {
            "📭 You have no uploaded files yet.\n\nSend me a photo, video, or document and it will appear here!"
        };
        return bot.send_message(chat_id, empty_msg).await;
    }

    let total_items = all_uploads.len();
    let total_pages = total_items.div_ceil(ITEMS_PER_PAGE);
    let current_page = page.min(total_pages.saturating_sub(1));

    let start_idx = current_page * ITEMS_PER_PAGE;
    let end_idx = (start_idx + ITEMS_PER_PAGE).min(total_items);
    let page_uploads = &all_uploads[start_idx..end_idx];

    // Build message text
    let mut text = String::from("📂 *Your uploads*\n\n");

    // Show active filters
    if let Some(ref mt) = media_type_filter {
        let icon = get_media_icon(mt);
        let filter_name = get_media_type_name(mt);
        text.push_str(&format!("Filter: {} {}\n\n", icon, filter_name));
    }
    if let Some(ref search) = search_text {
        text.push_str(&format!("🔍 Search: \"{}\"\n\n", escape_markdown(search)));
    }

    // List uploads
    for upload in page_uploads {
        let icon = get_media_icon(&upload.media_type);
        text.push_str(&format!("{} *{}*\n", icon, escape_markdown(&upload.title)));

        // Format metadata
        let mut metadata_parts = Vec::new();

        if let Some(size) = upload.file_size {
            metadata_parts.push(format_file_size(size));
        }

        if let Some(dur) = upload.duration {
            metadata_parts.push(format_duration(dur));
        }

        if let Some(w) = upload.width {
            if let Some(h) = upload.height {
                metadata_parts.push(format!("{}x{}", w, h));
            }
        }

        if !metadata_parts.is_empty() {
            let date_only: String = upload.uploaded_at.chars().take(10).collect();
            let metadata_str = escape_markdown(&metadata_parts.join(" · "));
            text.push_str(&format!("└ {} · {}\n\n", metadata_str, escape_markdown(&date_only)));
        } else {
            let date_only: String = upload.uploaded_at.chars().take(10).collect();
            text.push_str(&format!("└ {}\n\n", escape_markdown(&date_only)));
        }
    }

    // Page counter
    if total_pages > 1 {
        text.push_str(&format!("\n_Page {}/{}_", current_page + 1, total_pages));
    }

    // Build keyboard
    let mut keyboard_rows = Vec::new();

    // Each upload gets a button to open actions menu
    for upload in page_uploads {
        let button_text = format!(
            "{} {}",
            get_media_icon(&upload.media_type),
            if upload.title.chars().count() > 25 {
                let truncated: String = upload.title.chars().take(22).collect();
                format!("{}...", truncated)
            } else {
                upload.title.clone()
            }
        );
        keyboard_rows.push(vec![crate::telegram::cb(
            button_text,
            format!("videos:open:{}", upload.id),
        )]);
    }

    // Navigation row
    let mut nav_buttons = Vec::new();

    if current_page > 0 {
        nav_buttons.push(crate::telegram::cb(
            "⬅️".to_string(),
            format!(
                "videos:page:{}:{}:{}",
                current_page - 1,
                media_type_filter.as_deref().unwrap_or("all"),
                search_text.as_deref().unwrap_or("")
            ),
        ));
    }

    if total_pages > 1 {
        nav_buttons.push(crate::telegram::cb(
            format!("{}/{}", current_page + 1, total_pages),
            format!(
                "videos:page:{}:{}:{}",
                current_page,
                media_type_filter.as_deref().unwrap_or("all"),
                search_text.as_deref().unwrap_or("")
            ),
        ));
    }

    if current_page < total_pages - 1 {
        nav_buttons.push(crate::telegram::cb(
            "➡️".to_string(),
            format!(
                "videos:page:{}:{}:{}",
                current_page + 1,
                media_type_filter.as_deref().unwrap_or("all"),
                search_text.as_deref().unwrap_or("")
            ),
        ));
    }

    if !nav_buttons.is_empty() {
        keyboard_rows.push(nav_buttons);
    }

    // Filter buttons row
    let mut filter_row = Vec::new();

    if media_type_filter.as_deref() != Some("video") {
        filter_row.push(crate::telegram::cb(
            "🎬 Videos".to_string(),
            format!("videos:filter:video:{}", search_text.as_deref().unwrap_or("")),
        ));
    }

    if media_type_filter.as_deref() != Some("photo") {
        filter_row.push(crate::telegram::cb(
            "📷 Photos".to_string(),
            format!("videos:filter:photo:{}", search_text.as_deref().unwrap_or("")),
        ));
    }

    if media_type_filter.as_deref() != Some("document") {
        filter_row.push(crate::telegram::cb(
            "📄 Documents".to_string(),
            format!("videos:filter:document:{}", search_text.as_deref().unwrap_or("")),
        ));
    }

    if media_type_filter.is_some() {
        filter_row.push(crate::telegram::cb(
            "🔄 All".to_string(),
            format!("videos:filter:all:{}", search_text.as_deref().unwrap_or("")),
        ));
    }

    if !filter_row.is_empty() {
        keyboard_rows.push(filter_row);
    }

    // Close button
    keyboard_rows.push(vec![crate::telegram::cb(
        "❌ Close".to_string(),
        "videos:close".to_string(),
    )]);

    let keyboard = InlineKeyboardMarkup::new(keyboard_rows);

    bot.send_message(chat_id, text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(keyboard)
        .await
}

/// Build Level 1 action keyboard for a specific upload (category selection)
fn build_upload_action_keyboard(upload: &UploadEntry) -> InlineKeyboardMarkup {
    let mut rows = Vec::new();

    match upload.media_type.as_str() {
        "video" => {
            // Video: Send submenu + Convert submenu
            rows.push(vec![
                crate::telegram::cb("📤 Send".to_string(), format!("videos:submenu:send:{}", upload.id)),
                crate::telegram::cb(
                    "🔄 Convert".to_string(),
                    format!("videos:submenu:convert:{}", upload.id),
                ),
            ]);
        }
        "photo" | "audio" => {
            // Photo/Audio: Send submenu only (no conversions yet)
            rows.push(vec![crate::telegram::cb(
                "📤 Send".to_string(),
                format!("videos:submenu:send:{}", upload.id),
            )]);
        }
        _ => {
            // Document: Send directly (single option, no submenu)
            rows.push(vec![crate::telegram::cb(
                "📤 Send".to_string(),
                format!("videos:send:document:{}", upload.id),
            )]);
        }
    }

    // Delete + Cancel
    rows.push(vec![
        crate::telegram::cb("🗑️ Delete".to_string(), format!("videos:delete:{}", upload.id)),
        crate::telegram::cb("❌ Cancel".to_string(), "videos:cancel".to_string()),
    ]);

    InlineKeyboardMarkup::new(rows)
}

/// Build Level 2 send submenu keyboard
fn build_send_submenu_keyboard(upload: &UploadEntry) -> InlineKeyboardMarkup {
    let mut rows = Vec::new();

    match upload.media_type.as_str() {
        "video" => {
            rows.push(vec![
                crate::telegram::cb("📤 Video".to_string(), format!("videos:send:video:{}", upload.id)),
                crate::telegram::cb("📎 Document".to_string(), format!("videos:send:document:{}", upload.id)),
            ]);
        }
        "photo" => {
            rows.push(vec![
                crate::telegram::cb("📤 Photo".to_string(), format!("videos:send:photo:{}", upload.id)),
                crate::telegram::cb("📎 Document".to_string(), format!("videos:send:document:{}", upload.id)),
            ]);
        }
        "audio" => {
            rows.push(vec![
                crate::telegram::cb("📤 Audio".to_string(), format!("videos:send:audio:{}", upload.id)),
                crate::telegram::cb("📎 Document".to_string(), format!("videos:send:document:{}", upload.id)),
            ]);
        }
        _ => {
            rows.push(vec![crate::telegram::cb(
                "📤 Send".to_string(),
                format!("videos:send:document:{}", upload.id),
            )]);
        }
    }

    // Back button
    rows.push(vec![crate::telegram::cb(
        "⬅️ Back".to_string(),
        format!("videos:open:{}", upload.id),
    )]);

    InlineKeyboardMarkup::new(rows)
}

/// Build Level 2 convert submenu keyboard (video only)
fn build_convert_submenu_keyboard(upload: &UploadEntry) -> InlineKeyboardMarkup {
    let rows = vec![
        vec![
            crate::telegram::cb("⭕️ Circle".to_string(), format!("videos:convert:circle:{}", upload.id)),
            crate::telegram::cb("🎵 MP3".to_string(), format!("videos:convert:audio:{}", upload.id)),
        ],
        vec![
            crate::telegram::cb("🎞️ GIF".to_string(), format!("videos:convert:gif:{}", upload.id)),
            crate::telegram::cb(
                "📦 Compress".to_string(),
                format!("videos:convert:compress:{}", upload.id),
            ),
        ],
        // Back button
        vec![crate::telegram::cb(
            "⬅️ Back".to_string(),
            format!("videos:open:{}", upload.id),
        )],
    ];

    InlineKeyboardMarkup::new(rows)
}

/// Build upload info text for Level 1 display
fn build_upload_info_text(upload: &UploadEntry) -> String {
    let icon = get_media_icon(&upload.media_type);

    let mut info_parts = Vec::new();
    if let Some(size) = upload.file_size {
        info_parts.push(format_file_size(size));
    }
    if let Some(dur) = upload.duration {
        info_parts.push(format_duration(dur));
    }
    if let Some(w) = upload.width {
        if let Some(h) = upload.height {
            info_parts.push(format!("{}x{}", w, h));
        }
    }

    let info_str = if info_parts.is_empty() {
        String::new()
    } else {
        format!("\n└ {}", escape_markdown(&info_parts.join(" · ")))
    };

    format!(
        "{} *File:* {}{}\n\nWhat to do?",
        icon,
        escape_markdown(&upload.title),
        info_str
    )
}

/// Handle videos callback queries
pub async fn handle_videos_callback(
    bot: &Bot,
    callback_id: CallbackQueryId,
    chat_id: ChatId,
    message_id: MessageId,
    data: &str,
    db_pool: Arc<DbPool>,
) -> ResponseResult<()> {
    log::info!("📂 handle_videos_callback called with data: {}", data);
    bot.answer_callback_query(callback_id).await?;

    let parts: Vec<&str> = data.splitn(5, ':').collect();
    if parts.len() < 2 {
        return Ok(());
    }

    let action = parts[1];
    log::info!("📂 Videos action: {}", action);

    match action {
        "page" => {
            if parts.len() < 5 {
                return Ok(());
            }
            let page = parts[2].parse::<usize>().unwrap_or(0);
            let filter = if parts[3] == "all" {
                None
            } else {
                Some(parts[3].to_string())
            };
            let search = if parts[4].is_empty() {
                None
            } else {
                Some(parts[4].to_string())
            };

            bot.delete_message(chat_id, message_id).await?;
            show_videos_page(bot, chat_id, db_pool.clone(), page, filter, search).await?;
        }
        "filter" => {
            if parts.len() < 4 {
                return Ok(());
            }
            let filter = if parts[2] == "all" {
                None
            } else {
                Some(parts[2].to_string())
            };
            let search = if parts[3].is_empty() {
                None
            } else {
                Some(parts[3].to_string())
            };

            bot.delete_message(chat_id, message_id).await?;
            show_videos_page(bot, chat_id, db_pool.clone(), 0, filter, search).await?;
        }
        "open" => {
            if parts.len() < 3 {
                return Ok(());
            }
            let upload_id = parts[2].parse::<i64>().unwrap_or(0);

            let conn = db::get_connection(&db_pool)
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

            if let Some(upload) = get_upload_by_id(&conn, chat_id.0, upload_id)
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
            {
                let text = build_upload_info_text(&upload);
                let keyboard = build_upload_action_keyboard(&upload);

                // Try edit first (for "Back" navigation from submenu)
                let edit_result = bot
                    .edit_message_text(chat_id, message_id, &text)
                    .parse_mode(ParseMode::MarkdownV2)
                    .reply_markup(keyboard.clone())
                    .await;

                if edit_result.is_err() {
                    // Fall back to send + delete (for /videos list click)
                    bot.send_message(chat_id, text)
                        .parse_mode(ParseMode::MarkdownV2)
                        .reply_markup(keyboard)
                        .await?;
                    bot.delete_message(chat_id, message_id).await.ok();
                }
            }
        }
        "submenu" => {
            // videos:submenu:{type}:{upload_id}
            if parts.len() < 4 {
                return Ok(());
            }
            let submenu_type = parts[2];
            let upload_id = parts[3].parse::<i64>().unwrap_or(0);

            let conn = db::get_connection(&db_pool)
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

            if let Some(upload) = get_upload_by_id(&conn, chat_id.0, upload_id)
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
            {
                let (text, keyboard) = match submenu_type {
                    "send" => {
                        let text = format!("📤 *Send* _{}_*:*", escape_markdown(&upload.title));
                        (text, build_send_submenu_keyboard(&upload))
                    }
                    "convert" => {
                        let text = format!("🔄 *Convert* _{}_*:*", escape_markdown(&upload.title));
                        (text, build_convert_submenu_keyboard(&upload))
                    }
                    _ => return Ok(()),
                };

                bot.edit_message_text(chat_id, message_id, text)
                    .parse_mode(ParseMode::MarkdownV2)
                    .reply_markup(keyboard)
                    .await?;
            } else {
                bot.edit_message_text(chat_id, message_id, "❌ File not found").await?;
            }
        }
        "send" => {
            if parts.len() < 4 {
                return Ok(());
            }
            let send_type = parts[2];
            let upload_id = parts[3].parse::<i64>().unwrap_or(0);

            let conn = db::get_connection(&db_pool)
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

            if let Some(upload) = get_upload_by_id(&conn, chat_id.0, upload_id)
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
            {
                let file_id = teloxide::types::FileId(upload.file_id.clone());
                let caption = upload.title.clone();

                let status_msg = bot.send_message(chat_id, "⏳ Sending file...").await?;

                let send_result = match send_type {
                    "video" => {
                        bot.send_video(chat_id, InputFile::file_id(file_id))
                            .caption(caption)
                            .await
                    }
                    "photo" => {
                        bot.send_photo(chat_id, InputFile::file_id(file_id))
                            .caption(caption)
                            .await
                    }
                    "audio" => {
                        bot.send_audio(chat_id, InputFile::file_id(file_id))
                            .caption(caption)
                            .await
                    }
                    "document" => {
                        bot.send_document(chat_id, InputFile::file_id(file_id))
                            .caption(caption)
                            .await
                    }
                    _ => {
                        bot.delete_message(chat_id, status_msg.id).await.ok();
                        return Ok(());
                    }
                };

                bot.delete_message(chat_id, status_msg.id).await.ok();

                match send_result {
                    Ok(_) => {
                        bot.delete_message(chat_id, message_id).await.ok();
                    }
                    Err(e) => {
                        bot.send_message(chat_id, format!("❌ Failed to send file: {}", e))
                            .await
                            .ok();
                    }
                }
            }
        }
        "delete" => {
            if parts.len() < 3 {
                return Ok(());
            }
            let upload_id = parts[2].parse::<i64>().unwrap_or(0);

            let conn = db::get_connection(&db_pool)
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

            // Confirm deletion
            let keyboard = InlineKeyboardMarkup::new(vec![vec![
                crate::telegram::cb(
                    "✅ Yes, delete".to_string(),
                    format!("videos:confirm_delete:{}", upload_id),
                ),
                crate::telegram::cb("❌ Cancel".to_string(), "videos:cancel".to_string()),
            ]]);

            if let Some(upload) = get_upload_by_id(&conn, chat_id.0, upload_id)
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
            {
                bot.edit_message_text(
                    chat_id,
                    message_id,
                    format!("🗑️ Delete *{}*?", escape_markdown(&upload.title)),
                )
                .parse_mode(ParseMode::MarkdownV2)
                .reply_markup(keyboard)
                .await?;
            }
        }
        "confirm_delete" => {
            if parts.len() < 3 {
                return Ok(());
            }
            let upload_id = parts[2].parse::<i64>().unwrap_or(0);

            let conn = db::get_connection(&db_pool)
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

            match delete_upload(&conn, chat_id.0, upload_id) {
                Ok(true) => {
                    bot.delete_message(chat_id, message_id).await.ok();
                    bot.send_message(chat_id, "✅ File deleted").await?;
                }
                Ok(false) => {
                    bot.send_message(chat_id, "❌ File not found").await?;
                }
                Err(e) => {
                    bot.send_message(chat_id, format!("❌ Error deleting: {}", e)).await?;
                }
            }
        }
        "convert" => {
            if parts.len() < 4 {
                return Ok(());
            }
            let convert_type = parts[2];
            let upload_id = parts[3].parse::<i64>().unwrap_or(0);

            let conn = db::get_connection(&db_pool)
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

            if let Some(upload) = get_upload_by_id(&conn, chat_id.0, upload_id)
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
            {
                match convert_type {
                    "circle" => {
                        // Show duration selection for video note
                        let video_duration = upload.duration.unwrap_or(60) as u64;
                        let durations = [5, 10, 15, 20, 25, 30, 35, 40, 45, 50, 55, 60];
                        let mut rows: Vec<Vec<InlineKeyboardButton>> = vec![];
                        let mut current_row: Vec<InlineKeyboardButton> = vec![];

                        for dur in durations {
                            let button = crate::telegram::cb(
                                format!("{}s", dur),
                                format!("videos:circle_speed:{}:{}", upload_id, dur),
                            );
                            current_row.push(button);

                            if current_row.len() == 4 {
                                rows.push(current_row);
                                current_row = vec![];
                            }
                        }

                        if !current_row.is_empty() {
                            rows.push(current_row);
                        }

                        // Add "Full video" option for videos longer than 60s (splits into multiple circles)
                        if video_duration > VIDEO_NOTE_MAX_DURATION {
                            if let Some(split_info) = calculate_video_note_split(video_duration) {
                                let full_video_label = format!("📼 Full video ({} circles)", split_info.num_parts);
                                rows.push(vec![crate::telegram::cb(
                                    full_video_label,
                                    format!("videos:circle_speed:{}:{}", upload_id, video_duration),
                                )]);
                            } else if is_too_long_for_split(video_duration) {
                                // Video too long - show warning button (disabled)
                                rows.push(vec![crate::telegram::cb(
                                    "⚠️ Video is too long (max 6 min)".to_string(),
                                    "videos:noop".to_string(),
                                )]);
                            }
                        }

                        rows.push(vec![crate::telegram::cb(
                            "❌ Cancel".to_string(),
                            "videos:cancel".to_string(),
                        )]);

                        let keyboard = InlineKeyboardMarkup::new(rows);

                        // Build status message based on video duration
                        let status_text = if video_duration > VIDEO_NOTE_MAX_DURATION {
                            if is_too_long_for_split(video_duration) {
                                format!(
                                    "⭕️ *Choose circle duration* for *{}*:\n\n⚠️ Video is longer than 6 minutes — only circles up to 60s can be created\\.\n\nOr send an interval in the format `mm:ss\\-mm:ss`\\.",
                                    escape_markdown(&upload.title)
                                )
                            } else {
                                let split_info = calculate_video_note_split(video_duration);
                                let num_circles = split_info.map(|s| s.num_parts).unwrap_or(1);
                                format!(
                                    "⭕️ *Choose circle duration* for *{}*:\n\n💡 Video is longer than 60s — can create {} circles\\.\n\nOr send an interval in the format `mm:ss\\-mm:ss`\\.",
                                    escape_markdown(&upload.title),
                                    num_circles
                                )
                            }
                        } else {
                            format!(
                                "⭕️ *Choose circle duration* for *{}*:\n\nOr send an interval in the format `mm:ss\\-mm:ss`\\.",
                                escape_markdown(&upload.title)
                            )
                        };

                        bot.edit_message_text(chat_id, message_id, status_text)
                            .parse_mode(ParseMode::MarkdownV2)
                            .reply_markup(keyboard)
                            .await?;
                    }
                    "audio" | "gif" | "compress" => {
                        // Route to working conversion handler
                        let upload_id = parts.get(3).unwrap_or(&"0");
                        let convert_data = format!("convert:{}:{}", convert_type, upload_id);
                        handle_convert_callback(bot, chat_id, message_id, &convert_data, db_pool.clone()).await?;
                    }
                    _ => {}
                }
            }
        }
        "circle_speed" => {
            // videos:circle_speed:{upload_id}:{duration}
            if parts.len() < 4 {
                return Ok(());
            }
            let upload_id = parts[2];
            let duration = parts[3];

            let speeds = [
                ("x1", "1"),
                ("x1.2", "1.2"),
                ("x1.5", "1.5"),
                ("x1.8", "1.8"),
                ("x2", "2"),
            ];

            let speed_row: Vec<InlineKeyboardButton> = speeds
                .iter()
                .map(|(label, val)| {
                    crate::telegram::cb(
                        label.to_string(),
                        format!("convert:circle:{}:{}:{}", upload_id, duration, val),
                    )
                })
                .collect();

            let keyboard = InlineKeyboardMarkup::new(vec![
                speed_row,
                vec![crate::telegram::cb(
                    "⬅️ Back".to_string(),
                    format!("videos:convert:circle:{}", upload_id),
                )],
            ]);

            bot.edit_message_text(
                chat_id,
                message_id,
                format!("⚡ *Choose circle speed* \\({}s\\):", escape_markdown(duration)),
            )
            .parse_mode(ParseMode::MarkdownV2)
            .reply_markup(keyboard)
            .await?;
        }
        "cancel" | "close" => {
            bot.delete_message(chat_id, message_id).await.ok();
        }
        _ => {
            log::warn!("Unknown videos callback action: {}", action);
        }
    }

    // Handle convert: prefix callbacks (e.g., convert:circle:123:30)
    if data.starts_with("convert:") {
        handle_convert_callback(bot, chat_id, message_id, data, db_pool).await?;
    }

    Ok(())
}

/// Handle actual conversion callbacks (convert:circle:upload_id:duration, etc.)
async fn handle_convert_callback(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    data: &str,
    db_pool: Arc<DbPool>,
) -> ResponseResult<()> {
    let parts: Vec<&str> = data.split(':').collect();
    if parts.len() < 3 {
        return Ok(());
    }

    let convert_type = parts[1];
    log::info!("📼 Convert callback: type={}, data={}", convert_type, data);

    let conn = db::get_connection(&db_pool)
        .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

    match convert_type {
        "circle" => {
            // Format: convert:circle:upload_id:duration:speed
            if parts.len() < 4 {
                return Ok(());
            }
            let upload_id = parts[2].parse::<i64>().unwrap_or(0);
            let duration = parts[3].parse::<u64>().unwrap_or(30);
            let speed: Option<f64> = parts.get(4).and_then(|s| s.parse().ok()).filter(|&s: &f64| s != 1.0);

            if let Some(upload) = get_upload_by_id(&conn, chat_id.0, upload_id)
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
            {
                bot.delete_message(chat_id, message_id).await.ok();

                // Check if video is too long for splitting (> 360s)
                if is_too_long_for_split(duration) {
                    bot.send_message(
                        chat_id,
                        "❌ Video is too long\\. Maximum 6 minutes for splitting into circles\\.",
                    )
                    .parse_mode(ParseMode::MarkdownV2)
                    .await?;
                    return Ok(());
                }

                // Calculate split info for status message
                let split_info = calculate_video_note_split(duration);
                let num_circles = split_info.as_ref().map(|s| s.num_parts).unwrap_or(1);

                let speed_label = speed.map(|s| format!(" x{}", s)).unwrap_or_default();
                let status_text = if num_circles > 1 {
                    format!(
                        "⏳ Creating {} circles{} from *{}*\\.\\.\\.\n\n_This may take a few minutes_",
                        num_circles,
                        escape_markdown(&speed_label),
                        escape_markdown(&upload.title)
                    )
                } else {
                    format!(
                        "⏳ Creating circle{} from *{}*\\.\\.\\.\n\n_This may take a few minutes_",
                        escape_markdown(&speed_label),
                        escape_markdown(&upload.title)
                    )
                };

                let status_msg = bot
                    .send_message(chat_id, status_text)
                    .parse_mode(ParseMode::MarkdownV2)
                    .await?;

                // Download file from Telegram
                let temp_input = match download_file_from_telegram(bot, &upload.file_id, None).await {
                    Ok(path) => path,
                    Err(e) => {
                        bot.edit_message_text(chat_id, status_msg.id, format!("❌ Failed to download file: {}", e))
                            .await?;
                        return Ok(());
                    }
                };

                // Check if we need to split into multiple circles
                if duration > VIDEO_NOTE_MAX_DURATION {
                    // Create multiple video notes
                    match to_video_notes_split(&temp_input, duration, speed).await {
                        Ok(output_paths) => {
                            let total = output_paths.len();
                            for (i, output_path) in output_paths.iter().enumerate() {
                                let progress_text = format!("📤 Sending circle {}/{}...", i + 1, total);
                                bot.edit_message_text(chat_id, status_msg.id, &progress_text).await.ok();

                                // Calculate duration for this part
                                let part_duration = if i == total - 1 {
                                    // Last part may be shorter
                                    duration - (i as u64 * VIDEO_NOTE_MAX_DURATION)
                                } else {
                                    VIDEO_NOTE_MAX_DURATION
                                };

                                match bot
                                    .send_video_note(chat_id, InputFile::file(output_path))
                                    .duration(part_duration.min(VIDEO_NOTE_MAX_DURATION) as u32)
                                    .length(640)
                                    .await
                                {
                                    Ok(_) => {}
                                    Err(e) => {
                                        log::error!("Failed to send video note {}/{}: {}", i + 1, total, e);
                                        bot.edit_message_text(
                                            chat_id,
                                            status_msg.id,
                                            format!("❌ Failed to send circle {}/{}: {}", i + 1, total, e),
                                        )
                                        .await?;
                                        // Clean up remaining files
                                        for path in &output_paths {
                                            tokio::fs::remove_file(path).await.ok();
                                        }
                                        tokio::fs::remove_file(&temp_input).await.ok();
                                        return Ok(());
                                    }
                                }
                            }

                            // Success - clean up status message
                            bot.delete_message(chat_id, status_msg.id).await.ok();

                            // Clean up all output files
                            for path in &output_paths {
                                tokio::fs::remove_file(path).await.ok();
                            }
                        }
                        Err(e) => {
                            bot.edit_message_text(chat_id, status_msg.id, format!("❌ Error creating circles: {}", e))
                                .await?;
                        }
                    }
                } else {
                    // Single video note (original logic)
                    let options = VideoNoteOptions {
                        duration: Some(duration),
                        start_time: None,
                        speed,
                    };

                    match to_video_note(&temp_input, options).await {
                        Ok(output_path) => {
                            bot.edit_message_text(chat_id, status_msg.id, "📤 Sending circle...")
                                .await
                                .ok();

                            match bot
                                .send_video_note(chat_id, InputFile::file(&output_path))
                                .duration(duration.min(VIDEO_NOTE_MAX_DURATION) as u32)
                                .length(640)
                                .await
                            {
                                Ok(_) => {
                                    bot.delete_message(chat_id, status_msg.id).await.ok();
                                }
                                Err(e) => {
                                    bot.edit_message_text(
                                        chat_id,
                                        status_msg.id,
                                        format!("❌ Failed to send circle: {}", e),
                                    )
                                    .await?;
                                }
                            }

                            tokio::fs::remove_file(&output_path).await.ok();
                        }
                        Err(e) => {
                            bot.edit_message_text(chat_id, status_msg.id, format!("❌ Error creating circle: {}", e))
                                .await?;
                        }
                    }
                }

                // Clean up input file
                tokio::fs::remove_file(&temp_input).await.ok();
            }
        }
        "audio" => {
            // Format: convert:audio:upload_id
            if parts.len() < 3 {
                return Ok(());
            }
            let upload_id = parts[2].parse::<i64>().unwrap_or(0);

            if let Some(upload) = get_upload_by_id(&conn, chat_id.0, upload_id)
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
            {
                bot.delete_message(chat_id, message_id).await.ok();

                let status_msg = bot
                    .send_message(
                        chat_id,
                        format!("⏳ Extracting audio from *{}*\\.\\.\\.", escape_markdown(&upload.title)),
                    )
                    .parse_mode(ParseMode::MarkdownV2)
                    .await?;

                // Download file from Telegram
                let temp_input = match download_file_from_telegram(bot, &upload.file_id, None).await {
                    Ok(path) => path,
                    Err(e) => {
                        bot.edit_message_text(chat_id, status_msg.id, format!("❌ Failed to download file: {}", e))
                            .await?;
                        return Ok(());
                    }
                };

                // Extract audio
                match extract_audio(&temp_input, "320k").await {
                    Ok(output_path) => {
                        bot.edit_message_text(chat_id, status_msg.id, "📤 Sending audio...")
                            .await
                            .ok();

                        // Create audio title from video title
                        let audio_title = upload
                            .title
                            .strip_suffix(".mp4")
                            .or_else(|| upload.title.strip_suffix(".MP4"))
                            .unwrap_or(&upload.title)
                            .to_string();

                        match bot
                            .send_audio(chat_id, InputFile::file(&output_path))
                            .title(audio_title)
                            .await
                        {
                            Ok(_) => {
                                bot.delete_message(chat_id, status_msg.id).await.ok();
                            }
                            Err(e) => {
                                bot.edit_message_text(
                                    chat_id,
                                    status_msg.id,
                                    format!("❌ Failed to send audio: {}", e),
                                )
                                .await?;
                            }
                        }

                        // Clean up
                        tokio::fs::remove_file(&output_path).await.ok();
                    }
                    Err(e) => {
                        bot.edit_message_text(chat_id, status_msg.id, format!("❌ Error extracting audio: {}", e))
                            .await?;
                    }
                }

                // Clean up input file
                tokio::fs::remove_file(&temp_input).await.ok();
            }
        }
        "gif" => {
            // Format: convert:gif:upload_id
            if parts.len() < 3 {
                return Ok(());
            }
            let upload_id = parts[2].parse::<i64>().unwrap_or(0);

            if let Some(upload) = get_upload_by_id(&conn, chat_id.0, upload_id)
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
            {
                bot.delete_message(chat_id, message_id).await.ok();

                let status_msg = bot
                    .send_message(
                        chat_id,
                        format!(
                            "⏳ Creating GIF from *{}*\\.\\.\\.\n\n_This may take some time_",
                            escape_markdown(&upload.title)
                        ),
                    )
                    .parse_mode(ParseMode::MarkdownV2)
                    .await?;

                // Download file from Telegram
                let temp_input = match download_file_from_telegram(bot, &upload.file_id, None).await {
                    Ok(path) => path,
                    Err(e) => {
                        bot.edit_message_text(chat_id, status_msg.id, format!("❌ Failed to download file: {}", e))
                            .await?;
                        return Ok(());
                    }
                };

                // Convert to GIF
                let options = GifOptions::default();

                match to_gif(&temp_input, options).await {
                    Ok(output_path) => {
                        bot.edit_message_text(chat_id, status_msg.id, "📤 Sending GIF...")
                            .await
                            .ok();

                        // Send as animation
                        match bot.send_animation(chat_id, InputFile::file(&output_path)).await {
                            Ok(_) => {
                                bot.delete_message(chat_id, status_msg.id).await.ok();
                            }
                            Err(e) => {
                                bot.edit_message_text(chat_id, status_msg.id, format!("❌ Failed to send GIF: {}", e))
                                    .await?;
                            }
                        }

                        // Clean up
                        tokio::fs::remove_file(&output_path).await.ok();
                    }
                    Err(e) => {
                        bot.edit_message_text(chat_id, status_msg.id, format!("❌ Error creating GIF: {}", e))
                            .await?;
                    }
                }

                // Clean up input file
                tokio::fs::remove_file(&temp_input).await.ok();
            }
        }
        "compress" => {
            // Format: convert:compress:upload_id
            if parts.len() < 3 {
                return Ok(());
            }
            let upload_id = parts[2].parse::<i64>().unwrap_or(0);

            if let Some(upload) = get_upload_by_id(&conn, chat_id.0, upload_id)
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
            {
                bot.delete_message(chat_id, message_id).await.ok();

                let status_msg = bot
                    .send_message(
                        chat_id,
                        format!(
                            "⏳ Compressing *{}*\\.\\.\\.\n\n_This may take a few minutes_",
                            escape_markdown(&upload.title)
                        ),
                    )
                    .parse_mode(ParseMode::MarkdownV2)
                    .await?;

                // Download file from Telegram
                let temp_input = match download_file_from_telegram(bot, &upload.file_id, None).await {
                    Ok(path) => path,
                    Err(e) => {
                        bot.edit_message_text(chat_id, status_msg.id, format!("❌ Failed to download file: {}", e))
                            .await?;
                        return Ok(());
                    }
                };

                // Get original file size for comparison
                let original_size = tokio::fs::metadata(&temp_input).await.map(|m| m.len()).unwrap_or(0);

                // Compress video
                let options = CompressionOptions::default();

                match compress(&temp_input, options).await {
                    Ok(output_path) => {
                        let compressed_size = tokio::fs::metadata(&output_path).await.map(|m| m.len()).unwrap_or(0);

                        let size_reduction = if original_size > 0 {
                            ((original_size - compressed_size) as f64 / original_size as f64) * 100.0
                        } else {
                            0.0
                        };

                        bot.edit_message_text(
                            chat_id,
                            status_msg.id,
                            format!(
                                "📤 Sending compressed video...\n({} → {}, -{:.0}%)",
                                format_file_size(original_size as i64),
                                format_file_size(compressed_size as i64),
                                size_reduction
                            ),
                        )
                        .await
                        .ok();

                        match bot
                            .send_video(chat_id, InputFile::file(&output_path))
                            .caption(format!("{} (compressed, -{:.0}%)", upload.title, size_reduction))
                            .await
                        {
                            Ok(_) => {
                                bot.delete_message(chat_id, status_msg.id).await.ok();
                            }
                            Err(e) => {
                                bot.edit_message_text(
                                    chat_id,
                                    status_msg.id,
                                    format!("❌ Failed to send video: {}", e),
                                )
                                .await?;
                            }
                        }

                        // Clean up
                        tokio::fs::remove_file(&output_path).await.ok();
                    }
                    Err(e) => {
                        bot.edit_message_text(chat_id, status_msg.id, format!("❌ Error compressing video: {}", e))
                            .await?;
                    }
                }

                // Clean up input file
                tokio::fs::remove_file(&temp_input).await.ok();
            }
        }
        _ => {
            log::warn!("Unknown convert type: {}", convert_type);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_file_size() {
        assert_eq!(format_file_size(500), "500 B");
        assert_eq!(format_file_size(1024), "1.0 KB");
        assert_eq!(format_file_size(1024 * 1024), "1.0 MB");
        assert_eq!(format_file_size(1024 * 1024 * 1024), "1.00 GB");
    }

    #[test]
    fn test_format_duration() {
        assert_eq!(format_duration(0), "0:00");
        assert_eq!(format_duration(65), "1:05");
        assert_eq!(format_duration(3661), "1:01:01");
    }

    #[test]
    fn test_get_media_icon() {
        assert_eq!(get_media_icon("photo"), "📷");
        assert_eq!(get_media_icon("video"), "🎬");
        assert_eq!(get_media_icon("audio"), "🎵");
        assert_eq!(get_media_icon("document"), "📄");
        assert_eq!(get_media_icon("unknown"), "📎");
    }

    #[test]
    fn test_items_per_page() {
        assert_eq!(ITEMS_PER_PAGE, 5);
    }

    #[test]
    fn test_format_file_size_edge_cases() {
        assert_eq!(format_file_size(0), "0 B");
        assert_eq!(format_file_size(1), "1 B");
        assert_eq!(format_file_size(1023), "1023 B");
        assert_eq!(format_file_size(1024 * 1024 - 1), "1024.0 KB");
        assert_eq!(format_file_size(500 * 1024), "500.0 KB");
        assert_eq!(format_file_size(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(format_file_size(2 * 1024 * 1024 * 1024), "2.00 GB");
    }

    #[test]
    fn test_format_duration_edge_cases() {
        assert_eq!(format_duration(59), "0:59");
        assert_eq!(format_duration(60), "1:00");
        assert_eq!(format_duration(3600), "1:00:00");
        assert_eq!(format_duration(7200), "2:00:00");
        assert_eq!(format_duration(86400), "24:00:00");
    }

    #[test]
    fn test_get_media_type_name() {
        assert_eq!(get_media_type_name("photo"), "Photos");
        assert_eq!(get_media_type_name("video"), "Videos");
        assert_eq!(get_media_type_name("audio"), "Audio");
        assert_eq!(get_media_type_name("document"), "Documents");
        assert_eq!(get_media_type_name("unknown"), "All");
    }

    /// Helper to create a test UploadEntry
    fn make_upload(id: i64, media_type: &str, title: &str) -> UploadEntry {
        UploadEntry {
            id,
            user_id: 123,
            original_filename: Some(format!("{}.test", title)),
            title: title.to_string(),
            media_type: media_type.to_string(),
            file_format: Some("mp4".to_string()),
            file_id: format!("file_id_{}", id),
            file_unique_id: Some(format!("unique_{}", id)),
            file_size: Some(1024 * 1024),
            duration: Some(120),
            width: Some(1920),
            height: Some(1080),
            mime_type: None,
            message_id: None,
            chat_id: None,
            uploaded_at: "2025-01-01".to_string(),
            thumbnail_file_id: None,
        }
    }

    /// Helper: extract all callback_data strings from a keyboard
    fn all_callbacks(keyboard: &InlineKeyboardMarkup) -> Vec<String> {
        keyboard
            .inline_keyboard
            .iter()
            .flat_map(|row| {
                row.iter().filter_map(|btn| match &btn.kind {
                    teloxide::types::InlineKeyboardButtonKind::CallbackData(data) => Some(data.clone()),
                    _ => None,
                })
            })
            .collect()
    }

    /// Helper: extract all button labels from a keyboard
    fn all_labels(keyboard: &InlineKeyboardMarkup) -> Vec<String> {
        keyboard
            .inline_keyboard
            .iter()
            .flat_map(|row| row.iter().map(|btn| btn.text.clone()))
            .collect()
    }

    // === Level 1 keyboard tests ===

    #[test]
    fn test_video_level1_has_send_and_convert() {
        let upload = make_upload(42, "video", "Test Video");
        let keyboard = build_upload_action_keyboard(&upload);
        let cbs = all_callbacks(&keyboard);

        assert!(
            cbs.contains(&"videos:submenu:send:42".to_string()),
            "Should have send submenu"
        );
        assert!(
            cbs.contains(&"videos:submenu:convert:42".to_string()),
            "Should have convert submenu"
        );
        assert!(cbs.contains(&"videos:delete:42".to_string()), "Should have delete");
        assert!(cbs.contains(&"videos:cancel".to_string()), "Should have cancel");
    }

    #[test]
    fn test_photo_level1_has_send_only() {
        let upload = make_upload(10, "photo", "Photo");
        let keyboard = build_upload_action_keyboard(&upload);
        let cbs = all_callbacks(&keyboard);

        assert!(
            cbs.contains(&"videos:submenu:send:10".to_string()),
            "Should have send submenu"
        );
        assert!(
            !cbs.iter().any(|cb| cb.contains("submenu:convert")),
            "Photo should NOT have convert submenu"
        );
        assert!(
            !cbs.iter().any(|cb| cb.contains("convert:")),
            "Photo should NOT have any convert buttons"
        );
    }

    #[test]
    fn test_audio_level1_has_send_only() {
        let upload = make_upload(20, "audio", "Song");
        let keyboard = build_upload_action_keyboard(&upload);
        let cbs = all_callbacks(&keyboard);

        assert!(
            cbs.contains(&"videos:submenu:send:20".to_string()),
            "Should have send submenu"
        );
        assert!(
            !cbs.iter().any(|cb| cb.contains("submenu:convert")),
            "Audio should NOT have convert submenu"
        );
    }

    #[test]
    fn test_document_level1_sends_directly() {
        let upload = make_upload(30, "document", "Report");
        let keyboard = build_upload_action_keyboard(&upload);
        let cbs = all_callbacks(&keyboard);

        // Document bypasses submenu — sends directly
        assert!(
            cbs.contains(&"videos:send:document:30".to_string()),
            "Document should send directly"
        );
        assert!(
            !cbs.iter().any(|cb| cb.contains("submenu:send")),
            "Document should NOT have send submenu"
        );
    }

    // === Send submenu tests ===

    #[test]
    fn test_send_submenu_video() {
        let upload = make_upload(42, "video", "Test Video");
        let keyboard = build_send_submenu_keyboard(&upload);
        let cbs = all_callbacks(&keyboard);

        assert!(
            cbs.contains(&"videos:send:video:42".to_string()),
            "Should have send:video"
        );
        assert!(
            cbs.contains(&"videos:send:document:42".to_string()),
            "Should have send:document"
        );
        assert!(cbs.contains(&"videos:open:42".to_string()), "Should have back button");
    }

    #[test]
    fn test_send_submenu_photo() {
        let upload = make_upload(10, "photo", "Photo");
        let keyboard = build_send_submenu_keyboard(&upload);
        let cbs = all_callbacks(&keyboard);

        assert!(
            cbs.contains(&"videos:send:photo:10".to_string()),
            "Should have send:photo"
        );
        assert!(
            cbs.contains(&"videos:send:document:10".to_string()),
            "Should have send:document"
        );
        assert!(cbs.contains(&"videos:open:10".to_string()), "Should have back button");
    }

    #[test]
    fn test_send_submenu_audio() {
        let upload = make_upload(20, "audio", "Song");
        let keyboard = build_send_submenu_keyboard(&upload);
        let cbs = all_callbacks(&keyboard);

        assert!(
            cbs.contains(&"videos:send:audio:20".to_string()),
            "Should have send:audio"
        );
        assert!(
            cbs.contains(&"videos:send:document:20".to_string()),
            "Should have send:document"
        );
        assert!(cbs.contains(&"videos:open:20".to_string()), "Should have back button");
    }

    // === Convert submenu tests ===

    #[test]
    fn test_convert_submenu_has_all_options() {
        let upload = make_upload(42, "video", "Test Video");
        let keyboard = build_convert_submenu_keyboard(&upload);
        let cbs = all_callbacks(&keyboard);

        assert!(
            cbs.contains(&"videos:convert:circle:42".to_string()),
            "Should have circle"
        );
        assert!(cbs.contains(&"videos:convert:audio:42".to_string()), "Should have MP3");
        assert!(cbs.contains(&"videos:convert:gif:42".to_string()), "Should have GIF");
        assert!(
            cbs.contains(&"videos:convert:compress:42".to_string()),
            "Should have compress"
        );
        assert!(cbs.contains(&"videos:open:42".to_string()), "Should have back button");
    }

    // === Back button tests ===

    #[test]
    fn test_all_submenus_have_back_button() {
        let upload = make_upload(42, "video", "Test Video");

        let send_kb = build_send_submenu_keyboard(&upload);
        let send_labels = all_labels(&send_kb);
        assert!(
            send_labels.contains(&"⬅️ Back".to_string()),
            "Send submenu should have back"
        );

        let convert_kb = build_convert_submenu_keyboard(&upload);
        let convert_labels = all_labels(&convert_kb);
        assert!(
            convert_labels.contains(&"⬅️ Back".to_string()),
            "Convert submenu should have back"
        );

        // Both back buttons should navigate to videos:open:{id}
        let send_cbs = all_callbacks(&send_kb);
        assert!(send_cbs.contains(&"videos:open:42".to_string()));
        let convert_cbs = all_callbacks(&convert_kb);
        assert!(convert_cbs.contains(&"videos:open:42".to_string()));
    }

    // === Info text test ===

    #[test]
    fn test_build_upload_info_text() {
        let upload = make_upload(42, "video", "Test Video");
        let text = build_upload_info_text(&upload);

        assert!(text.contains("🎬"), "Should have video icon");
        assert!(text.contains("Test Video"), "Should contain title");
        assert!(text.contains("What to do"), "Should ask what to do");
    }
}
