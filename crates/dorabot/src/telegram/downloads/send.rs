use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardMarkup, ParseMode};

use crate::core::{escape_markdown, escape_markdown_url};

use super::is_youtube_url;
use super::subtitles::{add_audio_tools_buttons_from_history, add_video_cut_button_from_history, send_document_forced};
use super::CallbackCtx;

pub(super) async fn handle(ctx: &CallbackCtx, action: &str, parts: &[&str]) -> ResponseResult<()> {
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

            ctx.bot.delete_message(ctx.chat_id, ctx.message_id).await?;
            super::show_downloads_page(
                &ctx.bot,
                ctx.chat_id,
                ctx.db_pool.clone(),
                ctx.shared_storage.clone(),
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

            ctx.bot.delete_message(ctx.chat_id, ctx.message_id).await?;
            super::show_downloads_page(
                &ctx.bot,
                ctx.chat_id,
                ctx.db_pool.clone(),
                ctx.shared_storage.clone(),
                0,
                filter,
                search,
                None,
            )
            .await?;
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
            ctx.bot.delete_message(ctx.chat_id, ctx.message_id).await?;
            super::show_downloads_page(
                &ctx.bot,
                ctx.chat_id,
                ctx.db_pool.clone(),
                ctx.shared_storage.clone(),
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

            if let Some(download) = ctx
                .shared_storage
                .get_download_history_entry(ctx.chat_id.0, download_id)
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
                        &ctx.bot,
                        ctx.chat_id,
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

            if let Some(cut) = ctx
                .shared_storage
                .get_cut_entry(ctx.chat_id.0, cut_id)
                .await
                .map_err(|e| {
                    log::error!("📥 Failed to get cut entry: {}", e);
                    teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string())))
                })?
            {
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
                        &ctx.bot,
                        ctx.chat_id,
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

            if let Some(download) = ctx
                .shared_storage
                .get_download_history_entry(ctx.chat_id.0, download_id)
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
                    let status_msg = ctx.bot.send_message(ctx.chat_id, status_text).await?;

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
                            ctx.bot
                                .send_audio(
                                    ctx.chat_id,
                                    teloxide::types::InputFile::file_id(teloxide::types::FileId(
                                        telegram_file_id.clone(),
                                    )),
                                )
                                .caption(caption.clone())
                                .await
                        }
                        "video" => {
                            ctx.bot
                                .send_video(
                                    ctx.chat_id,
                                    teloxide::types::InputFile::file_id(teloxide::types::FileId(
                                        telegram_file_id.clone(),
                                    )),
                                )
                                .caption(caption.clone())
                                .await
                        }
                        "document" => {
                            send_document_forced(
                                &ctx.bot,
                                ctx.chat_id,
                                &telegram_file_id,
                                upload_file_name,
                                caption.clone(),
                            )
                            .await
                        }
                        _ => {
                            ctx.bot.delete_message(ctx.chat_id, status_msg.id).await.ok();
                            return Ok(());
                        }
                    };

                    match send_result {
                        Ok(sent_message) => {
                            ctx.bot.delete_message(ctx.chat_id, status_msg.id).await.ok();
                            if send_type == "audio" && download.format == "mp3" {
                                let duration = sent_message
                                    .audio()
                                    .map(|a| a.duration.seconds())
                                    .or_else(|| download.duration.map(|d| d.max(0) as u32))
                                    .unwrap_or(0);
                                if let Err(e) = add_audio_tools_buttons_from_history(
                                    &ctx.bot,
                                    Arc::clone(&ctx.db_pool),
                                    ctx.shared_storage.clone(),
                                    ctx.chat_id,
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
                                if let Err(e) = add_video_cut_button_from_history(
                                    &ctx.bot,
                                    ctx.chat_id,
                                    sent_message.id,
                                    download_id,
                                )
                                .await
                                {
                                    log::warn!("Failed to add video cut button: {}", e);
                                }
                            }
                            ctx.bot.delete_message(ctx.chat_id, ctx.message_id).await.ok();
                        }
                        Err(e) => {
                            ctx.bot.delete_message(ctx.chat_id, status_msg.id).await.ok();
                            ctx.bot
                                .send_message(ctx.chat_id, format!("❌ Failed to send file: {e}"))
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

            if let Some(cut) = ctx
                .shared_storage
                .get_cut_entry(ctx.chat_id.0, cut_id)
                .await
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
            {
                if let Some(fid) = cut.file_id {
                    let status_text = match send_type {
                        "video" => "⏳ Preparing to send as video…",
                        "document" => "⏳ Preparing to send as document…",
                        _ => "⏳ Preparing to send…",
                    };
                    let status_msg = ctx.bot.send_message(ctx.chat_id, status_text).await?;

                    let telegram_file_id = fid;
                    let upload_file_name = "doradura_edit.mp4";
                    let caption = cut.title;

                    let send_result = match send_type {
                        "video" => {
                            ctx.bot
                                .send_video(
                                    ctx.chat_id,
                                    teloxide::types::InputFile::file_id(teloxide::types::FileId(
                                        telegram_file_id.clone(),
                                    )),
                                )
                                .caption(caption.clone())
                                .await
                        }
                        "document" => {
                            send_document_forced(
                                &ctx.bot,
                                ctx.chat_id,
                                &telegram_file_id,
                                upload_file_name,
                                caption.clone(),
                            )
                            .await
                        }
                        _ => {
                            ctx.bot.delete_message(ctx.chat_id, status_msg.id).await.ok();
                            ctx.bot.send_message(ctx.chat_id, "❌ Unknown send mode.").await.ok();
                            return Ok(());
                        }
                    };

                    match send_result {
                        Ok(_) => {
                            ctx.bot.delete_message(ctx.chat_id, status_msg.id).await.ok();
                            ctx.bot.delete_message(ctx.chat_id, ctx.message_id).await.ok();
                        }
                        Err(e) => {
                            ctx.bot.delete_message(ctx.chat_id, status_msg.id).await.ok();
                            ctx.bot
                                .send_message(ctx.chat_id, format!("❌ Failed to send file: {e}"))
                                .await
                                .ok();
                        }
                    }
                }
            }
        }
        _ => {}
    }
    Ok(())
}
