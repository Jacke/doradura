use crate::core::config;
use crate::storage::{db, DbPool};
use crate::telegram::commands::{process_video_clip, CutSegment};
use crate::telegram::Bot;
use crate::timestamps::format_timestamp;
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

/// Build duration selection buttons for circle creation from cuts
/// Returns rows of buttons with time ranges (first/last/middle/full)
fn build_duration_buttons_for_cut(
    cut_id: i64,
    lang: &unic_langid::LanguageIdentifier,
) -> Vec<Vec<InlineKeyboardButton>> {
    let durations = [15, 30, 60];

    // Row 1: First N seconds (from beginning)
    let first_row: Vec<InlineKeyboardButton> = durations
        .iter()
        .map(|&dur| {
            let label = format!("▶ 0:00–{}", format_duration_short(dur));
            crate::telegram::cb(label, format!("cuts:dur:first:{}:{}", cut_id, dur))
        })
        .collect();

    // Row 2: Last N seconds (from end)
    let last_row: Vec<InlineKeyboardButton> = durations
        .iter()
        .map(|&dur| {
            let label = format!("◀ ...–{}", format_duration_short(dur));
            crate::telegram::cb(label, format!("cuts:dur:last:{}:{}", cut_id, dur))
        })
        .collect();

    // Row 3: Middle and Full (localized)
    let btn_middle = crate::i18n::t(lang, "video_circle.btn_middle");
    let btn_full = crate::i18n::t(lang, "video_circle.btn_full");
    let special_row = vec![
        crate::telegram::cb(btn_middle, format!("cuts:dur:middle:{}:30", cut_id)),
        crate::telegram::cb(btn_full, format!("cuts:dur:full:{}", cut_id)),
    ];

    vec![first_row, last_row, special_row]
}

/// Format duration as short string (0:15, 0:30, 1:00)
fn format_duration_short(seconds: i64) -> String {
    let mins = seconds / 60;
    let secs = seconds % 60;
    format!("{}:{:02}", mins, secs)
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
                "✂️ You have no clips yet.\n\nOpen /downloads and press ✂️ Clip on the desired video.",
            )
            .await;
    }

    let total_pages = total_items.div_ceil(ITEMS_PER_PAGE);
    let current_page = page.min(total_pages.saturating_sub(1));
    let offset = (current_page * ITEMS_PER_PAGE) as i64;

    let cuts = db::get_cuts_page(&conn, chat_id.0, ITEMS_PER_PAGE as i64, offset)
        .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

    let mut text = String::from("✂️ *Your clips*\n\n");
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
        rows.push(vec![crate::telegram::cb(
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
        nav.push(crate::telegram::cb(
            "⬅️".to_string(),
            format!("cuts:page:{}", current_page - 1),
        ));
    }
    if total_pages > 1 {
        nav.push(crate::telegram::cb(
            format!("{}/{}", current_page + 1, total_pages),
            format!("cuts:page:{}", current_page),
        ));
    }
    if current_page + 1 < total_pages {
        nav.push(crate::telegram::cb(
            "➡️".to_string(),
            format!("cuts:page:{}", current_page + 1),
        ));
    }
    if !nav.is_empty() {
        rows.push(nav);
    }

    rows.push(vec![crate::telegram::cb(
        "❌ Close".to_string(),
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
    username: Option<String>,
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
                    crate::telegram::cb("🎬 As video".to_string(), format!("cuts:send:video:{}", cut_id)),
                    crate::telegram::cb("📎 As document".to_string(), format!("cuts:send:document:{}", cut_id)),
                ]);
                options.push(vec![
                    crate::telegram::cb("✂️ Clip".to_string(), format!("cuts:clip:{}", cut_id)),
                    crate::telegram::cb("⭕️ Circle".to_string(), format!("cuts:circle:{}", cut_id)),
                    crate::telegram::cb("🔔 Ringtone".to_string(), format!("cuts:iphone_ringtone:{}", cut_id)),
                ]);
                options.push(vec![crate::telegram::cb(
                    "⚙️ Speed".to_string(),
                    format!("cuts:speed:{}", cut_id),
                )]);
                options.push(vec![crate::telegram::cb(
                    "❌ Cancel".to_string(),
                    "cuts:cancel".to_string(),
                )]);

                bot.send_message(
                    chat_id,
                    format!("What to do with *{}*?", crate::telegram::escape_markdown(&cut.title)),
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
                    bot.send_message(chat_id, "❌ This clip has no file_id for sending.")
                        .await
                        .ok();
                    return Ok(());
                };

                let status_text = match send_type {
                    "video" => "⏳ Preparing to send as video…",
                    "document" => "⏳ Preparing to send as document…",
                    _ => "⏳ Preparing to send…",
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
                        bot.send_message(chat_id, "❌ Unknown send mode.").await.ok();
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
                                        "⚠️ Couldn't send as document: Telegram rejected the file due to size.\nSent as video instead.\n\nIf you need it as a document — make a ✂️ shorter clip and send that as a document.",
                                    )
                                    .await
                                    .ok();
                                }
                                Err(e2) => {
                                    bot.send_message(chat_id, format!("❌ Failed to send file even as video: {e2}"))
                                        .await
                                        .ok();
                                }
                            }
                            return Ok(());
                        }
                        bot.delete_message(chat_id, status_msg.id).await.ok();
                        bot.send_message(chat_id, format!("❌ Failed to send file: {e}"))
                            .await
                            .ok();
                    }
                }
            }
        }
        "iphone_ringtone" => {
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
                    bot.send_message(chat_id, "❌ This clip has no file_id for ringtone.")
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
                    output_kind: "iphone_ringtone".to_string(),
                    created_at: chrono::Utc::now(),
                    expires_at: chrono::Utc::now() + chrono::Duration::minutes(10),
                };
                db::upsert_video_clip_session(&conn, &session).map_err(|e| {
                    teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string())))
                })?;

                let keyboard = InlineKeyboardMarkup::new(vec![vec![crate::telegram::cb(
                    "❌ Cancel".to_string(),
                    "cuts:clip_cancel".to_string(),
                )]]);

                bot.send_message(
                    chat_id,
                    "🔔 Send intervals for the ringtone in format `mm:ss-mm:ss` or `hh:mm:ss-hh:mm:ss`\\.\nMultiple intervals separated by commas\\.\n\n💡 If duration exceeds 30 seconds \\(iOS limit\\), audio will be automatically trimmed\\.\n\nExample: `00:10-00:25`",
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
                        crate::telegram::cb("0.5x".to_string(), format!("cuts:apply_speed:0.5:{}", cut_id)),
                        crate::telegram::cb("0.75x".to_string(), format!("cuts:apply_speed:0.75:{}", cut_id)),
                        crate::telegram::cb("1.0x".to_string(), format!("cuts:apply_speed:1.0:{}", cut_id)),
                    ],
                    vec![
                        crate::telegram::cb("1.25x".to_string(), format!("cuts:apply_speed:1.25:{}", cut_id)),
                        crate::telegram::cb("1.5x".to_string(), format!("cuts:apply_speed:1.5:{}", cut_id)),
                        crate::telegram::cb("2.0x".to_string(), format!("cuts:apply_speed:2.0:{}", cut_id)),
                    ],
                    vec![crate::telegram::cb("❌ Cancel".to_string(), "cuts:cancel".to_string())],
                ];

                bot.send_message(
                    chat_id,
                    format!("⚙️ Select speed for *{}*", crate::telegram::escape_markdown(&cut.title)),
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
                    bot.send_message(chat_id, "❌ This clip has no file_id for processing.")
                        .await
                        .ok();
                    return Ok(());
                };

                bot.delete_message(chat_id, message_id).await.ok();
                let processing = bot
                    .send_message(
                        chat_id,
                        format!(
                            "⚙️ Processing video at {}x speed\\.\\.\\.
This may take a few minutes\\.",
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
                        bot.send_message(
                            chat_id,
                            "❌ Failed to process video. The administrator has been notified.",
                        )
                        .await
                        .ok();
                        // Notify admin about the error with full details
                        crate::telegram::notifications::notify_admin_video_error(
                            bot,
                            chat_id.0,
                            username.as_deref(),
                            &e.to_string(),
                            &format!("cut_speed: {}x on '{}'", speed_str, cut.title),
                        )
                        .await;
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
                    bot.send_message(chat_id, "❌ This clip has no file_id for clipping.")
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

                let keyboard = InlineKeyboardMarkup::new(vec![vec![crate::telegram::cb(
                    "❌ Cancel".to_string(),
                    "cuts:clip_cancel".to_string(),
                )]]);

                bot.send_message(
                    chat_id,
                    "✂️ Send intervals for the clip in format `mm:ss-mm:ss` or `hh:mm:ss-hh:mm:ss`\\.\nMultiple intervals separated by commas\\.\n\nExample: `00:10-00:25, 01:00-01:10`",
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
                    bot.send_message(chat_id, "❌ This clip has no file_id for video note.")
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

                // Get user language for localization
                let lang = crate::i18n::user_lang(&conn, chat_id.0);

                // Build keyboard: duration buttons + cancel button
                let mut keyboard_rows = build_duration_buttons_for_cut(cut_id, &lang);
                keyboard_rows.push(vec![crate::telegram::cb(
                    crate::i18n::t(&lang, "common.cancel"),
                    "cuts:clip_cancel".to_string(),
                )]);
                let keyboard = InlineKeyboardMarkup::new(keyboard_rows);

                let message = crate::i18n::t(&lang, "video_circle.select_part");
                bot.send_message(chat_id, message)
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
        // Handle duration button clicks: cuts:dur:{position}:{cut_id}:{seconds}
        // position: first, last, middle, full
        "dur" => {
            if parts.len() < 4 {
                return Ok(());
            }
            let position = parts[2]; // first, last, middle, full
            let cut_id = parts[3].parse::<i64>().unwrap_or(0);
            let duration_seconds = if parts.len() >= 5 {
                parts[4].parse::<i64>().unwrap_or(30)
            } else {
                60 // default for "full"
            };

            let conn = db::get_connection(&db_pool)
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

            if let Some(cut) = db::get_cut_entry(&conn, chat_id.0, cut_id)
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
            {
                if cut.file_id.is_none() {
                    bot.send_message(chat_id, "❌ This clip has no file_id for video note.")
                        .await
                        .ok();
                    return Ok(());
                }

                // Delete the prompt message
                bot.delete_message(chat_id, message_id).await.ok();

                let cut_duration = cut.duration.unwrap_or(duration_seconds);

                // Calculate segment based on position
                let (start_secs, end_secs) = match position {
                    "first" => {
                        let end = std::cmp::min(duration_seconds, cut_duration).min(60);
                        (0, end)
                    }
                    "last" => {
                        let duration = std::cmp::min(duration_seconds, cut_duration).min(60);
                        let start = (cut_duration - duration).max(0);
                        (start, cut_duration.min(start + 60))
                    }
                    "middle" => {
                        let duration = std::cmp::min(duration_seconds, cut_duration).min(60);
                        let start = ((cut_duration - duration) / 2).max(0);
                        (start, (start + duration).min(cut_duration))
                    }
                    "full" => {
                        let end = cut_duration.min(60);
                        (0, end)
                    }
                    _ => (0, std::cmp::min(duration_seconds, 60)),
                };

                // Create session
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
                        log::error!("Failed to process duration circle from cut: {}", e);
                    }
                });
            }
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
            "⚠️ Cannot force send as document: local Bot API cannot find this file via /file (not in local cache/dir).\nKept as video.\n\n{}",
            bot_api_source_hint()
        ));
    }
    if lower.contains("local bot api file availability check failed")
        || lower.contains("local bot api file check failed")
    {
        return Some(format!(
            "⚠️ Cannot force send as document: error checking file on local Bot API.\nKept as video.\n\nReason: {}\n{}",
            short_error_text(download_error_text, 180),
            bot_api_source_hint()
        ));
    }
    if lower.contains("file is too big") {
        if config::bot_api::is_local() {
            return Some(format!(
                "⚠️ Cannot force send as document: local Bot API returned `file is too big` at the `getFile` stage.\nThis usually means the server is NOT running in `--local` mode (inheriting the ~20 MB official Bot API limit), or a server-side size limit is in effect.\nKept as video.\n\nReason: {}\n{}",
                short_error_text(download_error_text, 180),
                bot_api_source_hint()
            ));
        }
        return Some(format!(
            "⚠️ Cannot force send as document: to \"make a document\", the bot needs to download the file and re-upload it.\nOn the official Bot API, downloads are limited to ~20 MB; on the local Bot API this only works if the file is accessible via /file.\nKept as video.\n\nReason: {}\n{}",
            short_error_text(download_error_text, 180),
            bot_api_source_hint()
        ));
    }
    if lower.contains("telegram file download failed") {
        return Some(format!(
            "⚠️ Cannot force send as document: failed to download file from Bot API file endpoint.\nKept as video.\n\nReason: {}\n{}",
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
                    "⚠️ Couldn't force send as document: Telegram rejected the file due to size. Kept as video.",
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

    let temp_dir = PathBuf::from(crate::core::config::TEMP_FILES_DIR.as_str()).join("doradura_speed");
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
        .caption(format!("{} (speed {}x)", title, speed))
        .await?;
    let file_size = fs::metadata(&output_path).await.map(|m| m.len() as i64).unwrap_or(0);

    fs::remove_file(&input_path).await.ok();
    fs::remove_file(&output_path).await.ok();

    Ok((sent, file_size))
}
