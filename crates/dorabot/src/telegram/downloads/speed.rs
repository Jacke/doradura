use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardMarkup, ParseMode};

use crate::core::escape_markdown;

use super::subtitles::change_video_speed;
use super::CallbackCtx;

pub(super) async fn handle(ctx: &CallbackCtx, action: &str, parts: &[&str]) -> ResponseResult<()> {
    match action {
        "speed" => {
            if parts.len() < 3 {
                return Ok(());
            }
            let download_id = parts[2].parse::<i64>().unwrap_or(0);
            if let Some(download) = ctx
                .shared_storage
                .get_download_history_entry(ctx.chat_id.0, download_id)
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
                ctx.bot
                    .send_message(
                        ctx.chat_id,
                        format!("⚙️ Choose speed for *{}*", escape_markdown(&download.title)),
                    )
                    .parse_mode(ParseMode::MarkdownV2)
                    .reply_markup(keyboard)
                    .await?;

                ctx.bot.delete_message(ctx.chat_id, ctx.message_id).await.ok();
            }
        }
        "speed_cut" => {
            if parts.len() < 3 {
                return Ok(());
            }
            let cut_id = parts[2].parse::<i64>().unwrap_or(0);
            if let Some(cut) = ctx
                .shared_storage
                .get_cut_entry(ctx.chat_id.0, cut_id)
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
                ctx.bot
                    .send_message(
                        ctx.chat_id,
                        format!("⚙️ Choose speed for clip *{}*", escape_markdown(&cut.title)),
                    )
                    .parse_mode(ParseMode::MarkdownV2)
                    .reply_markup(keyboard)
                    .await?;

                ctx.bot.delete_message(ctx.chat_id, ctx.message_id).await.ok();
            }
        }
        "apply_speed" => {
            if parts.len() < 4 {
                return Ok(());
            }
            let speed_str = parts[2];
            let download_id = parts[3].parse::<i64>().unwrap_or(0);
            let speed: f32 = speed_str.parse().unwrap_or(1.0);
            if let Some(download) = ctx
                .shared_storage
                .get_download_history_entry(ctx.chat_id.0, download_id)
                .await
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
            {
                if let Some(file_id) = download.file_id {
                    ctx.bot.delete_message(ctx.chat_id, ctx.message_id).await.ok();
                    let processing_msg = ctx
                        .bot
                        .send_message(
                            ctx.chat_id,
                            format!(
                                "⚙️ Processing video at speed {}x\\.\\.\\.  \nThis may take a few minutes\\.",
                                speed_str.replace(".", "\\.")
                            ),
                        )
                        .parse_mode(ParseMode::MarkdownV2)
                        .await?;
                    match change_video_speed(&ctx.bot, ctx.chat_id, &file_id, speed, &download.title).await {
                        Ok((sent_message, file_size)) => {
                            ctx.bot.delete_message(ctx.chat_id, processing_msg.id).await.ok();

                            let new_title = format!("{} [speed {}x]", download.title, speed_str);
                            let new_duration = download.duration.map(|d| ((d as f32) / speed).round().max(1.0) as i64);
                            let new_file_id = sent_message
                                .video()
                                .map(|v| v.file.id.0.clone())
                                .or_else(|| sent_message.document().map(|d| d.file.id.0.clone()))
                                .or_else(|| sent_message.audio().map(|a| a.file.id.0.clone()));
                            if let Some(fid) = new_file_id {
                                if let Ok(db_id) = ctx
                                    .shared_storage
                                    .save_download_history(
                                        ctx.chat_id.0,
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
                                        Some(speed),
                                    )
                                    .await
                                {
                                    // Save message_id for MTProto file_reference refresh
                                    let _ = ctx
                                        .shared_storage
                                        .update_download_message_id(db_id, sent_message.id.0, ctx.chat_id.0)
                                        .await;
                                }
                            }
                        }
                        Err(e) => {
                            ctx.bot.delete_message(ctx.chat_id, processing_msg.id).await.ok();
                            ctx.bot
                                .send_message(
                                    ctx.chat_id,
                                    "❌ Failed to process video. The administrator has been notified.",
                                )
                                .await
                                .ok();
                            // Notify admin about the error with full details
                            crate::telegram::notifications::notify_admin_video_error(
                                &ctx.bot,
                                ctx.chat_id.0,
                                ctx.username.as_deref(),
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
            if let Some(cut) = ctx
                .shared_storage
                .get_cut_entry(ctx.chat_id.0, cut_id)
                .await
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
            {
                if let Some(file_id) = cut.file_id {
                    ctx.bot.delete_message(ctx.chat_id, ctx.message_id).await.ok();
                    let processing_msg = ctx
                        .bot
                        .send_message(
                            ctx.chat_id,
                            format!(
                                "⚙️ Processing clip at speed {}x\\.\\.\\.  \nThis may take a few minutes\\.",
                                speed_str.replace(".", "\\.")
                            ),
                        )
                        .parse_mode(ParseMode::MarkdownV2)
                        .await?;
                    match change_video_speed(&ctx.bot, ctx.chat_id, &file_id, speed, &cut.title).await {
                        Ok((sent_message, file_size)) => {
                            ctx.bot.delete_message(ctx.chat_id, processing_msg.id).await.ok();

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
                                if let Ok(db_id) = ctx
                                    .shared_storage
                                    .save_download_history(
                                        ctx.chat_id.0,
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
                                        Some(speed),
                                    )
                                    .await
                                {
                                    // Save message_id for MTProto file_reference refresh
                                    let _ = ctx
                                        .shared_storage
                                        .update_download_message_id(db_id, sent_message.id.0, ctx.chat_id.0)
                                        .await;
                                }
                            }
                        }
                        Err(e) => {
                            ctx.bot.delete_message(ctx.chat_id, processing_msg.id).await.ok();
                            ctx.bot
                                .send_message(
                                    ctx.chat_id,
                                    "❌ Failed to process video. The administrator has been notified.",
                                )
                                .await
                                .ok();
                            // Notify admin about the error with full details
                            crate::telegram::notifications::notify_admin_video_error(
                                &ctx.bot,
                                ctx.chat_id.0,
                                ctx.username.as_deref(),
                                &e.to_string(),
                                &format!("apply_speed_cut: {}x on '{}'", speed_str, cut.title),
                            )
                            .await;
                        }
                    }
                }
            }
        }
        _ => {}
    }
    Ok(())
}
