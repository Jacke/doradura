use crate::core::{escape_markdown, escape_markdown_url};
use crate::downsub::DownsubGateway;
use crate::storage::{db, DbPool, SharedStorage, SubtitleCache};
use crate::telegram::commands::{process_video_clip, CutSegment};
use crate::telegram::Bot;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{CallbackQueryId, InlineKeyboardButton, InlineKeyboardMarkup, InputFile, MessageId, ParseMode};

use super::subtitles::{
    add_audio_tools_buttons_from_history, add_video_cut_button_from_history, change_video_speed,
    fetch_subtitles_for_command, send_document_forced,
};
use super::{build_duration_buttons, build_timestamp_ui, format_timestamp, is_youtube_url};

/// Handle downloads callback queries
pub async fn handle_downloads_callback(
    bot: &Bot,
    callback_id: CallbackQueryId,
    chat_id: ChatId,
    message_id: MessageId,
    data: &str,
    db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
    username: Option<String>,
    downsub_gateway: Arc<DownsubGateway>,
    subtitle_cache: Arc<SubtitleCache>,
) -> ResponseResult<()> {
    log::info!("📥 handle_downloads_callback called with data: {}", data);
    bot.answer_callback_query(callback_id).await?;

    let parts: Vec<&str> = data.splitn(6, ':').collect();
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
            super::show_downloads_page(
                bot,
                chat_id,
                db_pool,
                shared_storage.clone(),
                page,
                filter,
                search,
                None,
            )
            .await?;
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
            super::show_downloads_page(bot, chat_id, db_pool, shared_storage.clone(), 0, filter, search, None).await?;
        }
        "catfilter" => {
            if parts.len() < 5 {
                return Ok(());
            }
            let category = if parts[2].is_empty() {
                None
            } else {
                Some(urlencoding::decode(parts[2]).unwrap_or_default().to_string())
            };
            let format = if parts[3].is_empty() {
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
            super::show_downloads_page(
                bot,
                chat_id,
                db_pool,
                shared_storage.clone(),
                0,
                format,
                search,
                category,
            )
            .await?;
        }
        "resend" => {
            log::info!("📥 Handling resend action");
            if parts.len() < 3 {
                log::warn!("📥 Not enough parts for resend");
                return Ok(());
            }
            let download_id = parts[2].parse::<i64>().unwrap_or(0);
            log::info!("📥 Download ID: {}", download_id);

            if let Some(download) = shared_storage
                .get_download_history_entry(chat_id.0, download_id)
                .await
                .map_err(|e| {
                    log::error!("📥 Failed to get download entry: {}", e);
                    teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string())))
                })?
            {
                if download.file_id.is_some() {
                    // Show options: resend as audio/document/video
                    let mut options = Vec::new();

                    if download.format == "mp3" {
                        // Row 1: send formats
                        options.push(vec![
                            crate::telegram::cb(
                                "🎵 As audio".to_string(),
                                format!("downloads:send:audio:{}", download_id),
                            ),
                            crate::telegram::cb(
                                "📎 As document".to_string(),
                                format!("downloads:send:document:{}", download_id),
                            ),
                        ]);
                        // Row 2: transform actions
                        options.push(vec![
                            crate::telegram::cb("✂️ Clip".to_string(), format!("downloads:clip:{}", download_id)),
                            crate::telegram::cb("🎙 Voice".to_string(), format!("downloads:voice:{}", download_id)),
                            crate::telegram::cb(
                                "🔔 Ringtone".to_string(),
                                format!("ringtone:select:download:{}", download_id),
                            ),
                        ]);
                        // Row 3: speed + lyrics
                        options.push(vec![
                            crate::telegram::cb("⚙️ Speed".to_string(), format!("downloads:speed:{}", download_id)),
                            crate::telegram::cb("📝 Lyrics".to_string(), format!("downloads:lyrics:{}", download_id)),
                        ]);
                    } else {
                        // Row 1: send formats
                        options.push(vec![
                            crate::telegram::cb(
                                "🎬 As video".to_string(),
                                format!("downloads:send:video:{}", download_id),
                            ),
                            crate::telegram::cb(
                                "📎 As document".to_string(),
                                format!("downloads:send:document:{}", download_id),
                            ),
                        ]);
                        // Row 2: transform actions
                        options.push(vec![
                            crate::telegram::cb("✂️ Clip".to_string(), format!("downloads:clip:{}", download_id)),
                            crate::telegram::cb("⭕️ Circle".to_string(), format!("downloads:circle:{}", download_id)),
                            crate::telegram::cb(
                                "🔔 Ringtone".to_string(),
                                format!("ringtone:select:download:{}", download_id),
                            ),
                        ]);
                        // Row 3: speed + burn subs (YouTube mp4 only)
                        if is_youtube_url(&download.url) {
                            options.push(vec![
                                crate::telegram::cb("⚙️ Speed".to_string(), format!("downloads:speed:{}", download_id)),
                                crate::telegram::cb(
                                    "🔤 Burn subs".to_string(),
                                    format!("downloads:burn_subs:{}", download_id),
                                ),
                            ]);
                        } else {
                            options.push(vec![crate::telegram::cb(
                                "⚙️ Speed".to_string(),
                                format!("downloads:speed:{}", download_id),
                            )]);
                        }
                    }

                    // Category button
                    let cat_label = match &download.category {
                        Some(c) => format!("🏷 {}", c),
                        None => "🏷 Add to Category".to_string(),
                    };
                    options.push(vec![crate::telegram::cb(
                        cat_label,
                        format!("downloads:setcat:{}", download_id),
                    )]);

                    options.push(vec![crate::telegram::cb(
                        "❌ Cancel".to_string(),
                        "downloads:cancel".to_string(),
                    )]);

                    let keyboard = InlineKeyboardMarkup::new(options);
                    let msg_text = format!(
                        "How to send *{}*?\n[🔗 Source]({})",
                        escape_markdown(&download.title),
                        escape_markdown_url(&download.url),
                    );

                    crate::telegram::styled::send_message_styled_or_fallback_opts(
                        bot,
                        chat_id,
                        &msg_text,
                        &keyboard,
                        Some(ParseMode::MarkdownV2),
                        true,
                    )
                    .await?;
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

            if let Some(cut) = shared_storage.get_cut_entry(chat_id.0, cut_id).await.map_err(|e| {
                log::error!("📥 Failed to get cut entry: {}", e);
                teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string())))
            })? {
                log::info!("📥 Found cut: {:?}", cut);
                if cut.file_id.is_some() {
                    let mut options = Vec::new();

                    // Cuts are usually MP4
                    options.push(vec![
                        crate::telegram::cb(
                            "🎬 As video".to_string(),
                            format!("downloads:send_cut:video:{}", cut_id),
                        ),
                        crate::telegram::cb(
                            "📎 As document".to_string(),
                            format!("downloads:send_cut:document:{}", cut_id),
                        ),
                    ]);

                    options.push(vec![
                        crate::telegram::cb("✂️ Clip".to_string(), format!("downloads:clip_cut:{}", cut_id)),
                        crate::telegram::cb("⭕️ Circle".to_string(), format!("downloads:circle_cut:{}", cut_id)),
                        crate::telegram::cb("🔔 Ringtone".to_string(), format!("ringtone:select:cut:{}", cut_id)),
                    ]);

                    options.push(vec![crate::telegram::cb(
                        "⚙️ Speed".to_string(),
                        format!("downloads:speed_cut:{}", cut_id),
                    )]);

                    options.push(vec![crate::telegram::cb(
                        "❌ Cancel".to_string(),
                        "downloads:cancel".to_string(),
                    )]);

                    let keyboard = InlineKeyboardMarkup::new(options);
                    let msg_text = format!(
                        "How to send clip *{}*?\n[🔗 Source]({})",
                        escape_markdown(&cut.title),
                        escape_markdown_url(&cut.original_url),
                    );

                    crate::telegram::styled::send_message_styled_or_fallback_opts(
                        bot,
                        chat_id,
                        &msg_text,
                        &keyboard,
                        Some(ParseMode::MarkdownV2),
                        true,
                    )
                    .await?;
                }
            }
        }
        "send" => {
            if parts.len() < 4 {
                return Ok(());
            }
            let send_type = parts[2];
            let download_id = parts[3].parse::<i64>().unwrap_or(0);

            if let Some(download) = shared_storage
                .get_download_history_entry(chat_id.0, download_id)
                .await
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
            {
                if let Some(fid) = download.file_id {
                    let status_text = match send_type {
                        "audio" => "⏳ Preparing to send as audio…",
                        "video" => "⏳ Preparing to send as video…",
                        "document" => "⏳ Preparing to send as document…",
                        "voice" => "⏳ Converting to voice message…",
                        _ => "⏳ Preparing to send…",
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
                                    shared_storage.clone(),
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
                            bot.send_message(chat_id, format!("❌ Failed to send file: {e}"))
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

            if let Some(cut) = shared_storage
                .get_cut_entry(chat_id.0, cut_id)
                .await
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
            {
                if let Some(fid) = cut.file_id {
                    let status_text = match send_type {
                        "video" => "⏳ Preparing to send as video…",
                        "document" => "⏳ Preparing to send as document…",
                        _ => "⏳ Preparing to send…",
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
                            bot.delete_message(chat_id, status_msg.id).await.ok();
                            bot.send_message(chat_id, format!("❌ Failed to send file: {e}"))
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
            if let Some(download) = shared_storage
                .get_download_history_entry(chat_id.0, download_id)
                .await
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
            {
                if download.format != "mp4" {
                    bot.send_message(chat_id, "✂️ Clipping is only available for MP4\\.")
                        .parse_mode(ParseMode::MarkdownV2)
                        .await
                        .ok();
                    return Ok(());
                }
                if download.file_id.is_none() {
                    bot.send_message(chat_id, "❌ Could not find file\\_id for this file\\.")
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
                    subtitle_lang: None,
                };
                shared_storage
                    .clone()
                    .upsert_video_clip_session(&session)
                    .await
                    .map_err(|e| {
                        teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string())))
                    })?;

                // Fetch timestamps and build UI
                let timestamps = shared_storage
                    .get_video_timestamps(download_id)
                    .await
                    .unwrap_or_default();
                let (ts_buttons, ts_text) = build_timestamp_ui(&timestamps, "clip", download_id);

                // Build keyboard with timestamp buttons and cancel button
                let mut keyboard_rows = ts_buttons;
                keyboard_rows.push(vec![crate::telegram::cb(
                    "❌ Cancel".to_string(),
                    "downloads:clip_cancel".to_string(),
                )]);
                let keyboard = InlineKeyboardMarkup::new(keyboard_rows);

                let base_message = "✂️ Send the intervals to clip in the format `mm:ss-mm:ss` or `hh:mm:ss-hh:mm:ss`\\.\nMultiple ranges separated by commas\\.\n\nExample: `00:10-00:25, 01:00-01:10`";
                let message = format!("{}{}", base_message, ts_text);
                bot.send_message(chat_id, message)
                    .parse_mode(ParseMode::MarkdownV2)
                    .reply_markup(keyboard)
                    .await?;

                bot.delete_message(chat_id, message_id).await.ok();
            }
        }
        "clip_cut" => {
            if parts.len() < 3 {
                return Ok(());
            }
            let cut_id = parts[2].parse::<i64>().unwrap_or(0);
            if let Some(cut) = shared_storage
                .get_cut_entry(chat_id.0, cut_id)
                .await
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
            {
                if cut.file_id.is_none() {
                    bot.send_message(chat_id, "❌ Could not find file\\_id for this file\\.")
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
                    subtitle_lang: None,
                };
                shared_storage
                    .clone()
                    .upsert_video_clip_session(&session)
                    .await
                    .map_err(|e| {
                        teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string())))
                    })?;
                let keyboard = InlineKeyboardMarkup::new(vec![vec![crate::telegram::cb(
                    "❌ Cancel".to_string(),
                    "downloads:clip_cancel".to_string(),
                )]]);
                bot.send_message(chat_id, "✂️ Send the intervals to clip in the format `mm:ss-mm:ss` or `hh:mm:ss-hh:mm:ss`\\.\nMultiple ranges separated by commas\\.\n\nExample: `00:10-00:25, 01:00-01:10`").parse_mode(ParseMode::MarkdownV2).reply_markup(keyboard).await?;

                bot.delete_message(chat_id, message_id).await.ok();
            }
        }
        "circle" => {
            if parts.len() < 3 {
                return Ok(());
            }
            let download_id = parts[2].parse::<i64>().unwrap_or(0);
            if let Some(download) = shared_storage
                .get_download_history_entry(chat_id.0, download_id)
                .await
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
            {
                if download.format != "mp4" {
                    bot.send_message(chat_id, "⭕️ Circle is only available for MP4\\.")
                        .parse_mode(ParseMode::MarkdownV2)
                        .await
                        .ok();
                    return Ok(());
                }
                if download.file_id.is_none() {
                    bot.send_message(chat_id, "❌ Could not find file\\_id for this file\\.")
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
                    subtitle_lang: None,
                };
                shared_storage
                    .clone()
                    .upsert_video_clip_session(&session)
                    .await
                    .map_err(|e| {
                        teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string())))
                    })?;

                // Get user language for localization
                let lang = crate::i18n::user_lang_from_storage(&shared_storage, chat_id.0).await;

                // Fetch timestamps and build UI
                let timestamps = shared_storage
                    .get_video_timestamps(download_id)
                    .await
                    .unwrap_or_default();
                let (ts_buttons, ts_text) = build_timestamp_ui(&timestamps, "circle", download_id);

                // Build keyboard: duration buttons + subtitle button + timestamp buttons + cancel button
                let mut keyboard_rows = build_duration_buttons(download_id, &lang);
                keyboard_rows.extend(ts_buttons);
                // Subtitle button
                let subs_label = crate::i18n::t(&lang, "video_circle.subtitles_button");
                keyboard_rows.push(vec![crate::telegram::cb(
                    subs_label,
                    format!("downloads:circle_subs:{}", download_id),
                )]);
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
                bot.delete_message(chat_id, message_id).await.ok();
            }
        }
        "circle_cut" => {
            if parts.len() < 3 {
                return Ok(());
            }
            let cut_id = parts[2].parse::<i64>().unwrap_or(0);
            if let Some(cut) = shared_storage
                .get_cut_entry(chat_id.0, cut_id)
                .await
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
            {
                if cut.file_id.is_none() {
                    bot.send_message(chat_id, "❌ Could not find file\\_id for this file\\.")
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
                    subtitle_lang: None,
                };
                shared_storage
                    .clone()
                    .upsert_video_clip_session(&session)
                    .await
                    .map_err(|e| {
                        teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string())))
                    })?;
                let keyboard = InlineKeyboardMarkup::new(vec![vec![crate::telegram::cb(
                    "❌ Cancel".to_string(),
                    "downloads:clip_cancel".to_string(),
                )]]);
                bot.send_message(chat_id, "⭕️ Send the intervals for the circle in the format `mm:ss-mm:ss` or `hh:mm:ss-hh:mm:ss`\\.\nMultiple ranges separated by commas\\.\n\nExample: `00:10-00:25` or `first30 2x`").parse_mode(ParseMode::MarkdownV2).reply_markup(keyboard).await?;
                bot.delete_message(chat_id, message_id).await.ok();
            }
        }
        "clip_cancel" => {
            shared_storage
                .clone()
                .delete_video_clip_session_by_user(chat_id.0)
                .await
                .ok();
            bot.delete_message(chat_id, message_id).await.ok();
        }
        // Show subtitle language picker for circle creation
        // Callback format: downloads:circle_subs:{download_id}
        "circle_subs" => {
            if parts.len() < 3 {
                return Ok(());
            }
            let download_id = parts[2].parse::<i64>().unwrap_or(0);
            let lang = crate::i18n::user_lang_from_storage(&shared_storage, chat_id.0).await;

            // Build language picker keyboard (2 rows of 5 languages + "No subs" row)
            const SUBTITLE_LANGS: [&str; 10] = ["en", "ru", "uk", "es", "pt", "ar", "fa", "fr", "de", "hi"];
            let row1: Vec<InlineKeyboardButton> = SUBTITLE_LANGS[..5]
                .iter()
                .map(|&l| {
                    crate::telegram::cb(
                        l.to_string(),
                        format!("downloads:circle_sub_lang:{}:{}", l, download_id),
                    )
                })
                .collect();
            let row2: Vec<InlineKeyboardButton> = SUBTITLE_LANGS[5..]
                .iter()
                .map(|&l| {
                    crate::telegram::cb(
                        l.to_string(),
                        format!("downloads:circle_sub_lang:{}:{}", l, download_id),
                    )
                })
                .collect();
            let no_subs_row = vec![crate::telegram::cb(
                crate::i18n::t(&lang, "video_circle.subtitles_none"),
                format!("downloads:circle_sub_lang:none:{}", download_id),
            )];

            let keyboard = InlineKeyboardMarkup::new(vec![row1, row2, no_subs_row]);
            let text = crate::i18n::t(&lang, "video_circle.subtitles_select_lang");
            bot.edit_message_text(chat_id, message_id, text)
                .reply_markup(keyboard)
                .await
                .ok();
        }
        // Handle subtitle language selection for circle
        // Callback format: downloads:circle_sub_lang:{lang}:{download_id}
        "circle_sub_lang" => {
            if parts.len() < 4 {
                return Ok(());
            }
            let sub_lang = parts[2];
            let download_id = parts[3].parse::<i64>().unwrap_or(0);

            // Update session subtitle_lang
            if let Some(mut session) = shared_storage
                .clone()
                .get_active_video_clip_session(chat_id.0)
                .await
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
            {
                session.subtitle_lang = if sub_lang == "none" {
                    None
                } else {
                    Some(sub_lang.to_string())
                };
                shared_storage
                    .clone()
                    .upsert_video_clip_session(&session)
                    .await
                    .map_err(|e| {
                        teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string())))
                    })?;

                // Rebuild circle menu with updated subtitle state
                let lang = crate::i18n::user_lang_from_storage(&shared_storage, chat_id.0).await;
                let timestamps = shared_storage
                    .get_video_timestamps(download_id)
                    .await
                    .unwrap_or_default();
                let (ts_buttons, ts_text) = build_timestamp_ui(&timestamps, "circle", download_id);

                let mut keyboard_rows = build_duration_buttons(download_id, &lang);
                keyboard_rows.extend(ts_buttons);
                // Subtitle button with current state
                let subs_label = match &session.subtitle_lang {
                    Some(sl) => {
                        let mut args = fluent_templates::fluent_bundle::FluentArgs::new();
                        args.set("lang", sl.as_str());
                        crate::i18n::t_args(&lang, "video_circle.subtitles_button_active", &args)
                    }
                    None => crate::i18n::t(&lang, "video_circle.subtitles_button"),
                };
                keyboard_rows.push(vec![crate::telegram::cb(
                    subs_label,
                    format!("downloads:circle_subs:{}", download_id),
                )]);
                keyboard_rows.push(vec![crate::telegram::cb(
                    crate::i18n::t(&lang, "common.cancel"),
                    "downloads:clip_cancel".to_string(),
                )]);
                let keyboard = InlineKeyboardMarkup::new(keyboard_rows);

                let base_message = crate::i18n::t(&lang, "video_circle.select_part");
                let message = format!("{}{}", base_message, ts_text);
                bot.edit_message_text(chat_id, message_id, message)
                    .parse_mode(ParseMode::MarkdownV2)
                    .reply_markup(keyboard)
                    .await
                    .ok();
            }
        }
        // Handle timestamp button clicks: downloads:ts:{output_kind}:{download_id}:{time_seconds}
        "ts" => {
            if parts.len() < 5 {
                return Ok(());
            }
            let output_kind = parts[2]; // "circle" or "clip"
            let download_id = parts[3].parse::<i64>().unwrap_or(0);
            let time_seconds = parts[4].parse::<i64>().unwrap_or(0);

            if let Some(download) = shared_storage
                .get_download_history_entry(chat_id.0, download_id)
                .await
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
                    subtitle_lang: None,
                };

                // Delete any existing session first
                shared_storage
                    .clone()
                    .delete_video_clip_session_by_user(chat_id.0)
                    .await
                    .ok();

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
                        shared_storage.clone(),
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

            if let Some(download) = shared_storage
                .get_download_history_entry(chat_id.0, download_id)
                .await
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
                    subtitle_lang: None,
                };

                // Delete any existing session first
                shared_storage
                    .clone()
                    .delete_video_clip_session_by_user(chat_id.0)
                    .await
                    .ok();

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
                        shared_storage.clone(),
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
            if let Some(download) = shared_storage
                .get_download_history_entry(chat_id.0, download_id)
                .await
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
                        "❌ Cancel".to_string(),
                        "downloads:cancel".to_string(),
                    )],
                ];
                let keyboard = InlineKeyboardMarkup::new(speed_options);
                bot.send_message(
                    chat_id,
                    format!("⚙️ Choose speed for *{}*", escape_markdown(&download.title)),
                )
                .parse_mode(ParseMode::MarkdownV2)
                .reply_markup(keyboard)
                .await?;

                bot.delete_message(chat_id, message_id).await.ok();
            }
        }
        "speed_cut" => {
            if parts.len() < 3 {
                return Ok(());
            }
            let cut_id = parts[2].parse::<i64>().unwrap_or(0);
            if let Some(cut) = shared_storage
                .get_cut_entry(chat_id.0, cut_id)
                .await
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
                        "❌ Cancel".to_string(),
                        "downloads:cancel".to_string(),
                    )],
                ];
                let keyboard = InlineKeyboardMarkup::new(speed_options);
                bot.send_message(
                    chat_id,
                    format!("⚙️ Choose speed for clip *{}*", escape_markdown(&cut.title)),
                )
                .parse_mode(ParseMode::MarkdownV2)
                .reply_markup(keyboard)
                .await?;

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
            if let Some(download) = shared_storage
                .get_download_history_entry(chat_id.0, download_id)
                .await
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
            {
                if let Some(file_id) = download.file_id {
                    bot.delete_message(chat_id, message_id).await.ok();
                    let processing_msg = bot
                        .send_message(
                            chat_id,
                            format!(
                                "⚙️ Processing video at speed {}x\\.\\.\\.  \nThis may take a few minutes\\.",
                                speed_str.replace(".", "\\.")
                            ),
                        )
                        .parse_mode(ParseMode::MarkdownV2)
                        .await?;
                    match change_video_speed(bot, chat_id, &file_id, speed, &download.title).await {
                        Ok((sent_message, file_size)) => {
                            bot.delete_message(chat_id, processing_msg.id).await.ok();

                            let new_title = format!("{} [speed {}x]", download.title, speed_str);
                            let new_duration = download.duration.map(|d| ((d as f32) / speed).round().max(1.0) as i64);
                            let new_file_id = sent_message
                                .video()
                                .map(|v| v.file.id.0.clone())
                                .or_else(|| sent_message.document().map(|d| d.file.id.0.clone()))
                                .or_else(|| sent_message.audio().map(|a| a.file.id.0.clone()));
                            if let Some(fid) = new_file_id {
                                if let Ok(db_id) = shared_storage
                                    .save_download_history(
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
                                    )
                                    .await
                                {
                                    // Save message_id for MTProto file_reference refresh
                                    let _ = shared_storage
                                        .update_download_message_id(db_id, sent_message.id.0, chat_id.0)
                                        .await;
                                }
                            }
                        }
                        Err(e) => {
                            bot.delete_message(chat_id, processing_msg.id).await.ok();
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
            if let Some(cut) = shared_storage
                .get_cut_entry(chat_id.0, cut_id)
                .await
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
            {
                if let Some(file_id) = cut.file_id {
                    bot.delete_message(chat_id, message_id).await.ok();
                    let processing_msg = bot
                        .send_message(
                            chat_id,
                            format!(
                                "⚙️ Processing clip at speed {}x\\.\\.\\.  \nThis may take a few minutes\\.",
                                speed_str.replace(".", "\\.")
                            ),
                        )
                        .parse_mode(ParseMode::MarkdownV2)
                        .await?;
                    match change_video_speed(bot, chat_id, &file_id, speed, &cut.title).await {
                        Ok((sent_message, file_size)) => {
                            bot.delete_message(chat_id, processing_msg.id).await.ok();

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
                                if let Ok(db_id) = shared_storage
                                    .save_download_history(
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
                                    )
                                    .await
                                {
                                    // Save message_id for MTProto file_reference refresh
                                    let _ = shared_storage
                                        .update_download_message_id(db_id, sent_message.id.0, chat_id.0)
                                        .await;
                                }
                            }
                        }
                        Err(e) => {
                            bot.delete_message(chat_id, processing_msg.id).await.ok();
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
                                &format!("apply_speed_cut: {}x on '{}'", speed_str, cut.title),
                            )
                            .await;
                        }
                    }
                }
            }
        }
        "subtitles" => {
            if parts.len() < 3 {
                return Ok(());
            }
            let download_id = parts[2].parse::<i64>().unwrap_or(0);
            if let Some(download) = shared_storage
                .get_download_history_entry(chat_id.0, download_id)
                .await
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
            {
                let loading_msg = bot
                    .edit_message_text(chat_id, message_id, "⏳ Fetching subtitles (SRT \\+ TXT)…")
                    .parse_mode(ParseMode::MarkdownV2)
                    .await
                    .ok();

                let url = download.url.clone();
                let lang = ""; // default: server decides

                match fetch_subtitles_for_command(&downsub_gateway, &subtitle_cache, chat_id.0, &url, lang).await {
                    Ok((srt_content, txt_content, segment_count)) => {
                        if let Some(msg) = loading_msg {
                            bot.edit_message_text(chat_id, msg.id, format!("✅ {} segments fetched", segment_count))
                                .await
                                .ok();
                        }
                        bot.send_document(
                            chat_id,
                            InputFile::memory(srt_content.into_bytes()).file_name("subtitles.srt"),
                        )
                        .await
                        .ok();
                        bot.send_document(
                            chat_id,
                            InputFile::memory(txt_content.into_bytes()).file_name("subtitles.txt"),
                        )
                        .await
                        .ok();
                    }
                    Err(e) => {
                        if let Some(msg) = loading_msg {
                            bot.edit_message_text(chat_id, msg.id, format!("❌ Error: {}", e))
                                .await
                                .ok();
                        } else {
                            bot.send_message(chat_id, format!("❌ Error: {}", e)).await.ok();
                        }
                    }
                }
            }
        }
        // Show language picker for burning subtitles into video
        "burn_subs" => {
            if parts.len() < 3 {
                return Ok(());
            }
            let download_id = parts[2].parse::<i64>().unwrap_or(0);
            if let Some(download) = shared_storage
                .get_download_history_entry(chat_id.0, download_id)
                .await
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
            {
                let lang_options = vec![
                    vec![
                        crate::telegram::cb("en".to_string(), format!("downloads:burn_subs_lang:en:{}", download_id)),
                        crate::telegram::cb("ru".to_string(), format!("downloads:burn_subs_lang:ru:{}", download_id)),
                        crate::telegram::cb("uk".to_string(), format!("downloads:burn_subs_lang:uk:{}", download_id)),
                        crate::telegram::cb("es".to_string(), format!("downloads:burn_subs_lang:es:{}", download_id)),
                        crate::telegram::cb("pt".to_string(), format!("downloads:burn_subs_lang:pt:{}", download_id)),
                    ],
                    vec![
                        crate::telegram::cb("ar".to_string(), format!("downloads:burn_subs_lang:ar:{}", download_id)),
                        crate::telegram::cb("fa".to_string(), format!("downloads:burn_subs_lang:fa:{}", download_id)),
                        crate::telegram::cb("fr".to_string(), format!("downloads:burn_subs_lang:fr:{}", download_id)),
                        crate::telegram::cb("de".to_string(), format!("downloads:burn_subs_lang:de:{}", download_id)),
                        crate::telegram::cb("hi".to_string(), format!("downloads:burn_subs_lang:hi:{}", download_id)),
                    ],
                    vec![crate::telegram::cb(
                        "❌ Cancel".to_string(),
                        "downloads:cancel".to_string(),
                    )],
                ];
                let keyboard = InlineKeyboardMarkup::new(lang_options);
                bot.send_message(
                    chat_id,
                    format!("🔤 Choose subtitle language for *{}*", escape_markdown(&download.title)),
                )
                .parse_mode(ParseMode::MarkdownV2)
                .reply_markup(keyboard)
                .await?;
                bot.delete_message(chat_id, message_id).await.ok();
            }
        }
        // Process: download video from Telegram, burn subtitles, send back
        "burn_subs_lang" => {
            if parts.len() < 4 {
                return Ok(());
            }
            let lang_code = parts[2].to_string();
            let download_id = parts[3].parse::<i64>().unwrap_or(0);
            if let Some(download) = shared_storage
                .get_download_history_entry(chat_id.0, download_id)
                .await
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
            {
                if let Some(file_id) = download.file_id.clone() {
                    bot.delete_message(chat_id, message_id).await.ok();

                    let user_lang = crate::i18n::user_lang_from_storage(&shared_storage, chat_id.0).await;
                    let mut args = fluent_templates::fluent_bundle::FluentArgs::new();
                    args.set("lang", lang_code.as_str());
                    let status_text = crate::i18n::t_args(&user_lang, "video_circle.burn_subs_status", &args);
                    let processing_msg = bot.send_message(chat_id, status_text).await?;

                    // Get message_id for MTProto fallback
                    let message_info = shared_storage
                        .get_download_message_info(download_id)
                        .await
                        .ok()
                        .flatten();
                    let (fallback_message_id, fallback_chat_id) = message_info.unzip();

                    let bot = bot.clone();
                    let url = download.url.clone();
                    let title = download.title.clone();
                    let shared_storage = Arc::clone(&shared_storage);
                    let username = username.clone();

                    tokio::spawn(async move {
                        let guard = match crate::core::utils::TempDirGuard::new("doradura_burn_subs").await {
                            Ok(g) => g,
                            Err(e) => {
                                log::error!("Failed to create burn_subs temp dir: {}", e);
                                bot.edit_message_text(chat_id, processing_msg.id, "❌ Internal error")
                                    .await
                                    .ok();
                                return;
                            }
                        };

                        let input_path = guard
                            .path()
                            .join(format!("input_{}_{}.mp4", chat_id.0, uuid::Uuid::new_v4()));

                        // Download video from Telegram
                        let download_result = crate::telegram::download_file_with_fallback(
                            &bot,
                            &file_id,
                            fallback_message_id,
                            fallback_chat_id,
                            Some(input_path.clone()),
                        )
                        .await;

                        if let Err(e) = download_result {
                            log::error!("Failed to download video for burn_subs: {}", e);
                            bot.edit_message_text(
                                chat_id,
                                processing_msg.id,
                                "❌ Failed to download video from Telegram",
                            )
                            .await
                            .ok();
                            return;
                        }

                        // Burn subtitles using existing function from commands.rs
                        use crate::telegram::commands::BurnSubsResult;
                        let burn_result = crate::telegram::commands::burn_circle_subtitles(
                            &url,
                            &lang_code,
                            &input_path,
                            guard.path(),
                            chat_id.0,
                            download_id,
                        )
                        .await;

                        let actual_path = match burn_result {
                            BurnSubsResult::Burned(path) => path,
                            BurnSubsResult::SubtitleReady(_) => {
                                // Legacy path: should not happen since burn_circle_subtitles
                                // always returns Burned, not SubtitleReady
                                input_path.clone()
                            }
                            BurnSubsResult::NotFound => {
                                bot.edit_message_text(
                                    chat_id,
                                    processing_msg.id,
                                    format!("❌ No {} subtitles found for this video", lang_code),
                                )
                                .await
                                .ok();
                                return;
                            }
                            BurnSubsResult::Failed(reason) => {
                                log::error!("❌ Subtitle burn failed: {}", reason);
                                // Truncate reason for Telegram (keep last meaningful line)
                                let short_reason = reason
                                    .lines()
                                    .rev()
                                    .find(|l| l.starts_with("ERROR:") || l.contains("Error"))
                                    .unwrap_or(&reason)
                                    .chars()
                                    .take(200)
                                    .collect::<String>();
                                bot.edit_message_text(
                                    chat_id,
                                    processing_msg.id,
                                    format!("❌ Failed to burn {} subtitles: {}", lang_code, short_reason),
                                )
                                .await
                                .ok();
                                return;
                            }
                        };

                        // Send the video with burned subtitles
                        let caption = format!("{} [{} subs]", title, lang_code);
                        let send_result = bot
                            .send_video(chat_id, InputFile::file(&actual_path))
                            .caption(&caption)
                            .await;

                        match send_result {
                            Ok(sent_message) => {
                                bot.delete_message(chat_id, processing_msg.id).await.ok();

                                // Save to download history
                                let new_file_id = sent_message
                                    .video()
                                    .map(|v| v.file.id.0.clone())
                                    .or_else(|| sent_message.document().map(|d| d.file.id.0.clone()));
                                let file_size = tokio::fs::metadata(&actual_path)
                                    .await
                                    .map(|m| m.len() as i64)
                                    .unwrap_or(0);
                                if let Some(fid) = new_file_id {
                                    let new_title = format!("{} [{} subs]", title, lang_code);
                                    if let Ok(db_id) = shared_storage
                                        .save_download_history(
                                            chat_id.0,
                                            &url,
                                            &new_title,
                                            "mp4",
                                            Some(&fid),
                                            None,
                                            Some(file_size),
                                            None,
                                            None,
                                            None,
                                            None,
                                            None,
                                        )
                                        .await
                                    {
                                        let _ = shared_storage
                                            .update_download_message_id(db_id, sent_message.id.0, chat_id.0)
                                            .await;
                                    }
                                }

                                // Add video cut button
                                if let Err(e) =
                                    add_video_cut_button_from_history(&bot, chat_id, sent_message.id, download_id).await
                                {
                                    log::warn!("Failed to add video cut button after burn_subs: {}", e);
                                }
                            }
                            Err(e) => {
                                log::error!("Failed to send video with burned subs: {}", e);
                                bot.edit_message_text(
                                    chat_id,
                                    processing_msg.id,
                                    "❌ Failed to send video. The administrator has been notified.",
                                )
                                .await
                                .ok();
                                crate::telegram::notifications::notify_admin_video_error(
                                    &bot,
                                    chat_id.0,
                                    username.as_deref(),
                                    &e.to_string(),
                                    &format!("burn_subs: {} on '{}'", lang_code, title),
                                )
                                .await;
                            }
                        }

                        // guard drops here, cleaning up the temp dir
                    });
                }
            }
        }
        // Show voice message duration picker (like circle)
        "voice" => {
            if parts.len() < 3 {
                return Ok(());
            }
            let download_id = parts[2].parse::<i64>().unwrap_or(0);
            if let Some(download) = shared_storage
                .get_download_history_entry(chat_id.0, download_id)
                .await
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
            {
                if download.file_id.is_none() {
                    return Ok(());
                }
                let durations = [15, 30, 60];
                let first_row: Vec<_> = durations
                    .iter()
                    .map(|&d| {
                        crate::telegram::cb(
                            format!("▶ 0:00–{}", super::format_duration_short(d)),
                            format!("downloads:voice_dur:first:{}:{}", download_id, d),
                        )
                    })
                    .collect();
                let last_row: Vec<_> = durations
                    .iter()
                    .map(|&d| {
                        crate::telegram::cb(
                            format!("◀ ...–{}", super::format_duration_short(d)),
                            format!("downloads:voice_dur:last:{}:{}", download_id, d),
                        )
                    })
                    .collect();
                let full_row = vec![crate::telegram::cb(
                    "🔊 Full".to_string(),
                    format!("downloads:voice_dur:full:{}", download_id),
                )];
                let mut keyboard_rows = vec![first_row, last_row, full_row];
                keyboard_rows.push(vec![crate::telegram::cb(
                    "❌ Cancel".to_string(),
                    "downloads:cancel".to_string(),
                )]);
                let keyboard = InlineKeyboardMarkup::new(keyboard_rows);
                let title = escape_markdown(&download.title);
                bot.send_message(chat_id, format!("🎙 Select voice message duration for *{}*:", title))
                    .parse_mode(ParseMode::MarkdownV2)
                    .reply_markup(keyboard)
                    .await?;
                bot.delete_message(chat_id, message_id).await.ok();
            }
        }
        // Process voice message with selected duration
        "voice_dur" => {
            if parts.len() < 4 {
                return Ok(());
            }
            let position = parts[2]; // first, last, middle, full
            let download_id = parts[3].parse::<i64>().unwrap_or(0);
            let duration_seconds = if parts.len() >= 5 {
                parts[4].parse::<i64>().unwrap_or(30)
            } else {
                60
            };

            if let Some(download) = shared_storage
                .get_download_history_entry(chat_id.0, download_id)
                .await
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
            {
                if let Some(ref fid) = download.file_id {
                    bot.delete_message(chat_id, message_id).await.ok();
                    let status_msg = bot.send_message(chat_id, "🎙 Converting to voice message…").await?;

                    let audio_duration = download.duration.unwrap_or(duration_seconds);
                    let (start_secs, end_secs) = match position {
                        "first" => (0i64, duration_seconds.min(audio_duration)),
                        "last" => {
                            let start = (audio_duration - duration_seconds).max(0);
                            (start, audio_duration)
                        }
                        "middle" => {
                            let start = ((audio_duration - duration_seconds) / 2).max(0);
                            (start, (start + duration_seconds).min(audio_duration))
                        }
                        "full" => (0, audio_duration),
                        _ => (0, duration_seconds.min(audio_duration)),
                    };

                    let seg_duration = end_secs - start_secs;
                    log::info!(
                        "Voice segment: start={}s, duration={}s for download {}",
                        start_secs,
                        seg_duration,
                        download_id
                    );
                    let result = send_as_voice_segment(bot, chat_id, fid, start_secs, seg_duration).await;

                    bot.delete_message(chat_id, status_msg.id).await.ok();
                    match result {
                        Ok(_) => log::info!("Voice message sent successfully for download {}", download_id),
                        Err(e) => {
                            log::error!("Voice conversion failed for download {}: {}", download_id, e);
                            bot.send_message(chat_id, format!("❌ Voice conversion failed: {e}"))
                                .await
                                .ok();
                        }
                    }
                }
            }
        }
        // Fetch lyrics and show section picker — selecting a section re-sends the audio with lyrics caption
        "lyrics" => {
            if parts.len() < 3 {
                return Ok(());
            }
            let download_id = parts[2].parse::<i64>().unwrap_or(0);

            if let Some(download) = shared_storage
                .get_download_history_entry(chat_id.0, download_id)
                .await
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
            {
                let display_title = if let Some(ref author) = download.author {
                    format!("{} - {}", author, download.title)
                } else {
                    download.title.clone()
                };
                let (artist, title) = crate::lyrics::parse_artist_title(&display_title);

                let status_msg = bot.send_message(chat_id, "🎵 Fetching lyrics…").await?;

                match crate::lyrics::fetch_lyrics(artist, title, None).await {
                    None => {
                        bot.delete_message(chat_id, status_msg.id).await.ok();
                        let escaped = escape_markdown(&display_title);
                        bot.send_message(chat_id, format!("❌ Lyrics not found for *{}*", escaped))
                            .parse_mode(ParseMode::MarkdownV2)
                            .await?;
                    }
                    Some(lyr) => {
                        bot.delete_message(chat_id, status_msg.id).await.ok();

                        // Save lyrics session — use short ID (Telegram callback data max 64 bytes)
                        let session_id = uuid::Uuid::new_v4().to_string()[..8].to_string();
                        let sections_json = serde_json::to_string(&lyr.sections).unwrap_or_default();
                        let _ = shared_storage
                            .create_lyrics_session(
                                &session_id,
                                chat_id.0,
                                &lyr.artist,
                                &lyr.title,
                                &sections_json,
                                lyr.has_structure,
                            )
                            .await;

                        // Build section picker — callbacks go to downloads:lyrics_send:{download_id}:{session_id}:{idx}
                        let display = format!("{} – {}", lyr.artist, lyr.title);
                        let keyboard = build_lyrics_audio_keyboard(download_id, &session_id, &lyr.sections);
                        let msg = if lyr.has_structure && lyr.sections.len() > 1 {
                            format!("🎵 {}\nChoose a section to send with audio:", display)
                        } else {
                            format!("🎵 {}\nSend audio with lyrics?", display)
                        };
                        bot.send_message(chat_id, msg).reply_markup(keyboard).await?;
                    }
                }
                bot.delete_message(chat_id, message_id).await.ok();
            }
        }
        // Re-send audio file with selected lyrics as caption
        "lyrics_send" => {
            // downloads:lyrics_send:{download_id}:{session_id}:{idx_or_all}
            if parts.len() < 5 {
                return Ok(());
            }
            let download_id = parts[2].parse::<i64>().unwrap_or(0);
            let session_id = parts[3];
            let idx_str = parts[4];

            let download = shared_storage
                .get_download_history_entry(chat_id.0, download_id)
                .await
                .ok()
                .flatten();
            let lyrics_session = shared_storage.get_lyrics_session(session_id).await.ok().flatten();

            if let (Some(dl), Some((_artist, _title, sections_json, _has_struct))) = (download, lyrics_session) {
                if let Some(ref fid) = dl.file_id {
                    let sections: Vec<crate::lyrics::LyricsSection> =
                        serde_json::from_str(&sections_json).unwrap_or_default();

                    let lyrics_text = if idx_str == "all" {
                        sections
                            .iter()
                            .map(|s| format!("[{}]\n{}", s.name, s.text()))
                            .collect::<Vec<_>>()
                            .join("\n\n")
                    } else if let Ok(idx) = idx_str.parse::<usize>() {
                        sections
                            .get(idx)
                            .map(|s| format!("[{}]\n{}", s.name, s.text()))
                            .unwrap_or_default()
                    } else {
                        String::new()
                    };

                    if lyrics_text.is_empty() {
                        bot.send_message(chat_id, "❌ Section not found.").await?;
                    } else {
                        // Telegram caption limit is 1024 chars — truncate on char boundary
                        let caption = if lyrics_text.chars().count() > 1024 {
                            let truncated: String = lyrics_text.chars().take(1020).collect();
                            format!("{truncated}…")
                        } else {
                            lyrics_text
                        };
                        bot.send_audio(
                            chat_id,
                            teloxide::types::InputFile::file_id(teloxide::types::FileId(fid.clone())),
                        )
                        .caption(caption)
                        .await?;
                    }
                    bot.delete_message(chat_id, message_id).await.ok();
                }
            }
        }
        // Show category picker for a download
        "setcat" => {
            if parts.len() < 3 {
                return Ok(());
            }
            let download_id = parts[2].parse::<i64>().unwrap_or(0);
            let user_cats = shared_storage.get_user_categories(chat_id.0).await.unwrap_or_default();
            let download = shared_storage
                .get_download_history_entry(chat_id.0, download_id)
                .await
                .ok()
                .flatten();
            let mut rows: Vec<Vec<InlineKeyboardButton>> = user_cats
                .iter()
                .map(|c| {
                    vec![crate::telegram::cb(
                        c.clone(),
                        format!("downloads:savecat:{}:{}", download_id, urlencoding::encode(c)),
                    )]
                })
                .collect();
            rows.push(vec![crate::telegram::cb(
                "➕ New category".to_string(),
                format!("downloads:newcat:{}", download_id),
            )]);
            if download.as_ref().and_then(|d| d.category.as_ref()).is_some() {
                rows.push(vec![crate::telegram::cb(
                    "✖ Remove category".to_string(),
                    format!("downloads:savecat:{}:", download_id),
                )]);
            }
            rows.push(vec![crate::telegram::cb(
                "« Back".to_string(),
                format!("downloads:resend:{}", download_id),
            )]);
            bot.edit_message_reply_markup(chat_id, message_id)
                .reply_markup(InlineKeyboardMarkup::new(rows))
                .await
                .ok();
        }
        // Save category assignment
        "savecat" => {
            if parts.len() < 3 {
                return Ok(());
            }
            let download_id = parts[2].parse::<i64>().unwrap_or(0);
            let category = parts
                .get(3)
                .filter(|s| !s.is_empty())
                .map(|s| urlencoding::decode(s).unwrap_or_default().to_string());
            let _ = shared_storage
                .set_download_category(chat_id.0, download_id, category.as_deref())
                .await;

            // Reload download and rebuild resend keyboard with updated category button
            if let Ok(Some(download)) = shared_storage.get_download_history_entry(chat_id.0, download_id).await {
                let mut options: Vec<Vec<InlineKeyboardButton>> = Vec::new();
                if download.format == "mp3" {
                    options.push(vec![
                        crate::telegram::cb(
                            "🎵 As audio".to_string(),
                            format!("downloads:send:audio:{}", download_id),
                        ),
                        crate::telegram::cb(
                            "📎 As document".to_string(),
                            format!("downloads:send:document:{}", download_id),
                        ),
                    ]);
                    options.push(vec![
                        crate::telegram::cb("✂️ Clip".to_string(), format!("downloads:clip:{}", download_id)),
                        crate::telegram::cb("⭕️ Circle".to_string(), format!("downloads:circle:{}", download_id)),
                        crate::telegram::cb(
                            "🔔 Make ringtone".to_string(),
                            format!("ringtone:select:download:{}", download_id),
                        ),
                    ]);
                    options.push(vec![crate::telegram::cb(
                        "⚙️ Change speed".to_string(),
                        format!("downloads:speed:{}", download_id),
                    )]);
                } else {
                    options.push(vec![
                        crate::telegram::cb(
                            "🎬 As video".to_string(),
                            format!("downloads:send:video:{}", download_id),
                        ),
                        crate::telegram::cb(
                            "📎 As document".to_string(),
                            format!("downloads:send:document:{}", download_id),
                        ),
                    ]);
                    options.push(vec![
                        crate::telegram::cb("✂️ Clip".to_string(), format!("downloads:clip:{}", download_id)),
                        crate::telegram::cb("⭕️ Circle".to_string(), format!("downloads:circle:{}", download_id)),
                        crate::telegram::cb(
                            "🔔 Make ringtone".to_string(),
                            format!("ringtone:select:download:{}", download_id),
                        ),
                    ]);
                    options.push(vec![crate::telegram::cb(
                        "⚙️ Change speed".to_string(),
                        format!("downloads:speed:{}", download_id),
                    )]);
                }
                if is_youtube_url(&download.url) {
                    options.push(vec![crate::telegram::cb(
                        "📝 Subtitles".to_string(),
                        format!("downloads:subtitles:{}", download_id),
                    )]);
                    if download.format == "mp4" {
                        options.push(vec![crate::telegram::cb(
                            "🔤 Burn subtitles".to_string(),
                            format!("downloads:burn_subs:{}", download_id),
                        )]);
                    }
                }
                let cat_label = match &download.category {
                    Some(c) => format!("🏷 {}", c),
                    None => "🏷 Add to Category".to_string(),
                };
                options.push(vec![crate::telegram::cb(
                    cat_label,
                    format!("downloads:setcat:{}", download_id),
                )]);
                options.push(vec![crate::telegram::cb(
                    "❌ Cancel".to_string(),
                    "downloads:cancel".to_string(),
                )]);
                bot.edit_message_reply_markup(chat_id, message_id)
                    .reply_markup(InlineKeyboardMarkup::new(options))
                    .await
                    .ok();
            }
        }
        // Start new-category text session
        "newcat" => {
            if parts.len() < 3 {
                return Ok(());
            }
            let download_id = parts[2].parse::<i64>().unwrap_or(0);
            let _ = shared_storage.create_new_category_session(chat_id.0, download_id).await;
            bot.edit_message_text(
                chat_id,
                message_id,
                "📝 *New Category*\n\nSend a name for the new category:",
            )
            .parse_mode(ParseMode::MarkdownV2)
            .reply_markup(InlineKeyboardMarkup::new(vec![vec![crate::telegram::cb(
                "« Cancel".to_string(),
                format!("downloads:setcat:{}", download_id),
            )]]))
            .await?;
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

/// Download an audio file from Telegram, convert to OGG Opus, and send as a voice message.
/// Download audio, extract segment, convert to OGG Opus mono, send as voice.
async fn send_as_voice_segment(
    bot: &Bot,
    chat_id: ChatId,
    telegram_file_id: &str,
    start_secs: i64,
    duration_secs: i64,
) -> ResponseResult<teloxide::types::Message> {
    // Use /tmp directly — /data gets cleaned by init-data script (removes subdirs with binlogs)
    let tmp_dir = std::path::PathBuf::from(format!("/tmp/voice_{}", uuid::Uuid::new_v4()));
    tokio::fs::create_dir_all(&tmp_dir)
        .await
        .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

    let input_path = tmp_dir.join("input.mp3");
    let output_path = tmp_dir.join("output.ogg");

    log::info!("Voice: downloading file {} to {:?}", telegram_file_id, input_path);
    crate::telegram::download_file_from_telegram(bot, telegram_file_id, Some(input_path.clone()))
        .await
        .map_err(|e| {
            log::error!("Voice: download failed: {}", e);
            teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string())))
        })?;
    log::info!(
        "Voice: download complete, converting segment start={}s dur={}s",
        start_secs,
        duration_secs
    );

    // Extract segment + convert to OGG Opus mono
    let in_str = input_path.to_string_lossy().to_string();
    let out_str = output_path.to_string_lossy().to_string();
    let seg_start = start_secs;
    let seg_dur = duration_secs;
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<Option<u32>> {
        let mut cmd = std::process::Command::new("ffmpeg");
        cmd.arg("-i").arg(&in_str);
        if seg_start > 0 {
            cmd.arg("-ss").arg(seg_start.to_string());
        }
        if seg_dur > 0 {
            cmd.arg("-t").arg(seg_dur.to_string());
        }
        cmd.args([
            "-c:a",
            "libopus",
            "-b:a",
            "64k",
            "-ac",
            "1",
            "-application",
            "voip",
            "-y",
        ])
        .arg(&out_str);
        let output = cmd.output()?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow::anyhow!("ffmpeg failed: {}", stderr));
        }
        // Probe duration for waveform
        let probe = std::process::Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-show_entries",
                "format=duration",
                "-of",
                "default=noprint_wrappers=1:nokey=1",
            ])
            .arg(&out_str)
            .output()?;
        let dur = if probe.status.success() {
            String::from_utf8_lossy(&probe.stdout)
                .trim()
                .parse::<f64>()
                .ok()
                .map(|d| d as u32)
        } else {
            None
        };
        Ok(dur)
    })
    .await
    .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
    .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

    // Always set duration — required for waveform. Fall back to segment length.
    let dur = result.unwrap_or(duration_secs.max(1) as u32);
    log::info!("Voice: sending OGG file {:?} (duration={}s)", output_path, dur);
    let send_result = bot
        .send_voice(chat_id, InputFile::file(&output_path))
        .duration(dur)
        .await;
    // Cleanup temp files
    let _ = tokio::fs::remove_dir_all(tmp_dir).await;
    send_result
}

/// Build section picker keyboard for lyrics + audio re-send.
/// Callbacks: `downloads:lyrics_send:{download_id}:{session_id}:{idx_or_all}`
fn build_lyrics_audio_keyboard(
    download_id: i64,
    session_id: &str,
    sections: &[crate::lyrics::LyricsSection],
) -> InlineKeyboardMarkup {
    use std::collections::HashMap;

    let mut total: HashMap<String, usize> = HashMap::new();
    for s in sections {
        *total.entry(s.name.clone()).or_insert(0) += 1;
    }
    let mut seen: HashMap<String, usize> = HashMap::new();

    let buttons: Vec<teloxide::types::InlineKeyboardButton> = sections
        .iter()
        .enumerate()
        .map(|(idx, s)| {
            let occ = seen.entry(s.name.clone()).or_insert(0);
            *occ += 1;
            let label = if total.get(&s.name).copied().unwrap_or(1) > 1 {
                format!("{} ({})", s.name, occ)
            } else {
                s.name.clone()
            };
            crate::telegram::cb(
                label,
                format!("downloads:lyrics_send:{}:{}:{}", download_id, session_id, idx),
            )
        })
        .collect();

    let mut rows: Vec<Vec<teloxide::types::InlineKeyboardButton>> = buttons.chunks(3).map(|c| c.to_vec()).collect();

    rows.push(vec![crate::telegram::cb(
        "📄 All Lyrics".to_string(),
        format!("downloads:lyrics_send:{}:{}:all", download_id, session_id),
    )]);

    rows.push(vec![crate::telegram::cb(
        "❌ Cancel".to_string(),
        "downloads:cancel".to_string(),
    )]);

    InlineKeyboardMarkup::new(rows)
}
