use crate::core::{config, escape_markdown};
use crate::storage::{db, DbPool};
use crate::telegram::commands::{process_video_clip, CutSegment};
use crate::telegram::Bot;
use crate::timestamps::{format_timestamp, select_best_timestamps, VideoTimestamp};
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
fn format_duration_short(seconds: i64) -> String {
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
        format!("\n\n📍 *Сохранённые таймкоды:*\n{}", text_lines.join("\n"))
    } else {
        String::new()
    };

    (button_rows, text)
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

    // Get filtered downloads
    let all_downloads = if file_type_filter.as_deref() == Some("edit") {
        db::get_cuts_history_filtered(&conn, chat_id.0, search_text.as_deref())
            .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
    } else {
        db::get_download_history_filtered(&conn, chat_id.0, file_type_filter.as_deref(), search_text.as_deref())
            .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
    };

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
        let icon = match ft.as_str() {
            "mp3" => "🎵",
            "mp4" => "🎬",
            "edit" => "✂️",
            _ => "📄",
        };
        let filter_name = if ft == "edit" {
            "Отрезки".to_string()
        } else {
            ft.to_uppercase()
        };
        text.push_str(&format!("Фильтр: {} {}\n\n", icon, filter_name));
    }
    if let Some(ref search) = search_text {
        text.push_str(&format!("🔍 Поиск: \"{}\"\n\n", search));
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
                search_text.as_deref().unwrap_or("")
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
                search_text.as_deref().unwrap_or("")
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
        filter_row.push(crate::telegram::cb(
            "🎵 MP3".to_string(),
            format!("downloads:filter:mp3:{}", search_text.as_deref().unwrap_or("")),
        ));
    }

    if file_type_filter.as_deref() != Some("mp4") {
        filter_row.push(crate::telegram::cb(
            "🎬 MP4".to_string(),
            format!("downloads:filter:mp4:{}", search_text.as_deref().unwrap_or("")),
        ));
    }

    if file_type_filter.as_deref() != Some("edit") {
        filter_row.push(crate::telegram::cb(
            "✂️ Отрезки".to_string(),
            format!("downloads:filter:edit:{}", search_text.as_deref().unwrap_or("")),
        ));
    }

    if file_type_filter.is_some() {
        filter_row.push(crate::telegram::cb(
            "🔄 Все".to_string(),
            format!("downloads:filter:all:{}", search_text.as_deref().unwrap_or("")),
        ));
    }

    if !filter_row.is_empty() {
        keyboard_rows.push(filter_row);
    }

    // Close button
    keyboard_rows.push(vec![crate::telegram::cb(
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
    username: Option<String>,
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

            if let Some(download) = db::get_download_history_entry(&conn, chat_id.0, download_id).map_err(|e| {
                log::error!("📥 Failed to get download entry: {}", e);
                teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string())))
            })? {
                if download.file_id.is_some() {
                    // Show options: resend as audio/document/video
                    let mut options = Vec::new();

                    if download.format == "mp3" {
                        options.push(vec![
                            crate::telegram::cb(
                                "🎵 Как аудио".to_string(),
                                format!("downloads:send:audio:{}", download_id),
                            ),
                            crate::telegram::cb(
                                "📎 Как документ".to_string(),
                                format!("downloads:send:document:{}", download_id),
                            ),
                        ]);
                        options.push(vec![
                            crate::telegram::cb("✂️ Вырезка".to_string(), format!("downloads:clip:{}", download_id)),
                            crate::telegram::cb("⭕️ Кружок".to_string(), format!("downloads:circle:{}", download_id)),
                            crate::telegram::cb(
                                "🔔 Сделать рингтон".to_string(),
                                format!("downloads:iphone_ringtone:{}", download_id),
                            ),
                        ]);
                        options.push(vec![crate::telegram::cb(
                            "⚙️ Изменить скорость".to_string(),
                            format!("downloads:speed:{}", download_id),
                        )]);
                    } else {
                        options.push(vec![
                            crate::telegram::cb(
                                "🎬 Как видео".to_string(),
                                format!("downloads:send:video:{}", download_id),
                            ),
                            crate::telegram::cb(
                                "📎 Как документ".to_string(),
                                format!("downloads:send:document:{}", download_id),
                            ),
                        ]);
                        options.push(vec![
                            crate::telegram::cb("✂️ Вырезка".to_string(), format!("downloads:clip:{}", download_id)),
                            crate::telegram::cb("⭕️ Кружок".to_string(), format!("downloads:circle:{}", download_id)),
                            crate::telegram::cb(
                                "🔔 Сделать рингтон".to_string(),
                                format!("downloads:iphone_ringtone:{}", download_id),
                            ),
                        ]);
                        options.push(vec![crate::telegram::cb(
                            "⚙️ Изменить скорость".to_string(),
                            format!("downloads:speed:{}", download_id),
                        )]);
                    }

                    options.push(vec![crate::telegram::cb(
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
        "resend_cut" => {
            log::info!("📥 Handling resend_cut action");
            if parts.len() < 3 {
                log::warn!("📥 Not enough parts for resend_cut");
                return Ok(());
            }
            let cut_id = parts[2].parse::<i64>().unwrap_or(0);
            log::info!("📥 Cut ID: {}", cut_id);

            let conn = db::get_connection(&db_pool).map_err(|e| {
                log::error!("📥 Failed to get DB connection: {}", e);
                teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string())))
            })?;

            if let Some(cut) = db::get_cut_entry(&conn, chat_id.0, cut_id).map_err(|e| {
                log::error!("📥 Failed to get cut entry: {}", e);
                teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string())))
            })? {
                log::info!("📥 Found cut: {:?}", cut);
                if cut.file_id.is_some() {
                    let mut options = Vec::new();

                    // Cuts are usually MP4
                    options.push(vec![
                        crate::telegram::cb(
                            "🎬 Как видео".to_string(),
                            format!("downloads:send_cut:video:{}", cut_id),
                        ),
                        crate::telegram::cb(
                            "📎 Как документ".to_string(),
                            format!("downloads:send_cut:document:{}", cut_id),
                        ),
                    ]);

                    options.push(vec![
                        crate::telegram::cb("✂️ Вырезка".to_string(), format!("downloads:clip_cut:{}", cut_id)),
                        crate::telegram::cb("⭕️ Кружок".to_string(), format!("downloads:circle_cut:{}", cut_id)),
                        crate::telegram::cb(
                            "🔔 Сделать рингтон".to_string(),
                            format!("downloads:iphone_ringtone_cut:{}", cut_id),
                        ),
                    ]);

                    options.push(vec![crate::telegram::cb(
                        "⚙️ Изменить скорость".to_string(),
                        format!("downloads:speed_cut:{}", cut_id),
                    )]);

                    options.push(vec![crate::telegram::cb(
                        "❌ Отмена".to_string(),
                        "downloads:cancel".to_string(),
                    )]);

                    let keyboard = InlineKeyboardMarkup::new(options);

                    bot.send_message(
                        chat_id,
                        format!("Как отправить отрезок *{}*?", escape_markdown(&cut.title)),
                    )
                    .parse_mode(ParseMode::MarkdownV2)
                    .reply_markup(keyboard)
                    .await?;
                    bot.send_message(chat_id, cut.original_url.clone()).await.ok();
                }
            }
        }
        "send" => {
            if parts.len() < 4 {
                return Ok(());
            }
            let send_type = parts[2];
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
                    let caption = if let Some(ref author) = download.author {
                        format!("{} - {}", author, download.title)
                    } else {
                        download.title.clone()
                    };

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
                            return Ok(());
                        }
                    };

                    match send_result {
                        Ok(sent_message) => {
                            bot.delete_message(chat_id, status_msg.id).await.ok();
                            if send_type == "audio" && download.format == "mp3" {
                                let duration = sent_message
                                    .audio()
                                    .map(|a| a.duration.seconds())
                                    .or_else(|| download.duration.map(|d| d.max(0) as u32))
                                    .unwrap_or(0);
                                if let Err(e) = add_audio_tools_buttons_from_history(
                                    bot,
                                    Arc::clone(&db_pool),
                                    chat_id,
                                    sent_message.id,
                                    &telegram_file_id,
                                    caption.clone(),
                                    duration,
                                )
                                .await
                                {
                                    log::warn!("Failed to add audio tools buttons: {}", e);
                                }
                            }
                            if (send_type == "video" || send_type == "document") && download.format == "mp4" {
                                if let Err(e) =
                                    add_video_cut_button_from_history(bot, chat_id, sent_message.id, download_id).await
                                {
                                    log::warn!("Failed to add video cut button: {}", e);
                                }
                            }
                            bot.delete_message(chat_id, message_id).await.ok();
                        }
                        Err(e) => {
                            bot.delete_message(chat_id, status_msg.id).await.ok();
                            bot.send_message(chat_id, format!("❌ Не удалось отправить файл: {e}"))
                                .await
                                .ok();
                        }
                    }
                }
            }
        }
        "send_cut" => {
            if parts.len() < 4 {
                return Ok(());
            }
            let send_type = parts[2];
            let cut_id = parts[3].parse::<i64>().unwrap_or(0);

            let conn = db::get_connection(&db_pool)
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

            if let Some(cut) = db::get_cut_entry(&conn, chat_id.0, cut_id)
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
            {
                if let Some(fid) = cut.file_id {
                    let status_text = match send_type {
                        "video" => "⏳ Готовлю отправку как видео…",
                        "document" => "⏳ Готовлю отправку как документ…",
                        _ => "⏳ Готовлю отправку…",
                    };
                    let status_msg = bot.send_message(chat_id, status_text).await?;

                    let telegram_file_id = fid;
                    let upload_file_name = "doradura_edit.mp4";
                    let caption = cut.title;

                    let send_result = match send_type {
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

                // Fetch timestamps and build UI
                let timestamps = db::get_video_timestamps(&conn, download_id).unwrap_or_default();
                let (ts_buttons, ts_text) = build_timestamp_ui(&timestamps, "clip", download_id);

                // Build keyboard with timestamp buttons and cancel button
                let mut keyboard_rows = ts_buttons;
                keyboard_rows.push(vec![crate::telegram::cb(
                    "❌ Отмена".to_string(),
                    "downloads:clip_cancel".to_string(),
                )]);
                let keyboard = InlineKeyboardMarkup::new(keyboard_rows);

                let base_message = "✂️ Отправь интервалы для вырезки в формате `мм:сс-мм:сс` или `чч:мм:сс-чч:мм:сс`\\.\nМожно несколько через запятую\\.\n\nПример: `00:10-00:25, 01:00-01:10`";
                let message = format!("{}{}", base_message, ts_text);
                bot.send_message(chat_id, message)
                    .parse_mode(ParseMode::MarkdownV2)
                    .reply_markup(keyboard)
                    .await?;
                bot.send_message(chat_id, download.url.clone()).await.ok();
                bot.delete_message(chat_id, message_id).await.ok();
            }
        }
        "clip_cut" => {
            if parts.len() < 3 {
                return Ok(());
            }
            let cut_id = parts[2].parse::<i64>().unwrap_or(0);
            let conn = db::get_connection(&db_pool)
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;
            if let Some(cut) = db::get_cut_entry(&conn, chat_id.0, cut_id)
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
            {
                if cut.file_id.is_none() {
                    bot.send_message(chat_id, "❌ Не удалось найти file\\_id для этого файла\\.")
                        .parse_mode(ParseMode::MarkdownV2)
                        .await
                        .ok();
                    return Ok(());
                }
                let session = crate::storage::db::VideoClipSession {
                    id: uuid::Uuid::new_v4().to_string(),
                    user_id: chat_id.0,
                    source_download_id: 0, // Not applicable for cut-from-cut
                    source_kind: "cut".to_string(),
                    source_id: cut_id,
                    original_url: cut.original_url.clone(),
                    output_kind: "cut".to_string(),
                    created_at: chrono::Utc::now(),
                    expires_at: chrono::Utc::now() + chrono::Duration::minutes(10),
                };
                crate::storage::db::upsert_video_clip_session(&conn, &session).map_err(|e| {
                    teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string())))
                })?;
                let keyboard = InlineKeyboardMarkup::new(vec![vec![crate::telegram::cb(
                    "❌ Отмена".to_string(),
                    "downloads:clip_cancel".to_string(),
                )]]);
                bot.send_message(chat_id, "✂️ Отправь интервалы для вырезки в формате `мм:сс-мм:сс` или `чч:мм:сс-чч:мм:сс`\\.\nМожно несколько через запятую\\.\n\nПример: `00:10-00:25, 01:00-01:10`").parse_mode(ParseMode::MarkdownV2).reply_markup(keyboard).await?;
                bot.send_message(chat_id, cut.original_url.clone()).await.ok();
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

                // Get user language for localization
                let lang = crate::i18n::user_lang(&conn, chat_id.0);

                // Fetch timestamps and build UI
                let timestamps = db::get_video_timestamps(&conn, download_id).unwrap_or_default();
                let (ts_buttons, ts_text) = build_timestamp_ui(&timestamps, "circle", download_id);

                // Build keyboard: duration buttons + timestamp buttons + cancel button
                let mut keyboard_rows = build_duration_buttons(download_id, &lang);
                keyboard_rows.extend(ts_buttons);
                keyboard_rows.push(vec![crate::telegram::cb(
                    crate::i18n::t(&lang, "common.cancel"),
                    "downloads:clip_cancel".to_string(),
                )]);
                let keyboard = InlineKeyboardMarkup::new(keyboard_rows);

                let base_message = crate::i18n::t(&lang, "video_circle.select_part");
                let message = format!("{}{}", base_message, ts_text);
                bot.send_message(chat_id, message)
                    .parse_mode(ParseMode::MarkdownV2)
                    .reply_markup(keyboard)
                    .await?;
                bot.send_message(chat_id, download.url.clone()).await.ok();
                bot.delete_message(chat_id, message_id).await.ok();
            }
        }
        "circle_cut" => {
            if parts.len() < 3 {
                return Ok(());
            }
            let cut_id = parts[2].parse::<i64>().unwrap_or(0);
            let conn = db::get_connection(&db_pool)
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;
            if let Some(cut) = db::get_cut_entry(&conn, chat_id.0, cut_id)
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
            {
                if cut.file_id.is_none() {
                    bot.send_message(chat_id, "❌ Не удалось найти file\\_id для этого файла\\.")
                        .parse_mode(ParseMode::MarkdownV2)
                        .await
                        .ok();
                    return Ok(());
                }
                let session = crate::storage::db::VideoClipSession {
                    id: uuid::Uuid::new_v4().to_string(),
                    user_id: chat_id.0,
                    source_download_id: 0,
                    source_kind: "cut".to_string(),
                    source_id: cut_id,
                    original_url: cut.original_url.clone(),
                    output_kind: "video_note".to_string(),
                    created_at: chrono::Utc::now(),
                    expires_at: chrono::Utc::now() + chrono::Duration::minutes(10),
                };
                crate::storage::db::upsert_video_clip_session(&conn, &session).map_err(|e| {
                    teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string())))
                })?;
                let keyboard = InlineKeyboardMarkup::new(vec![vec![crate::telegram::cb(
                    "❌ Отмена".to_string(),
                    "downloads:clip_cancel".to_string(),
                )]]);
                bot.send_message(chat_id, "⭕️ Отправь интервалы для кружка в формате `мм:сс-мм:сс` или `чч:мм:сс-чч:мм:сс`\\.\nМожно несколько через запятую\\.\n\nПример: `00:10-00:25` или `first30 2x`").parse_mode(ParseMode::MarkdownV2).reply_markup(keyboard).await?;
                bot.send_message(chat_id, cut.original_url.clone()).await.ok();
                bot.delete_message(chat_id, message_id).await.ok();
            }
        }
        "iphone_ringtone_cut" => {
            if parts.len() < 3 {
                return Ok(());
            }
            let cut_id = parts[2].parse::<i64>().unwrap_or(0);
            let conn = db::get_connection(&db_pool)
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;
            if let Some(cut) = db::get_cut_entry(&conn, chat_id.0, cut_id)
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
            {
                if cut.file_id.is_none() {
                    bot.send_message(chat_id, "❌ Не удалось найти file\\_id для этого файла\\.")
                        .parse_mode(ParseMode::MarkdownV2)
                        .await
                        .ok();
                    return Ok(());
                }
                let session = crate::storage::db::VideoClipSession {
                    id: uuid::Uuid::new_v4().to_string(),
                    user_id: chat_id.0,
                    source_download_id: 0, // Not applicable for cut-from-cut
                    source_kind: "cut".to_string(),
                    source_id: cut_id,
                    original_url: cut.original_url.clone(),
                    output_kind: "iphone_ringtone".to_string(),
                    created_at: chrono::Utc::now(),
                    expires_at: chrono::Utc::now() + chrono::Duration::minutes(10),
                };
                crate::storage::db::upsert_video_clip_session(&conn, &session).map_err(|e| {
                    teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string())))
                })?;
                let keyboard = InlineKeyboardMarkup::new(vec![vec![crate::telegram::cb(
                    "❌ Отмена".to_string(),
                    "downloads:clip_cancel".to_string(),
                )]]);
                bot.send_message(chat_id, "🔔 Отправь интервалы для рингтона в формате `мм:сс-мм:сс` или `чч:мм:сс-чч:мм:сс`\\.\nМожно несколько через запятую\\.\n\n💡 Если длительность превысит 40 секунд \\(лимит iOS\\), аудио будет автоматически обрезано\\.\n\nПример: `00:10-00:25`").parse_mode(ParseMode::MarkdownV2).reply_markup(keyboard).await?;
                bot.send_message(chat_id, cut.original_url.clone()).await.ok();
                bot.delete_message(chat_id, message_id).await.ok();
            }
        }
        "clip_cancel" => {
            let conn = db::get_connection(&db_pool)
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;
            crate::storage::db::delete_video_clip_session_by_user(&conn, chat_id.0).ok();
            bot.delete_message(chat_id, message_id).await.ok();
        }
        // Handle timestamp button clicks: downloads:ts:{output_kind}:{download_id}:{time_seconds}
        "ts" => {
            if parts.len() < 5 {
                return Ok(());
            }
            let output_kind = parts[2]; // "circle" or "clip"
            let download_id = parts[3].parse::<i64>().unwrap_or(0);
            let time_seconds = parts[4].parse::<i64>().unwrap_or(0);

            let conn = db::get_connection(&db_pool)
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

            if let Some(download) = db::get_download_history_entry(&conn, chat_id.0, download_id)
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
            {
                // Delete the prompt message
                bot.delete_message(chat_id, message_id).await.ok();

                // Determine segment duration based on output kind
                let segment_duration = match output_kind {
                    "circle" => 30, // 30 seconds for video notes (max 60s limit)
                    _ => 30,        // Default 30 seconds for clips
                };

                // Adjust end time based on video duration if available
                let end_seconds = if let Some(duration) = download.duration {
                    std::cmp::min(time_seconds + segment_duration, duration)
                } else {
                    time_seconds + segment_duration
                };

                // Create session
                let session = db::VideoClipSession {
                    id: uuid::Uuid::new_v4().to_string(),
                    user_id: chat_id.0,
                    source_download_id: download_id,
                    source_kind: "download".to_string(),
                    source_id: download_id,
                    original_url: download.url.clone(),
                    output_kind: if output_kind == "circle" {
                        "video_note".to_string()
                    } else {
                        "cut".to_string()
                    },
                    created_at: chrono::Utc::now(),
                    expires_at: chrono::Utc::now() + chrono::Duration::minutes(10),
                };

                // Delete any existing session first
                db::delete_video_clip_session_by_user(&conn, chat_id.0).ok();

                // Create segment
                let segment = CutSegment {
                    start_secs: time_seconds,
                    end_secs: end_seconds,
                };
                let segments_text = format!("{}-{}", format_timestamp(time_seconds), format_timestamp(end_seconds));

                // Process the clip
                let bot_clone = bot.clone();
                let db_pool_clone = db_pool.clone();
                tokio::spawn(async move {
                    if let Err(e) = process_video_clip(
                        bot_clone,
                        db_pool_clone,
                        chat_id,
                        session,
                        vec![segment],
                        segments_text,
                        None, // no speed modifier
                    )
                    .await
                    {
                        log::error!("Failed to process timestamp clip: {}", e);
                    }
                });
            }
        }
        // Handle duration button clicks: downloads:dur:{position}:{download_id}:{seconds}
        // position: first, last, middle, full
        "dur" => {
            if parts.len() < 4 {
                return Ok(());
            }
            let position = parts[2]; // first, last, middle, full
            let download_id = parts[3].parse::<i64>().unwrap_or(0);
            let duration_seconds = if parts.len() >= 5 {
                parts[4].parse::<i64>().unwrap_or(30)
            } else {
                60 // default for "full"
            };

            let conn = db::get_connection(&db_pool)
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

            if let Some(download) = db::get_download_history_entry(&conn, chat_id.0, download_id)
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
            {
                // Delete the prompt message
                bot.delete_message(chat_id, message_id).await.ok();

                let video_duration = download.duration.unwrap_or(duration_seconds);

                // Calculate segment based on position
                let (start_secs, end_secs) = match position {
                    "first" => {
                        let end = std::cmp::min(duration_seconds, video_duration).min(60);
                        (0, end)
                    }
                    "last" => {
                        let duration = std::cmp::min(duration_seconds, video_duration).min(60);
                        let start = (video_duration - duration).max(0);
                        (start, video_duration.min(start + 60))
                    }
                    "middle" => {
                        let duration = std::cmp::min(duration_seconds, video_duration).min(60);
                        let start = ((video_duration - duration) / 2).max(0);
                        (start, (start + duration).min(video_duration))
                    }
                    "full" => {
                        let end = video_duration.min(60);
                        (0, end)
                    }
                    _ => (0, std::cmp::min(duration_seconds, 60)),
                };

                // Create session
                let session = db::VideoClipSession {
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

                // Delete any existing session first
                db::delete_video_clip_session_by_user(&conn, chat_id.0).ok();

                // Create segment
                let segment = CutSegment { start_secs, end_secs };
                let segments_text = format!("{}-{}", format_timestamp(start_secs), format_timestamp(end_secs));

                // Process the clip
                let bot_clone = bot.clone();
                let db_pool_clone = db_pool.clone();
                tokio::spawn(async move {
                    if let Err(e) = process_video_clip(
                        bot_clone,
                        db_pool_clone,
                        chat_id,
                        session,
                        vec![segment],
                        segments_text,
                        None, // no speed modifier
                    )
                    .await
                    {
                        log::error!("Failed to process duration circle: {}", e);
                    }
                });
            }
        }
        "speed" => {
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
                        crate::telegram::cb("0.5x".to_string(), format!("downloads:apply_speed:0.5:{}", download_id)),
                        crate::telegram::cb(
                            "0.75x".to_string(),
                            format!("downloads:apply_speed:0.75:{}", download_id),
                        ),
                        crate::telegram::cb("1.0x".to_string(), format!("downloads:apply_speed:1.0:{}", download_id)),
                    ],
                    vec![
                        crate::telegram::cb(
                            "1.25x".to_string(),
                            format!("downloads:apply_speed:1.25:{}", download_id),
                        ),
                        crate::telegram::cb("1.5x".to_string(), format!("downloads:apply_speed:1.5:{}", download_id)),
                        crate::telegram::cb("2.0x".to_string(), format!("downloads:apply_speed:2.0:{}", download_id)),
                    ],
                    vec![crate::telegram::cb(
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
                bot.delete_message(chat_id, message_id).await.ok();
            }
        }
        "speed_cut" => {
            if parts.len() < 3 {
                return Ok(());
            }
            let cut_id = parts[2].parse::<i64>().unwrap_or(0);
            let conn = db::get_connection(&db_pool)
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;
            if let Some(cut) = db::get_cut_entry(&conn, chat_id.0, cut_id)
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
            {
                let speed_options = vec![
                    vec![
                        crate::telegram::cb("0.5x".to_string(), format!("downloads:apply_speed_cut:0.5:{}", cut_id)),
                        crate::telegram::cb(
                            "0.75x".to_string(),
                            format!("downloads:apply_speed_cut:0.75:{}", cut_id),
                        ),
                        crate::telegram::cb("1.0x".to_string(), format!("downloads:apply_speed_cut:1.0:{}", cut_id)),
                    ],
                    vec![
                        crate::telegram::cb(
                            "1.25x".to_string(),
                            format!("downloads:apply_speed_cut:1.25:{}", cut_id),
                        ),
                        crate::telegram::cb("1.5x".to_string(), format!("downloads:apply_speed_cut:1.5:{}", cut_id)),
                        crate::telegram::cb("2.0x".to_string(), format!("downloads:apply_speed_cut:2.0:{}", cut_id)),
                    ],
                    vec![crate::telegram::cb(
                        "❌ Отмена".to_string(),
                        "downloads:cancel".to_string(),
                    )],
                ];
                let keyboard = InlineKeyboardMarkup::new(speed_options);
                bot.send_message(
                    chat_id,
                    format!("⚙️ Выбери скорость для отрезка *{}*", escape_markdown(&cut.title)),
                )
                .parse_mode(ParseMode::MarkdownV2)
                .reply_markup(keyboard)
                .await?;
                bot.send_message(chat_id, cut.original_url.clone()).await.ok();
                bot.delete_message(chat_id, message_id).await.ok();
            }
        }
        "apply_speed" => {
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
                    bot.delete_message(chat_id, message_id).await.ok();
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
                                .or_else(|| sent_message.document().map(|d| d.file.id.0.clone()))
                                .or_else(|| sent_message.audio().map(|a| a.file.id.0.clone()));
                            if let Some(fid) = new_file_id {
                                if let Ok(db_id) = db::save_download_history(
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
                                    None,
                                    None,
                                ) {
                                    // Save message_id for MTProto file_reference refresh
                                    let _ = db::update_download_message_id(&conn, db_id, sent_message.id.0, chat_id.0);
                                }
                            }
                        }
                        Err(e) => {
                            bot.delete_message(chat_id, processing_msg.id).await.ok();
                            bot.send_message(
                                chat_id,
                                "❌ Не удалось обработать видео. Администратор получил уведомление о проблеме.",
                            )
                            .await
                            .ok();
                            // Notify admin about the error with full details
                            crate::telegram::notifications::notify_admin_video_error(
                                bot,
                                chat_id.0,
                                username.as_deref(),
                                &e.to_string(),
                                &format!("apply_speed: {}x on '{}'", speed_str, download.title),
                            )
                            .await;
                        }
                    }
                }
            }
        }
        "apply_speed_cut" => {
            if parts.len() < 4 {
                return Ok(());
            }
            let speed_str = parts[2];
            let cut_id = parts[3].parse::<i64>().unwrap_or(0);
            let speed: f32 = speed_str.parse().unwrap_or(1.0);
            let conn = db::get_connection(&db_pool)
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;
            if let Some(cut) = db::get_cut_entry(&conn, chat_id.0, cut_id)
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
            {
                if let Some(file_id) = cut.file_id {
                    bot.delete_message(chat_id, message_id).await.ok();
                    let processing_msg = bot.send_message(chat_id, format!("⚙️ Обрабатываю отрезок со скоростью {}x\\.\\.\\.  \nЭто может занять несколько минут\\.", speed_str.replace(".", "\\.")))
                        .parse_mode(ParseMode::MarkdownV2).await?;
                    match change_video_speed(bot, chat_id, &file_id, speed, &cut.title).await {
                        Ok((sent_message, file_size)) => {
                            bot.delete_message(chat_id, processing_msg.id).await.ok();
                            bot.send_message(chat_id, cut.original_url.clone()).await.ok();
                            // Note: Speed change of a cut produces a new cut?
                            // For simplicity, we could save it to download_history or as a new cut.
                            // Existing change_video_speed logic for downloads saves to download_history.
                            // Let's do the same for consistency.
                            let new_title = format!("{} [speed {}x]", cut.title, speed_str);
                            let new_duration = cut.duration.map(|d| ((d as f32) / speed).round().max(1.0) as i64);
                            let new_file_id = sent_message
                                .video()
                                .map(|v| v.file.id.0.clone())
                                .or_else(|| sent_message.document().map(|d| d.file.id.0.clone()))
                                .or_else(|| sent_message.audio().map(|a| a.file.id.0.clone()));
                            if let Some(fid) = new_file_id {
                                if let Ok(db_id) = db::save_download_history(
                                    &conn,
                                    chat_id.0,
                                    &cut.original_url,
                                    &new_title,
                                    "mp4",
                                    Some(&fid),
                                    None,
                                    Some(file_size),
                                    new_duration,
                                    cut.video_quality.as_deref(),
                                    None,
                                    None,
                                    None,
                                ) {
                                    // Save message_id for MTProto file_reference refresh
                                    let _ = db::update_download_message_id(&conn, db_id, sent_message.id.0, chat_id.0);
                                }
                            }
                        }
                        Err(e) => {
                            bot.delete_message(chat_id, processing_msg.id).await.ok();
                            bot.send_message(
                                chat_id,
                                "❌ Не удалось обработать видео. Администратор получил уведомление о проблеме.",
                            )
                            .await
                            .ok();
                            // Notify admin about the error with full details
                            crate::telegram::notifications::notify_admin_video_error(
                                bot,
                                chat_id.0,
                                username.as_deref(),
                                &e.to_string(),
                                &format!("apply_speed_cut: {}x on '{}'", speed_str, cut.title),
                            )
                            .await;
                        }
                    }
                }
            }
        }
        "iphone_ringtone" => {
            if parts.len() < 3 {
                return Ok(());
            }
            let download_id = parts[2].parse::<i64>().unwrap_or(0);
            let conn = db::get_connection(&db_pool)
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

            if let Some(download) = db::get_download_history_entry(&conn, chat_id.0, download_id)
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
            {
                if let Some(file_id) = download.file_id {
                    let lang = crate::i18n::user_lang(&conn, chat_id.0);
                    bot.delete_message(chat_id, message_id).await.ok();
                    let processing_msg = bot.send_message(chat_id, "⏳ Готовлю рингтон...").await?;
                    match handle_iphone_ringtone(bot, chat_id, &file_id, &download.title, &lang).await {
                        Ok(_) => {
                            bot.delete_message(chat_id, processing_msg.id).await.ok();
                        }
                        Err(e) => {
                            bot.delete_message(chat_id, processing_msg.id).await.ok();
                            bot.send_message(chat_id, format!("❌ Ошибка при создании рингтона: {}", e))
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

async fn add_audio_tools_buttons_from_history(
    bot: &Bot,
    db_pool: Arc<DbPool>,
    chat_id: ChatId,
    message_id: MessageId,
    telegram_file_id: &str,
    title: String,
    duration: u32,
) -> Result<(), String> {
    use crate::core::config;
    use crate::download::audio_effects::{self, AudioEffectSession};
    use std::path::Path;

    let conn = db::get_connection(&db_pool).map_err(|e| e.to_string())?;
    let session_id = uuid::Uuid::new_v4().to_string();
    let session_file_path_raw = audio_effects::get_original_file_path(&session_id, &config::DOWNLOAD_FOLDER);
    let session_file_path = shellexpand::tilde(&session_file_path_raw).into_owned();
    if let Some(parent) = Path::new(&session_file_path).parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| e.to_string())?;
    }

    crate::telegram::download_file_from_telegram(
        bot,
        telegram_file_id,
        Some(std::path::PathBuf::from(&session_file_path)),
    )
    .await
    .map_err(|e| e.to_string())?;

    let session = AudioEffectSession::new(
        session_id.clone(),
        chat_id.0,
        session_file_path,
        message_id.0,
        title,
        duration,
    );
    db::create_audio_effect_session(&conn, &session).map_err(|e| e.to_string())?;

    let keyboard = InlineKeyboardMarkup::new(vec![vec![
        crate::telegram::cb("🎛️ Edit Audio", format!("ae:open:{}", session_id)),
        crate::telegram::cb("✂️ Cut Audio", format!("ac:open:{}", session_id)),
    ]]);

    bot.edit_message_reply_markup(chat_id, message_id)
        .reply_markup(keyboard)
        .await
        .map_err(|e| e.to_string())?;

    Ok(())
}

async fn add_video_cut_button_from_history(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    download_id: i64,
) -> Result<(), String> {
    let keyboard = InlineKeyboardMarkup::new(vec![vec![crate::telegram::cb(
        "✂️ Cut Video",
        format!("downloads:clip:{}", download_id),
    )]]);

    bot.edit_message_reply_markup(chat_id, message_id)
        .reply_markup(keyboard)
        .await
        .map_err(|e| e.to_string())?;

    Ok(())
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

// is_local_bot_api_env is now crate::core::config::bot_api::is_local()

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
        if config::bot_api::is_local() {
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

    let temp_dir = std::path::PathBuf::from(crate::core::config::TEMP_FILES_DIR.as_str()).join("doradura_telegram");
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
    let temp_dir = PathBuf::from(crate::core::config::TEMP_FILES_DIR.as_str()).join("doradura_speed");
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

async fn handle_iphone_ringtone(
    bot: &Bot,
    chat_id: ChatId,
    file_id: &str,
    title: &str,
    lang: &unic_langid::LanguageIdentifier,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use std::path::PathBuf;
    use tokio::fs;

    // Create temp directory
    let temp_dir = PathBuf::from(crate::core::config::TEMP_FILES_DIR.as_str()).join("doradura_ringtone");
    fs::create_dir_all(&temp_dir).await?;

    // Save input file
    let input_filename = format!("ringtone_in_{}_{}", chat_id.0, uuid::Uuid::new_v4());
    let input_path = temp_dir.join(&input_filename);

    crate::telegram::download_file_from_telegram(bot, file_id, Some(input_path.clone()))
        .await
        .map_err(|e| format!("Failed to download file from Telegram: {}", e))?;

    // Output file path
    let output_path = temp_dir.join(format!("{}.m4r", title.replace("/", "_")));

    // Convert to ringtone (first 30 seconds)
    crate::download::ringtone::create_iphone_ringtone(&input_path, &output_path, 0, 30).await?;

    // Send the ringtone as a document (required for iOS to recognize it)
    let caption = crate::i18n::t(lang, "history.iphone_ringtone_instructions");

    bot.send_document(chat_id, teloxide::types::InputFile::file(output_path.clone()))
        .caption(caption)
        .parse_mode(ParseMode::MarkdownV2)
        .await?;

    // Cleanup temp files
    fs::remove_file(&input_path).await.ok();
    fs::remove_file(&output_path).await.ok();

    Ok(())
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

    // ==================== short_error_text tests ====================

    #[test]
    fn test_short_error_text_fits() {
        assert_eq!(short_error_text("Short text", 50), "Short text");
        assert_eq!(short_error_text("  Trimmed  ", 50), "Trimmed");
    }

    #[test]
    fn test_short_error_text_truncated() {
        let text = "This is a very long error message that should be truncated";
        let result = short_error_text(text, 20);
        assert!(result.len() <= 22); // 20 chars + ellipsis (UTF-8)
        assert!(result.ends_with('…'));
    }

    #[test]
    fn test_short_error_text_newlines() {
        assert_eq!(short_error_text("Line1\nLine2\nLine3", 50), "Line1 Line2 Line3");
    }

    #[test]
    fn test_short_error_text_empty() {
        assert_eq!(short_error_text("", 10), "");
        assert_eq!(short_error_text("   ", 10), "");
    }

    // ==================== is_file_too_big_error tests ====================

    #[test]
    fn test_is_file_too_big_error_true() {
        // Create a mock error containing "file is too big"
        let err = teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other("Error: file is too big")));
        assert!(is_file_too_big_error(&err));
    }

    #[test]
    fn test_is_file_too_big_error_case_insensitive() {
        let err = teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other("FILE IS TOO BIG")));
        assert!(is_file_too_big_error(&err));
    }

    #[test]
    fn test_is_file_too_big_error_false() {
        let err = teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other("Network error occurred")));
        assert!(!is_file_too_big_error(&err));
    }

    // ==================== escape_markdown tests ====================

    #[test]
    fn test_escape_markdown_underscore() {
        assert_eq!(escape_markdown("hello_world"), "hello\\_world");
    }

    #[test]
    fn test_escape_markdown_asterisk() {
        assert_eq!(escape_markdown("*bold*"), "\\*bold\\*");
    }

    #[test]
    fn test_escape_markdown_brackets() {
        assert_eq!(escape_markdown("[link](url)"), "\\[link\\]\\(url\\)");
    }

    #[test]
    fn test_escape_markdown_all_special() {
        let all_special = "_*[]()~`>#+-=|{}.!";
        let escaped = escape_markdown(all_special);
        assert_eq!(escaped, "\\_\\*\\[\\]\\(\\)\\~\\`\\>\\#\\+\\-\\=\\|\\{\\}\\.\\!");
    }

    #[test]
    fn test_escape_markdown_no_special() {
        assert_eq!(escape_markdown("hello world 123"), "hello world 123");
    }

    #[test]
    fn test_escape_markdown_empty() {
        assert_eq!(escape_markdown(""), "");
    }

    // ==================== forced_document_unavailable_notice tests ====================

    #[test]
    fn test_forced_document_notice_local_api_unavailable() {
        let error = "Not available on local bot api server";
        let notice = forced_document_unavailable_notice(error);
        assert!(notice.is_some());
        assert!(notice.unwrap().contains("локальный Bot API"));
    }

    #[test]
    fn test_forced_document_notice_file_too_big() {
        let error = "file is too big";
        let notice = forced_document_unavailable_notice(error);
        assert!(notice.is_some());
        assert!(notice.unwrap().contains("Не могу принудительно отправить"));
    }

    #[test]
    fn test_forced_document_notice_download_failed() {
        let error = "telegram file download failed";
        let notice = forced_document_unavailable_notice(error);
        assert!(notice.is_some());
        assert!(notice.unwrap().contains("не получилось скачать"));
    }

    #[test]
    fn test_forced_document_notice_none() {
        let error = "Some random error";
        let notice = forced_document_unavailable_notice(error);
        assert!(notice.is_none());
    }

    // ==================== ITEMS_PER_PAGE constant tests ====================

    #[test]
    fn test_items_per_page_value() {
        assert_eq!(ITEMS_PER_PAGE, 5);
    }
}
