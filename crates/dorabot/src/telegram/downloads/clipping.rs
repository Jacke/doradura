use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup, ParseMode};

use crate::storage::db::{self, OutputKind, SourceKind};
use crate::telegram::commands::{process_video_clip, CutSegment};

use super::{build_duration_buttons, build_timestamp_ui, format_timestamp, CallbackCtx};

/// Convert an anyhow error into a `teloxide::RequestError` for use with `?`.
fn to_req_err(e: impl std::fmt::Display) -> teloxide::RequestError {
    teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string())))
}

/// Start a clip/circle/gif session from a download history entry.
///
/// Shared logic for the `clip`, `circle`, and `gif` callback handlers:
/// fetch the download, validate it is MP4 with a `file_id`, create a
/// `VideoClipSession`, fetch timestamps, build the keyboard, and send the
/// prompt message.
async fn start_session_from_download(
    ctx: &CallbackCtx,
    download_id: i64,
    output_kind: OutputKind,
    mp4_required_msg: &str,
    prompt_text: &str,
    ts_kind: &str,
    show_duration_buttons: bool,
    show_subtitle_button: bool,
) -> ResponseResult<()> {
    let Some(download) = ctx
        .shared_storage
        .get_download_history_entry(ctx.chat_id.0, download_id)
        .await
        .map_err(to_req_err)?
    else {
        return Ok(());
    };

    if download.format != "mp4" {
        ctx.bot
            .send_message(ctx.chat_id, mp4_required_msg)
            .parse_mode(ParseMode::MarkdownV2)
            .await
            .ok();
        return Ok(());
    }
    if download.file_id.is_none() {
        ctx.bot
            .send_message(ctx.chat_id, "❌ Could not find file\\_id for this file\\.")
            .parse_mode(ParseMode::MarkdownV2)
            .await
            .ok();
        return Ok(());
    }

    let session = db::VideoClipSession {
        id: uuid::Uuid::new_v4().to_string(),
        user_id: ctx.chat_id.0,
        source_download_id: download_id,
        source_kind: SourceKind::Download,
        source_id: download_id,
        original_url: download.url.clone(),
        output_kind,
        created_at: chrono::Utc::now(),
        expires_at: chrono::Utc::now() + chrono::Duration::minutes(10),
        subtitle_lang: None,
    };
    ctx.shared_storage
        .clone()
        .upsert_video_clip_session(&session)
        .await
        .map_err(to_req_err)?;

    // Fetch timestamps and build UI
    let timestamps = ctx
        .shared_storage
        .get_video_timestamps(download_id)
        .await
        .unwrap_or_default();
    let (ts_buttons, ts_text) = build_timestamp_ui(&timestamps, ts_kind, download_id);

    // Build keyboard rows
    let mut keyboard_rows: Vec<Vec<InlineKeyboardButton>> = Vec::new();

    if show_duration_buttons || show_subtitle_button {
        let lang = crate::i18n::user_lang_from_storage(&ctx.shared_storage, ctx.chat_id.0).await;

        if show_duration_buttons {
            keyboard_rows = build_duration_buttons(download_id, &lang);
        }
        keyboard_rows.extend(ts_buttons);

        if show_subtitle_button {
            let subs_label = crate::i18n::t(&lang, "video_circle.subtitles_button");
            keyboard_rows.push(vec![crate::telegram::cb(
                subs_label,
                format!("downloads:circle_subs:{}", download_id),
            )]);
        }

        keyboard_rows.push(vec![crate::telegram::cb(
            crate::i18n::t(&lang, "common.cancel"),
            "downloads:clip_cancel".to_string(),
        )]);
    } else {
        keyboard_rows.extend(ts_buttons);
        keyboard_rows.push(vec![crate::telegram::cb(
            "❌ Cancel".to_string(),
            "downloads:clip_cancel".to_string(),
        )]);
    }

    let keyboard = InlineKeyboardMarkup::new(keyboard_rows);
    let message = format!("{}{}", prompt_text, ts_text);
    ctx.bot
        .send_message(ctx.chat_id, message)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(keyboard)
        .await?;

    ctx.bot.delete_message(ctx.chat_id, ctx.message_id).await.ok();
    Ok(())
}

/// Start a clip/circle/gif session from an existing cut entry.
///
/// Shared logic for `clip_cut`, `circle_cut`, and `gif_cut` callbacks:
/// fetch the cut, validate `file_id`, create a `VideoClipSession`, and send
/// the prompt message with a cancel button.
async fn start_session_from_cut(
    ctx: &CallbackCtx,
    cut_id: i64,
    output_kind: OutputKind,
    prompt_text: &str,
) -> ResponseResult<()> {
    let Some(cut) = ctx
        .shared_storage
        .get_cut_entry(ctx.chat_id.0, cut_id)
        .await
        .map_err(to_req_err)?
    else {
        return Ok(());
    };

    if cut.file_id.is_none() {
        ctx.bot
            .send_message(ctx.chat_id, "❌ Could not find file\\_id for this file\\.")
            .parse_mode(ParseMode::MarkdownV2)
            .await
            .ok();
        return Ok(());
    }

    let session = db::VideoClipSession {
        id: uuid::Uuid::new_v4().to_string(),
        user_id: ctx.chat_id.0,
        source_download_id: 0,
        source_kind: SourceKind::Cut,
        source_id: cut_id,
        original_url: cut.original_url.clone(),
        output_kind,
        created_at: chrono::Utc::now(),
        expires_at: chrono::Utc::now() + chrono::Duration::minutes(10),
        subtitle_lang: None,
    };
    ctx.shared_storage
        .clone()
        .upsert_video_clip_session(&session)
        .await
        .map_err(to_req_err)?;

    let keyboard = InlineKeyboardMarkup::new(vec![vec![crate::telegram::cb(
        "❌ Cancel".to_string(),
        "downloads:clip_cancel".to_string(),
    )]]);
    ctx.bot
        .send_message(ctx.chat_id, prompt_text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(keyboard)
        .await?;

    ctx.bot.delete_message(ctx.chat_id, ctx.message_id).await.ok();
    Ok(())
}

pub(super) async fn handle(ctx: &CallbackCtx, action: &str, parts: &[&str]) -> ResponseResult<()> {
    match action {
        "clip" => {
            if parts.len() < 3 {
                return Ok(());
            }
            let download_id = parts[2].parse::<i64>().unwrap_or(0);
            start_session_from_download(
                ctx,
                download_id,
                OutputKind::Cut,
                "✂️ Clipping is only available for MP4\\.",
                "✂️ Send the intervals to clip in the format `mm:ss-mm:ss` or `hh:mm:ss-hh:mm:ss`\\.\nMultiple ranges separated by commas\\.\n\nExample: `00:10-00:25, 01:00-01:10`",
                "clip",
                false,
                false,
            )
            .await?;
        }
        "clip_cut" => {
            if parts.len() < 3 {
                return Ok(());
            }
            let cut_id = parts[2].parse::<i64>().unwrap_or(0);
            start_session_from_cut(
                ctx,
                cut_id,
                OutputKind::Cut,
                "✂️ Send the intervals to clip in the format `mm:ss-mm:ss` or `hh:mm:ss-hh:mm:ss`\\.\nMultiple ranges separated by commas\\.\n\nExample: `00:10-00:25, 01:00-01:10`",
            )
            .await?;
        }
        "circle" => {
            if parts.len() < 3 {
                return Ok(());
            }
            let download_id = parts[2].parse::<i64>().unwrap_or(0);
            // Circle uses i18n for the prompt text; resolve it before calling the helper.
            let lang = crate::i18n::user_lang_from_storage(&ctx.shared_storage, ctx.chat_id.0).await;
            let prompt = crate::i18n::t(&lang, "video_circle.select_part");
            start_session_from_download(
                ctx,
                download_id,
                OutputKind::VideoNote,
                "⭕️ Circle is only available for MP4\\.",
                &prompt,
                "circle",
                true,
                true,
            )
            .await?;
        }
        "circle_cut" => {
            if parts.len() < 3 {
                return Ok(());
            }
            let cut_id = parts[2].parse::<i64>().unwrap_or(0);
            start_session_from_cut(
                ctx,
                cut_id,
                OutputKind::VideoNote,
                "⭕️ Send the intervals for the circle in the format `mm:ss-mm:ss` or `hh:mm:ss-hh:mm:ss`\\.\nMultiple ranges separated by commas\\.\n\nExample: `00:10-00:25` or `first30 2x`",
            )
            .await?;
        }
        "gif" => {
            if parts.len() < 3 {
                return Ok(());
            }
            let download_id = parts[2].parse::<i64>().unwrap_or(0);
            start_session_from_download(
                ctx,
                download_id,
                OutputKind::Gif,
                "🎞 GIF is only available for MP4\\.",
                "🎞 Send the time range for the GIF in the format `mm:ss-mm:ss`\\.\nMax 30 seconds\\.\n\nExample: `00:10-00:25`",
                "gif",
                false,
                false,
            )
            .await?;
        }
        "gif_cut" => {
            if parts.len() < 3 {
                return Ok(());
            }
            let cut_id = parts[2].parse::<i64>().unwrap_or(0);
            start_session_from_cut(
                ctx,
                cut_id,
                OutputKind::Gif,
                "🎞 Send the time range for the GIF in the format `mm:ss-mm:ss`\\.\nMax 30 seconds\\.\n\nExample: `00:10-00:25`",
            )
            .await?;
        }
        "clip_cancel" => {
            ctx.shared_storage
                .clone()
                .delete_video_clip_session_by_user(ctx.chat_id.0)
                .await
                .ok();
            ctx.bot.delete_message(ctx.chat_id, ctx.message_id).await.ok();
        }
        // Show subtitle language picker for circle creation
        // Callback format: downloads:circle_subs:{download_id}
        "circle_subs" => {
            if parts.len() < 3 {
                return Ok(());
            }
            let download_id = parts[2].parse::<i64>().unwrap_or(0);
            let lang = crate::i18n::user_lang_from_storage(&ctx.shared_storage, ctx.chat_id.0).await;

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
            ctx.bot
                .edit_message_text(ctx.chat_id, ctx.message_id, text)
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
            if let Some(mut session) = ctx
                .shared_storage
                .clone()
                .get_active_video_clip_session(ctx.chat_id.0)
                .await
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
            {
                session.subtitle_lang = if sub_lang == "none" {
                    None
                } else {
                    Some(sub_lang.to_string())
                };
                ctx.shared_storage
                    .clone()
                    .upsert_video_clip_session(&session)
                    .await
                    .map_err(|e| {
                        teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string())))
                    })?;

                // Rebuild circle menu with updated subtitle state
                let lang = crate::i18n::user_lang_from_storage(&ctx.shared_storage, ctx.chat_id.0).await;
                let timestamps = ctx
                    .shared_storage
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
                ctx.bot
                    .edit_message_text(ctx.chat_id, ctx.message_id, message)
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

            if let Some(download) = ctx
                .shared_storage
                .get_download_history_entry(ctx.chat_id.0, download_id)
                .await
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
            {
                // Delete the prompt message
                ctx.bot.delete_message(ctx.chat_id, ctx.message_id).await.ok();

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
                    user_id: ctx.chat_id.0,
                    source_download_id: download_id,
                    source_kind: SourceKind::Download,
                    source_id: download_id,
                    original_url: download.url.clone(),
                    output_kind: match output_kind {
                        "circle" => OutputKind::VideoNote,
                        "gif" => OutputKind::Gif,
                        _ => OutputKind::Cut,
                    },
                    created_at: chrono::Utc::now(),
                    expires_at: chrono::Utc::now() + chrono::Duration::minutes(10),
                    subtitle_lang: None,
                };

                // Delete any existing session first
                ctx.shared_storage
                    .clone()
                    .delete_video_clip_session_by_user(ctx.chat_id.0)
                    .await
                    .ok();

                // Create segment
                let segment = CutSegment {
                    start_secs: time_seconds,
                    end_secs: end_seconds,
                };
                let segments_text = format!("{}-{}", format_timestamp(time_seconds), format_timestamp(end_seconds));

                // Process the clip
                let bot_clone = ctx.bot.clone();
                let db_pool_clone = ctx.db_pool.clone();
                let shared_storage = ctx.shared_storage.clone();
                let chat_id = ctx.chat_id;
                tokio::spawn(async move {
                    if let Err(e) = process_video_clip(
                        bot_clone,
                        db_pool_clone,
                        shared_storage,
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

            if let Some(download) = ctx
                .shared_storage
                .get_download_history_entry(ctx.chat_id.0, download_id)
                .await
                .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
            {
                // Delete the prompt message
                ctx.bot.delete_message(ctx.chat_id, ctx.message_id).await.ok();

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
                    user_id: ctx.chat_id.0,
                    source_download_id: download_id,
                    source_kind: SourceKind::Download,
                    source_id: download_id,
                    original_url: download.url.clone(),
                    output_kind: OutputKind::VideoNote,
                    created_at: chrono::Utc::now(),
                    expires_at: chrono::Utc::now() + chrono::Duration::minutes(10),
                    subtitle_lang: None,
                };

                // Delete any existing session first
                ctx.shared_storage
                    .clone()
                    .delete_video_clip_session_by_user(ctx.chat_id.0)
                    .await
                    .ok();

                // Create segment
                let segment = CutSegment { start_secs, end_secs };
                let segments_text = format!("{}-{}", format_timestamp(start_secs), format_timestamp(end_secs));

                // Process the clip
                let bot_clone = ctx.bot.clone();
                let db_pool_clone = ctx.db_pool.clone();
                let shared_storage = ctx.shared_storage.clone();
                let chat_id = ctx.chat_id;
                tokio::spawn(async move {
                    if let Err(e) = process_video_clip(
                        bot_clone,
                        db_pool_clone,
                        shared_storage,
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
        _ => {}
    }
    Ok(())
}
