use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardMarkup, InputFile, MessageId};

use crate::telegram::BotExt;

use crate::core::escape_markdown;

use super::cb_helpers::{build_lyrics_audio_keyboard, send_as_voice_segment};
use super::subtitles::{add_video_cut_button_from_history, fetch_subtitles_for_command};
use super::CallbackCtx;

pub(super) async fn handle(ctx: &CallbackCtx, action: &str, parts: &[&str]) -> ResponseResult<()> {
    match action {
        "subtitles" => {
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
                let loading_msg = ctx
                    .bot
                    .edit_md(ctx.chat_id, ctx.message_id, "⏳ Fetching subtitles (SRT \\+ TXT)…")
                    .await
                    .ok();

                let url = download.url.clone();
                let lang = ""; // default: server decides

                match fetch_subtitles_for_command(&ctx.downsub_gateway, &ctx.subtitle_cache, ctx.chat_id.0, &url, lang)
                    .await
                {
                    Ok((srt_content, txt_content, segment_count)) => {
                        if let Some(msg) = loading_msg {
                            ctx.bot
                                .edit_message_text(
                                    ctx.chat_id,
                                    msg.id,
                                    format!("✅ {} segments fetched", segment_count),
                                )
                                .await
                                .ok();
                        }
                        ctx.bot
                            .send_document(
                                ctx.chat_id,
                                InputFile::memory(srt_content.into_bytes()).file_name("subtitles.srt"),
                            )
                            .await
                            .ok();
                        ctx.bot
                            .send_document(
                                ctx.chat_id,
                                InputFile::memory(txt_content.into_bytes()).file_name("subtitles.txt"),
                            )
                            .await
                            .ok();
                    }
                    Err(e) => {
                        if let Some(msg) = loading_msg {
                            ctx.bot
                                .edit_message_text(ctx.chat_id, msg.id, format!("❌ Error: {}", e))
                                .await
                                .ok();
                        } else {
                            ctx.bot.send_message(ctx.chat_id, format!("❌ Error: {}", e)).await.ok();
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
            if let Some(download) = ctx
                .shared_storage
                .get_download_history_entry(ctx.chat_id.0, download_id)
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
                ctx.bot
                    .send_md_kb(
                        ctx.chat_id,
                        format!("🔤 Choose subtitle language for *{}*", escape_markdown(&download.title)),
                        keyboard,
                    )
                    .await?;
                ctx.bot.delete_message(ctx.chat_id, ctx.message_id).await.ok();
            }
        }
        // Process: download video from Telegram, burn subtitles, send back
        "burn_subs_lang" => {
            if parts.len() < 4 {
                return Ok(());
            }
            let lang_code = parts[2].to_string();
            let download_id = parts[3].parse::<i64>().unwrap_or(0);
            if let Some(download) = ctx
                .shared_storage
                .get_download_history_entry(ctx.chat_id.0, download_id)
                .await
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
            {
                if let Some(file_id) = download.file_id.clone() {
                    ctx.bot.delete_message(ctx.chat_id, ctx.message_id).await.ok();

                    let user_lang = crate::i18n::user_lang_from_storage(&ctx.shared_storage, ctx.chat_id.0).await;
                    let args = doracore::fluent_args!("lang" => lang_code.as_str());
                    let status_text = crate::i18n::t_args(&user_lang, "video_circle.burn_subs_status", &args);
                    let processing_msg = ctx.bot.send_message(ctx.chat_id, status_text).await?;

                    // Get message_id for MTProto fallback
                    let message_info = ctx
                        .shared_storage
                        .get_download_message_info(download_id)
                        .await
                        .ok()
                        .flatten();
                    let (fallback_message_id, fallback_chat_id) = message_info.unzip();

                    let bot = ctx.bot.clone();
                    let chat_id = ctx.chat_id;
                    let url = download.url.clone();
                    let title = download.title.clone();
                    let shared_storage = Arc::clone(&ctx.shared_storage);
                    let username = ctx.username.clone();

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
            if let Some(download) = ctx
                .shared_storage
                .get_download_history_entry(ctx.chat_id.0, download_id)
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
                ctx.bot
                    .send_md_kb(
                        ctx.chat_id,
                        format!("🎙 Select voice message duration for *{}*:", title),
                        keyboard,
                    )
                    .await?;
                ctx.bot.delete_message(ctx.chat_id, ctx.message_id).await.ok();
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

            if let Some(download) = ctx
                .shared_storage
                .get_download_history_entry(ctx.chat_id.0, download_id)
                .await
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
            {
                if let Some(ref fid) = download.file_id {
                    ctx.bot.delete_message(ctx.chat_id, ctx.message_id).await.ok();
                    let status_msg = ctx
                        .bot
                        .send_message(ctx.chat_id, "🎙 Converting to voice message…")
                        .await?;

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
                    let result = send_as_voice_segment(&ctx.bot, ctx.chat_id, fid, start_secs, seg_duration).await;

                    ctx.bot.delete_message(ctx.chat_id, status_msg.id).await.ok();
                    match result {
                        Ok(_) => {
                            log::info!("Voice message sent successfully for download {}", download_id)
                        }
                        Err(e) => {
                            log::error!("Voice conversion failed for download {}: {}", download_id, e);
                            ctx.bot
                                .send_message(ctx.chat_id, format!("❌ Voice conversion failed: {e}"))
                                .await
                                .ok();
                        }
                    }
                }
            }
        }
        // Fetch lyrics and show section picker -- selecting a section re-sends the audio with lyrics caption
        "lyrics" => {
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
                // Use author from metadata directly when available -- avoids misparse
                // from titles like "Music Audio - Nirvana- Rape Me (Audio)"
                let (artist, title) = if let Some(ref author) = download.author {
                    (author.as_str(), download.title.as_str())
                } else {
                    crate::lyrics::parse_artist_title(&download.title)
                };

                let status_msg = ctx.bot.send_message(ctx.chat_id, "🎵 Fetching lyrics…").await?;

                match crate::lyrics::fetch_lyrics(artist, title, None).await {
                    None => {
                        ctx.bot.delete_message(ctx.chat_id, status_msg.id).await.ok();
                        let display = format!("{} - {}", artist, title);
                        let escaped = escape_markdown(&display);
                        ctx.bot
                            .send_md(ctx.chat_id, format!("❌ Lyrics not found for *{}*", escaped))
                            .await?;
                    }
                    Some(lyr) => {
                        ctx.bot.delete_message(ctx.chat_id, status_msg.id).await.ok();

                        // Save lyrics session -- use short ID (Telegram callback data max 64 bytes)
                        let session_id = uuid::Uuid::new_v4().to_string()[..8].to_string();
                        let sections_json = serde_json::to_string(&lyr.sections).unwrap_or_default();
                        let _ = ctx
                            .shared_storage
                            .create_lyrics_session(
                                &session_id,
                                ctx.chat_id.0,
                                &lyr.artist,
                                &lyr.title,
                                &sections_json,
                                lyr.has_structure,
                            )
                            .await;

                        // Build section picker -- callbacks go to downloads:lyrics_send:{download_id}:{session_id}:{idx}
                        let display = format!("{} – {}", lyr.artist, lyr.title);
                        let keyboard = build_lyrics_audio_keyboard(download_id, &session_id, &lyr.sections);
                        let msg = if lyr.has_structure && lyr.sections.len() > 1 {
                            format!("🎵 {}\nChoose a section to send with audio:", display)
                        } else {
                            format!("🎵 {}\nSend audio with lyrics?", display)
                        };
                        ctx.bot.send_message(ctx.chat_id, msg).reply_markup(keyboard).await?;
                    }
                }
                ctx.bot.delete_message(ctx.chat_id, ctx.message_id).await.ok();
            }
        }
        // Edit audio message caption with selected lyrics (from with_lyrics toggle)
        "lyr_cap" => {
            // downloads:lyr_cap:{audio_msg_id}:{session_id}:{idx_or_all}
            if parts.len() < 5 {
                return Ok(());
            }
            let audio_msg_id = parts[2].parse::<i32>().unwrap_or(0);
            let session_id = parts[3];
            let idx_str = parts[4];

            let lyrics_session = ctx.shared_storage.get_lyrics_session(session_id).await.ok().flatten();

            if let Some((_artist, _title, sections_json, _has_struct)) = lyrics_session {
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

                if !lyrics_text.is_empty() {
                    let caption = if lyrics_text.chars().count() > 1024 {
                        let truncated: String = lyrics_text.chars().take(1020).collect();
                        format!("{truncated}…")
                    } else {
                        lyrics_text
                    };
                    let _ = ctx
                        .bot
                        .edit_message_caption(ctx.chat_id, MessageId(audio_msg_id))
                        .caption(caption)
                        .await;
                }
                // Delete the picker message
                ctx.bot.delete_message(ctx.chat_id, ctx.message_id).await.ok();
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

            let download = ctx
                .shared_storage
                .get_download_history_entry(ctx.chat_id.0, download_id)
                .await
                .ok()
                .flatten();
            let lyrics_session = ctx.shared_storage.get_lyrics_session(session_id).await.ok().flatten();

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
                        ctx.bot.send_message(ctx.chat_id, "❌ Section not found.").await?;
                    } else {
                        // Telegram caption limit is 1024 chars -- truncate on char boundary
                        let caption = if lyrics_text.chars().count() > 1024 {
                            let truncated: String = lyrics_text.chars().take(1020).collect();
                            format!("{truncated}…")
                        } else {
                            lyrics_text
                        };
                        ctx.bot
                            .send_audio(
                                ctx.chat_id,
                                teloxide::types::InputFile::file_id(teloxide::types::FileId(fid.clone())),
                            )
                            .caption(caption)
                            .await?;
                    }
                    ctx.bot.delete_message(ctx.chat_id, ctx.message_id).await.ok();
                }
            }
        }
        _ => {}
    }
    Ok(())
}
