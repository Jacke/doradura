use crate::storage::{db, DbPool};
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{CallbackQueryId, InlineKeyboardButton, InlineKeyboardMarkup, MessageId, ParseMode};

const ITEMS_PER_PAGE: usize = 5;

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

pub async fn show_cuts_page(bot: &Bot, chat_id: ChatId, db_pool: Arc<DbPool>, page: usize) -> ResponseResult<Message> {
    let conn = db::get_connection(&db_pool)
        .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

    let total_items = db::get_cuts_count(&conn, chat_id.0)
        .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
        as usize;

    if total_items == 0 {
        return bot
            .send_message(
                chat_id,
                "✂️ У тебя пока нет вырезок.\n\nОткрой /downloads и нажми ✂️ Вырезка у нужного видео.",
            )
            .await;
    }

    let total_pages = total_items.div_ceil(ITEMS_PER_PAGE);
    let current_page = page.min(total_pages.saturating_sub(1));
    let offset = (current_page * ITEMS_PER_PAGE) as i64;

    let cuts = db::get_cuts_page(&conn, chat_id.0, ITEMS_PER_PAGE as i64, offset)
        .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

    let mut text = String::from("✂️ *Твои вырезки*\n\n");
    for cut in &cuts {
        let title = crate::telegram::escape_markdown(&cut.title);
        let icon = if cut.output_kind == "video_note" {
            "⭕️"
        } else {
            "✂️"
        };
        text.push_str(&format!("{} *{}*\n", icon, title));

        let mut meta = Vec::new();
        if let Some(size) = cut.file_size {
            meta.push(format_file_size(size));
        }
        if let Some(dur) = cut.duration {
            meta.push(format_duration(dur));
        }
        if !cut.segments_text.trim().is_empty() {
            meta.push(cut.segments_text.clone());
        }

        if !meta.is_empty() {
            let date_only: String = cut.created_at.chars().take(10).collect();
            let meta_str = crate::telegram::escape_markdown(&meta.join(" · "));
            let date_str = crate::telegram::escape_markdown(&date_only);
            text.push_str(&format!("└ {} · {}\n\n", meta_str, date_str));
        } else {
            text.push('\n');
        }
    }

    let mut rows = Vec::new();
    for cut in &cuts {
        let short_title = if cut.title.chars().count() > 30 {
            format!("{}…", cut.title.chars().take(30).collect::<String>())
        } else {
            cut.title.clone()
        };
        rows.push(vec![InlineKeyboardButton::callback(
            format!(
                "{} {}",
                if cut.output_kind == "video_note" {
                    "⭕️"
                } else {
                    "✂️"
                },
                short_title
            ),
            format!("cuts:open:{}", cut.id),
        )]);
    }

    let mut nav = Vec::new();
    if current_page > 0 {
        nav.push(InlineKeyboardButton::callback(
            "⬅️".to_string(),
            format!("cuts:page:{}", current_page - 1),
        ));
    }
    if total_pages > 1 {
        nav.push(InlineKeyboardButton::callback(
            format!("{}/{}", current_page + 1, total_pages),
            format!("cuts:page:{}", current_page),
        ));
    }
    if current_page + 1 < total_pages {
        nav.push(InlineKeyboardButton::callback(
            "➡️".to_string(),
            format!("cuts:page:{}", current_page + 1),
        ));
    }
    if !nav.is_empty() {
        rows.push(nav);
    }

    rows.push(vec![InlineKeyboardButton::callback(
        "❌ Закрыть".to_string(),
        "cuts:close".to_string(),
    )]);

    bot.send_message(chat_id, text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(InlineKeyboardMarkup::new(rows))
        .await
}

pub async fn handle_cuts_callback(
    bot: &Bot,
    callback_id: CallbackQueryId,
    chat_id: ChatId,
    message_id: MessageId,
    data: &str,
    db_pool: Arc<DbPool>,
) -> ResponseResult<()> {
    bot.answer_callback_query(callback_id).await?;

    let parts: Vec<&str> = data.splitn(5, ':').collect();
    if parts.len() < 2 {
        return Ok(());
    }

    match parts[1] {
        "page" => {
            if parts.len() < 3 {
                return Ok(());
            }
            let page = parts[2].parse::<usize>().unwrap_or(0);
            bot.delete_message(chat_id, message_id).await.ok();
            show_cuts_page(bot, chat_id, db_pool, page).await?;
        }
        "open" => {
            if parts.len() < 3 {
                return Ok(());
            }
            let cut_id = parts[2].parse::<i64>().unwrap_or(0);
            let conn = db::get_connection(&db_pool)
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;
            if let Some(cut) = db::get_cut_entry(&conn, chat_id.0, cut_id)
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
            {
                let mut options = Vec::new();
                options.push(vec![
                    InlineKeyboardButton::callback("🎬 Как видео".to_string(), format!("cuts:send:video:{}", cut_id)),
                    InlineKeyboardButton::callback(
                        "📎 Как документ".to_string(),
                        format!("cuts:send:document:{}", cut_id),
                    ),
                ]);
                options.push(vec![
                    InlineKeyboardButton::callback("✂️ Вырезка".to_string(), format!("cuts:clip:{}", cut_id)),
                    InlineKeyboardButton::callback("⭕️ Кружок".to_string(), format!("cuts:circle:{}", cut_id)),
                    InlineKeyboardButton::callback(
                        "⚙️ Изменить скорость".to_string(),
                        format!("cuts:speed:{}", cut_id),
                    ),
                ]);
                options.push(vec![InlineKeyboardButton::callback(
                    "❌ Отмена".to_string(),
                    "cuts:cancel".to_string(),
                )]);

                bot.send_message(
                    chat_id,
                    format!("Что сделать с *{}*?", crate::telegram::escape_markdown(&cut.title)),
                )
                .parse_mode(ParseMode::MarkdownV2)
                .reply_markup(InlineKeyboardMarkup::new(options))
                .await?;

                if !cut.original_url.trim().is_empty() {
                    bot.send_message(chat_id, cut.original_url.clone()).await.ok();
                }
                bot.delete_message(chat_id, message_id).await.ok();
            }
        }
        "send" => {
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
                let Some(telegram_file_id) = cut.file_id.clone() else {
                    bot.send_message(chat_id, "❌ У этой вырезки нет file_id для отправки.")
                        .await
                        .ok();
                    return Ok(());
                };

                let status_text = match send_type {
                    "video" => "⏳ Готовлю отправку как видео…",
                    "document" => "⏳ Готовлю отправку как документ…",
                    _ => "⏳ Готовлю отправку…",
                };
                let status_msg = bot.send_message(chat_id, status_text).await?;

                let caption = cut.title.clone();
                let send_result = match send_type {
                    "video" => {
                        bot.send_video(
                            chat_id,
                            teloxide::types::InputFile::file_id(teloxide::types::FileId(telegram_file_id.clone())),
                        )
                        .caption(caption)
                        .await
                    }
                    "document" => send_document_forced(bot, chat_id, &telegram_file_id, "doradura.mp4", caption).await,
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
                            log::warn!("Cut document send failed due to size (cut_id={}): {}", cut_id, e);

                            let video_fallback = bot
                                .send_video(
                                    chat_id,
                                    teloxide::types::InputFile::file_id(teloxide::types::FileId(
                                        telegram_file_id.clone(),
                                    )),
                                )
                                .caption(cut.title.clone())
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
        "speed" => {
            if parts.len() < 3 {
                return Ok(());
            }
            let cut_id = parts[2].parse::<i64>().unwrap_or(0);
            let conn = db::get_connection(&db_pool)
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;
            if let Some(cut) = db::get_cut_entry(&conn, chat_id.0, cut_id)
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
            {
                let rows = vec![
                    vec![
                        InlineKeyboardButton::callback("0.5x".to_string(), format!("cuts:apply_speed:0.5:{}", cut_id)),
                        InlineKeyboardButton::callback(
                            "0.75x".to_string(),
                            format!("cuts:apply_speed:0.75:{}", cut_id),
                        ),
                        InlineKeyboardButton::callback("1.0x".to_string(), format!("cuts:apply_speed:1.0:{}", cut_id)),
                    ],
                    vec![
                        InlineKeyboardButton::callback(
                            "1.25x".to_string(),
                            format!("cuts:apply_speed:1.25:{}", cut_id),
                        ),
                        InlineKeyboardButton::callback("1.5x".to_string(), format!("cuts:apply_speed:1.5:{}", cut_id)),
                        InlineKeyboardButton::callback("2.0x".to_string(), format!("cuts:apply_speed:2.0:{}", cut_id)),
                    ],
                    vec![InlineKeyboardButton::callback(
                        "❌ Отмена".to_string(),
                        "cuts:cancel".to_string(),
                    )],
                ];

                bot.send_message(
                    chat_id,
                    format!(
                        "⚙️ Выбери скорость для *{}*",
                        crate::telegram::escape_markdown(&cut.title)
                    ),
                )
                .parse_mode(ParseMode::MarkdownV2)
                .reply_markup(InlineKeyboardMarkup::new(rows))
                .await?;

                if !cut.original_url.trim().is_empty() {
                    bot.send_message(chat_id, cut.original_url).await.ok();
                }
                bot.delete_message(chat_id, message_id).await.ok();
            }
        }
        "apply_speed" => {
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
                let Some(file_id) = cut.file_id.clone() else {
                    bot.send_message(chat_id, "❌ У этой вырезки нет file_id для обработки.")
                        .await
                        .ok();
                    return Ok(());
                };

                bot.delete_message(chat_id, message_id).await.ok();
                let processing = bot
                    .send_message(
                        chat_id,
                        format!(
                            "⚙️ Обрабатываю видео со скоростью {}x\\.\\.\\.\nЭто может занять несколько минут\\.",
                            speed_str.replace('.', "\\.")
                        ),
                    )
                    .parse_mode(ParseMode::MarkdownV2)
                    .await?;

                match change_video_speed(bot, chat_id, &file_id, speed, &cut.title).await {
                    Ok((sent_message, file_size)) => {
                        bot.delete_message(chat_id, processing.id).await.ok();
                        if !cut.original_url.trim().is_empty() {
                            bot.send_message(chat_id, cut.original_url.clone()).await.ok();
                        }

                        let new_title = format!("{} [speed {}x]", cut.title, speed_str);
                        let new_duration = cut.duration.map(|d| ((d as f32) / speed).round().max(1.0) as i64);
                        let new_file_id = sent_message
                            .video()
                            .map(|v| v.file.id.0.clone())
                            .or_else(|| sent_message.document().map(|d| d.file.id.0.clone()));

                        if let Some(fid) = new_file_id {
                            let _ = db::create_cut(
                                &conn,
                                chat_id.0,
                                &cut.original_url,
                                "cut",
                                cut_id,
                                "clip",
                                &cut.segments_json,
                                &cut.segments_text,
                                &new_title,
                                Some(&fid),
                                Some(file_size),
                                new_duration,
                                cut.video_quality.as_deref(),
                            );
                        }
                    }
                    Err(e) => {
                        bot.delete_message(chat_id, processing.id).await.ok();
                        bot.send_message(chat_id, format!("❌ Ошибка при обработке видео: {}", e))
                            .await
                            .ok();
                    }
                }
            }
        }
        "clip" => {
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
                    bot.send_message(chat_id, "❌ У этой вырезки нет file_id для вырезки.")
                        .await
                        .ok();
                    return Ok(());
                }
                let session = db::VideoClipSession {
                    id: uuid::Uuid::new_v4().to_string(),
                    user_id: chat_id.0,
                    source_download_id: 0,
                    source_kind: "cut".to_string(),
                    source_id: cut_id,
                    original_url: cut.original_url.clone(),
                    output_kind: "cut".to_string(),
                    created_at: chrono::Utc::now(),
                    expires_at: chrono::Utc::now() + chrono::Duration::minutes(10),
                };
                db::upsert_video_clip_session(&conn, &session).map_err(|e| {
                    teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string())))
                })?;

                let keyboard = InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::callback(
                    "❌ Отмена".to_string(),
                    "cuts:clip_cancel".to_string(),
                )]]);

                bot.send_message(
                    chat_id,
                    "✂️ Отправь интервалы для вырезки в формате `мм:сс-мм:сс` или `чч:мм:сс-чч:мм:сс`\\.\nМожно несколько через запятую.\n\nПример: `00:10-00:25, 01:00-01:10`",
                )
                .parse_mode(ParseMode::MarkdownV2)
                .reply_markup(keyboard)
                .await?;

                if !cut.original_url.trim().is_empty() {
                    bot.send_message(chat_id, cut.original_url).await.ok();
                }
                bot.delete_message(chat_id, message_id).await.ok();
            }
        }
        "circle" => {
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
                    bot.send_message(chat_id, "❌ У этой вырезки нет file_id для кружка.")
                        .await
                        .ok();
                    return Ok(());
                }
                let session = db::VideoClipSession {
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
                db::upsert_video_clip_session(&conn, &session).map_err(|e| {
                    teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string())))
                })?;

                let keyboard = InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::callback(
                    "❌ Отмена".to_string(),
                    "cuts:clip_cancel".to_string(),
                )]]);

                bot.send_message(
                    chat_id,
                    "⭕️ Отправь интервалы для кружка в формате `мм:сс-мм:сс` или `чч:мм:сс-чч:мм:сс`\\.\nМожно несколько через запятую\\.\n\nИли используй команды:\n• `full` \\- всё видео\n• `first30` \\- первые 30 секунд\n• `last30` \\- последние 30 секунд\n• `middle30` \\- 30 секунд из середины\n\n💡 Можно добавить скорость: `first30 2x`, `full 1\\.5x`\n\n💡 Если длительность превысит 60 секунд \\(лимит Telegram\\), видео будет автоматически обрезано\\.\n\nПример: `00:10-00:25` или `first30 2x`",
                )
                .parse_mode(ParseMode::MarkdownV2)
                .reply_markup(keyboard)
                .await?;

                if !cut.original_url.trim().is_empty() {
                    bot.send_message(chat_id, cut.original_url).await.ok();
                }
                bot.delete_message(chat_id, message_id).await.ok();
            }
        }
        "clip_cancel" => {
            let conn = db::get_connection(&db_pool)
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;
            db::delete_video_clip_session_by_user(&conn, chat_id.0).ok();
            bot.delete_message(chat_id, message_id).await.ok();
        }
        "cancel" => {
            bot.delete_message(chat_id, message_id).await.ok();
        }
        "close" => {
            bot.delete_message(chat_id, message_id).await.ok();
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

    // Don't delete the first message unless the forced re-upload succeeds.

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

    let temp_dir = PathBuf::from("/tmp/doradura_speed");
    fs::create_dir_all(&temp_dir).await?;

    let input_path = temp_dir.join(format!("input_{}_{}.mp4", chat_id.0, uuid::Uuid::new_v4()));
    crate::telegram::download_file_from_telegram(bot, file_id, Some(input_path.clone()))
        .await
        .map_err(|e| format!("Failed to download file from Telegram: {}", e))?;

    let output_path = temp_dir.join(format!("output_{}_{}.mp4", chat_id.0, speed));

    let atempo_filter = if speed > 2.0 {
        format!("atempo=2.0,atempo={}", speed / 2.0)
    } else if speed < 0.5 {
        format!("atempo=0.5,atempo={}", speed / 0.5)
    } else {
        format!("atempo={}", speed)
    };

    let filter_complex = format!("[0:v]setpts={}*PTS[v];[0:a]{}[a]", 1.0 / speed, atempo_filter);

    let output = Command::new("ffmpeg")
        .arg("-i")
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
        .arg(&output_path)
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("ffmpeg failed: {}", stderr).into());
    }

    let sent = bot
        .send_video(chat_id, teloxide::types::InputFile::file(output_path.clone()))
        .caption(format!("{} (скорость {}x)", title, speed))
        .await?;
    let file_size = fs::metadata(&output_path).await.map(|m| m.len() as i64).unwrap_or(0);

    fs::remove_file(&input_path).await.ok();
    fs::remove_file(&output_path).await.ok();

    Ok((sent, file_size))
}
