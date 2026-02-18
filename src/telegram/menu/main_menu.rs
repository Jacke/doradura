use crate::core::config::admin::ADMIN_USERNAME;
use crate::core::escape_markdown;
use crate::core::types::Plan;
use crate::i18n;
use crate::storage::db::{self, DbPool};
use crate::telegram::Bot;
use fluent_templates::fluent_bundle::FluentArgs;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardMarkup, MessageId};
use teloxide::RequestError;
use unic_langid::LanguageIdentifier;

use super::helpers::edit_caption_or_text;

/// Shows the main settings menu for the download mode.
///
/// Displays inline buttons for video quality, audio bitrate, and supported services.
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
    let mut args = FluentArgs::new();
    args.set("format", format_emoji);
    args.set("quality", quality_line);
    args.set("plan", plan_display);

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
            crate::telegram::cb(i18n::t(lang, "menu.button_feedback"), "main:feedback"),
        ],
    ]);

    (text, keyboard)
}

// Edit message to show main menu (for callbacks that need to edit existing message)
pub(crate) async fn edit_main_menu(
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
pub async fn show_enhanced_main_menu(bot: &Bot, chat_id: ChatId, db_pool: Arc<DbPool>) -> ResponseResult<Message> {
    let lang = i18n::user_lang_from_pool(&db_pool, chat_id.0);
    let (format, video_quality, audio_bitrate, plan) = match db::get_connection(&db_pool) {
        Ok(conn) => {
            let format = db::get_user_download_format(&conn, chat_id.0).unwrap_or_else(|_| "mp3".to_string());
            let video_quality = db::get_user_video_quality(&conn, chat_id.0).unwrap_or_else(|_| "best".to_string());
            let audio_bitrate = db::get_user_audio_bitrate(&conn, chat_id.0).unwrap_or_else(|_| "320k".to_string());
            let plan = match db::get_user(&conn, chat_id.0) {
                Ok(Some(user)) => user.plan,
                _ => Plan::default(),
            };
            (format, video_quality, audio_bitrate, plan)
        }
        Err(e) => {
            log::error!("Failed to get DB connection for enhanced menu: {}", e);
            (
                "mp3".to_string(),
                "best".to_string(),
                "320k".to_string(),
                Plan::default(),
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
                _ => Plan::default(),
            };
            (format, video_quality, audio_bitrate, plan)
        }
        Err(e) => {
            log::error!("Failed to get DB connection for enhanced menu: {}", e);
            (
                "mp3".to_string(),
                "best".to_string(),
                "320k".to_string(),
                Plan::default(),
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
        _ => Plan::default(),
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

    let plan_display = match plan {
        Plan::Premium => "Premium ⭐",
        Plan::Vip => "VIP 💎",
        Plan::Free => "Free",
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

    let keyboard = InlineKeyboardMarkup::new(vec![vec![crate::telegram::cb(
        "🔙 Назад в меню".to_string(),
        "back:enhanced_main",
    )]]);

    edit_caption_or_text(bot, chat_id, message_id, text, Some(keyboard)).await
}

/// Shows help and FAQ information.
///
/// Displays common questions and answers about using the bot.
pub(crate) async fn show_help_menu(bot: &Bot, chat_id: ChatId, message_id: MessageId) -> ResponseResult<()> {
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

    let keyboard = InlineKeyboardMarkup::new(vec![vec![crate::telegram::cb(
        "🔙 Назад в меню".to_string(),
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
