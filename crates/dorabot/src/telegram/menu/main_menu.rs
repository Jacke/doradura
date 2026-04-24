use crate::core::config::admin::ADMIN_USERNAME;
use crate::core::escape_markdown;
use crate::core::types::Plan;
use crate::i18n;
use crate::storage::db::DbPool;
use crate::storage::SharedStorage;
use crate::telegram::Bot;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardMarkup, MessageId};
use unic_langid::LanguageIdentifier;

use super::helpers::edit_caption_or_text;

// Experimental features graduated to main workflow — experimental_button_row removed.
async fn load_menu_user_state(shared_storage: &SharedStorage, chat_id: ChatId) -> (String, String, String, Plan) {
    let user = shared_storage.get_user(chat_id.0).await.ok().flatten();
    let format = user
        .as_ref()
        .map(|user| user.download_format.clone())
        .unwrap_or_else(|| "mp3".to_string());
    let video_quality = user
        .as_ref()
        .map(|user| user.video_quality.clone())
        .unwrap_or_else(|| "best".to_string());
    let audio_bitrate = user
        .as_ref()
        .map(|user| user.audio_bitrate.clone())
        .unwrap_or_else(|| "320k".to_string());
    let plan = user.map(|user| user.plan).unwrap_or_default();
    (format, video_quality, audio_bitrate, plan)
}

/// Shows the main settings menu for the download mode.
///
/// Displays inline buttons for video quality, audio bitrate, and supported services.
pub async fn show_main_menu(
    bot: &Bot,
    chat_id: ChatId,
    _db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
) -> ResponseResult<Message> {
    let (_, video_quality, audio_bitrate, _) = load_menu_user_state(&shared_storage, chat_id).await;
    let lang = i18n::user_lang_from_storage(&shared_storage, chat_id.0).await;

    let quality_emoji = match video_quality.as_str() {
        "4320p" => "🎬 8K",
        "2160p" => "🎬 4K",
        "1440p" => "🎬 2K",
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

    let quality_args = doracore::fluent_args!("quality" => quality_emoji);
    let bitrate_args = doracore::fluent_args!("bitrate" => bitrate_display);

    let keyboard = InlineKeyboardMarkup::new(vec![
        vec![crate::telegram::cb(
            i18n::t_args(&lang, "menu.video_quality_button", &quality_args),
            "mode:video_quality",
        )],
        vec![crate::telegram::cb(
            i18n::t_args(&lang, "menu.audio_bitrate_button", &bitrate_args),
            "mode:audio_bitrate",
        )],
        vec![crate::telegram::cb(
            i18n::t(&lang, "menu.services_button"),
            "mode:services",
        )],
        vec![crate::telegram::cb(
            i18n::t(&lang, "menu.subscription_button"),
            "mode:subscription",
        )],
        vec![crate::telegram::cb(
            i18n::t(&lang, "menu.subtitle_style_button"),
            "mode:subtitle_style",
        )],
        vec![crate::telegram::cb(
            i18n::t(&lang, "menu.progress_bar_style_button"),
            "mode:progress_bar_style",
        )],
        vec![crate::telegram::cb(
            i18n::t(&lang, "menu.language_button"),
            "mode:language",
        )],
    ]);

    bot.send_message(chat_id, i18n::t(&lang, "menu.title"))
        .parse_mode(teloxide::types::ParseMode::MarkdownV2)
        .reply_markup(keyboard)
        .await
}

pub(crate) fn build_enhanced_menu(
    lang: &LanguageIdentifier,
    format_emoji: &str,
    quality_line: &str,
    plan_display: &str,
) -> (String, InlineKeyboardMarkup) {
    let args = doracore::fluent_args!("format" => format_emoji, "quality" => quality_line, "plan" => plan_display);

    let text = i18n::t_args(lang, "menu.enhanced_text", &args);

    let keyboard = InlineKeyboardMarkup::new(vec![
        vec![
            crate::telegram::cb(i18n::t(lang, "menu.button_settings"), "main:settings"),
            crate::telegram::cb(i18n::t(lang, "menu.button_current"), "main:current"),
        ],
        vec![
            crate::telegram::cb(i18n::t(lang, "menu.button_stats"), "main:stats"),
            crate::telegram::cb(i18n::t(lang, "menu.button_history"), "main:history"),
        ],
        vec![
            crate::telegram::cb(i18n::t(lang, "menu.services_button"), "main:services"),
            crate::telegram::cb(i18n::t(lang, "menu.button_subscription"), "main:subscription"),
        ],
        vec![
            crate::telegram::cb(i18n::t(lang, "menu.language_button"), "mode:language"),
            crate::telegram::cb("\u{1f5c4} Vault", "vault:menu"),
        ],
        vec![crate::telegram::cb(
            i18n::t(lang, "menu.button_feedback"),
            "main:feedback",
        )],
    ]);

    (text, keyboard)
}

// Edit message to show main menu (for callbacks that need to edit existing message)
pub(crate) async fn edit_main_menu(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    _db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
    url_id: Option<&str>,
    _preview_msg_id: Option<MessageId>,
) -> ResponseResult<()> {
    let (_, video_quality, audio_bitrate, _) = load_menu_user_state(&shared_storage, chat_id).await;
    let lang = i18n::user_lang_from_storage(&shared_storage, chat_id.0).await;

    let quality_emoji = match video_quality.as_str() {
        "4320p" => "🎬 8K",
        "2160p" => "🎬 4K",
        "1440p" => "🎬 2K",
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

    let mode_callback = |mode: &str| {
        if let Some(id) = url_id {
            format!("mode:{}:preview:{}", mode, id)
        } else {
            format!("mode:{}", mode)
        }
    };

    let quality_args = doracore::fluent_args!("quality" => quality_emoji);
    let bitrate_args = doracore::fluent_args!("bitrate" => bitrate_display);

    let mut keyboard_rows = vec![
        vec![crate::telegram::cb(
            i18n::t_args(&lang, "menu.video_quality_button", &quality_args),
            mode_callback("video_quality"),
        )],
        vec![crate::telegram::cb(
            i18n::t_args(&lang, "menu.audio_bitrate_button", &bitrate_args),
            mode_callback("audio_bitrate"),
        )],
        vec![crate::telegram::cb(
            i18n::t(&lang, "menu.services_button"),
            mode_callback("services"),
        )],
        vec![crate::telegram::cb(
            i18n::t(&lang, "menu.subscription_button"),
            mode_callback("subscription"),
        )],
        vec![crate::telegram::cb(
            i18n::t(&lang, "menu.subtitle_style_button"),
            mode_callback("subtitle_style"),
        )],
        vec![crate::telegram::cb(
            i18n::t(&lang, "menu.progress_bar_style_button"),
            mode_callback("progress_bar_style"),
        )],
        vec![crate::telegram::cb(
            i18n::t(&lang, "menu.language_button"),
            mode_callback("language"),
        )],
    ];

    // Add a Back button when the menu is opened from preview
    if let Some(id) = url_id {
        keyboard_rows.push(vec![crate::telegram::cb(
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
pub async fn send_main_menu_as_new(
    bot: &Bot,
    chat_id: ChatId,
    _db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
    url_id: Option<&str>,
    preview_msg_id: Option<MessageId>,
) -> ResponseResult<()> {
    let (_, video_quality, audio_bitrate, _) = load_menu_user_state(&shared_storage, chat_id).await;
    let lang = i18n::user_lang_from_storage(&shared_storage, chat_id.0).await;

    let quality_emoji = match video_quality.as_str() {
        "4320p" => "🎬 8K",
        "2160p" => "🎬 4K",
        "1440p" => "🎬 2K",
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

    let quality_args = doracore::fluent_args!("quality" => quality_emoji);
    let bitrate_args = doracore::fluent_args!("bitrate" => bitrate_display);

    let mut keyboard_rows = vec![
        vec![crate::telegram::cb(
            i18n::t_args(&lang, "menu.video_quality_button", &quality_args),
            mode_callback("video_quality"),
        )],
        vec![crate::telegram::cb(
            i18n::t_args(&lang, "menu.audio_bitrate_button", &bitrate_args),
            mode_callback("audio_bitrate"),
        )],
        vec![crate::telegram::cb(
            i18n::t(&lang, "menu.services_button"),
            mode_callback("services"),
        )],
        vec![crate::telegram::cb(
            i18n::t(&lang, "menu.subscription_button"),
            mode_callback("subscription"),
        )],
        vec![crate::telegram::cb(
            i18n::t(&lang, "menu.subtitle_style_button"),
            mode_callback("subtitle_style"),
        )],
        vec![crate::telegram::cb(
            i18n::t(&lang, "menu.progress_bar_style_button"),
            mode_callback("progress_bar_style"),
        )],
        vec![crate::telegram::cb(
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
        keyboard_rows.push(vec![crate::telegram::cb(
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

/// Shows the enhanced main menu with user's current settings and main action buttons.
///
/// This is the improved main menu that replaces the old /start handler.
pub async fn show_enhanced_main_menu(
    bot: &Bot,
    chat_id: ChatId,
    _db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
) -> ResponseResult<Message> {
    let lang = i18n::user_lang_from_storage(&shared_storage, chat_id.0).await;
    let (format, video_quality, audio_bitrate, plan) = load_menu_user_state(&shared_storage, chat_id).await;

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
            "4320p" => "8K",
            "2160p" => "4K",
            "1440p" => "2K",
            "1080p" => "1080p",
            "720p" => "720p",
            "480p" => "480p",
            "360p" => "360p",
            _ => "Best",
        };
        let args = doracore::fluent_args!("value" => quality_display);
        i18n::t_args(&lang, "menu.quality_line", &args)
    } else {
        let bitrate_display = match audio_bitrate.as_str() {
            "128k" => "128 kbps",
            "192k" => "192 kbps",
            "256k" => "256 kbps",
            "320k" => "320 kbps",
            _ => "320 kbps",
        };
        let args = doracore::fluent_args!("value" => bitrate_display);
        i18n::t_args(&lang, "menu.bitrate_line", &args)
    };

    // Plan display
    let plan_display = match plan {
        Plan::Premium => i18n::t(&lang, "menu.plan_premium"),
        Plan::Vip => i18n::t(&lang, "menu.plan_vip"),
        Plan::Free => i18n::t(&lang, "menu.plan_free"),
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
pub(crate) async fn edit_enhanced_main_menu(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    _db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
) -> ResponseResult<()> {
    let lang = i18n::user_lang_from_storage(&shared_storage, chat_id.0).await;
    let (format, video_quality, audio_bitrate, plan) = load_menu_user_state(&shared_storage, chat_id).await;

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
            "4320p" => "8K",
            "2160p" => "4K",
            "1440p" => "2K",
            "1080p" => "1080p",
            "720p" => "720p",
            "480p" => "480p",
            "360p" => "360p",
            _ => "Best",
        };
        let args = doracore::fluent_args!("value" => quality_display);
        i18n::t_args(&lang, "menu.quality_line", &args)
    } else {
        let bitrate_display = match audio_bitrate.as_str() {
            "128k" => "128 kbps",
            "192k" => "192 kbps",
            "256k" => "256 kbps",
            "320k" => "320 kbps",
            _ => "320 kbps",
        };
        let args = doracore::fluent_args!("value" => bitrate_display);
        i18n::t_args(&lang, "menu.bitrate_line", &args)
    };

    let plan_display = match plan {
        Plan::Premium => i18n::t(&lang, "menu.plan_premium"),
        Plan::Vip => i18n::t(&lang, "menu.plan_vip"),
        Plan::Free => i18n::t(&lang, "menu.plan_free"),
    };

    let (text, keyboard) = build_enhanced_menu(&lang, format_emoji, &quality_line, &plan_display);

    edit_caption_or_text(bot, chat_id, message_id, text, Some(keyboard)).await
}

/// Shows detailed view of user's current settings.
///
/// Displays all user preferences including format, quality, bitrate, send type, and plan.
pub(crate) async fn show_current_settings_detail(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
) -> ResponseResult<()> {
    let _ = db_pool;
    let user = shared_storage.get_user(chat_id.0).await.ok().flatten();
    let format = user
        .as_ref()
        .map(|user| user.download_format.clone())
        .unwrap_or_else(|| "mp3".to_string());
    let video_quality = user
        .as_ref()
        .map(|user| user.video_quality.clone())
        .unwrap_or_else(|| "best".to_string());
    let audio_bitrate = user
        .as_ref()
        .map(|user| user.audio_bitrate.clone())
        .unwrap_or_else(|| "320k".to_string());
    let send_as_document = user.as_ref().map(|user| user.send_as_document).unwrap_or(0);
    let send_audio_as_document = user.as_ref().map(|user| user.send_audio_as_document).unwrap_or(0);
    let plan = user.map(|user| user.plan).unwrap_or_else(Plan::default);

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
            "4320p" => "8K",
            "2160p" => "4K",
            "1440p" => "2K",
            "1080p" => "1080p",
            "720p" => "720p",
            "480p" => "480p",
            "360p" => "360p",
            _ => "Best",
        };
        format!("🎬 *Video quality:* {}", quality_display)
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
        format!("🎵 *Audio bitrate:* {}", bitrate_display)
    } else {
        "".to_string()
    };

    let video_send_type = if send_as_document == 1 {
        "📎 *Video send type:* Document"
    } else {
        "📹 *Video send type:* Media"
    };

    let audio_send_type = if send_audio_as_document == 1 {
        "📎 *Audio send type:* Document"
    } else {
        "🎵 *Audio send type:* Media"
    };

    let plan_display = match plan {
        Plan::Premium => "Premium ⭐",
        Plan::Vip => "VIP 💎",
        Plan::Free => "Free",
    };

    let mut text = format!(
        "🎬 *Your download settings*\n\n\
        📥 *Format:* {}\n",
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
        💎 *Subscription:* {}\n\n\
        To change settings, press \"⚙️ Download Settings\" in the main menu\\.",
        video_send_type, audio_send_type, plan_display
    ));

    let keyboard = InlineKeyboardMarkup::new(vec![vec![crate::telegram::cb(
        "🔙 Back to menu".to_string(),
        "back:enhanced_main",
    )]]);

    edit_caption_or_text(bot, chat_id, message_id, text, Some(keyboard)).await
}

/// Shows help and FAQ information.
///
/// Displays common questions and answers about using the bot.
pub(crate) async fn show_help_menu(bot: &Bot, chat_id: ChatId, message_id: MessageId) -> ResponseResult<()> {
    let admin_line = if ADMIN_USERNAME.is_empty() {
        "*Need help?*\nContact the administrator.".to_string()
    } else {
        format!(
            "*Need help?*\nWrite to @{} \\(administrator\\)",
            escape_markdown(ADMIN_USERNAME.as_str())
        )
    };

    let text = format!(
        "❓ *Help and FAQ*\n\n\
        *How to use the bot?*\n\
        Simply send me a link to a video or track from YouTube, SoundCloud, VK, TikTok, Instagram or other services\\.\n\n\
        *What formats are supported?*\n\
        🎵 MP3 \\- audio only\n\
        🎬 MP4 \\- video\n\
        🎬🎵 MP4 \\+ MP3 \\- both video and audio\n\
        📝 SRT \\- subtitles\n\
        📄 TXT \\- text subtitles\n\n\
        *How to change quality?*\n\
        Use the \"⚙️ Download Settings\" button in the main menu\\.\n\n\
        *What services are supported?*\n\
        YouTube, SoundCloud, VK, TikTok, Instagram, Twitch, Spotify and many others\\! Full list in the \"🌐 Available Services\" section\\.\n\n\
        {}",
        admin_line
    );

    let keyboard = InlineKeyboardMarkup::new(vec![vec![crate::telegram::cb(
        "🔙 Back to menu".to_string(),
        "back:enhanced_main",
    )]]);

    edit_caption_or_text(bot, chat_id, message_id, text.to_string(), Some(keyboard)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_enhanced_menu_returns_keyboard() {
        let lang = i18n::lang_from_code("en");
        let (text, keyboard) = build_enhanced_menu(&lang, "🎵 MP3", "🎬 720p / 🎵 320 kbps", "⭐ Premium");

        // Text should not be empty
        assert!(!text.is_empty());

        // Keyboard should include the dedicated feedback row.
        assert_eq!(keyboard.inline_keyboard.len(), 5);
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
        // Fourth row: Language + Vault
        assert_eq!(keyboard.inline_keyboard[3].len(), 2);
        // Fifth row: Feedback
        assert_eq!(keyboard.inline_keyboard[4].len(), 1);
    }
}
