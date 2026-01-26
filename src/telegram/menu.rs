use crate::core::config::admin::ADMIN_USERNAME;
use crate::core::escape_markdown;
use crate::core::export::handle_export;
use crate::core::history::handle_history_callback;
use crate::core::rate_limiter::RateLimiter;
use crate::core::subscription::{create_subscription_invoice, show_subscription_info};
use crate::download::queue::{DownloadQueue, DownloadTask};
use crate::i18n;
use crate::storage::cache;
use crate::storage::db::{self, DbPool};
use crate::telegram::admin;
use crate::telegram::cache as tg_cache;
use crate::telegram::setup_chat_bot_commands;
use crate::telegram::Bot;
use fluent_templates::fluent_bundle::FluentArgs;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{CallbackQueryId, InlineKeyboardButton, InlineKeyboardMarkup, MessageId, ParseMode};
use teloxide::RequestError;
use unic_langid::LanguageIdentifier;
use url::Url;
use uuid::Uuid;

/// Edit caption if present, else fallback to editing text.
async fn edit_caption_or_text(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    text: String,
    keyboard: Option<InlineKeyboardMarkup>,
) -> ResponseResult<()> {
    let mut caption_req = bot
        .edit_message_caption(chat_id, message_id)
        .caption(text.clone())
        .parse_mode(teloxide::types::ParseMode::MarkdownV2);

    if let Some(ref kb) = keyboard {
        caption_req = caption_req.reply_markup(kb.clone());
    }

    match caption_req.await {
        Ok(_) => Ok(()),
        Err(_) => {
            let mut text_req = bot
                .edit_message_text(chat_id, message_id, text)
                .parse_mode(teloxide::types::ParseMode::MarkdownV2);
            if let Some(kb) = keyboard {
                text_req = text_req.reply_markup(kb);
            }
            text_req.await?;
            Ok(())
        }
    }
}

async fn start_download_from_preview(
    bot: &Bot,
    callback_id: &CallbackQueryId,
    chat_id: ChatId,
    message_id: MessageId,
    preview_msg_id: Option<MessageId>,
    url_id: &str,
    format: &str,
    selected_quality: Option<String>,
    db_pool: Arc<DbPool>,
    download_queue: Arc<DownloadQueue>,
    rate_limiter: Arc<RateLimiter>,
) -> ResponseResult<()> {
    let url_str = match cache::get_url(&db_pool, url_id).await {
        Some(url_str) => url_str,
        None => {
            log::warn!("URL not found in cache for ID: {} (expired or invalid)", url_id);
            bot.answer_callback_query(callback_id.clone())
                .text("Ссылка устарела, отправь её снова")
                .await?;
            return Ok(());
        }
    };

    let url = match Url::parse(&url_str) {
        Ok(url) => url,
        Err(e) => {
            log::error!("Failed to parse URL from cache: {}", e);
            bot.answer_callback_query(callback_id.clone())
                .text("Ошибка: неверная ссылка")
                .await?;
            return Ok(());
        }
    };

    let original_message_id = tg_cache::get_link_message_id(&url_str).await;
    let conn = db::get_connection(&db_pool)
        .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;
    let plan = match db::get_user(&conn, chat_id.0) {
        Ok(Some(ref user)) => user.plan.clone(),
        _ => "free".to_string(),
    };

    // Rate limit disabled
    let _ = (rate_limiter, &plan);

    let _ = bot
        .answer_callback_query(callback_id.clone())
        .text("⏳ Обрабатываю...")
        .await;

    if let Err(e) = bot.delete_message(chat_id, message_id).await {
        log::warn!("Failed to delete preview message: {:?}", e);
    }
    if let Some(prev_msg_id) = preview_msg_id {
        if prev_msg_id != message_id {
            if let Err(e) = bot.delete_message(chat_id, prev_msg_id).await {
                log::warn!("Failed to delete preview message: {:?}", e);
            }
        }
    }

    if format == "mp4+mp3" {
        let video_quality = if let Some(quality) = selected_quality {
            Some(quality)
        } else {
            Some(db::get_user_video_quality(&conn, chat_id.0).unwrap_or_else(|_| "best".to_string()))
        };
        let task_mp4 = DownloadTask::from_plan(
            url.as_str().to_string(),
            chat_id,
            original_message_id,
            true,
            "mp4".to_string(),
            video_quality,
            None,
            &plan,
        );
        download_queue.add_task(task_mp4, Some(Arc::clone(&db_pool))).await;

        let audio_bitrate = Some(db::get_user_audio_bitrate(&conn, chat_id.0).unwrap_or_else(|_| "320k".to_string()));
        let task_mp3 = DownloadTask::from_plan(
            url.as_str().to_string(),
            chat_id,
            original_message_id,
            false,
            "mp3".to_string(),
            None,
            audio_bitrate,
            &plan,
        );
        download_queue.add_task(task_mp3, Some(Arc::clone(&db_pool))).await;
    } else {
        let video_quality = if format == "mp4" {
            if let Some(quality) = selected_quality {
                Some(quality)
            } else {
                Some(db::get_user_video_quality(&conn, chat_id.0).unwrap_or_else(|_| "best".to_string()))
            }
        } else {
            None
        };
        let audio_bitrate = if format == "mp3" {
            Some(db::get_user_audio_bitrate(&conn, chat_id.0).unwrap_or_else(|_| "320k".to_string()))
        } else {
            None
        };

        let is_video = format == "mp4";
        let task = DownloadTask::from_plan(
            url.as_str().to_string(),
            chat_id,
            original_message_id,
            is_video,
            format.to_string(),
            video_quality,
            audio_bitrate,
            &plan,
        );
        download_queue.add_task(task, Some(Arc::clone(&db_pool))).await;
    }

    Ok(())
}

/// Shows the main settings menu for the download mode.
///
/// Displays inline buttons for video quality, audio bitrate, and supported services.
///
/// # Arguments
///
/// * `bot` - Telegram bot instance
/// * `chat_id` - User chat ID
/// * `db_pool` - Database connection pool
///
/// # Returns
///
/// Returns `ResponseResult<Message>` with the sent message or an error.
///
/// # Errors
///
/// Returns an error if the database connection or sending the message fails.
pub async fn show_main_menu(bot: &Bot, chat_id: ChatId, db_pool: Arc<DbPool>) -> ResponseResult<Message> {
    let conn = db::get_connection(&db_pool)
        .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;
    let video_quality = db::get_user_video_quality(&conn, chat_id.0).unwrap_or_else(|_| "best".to_string());
    let audio_bitrate = db::get_user_audio_bitrate(&conn, chat_id.0).unwrap_or_else(|_| "320k".to_string());
    let lang = i18n::user_lang_from_pool(&db_pool, chat_id.0);

    let quality_emoji = match video_quality.as_str() {
        "1080p" => "🎬 1080p",
        "720p" => "🎬 720p",
        "480p" => "🎬 480p",
        "360p" => "🎬 360p",
        _ => "🎬 Best",
    };

    let bitrate_display = match audio_bitrate.as_str() {
        "128k" => "128 kbps",
        "192k" => "192 kbps",
        "256k" => "256 kbps",
        "320k" => "320 kbps",
        _ => "320 kbps",
    };

    let mut quality_args = FluentArgs::new();
    quality_args.set("quality", quality_emoji);
    let mut bitrate_args = FluentArgs::new();
    bitrate_args.set("bitrate", bitrate_display);

    let keyboard = InlineKeyboardMarkup::new(vec![
        vec![InlineKeyboardButton::callback(
            i18n::t_args(&lang, "menu.video_quality_button", &quality_args),
            "mode:video_quality",
        )],
        vec![InlineKeyboardButton::callback(
            i18n::t_args(&lang, "menu.audio_bitrate_button", &bitrate_args),
            "mode:audio_bitrate",
        )],
        vec![InlineKeyboardButton::callback(
            i18n::t(&lang, "menu.services_button"),
            "mode:services",
        )],
        vec![InlineKeyboardButton::callback(
            i18n::t(&lang, "menu.subscription_button"),
            "mode:subscription",
        )],
        vec![InlineKeyboardButton::callback(
            i18n::t(&lang, "menu.language_button"),
            "mode:language",
        )],
    ]);

    bot.send_message(chat_id, i18n::t(&lang, "menu.title"))
        .parse_mode(teloxide::types::ParseMode::MarkdownV2)
        .reply_markup(keyboard)
        .await
}

/// Shows the download type menu.
///
/// Displays available formats (MP3, MP4, SRT, TXT) and marks the current choice.
///
/// # Arguments
///
/// * `bot` - Telegram bot instance
/// * `chat_id` - User chat ID
/// * `message_id` - ID of the message to edit
/// * `db_pool` - Database connection pool
/// * `url_id` - Optional preview URL ID when opened from preview
/// * `preview_msg_id` - Optional preview message ID to delete when changing the format
///
/// # Returns
///
/// Returns `ResponseResult<()>` or an error while editing the message.
pub async fn show_download_type_menu(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    db_pool: Arc<DbPool>,
    url_id: Option<&str>,
    preview_msg_id: Option<MessageId>,
) -> ResponseResult<()> {
    let conn = db::get_connection(&db_pool)
        .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;
    let current_format = db::get_user_download_format(&conn, chat_id.0).unwrap_or_else(|_| "mp3".to_string());
    let lang = i18n::user_lang_from_pool(&db_pool, chat_id.0);

    // Build callback data with url_id and preview_msg_id when they are present
    let format_callback = |format: &str| {
        if let Some(id) = url_id {
            if let Some(preview_id) = preview_msg_id {
                format!("format:{}:preview:{}:{}", format, id, preview_id.0)
            } else {
                format!("format:{}:preview:{}", format, id)
            }
        } else {
            format!("format:{}", format)
        }
    };

    let back_callback = if let Some(id) = url_id {
        if let Some(preview_id) = preview_msg_id {
            format!("back:preview:{}:{}", id, preview_id.0)
        } else {
            format!("back:preview:{}", id)
        }
    } else {
        "back:main".to_string()
    };

    let keyboard = InlineKeyboardMarkup::new(vec![
        vec![
            InlineKeyboardButton::callback(
                if current_format == "mp3" {
                    "🎵 MP3 ✓"
                } else {
                    "🎵 MP3"
                }
                .to_string(),
                format_callback("mp3"),
            ),
            InlineKeyboardButton::callback(
                if current_format == "mp4" {
                    "🎬 MP4 ✓"
                } else {
                    "🎬 MP4"
                }
                .to_string(),
                format_callback("mp4"),
            ),
        ],
        vec![InlineKeyboardButton::callback(
            if current_format == "mp4+mp3" {
                "🎬🎵 MP4 + MP3 ✓"
            } else {
                "🎬🎵 MP4 + MP3"
            }
            .to_string(),
            format_callback("mp4+mp3"),
        )],
        vec![
            InlineKeyboardButton::callback(
                if current_format == "srt" {
                    "📝 SRT ✓"
                } else {
                    "📝 SRT"
                }
                .to_string(),
                format_callback("srt"),
            ),
            InlineKeyboardButton::callback(
                if current_format == "txt" {
                    "📄 TXT ✓"
                } else {
                    "📄 TXT"
                }
                .to_string(),
                format_callback("txt"),
            ),
        ],
        vec![InlineKeyboardButton::callback(
            i18n::t(&lang, "common.back"),
            back_callback,
        )],
    ]);

    let format_display = match current_format.as_str() {
        "mp3" => "🎵 MP3",
        "mp4" => "🎬 MP4",
        "mp4+mp3" => "🎬🎵 MP4 + MP3",
        "srt" => "📝 SRT",
        "txt" => "📄 TXT",
        _ => "🎵 MP3",
    };

    let escaped_format = escape_markdown(format_display);
    let mut args = FluentArgs::new();
    args.set("format", escaped_format.clone());
    edit_caption_or_text(
        bot,
        chat_id,
        message_id,
        i18n::t_args(&lang, "menu.download_type_title", &args),
        Some(keyboard),
    )
    .await?;
    Ok(())
}

/// Sends the download type menu as a new text message.
///
/// Used when we need to send a menu instead of editing an existing message
/// (for example, when the original message contains media and cannot be edited).
///
/// # Arguments
///
/// * `bot` - Telegram bot instance
/// * `chat_id` - User chat ID
/// * `db_pool` - Database connection pool
/// * `url_id` - Optional preview URL ID when opened from preview
///
/// # Returns
///
/// Returns `ResponseResult<()>` or an error when sending the message.
pub async fn send_download_type_menu_as_new(
    bot: &Bot,
    chat_id: ChatId,
    db_pool: Arc<DbPool>,
    url_id: Option<&str>,
    preview_msg_id: Option<MessageId>,
) -> ResponseResult<()> {
    let conn = db::get_connection(&db_pool)
        .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;
    let current_format = db::get_user_download_format(&conn, chat_id.0).unwrap_or_else(|_| "mp3".to_string());
    let lang = i18n::user_lang_from_pool(&db_pool, chat_id.0);

    // Build callback data with url_id and preview_msg_id when they are present
    let format_callback = |format: &str| {
        if let Some(id) = url_id {
            if let Some(preview_id) = preview_msg_id {
                format!("format:{}:preview:{}:{}", format, id, preview_id.0)
            } else {
                format!("format:{}:preview:{}", format, id)
            }
        } else {
            format!("format:{}", format)
        }
    };

    let back_callback = if let Some(id) = url_id {
        if let Some(preview_id) = preview_msg_id {
            format!("back:preview:{}:{}", id, preview_id.0)
        } else {
            format!("back:preview:{}", id)
        }
    } else {
        "back:main".to_string()
    };

    let keyboard = InlineKeyboardMarkup::new(vec![
        vec![
            InlineKeyboardButton::callback(
                if current_format == "mp3" {
                    "🎵 MP3 ✓"
                } else {
                    "🎵 MP3"
                }
                .to_string(),
                format_callback("mp3"),
            ),
            InlineKeyboardButton::callback(
                if current_format == "mp4" {
                    "🎬 MP4 ✓"
                } else {
                    "🎬 MP4"
                }
                .to_string(),
                format_callback("mp4"),
            ),
        ],
        vec![InlineKeyboardButton::callback(
            if current_format == "mp4+mp3" {
                "🎬🎵 MP4 + MP3 ✓"
            } else {
                "🎬🎵 MP4 + MP3"
            }
            .to_string(),
            format_callback("mp4+mp3"),
        )],
        vec![
            InlineKeyboardButton::callback(
                if current_format == "srt" {
                    "📝 SRT ✓"
                } else {
                    "📝 SRT"
                }
                .to_string(),
                format_callback("srt"),
            ),
            InlineKeyboardButton::callback(
                if current_format == "txt" {
                    "📄 TXT ✓"
                } else {
                    "📄 TXT"
                }
                .to_string(),
                format_callback("txt"),
            ),
        ],
        vec![InlineKeyboardButton::callback(
            i18n::t(&lang, "common.back"),
            back_callback,
        )],
    ]);

    let format_display = match current_format.as_str() {
        "mp3" => "🎵 MP3",
        "mp4" => "🎬 MP4",
        "mp4+mp3" => "🎬🎵 MP4 + MP3",
        "srt" => "📝 SRT",
        "txt" => "📄 TXT",
        _ => "🎵 MP3",
    };
    let escaped_format = escape_markdown(format_display);
    let mut args = FluentArgs::new();
    args.set("format", escaped_format.clone());
    bot.send_message(chat_id, i18n::t_args(&lang, "menu.download_type_title", &args))
        .parse_mode(teloxide::types::ParseMode::MarkdownV2)
        .reply_markup(keyboard)
        .await?;
    Ok(())
}

/// Shows the video quality selection menu.
///
/// Displays available qualities (1080p, 720p, 480p, 360p, best) and marks the current choice.
///
/// # Arguments
///
/// * `bot` - Telegram bot instance
/// * `chat_id` - User chat ID
/// * `message_id` - ID of the message to edit
/// * `db_pool` - Database connection pool
/// * `url_id` - Optional preview URL ID when opened from preview
///
/// # Returns
///
/// Returns `ResponseResult<()>` or an error while editing the message.
pub async fn show_video_quality_menu(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    db_pool: Arc<DbPool>,
    url_id: Option<&str>,
) -> ResponseResult<()> {
    let conn = db::get_connection(&db_pool)
        .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;
    let current_quality = db::get_user_video_quality(&conn, chat_id.0).unwrap_or_else(|_| "best".to_string());
    let send_as_document = db::get_user_send_as_document(&conn, chat_id.0).unwrap_or(0);
    let download_subs = db::get_user_download_subtitles(&conn, chat_id.0).unwrap_or(false);
    let burn_subs = db::get_user_burn_subtitles(&conn, chat_id.0).unwrap_or(false);
    let lang = i18n::user_lang_from_pool(&db_pool, chat_id.0);

    let mut keyboard_rows = vec![
        vec![
            InlineKeyboardButton::callback(
                if current_quality == "1080p" {
                    "🎬 1080p (Full HD) ✓"
                } else {
                    "🎬 1080p (Full HD)"
                }
                .to_string(),
                "quality:1080p",
            ),
            InlineKeyboardButton::callback(
                if current_quality == "720p" {
                    "🎬 720p (HD) ✓"
                } else {
                    "🎬 720p (HD)"
                }
                .to_string(),
                "quality:720p",
            ),
        ],
        vec![
            InlineKeyboardButton::callback(
                if current_quality == "480p" {
                    "🎬 480p (SD) ✓"
                } else {
                    "🎬 480p (SD)"
                }
                .to_string(),
                "quality:480p",
            ),
            InlineKeyboardButton::callback(
                if current_quality == "360p" {
                    "🎬 360p (Low) ✓"
                } else {
                    "🎬 360p (Low)"
                }
                .to_string(),
                "quality:360p",
            ),
        ],
        vec![InlineKeyboardButton::callback(
            if current_quality == "best" {
                "🎬 Best (Авто) ✓"
            } else {
                "🎬 Best (Авто)"
            }
            .to_string(),
            "quality:best",
        )],
        vec![InlineKeyboardButton::callback(
            if send_as_document == 0 {
                i18n::t(&lang, "menu.send_video_media")
            } else {
                i18n::t(&lang, "menu.send_video_document")
            },
            "send_type:toggle",
        )],
    ];

    // Добавляем кнопку для burn_subtitles только если download_subtitles включен
    if download_subs {
        let mut burn_args = FluentArgs::new();
        let status = if burn_subs {
            i18n::t(&lang, "menu.burn_subtitles_on")
        } else {
            i18n::t(&lang, "menu.burn_subtitles_off")
        };
        burn_args.set("status", status);

        keyboard_rows.push(vec![InlineKeyboardButton::callback(
            i18n::t_args(&lang, "menu.burn_subtitles_button", &burn_args),
            "video:toggle_burn_subs",
        )]);
    }

    keyboard_rows.push(vec![InlineKeyboardButton::callback(
        i18n::t(&lang, "common.back"),
        url_id.map_or_else(|| "back:main".to_string(), |id| format!("back:main:preview:{}", id)),
    )]);

    let keyboard = InlineKeyboardMarkup::new(keyboard_rows);

    let quality_display = match current_quality.as_str() {
        "1080p" => "🎬 1080p (Full HD)",
        "720p" => "🎬 720p (HD)",
        "480p" => "🎬 480p (SD)",
        "360p" => "🎬 360p (Low)",
        _ => "🎬 Best (Auto)",
    };

    let send_type_display = if send_as_document == 0 {
        i18n::t(&lang, "menu.send_type_media")
    } else {
        i18n::t(&lang, "menu.send_type_document")
    };

    let escaped_quality = escape_markdown(quality_display);
    let escaped_send_type = escape_markdown(&send_type_display);
    let mut args = FluentArgs::new();
    args.set("quality", escaped_quality.clone());
    args.set("send_type", escaped_send_type.clone());
    edit_caption_or_text(
        bot,
        chat_id,
        message_id,
        i18n::t_args(&lang, "menu.video_quality_title", &args),
        Some(keyboard),
    )
    .await?;
    Ok(())
}

/// Shows the audio bitrate selection menu.
///
/// Displays available bitrates (128kbps, 192kbps, 256kbps, 320kbps) and marks the current choice.
///
/// # Arguments
///
/// * `bot` - Telegram bot instance
/// * `chat_id` - User chat ID
/// * `message_id` - ID of the message to edit
/// * `db_pool` - Database connection pool
/// * `url_id` - Optional preview URL ID when opened from preview
///
/// # Returns
///
/// Returns `ResponseResult<()>` or an error while editing the message.
pub async fn show_audio_bitrate_menu(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    db_pool: Arc<DbPool>,
    url_id: Option<&str>,
) -> ResponseResult<()> {
    let conn = db::get_connection(&db_pool)
        .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;
    let current_bitrate = db::get_user_audio_bitrate(&conn, chat_id.0).unwrap_or_else(|_| "320k".to_string());
    let send_audio_as_document = db::get_user_send_audio_as_document(&conn, chat_id.0).unwrap_or(0);
    let lang = i18n::user_lang_from_pool(&db_pool, chat_id.0);

    let keyboard = InlineKeyboardMarkup::new(vec![
        vec![
            InlineKeyboardButton::callback(
                if current_bitrate == "128k" {
                    "🎵 128 kbps ✓"
                } else {
                    "🎵 128 kbps"
                }
                .to_string(),
                "bitrate:128k",
            ),
            InlineKeyboardButton::callback(
                if current_bitrate == "192k" {
                    "🎵 192 kbps ✓"
                } else {
                    "🎵 192 kbps"
                }
                .to_string(),
                "bitrate:192k",
            ),
        ],
        vec![
            InlineKeyboardButton::callback(
                if current_bitrate == "256k" {
                    "🎵 256 kbps ✓"
                } else {
                    "🎵 256 kbps"
                }
                .to_string(),
                "bitrate:256k",
            ),
            InlineKeyboardButton::callback(
                if current_bitrate == "320k" {
                    "🎵 320 kbps ✓"
                } else {
                    "🎵 320 kbps"
                }
                .to_string(),
                "bitrate:320k",
            ),
        ],
        vec![InlineKeyboardButton::callback(
            if send_audio_as_document == 0 {
                i18n::t(&lang, "menu.send_audio_media")
            } else {
                i18n::t(&lang, "menu.send_audio_document")
            },
            "audio_send_type:toggle",
        )],
        vec![InlineKeyboardButton::callback(
            i18n::t(&lang, "common.back"),
            url_id.map_or_else(|| "back:main".to_string(), |id| format!("back:main:preview:{}", id)),
        )],
    ]);

    let send_type_display = if send_audio_as_document == 0 {
        i18n::t(&lang, "menu.send_type_media")
    } else {
        i18n::t(&lang, "menu.send_type_document")
    };

    let escaped_bitrate = escape_markdown(&current_bitrate);
    let escaped_send_type = escape_markdown(&send_type_display);
    let mut args = FluentArgs::new();
    args.set("bitrate", escaped_bitrate.clone());
    args.set("send_type", escaped_send_type.clone());

    edit_caption_or_text(
        bot,
        chat_id,
        message_id,
        i18n::t_args(&lang, "menu.audio_bitrate_title", &args),
        Some(keyboard),
    )
    .await?;
    Ok(())
}

/// Shows information about supported services.
///
/// Displays the list of available services (YouTube, SoundCloud) and supported formats.
///
/// # Arguments
///
/// * `bot` - Telegram bot instance
/// * `chat_id` - User chat ID
/// * `message_id` - ID of the message to edit
///
/// # Returns
///
/// Returns `ResponseResult<()>` or an error while editing the message.
pub async fn show_services_menu(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    lang: &LanguageIdentifier,
) -> ResponseResult<()> {
    let keyboard = InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::callback(
        i18n::t(lang, "common.back"),
        "back:enhanced_main",
    )]]);

    edit_caption_or_text(
        bot,
        chat_id,
        message_id,
        i18n::t(lang, "menu.services_text"),
        Some(keyboard),
    )
    .await?;
    Ok(())
}

/// Shows the language selection menu.
pub async fn show_language_menu(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    db_pool: Arc<DbPool>,
    url_id: Option<&str>,
) -> ResponseResult<()> {
    let conn = db::get_connection(&db_pool)
        .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;
    let current_lang_code = db::get_user_language(&conn, chat_id.0).unwrap_or_else(|_| "ru".to_string());
    let lang = i18n::lang_from_code(&current_lang_code);

    let mut buttons = Vec::new();
    for (code, name) in i18n::SUPPORTED_LANGS.iter() {
        let flag = match *code {
            "en" => "🇺🇸",
            "ru" => "🇷🇺",
            "fr" => "🇫🇷",
            "de" => "🇩🇪",
            _ => "🏳️",
        };
        let label = if current_lang_code.eq_ignore_ascii_case(code) {
            format!("{} {} ✓", flag, name)
        } else {
            format!("{} {}", flag, name)
        };
        let callback = if let Some(id) = url_id {
            format!("language:set:{}:{}", code, id)
        } else {
            format!("language:set:{}", code)
        };
        buttons.push(vec![InlineKeyboardButton::callback(label, callback)]);
    }

    buttons.push(vec![InlineKeyboardButton::callback(
        i18n::t(&lang, "common.back"),
        url_id
            .map(|id| format!("back:preview:{}", id))
            .unwrap_or_else(|| "back:main".to_string()),
    )]);

    let keyboard = InlineKeyboardMarkup::new(buttons);
    bot.edit_message_text(chat_id, message_id, i18n::t(&lang, "menu.language_prompt"))
        .reply_markup(keyboard)
        .parse_mode(teloxide::types::ParseMode::MarkdownV2)
        .await?;
    Ok(())
}

/// Shows language selection menu for new users during onboarding.
///
/// Sends a new message with language selection buttons without a back button.
/// This is used during the /start flow for users who haven't selected a language yet.
///
/// # Arguments
///
/// * `bot` - Telegram bot instance
/// * `chat_id` - User chat ID
///
/// # Returns
///
/// Returns `ResponseResult<Message>` with the sent message or an error.
pub async fn show_language_selection_menu(bot: &Bot, chat_id: ChatId) -> ResponseResult<Message> {
    // Use default language (ru) for the welcome message since user hasn't selected yet
    let lang = i18n::lang_from_code("ru");

    let mut buttons = Vec::new();
    for (code, name) in i18n::SUPPORTED_LANGS.iter() {
        let flag = match *code {
            "en" => "🇺🇸",
            "ru" => "🇷🇺",
            "fr" => "🇫🇷",
            "de" => "🇩🇪",
            _ => "🏳️",
        };
        let label = format!("{} {}", flag, name);
        // Use special callback for new user language selection
        let callback = format!("language:select_new:{}", code);
        buttons.push(vec![InlineKeyboardButton::callback(label, callback)]);
    }

    let keyboard = InlineKeyboardMarkup::new(buttons);
    bot.send_message(chat_id, i18n::t(&lang, "menu.welcome_new_user"))
        .reply_markup(keyboard)
        .parse_mode(teloxide::types::ParseMode::MarkdownV2)
        .await
}

fn build_enhanced_menu(
    lang: &LanguageIdentifier,
    format_emoji: &str,
    quality_line: &str,
    plan_display: &str,
) -> (String, InlineKeyboardMarkup) {
    let mut args = FluentArgs::new();
    args.set("format", format_emoji);
    args.set("quality", quality_line);
    args.set("plan", plan_display);

    let text = i18n::t_args(lang, "menu.enhanced_text", &args);

    let keyboard = InlineKeyboardMarkup::new(vec![
        vec![
            InlineKeyboardButton::callback(i18n::t(lang, "menu.button_settings"), "main:settings"),
            InlineKeyboardButton::callback(i18n::t(lang, "menu.button_current"), "main:current"),
        ],
        vec![
            InlineKeyboardButton::callback(i18n::t(lang, "menu.button_stats"), "main:stats"),
            InlineKeyboardButton::callback(i18n::t(lang, "menu.button_history"), "main:history"),
        ],
        vec![
            InlineKeyboardButton::callback(i18n::t(lang, "menu.services_button"), "main:services"),
            InlineKeyboardButton::callback(i18n::t(lang, "menu.button_subscription"), "main:subscription"),
        ],
        vec![
            InlineKeyboardButton::callback(i18n::t(lang, "menu.language_button"), "mode:language"),
            InlineKeyboardButton::callback(i18n::t(lang, "menu.button_feedback"), "main:feedback"),
        ],
    ]);

    (text, keyboard)
}

// Edit message to show main menu (for callbacks that need to edit existing message)
// Args: bot - telegram bot instance, chat_id - user's chat ID, message_id - ID of message to edit, db_pool - database connection pool
// Functionality: Edits existing message to show main mode menu
// url_id - Optional preview URL ID (when the menu is opened from preview)
// preview_msg_id - Optional preview message ID to delete when changing the format
async fn edit_main_menu(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    db_pool: Arc<DbPool>,
    url_id: Option<&str>,
    _preview_msg_id: Option<MessageId>,
) -> ResponseResult<()> {
    let conn = db::get_connection(&db_pool)
        .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;
    let video_quality = db::get_user_video_quality(&conn, chat_id.0).unwrap_or_else(|_| "best".to_string());
    let audio_bitrate = db::get_user_audio_bitrate(&conn, chat_id.0).unwrap_or_else(|_| "320k".to_string());
    let lang = i18n::user_lang_from_pool(&db_pool, chat_id.0);

    let quality_emoji = match video_quality.as_str() {
        "1080p" => "🎬 1080p",
        "720p" => "🎬 720p",
        "480p" => "🎬 480p",
        "360p" => "🎬 360p",
        _ => "🎬 Best",
    };

    let bitrate_display = match audio_bitrate.as_str() {
        "128k" => "128 kbps",
        "192k" => "192 kbps",
        "256k" => "256 kbps",
        "320k" => "320 kbps",
        _ => "320 kbps",
    };

    // Build callback data with url_id when it is provided
    let mode_callback = |mode: &str| {
        if let Some(id) = url_id {
            format!("mode:{}:preview:{}", mode, id)
        } else {
            format!("mode:{}", mode)
        }
    };

    let mut quality_args = FluentArgs::new();
    quality_args.set("quality", quality_emoji);
    let mut bitrate_args = FluentArgs::new();
    bitrate_args.set("bitrate", bitrate_display);

    let mut keyboard_rows = vec![
        vec![InlineKeyboardButton::callback(
            i18n::t_args(&lang, "menu.video_quality_button", &quality_args),
            mode_callback("video_quality"),
        )],
        vec![InlineKeyboardButton::callback(
            i18n::t_args(&lang, "menu.audio_bitrate_button", &bitrate_args),
            mode_callback("audio_bitrate"),
        )],
        vec![InlineKeyboardButton::callback(
            i18n::t(&lang, "menu.services_button"),
            mode_callback("services"),
        )],
        vec![InlineKeyboardButton::callback(
            i18n::t(&lang, "menu.subscription_button"),
            mode_callback("subscription"),
        )],
        vec![InlineKeyboardButton::callback(
            i18n::t(&lang, "menu.language_button"),
            mode_callback("language"),
        )],
    ];

    // Add a Back button when the menu is opened from preview
    if let Some(id) = url_id {
        keyboard_rows.push(vec![InlineKeyboardButton::callback(
            i18n::t(&lang, "menu.back_to_preview"),
            format!("back:preview:{}", id),
        )]);
    }

    let keyboard = InlineKeyboardMarkup::new(keyboard_rows);

    edit_caption_or_text(bot, chat_id, message_id, i18n::t(&lang, "menu.title"), Some(keyboard)).await?;
    Ok(())
}

/// Sends the main settings menu as a new text message.
///
/// Used when we need to send a menu instead of editing an existing message
/// (for example, when the original message contains media and cannot be edited).
///
/// # Arguments
///
/// * `bot` - Telegram bot instance
/// * `chat_id` - User chat ID
/// * `db_pool` - Database connection pool
/// * `url_id` - Optional preview URL ID when opened from preview
/// * `preview_msg_id` - Optional preview message ID to delete when changing the format
///
/// # Returns
///
/// Returns `ResponseResult<()>` or an error when sending the message.
pub async fn send_main_menu_as_new(
    bot: &Bot,
    chat_id: ChatId,
    db_pool: Arc<DbPool>,
    url_id: Option<&str>,
    preview_msg_id: Option<MessageId>,
) -> ResponseResult<()> {
    let conn = db::get_connection(&db_pool)
        .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;
    let video_quality = db::get_user_video_quality(&conn, chat_id.0).unwrap_or_else(|_| "best".to_string());
    let audio_bitrate = db::get_user_audio_bitrate(&conn, chat_id.0).unwrap_or_else(|_| "320k".to_string());
    let lang = i18n::user_lang_from_pool(&db_pool, chat_id.0);

    let quality_emoji = match video_quality.as_str() {
        "1080p" => "🎬 1080p",
        "720p" => "🎬 720p",
        "480p" => "🎬 480p",
        "360p" => "🎬 360p",
        _ => "🎬 Best",
    };

    let bitrate_display = match audio_bitrate.as_str() {
        "128k" => "128 kbps",
        "192k" => "192 kbps",
        "256k" => "256 kbps",
        "320k" => "320 kbps",
        _ => "320 kbps",
    };

    // Build callback data with url_id and preview_msg_id when they are present
    let mode_callback = |mode: &str| {
        if let Some(id) = url_id {
            if let Some(preview_id) = preview_msg_id {
                format!("mode:{}:preview:{}:{}", mode, id, preview_id.0)
            } else {
                format!("mode:{}:preview:{}", mode, id)
            }
        } else {
            format!("mode:{}", mode)
        }
    };

    let mut quality_args = FluentArgs::new();
    quality_args.set("quality", quality_emoji);
    let mut bitrate_args = FluentArgs::new();
    bitrate_args.set("bitrate", bitrate_display);

    let mut keyboard_rows = vec![
        vec![InlineKeyboardButton::callback(
            i18n::t_args(&lang, "menu.video_quality_button", &quality_args),
            mode_callback("video_quality"),
        )],
        vec![InlineKeyboardButton::callback(
            i18n::t_args(&lang, "menu.audio_bitrate_button", &bitrate_args),
            mode_callback("audio_bitrate"),
        )],
        vec![InlineKeyboardButton::callback(
            i18n::t(&lang, "menu.services_button"),
            mode_callback("services"),
        )],
        vec![InlineKeyboardButton::callback(
            i18n::t(&lang, "menu.subscription_button"),
            mode_callback("subscription"),
        )],
        vec![InlineKeyboardButton::callback(
            i18n::t(&lang, "menu.language_button"),
            mode_callback("language"),
        )],
    ];

    // Add a Back button when the menu is opened from preview
    if let Some(id) = url_id {
        let back_callback = if let Some(preview_id) = preview_msg_id {
            format!("back:preview:{}:{}", id, preview_id.0)
        } else {
            format!("back:preview:{}", id)
        };
        keyboard_rows.push(vec![InlineKeyboardButton::callback(
            i18n::t(&lang, "menu.back_to_preview"),
            back_callback,
        )]);
    }

    let keyboard = InlineKeyboardMarkup::new(keyboard_rows);

    bot.send_message(chat_id, i18n::t(&lang, "menu.title"))
        .parse_mode(teloxide::types::ParseMode::MarkdownV2)
        .reply_markup(keyboard)
        .await?;
    Ok(())
}

/// Handles callback queries from the menu inline keyboards.
///
/// Processes button presses, updates user settings, or switches between menus.
///
/// # Arguments
///
/// * `bot` - Telegram bot instance
/// * `q` - Callback query to process
/// * `db_pool` - Database connection pool
/// * `download_queue` - Download queue
/// * `rate_limiter` - Rate limiter
///
/// # Returns
///
/// Returns `ResponseResult<()>` or an error while processing the callback.
///
/// # Supported Callbacks
///
/// - `mode:download_type` - Go to the format selection menu
/// - `mode:services` - Show information about supported services
/// - `back:main` - Return to the main menu
/// - `format:mp3|mp4|srt|txt` - Set the download format
/// - `dl:format:url_id` - Start a download with the specified format (url_id is the short cache ID)
/// - `pv:set:url_id` - Show settings for the preview message
/// - `pv:cancel:url_id` - Cancel the preview
pub async fn handle_menu_callback(
    bot: Bot,
    q: CallbackQuery,
    db_pool: Arc<DbPool>,
    download_queue: Arc<DownloadQueue>,
    rate_limiter: Arc<RateLimiter>,
) -> ResponseResult<()> {
    let callback_id = q.id.clone();
    let data_clone = q.data.clone();
    let message_clone = q.message.clone();

    if let Some(data) = q.data {
        let chat_id = q.message.as_ref().map(|m| m.chat().id);
        let message_id = q.message.as_ref().map(|m| m.id());

        if let (Some(chat_id), Some(message_id)) = (chat_id, message_id) {
            let lang = i18n::user_lang_from_pool(&db_pool, chat_id.0);
            // Handle audio cut/effects callbacks first
            if data.starts_with("ac:") {
                // Reconstruct CallbackQuery for audio cut handler
                let ac_query = CallbackQuery {
                    id: callback_id.clone(),
                    from: q.from.clone(),
                    message: message_clone,
                    inline_message_id: q.inline_message_id.clone(),
                    chat_instance: q.chat_instance.clone(),
                    data: data_clone,
                    game_short_name: q.game_short_name.clone(),
                };
                if let Err(e) = handle_audio_cut_callback(bot.clone(), ac_query, Arc::clone(&db_pool)).await {
                    log::error!("Audio cut callback error: {}", e);
                }
                return Ok(());
            }
            if data.starts_with("ae:") {
                // Reconstruct CallbackQuery for audio effects handler
                let ae_query = CallbackQuery {
                    id: callback_id.clone(),
                    from: q.from.clone(),
                    message: message_clone,
                    inline_message_id: q.inline_message_id.clone(),
                    chat_instance: q.chat_instance.clone(),
                    data: data_clone,
                    game_short_name: q.game_short_name.clone(),
                };
                if let Err(e) = handle_audio_effects_callback(bot.clone(), ae_query, Arc::clone(&db_pool)).await {
                    log::error!("Audio effects callback error: {}", e);
                }
                return Ok(());
            }

            if data.starts_with("mode:") {
                let _ = bot.answer_callback_query(callback_id.clone()).await;
                // Format: mode:action or mode:action:preview:url_id or mode:action:preview:url_id:preview_msg_id
                let parts: Vec<&str> = data.split(':').collect();
                let action = parts.get(1).unwrap_or(&"");
                let is_from_preview = parts.len() >= 4 && parts[2] == "preview";
                let url_id = if is_from_preview { Some(parts[3]) } else { None };
                let preview_msg_id = if is_from_preview && parts.len() >= 5 {
                    parts[4].parse::<i32>().ok().map(teloxide::types::MessageId)
                } else {
                    None
                };

                match *action {
                    "download_type" => {
                        show_download_type_menu(
                            &bot,
                            chat_id,
                            message_id,
                            Arc::clone(&db_pool),
                            url_id,
                            preview_msg_id,
                        )
                        .await?;
                    }
                    "video_quality" => {
                        show_video_quality_menu(&bot, chat_id, message_id, Arc::clone(&db_pool), url_id).await?;
                    }
                    "audio_bitrate" => {
                        show_audio_bitrate_menu(&bot, chat_id, message_id, Arc::clone(&db_pool), url_id).await?;
                    }
                    "services" => {
                        show_services_menu(&bot, chat_id, message_id, &lang).await?;
                    }
                    "language" => {
                        show_language_menu(&bot, chat_id, message_id, Arc::clone(&db_pool), url_id).await?;
                    }
                    "subscription" => {
                        // Delete the old message and show subscription info
                        let _ = bot.delete_message(chat_id, message_id).await;
                        let _ = show_subscription_info(&bot, chat_id, Arc::clone(&db_pool)).await;
                    }
                    _ => {}
                }
            } else if data.starts_with("main:") {
                let _ = bot.answer_callback_query(callback_id.clone()).await;
                let action = data.strip_prefix("main:").unwrap_or("");

                match action {
                    "settings" => {
                        // Show the old main menu (current /mode functionality)
                        edit_main_menu(&bot, chat_id, message_id, Arc::clone(&db_pool), None, None).await?;
                    }
                    "current" => {
                        // Show detailed current settings
                        show_current_settings_detail(&bot, chat_id, message_id, Arc::clone(&db_pool)).await?;
                    }
                    "stats" => {
                        // Delete current message and show stats
                        let _ = bot.delete_message(chat_id, message_id).await;
                        let _ = crate::core::stats::show_user_stats(&bot, chat_id, Arc::clone(&db_pool)).await;
                    }
                    "history" => {
                        // Delete current message and show history
                        let _ = bot.delete_message(chat_id, message_id).await;
                        let _ = crate::core::history::show_history(&bot, chat_id, Arc::clone(&db_pool)).await;
                    }
                    "services" => {
                        // Edit message to show services
                        show_services_menu(&bot, chat_id, message_id, &lang).await?;
                    }
                    "subscription" => {
                        // Delete current message and show subscription info
                        let _ = bot.delete_message(chat_id, message_id).await;
                        let _ = crate::core::subscription::show_subscription_info(&bot, chat_id, Arc::clone(&db_pool))
                            .await;
                    }
                    "help" => {
                        // Edit message to show help
                        show_help_menu(&bot, chat_id, message_id).await?;
                    }
                    "feedback" => {
                        // Delete current message and send feedback prompt
                        let _ = bot.delete_message(chat_id, message_id).await;
                        let _ = crate::telegram::feedback::send_feedback_prompt(&bot, chat_id, &lang).await;
                    }
                    _ => {}
                }
            } else if let Some(plan) = data.strip_prefix("subscribe:") {
                log::info!("🔔 Subscribe callback received: data={}, chat_id={}", data, chat_id.0);
                bot.answer_callback_query(callback_id.clone()).await?;
                // Remove "subscribe:" prefix
                log::info!("📌 Extracted plan: {}", plan);
                match plan {
                    "premium" | "vip" => {
                        log::info!("✅ Valid plan '{}', creating invoice for chat_id={}", plan, chat_id.0);
                        // Create an invoice for payment through Telegram Stars
                        match create_subscription_invoice(&bot, chat_id, plan).await {
                            Ok(msg) => {
                                log::info!(
                                    "✅ Invoice created successfully for user {} plan {}. Message ID: {}",
                                    chat_id.0,
                                    plan,
                                    msg.id.0
                                );
                            }
                            Err(e) => {
                                log::error!(
                                    "❌ Failed to create invoice for user {} plan {}: {:?}",
                                    chat_id.0,
                                    plan,
                                    e
                                );
                                log::error!("❌ Error type: {}", e);
                                let _ = bot.send_message(
                                    chat_id,
                                    "❌ Произошла ошибка при создании инвойса. Попробуй позже или обратись к администратору."
                                ).await;
                            }
                        }
                    }
                    _ => {
                        log::warn!("⚠️ Unknown plan requested: {}", plan);
                        bot.answer_callback_query(callback_id).text("Неизвестный план").await?;
                    }
                }
            } else if let Some(action) = data.strip_prefix("subscription:") {
                bot.answer_callback_query(callback_id.clone()).await?;
                // Remove "subscription:" prefix
                match action {
                    "cancel" => {
                        // Cancel the user's subscription
                        match crate::core::subscription::cancel_subscription(&bot, chat_id.0, Arc::clone(&db_pool))
                            .await
                        {
                            Ok(_) => {
                                log::info!("Subscription canceled for user {}", chat_id.0);
                                let _ = bot
                                    .send_message(
                                        chat_id,
                                        "✅ Подписка успешно отменена\\. Она будет действовать до конца оплаченного периода\\.",
                                    )
                                    .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                                    .await;

                                // Refresh the subscription menu
                                let _ = bot.delete_message(chat_id, message_id).await;
                                let _ = show_subscription_info(&bot, chat_id, Arc::clone(&db_pool)).await;
                            }
                            Err(e) => {
                                log::error!("Failed to cancel subscription: {}", e);

                                // Check if subscription is already non-recurring
                                let message = if e.contains("already non-recurring") {
                                    "ℹ️ У тебя разовая подписка без автопродления\\. Она будет действовать до конца оплаченного периода\\."
                                } else {
                                    "❌ Не удалось отменить подписку\\. Попробуй позже или обратись к администратору\\."
                                };

                                let _ = bot
                                    .send_message(chat_id, message)
                                    .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                                    .await;
                            }
                        }
                    }
                    _ => {
                        bot.answer_callback_query(callback_id)
                            .text("Неизвестное действие")
                            .await?;
                    }
                }
            } else if let Some(lang_code) = data.strip_prefix("language:select_new:") {
                // Handle language selection for new users (during onboarding)
                if i18n::SUPPORTED_LANGS
                    .iter()
                    .any(|(code, _)| code.eq_ignore_ascii_case(lang_code))
                {
                    if let Ok(conn) = db::get_connection(&db_pool) {
                        let username = q.from.username.clone();
                        // Create user with selected language
                        if let Err(e) = db::create_user_with_language(&conn, chat_id.0, username.clone(), lang_code) {
                            log::warn!("Failed to create user with language: {}", e);
                        } else {
                            log::info!(
                                "New user created with language: chat_id={}, language={}",
                                chat_id.0,
                                lang_code
                            );
                            // Notify admins about new user
                            use crate::telegram::notifications::notify_admin_new_user;
                            let bot_notify = bot.clone();
                            let user_id = chat_id.0;
                            let first_name = q.from.first_name.clone();
                            let lang = lang_code.to_string();
                            tokio::spawn(async move {
                                notify_admin_new_user(
                                    &bot_notify,
                                    user_id,
                                    username.as_deref(),
                                    Some(&first_name),
                                    Some(&lang),
                                    Some("/start → язык"),
                                )
                                .await;
                            });
                        }
                    }

                    let new_lang = i18n::lang_from_code(lang_code);
                    if let Err(e) = setup_chat_bot_commands(&bot, chat_id, &new_lang).await {
                        log::warn!("Failed to set chat-specific commands for lang {}: {}", lang_code, e);
                    }
                    let _ = bot
                        .answer_callback_query(callback_id.clone())
                        .text(i18n::t(&new_lang, "menu.language_saved"))
                        .await;

                    // Delete language selection message and show main menu
                    let _ = bot.delete_message(chat_id, message_id).await;
                    let _ = show_enhanced_main_menu(&bot, chat_id, Arc::clone(&db_pool)).await;

                    // Send random voice message in background
                    let bot_voice = bot.clone();
                    let chat_id_voice = chat_id;
                    tokio::spawn(async move {
                        crate::telegram::voice::send_random_voice_message(bot_voice, chat_id_voice).await;
                    });
                } else {
                    let fallback_lang = i18n::lang_from_code("ru");
                    bot.answer_callback_query(callback_id)
                        .text(i18n::t(&fallback_lang, "menu.language_invalid"))
                        .await?;
                }
            } else if let Some(lang_data) = data.strip_prefix("language:set:") {
                let mut parts = lang_data.split(':');
                let lang_code = parts.next().unwrap_or("ru");
                let preview_url_id = parts.next();

                if i18n::SUPPORTED_LANGS
                    .iter()
                    .any(|(code, _)| code.eq_ignore_ascii_case(lang_code))
                {
                    if let Ok(conn) = db::get_connection(&db_pool) {
                        if let Ok(None) = db::get_user(&conn, chat_id.0) {
                            log::info!(
                                "Creating user before setting language: chat_id={}, username={:?}",
                                chat_id.0,
                                q.from.username
                            );
                            let username = q.from.username.clone();
                            if let Err(e) = db::create_user(&conn, chat_id.0, username.clone()) {
                                log::warn!("Failed to create user before setting language: {}", e);
                            } else {
                                // Notify admins about new user
                                use crate::telegram::notifications::notify_admin_new_user;
                                let bot_notify = bot.clone();
                                let user_id = chat_id.0;
                                let first_name = q.from.first_name.clone();
                                let lang = lang_code.to_string();
                                tokio::spawn(async move {
                                    notify_admin_new_user(
                                        &bot_notify,
                                        user_id,
                                        username.as_deref(),
                                        Some(&first_name),
                                        Some(&lang),
                                        Some("смена языка"),
                                    )
                                    .await;
                                });
                            }
                        }
                        let _ = db::set_user_language(&conn, chat_id.0, lang_code);
                    }

                    let new_lang = i18n::lang_from_code(lang_code);
                    if let Err(e) = setup_chat_bot_commands(&bot, chat_id, &new_lang).await {
                        log::warn!("Failed to set chat-specific commands for lang {}: {}", lang_code, e);
                    }
                    let _ = bot
                        .answer_callback_query(callback_id.clone())
                        .text(i18n::t(&new_lang, "menu.language_saved"))
                        .await;

                    if preview_url_id.is_some() {
                        edit_main_menu(&bot, chat_id, message_id, Arc::clone(&db_pool), preview_url_id, None).await?;
                    } else {
                        edit_enhanced_main_menu(&bot, chat_id, message_id, Arc::clone(&db_pool)).await?;
                    }
                } else {
                    bot.answer_callback_query(callback_id)
                        .text(i18n::t(&lang, "menu.language_invalid"))
                        .await?;
                }
            } else if let Some(quality) = data.strip_prefix("quality:") {
                let _ = bot.answer_callback_query(callback_id.clone()).await;
                // Remove "quality:" prefix
                let conn = db::get_connection(&db_pool)
                    .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;
                db::set_user_video_quality(&conn, chat_id.0, quality)
                    .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

                // Get url_id from message context if available (check if we came from preview)
                // For now, we'll need to get it from the current menu's callback data
                // Since we don't have direct access, we'll check if back button has preview context
                // This is a limitation - we'd need to store url_id in quality callback data too
                // For simplicity, we'll just update the menu without url_id
                // Update the menu to show new selection
                show_video_quality_menu(&bot, chat_id, message_id, Arc::clone(&db_pool), None).await?;
            } else if data == "send_type:toggle" {
                let _ = bot.answer_callback_query(callback_id.clone()).await;
                let conn = db::get_connection(&db_pool)
                    .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

                // Get the current value and toggle it
                let current_value = db::get_user_send_as_document(&conn, chat_id.0).unwrap_or(0);
                let new_value = if current_value == 0 { 1 } else { 0 };

                db::set_user_send_as_document(&conn, chat_id.0, new_value)
                    .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

                // Refresh the menu
                show_video_quality_menu(&bot, chat_id, message_id, Arc::clone(&db_pool), None).await?;
            } else if data == "video:toggle_burn_subs" {
                let _ = bot.answer_callback_query(callback_id.clone()).await;
                let conn = db::get_connection(&db_pool)
                    .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

                // Get the current value and toggle it
                let current_value = db::get_user_burn_subtitles(&conn, chat_id.0).unwrap_or(false);
                let new_value = !current_value;

                db::set_user_burn_subtitles(&conn, chat_id.0, new_value)
                    .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

                log::info!(
                    "User {} toggled burn_subtitles: {} -> {}",
                    chat_id.0,
                    current_value,
                    new_value
                );

                // Refresh the menu
                show_video_quality_menu(&bot, chat_id, message_id, Arc::clone(&db_pool), None).await?;
            } else if let Some(bitrate) = data.strip_prefix("bitrate:") {
                let _ = bot.answer_callback_query(callback_id.clone()).await;
                // Remove "bitrate:" prefix
                let conn = db::get_connection(&db_pool)
                    .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;
                db::set_user_audio_bitrate(&conn, chat_id.0, bitrate)
                    .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

                // Update the menu to show new selection
                show_audio_bitrate_menu(&bot, chat_id, message_id, Arc::clone(&db_pool), None).await?;
            } else if data == "audio_send_type:toggle" {
                let _ = bot.answer_callback_query(callback_id.clone()).await;
                let conn = db::get_connection(&db_pool)
                    .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

                // Get the current value and toggle it
                let current_value = db::get_user_send_audio_as_document(&conn, chat_id.0).unwrap_or(0);
                let new_value = if current_value == 0 { 1 } else { 0 };

                db::set_user_send_audio_as_document(&conn, chat_id.0, new_value)
                    .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

                // Refresh the menu
                show_audio_bitrate_menu(&bot, chat_id, message_id, Arc::clone(&db_pool), None).await?;
            } else if data.starts_with("video_send_type:toggle:") {
                let _ = bot.answer_callback_query(callback_id.clone()).await;

                // Extract url_id from callback data: video_send_type:toggle:url_id
                let parts: Vec<&str> = data.split(':').collect();
                if parts.len() >= 3 {
                    let url_id = parts[2];

                    let conn = db::get_connection(&db_pool)
                        .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

                    // Get the current value and toggle it
                    let current_value = db::get_user_send_as_document(&conn, chat_id.0).unwrap_or(0);
                    let new_value = if current_value == 0 { 1 } else { 0 };

                    // Log the change
                    log::info!(
                        "🔄 Video send type toggled for user {}: {} -> {} ({})",
                        chat_id.0,
                        if current_value == 0 { "Media" } else { "Document" },
                        if new_value == 0 { "Media" } else { "Document" },
                        if new_value == 0 { "send_video" } else { "send_document" }
                    );

                    db::set_user_send_as_document(&conn, chat_id.0, new_value)
                        .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

                    // Get the current keyboard from the message and update only the toggle button
                    if let Some(teloxide::types::MaybeInaccessibleMessage::Regular(regular_msg)) = q.message.as_ref() {
                        // Get the current keyboard
                        if let Some(keyboard) = regular_msg.reply_markup() {
                            // Clone the keyboard and update the toggle button
                            let mut new_buttons = keyboard.inline_keyboard.clone();

                            // Find and update the toggle button (looking for callback video_send_type:toggle)
                            for row in &mut new_buttons {
                                for button in row {
                                    if let teloxide::types::InlineKeyboardButtonKind::CallbackData(ref cb_data) =
                                        button.kind
                                    {
                                        if cb_data.starts_with("video_send_type:toggle:") {
                                            // Update the button text
                                            button.text = if new_value == 0 {
                                                "📹 Отправка: Media ✓".to_string()
                                            } else {
                                                "📄 Отправка: Document ✓".to_string()
                                            };
                                            log::debug!("Updated toggle button text to: {}", button.text);
                                        }
                                    }
                                }
                            }

                            // Update only the keyboard without touching text or media
                            let new_keyboard = teloxide::types::InlineKeyboardMarkup::new(new_buttons);
                            let _ = bot
                                .edit_message_reply_markup(chat_id, message_id)
                                .reply_markup(new_keyboard)
                                .await;

                            log::info!(
                                "✅ Updated video preview keyboard for user {} (url_id: {})",
                                chat_id.0,
                                url_id
                            );
                        }
                    }
                }
            } else if data.starts_with("back:") {
                let _ = bot.answer_callback_query(callback_id.clone()).await;
                if data.starts_with("back:preview:") {
                    // Format: back:preview:url_id or back:preview:url_id:preview_msg_id
                    let parts: Vec<&str> = data.split(':').collect();
                    let url_id = parts[2];
                    let _preview_msg_id = if parts.len() >= 4 {
                        parts[3].parse::<i32>().ok().map(teloxide::types::MessageId)
                    } else {
                        None
                    };

                    // Get URL from cache and send new preview with updated format
                    match cache::get_url(&db_pool, url_id).await {
                        Some(url_str) => {
                            match url::Url::parse(&url_str) {
                                Ok(url) => {
                                    let conn = db::get_connection(&db_pool).map_err(|e| {
                                        RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string())))
                                    })?;
                                    let current_format = db::get_user_download_format(&conn, chat_id.0)
                                        .unwrap_or_else(|_| "mp3".to_string());
                                    let video_quality = if current_format == "mp4" {
                                        db::get_user_video_quality(&conn, chat_id.0).ok()
                                    } else {
                                        None
                                    };

                                    // Get metadata and update preview
                                    match crate::telegram::preview::get_preview_metadata(
                                        &url,
                                        Some(&current_format),
                                        video_quality.as_deref(),
                                    )
                                    .await
                                    {
                                        Ok(metadata) => {
                                            // Update existing preview message
                                            match crate::telegram::preview::update_preview_message(
                                                &bot,
                                                chat_id,
                                                message_id, // Use current message_id (which is the menu) to update it back to preview
                                                &url,
                                                &metadata,
                                                &current_format,
                                                video_quality.as_deref(),
                                                Arc::clone(&db_pool),
                                            )
                                            .await
                                            {
                                                Ok(_) => {}
                                                Err(e) => {
                                                    log::error!("Failed to update preview message: {:?}", e);
                                                    let _ = bot.send_message(chat_id, "Не удалось обновить превью. Попробуй отправить ссылку снова.").await;
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            log::error!("Failed to get preview metadata: {:?}", e);
                                            let _ = bot
                                                .send_message(
                                                    chat_id,
                                                    "Не удалось обновить превью. Попробуй отправить ссылку снова.",
                                                )
                                                .await;
                                        }
                                    }
                                }
                                Err(e) => {
                                    log::error!("Failed to parse URL from cache: {}", e);
                                    bot.answer_callback_query(callback_id)
                                        .text("Ошибка: неверная ссылка")
                                        .await?;
                                }
                            }
                        }
                        None => {
                            log::warn!("URL not found in cache for ID: {} (expired or invalid)", url_id);
                            bot.answer_callback_query(callback_id)
                                .text("Ссылка устарела, отправь её снова")
                                .await?;
                        }
                    }
                } else if data.starts_with("back:main:preview:") {
                    // Format: back:main:preview:url_id or back:main:preview:url_id:preview_msg_id
                    let parts: Vec<&str> = data.split(':').collect();
                    let url_id = parts[3];
                    let preview_msg_id = if parts.len() >= 5 {
                        parts[4].parse::<i32>().ok().map(teloxide::types::MessageId)
                    } else {
                        None
                    };
                    edit_main_menu(
                        &bot,
                        chat_id,
                        message_id,
                        Arc::clone(&db_pool),
                        Some(url_id),
                        preview_msg_id,
                    )
                    .await?;
                } else {
                    match data.as_str() {
                        "back:main" => {
                            edit_main_menu(&bot, chat_id, message_id, Arc::clone(&db_pool), None, None).await?;
                        }
                        "back:enhanced_main" => {
                            edit_enhanced_main_menu(&bot, chat_id, message_id, Arc::clone(&db_pool)).await?;
                        }
                        "back:start" => {
                            bot.edit_message_text(chat_id, message_id, "Хэй\\! Я Дора, дай мне ссылку и я скачаю ❤️‍🔥")
                                .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                                .await?;
                        }
                        _ => {}
                    }
                }
            } else if data.starts_with("format:") {
                // Format: format:mp3 or format:mp3:preview:url_id or format:mp3:preview:url_id:preview_msg_id
                let parts: Vec<&str> = data.split(':').collect();
                let format = parts[1];
                let is_from_preview = parts.len() >= 4 && parts[2] == "preview";
                let url_id = if is_from_preview { Some(parts[3]) } else { None };
                let preview_msg_id = if is_from_preview && parts.len() >= 5 {
                    parts[4].parse::<i32>().ok().map(teloxide::types::MessageId)
                } else {
                    None
                };
                if !is_from_preview {
                    let _ = bot.answer_callback_query(callback_id.clone()).await;
                }

                let conn = db::get_connection(&db_pool)
                    .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;
                db::set_user_download_format(&conn, chat_id.0, format)
                    .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

                if is_from_preview {
                    if let Some(id) = url_id {
                        start_download_from_preview(
                            &bot,
                            &callback_id,
                            chat_id,
                            message_id,
                            preview_msg_id,
                            id,
                            format,
                            None,
                            Arc::clone(&db_pool),
                            Arc::clone(&download_queue),
                            Arc::clone(&rate_limiter),
                        )
                        .await?;
                    }
                } else {
                    // Update the menu to show new selection
                    show_download_type_menu(&bot, chat_id, message_id, Arc::clone(&db_pool), None, None).await?;
                }
            } else if data.starts_with("dl:") {
                // Answer callback and delete preview IMMEDIATELY to prevent double-clicks
                // This gives instant visual feedback that the action was processed
                let _ = bot.answer_callback_query(callback_id.clone()).await;
                if let Err(e) = bot.delete_message(chat_id, message_id).await {
                    log::warn!("Failed to delete preview message: {:?}", e);
                }

                // Format: dl:format:url_id (legacy format)
                // Format: dl:format:quality:url_id (new format for video with quality selection)
                let parts: Vec<&str> = data.split(':').collect();

                if parts.len() >= 3 {
                    let format = parts[1];
                    let url_id = if parts.len() == 3 {
                        // Legacy format: dl:format:url_id
                        parts[2]
                    } else if parts.len() == 4 {
                        // New format: dl:format:quality:url_id
                        parts[3]
                    } else {
                        log::warn!("Invalid dl callback format: {}", data);
                        // Preview already deleted, send error as new message
                        let _ = bot.send_message(chat_id, "Ошибка: неверный формат запроса").await;
                        return Ok(());
                    };

                    // Extract quality if provided (new format)
                    let selected_quality = if parts.len() == 4 && format == "mp4" {
                        Some(parts[2].to_string()) // quality from dl:mp4:quality:url_id
                    } else {
                        None
                    };

                    log::debug!(
                        "Download button clicked: chat={}, url_id={}, format={}",
                        chat_id.0,
                        url_id,
                        format
                    );

                    // Get URL from cache by ID
                    match cache::get_url(&db_pool, url_id).await {
                        Some(url_str) => {
                            match Url::parse(&url_str) {
                                Ok(url) => {
                                    let original_message_id = tg_cache::get_link_message_id(&url_str).await;
                                    // Get user preferences for quality/bitrate and plan
                                    let conn = db::get_connection(&db_pool).map_err(|e| {
                                        RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string())))
                                    })?;
                                    let plan = match db::get_user(&conn, chat_id.0) {
                                        Ok(Some(ref user)) => user.plan.clone(),
                                        _ => "free".to_string(),
                                    };

                                    // Rate limit disabled - users can download without waiting
                                    let _ = (rate_limiter, &plan); // silence unused warnings

                                    // Handle "mp4+mp3" by adding two tasks to the queue
                                    if format == "mp4+mp3" {
                                        // Task 1: MP4 (video)
                                        let video_quality = if let Some(quality) = selected_quality {
                                            Some(quality)
                                        } else {
                                            Some(
                                                db::get_user_video_quality(&conn, chat_id.0)
                                                    .unwrap_or_else(|_| "best".to_string()),
                                            )
                                        };
                                        let task_mp4 = DownloadTask::from_plan(
                                            url.as_str().to_string(),
                                            chat_id,
                                            original_message_id,
                                            true, // is_video = true
                                            "mp4".to_string(),
                                            video_quality,
                                            None, // audio_bitrate is not needed for video
                                            &plan,
                                        );
                                        download_queue.add_task(task_mp4, Some(Arc::clone(&db_pool))).await;

                                        // Task 2: MP3 (audio)
                                        let audio_bitrate = Some(
                                            db::get_user_audio_bitrate(&conn, chat_id.0)
                                                .unwrap_or_else(|_| "320k".to_string()),
                                        );
                                        let task_mp3 = DownloadTask::from_plan(
                                            url.as_str().to_string(),
                                            chat_id,
                                            original_message_id,
                                            false, // is_video = false
                                            "mp3".to_string(),
                                            None, // video_quality is not needed for audio
                                            audio_bitrate,
                                            &plan,
                                        );
                                        download_queue.add_task(task_mp3, Some(Arc::clone(&db_pool))).await;

                                        log::info!(
                                            "Added 2 tasks to queue for mp4+mp3: MP4 and MP3 for chat {}",
                                            chat_id.0
                                        );
                                    } else {
                                        // Regular handling for a single format
                                        let video_quality = if format == "mp4" {
                                            if let Some(quality) = selected_quality {
                                                // Quality chosen by the user from preview
                                                Some(quality)
                                            } else {
                                                // Use the user's saved settings
                                                Some(
                                                    db::get_user_video_quality(&conn, chat_id.0)
                                                        .unwrap_or_else(|_| "best".to_string()),
                                                )
                                            }
                                        } else {
                                            None
                                        };
                                        let audio_bitrate = if format == "mp3" {
                                            Some(
                                                db::get_user_audio_bitrate(&conn, chat_id.0)
                                                    .unwrap_or_else(|_| "320k".to_string()),
                                            )
                                        } else {
                                            None
                                        };

                                        // Add task to queue
                                        let is_video = format == "mp4";
                                        let task = DownloadTask::from_plan(
                                            url.as_str().to_string(),
                                            chat_id,
                                            original_message_id,
                                            is_video,
                                            format.to_string(),
                                            video_quality,
                                            audio_bitrate,
                                            &plan,
                                        );
                                        download_queue.add_task(task, Some(Arc::clone(&db_pool))).await;
                                    }
                                }
                                Err(e) => {
                                    log::error!("Failed to parse URL from cache: {}", e);
                                    // Preview already deleted, send error as new message
                                    let _ = bot.send_message(chat_id, "❌ Ошибка: неверная ссылка").await;
                                }
                            }
                        }
                        None => {
                            log::warn!("URL not found in cache for ID: {} (expired or invalid)", url_id);
                            // Preview already deleted, send error as new message
                            let _ = bot.send_message(chat_id, "⏰ Ссылка устарела, отправь её снова").await;
                        }
                    }
                }
            } else if data.starts_with("pv:") {
                // Format: pv:action:url_id
                let parts: Vec<&str> = data.splitn(3, ':').collect();
                if parts.len() == 3 {
                    let action = parts[1];
                    match action {
                        "cancel" => {
                            let _ = bot.answer_callback_query(callback_id.clone()).await;
                            // Delete preview message
                            if let Err(e) = bot.delete_message(chat_id, message_id).await {
                                log::warn!("Failed to delete preview message: {:?}", e);
                            }
                        }
                        "set" => {
                            let _ = bot.answer_callback_query(callback_id.clone()).await;
                            let url_id = parts[2]; // Extract url_id from pv:set:url_id
                            let preview_msg_id = message_id; // Save preview message ID for later deletion

                            // Check if message contains photo (media message)
                            // If yes, delete it and send new text message with menu
                            // If no, edit existing text message
                            let has_photo = q
                                .message
                                .as_ref()
                                .and_then(|m| match m {
                                    teloxide::types::MaybeInaccessibleMessage::Regular(msg) => msg.photo(),
                                    _ => None,
                                })
                                .is_some();

                            if has_photo {
                                // Delete media message and send new text message
                                if let Err(e) = bot.delete_message(chat_id, message_id).await {
                                    log::warn!("Failed to delete preview message: {:?}", e);
                                }
                                // Send new text message with main settings menu, pass url_id and preview_msg_id
                                send_main_menu_as_new(
                                    &bot,
                                    chat_id,
                                    Arc::clone(&db_pool),
                                    Some(url_id),
                                    Some(preview_msg_id),
                                )
                                .await?;
                            } else {
                                // Edit existing text message to show main menu, pass url_id and preview_msg_id
                                edit_main_menu(
                                    &bot,
                                    chat_id,
                                    message_id,
                                    Arc::clone(&db_pool),
                                    Some(url_id),
                                    Some(preview_msg_id),
                                )
                                .await?;
                            }
                        }
                        _ => {
                            bot.answer_callback_query(callback_id.clone())
                                .text("Неизвестное действие")
                                .await?;
                        }
                    }
                }
            } else if data.starts_with("history:") {
                // Handle history callbacks
                handle_history_callback(
                    &bot,
                    callback_id,
                    chat_id,
                    message_id,
                    &data,
                    Arc::clone(&db_pool),
                    Arc::clone(&download_queue),
                    Arc::clone(&rate_limiter),
                )
                .await?;
            } else if let Some(format) = data.strip_prefix("export:") {
                // Handle export callbacks
                let _ = bot.answer_callback_query(callback_id.clone()).await;
                // Remove "export:" prefix
                handle_export(&bot, chat_id, format, Arc::clone(&db_pool)).await?;
            } else if data.starts_with("analytics:") {
                // Handle analytics callback buttons
                let _ = bot.answer_callback_query(callback_id.clone()).await;

                // Check administrator privileges
                let is_admin = i64::try_from(q.from.id.0).ok().map(admin::is_admin).unwrap_or(false);

                if !is_admin {
                    bot.send_message(chat_id, "❌ У тебя нет прав для выполнения этой команды.")
                        .await?;
                    return Ok(());
                }

                match data.as_str() {
                    "analytics:refresh" => {
                        // Re-generate and update analytics dashboard
                        use crate::telegram::analytics::generate_analytics_dashboard;
                        let dashboard = generate_analytics_dashboard(&db_pool).await;

                        let keyboard = InlineKeyboardMarkup::new(vec![
                            vec![
                                InlineKeyboardButton::callback("🔄 Обновить", "analytics:refresh"),
                                InlineKeyboardButton::callback("📊 Детали", "analytics:details"),
                            ],
                            vec![InlineKeyboardButton::callback("🔙 Закрыть", "analytics:close")],
                        ]);

                        bot.edit_message_text(chat_id, message_id, dashboard)
                            .parse_mode(ParseMode::MarkdownV2)
                            .reply_markup(keyboard)
                            .await?;
                    }
                    "analytics:details" => {
                        // Show detailed metrics menu
                        let details_text = "📊 *Детальные Метрики*\n\nВыберите категорию:";
                        let keyboard = InlineKeyboardMarkup::new(vec![
                            vec![InlineKeyboardButton::callback("⚡ Performance", "metrics:performance")],
                            vec![InlineKeyboardButton::callback("💰 Business", "metrics:business")],
                            vec![InlineKeyboardButton::callback("👥 Engagement", "metrics:engagement")],
                            vec![InlineKeyboardButton::callback("🔙 Назад", "analytics:refresh")],
                        ]);

                        bot.edit_message_text(chat_id, message_id, details_text)
                            .parse_mode(ParseMode::MarkdownV2)
                            .reply_markup(keyboard)
                            .await?;
                    }
                    "analytics:close" => {
                        // Delete the message
                        let _ = bot.delete_message(chat_id, message_id).await;
                    }
                    _ => {}
                }
            } else if data.starts_with("metrics:") {
                // Handle detailed metrics category callbacks
                let _ = bot.answer_callback_query(callback_id.clone()).await;

                // Check administrator privileges
                let is_admin = i64::try_from(q.from.id.0).ok().map(admin::is_admin).unwrap_or(false);

                if !is_admin {
                    bot.send_message(chat_id, "❌ У тебя нет прав для выполнения этой команды.")
                        .await?;
                    return Ok(());
                }

                let category = data.strip_prefix("metrics:").unwrap_or("");

                use crate::telegram::analytics::generate_metrics_report;
                let metrics_text = generate_metrics_report(&db_pool, Some(category.to_string())).await;

                let keyboard = InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::callback(
                    "🔙 К общей панели",
                    "analytics:refresh",
                )]]);

                bot.edit_message_text(chat_id, message_id, metrics_text)
                    .parse_mode(ParseMode::MarkdownV2)
                    .reply_markup(keyboard)
                    .await?;
            } else if data.starts_with("downloads:") {
                // Handle downloads callback queries
                use crate::telegram::downloads::handle_downloads_callback;
                handle_downloads_callback(
                    &bot,
                    callback_id.clone(),
                    chat_id,
                    message_id,
                    &data,
                    db_pool.clone(),
                    q.from.username.clone(),
                )
                .await?;
            } else if data.starts_with("cuts:") {
                use crate::telegram::cuts::handle_cuts_callback;
                handle_cuts_callback(
                    &bot,
                    callback_id.clone(),
                    chat_id,
                    message_id,
                    &data,
                    db_pool.clone(),
                    q.from.username.clone(),
                )
                .await?;
            } else if data.starts_with("admin:") {
                // Handle admin panel callbacks
                let _ = bot.answer_callback_query(callback_id.clone()).await;

                // Check administrator privileges
                let is_admin = i64::try_from(q.from.id.0).ok().map(admin::is_admin).unwrap_or(false);

                if !is_admin {
                    bot.send_message(chat_id, "❌ У тебя нет прав для выполнения этой команды.")
                        .await?;
                    return Ok(());
                }

                // Handle yt-dlp version/update callbacks
                if data == "admin:update_ytdlp" {
                    if let Err(e) = admin::handle_update_ytdlp_callback(&bot, chat_id, message_id).await {
                        log::error!("Failed to handle update_ytdlp callback: {}", e);
                    }
                    return Ok(());
                }

                if data == "admin:check_ytdlp_version" {
                    if let Err(e) = admin::handle_check_ytdlp_version_callback(&bot, chat_id, message_id).await {
                        log::error!("Failed to handle check_ytdlp_version callback: {}", e);
                    }
                    return Ok(());
                }

                if let Some(user_id_str) = data.strip_prefix("admin:user:") {
                    // Show the management menu for a specific user
                    // Remove "admin:user:" prefix

                    if let Ok(user_id) = user_id_str.parse::<i64>() {
                        match db::get_connection(&db_pool) {
                            Ok(conn) => {
                                match db::get_user(&conn, user_id) {
                                    Ok(Some(user)) => {
                                        let username_display = user
                                            .username
                                            .as_ref()
                                            .map(|u| format!("@{}", u))
                                            .unwrap_or_else(|| format!("ID: {}", user.telegram_id));

                                        let plan_emoji = match user.plan.as_str() {
                                            "premium" => "⭐",
                                            "vip" => "👑",
                                            _ => "🌟",
                                        };

                                        let sub_status = if user.telegram_charge_id.is_some() {
                                            if user.is_recurring {
                                                "💫🔄 Активная подписка \\(автопродление\\)"
                                            } else {
                                                "💫 Активная подписка \\(разовая\\)"
                                            }
                                        } else {
                                            "🔒 Нет подписки"
                                        };

                                        let expires_info = if let Some(expires) = &user.subscription_expires_at {
                                            let escaped_expires = expires.replace("-", "\\-").replace(":", "\\:");
                                            if user.is_recurring {
                                                format!("\n📅 Следующее списание: {}", escaped_expires)
                                            } else {
                                                format!("\n📅 Истекает: {}", escaped_expires)
                                            }
                                        } else {
                                            String::new()
                                        };

                                        // Build an action keyboard
                                        use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};

                                        let keyboard = InlineKeyboardMarkup::new(vec![
                                            vec![InlineKeyboardButton::callback(
                                                "🌟 Set Free",
                                                format!("admin:setplan:{}:free", user_id),
                                            )],
                                            vec![InlineKeyboardButton::callback(
                                                "⭐ Set Premium",
                                                format!("admin:setplan:{}:premium", user_id),
                                            )],
                                            vec![InlineKeyboardButton::callback(
                                                "👑 Set VIP",
                                                format!("admin:setplan:{}:vip", user_id),
                                            )],
                                            vec![InlineKeyboardButton::callback("🔙 Назад к списку", "admin:back")],
                                        ]);

                                        let _ = bot
                                            .edit_message_text(
                                                chat_id,
                                                message_id,
                                                format!(
                                                    "👤 *Управление пользователем*\n\n\
                                    Пользователь: {}\n\
                                    ID: `{}`\n\
                                    Текущий план: {} {}\n\
                                    Статус: {}{}\n\n\
                                    Выбери действие:",
                                                    username_display,
                                                    user.telegram_id,
                                                    plan_emoji,
                                                    user.plan,
                                                    sub_status,
                                                    expires_info
                                                ),
                                            )
                                            .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                                            .reply_markup(keyboard)
                                            .await;
                                    }
                                    Ok(None) => {}
                                    Err(e) => {
                                        log::error!("Failed to get user {}: {}", user_id, e);
                                    }
                                }
                            }
                            Err(e) => {
                                log::error!("Failed to get database connection: {}", e);
                            }
                        }
                    }
                } else if data.starts_with("admin:setplan:") {
                    // Change the user's plan
                    let parts: Vec<&str> = data.split(':').collect();
                    if parts.len() == 4 {
                        if let Ok(user_id) = parts[2].parse::<i64>() {
                            let new_plan = parts[3];

                            match db::get_connection(&db_pool) {
                                Ok(conn) => {
                                    match db::update_user_plan(&conn, user_id, new_plan) {
                                        Ok(_) => {
                                            let plan_emoji = match new_plan {
                                                "premium" => "⭐",
                                                "vip" => "👑",
                                                _ => "🌟",
                                            };
                                            let plan_name = match new_plan {
                                                "premium" => "Premium",
                                                "vip" => "VIP",
                                                _ => "Free",
                                            };

                                            // Send a notification to the user
                                            let user_chat_id = teloxide::types::ChatId(user_id);
                                            let _ = bot
                                                .send_message(
                                                    user_chat_id,
                                                    format!(
                                                        "💳 *Изменение плана подписки*\n\n\
                                                    Твой план был изменен администратором.\n\n\
                                                    *Новый план:* {} {}\n\n\
                                                    Изменения вступят в силу немедленно! 🎉",
                                                        plan_emoji, plan_name
                                                    ),
                                                )
                                                .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                                                .await;

                                            let _ = bot
                                                .edit_message_text(
                                                    chat_id,
                                                    message_id,
                                                    format!(
                                                        "✅ План пользователя {} изменен на {} {}",
                                                        user_id, plan_emoji, new_plan
                                                    ),
                                                )
                                                .await;
                                        }
                                        Err(e) => {
                                            log::error!("Failed to update user plan: {}", e);
                                        }
                                    }
                                }
                                Err(e) => {
                                    log::error!("Failed to get database connection: {}", e);
                                }
                            }
                        }
                    }
                } else if data == "admin:back" {
                    // Return to the user list
                    match db::get_connection(&db_pool) {
                        Ok(conn) => match db::get_all_users(&conn) {
                            Ok(users) => {
                                use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};

                                let mut keyboard_rows = Vec::new();
                                let mut current_row = Vec::new();

                                for user in users.iter().take(20) {
                                    let username_display = user
                                        .username
                                        .as_ref()
                                        .map(|u| format!("@{}", u))
                                        .unwrap_or_else(|| format!("ID:{}", user.telegram_id));

                                    let plan_emoji = match user.plan.as_str() {
                                        "premium" => "⭐",
                                        "vip" => "👑",
                                        _ => "🌟",
                                    };

                                    let button_text = format!("{} {}", plan_emoji, username_display);
                                    let callback_data = format!("admin:user:{}", user.telegram_id);

                                    current_row.push(InlineKeyboardButton::callback(button_text, callback_data));

                                    if current_row.len() == 2 {
                                        keyboard_rows.push(current_row.clone());
                                        current_row.clear();
                                    }
                                }

                                if !current_row.is_empty() {
                                    keyboard_rows.push(current_row);
                                }

                                let keyboard = InlineKeyboardMarkup::new(keyboard_rows);

                                let _ = bot
                                    .edit_message_text(
                                        chat_id,
                                        message_id,
                                        format!(
                                            "🔧 *Панель управления пользователями*\n\n\
                            Выбери пользователя для управления:\n\n\
                            Показано: {} из {}\n\n\
                            💡 Для управления конкретным пользователем используй:\n\
                            `/setplan <user_id> <plan>`",
                                            users.len().min(20),
                                            users.len()
                                        ),
                                    )
                                    .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                                    .reply_markup(keyboard)
                                    .await;
                            }
                            Err(e) => {
                                log::error!("Failed to get users: {}", e);
                            }
                        },
                        Err(e) => {
                            log::error!("Failed to get database connection: {}", e);
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

// ==================== Audio Cut ====================

async fn handle_audio_cut_callback(bot: Bot, q: CallbackQuery, db_pool: Arc<DbPool>) -> ResponseResult<()> {
    let callback_id = q.id.clone();
    let data = q.data.clone().unwrap_or_default();
    let chat_id = q.message.as_ref().map(|m| m.chat().id);
    let message_id = q.message.as_ref().map(|m| m.id());

    if let (Some(chat_id), Some(message_id)) = (chat_id, message_id) {
        let parts: Vec<&str> = data.split(':').collect();
        if parts.len() < 2 {
            bot.answer_callback_query(callback_id).await?;
            return Ok(());
        }

        let action = parts[1];
        let conn = db::get_connection(&db_pool)
            .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;
        if !db::is_premium_or_vip(&conn, chat_id.0)
            .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
        {
            bot.answer_callback_query(callback_id)
                .text("⭐ Эта функция доступна в Premium за ~$6/мес → /plan")
                .show_alert(true)
                .await?;
            return Ok(());
        }

        match action {
            "open" => {
                let session_id = if let Some(session_id) = parts.get(2) {
                    *session_id
                } else {
                    bot.answer_callback_query(callback_id)
                        .text("❌ Неверный запрос")
                        .await?;
                    return Ok(());
                };
                let session = match db::get_audio_effect_session(&conn, session_id)
                    .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
                {
                    Some(session) => session,
                    None => {
                        bot.answer_callback_query(callback_id)
                            .text("❌ Сессия не найдена")
                            .show_alert(true)
                            .await?;
                        return Ok(());
                    }
                };

                if session.is_expired() {
                    bot.answer_callback_query(callback_id)
                        .text("❌ Сессия истекла (24 часа). Скачайте трек заново.")
                        .show_alert(true)
                        .await?;
                    return Ok(());
                }

                let now = chrono::Utc::now();
                let cut_session = db::AudioCutSession {
                    id: Uuid::new_v4().to_string(),
                    user_id: chat_id.0,
                    audio_session_id: session_id.to_string(),
                    created_at: now,
                    expires_at: now + chrono::Duration::minutes(10),
                };
                db::upsert_audio_cut_session(&conn, &cut_session)
                    .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

                bot.answer_callback_query(callback_id).await?;

                if let Err(e) = bot.edit_message_reply_markup(chat_id, message_id).await {
                    log::warn!("Failed to remove buttons from audio message: {}", e);
                }

                let keyboard = InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::callback(
                    "❌ Отмена".to_string(),
                    "ac:cancel".to_string(),
                )]]);

                crate::telegram::send_message_markdown_v2(
                    &bot,
                    chat_id,
                    "✂️ Отправь интервалы для вырезки аудио в формате `мм:сс-мм:сс` или `чч:мм:сс-чч:мм:сс`\\.\nМожно несколько через запятую\\.\n\nПример: `00:10-00:25, 01:00-01:10`\n\nИли напиши `отмена`\\.",
                    Some(keyboard),
                )
                .await?;
            }
            "cancel" => {
                db::delete_audio_cut_session_by_user(&conn, chat_id.0)
                    .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;
                bot.answer_callback_query(callback_id).await?;
                let _ = bot.delete_message(chat_id, message_id).await;
            }
            _ => {
                bot.answer_callback_query(callback_id).await?;
            }
        }
    }

    Ok(())
}

// ==================== Audio Effects UI ====================

/// Create audio effects keyboard with pitch and tempo controls
fn create_audio_effects_keyboard(
    session_id: &str,
    current_pitch: i8,
    current_tempo: f32,
    current_bass: i8,
    current_morph: crate::download::audio_effects::MorphProfile,
) -> InlineKeyboardMarkup {
    use teloxide::types::InlineKeyboardButton;

    let build_pitch_row = |values: &[i8]| -> Vec<InlineKeyboardButton> {
        values
            .iter()
            .map(|&value| {
                let marker = if current_pitch == value { " ✓" } else { "" };
                let prefix = if value >= 0 { "P+" } else { "P" };
                let label = format!("{}{}{}", prefix, value.abs(), marker);
                InlineKeyboardButton::callback(label, format!("ae:pitch:{}:{}", session_id, value))
            })
            .collect()
    };

    let pitch_rows = vec![build_pitch_row(&[-3, -2, -1]), build_pitch_row(&[0, 1, 2, 3])];

    let build_tempo_row = |values: &[f32]| -> Vec<InlineKeyboardButton> {
        values
            .iter()
            .map(|&value| {
                let marker = if (current_tempo - value).abs() < 0.01 {
                    " ✓"
                } else {
                    ""
                };
                InlineKeyboardButton::callback(
                    format!("T{}x{}", value, marker),
                    format!("ae:tempo:{}:{}", session_id, value),
                )
            })
            .collect()
    };

    let tempo_rows = vec![build_tempo_row(&[0.5, 0.75]), build_tempo_row(&[1.0, 1.25, 1.5, 2.0])];

    let build_bass_row = |values: &[i8]| -> Vec<InlineKeyboardButton> {
        values
            .iter()
            .map(|&value| {
                let marker = if current_bass == value { " ✓" } else { "" };
                InlineKeyboardButton::callback(
                    format!("B{:+}{}", value, marker),
                    format!("ae:bass:{}:{:+}", session_id, value),
                )
            })
            .collect()
    };

    let bass_rows = vec![build_bass_row(&[-6, -3, 0]), build_bass_row(&[3, 6])];

    let action_row = vec![
        InlineKeyboardButton::callback("✅ Apply Changes", format!("ae:apply:{}", session_id)),
        InlineKeyboardButton::callback("🔄 Reset", format!("ae:reset:{}", session_id)),
    ];

    let skip_row = vec![InlineKeyboardButton::callback(
        "⏭️ Skip",
        format!("ae:skip:{}", session_id),
    )];

    let morph_row = vec![InlineKeyboardButton::callback(
        format!(
            "🤖 M: {}",
            match current_morph {
                crate::download::audio_effects::MorphProfile::None => "Off",
                crate::download::audio_effects::MorphProfile::Soft => "Soft",
                crate::download::audio_effects::MorphProfile::Aggressive => "Aggro",
                crate::download::audio_effects::MorphProfile::Lofi => "LoFi",
                crate::download::audio_effects::MorphProfile::Wide => "Wide",
            }
        ),
        format!("ae:morph:{}", session_id),
    )];

    let mut rows: Vec<Vec<InlineKeyboardButton>> = Vec::new();
    rows.extend(pitch_rows);
    rows.extend(tempo_rows);
    rows.extend(bass_rows);
    rows.push(morph_row);
    rows.push(action_row);
    rows.push(skip_row);

    InlineKeyboardMarkup::new(rows)
}

/// Show audio effects editor by sending a new message
async fn show_audio_effects_editor(
    bot: &Bot,
    chat_id: ChatId,
    session: &crate::download::audio_effects::AudioEffectSession,
) -> ResponseResult<()> {
    let pitch_str = escape_markdown(&format!("{:+}", session.pitch_semitones));
    let tempo_str = escape_markdown(&format!("{}", session.tempo_factor));

    let bass_str = escape_markdown(&format!("{:+} dB", session.bass_gain_db));
    let morph_str = match session.morph_profile {
        crate::download::audio_effects::MorphProfile::None => "Off",
        crate::download::audio_effects::MorphProfile::Soft => "Soft",
        crate::download::audio_effects::MorphProfile::Aggressive => "Aggro",
        crate::download::audio_effects::MorphProfile::Lofi => "LoFi",
        crate::download::audio_effects::MorphProfile::Wide => "Wide",
    };

    let text = format!(
        "🎵 *Audio Effects Editor*\n\
        Title: {}\n\
        Current: P {} \\| T {}x \\| B {} \\| M {}\n\n\
        Adjust pitch, tempo, bass, morph preset, then press Apply\\.",
        escape_markdown(&session.title),
        pitch_str,
        tempo_str,
        bass_str,
        escape_markdown(morph_str),
    );

    let keyboard = create_audio_effects_keyboard(
        &session.id,
        session.pitch_semitones,
        session.tempo_factor,
        session.bass_gain_db,
        session.morph_profile,
    );

    bot.send_message(chat_id, "P = Pitch • T = Tempo • B = Bass").await?;

    bot.send_message(chat_id, text)
        .parse_mode(teloxide::types::ParseMode::MarkdownV2)
        .reply_markup(keyboard)
        .await?;

    Ok(())
}

/// Update existing audio effects editor message
async fn update_audio_effects_editor(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    session: &crate::download::audio_effects::AudioEffectSession,
) -> ResponseResult<()> {
    let pitch_str = escape_markdown(&format!("{:+}", session.pitch_semitones));
    let tempo_str = escape_markdown(&format!("{}", session.tempo_factor));

    let bass_str = escape_markdown(&format!("{:+} dB", session.bass_gain_db));
    let morph_str = match session.morph_profile {
        crate::download::audio_effects::MorphProfile::None => "Off",
        crate::download::audio_effects::MorphProfile::Soft => "Soft",
        crate::download::audio_effects::MorphProfile::Aggressive => "Aggro",
        crate::download::audio_effects::MorphProfile::Lofi => "LoFi",
        crate::download::audio_effects::MorphProfile::Wide => "Wide",
    };

    let text = format!(
        "🎵 *Audio Effects Editor*\n\
        Title: {}\n\
        Current: P {} \\| T {}x \\| B {} \\| M {}\n\n\
        Adjust pitch, tempo, bass, morph preset, then press Apply\\.",
        escape_markdown(&session.title),
        pitch_str,
        tempo_str,
        bass_str,
        escape_markdown(morph_str),
    );

    let keyboard = create_audio_effects_keyboard(
        &session.id,
        session.pitch_semitones,
        session.tempo_factor,
        session.bass_gain_db,
        session.morph_profile,
    );

    bot.edit_message_text(chat_id, message_id, text)
        .parse_mode(teloxide::types::ParseMode::MarkdownV2)
        .reply_markup(keyboard)
        .await?;

    Ok(())
}

/// Handle audio effects callbacks
pub async fn handle_audio_effects_callback(
    bot: Bot,
    q: CallbackQuery,
    db_pool: Arc<crate::storage::db::DbPool>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use crate::storage::db;

    let callback_id = q.id.clone();
    let data = q.data.clone().ok_or("No callback data")?;

    let message = q.message.ok_or("No message in callback")?;
    let chat_id = message.chat().id;
    let message_id = message.id();

    // Parse callback data
    let parts: Vec<&str> = data.split(':').collect();
    if parts.len() < 2 {
        bot.answer_callback_query(callback_id).await?;
        return Ok(());
    }

    let action = parts[1];

    // Check Premium/VIP access
    let conn = db::get_connection(&db_pool)?;
    if !db::is_premium_or_vip(&conn, chat_id.0)? {
        bot.answer_callback_query(callback_id)
            .text("⭐ Эта функция доступна в Premium за ~$6/мес → /plan")
            .show_alert(true)
            .await?;
        return Ok(());
    }

    match action {
        "open" => {
            let session_id = parts.get(2).ok_or("Missing session_id")?;

            let session = db::get_audio_effect_session(&conn, session_id)?.ok_or("Session not found")?;

            if session.is_expired() {
                bot.answer_callback_query(callback_id)
                    .text("❌ Сессия истекла (24 часа). Скачайте трек заново.")
                    .show_alert(true)
                    .await?;
                return Ok(());
            }

            bot.answer_callback_query(callback_id).await?;

            // Remove the "Edit Audio" button from the audio message
            if let Err(e) = bot.edit_message_reply_markup(chat_id, message_id).await {
                log::warn!("Failed to remove button from audio message: {}", e);
            }

            // Send a new editor message
            show_audio_effects_editor(&bot, chat_id, &session).await?;
        }

        "pitch" => {
            let session_id = parts.get(2).ok_or("Missing session_id")?;
            let pitch_str = parts.get(3).ok_or("Missing pitch value")?;
            let pitch: i8 = pitch_str.parse().map_err(|_| "Invalid pitch")?;

            let mut session = db::get_audio_effect_session(&conn, session_id)?.ok_or("Session not found")?;

            if session.processing {
                bot.answer_callback_query(callback_id)
                    .text("⏳ Подождите, идёт обработка...")
                    .await?;
                return Ok(());
            }

            session.pitch_semitones = pitch;
            db::update_audio_effect_session(
                &conn,
                session_id,
                pitch,
                session.tempo_factor,
                session.bass_gain_db,
                session.morph_profile.as_str(),
                &session.current_file_path,
                session.version,
            )?;

            bot.answer_callback_query(callback_id).await?;
            update_audio_effects_editor(&bot, chat_id, message_id, &session).await?;
        }

        "tempo" => {
            let session_id = parts.get(2).ok_or("Missing session_id")?;
            let tempo_str = parts.get(3).ok_or("Missing tempo value")?;
            let tempo: f32 = tempo_str.parse().map_err(|_| "Invalid tempo")?;

            let mut session = db::get_audio_effect_session(&conn, session_id)?.ok_or("Session not found")?;

            if session.processing {
                bot.answer_callback_query(callback_id)
                    .text("⏳ Подождите, идёт обработка...")
                    .await?;
                return Ok(());
            }

            session.tempo_factor = tempo;
            db::update_audio_effect_session(
                &conn,
                session_id,
                session.pitch_semitones,
                tempo,
                session.bass_gain_db,
                session.morph_profile.as_str(),
                &session.current_file_path,
                session.version,
            )?;

            bot.answer_callback_query(callback_id).await?;
            update_audio_effects_editor(&bot, chat_id, message_id, &session).await?;
        }

        "bass" => {
            let session_id = parts.get(2).ok_or("Missing session_id")?;
            let bass_str = parts.get(3).ok_or("Missing bass value")?;
            let bass: i8 = bass_str.parse().map_err(|_| "Invalid bass")?;

            let mut session = db::get_audio_effect_session(&conn, session_id)?.ok_or("Session not found")?;

            if session.processing {
                bot.answer_callback_query(callback_id)
                    .text("⏳ Подождите, идёт обработка...")
                    .await?;
                return Ok(());
            }

            session.bass_gain_db = bass;
            db::update_audio_effect_session(
                &conn,
                session_id,
                session.pitch_semitones,
                session.tempo_factor,
                bass,
                session.morph_profile.as_str(),
                &session.current_file_path,
                session.version,
            )?;

            bot.answer_callback_query(callback_id).await?;
            update_audio_effects_editor(&bot, chat_id, message_id, &session).await?;
        }

        "morph" => {
            let session_id = parts.get(2).ok_or("Missing session_id")?;

            let mut session = db::get_audio_effect_session(&conn, session_id)?.ok_or("Session not found")?;

            if session.processing {
                bot.answer_callback_query(callback_id)
                    .text("⏳ Подождите, идёт обработка...")
                    .await?;
                return Ok(());
            }

            // Cycle morph profiles
            session.morph_profile = match session.morph_profile {
                crate::download::audio_effects::MorphProfile::None => {
                    crate::download::audio_effects::MorphProfile::Soft
                }
                crate::download::audio_effects::MorphProfile::Soft => {
                    crate::download::audio_effects::MorphProfile::Aggressive
                }
                crate::download::audio_effects::MorphProfile::Aggressive => {
                    crate::download::audio_effects::MorphProfile::Lofi
                }
                crate::download::audio_effects::MorphProfile::Lofi => {
                    crate::download::audio_effects::MorphProfile::Wide
                }
                crate::download::audio_effects::MorphProfile::Wide => {
                    crate::download::audio_effects::MorphProfile::None
                }
            };

            db::update_audio_effect_session(
                &conn,
                session_id,
                session.pitch_semitones,
                session.tempo_factor,
                session.bass_gain_db,
                session.morph_profile.as_str(),
                &session.current_file_path,
                session.version,
            )?;

            bot.answer_callback_query(callback_id).await?;
            update_audio_effects_editor(&bot, chat_id, message_id, &session).await?;
        }

        "apply" => {
            let session_id = parts.get(2).ok_or("Missing session_id")?;

            let session = db::get_audio_effect_session(&conn, session_id)?.ok_or("Session not found")?;

            if session.processing {
                bot.answer_callback_query(callback_id)
                    .text("⏳ Подождите, идёт обработка...")
                    .await?;
                return Ok(());
            }

            bot.answer_callback_query(callback_id).await?;

            // Set processing flag
            db::set_session_processing(&conn, session_id, true)?;

            // Show processing message
            edit_caption_or_text(
                &bot,
                chat_id,
                message_id,
                format!(
                    "⏳ *Обрабатываю аудио\\.\\.\\.*\n\n\
                    Pitch: {}\n\
                    Tempo: {}x\n\
                    Bass: {}\n\
                    Morph: {}\n\n\
                    {}",
                    escape_markdown(&format!("{:+}", session.pitch_semitones)),
                    escape_markdown(&format!("{}", session.tempo_factor)),
                    escape_markdown(&format!("{:+} dB", session.bass_gain_db)),
                    escape_markdown(match session.morph_profile {
                        crate::download::audio_effects::MorphProfile::None => "Off",
                        crate::download::audio_effects::MorphProfile::Soft => "Soft",
                        crate::download::audio_effects::MorphProfile::Aggressive => "Aggro",
                        crate::download::audio_effects::MorphProfile::Lofi => "LoFi",
                        crate::download::audio_effects::MorphProfile::Wide => "Wide",
                    }),
                    if session.duration > 300 {
                        "Это может занять до 30 секунд\\.\\.\\."
                    } else {
                        "Подождите несколько секунд\\.\\.\\."
                    }
                ),
                None,
            )
            .await?;

            // Spawn processing task
            let bot_clone = bot.clone();
            let db_pool_clone = Arc::clone(&db_pool);
            let session_clone = session.clone();
            tokio::spawn(async move {
                if let Err(e) =
                    process_audio_effects(bot_clone, chat_id, message_id, session_clone, db_pool_clone).await
                {
                    log::error!("Failed to process audio effects: {}", e);
                }
            });
        }

        "reset" => {
            let session_id = parts.get(2).ok_or("Missing session_id")?;

            let mut session = db::get_audio_effect_session(&conn, session_id)?.ok_or("Session not found")?;

            session.pitch_semitones = 0;
            session.tempo_factor = 1.0;
            session.bass_gain_db = 0;
            session.morph_profile = crate::download::audio_effects::MorphProfile::None;
            db::update_audio_effect_session(
                &conn,
                session_id,
                0,
                1.0,
                0,
                crate::download::audio_effects::MorphProfile::None.as_str(),
                &session.current_file_path,
                session.version,
            )?;

            bot.answer_callback_query(callback_id).await?;
            update_audio_effects_editor(&bot, chat_id, message_id, &session).await?;
        }

        "cancel" => {
            bot.answer_callback_query(callback_id).await?;
            bot.delete_message(chat_id, message_id).await?;
        }

        "skip" => {
            bot.answer_callback_query(callback_id).await?;
            bot.delete_message(chat_id, message_id).await?;
        }

        "again" => {
            let session_id = parts.get(2).ok_or("Missing session_id")?;

            let session = db::get_audio_effect_session(&conn, session_id)?.ok_or("Session not found")?;

            if session.is_expired() {
                bot.answer_callback_query(callback_id)
                    .text("❌ Сессия истекла (24 часа). Скачайте трек заново.")
                    .show_alert(true)
                    .await?;
                return Ok(());
            }

            bot.answer_callback_query(callback_id).await?;

            // Send new editor message
            let pitch_str = escape_markdown(&format!("{:+}", session.pitch_semitones));
            let tempo_str = escape_markdown(&format!("{}", session.tempo_factor));

            let text = format!(
                "🎵 *Audio Effects Editor*\n\
                Title: {}\n\
                Current: Pitch {} \\| Tempo {}x \\| Bass {} \\| Morph {}\n\n\
                Adjust pitch, tempo, bass, morph preset, then press Apply\\.",
                escape_markdown(&session.title),
                pitch_str,
                tempo_str,
                escape_markdown(&format!("{:+} dB", session.bass_gain_db)),
                escape_markdown(match session.morph_profile {
                    crate::download::audio_effects::MorphProfile::None => "Off",
                    crate::download::audio_effects::MorphProfile::Soft => "Soft",
                    crate::download::audio_effects::MorphProfile::Aggressive => "Aggro",
                    crate::download::audio_effects::MorphProfile::Lofi => "LoFi",
                    crate::download::audio_effects::MorphProfile::Wide => "Wide",
                })
            );

            let keyboard = create_audio_effects_keyboard(
                &session.id,
                session.pitch_semitones,
                session.tempo_factor,
                session.bass_gain_db,
                session.morph_profile,
            );

            // New editor message after applying again (plain text message)
            bot.send_message(chat_id, text)
                .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                .reply_markup(keyboard)
                .await?;
        }

        "original" => {
            let session_id = parts.get(2).ok_or("Missing session_id")?;

            let session = db::get_audio_effect_session(&conn, session_id)?.ok_or("Session not found")?;

            bot.answer_callback_query(callback_id).await?;

            // Send original file
            if std::path::Path::new(&session.original_file_path).exists() {
                let file = teloxide::types::InputFile::file(&session.original_file_path);
                bot.send_audio(chat_id, file)
                    .title(format!("{} (Original)", session.title))
                    .duration(session.duration)
                    .await?;
            } else {
                bot.send_message(chat_id, "❌ Оригинальный файл не найден. Возможно, сессия истекла.")
                    .await?;
            }
        }

        _ => {
            bot.answer_callback_query(callback_id).await?;
        }
    }

    Ok(())
}

/// Process audio effects and send modified file
async fn process_audio_effects(
    bot: Bot,
    chat_id: ChatId,
    editor_message_id: MessageId,
    session: crate::download::audio_effects::AudioEffectSession,
    db_pool: Arc<crate::storage::db::DbPool>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use crate::core::config;
    use crate::storage::db;
    use std::path::Path;

    let session_id = session.id.clone();
    let new_version = session.version + 1;

    // Generate output path
    let output_path_raw =
        crate::download::audio_effects::get_modified_file_path(&session_id, new_version, &config::DOWNLOAD_FOLDER);
    let output_path = shellexpand::tilde(&output_path_raw).into_owned();
    if let Some(parent) = Path::new(&output_path).parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    // Apply effects
    let settings = session.settings();
    let result =
        crate::download::audio_effects::apply_audio_effects(&session.original_file_path, &output_path, &settings).await;

    // Clear processing flag
    let conn = db::get_connection(&db_pool)?;
    db::set_session_processing(&conn, &session_id, false)?;

    match result {
        Ok(_) => {
            // Send modified audio
            let file = teloxide::types::InputFile::file(&output_path);
            let title = format!(
                "{} (Pitch {:+}, Tempo {}x, Bass {:+} dB, Morph {})",
                session.title,
                session.pitch_semitones,
                session.tempo_factor,
                session.bass_gain_db,
                match session.morph_profile {
                    crate::download::audio_effects::MorphProfile::None => "Off",
                    crate::download::audio_effects::MorphProfile::Soft => "Soft",
                    crate::download::audio_effects::MorphProfile::Aggressive => "Aggro",
                    crate::download::audio_effects::MorphProfile::Lofi => "LoFi",
                    crate::download::audio_effects::MorphProfile::Wide => "Wide",
                }
            );

            let sent_message = bot
                .send_audio(chat_id, file)
                .title(&title)
                .duration(session.duration)
                .await?;

            // Add "Edit Again" and "Get Original" buttons
            let keyboard = InlineKeyboardMarkup::new(vec![vec![
                InlineKeyboardButton::callback("🎛️ Edit Again", format!("ae:again:{}", session_id)),
                InlineKeyboardButton::callback("📥 Get Original", format!("ae:original:{}", session_id)),
            ]]);

            // Replace the sent audio message caption with the new buttons (no text change)
            bot.edit_message_reply_markup(chat_id, sent_message.id)
                .reply_markup(keyboard)
                .await?;

            // Update session in DB
            db::update_audio_effect_session(
                &conn,
                &session_id,
                session.pitch_semitones,
                session.tempo_factor,
                session.bass_gain_db,
                session.morph_profile.as_str(),
                &output_path,
                new_version,
            )?;

            // Delete old version file if exists
            if session.version > 0 && session.current_file_path != session.original_file_path {
                let _ = tokio::fs::remove_file(&session.current_file_path).await;
            }

            // Delete editor message
            bot.delete_message(chat_id, editor_message_id).await?;

            log::info!(
                "Audio effects applied for session {}: pitch {:+}, tempo {}x",
                session_id,
                session.pitch_semitones,
                session.tempo_factor
            );
        }
        Err(e) => {
            log::error!("Failed to apply audio effects: {}", e);

            let mut error_msg = e.to_string();
            if error_msg.chars().count() > 900 {
                let trimmed: String = error_msg.chars().take(900).collect();
                error_msg = format!("{} …", trimmed);
            }

            let error_text = format!("❌ *Ошибка обработки*\n\n{}", escape_markdown(&error_msg));

            edit_caption_or_text(&bot, chat_id, editor_message_id, error_text, None).await?;
        }
    }

    Ok(())
}

/// Shows the enhanced main menu with user's current settings and main action buttons.
///
/// This is the improved main menu that replaces the old /start handler.
///
/// # Arguments
///
/// * `bot` - Telegram bot instance
/// * `chat_id` - User's chat ID
/// * `db_pool` - Database connection pool
///
/// # Returns
///
/// Returns `ResponseResult<Message>` with the sent message or an error.
pub async fn show_enhanced_main_menu(bot: &Bot, chat_id: ChatId, db_pool: Arc<DbPool>) -> ResponseResult<Message> {
    let lang = i18n::user_lang_from_pool(&db_pool, chat_id.0);
    let (format, video_quality, audio_bitrate, plan) = match db::get_connection(&db_pool) {
        Ok(conn) => {
            let format = db::get_user_download_format(&conn, chat_id.0).unwrap_or_else(|_| "mp3".to_string());
            let video_quality = db::get_user_video_quality(&conn, chat_id.0).unwrap_or_else(|_| "best".to_string());
            let audio_bitrate = db::get_user_audio_bitrate(&conn, chat_id.0).unwrap_or_else(|_| "320k".to_string());
            let plan = match db::get_user(&conn, chat_id.0) {
                Ok(Some(user)) => user.plan,
                _ => "free".to_string(),
            };
            (format, video_quality, audio_bitrate, plan)
        }
        Err(e) => {
            log::error!("Failed to get DB connection for enhanced menu: {}", e);
            (
                "mp3".to_string(),
                "best".to_string(),
                "320k".to_string(),
                "free".to_string(),
            )
        }
    };

    // Format emoji
    let format_emoji = match format.as_str() {
        "mp3" => "🎵 MP3",
        "mp4" => "🎬 MP4",
        "mp4+mp3" => "🎬🎵 MP4 \\+ MP3",
        "srt" => "📝 SRT",
        "txt" => "📄 TXT",
        _ => "🎵 MP3",
    };

    // Quality or bitrate line based on format
    let quality_line = if format == "mp4" {
        let quality_display = match video_quality.as_str() {
            "1080p" => "1080p",
            "720p" => "720p",
            "480p" => "480p",
            "360p" => "360p",
            _ => "Best",
        };
        let mut args = FluentArgs::new();
        args.set("value", quality_display);
        i18n::t_args(&lang, "menu.quality_line", &args)
    } else {
        let bitrate_display = match audio_bitrate.as_str() {
            "128k" => "128 kbps",
            "192k" => "192 kbps",
            "256k" => "256 kbps",
            "320k" => "320 kbps",
            _ => "320 kbps",
        };
        let mut args = FluentArgs::new();
        args.set("value", bitrate_display);
        i18n::t_args(&lang, "menu.bitrate_line", &args)
    };

    // Plan display
    let plan_display = match plan.as_str() {
        "premium" => i18n::t(&lang, "menu.plan_premium"),
        "vip" => i18n::t(&lang, "menu.plan_vip"),
        _ => i18n::t(&lang, "menu.plan_free"),
    };

    let (text, keyboard) = build_enhanced_menu(&lang, format_emoji, &quality_line, &plan_display);

    bot.send_message(chat_id, text)
        .parse_mode(teloxide::types::ParseMode::MarkdownV2)
        .reply_markup(keyboard)
        .await
}

/// Edits existing message to show the enhanced main menu.
///
/// Used for "back" buttons to return to main menu without sending a new message.
///
/// # Arguments
///
/// * `bot` - Telegram bot instance
/// * `chat_id` - User's chat ID
/// * `message_id` - ID of message to edit
/// * `db_pool` - Database connection pool
async fn edit_enhanced_main_menu(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    db_pool: Arc<DbPool>,
) -> ResponseResult<()> {
    let lang = i18n::user_lang_from_pool(&db_pool, chat_id.0);
    let (format, video_quality, audio_bitrate, plan) = match db::get_connection(&db_pool) {
        Ok(conn) => {
            let format = db::get_user_download_format(&conn, chat_id.0).unwrap_or_else(|_| "mp3".to_string());
            let video_quality = db::get_user_video_quality(&conn, chat_id.0).unwrap_or_else(|_| "best".to_string());
            let audio_bitrate = db::get_user_audio_bitrate(&conn, chat_id.0).unwrap_or_else(|_| "320k".to_string());
            let plan = match db::get_user(&conn, chat_id.0) {
                Ok(Some(user)) => user.plan,
                _ => "free".to_string(),
            };
            (format, video_quality, audio_bitrate, plan)
        }
        Err(e) => {
            log::error!("Failed to get DB connection for enhanced menu: {}", e);
            (
                "mp3".to_string(),
                "best".to_string(),
                "320k".to_string(),
                "free".to_string(),
            )
        }
    };

    let format_emoji = match format.as_str() {
        "mp3" => "🎵 MP3",
        "mp4" => "🎬 MP4",
        "mp4+mp3" => "🎬🎵 MP4 \\+ MP3",
        "srt" => "📝 SRT",
        "txt" => "📄 TXT",
        _ => "🎵 MP3",
    };

    let quality_line = if format == "mp4" {
        let quality_display = match video_quality.as_str() {
            "1080p" => "1080p",
            "720p" => "720p",
            "480p" => "480p",
            "360p" => "360p",
            _ => "Best",
        };
        let mut args = FluentArgs::new();
        args.set("value", quality_display);
        i18n::t_args(&lang, "menu.quality_line", &args)
    } else {
        let bitrate_display = match audio_bitrate.as_str() {
            "128k" => "128 kbps",
            "192k" => "192 kbps",
            "256k" => "256 kbps",
            "320k" => "320 kbps",
            _ => "320 kbps",
        };
        let mut args = FluentArgs::new();
        args.set("value", bitrate_display);
        i18n::t_args(&lang, "menu.bitrate_line", &args)
    };

    let plan_display = match plan.as_str() {
        "premium" => i18n::t(&lang, "menu.plan_premium"),
        "vip" => i18n::t(&lang, "menu.plan_vip"),
        _ => i18n::t(&lang, "menu.plan_free"),
    };

    let (text, keyboard) = build_enhanced_menu(&lang, format_emoji, &quality_line, &plan_display);

    edit_caption_or_text(bot, chat_id, message_id, text, Some(keyboard)).await
}

/// Shows detailed view of user's current settings.
///
/// Displays all user preferences including format, quality, bitrate, send type, and plan.
///
/// # Arguments
///
/// * `bot` - Telegram bot instance
/// * `chat_id` - User's chat ID
/// * `message_id` - ID of message to edit
/// * `db_pool` - Database connection pool
async fn show_current_settings_detail(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    db_pool: Arc<DbPool>,
) -> ResponseResult<()> {
    let conn = db::get_connection(&db_pool)
        .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

    let format = db::get_user_download_format(&conn, chat_id.0).unwrap_or_else(|_| "mp3".to_string());
    let video_quality = db::get_user_video_quality(&conn, chat_id.0).unwrap_or_else(|_| "best".to_string());
    let audio_bitrate = db::get_user_audio_bitrate(&conn, chat_id.0).unwrap_or_else(|_| "320k".to_string());
    let send_as_document = db::get_user_send_as_document(&conn, chat_id.0).unwrap_or(0);
    let send_audio_as_document = db::get_user_send_audio_as_document(&conn, chat_id.0).unwrap_or(0);

    let plan = match db::get_user(&conn, chat_id.0) {
        Ok(Some(user)) => user.plan,
        _ => "free".to_string(),
    };

    let format_emoji = match format.as_str() {
        "mp3" => "🎵 MP3",
        "mp4" => "🎬 MP4",
        "mp4+mp3" => "🎬🎵 MP4 \\+ MP3",
        "srt" => "📝 SRT",
        "txt" => "📄 TXT",
        _ => "🎵 MP3",
    };

    let quality_line = if format == "mp4" || format == "mp4+mp3" {
        let quality_display = match video_quality.as_str() {
            "1080p" => "1080p",
            "720p" => "720p",
            "480p" => "480p",
            "360p" => "360p",
            _ => "Best",
        };
        format!("🎬 *Качество видео:* {}", quality_display)
    } else {
        "".to_string()
    };

    let bitrate_line = if format == "mp3" || format == "mp4+mp3" {
        let bitrate_display = match audio_bitrate.as_str() {
            "128k" => "128 kbps",
            "192k" => "192 kbps",
            "256k" => "256 kbps",
            "320k" => "320 kbps",
            _ => "320 kbps",
        };
        format!("🎵 *Битрейт аудио:* {}", bitrate_display)
    } else {
        "".to_string()
    };

    let video_send_type = if send_as_document == 1 {
        "📎 *Отправка видео:* Документ"
    } else {
        "📹 *Отправка видео:* Медиа"
    };

    let audio_send_type = if send_audio_as_document == 1 {
        "📎 *Отправка аудио:* Документ"
    } else {
        "🎵 *Отправка аудио:* Медиа"
    };

    let plan_display = match plan.as_str() {
        "premium" => "Premium ⭐",
        "vip" => "VIP 💎",
        _ => "Free",
    };

    let mut text = format!(
        "🎬 *Твои настройки загрузки*\n\n\
        📥 *Формат:* {}\n",
        format_emoji
    );

    if !quality_line.is_empty() {
        text.push_str(&format!("{}\n", quality_line));
    }
    if !bitrate_line.is_empty() {
        text.push_str(&format!("{}\n", bitrate_line));
    }

    text.push_str(&format!(
        "{}\n\
        {}\n\n\
        💎 *Подписка:* {}\n\n\
        Чтобы изменить настройки, нажми \"⚙️ Настройки загрузки\" в главном меню\\.",
        video_send_type, audio_send_type, plan_display
    ));

    let keyboard = InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::callback(
        "🔙 Назад в меню".to_string(),
        "back:enhanced_main",
    )]]);

    edit_caption_or_text(bot, chat_id, message_id, text, Some(keyboard)).await
}

/// Shows help and FAQ information.
///
/// Displays common questions and answers about using the bot.
///
/// # Arguments
///
/// * `bot` - Telegram bot instance
/// * `chat_id` - User's chat ID
/// * `message_id` - ID of message to edit
async fn show_help_menu(bot: &Bot, chat_id: ChatId, message_id: MessageId) -> ResponseResult<()> {
    let admin_line = if ADMIN_USERNAME.is_empty() {
        "*Нужна помощь?*\nНапиши администратору.".to_string()
    } else {
        format!(
            "*Нужна помощь?*\nНапиши @{} \\(администратор\\)",
            escape_markdown(ADMIN_USERNAME.as_str())
        )
    };

    let text = format!(
        "❓ *Помощь и FAQ*\n\n\
        *Как пользоваться ботом?*\n\
        Просто отправь мне ссылку на видео или трек с YouTube, SoundCloud, VK, TikTok, Instagram или других сервисов\\.\n\n\
        *Какие форматы поддерживаются?*\n\
        🎵 MP3 \\- только аудио\n\
        🎬 MP4 \\- видео\n\
        🎬🎵 MP4 \\+ MP3 \\- и видео, и аудио\n\
        📝 SRT \\- субтитры\n\
        📄 TXT \\- текстовые субтитры\n\n\
        *Как изменить качество?*\n\
        Используй кнопку \"⚙️ Настройки загрузки\" в главном меню\\.\n\n\
        *Какие сервисы поддерживаются?*\n\
        YouTube, SoundCloud, VK, TikTok, Instagram, Twitch, Spotify и многие другие\\! Полный список в разделе \"🌐 Доступные сервисы\"\\.\n\n\
        {}",
        admin_line
    );

    let keyboard = InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::callback(
        "🔙 Назад в меню".to_string(),
        "back:enhanced_main",
    )]]);

    edit_caption_or_text(bot, chat_id, message_id, text.to_string(), Some(keyboard)).await
}

// ==================== Pure Helper Functions ====================
// These functions are extracted for testing purposes and may be used in production later

#[allow(dead_code)]
/// Formats video quality for display
fn format_video_quality_display(quality: &str) -> &'static str {
    match quality {
        "1080p" => "🎬 1080p",
        "720p" => "🎬 720p",
        "480p" => "🎬 480p",
        "360p" => "🎬 360p",
        _ => "🎬 Best",
    }
}

#[allow(dead_code)]
/// Formats audio bitrate for display
fn format_audio_bitrate_display(bitrate: &str) -> &'static str {
    match bitrate {
        "128k" => "128 kbps",
        "192k" => "192 kbps",
        "256k" => "256 kbps",
        "320k" => "320 kbps",
        _ => "320 kbps",
    }
}

#[allow(dead_code)]
/// Formats download format for display
fn format_download_format_display(format: &str) -> &'static str {
    match format {
        "mp3" => "🎵 MP3",
        "mp4" => "🎬 MP4",
        "mp4+mp3" => "🎬🎵 MP4 + MP3",
        "srt" => "📝 SRT",
        "txt" => "📄 TXT",
        _ => "🎵 MP3",
    }
}

#[allow(dead_code)]
/// Formats subscription plan for display
fn format_plan_display(plan: &str) -> &'static str {
    match plan {
        "vip" => "💎 VIP",
        "premium" => "⭐ Premium",
        _ => "🆓 Free",
    }
}

#[allow(dead_code)]
/// Builds a format callback string with optional preview context
fn build_format_callback(format: &str, url_id: Option<&str>, preview_msg_id: Option<i32>) -> String {
    match (url_id, preview_msg_id) {
        (Some(id), Some(msg_id)) => format!("format:{}:preview:{}:{}", format, id, msg_id),
        (Some(id), None) => format!("format:{}:preview:{}", format, id),
        _ => format!("format:{}", format),
    }
}

#[allow(dead_code)]
/// Builds a back callback string with optional preview context
fn build_back_callback(url_id: Option<&str>, preview_msg_id: Option<i32>) -> String {
    match (url_id, preview_msg_id) {
        (Some(id), Some(msg_id)) => format!("back:preview:{}:{}", id, msg_id),
        (Some(id), None) => format!("back:preview:{}", id),
        _ => "back:main".to_string(),
    }
}

#[allow(dead_code)]
/// Builds a mode callback string with optional preview context
fn build_mode_callback(mode: &str, url_id: Option<&str>) -> String {
    match url_id {
        Some(id) => format!("mode:{}:preview:{}", mode, id),
        None => format!("mode:{}", mode),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ==================== escape_markdown Tests ====================

    #[test]
    fn test_escape_markdown_basic() {
        assert_eq!(escape_markdown("hello"), "hello");
        assert_eq!(escape_markdown("hello world"), "hello world");
    }

    #[test]
    fn test_escape_markdown_special_chars() {
        assert_eq!(escape_markdown("hello_world"), "hello\\_world");
        assert_eq!(escape_markdown("hello*world"), "hello\\*world");
        assert_eq!(escape_markdown("hello.world"), "hello\\.world");
    }

    #[test]
    fn test_escape_markdown_brackets() {
        assert_eq!(escape_markdown("(test)"), "\\(test\\)");
        assert_eq!(escape_markdown("[test]"), "\\[test\\]");
        assert_eq!(escape_markdown("{test}"), "\\{test\\}");
    }

    #[test]
    fn test_escape_markdown_all_special() {
        let input = r"_*[]()~`>#+-=|{}.!";
        let expected = r"\_\*\[\]\(\)\~\`\>\#\+\-\=\|\{\}\.\!";
        assert_eq!(escape_markdown(input), expected);
    }

    #[test]
    fn test_escape_markdown_empty() {
        assert_eq!(escape_markdown(""), "");
    }

    #[test]
    fn test_escape_markdown_cyrillic() {
        assert_eq!(escape_markdown("Привет мир!"), "Привет мир\\!");
    }

    // ==================== Format Display Functions ====================

    #[test]
    fn test_format_video_quality_display() {
        assert_eq!(format_video_quality_display("1080p"), "🎬 1080p");
        assert_eq!(format_video_quality_display("720p"), "🎬 720p");
        assert_eq!(format_video_quality_display("480p"), "🎬 480p");
        assert_eq!(format_video_quality_display("360p"), "🎬 360p");
        assert_eq!(format_video_quality_display("best"), "🎬 Best");
        assert_eq!(format_video_quality_display("unknown"), "🎬 Best");
    }

    #[test]
    fn test_format_audio_bitrate_display() {
        assert_eq!(format_audio_bitrate_display("128k"), "128 kbps");
        assert_eq!(format_audio_bitrate_display("192k"), "192 kbps");
        assert_eq!(format_audio_bitrate_display("256k"), "256 kbps");
        assert_eq!(format_audio_bitrate_display("320k"), "320 kbps");
        assert_eq!(format_audio_bitrate_display("unknown"), "320 kbps");
    }

    #[test]
    fn test_format_download_format_display() {
        assert_eq!(format_download_format_display("mp3"), "🎵 MP3");
        assert_eq!(format_download_format_display("mp4"), "🎬 MP4");
        assert_eq!(format_download_format_display("mp4+mp3"), "🎬🎵 MP4 + MP3");
        assert_eq!(format_download_format_display("srt"), "📝 SRT");
        assert_eq!(format_download_format_display("txt"), "📄 TXT");
        assert_eq!(format_download_format_display("unknown"), "🎵 MP3");
    }

    #[test]
    fn test_format_plan_display() {
        assert_eq!(format_plan_display("vip"), "💎 VIP");
        assert_eq!(format_plan_display("premium"), "⭐ Premium");
        assert_eq!(format_plan_display("free"), "🆓 Free");
        assert_eq!(format_plan_display("unknown"), "🆓 Free");
    }

    // ==================== Callback Builders ====================

    #[test]
    fn test_build_format_callback_simple() {
        assert_eq!(build_format_callback("mp3", None, None), "format:mp3");
        assert_eq!(build_format_callback("mp4", None, None), "format:mp4");
    }

    #[test]
    fn test_build_format_callback_with_preview() {
        assert_eq!(
            build_format_callback("mp3", Some("url123"), None),
            "format:mp3:preview:url123"
        );
    }

    #[test]
    fn test_build_format_callback_with_preview_and_msg() {
        assert_eq!(
            build_format_callback("mp4", Some("url123"), Some(456)),
            "format:mp4:preview:url123:456"
        );
    }

    #[test]
    fn test_build_back_callback_simple() {
        assert_eq!(build_back_callback(None, None), "back:main");
    }

    #[test]
    fn test_build_back_callback_with_preview() {
        assert_eq!(build_back_callback(Some("url123"), None), "back:preview:url123");
        assert_eq!(
            build_back_callback(Some("url123"), Some(789)),
            "back:preview:url123:789"
        );
    }

    #[test]
    fn test_build_mode_callback_simple() {
        assert_eq!(build_mode_callback("video_quality", None), "mode:video_quality");
        assert_eq!(build_mode_callback("audio_bitrate", None), "mode:audio_bitrate");
    }

    #[test]
    fn test_build_mode_callback_with_preview() {
        assert_eq!(
            build_mode_callback("video_quality", Some("url123")),
            "mode:video_quality:preview:url123"
        );
    }

    // ==================== create_audio_effects_keyboard Tests ====================

    #[test]
    fn test_create_audio_effects_keyboard_default_values() {
        use crate::download::audio_effects::MorphProfile;
        let keyboard = create_audio_effects_keyboard("session123", 0, 1.0, 0, MorphProfile::None);

        // Keyboard should have 9 rows (2 pitch + 2 tempo + 2 bass + 1 morph + 1 action + 1 skip)
        assert_eq!(keyboard.inline_keyboard.len(), 9);
    }

    #[test]
    fn test_create_audio_effects_keyboard_with_changes() {
        use crate::download::audio_effects::MorphProfile;
        let keyboard = create_audio_effects_keyboard("session456", 2, 1.5, 3, MorphProfile::Lofi);

        // Verify the keyboard is created correctly
        assert!(!keyboard.inline_keyboard.is_empty());

        // Find the morph row (row 6, 0-indexed)
        let morph_row = &keyboard.inline_keyboard[6];
        let morph_button = &morph_row[0];
        // LoFi profile should show "LoFi" in the button text
        assert!(
            morph_button.text.contains("LoFi"),
            "Morph button: {}",
            morph_button.text
        );
    }

    #[test]
    fn test_create_audio_effects_keyboard_action_row() {
        use crate::download::audio_effects::MorphProfile;
        let keyboard = create_audio_effects_keyboard("test_id", 0, 1.0, 0, MorphProfile::None);

        // Action row (row 7, 0-indexed) should have Apply and Reset buttons
        let action_row = &keyboard.inline_keyboard[7];
        assert!(action_row[0].text.contains("Apply"), "Button: {}", action_row[0].text);
    }

    #[test]
    fn test_create_audio_effects_keyboard_skip_row() {
        use crate::download::audio_effects::MorphProfile;
        let keyboard = create_audio_effects_keyboard("test_id", 0, 1.0, 0, MorphProfile::None);

        // Skip row should be the last row (row 8, 0-indexed)
        let skip_row = &keyboard.inline_keyboard[8];
        assert!(skip_row[0].text.contains("Skip"), "Button: {}", skip_row[0].text);
    }

    // ==================== build_enhanced_menu Tests ====================

    #[test]
    fn test_build_enhanced_menu_returns_keyboard() {
        let lang = i18n::lang_from_code("en");
        let (text, keyboard) = build_enhanced_menu(&lang, "🎵 MP3", "🎬 720p / 🎵 320 kbps", "⭐ Premium");

        // Text should not be empty
        assert!(!text.is_empty());

        // Keyboard should have 4 rows
        assert_eq!(keyboard.inline_keyboard.len(), 4);
    }

    #[test]
    fn test_build_enhanced_menu_keyboard_structure() {
        let lang = i18n::lang_from_code("ru");
        let (_, keyboard) = build_enhanced_menu(&lang, "🎵 MP3", "🎬 Best / 🎵 320 kbps", "🆓 Free");

        // First row: Settings + Current
        assert_eq!(keyboard.inline_keyboard[0].len(), 2);
        // Second row: Stats + History
        assert_eq!(keyboard.inline_keyboard[1].len(), 2);
        // Third row: Services + Subscription
        assert_eq!(keyboard.inline_keyboard[2].len(), 2);
        // Fourth row: Language + Feedback
        assert_eq!(keyboard.inline_keyboard[3].len(), 2);
    }
}
