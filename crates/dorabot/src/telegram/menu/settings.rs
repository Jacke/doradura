use crate::core::escape_markdown;
use crate::i18n;
use crate::storage::{DbPool, SharedStorage};
use crate::telegram::Bot;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardMarkup, MessageId};

use super::helpers::edit_caption_or_text;

/// Label for the quality-preset cycle button. Maps the stored preset string
/// to a localised button label including the icon and rough VMAF tier.
fn preset_button_label(lang: &unic_langid::LanguageIdentifier, preset: &str) -> String {
    let key = match preset {
        "balanced" => "menu.quality_preset_balanced",
        "transparent" => "menu.quality_preset_transparent",
        "lossless" => "menu.quality_preset_lossless",
        _ => "menu.quality_preset_master", // default + unknown values land here
    };
    i18n::t(lang, key)
}

/// Cycle order for the preset button. Click moves to the next preset.
pub fn next_preset(current: &str) -> &'static str {
    match current {
        "balanced" => "transparent",
        "transparent" => "master",
        "master" => "lossless",
        "lossless" => "balanced",
        _ => "master",
    }
}

/// Pop-up alert text shown when the user switches preset. Surfaces the
/// trade-off so users opting into Master/Lossless aren't surprised by the
/// long encode time.
pub fn preset_alert_text(lang: &unic_langid::LanguageIdentifier, preset: &str) -> String {
    let key = match preset {
        "balanced" => "menu.quality_preset_alert_balanced",
        "transparent" => "menu.quality_preset_alert_transparent",
        "lossless" => "menu.quality_preset_alert_lossless",
        _ => "menu.quality_preset_alert_master",
    };
    i18n::t(lang, key)
}

/// Shows the download type menu.
///
/// Displays available formats (MP3, MP4, SRT, TXT) and marks the current choice.
pub async fn show_download_type_menu(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
    url_id: Option<&str>,
    preview_msg_id: Option<MessageId>,
) -> ResponseResult<()> {
    let _ = db_pool;
    let current_format = shared_storage
        .get_user(chat_id.0)
        .await
        .ok()
        .flatten()
        .map(|user| user.download_format)
        .unwrap_or_else(|| "mp3".to_string());
    let lang = i18n::user_lang_from_storage(&shared_storage, chat_id.0).await;

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
            crate::telegram::cb(
                if current_format == "mp3" {
                    "🎵 MP3 ✓"
                } else {
                    "🎵 MP3"
                }
                .to_string(),
                format_callback("mp3"),
            ),
            crate::telegram::cb(
                if current_format == "mp4" {
                    "🎬 MP4 ✓"
                } else {
                    "🎬 MP4"
                }
                .to_string(),
                format_callback("mp4"),
            ),
        ],
        vec![crate::telegram::cb(
            if current_format == "mp4+mp3" {
                "🎬🎵 MP4 + MP3 ✓"
            } else {
                "🎬🎵 MP4 + MP3"
            }
            .to_string(),
            format_callback("mp4+mp3"),
        )],
        vec![
            crate::telegram::cb(
                if current_format == "srt" {
                    "📝 SRT ✓"
                } else {
                    "📝 SRT"
                }
                .to_string(),
                format_callback("srt"),
            ),
            crate::telegram::cb(
                if current_format == "txt" {
                    "📄 TXT ✓"
                } else {
                    "📄 TXT"
                }
                .to_string(),
                format_callback("txt"),
            ),
        ],
        vec![crate::telegram::cb(i18n::t(&lang, "common.back"), back_callback)],
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
    let args = doracore::fluent_args!("format" => escaped_format.clone());
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
pub async fn send_download_type_menu_as_new(
    bot: &Bot,
    chat_id: ChatId,
    db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
    url_id: Option<&str>,
    preview_msg_id: Option<MessageId>,
) -> ResponseResult<()> {
    let _ = db_pool;
    let current_format = shared_storage
        .get_user(chat_id.0)
        .await
        .ok()
        .flatten()
        .map(|user| user.download_format)
        .unwrap_or_else(|| "mp3".to_string());
    let lang = i18n::user_lang_from_storage(&shared_storage, chat_id.0).await;

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
            crate::telegram::cb(
                if current_format == "mp3" {
                    "🎵 MP3 ✓"
                } else {
                    "🎵 MP3"
                }
                .to_string(),
                format_callback("mp3"),
            ),
            crate::telegram::cb(
                if current_format == "mp4" {
                    "🎬 MP4 ✓"
                } else {
                    "🎬 MP4"
                }
                .to_string(),
                format_callback("mp4"),
            ),
        ],
        vec![crate::telegram::cb(
            if current_format == "mp4+mp3" {
                "🎬🎵 MP4 + MP3 ✓"
            } else {
                "🎬🎵 MP4 + MP3"
            }
            .to_string(),
            format_callback("mp4+mp3"),
        )],
        vec![
            crate::telegram::cb(
                if current_format == "srt" {
                    "📝 SRT ✓"
                } else {
                    "📝 SRT"
                }
                .to_string(),
                format_callback("srt"),
            ),
            crate::telegram::cb(
                if current_format == "txt" {
                    "📄 TXT ✓"
                } else {
                    "📄 TXT"
                }
                .to_string(),
                format_callback("txt"),
            ),
        ],
        vec![crate::telegram::cb(i18n::t(&lang, "common.back"), back_callback)],
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
    let args = doracore::fluent_args!("format" => escaped_format.clone());
    bot.send_message(chat_id, i18n::t_args(&lang, "menu.download_type_title", &args))
        .parse_mode(teloxide::types::ParseMode::MarkdownV2)
        .reply_markup(keyboard)
        .await?;
    Ok(())
}

/// Shows the video quality selection menu.
///
/// Displays available qualities (1080p, 720p, 480p, 360p, best) and marks the current choice.
pub async fn show_video_quality_menu(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
    url_id: Option<&str>,
) -> ResponseResult<()> {
    let _ = db_pool;
    let current_quality = shared_storage
        .get_user_video_quality(chat_id.0)
        .await
        .unwrap_or_else(|_| "best".to_string());
    let send_as_document = shared_storage.get_user_send_as_document(chat_id.0).await.unwrap_or(0);
    let quality_preset = shared_storage
        .get_user_video_quality_preset(chat_id.0)
        .await
        .unwrap_or_else(|_| "master".to_string());
    let experimental = shared_storage
        .get_user_experimental_features(chat_id.0)
        .await
        .unwrap_or(false);
    let download_subs = shared_storage
        .get_user_download_subtitles(chat_id.0)
        .await
        .unwrap_or(false);
    let burn_subs = shared_storage.get_user_burn_subtitles(chat_id.0).await.unwrap_or(false);
    let no_caption = shared_storage
        .get_user_video_no_caption(chat_id.0)
        .await
        .unwrap_or(false);
    let lang = i18n::user_lang_from_storage(&shared_storage, chat_id.0).await;

    // 4320p (8K) intentionally hidden — Railway workers can't reliably
    // recode 8K AV1 → H.264 (yt-dlp postprocessor SIGKILL'd at frame=0).
    // Re-enable once we have a smart fallback in the download pipeline
    // that drops to 2160p when 4320p postprocessing fails.
    let mut keyboard_rows = vec![
        vec![crate::telegram::cb(
            if current_quality == "2160p" {
                "🎬 4K (2160p) ✓"
            } else {
                "🎬 4K (2160p)"
            }
            .to_string(),
            "quality:2160p",
        )],
        vec![crate::telegram::cb(
            if current_quality == "1440p" {
                "🎬 2K (1440p) ✓"
            } else {
                "🎬 2K (1440p)"
            }
            .to_string(),
            "quality:1440p",
        )],
        vec![
            crate::telegram::cb(
                if current_quality == "1080p" {
                    "🎬 1080p (Full HD) ✓"
                } else {
                    "🎬 1080p (Full HD)"
                }
                .to_string(),
                "quality:1080p",
            ),
            crate::telegram::cb(
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
            crate::telegram::cb(
                if current_quality == "480p" {
                    "🎬 480p (SD) ✓"
                } else {
                    "🎬 480p (SD)"
                }
                .to_string(),
                "quality:480p",
            ),
            crate::telegram::cb(
                if current_quality == "360p" {
                    "🎬 360p (Low) ✓"
                } else {
                    "🎬 360p (Low)"
                }
                .to_string(),
                "quality:360p",
            ),
        ],
        vec![crate::telegram::cb(
            if current_quality == "best" {
                "🎬 Best (Auto) ✓"
            } else {
                "🎬 Best (Auto)"
            }
            .to_string(),
            "quality:best",
        )],
        vec![crate::telegram::cb(
            if send_as_document == 0 {
                i18n::t(&lang, "menu.send_video_media")
            } else {
                i18n::t(&lang, "menu.send_video_document")
            },
            "send_type:toggle",
        )],
        vec![crate::telegram::cb(
            preset_button_label(&lang, &quality_preset),
            "qpreset:cycle",
        )],
        vec![crate::telegram::cb(
            if experimental {
                i18n::t(&lang, "menu.experimental_on")
            } else {
                i18n::t(&lang, "menu.experimental_off")
            },
            "experimental:toggle",
        )],
    ];

    // Add burn_subtitles button only if download_subtitles is enabled
    if download_subs {
        let status = if burn_subs {
            i18n::t(&lang, "menu.burn_subtitles_on")
        } else {
            i18n::t(&lang, "menu.burn_subtitles_off")
        };
        let burn_args = doracore::fluent_args!("status" => status);

        keyboard_rows.push(vec![crate::telegram::cb(
            i18n::t_args(&lang, "menu.burn_subtitles_button", &burn_args),
            "video:toggle_burn_subs",
        )]);
    }

    // Video caption toggle (suppress `*artist* — _title_` under sent video).
    {
        let status = if no_caption {
            i18n::t(&lang, "menu.video_no_caption_on")
        } else {
            i18n::t(&lang, "menu.video_no_caption_off")
        };
        let args = doracore::fluent_args!("status" => status);
        keyboard_rows.push(vec![crate::telegram::cb(
            i18n::t_args(&lang, "menu.video_no_caption_button", &args),
            "video:toggle_no_caption",
        )]);
    }

    keyboard_rows.push(vec![crate::telegram::cb(
        i18n::t(&lang, "common.back"),
        url_id.map_or_else(|| "back:main".to_string(), |id| format!("back:main:preview:{}", id)),
    )]);

    let keyboard = InlineKeyboardMarkup::new(keyboard_rows);

    let quality_display = match current_quality.as_str() {
        "4320p" => "🎬 8K (4320p)",
        "2160p" => "🎬 4K (2160p)",
        "1440p" => "🎬 2K (1440p)",
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
    let args = doracore::fluent_args!("quality" => escaped_quality.clone(), "send_type" => escaped_send_type.clone());
    let mut body = i18n::t_args(&lang, "menu.video_quality_title", &args);
    if matches!(current_quality.as_str(), "4320p" | "2160p" | "1440p") {
        body.push_str("\n\n");
        body.push_str(&i18n::t(&lang, "menu.warning_highres"));
    }
    edit_caption_or_text(bot, chat_id, message_id, body, Some(keyboard)).await?;
    Ok(())
}

/// Shows the audio bitrate selection menu.
///
/// Displays available bitrates (128kbps, 192kbps, 256kbps, 320kbps) and marks the current choice.
pub async fn show_audio_bitrate_menu(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
    url_id: Option<&str>,
) -> ResponseResult<()> {
    let _ = db_pool;
    let current_bitrate = shared_storage
        .get_user_audio_bitrate(chat_id.0)
        .await
        .unwrap_or_else(|_| "320k".to_string());
    let send_audio_as_document = shared_storage
        .get_user_send_audio_as_document(chat_id.0)
        .await
        .unwrap_or(0);
    let lang = i18n::user_lang_from_storage(&shared_storage, chat_id.0).await;

    let keyboard = InlineKeyboardMarkup::new(vec![
        vec![
            crate::telegram::cb(
                if current_bitrate == "128k" {
                    "🎵 128 kbps ✓"
                } else {
                    "🎵 128 kbps"
                }
                .to_string(),
                "bitrate:128k",
            ),
            crate::telegram::cb(
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
            crate::telegram::cb(
                if current_bitrate == "256k" {
                    "🎵 256 kbps ✓"
                } else {
                    "🎵 256 kbps"
                }
                .to_string(),
                "bitrate:256k",
            ),
            crate::telegram::cb(
                if current_bitrate == "320k" {
                    "🎵 320 kbps ✓"
                } else {
                    "🎵 320 kbps"
                }
                .to_string(),
                "bitrate:320k",
            ),
        ],
        vec![crate::telegram::cb(
            if send_audio_as_document == 0 {
                i18n::t(&lang, "menu.send_audio_media")
            } else {
                i18n::t(&lang, "menu.send_audio_document")
            },
            "audio_send_type:toggle",
        )],
        vec![crate::telegram::cb(
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
    let args = doracore::fluent_args!("bitrate" => escaped_bitrate.clone(), "send_type" => escaped_send_type.clone());

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

/// Shows the language selection menu.
pub async fn show_language_menu(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
    url_id: Option<&str>,
) -> ResponseResult<()> {
    let _ = db_pool;
    let current_lang_code = shared_storage
        .get_user_language(chat_id.0)
        .await
        .unwrap_or_else(|_| "ru".to_string());
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
        buttons.push(vec![crate::telegram::cb(label, callback)]);
    }

    buttons.push(vec![crate::telegram::cb(
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
        buttons.push(vec![crate::telegram::cb(label, callback)]);
    }

    let keyboard = InlineKeyboardMarkup::new(buttons);
    bot.send_message(chat_id, i18n::t(&lang, "menu.welcome_new_user"))
        .reply_markup(keyboard)
        .parse_mode(teloxide::types::ParseMode::MarkdownV2)
        .await
}

/// Shows the subtitle style settings menu.
///
/// Displays cycle-through buttons for font size, text color, outline, shadow, and position.
pub async fn show_subtitle_style_menu(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
) -> ResponseResult<()> {
    let _ = db_pool;
    let style = shared_storage
        .get_user_subtitle_style(chat_id.0)
        .await
        .unwrap_or_default();
    let lang = i18n::user_lang_from_storage(&shared_storage, chat_id.0).await;

    let size_label = match style.font_size.as_str() {
        "small" => i18n::t(&lang, "menu.subtitle_size_small"),
        "medium" => i18n::t(&lang, "menu.subtitle_size_medium"),
        "large" => i18n::t(&lang, "menu.subtitle_size_large"),
        "xlarge" => i18n::t(&lang, "menu.subtitle_size_xlarge"),
        _ => i18n::t(&lang, "menu.subtitle_size_medium"),
    };

    let color_label = match style.text_color.as_str() {
        "white" => i18n::t(&lang, "menu.subtitle_color_white"),
        "yellow" => i18n::t(&lang, "menu.subtitle_color_yellow"),
        "cyan" => i18n::t(&lang, "menu.subtitle_color_cyan"),
        "green" => i18n::t(&lang, "menu.subtitle_color_green"),
        _ => i18n::t(&lang, "menu.subtitle_color_white"),
    };

    let outline_label = match style.outline_color.as_str() {
        "black" => i18n::t(&lang, "menu.subtitle_outline_black"),
        "dark_gray" => i18n::t(&lang, "menu.subtitle_outline_dark_gray"),
        "none" => i18n::t(&lang, "menu.subtitle_outline_none"),
        _ => i18n::t(&lang, "menu.subtitle_outline_black"),
    };

    let position_label = match style.position.as_str() {
        "bottom" => i18n::t(&lang, "menu.subtitle_pos_bottom"),
        "top" => i18n::t(&lang, "menu.subtitle_pos_top"),
        _ => i18n::t(&lang, "menu.subtitle_pos_bottom"),
    };

    let size_args = doracore::fluent_args!("value" => size_label);
    let color_args = doracore::fluent_args!("value" => color_label);
    let outline_args = doracore::fluent_args!("value" => outline_label);
    let width_args = doracore::fluent_args!("value" => format!("{}", style.outline_width));
    let shadow_args = doracore::fluent_args!("value" => format!("{}", style.shadow));
    let pos_args = doracore::fluent_args!("value" => position_label);

    let keyboard = InlineKeyboardMarkup::new(vec![
        vec![crate::telegram::cb(
            i18n::t_args(&lang, "menu.subtitle_font_size", &size_args),
            "subtitle:font_size",
        )],
        vec![crate::telegram::cb(
            i18n::t_args(&lang, "menu.subtitle_text_color", &color_args),
            "subtitle:text_color",
        )],
        vec![crate::telegram::cb(
            i18n::t_args(&lang, "menu.subtitle_outline_color", &outline_args),
            "subtitle:outline_color",
        )],
        vec![crate::telegram::cb(
            i18n::t_args(&lang, "menu.subtitle_outline_width", &width_args),
            "subtitle:outline_width",
        )],
        vec![crate::telegram::cb(
            i18n::t_args(&lang, "menu.subtitle_shadow", &shadow_args),
            "subtitle:shadow",
        )],
        vec![crate::telegram::cb(
            i18n::t_args(&lang, "menu.subtitle_position", &pos_args),
            "subtitle:position",
        )],
        vec![crate::telegram::cb(i18n::t(&lang, "common.back"), "back:main")],
    ]);

    edit_caption_or_text(
        bot,
        chat_id,
        message_id,
        i18n::t(&lang, "menu.subtitle_style_title"),
        Some(keyboard),
    )
    .await?;
    Ok(())
}

/// Shows the progress bar style selection menu.
///
/// Displays 8 progress bar styles with previews and marks the current selection.
pub async fn show_progress_bar_style_menu(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
) -> ResponseResult<()> {
    use crate::download::progress::ProgressBarStyle;

    let _ = db_pool;
    let current_style = shared_storage
        .get_user_progress_bar_style(chat_id.0)
        .await
        .unwrap_or_else(|_| "classic".to_string());
    let lang = i18n::user_lang_from_storage(&shared_storage, chat_id.0).await;

    let mut buttons = Vec::new();
    for style in ProgressBarStyle::all() {
        let is_selected = style.as_str() == current_style;
        let label = if is_selected {
            format!("{} {} \u{2713}", style.display_name(), style.preview())
        } else {
            format!("{} {}", style.display_name(), style.preview())
        };
        buttons.push(vec![crate::telegram::cb(
            label,
            format!("pbar_style:{}", style.as_str()),
        )]);
    }
    buttons.push(vec![crate::telegram::cb(
        i18n::t(&lang, "common.back"),
        "back:main".to_string(),
    )]);

    let keyboard = InlineKeyboardMarkup::new(buttons);
    edit_caption_or_text(
        bot,
        chat_id,
        message_id,
        "\u{1f3a8} Choose progress bar style:".to_string(),
        Some(keyboard),
    )
    .await?;
    Ok(())
}
