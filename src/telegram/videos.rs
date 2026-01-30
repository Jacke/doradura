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

/// Get media type name in Russian
fn get_media_type_name(media_type: &str) -> &'static str {
    match media_type {
        "photo" => "Фото",
        "video" => "Видео",
        "audio" => "Аудио",
        "document" => "Документы",
        _ => "Все",
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
            "📭 Ничего не найдено.\n\nПопробуй изменить фильтры."
        } else {
            "📭 У тебя пока нет загруженных файлов.\n\nОтправь мне фото, видео или документ, и он появится здесь!"
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
    let mut text = String::from("📂 *Твои загрузки*\n\n");

    // Show active filters
    if let Some(ref mt) = media_type_filter {
        let icon = get_media_icon(mt);
        let filter_name = get_media_type_name(mt);
        text.push_str(&format!("Фильтр: {} {}\n\n", icon, filter_name));
    }
    if let Some(ref search) = search_text {
        text.push_str(&format!("🔍 Поиск: \"{}\"\n\n", escape_markdown(search)));
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
        text.push_str(&format!("\n_Страница {}/{}_", current_page + 1, total_pages));
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
        keyboard_rows.push(vec![InlineKeyboardButton::callback(
            button_text,
            format!("videos:open:{}", upload.id),
        )]);
    }

    // Navigation row
    let mut nav_buttons = Vec::new();

    if current_page > 0 {
        nav_buttons.push(InlineKeyboardButton::callback(
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
        nav_buttons.push(InlineKeyboardButton::callback(
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
        nav_buttons.push(InlineKeyboardButton::callback(
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
        filter_row.push(InlineKeyboardButton::callback(
            "🎬 Видео".to_string(),
            format!("videos:filter:video:{}", search_text.as_deref().unwrap_or("")),
        ));
    }

    if media_type_filter.as_deref() != Some("photo") {
        filter_row.push(InlineKeyboardButton::callback(
            "📷 Фото".to_string(),
            format!("videos:filter:photo:{}", search_text.as_deref().unwrap_or("")),
        ));
    }

    if media_type_filter.as_deref() != Some("document") {
        filter_row.push(InlineKeyboardButton::callback(
            "📄 Документы".to_string(),
            format!("videos:filter:document:{}", search_text.as_deref().unwrap_or("")),
        ));
    }

    if media_type_filter.is_some() {
        filter_row.push(InlineKeyboardButton::callback(
            "🔄 Все".to_string(),
            format!("videos:filter:all:{}", search_text.as_deref().unwrap_or("")),
        ));
    }

    if !filter_row.is_empty() {
        keyboard_rows.push(filter_row);
    }

    // Close button
    keyboard_rows.push(vec![InlineKeyboardButton::callback(
        "❌ Закрыть".to_string(),
        "videos:close".to_string(),
    )]);

    let keyboard = InlineKeyboardMarkup::new(keyboard_rows);

    bot.send_message(chat_id, text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(keyboard)
        .await
}

/// Build action keyboard for a specific upload
fn build_upload_action_keyboard(upload: &UploadEntry) -> InlineKeyboardMarkup {
    let mut rows = Vec::new();

    // Row 1: Send options
    let mut send_row = Vec::new();
    match upload.media_type.as_str() {
        "video" => {
            send_row.push(InlineKeyboardButton::callback(
                "📤 Как видео".to_string(),
                format!("videos:send:video:{}", upload.id),
            ));
            send_row.push(InlineKeyboardButton::callback(
                "📎 Как документ".to_string(),
                format!("videos:send:document:{}", upload.id),
            ));
        }
        "photo" => {
            send_row.push(InlineKeyboardButton::callback(
                "📤 Как фото".to_string(),
                format!("videos:send:photo:{}", upload.id),
            ));
            send_row.push(InlineKeyboardButton::callback(
                "📎 Как документ".to_string(),
                format!("videos:send:document:{}", upload.id),
            ));
        }
        "audio" => {
            send_row.push(InlineKeyboardButton::callback(
                "📤 Как аудио".to_string(),
                format!("videos:send:audio:{}", upload.id),
            ));
            send_row.push(InlineKeyboardButton::callback(
                "📎 Как документ".to_string(),
                format!("videos:send:document:{}", upload.id),
            ));
        }
        _ => {
            send_row.push(InlineKeyboardButton::callback(
                "📤 Отправить".to_string(),
                format!("videos:send:document:{}", upload.id),
            ));
        }
    }
    rows.push(send_row);

    // Row 2: Conversion options (only for video)
    if upload.media_type == "video" {
        rows.push(vec![
            InlineKeyboardButton::callback("⭕️ Кружок".to_string(), format!("videos:convert:circle:{}", upload.id)),
            InlineKeyboardButton::callback("🎵 MP3".to_string(), format!("videos:convert:audio:{}", upload.id)),
            InlineKeyboardButton::callback("🎞️ GIF".to_string(), format!("videos:convert:gif:{}", upload.id)),
        ]);
        rows.push(vec![InlineKeyboardButton::callback(
            "📦 Сжать".to_string(),
            format!("videos:convert:compress:{}", upload.id),
        )]);
    }

    // Row 3: Delete and cancel
    rows.push(vec![
        InlineKeyboardButton::callback("🗑️ Удалить".to_string(), format!("videos:delete:{}", upload.id)),
        InlineKeyboardButton::callback("❌ Отмена".to_string(), "videos:cancel".to_string()),
    ]);

    InlineKeyboardMarkup::new(rows)
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
                let icon = get_media_icon(&upload.media_type);

                // Build info string
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

                let keyboard = build_upload_action_keyboard(&upload);

                bot.send_message(
                    chat_id,
                    format!(
                        "Что сделать с {} *{}*?{}",
                        icon,
                        escape_markdown(&upload.title),
                        info_str
                    ),
                )
                .parse_mode(ParseMode::MarkdownV2)
                .reply_markup(keyboard)
                .await?;

                bot.delete_message(chat_id, message_id).await.ok();
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

                let status_msg = bot.send_message(chat_id, "⏳ Отправляю файл...").await?;

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
                        bot.send_message(chat_id, format!("❌ Не удалось отправить файл: {}", e))
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
                InlineKeyboardButton::callback(
                    "✅ Да, удалить".to_string(),
                    format!("videos:confirm_delete:{}", upload_id),
                ),
                InlineKeyboardButton::callback("❌ Отмена".to_string(), "videos:cancel".to_string()),
            ]]);

            if let Some(upload) = get_upload_by_id(&conn, chat_id.0, upload_id)
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
            {
                bot.edit_message_text(
                    chat_id,
                    message_id,
                    format!("🗑️ Удалить *{}*?", escape_markdown(&upload.title)),
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
                    bot.send_message(chat_id, "✅ Файл удалён").await?;
                }
                Ok(false) => {
                    bot.send_message(chat_id, "❌ Файл не найден").await?;
                }
                Err(e) => {
                    bot.send_message(chat_id, format!("❌ Ошибка при удалении: {}", e))
                        .await?;
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
                            let button = InlineKeyboardButton::callback(
                                format!("{}s", dur),
                                format!("convert:circle:{}:{}", upload_id, dur),
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
                                let full_video_label = format!("📼 Всё видео ({} кружков)", split_info.num_parts);
                                rows.push(vec![InlineKeyboardButton::callback(
                                    full_video_label,
                                    format!("convert:circle:{}:{}", upload_id, video_duration),
                                )]);
                            } else if is_too_long_for_split(video_duration) {
                                // Video too long - show warning button (disabled)
                                rows.push(vec![InlineKeyboardButton::callback(
                                    "⚠️ Видео слишком длинное (макс. 6 мин)".to_string(),
                                    "videos:noop".to_string(),
                                )]);
                            }
                        }

                        rows.push(vec![InlineKeyboardButton::callback(
                            "❌ Отмена".to_string(),
                            "videos:cancel".to_string(),
                        )]);

                        let keyboard = InlineKeyboardMarkup::new(rows);

                        // Build status message based on video duration
                        let status_text = if video_duration > VIDEO_NOTE_MAX_DURATION {
                            if is_too_long_for_split(video_duration) {
                                format!(
                                    "⭕️ *Выбери длительность кружка* для *{}*:\n\n⚠️ Видео длиннее 6 минут — можно создать только кружок до 60с\\.\n\nИли отправь интервал в формате `мм:сс\\-мм:сс`\\.",
                                    escape_markdown(&upload.title)
                                )
                            } else {
                                let split_info = calculate_video_note_split(video_duration);
                                let num_circles = split_info.map(|s| s.num_parts).unwrap_or(1);
                                format!(
                                    "⭕️ *Выбери длительность кружка* для *{}*:\n\n💡 Видео длиннее 60с — можно создать {} кружков\\.\n\nИли отправь интервал в формате `мм:сс\\-мм:сс`\\.",
                                    escape_markdown(&upload.title),
                                    num_circles
                                )
                            }
                        } else {
                            format!(
                                "⭕️ *Выбери длительность кружка* для *{}*:\n\nИли отправь интервал в формате `мм:сс\\-мм:сс`\\.",
                                escape_markdown(&upload.title)
                            )
                        };

                        bot.edit_message_text(chat_id, message_id, status_text)
                            .parse_mode(ParseMode::MarkdownV2)
                            .reply_markup(keyboard)
                            .await?;
                    }
                    "audio" | "gif" | "compress" => {
                        // TODO: Implement these conversions in the conversion module
                        bot.send_message(
                            chat_id,
                            format!(
                                "🚧 Конвертация в {} пока в разработке\\.\n\nСкоро будет доступна\\!",
                                match convert_type {
                                    "audio" => "MP3",
                                    "gif" => "GIF",
                                    "compress" => "сжатое видео",
                                    _ => "неизвестный формат",
                                }
                            ),
                        )
                        .parse_mode(ParseMode::MarkdownV2)
                        .await?;
                    }
                    _ => {}
                }
            }
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
            // Format: convert:circle:upload_id:duration
            if parts.len() < 4 {
                return Ok(());
            }
            let upload_id = parts[2].parse::<i64>().unwrap_or(0);
            let duration = parts[3].parse::<u64>().unwrap_or(30);

            if let Some(upload) = get_upload_by_id(&conn, chat_id.0, upload_id)
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
            {
                bot.delete_message(chat_id, message_id).await.ok();

                // Check if video is too long for splitting (> 360s)
                if is_too_long_for_split(duration) {
                    bot.send_message(
                        chat_id,
                        "❌ Видео слишком длинное\\. Максимум 6 минут для разбивки на кружки\\.",
                    )
                    .parse_mode(ParseMode::MarkdownV2)
                    .await?;
                    return Ok(());
                }

                // Calculate split info for status message
                let split_info = calculate_video_note_split(duration);
                let num_circles = split_info.as_ref().map(|s| s.num_parts).unwrap_or(1);

                let status_text = if num_circles > 1 {
                    format!(
                        "⏳ Создаю {} кружков из *{}*\\.\\.\\.\n\n_Это может занять несколько минут_",
                        num_circles,
                        escape_markdown(&upload.title)
                    )
                } else {
                    format!(
                        "⏳ Создаю кружок из *{}*\\.\\.\\.\n\n_Это может занять несколько минут_",
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
                        bot.edit_message_text(chat_id, status_msg.id, format!("❌ Не удалось скачать файл: {}", e))
                            .await?;
                        return Ok(());
                    }
                };

                // Check if we need to split into multiple circles
                if duration > VIDEO_NOTE_MAX_DURATION {
                    // Create multiple video notes
                    match to_video_notes_split(&temp_input, duration, None).await {
                        Ok(output_paths) => {
                            let total = output_paths.len();
                            for (i, output_path) in output_paths.iter().enumerate() {
                                let progress_text = format!("📤 Отправляю кружок {}/{}...", i + 1, total);
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
                                            format!("❌ Не удалось отправить кружок {}/{}: {}", i + 1, total, e),
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
                            bot.edit_message_text(
                                chat_id,
                                status_msg.id,
                                format!("❌ Ошибка при создании кружков: {}", e),
                            )
                            .await?;
                        }
                    }
                } else {
                    // Single video note (original logic)
                    let options = VideoNoteOptions {
                        duration: Some(duration),
                        start_time: None,
                        speed: None,
                    };

                    match to_video_note(&temp_input, options).await {
                        Ok(output_path) => {
                            bot.edit_message_text(chat_id, status_msg.id, "📤 Отправляю кружок...")
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
                                        format!("❌ Не удалось отправить кружок: {}", e),
                                    )
                                    .await?;
                                }
                            }

                            tokio::fs::remove_file(&output_path).await.ok();
                        }
                        Err(e) => {
                            bot.edit_message_text(
                                chat_id,
                                status_msg.id,
                                format!("❌ Ошибка при создании кружка: {}", e),
                            )
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
                        format!("⏳ Извлекаю аудио из *{}*\\.\\.\\.", escape_markdown(&upload.title)),
                    )
                    .parse_mode(ParseMode::MarkdownV2)
                    .await?;

                // Download file from Telegram
                let temp_input = match download_file_from_telegram(bot, &upload.file_id, None).await {
                    Ok(path) => path,
                    Err(e) => {
                        bot.edit_message_text(chat_id, status_msg.id, format!("❌ Не удалось скачать файл: {}", e))
                            .await?;
                        return Ok(());
                    }
                };

                // Extract audio
                match extract_audio(&temp_input, "320k").await {
                    Ok(output_path) => {
                        bot.edit_message_text(chat_id, status_msg.id, "📤 Отправляю аудио...")
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
                                    format!("❌ Не удалось отправить аудио: {}", e),
                                )
                                .await?;
                            }
                        }

                        // Clean up
                        tokio::fs::remove_file(&output_path).await.ok();
                    }
                    Err(e) => {
                        bot.edit_message_text(chat_id, status_msg.id, format!("❌ Ошибка при извлечении аудио: {}", e))
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
                            "⏳ Создаю GIF из *{}*\\.\\.\\.\n\n_Это может занять некоторое время_",
                            escape_markdown(&upload.title)
                        ),
                    )
                    .parse_mode(ParseMode::MarkdownV2)
                    .await?;

                // Download file from Telegram
                let temp_input = match download_file_from_telegram(bot, &upload.file_id, None).await {
                    Ok(path) => path,
                    Err(e) => {
                        bot.edit_message_text(chat_id, status_msg.id, format!("❌ Не удалось скачать файл: {}", e))
                            .await?;
                        return Ok(());
                    }
                };

                // Convert to GIF
                let options = GifOptions::default();

                match to_gif(&temp_input, options).await {
                    Ok(output_path) => {
                        bot.edit_message_text(chat_id, status_msg.id, "📤 Отправляю GIF...")
                            .await
                            .ok();

                        // Send as animation
                        match bot.send_animation(chat_id, InputFile::file(&output_path)).await {
                            Ok(_) => {
                                bot.delete_message(chat_id, status_msg.id).await.ok();
                            }
                            Err(e) => {
                                bot.edit_message_text(
                                    chat_id,
                                    status_msg.id,
                                    format!("❌ Не удалось отправить GIF: {}", e),
                                )
                                .await?;
                            }
                        }

                        // Clean up
                        tokio::fs::remove_file(&output_path).await.ok();
                    }
                    Err(e) => {
                        bot.edit_message_text(chat_id, status_msg.id, format!("❌ Ошибка при создании GIF: {}", e))
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
                            "⏳ Сжимаю *{}*\\.\\.\\.\n\n_Это может занять несколько минут_",
                            escape_markdown(&upload.title)
                        ),
                    )
                    .parse_mode(ParseMode::MarkdownV2)
                    .await?;

                // Download file from Telegram
                let temp_input = match download_file_from_telegram(bot, &upload.file_id, None).await {
                    Ok(path) => path,
                    Err(e) => {
                        bot.edit_message_text(chat_id, status_msg.id, format!("❌ Не удалось скачать файл: {}", e))
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
                                "📤 Отправляю сжатое видео...\n({} → {}, -{:.0}%)",
                                format_file_size(original_size as i64),
                                format_file_size(compressed_size as i64),
                                size_reduction
                            ),
                        )
                        .await
                        .ok();

                        match bot
                            .send_video(chat_id, InputFile::file(&output_path))
                            .caption(format!("{} (сжато, -{:.0}%)", upload.title, size_reduction))
                            .await
                        {
                            Ok(_) => {
                                bot.delete_message(chat_id, status_msg.id).await.ok();
                            }
                            Err(e) => {
                                bot.edit_message_text(
                                    chat_id,
                                    status_msg.id,
                                    format!("❌ Не удалось отправить видео: {}", e),
                                )
                                .await?;
                            }
                        }

                        // Clean up
                        tokio::fs::remove_file(&output_path).await.ok();
                    }
                    Err(e) => {
                        bot.edit_message_text(chat_id, status_msg.id, format!("❌ Ошибка при сжатии видео: {}", e))
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
}
