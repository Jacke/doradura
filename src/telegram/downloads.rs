use crate::storage::{db, DbPool};
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{CallbackQueryId, InlineKeyboardButton, InlineKeyboardMarkup, MessageId, ParseMode};

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

/// Show downloads page
pub async fn show_downloads_page(
    bot: &Bot,
    chat_id: ChatId,
    db_pool: Arc<DbPool>,
    page: usize,
    file_type_filter: Option<String>,
    search_text: Option<String>,
) -> ResponseResult<Message> {
    let conn = db::get_connection(&db_pool)
        .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

    // Get filtered downloads (only mp3/mp4 with file_id)
    let all_downloads =
        db::get_download_history_filtered(&conn, chat_id.0, file_type_filter.as_deref(), search_text.as_deref())
            .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

    if all_downloads.is_empty() {
        let empty_msg = if file_type_filter.is_some() || search_text.is_some() {
            "📭 Ничего не найдено.\n\nПопробуй изменить фильтры."
        } else {
            "📭 У тебя пока нет скачанных файлов.\n\nСкачай что-нибудь, и оно появится здесь!"
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
    let mut text = String::from("📥 *Твои загрузки*\n\n");

    // Show active filters
    if let Some(ref ft) = file_type_filter {
        let icon = if ft == "mp3" { "🎵" } else { "🎬" };
        text.push_str(&format!("Фильтр: {} {}\n\n", icon, ft.to_uppercase()));
    }
    if let Some(ref search) = search_text {
        text.push_str(&format!("🔍 Поиск: \"{}\"\n\n", search));
    }

    // List downloads
    for download in page_downloads {
        let icon = if download.format == "mp3" { "🎵" } else { "🎬" };
        let title = if let Some(ref author) = download.author {
            format!("{} - {}", author, download.title)
        } else {
            download.title.clone()
        };

        text.push_str(&format!("{} *{}*\n", icon, escape_markdown(&title)));

        // Format metadata
        let mut metadata_parts = Vec::new();

        if let Some(is_local) = download.bot_api_is_local {
            if is_local == 1 {
                metadata_parts.push("🏠 local Bot API".to_string());
            } else {
                metadata_parts.push("☁️ official Bot API".to_string());
            }
        }

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
        text.push_str(&format!("\n_Страница {}/{}_", current_page + 1, total_pages));
    }

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
        keyboard_rows.push(vec![InlineKeyboardButton::callback(
            button_text,
            format!("downloads:resend:{}", download.id),
        )]);
    }

    // Navigation row
    let mut nav_buttons = Vec::new();

    if current_page > 0 {
        nav_buttons.push(InlineKeyboardButton::callback(
            "⬅️".to_string(),
            format!(
                "downloads:page:{}:{}:{}",
                current_page - 1,
                file_type_filter.as_deref().unwrap_or("all"),
                search_text.as_deref().unwrap_or("")
            ),
        ));
    }

    if total_pages > 1 {
        nav_buttons.push(InlineKeyboardButton::callback(
            format!("{}/{}", current_page + 1, total_pages),
            format!(
                "downloads:page:{}:{}:{}",
                current_page,
                file_type_filter.as_deref().unwrap_or("all"),
                search_text.as_deref().unwrap_or("")
            ),
        ));
    }

    if current_page < total_pages - 1 {
        nav_buttons.push(InlineKeyboardButton::callback(
            "➡️".to_string(),
            format!(
                "downloads:page:{}:{}:{}",
                current_page + 1,
                file_type_filter.as_deref().unwrap_or("all"),
                search_text.as_deref().unwrap_or("")
            ),
        ));
    }

    if !nav_buttons.is_empty() {
        keyboard_rows.push(nav_buttons);
    }

    // Filter buttons row
    let mut filter_row = Vec::new();

    if file_type_filter.as_deref() != Some("mp3") {
        filter_row.push(InlineKeyboardButton::callback(
            "🎵 MP3".to_string(),
            format!("downloads:filter:mp3:{}", search_text.as_deref().unwrap_or("")),
        ));
    }

    if file_type_filter.as_deref() != Some("mp4") {
        filter_row.push(InlineKeyboardButton::callback(
            "🎬 MP4".to_string(),
            format!("downloads:filter:mp4:{}", search_text.as_deref().unwrap_or("")),
        ));
    }

    if file_type_filter.is_some() {
        filter_row.push(InlineKeyboardButton::callback(
            "🔄 Все".to_string(),
            format!("downloads:filter:all:{}", search_text.as_deref().unwrap_or("")),
        ));
    }

    if !filter_row.is_empty() {
        keyboard_rows.push(filter_row);
    }

    // Close button
    keyboard_rows.push(vec![InlineKeyboardButton::callback(
        "❌ Закрыть".to_string(),
        "downloads:close".to_string(),
    )]);

    let keyboard = InlineKeyboardMarkup::new(keyboard_rows);

    bot.send_message(chat_id, text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(keyboard)
        .await
}

/// Handle downloads callback queries
pub async fn handle_downloads_callback(
    bot: &Bot,
    callback_id: CallbackQueryId,
    chat_id: ChatId,
    message_id: MessageId,
    data: &str,
    db_pool: Arc<DbPool>,
) -> ResponseResult<()> {
    log::info!("📥 handle_downloads_callback called with data: {}", data);
    bot.answer_callback_query(callback_id).await?;

    let parts: Vec<&str> = data.splitn(5, ':').collect();
    log::info!("📥 Parsed parts: {:?}", parts);
    if parts.len() < 2 {
        log::warn!("📥 Not enough parts in callback data");
        return Ok(());
    }

    let action = parts[1];
    log::info!("📥 Action: {}", action);

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
            show_downloads_page(bot, chat_id, db_pool, page, filter, search).await?;
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
            show_downloads_page(bot, chat_id, db_pool, 0, filter, search).await?;
        }
        "resend" => {
            log::info!("📥 Handling resend action");
            if parts.len() < 3 {
                log::warn!("📥 Not enough parts for resend");
                return Ok(());
            }
            let download_id = parts[2].parse::<i64>().unwrap_or(0);
            log::info!("📥 Download ID: {}", download_id);

            let conn = db::get_connection(&db_pool).map_err(|e| {
                log::error!("📥 Failed to get DB connection: {}", e);
                teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string())))
            })?;
            log::info!("📥 Got DB connection");

            if let Some(download) = db::get_download_history_entry(&conn, chat_id.0, download_id).map_err(|e| {
                log::error!("📥 Failed to get download entry: {}", e);
                teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string())))
            })? {
                log::info!("📥 Found download: {:?}", download);
                log::info!("📥 file_id: {:?}, format: {}", download.file_id, download.format);
                if download.file_id.is_some() {
                    log::info!("📥 Building options menu");
                    // Show options: resend as audio/document/video
                    let mut options = Vec::new();

                    if download.format == "mp3" {
                        log::info!("📥 Format is mp3");
                        options.push(vec![
                            InlineKeyboardButton::callback(
                                "🎵 Как аудио".to_string(),
                                format!("downloads:send:audio:{}", download_id),
                            ),
                            InlineKeyboardButton::callback(
                                "📎 Как документ".to_string(),
                                format!("downloads:send:document:{}", download_id),
                            ),
                        ]);
                    } else {
                        options.push(vec![
                            InlineKeyboardButton::callback(
                                "🎬 Как видео".to_string(),
                                format!("downloads:send:video:{}", download_id),
                            ),
                            InlineKeyboardButton::callback(
                                "📎 Как документ".to_string(),
                                format!("downloads:send:document:{}", download_id),
                            ),
                        ]);
                        options.push(vec![
                            InlineKeyboardButton::callback(
                                "✂️ Вырезка".to_string(),
                                format!("downloads:clip:{}", download_id),
                            ),
                            InlineKeyboardButton::callback(
                                "⭕️ Кружок".to_string(),
                                format!("downloads:circle:{}", download_id),
                            ),
                            InlineKeyboardButton::callback(
                                "⚙️ Изменить скорость".to_string(),
                                format!("downloads:speed:{}", download_id),
                            ),
                        ]);
                    }

                    options.push(vec![InlineKeyboardButton::callback(
                        "❌ Отмена".to_string(),
                        "downloads:cancel".to_string(),
                    )]);

                    let keyboard = InlineKeyboardMarkup::new(options);

                    bot.send_message(
                        chat_id,
                        format!("Как отправить *{}*?", escape_markdown(&download.title)),
                    )
                    .parse_mode(ParseMode::MarkdownV2)
                    .reply_markup(keyboard)
                    .await?;
                    bot.send_message(chat_id, download.url.clone()).await.ok();
                }
            }
        }
        "send" => {
            // Actually send the file in chosen format
            if parts.len() < 4 {
                return Ok(());
            }
            let send_type = parts[2]; // "audio", "video", "document"
            let download_id = parts[3].parse::<i64>().unwrap_or(0);

            let conn = db::get_connection(&db_pool)
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

            if let Some(download) = db::get_download_history_entry(&conn, chat_id.0, download_id)
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
            {
                if let Some(fid) = download.file_id {
                    let status_text = match send_type {
                        "audio" => "⏳ Готовлю отправку как аудио…",
                        "video" => "⏳ Готовлю отправку как видео…",
                        "document" => "⏳ Готовлю отправку как документ…",
                        _ => "⏳ Готовлю отправку…",
                    };
                    let status_msg = bot.send_message(chat_id, status_text).await?;

                    let telegram_file_id = fid;
                    let upload_file_name = if download.format == "mp3" {
                        "doradura.mp3"
                    } else {
                        "doradura.mp4"
                    };
                    let title_with_author = if let Some(ref author) = download.author {
                        format!("{} - {}", author, download.title)
                    } else {
                        download.title.clone()
                    };
                    let caption = title_with_author;

                    let send_result = match send_type {
                        "audio" => {
                            bot.send_audio(
                                chat_id,
                                teloxide::types::InputFile::file_id(teloxide::types::FileId(telegram_file_id.clone())),
                            )
                            .caption(caption.clone())
                            .await
                        }
                        "video" => {
                            bot.send_video(
                                chat_id,
                                teloxide::types::InputFile::file_id(teloxide::types::FileId(telegram_file_id.clone())),
                            )
                            .caption(caption.clone())
                            .await
                        }
                        "document" => {
                            send_document_forced(bot, chat_id, &telegram_file_id, upload_file_name, caption.clone())
                                .await
                        }
                        _ => {
                            bot.delete_message(chat_id, status_msg.id).await.ok();
                            bot.send_message(chat_id, "❌ Неизвестный режим отправки.").await.ok();
                            return Ok(());
                        }
                    };

                    match send_result {
                        Ok(_) => {
                            bot.delete_message(chat_id, status_msg.id).await.ok();
                            bot.delete_message(chat_id, message_id).await.ok();
                        }
                        Err(e) => {
                            if send_type == "document" && is_file_too_big_error(&e) {
                                log::warn!("Document send failed due to size (download_id={}): {}", download_id, e);

                                let video_fallback = bot
                                    .send_video(
                                        chat_id,
                                        teloxide::types::InputFile::file_id(teloxide::types::FileId(
                                            telegram_file_id.clone(),
                                        )),
                                    )
                                    .caption(caption.clone())
                                    .await;

                                bot.delete_message(chat_id, status_msg.id).await.ok();
                                bot.delete_message(chat_id, message_id).await.ok();

                                match video_fallback {
                                    Ok(_) => {
                                        bot.send_message(
			                                            chat_id,
			                                            "⚠️ Не смог отправить как документ: Telegram отклонил файл по размеру.\nОтправил как видео.\n\nЕсли нужен именно документ — сделай ✂️ вырезку поменьше и отправь её как документ.",
			                                        )
			                                        .await
			                                        .ok();
                                    }
                                    Err(e2) => {
                                        bot.send_message(
                                            chat_id,
                                            format!("❌ Не удалось отправить файл даже как видео: {e2}"),
                                        )
                                        .await
                                        .ok();
                                    }
                                }
                                return Ok(());
                            }
                            bot.delete_message(chat_id, status_msg.id).await.ok();
                            bot.send_message(chat_id, format!("❌ Не удалось отправить файл: {e}"))
                                .await
                                .ok();
                        }
                    }
                }
            }
        }
        "clip" => {
            if parts.len() < 3 {
                return Ok(());
            }
            let download_id = parts[2].parse::<i64>().unwrap_or(0);
            let conn = db::get_connection(&db_pool)
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

            if let Some(download) = db::get_download_history_entry(&conn, chat_id.0, download_id)
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
            {
                if download.format != "mp4" {
                    bot.send_message(chat_id, "✂️ Вырезка доступна только для MP4\\.")
                        .parse_mode(ParseMode::MarkdownV2)
                        .await
                        .ok();
                    return Ok(());
                }
                if download.file_id.is_none() {
                    bot.send_message(chat_id, "❌ Не удалось найти file\\_id для этого файла\\.")
                        .parse_mode(ParseMode::MarkdownV2)
                        .await
                        .ok();
                    return Ok(());
                }

                let session = crate::storage::db::VideoClipSession {
                    id: uuid::Uuid::new_v4().to_string(),
                    user_id: chat_id.0,
                    source_download_id: download_id,
                    source_kind: "download".to_string(),
                    source_id: download_id,
                    original_url: download.url.clone(),
                    output_kind: "cut".to_string(),
                    created_at: chrono::Utc::now(),
                    expires_at: chrono::Utc::now() + chrono::Duration::minutes(10),
                };
                crate::storage::db::upsert_video_clip_session(&conn, &session).map_err(|e| {
                    teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string())))
                })?;

                let keyboard = InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::callback(
                    "❌ Отмена".to_string(),
                    "downloads:clip_cancel".to_string(),
                )]]);

                bot.send_message(
	                    chat_id,
	                    "✂️ Отправь интервалы для вырезки в формате `мм:сс-мм:сс` или `чч:мм:сс-чч:мм:сс`\\.\nМожно несколько через запятую\\.\n\nПример: `00:10-00:25, 01:00-01:10`",
	                )
	                .parse_mode(ParseMode::MarkdownV2)
	                .reply_markup(keyboard)
	                .await?;

                bot.send_message(chat_id, download.url.clone()).await.ok();
                bot.delete_message(chat_id, message_id).await.ok();
            }
        }
        "circle" => {
            if parts.len() < 3 {
                return Ok(());
            }
            let download_id = parts[2].parse::<i64>().unwrap_or(0);
            let conn = db::get_connection(&db_pool)
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

            if let Some(download) = db::get_download_history_entry(&conn, chat_id.0, download_id)
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
            {
                if download.format != "mp4" {
                    bot.send_message(chat_id, "⭕️ Кружок доступен только для MP4\\.")
                        .parse_mode(ParseMode::MarkdownV2)
                        .await
                        .ok();
                    return Ok(());
                }
                if download.file_id.is_none() {
                    bot.send_message(chat_id, "❌ Не удалось найти file\\_id для этого файла\\.")
                        .parse_mode(ParseMode::MarkdownV2)
                        .await
                        .ok();
                    return Ok(());
                }

                let session = crate::storage::db::VideoClipSession {
                    id: uuid::Uuid::new_v4().to_string(),
                    user_id: chat_id.0,
                    source_download_id: download_id,
                    source_kind: "download".to_string(),
                    source_id: download_id,
                    original_url: download.url.clone(),
                    output_kind: "video_note".to_string(),
                    created_at: chrono::Utc::now(),
                    expires_at: chrono::Utc::now() + chrono::Duration::minutes(10),
                };
                crate::storage::db::upsert_video_clip_session(&conn, &session).map_err(|e| {
                    teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string())))
                })?;

                let keyboard = InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::callback(
                    "❌ Отмена".to_string(),
                    "downloads:clip_cancel".to_string(),
                )]]);

                bot.send_message(
	                    chat_id,
	                    "⭕️ Отправь интервалы для кружка в формате `мм:сс-мм:сс` или `чч:мм:сс-чч:мм:сс`\\.\nМожно несколько через запятую\\.\n\nИли используй команды:\n• `full` \\- всё видео\n• `first30` \\- первые 30 секунд\n• `last30` \\- последние 30 секунд\n• `middle30` \\- 30 секунд из середины\n\n💡 Можно добавить скорость: `first30 2x`, `full 1\\.5x`\n\n💡 Если длительность превысит 60 секунд \\(лимит Telegram\\), видео будет автоматически обрезано\\.\n\nПример: `00:10-00:25` или `first30 2x`",
	                )
	                .parse_mode(ParseMode::MarkdownV2)
	                .reply_markup(keyboard)
	                .await?;

                bot.send_message(chat_id, download.url.clone()).await.ok();
                bot.delete_message(chat_id, message_id).await.ok();
            }
        }
        "clip_cancel" => {
            let conn = db::get_connection(&db_pool)
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;
            crate::storage::db::delete_video_clip_session_by_user(&conn, chat_id.0).ok();
            bot.delete_message(chat_id, message_id).await.ok();
        }
        "speed" => {
            // Show speed options
            if parts.len() < 3 {
                return Ok(());
            }
            let download_id = parts[2].parse::<i64>().unwrap_or(0);

            let conn = db::get_connection(&db_pool)
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

            if let Some(download) = db::get_download_history_entry(&conn, chat_id.0, download_id)
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
            {
                let speed_options = vec![
                    vec![
                        InlineKeyboardButton::callback(
                            "0.5x".to_string(),
                            format!("downloads:apply_speed:0.5:{}", download_id),
                        ),
                        InlineKeyboardButton::callback(
                            "0.75x".to_string(),
                            format!("downloads:apply_speed:0.75:{}", download_id),
                        ),
                        InlineKeyboardButton::callback(
                            "1.0x".to_string(),
                            format!("downloads:apply_speed:1.0:{}", download_id),
                        ),
                    ],
                    vec![
                        InlineKeyboardButton::callback(
                            "1.25x".to_string(),
                            format!("downloads:apply_speed:1.25:{}", download_id),
                        ),
                        InlineKeyboardButton::callback(
                            "1.5x".to_string(),
                            format!("downloads:apply_speed:1.5:{}", download_id),
                        ),
                        InlineKeyboardButton::callback(
                            "2.0x".to_string(),
                            format!("downloads:apply_speed:2.0:{}", download_id),
                        ),
                    ],
                    vec![InlineKeyboardButton::callback(
                        "❌ Отмена".to_string(),
                        "downloads:cancel".to_string(),
                    )],
                ];

                let keyboard = InlineKeyboardMarkup::new(speed_options);

                bot.send_message(
                    chat_id,
                    format!("⚙️ Выбери скорость для *{}*", escape_markdown(&download.title)),
                )
                .parse_mode(ParseMode::MarkdownV2)
                .reply_markup(keyboard)
                .await?;
                bot.send_message(chat_id, download.url.clone()).await.ok();

                // Delete the previous options message
                bot.delete_message(chat_id, message_id).await.ok();
            }
        }
        "apply_speed" => {
            // Apply speed change and process video
            if parts.len() < 4 {
                return Ok(());
            }
            let speed_str = parts[2];
            let download_id = parts[3].parse::<i64>().unwrap_or(0);

            let speed: f32 = speed_str.parse().unwrap_or(1.0);

            let conn = db::get_connection(&db_pool)
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

            if let Some(download) = db::get_download_history_entry(&conn, chat_id.0, download_id)
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
            {
                if let Some(file_id) = download.file_id {
                    // Delete the speed selection message
                    bot.delete_message(chat_id, message_id).await.ok();

                    // Send processing message
                    let processing_msg = bot
                        .send_message(
                            chat_id,
                            format!(
                                "⚙️ Обрабатываю видео со скоростью {}x\\.\\.\\.  \nЭто может занять несколько минут\\.",
                                speed_str.replace(".", "\\.")
                            ),
                        )
                        .parse_mode(ParseMode::MarkdownV2)
                        .await?;

                    match change_video_speed(bot, chat_id, &file_id, speed, &download.title).await {
                        Ok((sent_message, file_size)) => {
                            bot.delete_message(chat_id, processing_msg.id).await.ok();
                            bot.send_message(chat_id, download.url.clone()).await.ok();

                            let new_title = format!("{} [speed {}x]", download.title, speed_str);
                            let new_duration = download.duration.map(|d| ((d as f32) / speed).round().max(1.0) as i64);

                            let new_file_id = sent_message
                                .video()
                                .map(|v| v.file.id.0.clone())
                                .or_else(|| sent_message.document().map(|d| d.file.id.0.clone()));

                            if let Some(fid) = new_file_id {
                                let _ = db::save_download_history(
                                    &conn,
                                    chat_id.0,
                                    &download.url,
                                    &new_title,
                                    "mp4",
                                    Some(&fid),
                                    download.author.as_deref(),
                                    Some(file_size),
                                    new_duration,
                                    download.video_quality.as_deref(),
                                    None,
                                );
                            }
                        }
                        Err(e) => {
                            bot.delete_message(chat_id, processing_msg.id).await.ok();
                            bot.send_message(chat_id, format!("❌ Ошибка при обработке видео: {}", e))
                                .await
                                .ok();
                        }
                    }
                }
            }
        }
        "cancel" => {
            bot.delete_message(chat_id, message_id).await?;
        }
        "close" => {
            bot.delete_message(chat_id, message_id).await?;
        }
        _ => {}
    }

    Ok(())
}

fn request_error_from_text(text: String) -> teloxide::RequestError {
    teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(text)))
}

fn is_file_too_big_error(e: &teloxide::RequestError) -> bool {
    e.to_string().to_lowercase().contains("file is too big")
}

fn bot_api_source_hint() -> String {
    match std::env::var("BOT_API_URL").ok() {
        Some(url) if !url.contains("api.telegram.org") => format!("Bot API: local ({})", url),
        Some(url) => format!("Bot API: {}", url),
        None => "Bot API: https://api.telegram.org".to_string(),
    }
}

fn is_local_bot_api_env() -> bool {
    std::env::var("BOT_API_URL")
        .ok()
        .map(|u| !u.contains("api.telegram.org"))
        .unwrap_or(false)
}

fn short_error_text(text: &str, max_chars: usize) -> String {
    let t = text.trim().replace('\n', " ");
    if t.chars().count() <= max_chars {
        return t;
    }
    let truncated: String = t.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{}…", truncated)
}

fn forced_document_unavailable_notice(download_error_text: &str) -> Option<String> {
    let lower = download_error_text.to_lowercase();
    if lower.contains("not available on local bot api server") {
        return Some(format!(
            "⚠️ Не могу принудительно отправить как документ: локальный Bot API не видит этот файл по /file (нет в local cache/dir).\nОставил как видео.\n\n{}",
            bot_api_source_hint()
        ));
    }
    if lower.contains("local bot api file availability check failed")
        || lower.contains("local bot api file check failed")
    {
        return Some(format!(
            "⚠️ Не могу принудительно отправить как документ: ошибка при проверке файла на локальном Bot API.\nОставил как видео.\n\nПричина: {}\n{}",
            short_error_text(download_error_text, 180),
            bot_api_source_hint()
        ));
    }
    if lower.contains("file is too big") {
        if is_local_bot_api_env() {
            return Some(format!(
                "⚠️ Не могу принудительно отправить как документ: локальный Bot API вернул `file is too big` ещё на `getFile`.\nОбычно это значит, что сервер запущен НЕ в `--local` режиме (и наследует лимит официального Bot API ~20 MB), либо реально применён лимит на стороне сервера.\nОставил как видео.\n\nПричина: {}\n{}",
                short_error_text(download_error_text, 180),
                bot_api_source_hint()
            ));
        }
        return Some(format!(
            "⚠️ Не могу принудительно отправить как документ: чтобы «сделать документ», боту нужно скачать файл и пере-залить его.\nНа официальном Bot API скачивание ограничено ~20 MB; на локальном Bot API это работает только если файл доступен через /file.\nОставил как видео.\n\nПричина: {}\n{}",
            short_error_text(download_error_text, 180),
            bot_api_source_hint()
        ));
    }
    if lower.contains("telegram file download failed") {
        return Some(format!(
            "⚠️ Не могу принудительно отправить как документ: не получилось скачать файл с file-endpoint Bot API.\nОставил как видео.\n\nПричина: {}\n{}",
            short_error_text(download_error_text, 180),
            bot_api_source_hint()
        ));
    }
    None
}

async fn send_document_forced(
    bot: &Bot,
    chat_id: ChatId,
    telegram_file_id: &str,
    upload_file_name: &str,
    caption: String,
) -> ResponseResult<teloxide::types::Message> {
    let first_msg = bot
        .send_document(
            chat_id,
            teloxide::types::InputFile::file_id(teloxide::types::FileId(telegram_file_id.to_string())),
        )
        .disable_content_type_detection(true)
        .caption(caption.clone())
        .await?;

    if first_msg.document().is_some() {
        return Ok(first_msg);
    }

    // If Telegram still renders it as media, try to force a re-upload as a document.
    // Important: do NOT delete the first message unless the re-upload succeeds, otherwise user gets nothing.

    let temp_dir = std::path::PathBuf::from("/tmp/doradura_telegram");
    tokio::fs::create_dir_all(&temp_dir)
        .await
        .map_err(|e| request_error_from_text(e.to_string()))?;
    let temp_path = temp_dir.join(format!("{}_{}", chat_id.0, upload_file_name));

    match crate::telegram::download_file_from_telegram(bot, telegram_file_id, Some(temp_path.clone())).await {
        Ok(_) => {}
        Err(e) => {
            let msg = e.to_string();
            if let Some(notice) = forced_document_unavailable_notice(&msg) {
                log::warn!("Forced document re-upload is not possible: {}", msg);
                bot.send_message(chat_id, notice).await.ok();
                tokio::fs::remove_file(&temp_path).await.ok();
                return Ok(first_msg);
            }
            tokio::fs::remove_file(&temp_path).await.ok();
            return Err(request_error_from_text(msg));
        }
    }

    let result = bot
        .send_document(chat_id, teloxide::types::InputFile::file(temp_path.clone()))
        .disable_content_type_detection(true)
        .caption(caption)
        .await;

    tokio::fs::remove_file(&temp_path).await.ok();

    match result {
        Ok(msg) => {
            bot.delete_message(chat_id, first_msg.id).await.ok();
            Ok(msg)
        }
        Err(e) => {
            if is_file_too_big_error(&e) {
                bot.send_message(
                    chat_id,
                    "⚠️ Не смог принудительно отправить как документ: Telegram отклонил файл по размеру. Оставил как видео.",
                )
                .await
                .ok();
                return Ok(first_msg);
            }
            Err(e)
        }
    }
}

/// Change video speed using ffmpeg
async fn change_video_speed(
    bot: &Bot,
    chat_id: ChatId,
    file_id: &str,
    speed: f32,
    title: &str,
) -> Result<(teloxide::types::Message, i64), Box<dyn std::error::Error + Send + Sync>> {
    use std::path::PathBuf;
    use tokio::fs;
    use tokio::process::Command;

    // Create temp directory
    let temp_dir = PathBuf::from("/tmp/doradura_speed");
    fs::create_dir_all(&temp_dir).await?;

    // Save input file
    let input_path = temp_dir.join(format!("input_{}_{}.mp4", chat_id.0, uuid::Uuid::new_v4()));
    crate::telegram::download_file_from_telegram(bot, file_id, Some(input_path.clone()))
        .await
        .map_err(|e| format!("Failed to download file from Telegram: {}", e))?;

    // Output file path
    let output_path = temp_dir.join(format!("output_{}_{}.mp4", chat_id.0, speed));

    // Calculate audio tempo (pitch correction)
    let atempo = speed;

    // Build ffmpeg command
    // For speed > 2.0, we need to chain multiple atempo filters (max is 2.0 per filter)
    let atempo_filter = if speed > 2.0 {
        format!("atempo=2.0,atempo={}", speed / 2.0)
    } else if speed < 0.5 {
        format!("atempo=0.5,atempo={}", speed / 0.5)
    } else {
        format!("atempo={}", atempo)
    };

    let filter_complex = format!("[0:v]setpts={}*PTS[v];[0:a]{}[a]", 1.0 / speed, atempo_filter);

    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-i")
        .arg(&input_path)
        .arg("-filter_complex")
        .arg(&filter_complex)
        .arg("-map")
        .arg("[v]")
        .arg("-map")
        .arg("[a]")
        .arg("-c:v")
        .arg("libx264")
        .arg("-preset")
        .arg("fast")
        .arg("-crf")
        .arg("23")
        .arg("-c:a")
        .arg("aac")
        .arg("-b:a")
        .arg("192k")
        .arg("-y")
        .arg(&output_path);

    let output = cmd.output().await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("ffmpeg failed: {}", stderr).into());
    }

    let file_size = fs::metadata(&output_path).await.map(|m| m.len() as i64).unwrap_or(0);
    let sent = bot
        .send_video(chat_id, teloxide::types::InputFile::file(output_path.clone()))
        .caption(format!("{} (скорость {}x)", title, speed))
        .await?;

    // Cleanup temp files
    fs::remove_file(&input_path).await.ok();
    fs::remove_file(&output_path).await.ok();

    Ok((sent, file_size))
}

/// Escape markdown special characters
fn escape_markdown(text: &str) -> String {
    text.replace('_', "\\_")
        .replace('*', "\\*")
        .replace('[', "\\[")
        .replace(']', "\\]")
        .replace('(', "\\(")
        .replace(')', "\\)")
        .replace('~', "\\~")
        .replace('`', "\\`")
        .replace('>', "\\>")
        .replace('#', "\\#")
        .replace('+', "\\+")
        .replace('-', "\\-")
        .replace('=', "\\=")
        .replace('|', "\\|")
        .replace('{', "\\{")
        .replace('}', "\\}")
        .replace('.', "\\.")
        .replace('!', "\\!")
}
