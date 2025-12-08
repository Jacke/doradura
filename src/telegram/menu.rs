use crate::core::export::handle_export;
use crate::core::history::handle_history_callback;
use crate::core::rate_limiter::RateLimiter;
use crate::core::subscription::{create_subscription_invoice, show_subscription_info};
use crate::download::queue::{DownloadQueue, DownloadTask};
use crate::storage::cache;
use crate::storage::db::{self, DbPool};
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup, MessageId};
use teloxide::RequestError;
use url::Url;

/// Экранирует специальные символы для MarkdownV2
///
/// В Telegram MarkdownV2 требуется экранировать следующие символы:
/// _ * [ ] ( ) ~ ` > # + - = | { } . !
///
/// Важно: обратный слеш должен экранироваться первым, чтобы избежать повторного экранирования
fn escape_markdown(text: &str) -> String {
    let mut result = String::with_capacity(text.len() * 2);

    for c in text.chars() {
        match c {
            '\\' => result.push_str("\\\\"),
            '_' => result.push_str("\\_"),
            '*' => result.push_str("\\*"),
            '[' => result.push_str("\\["),
            ']' => result.push_str("\\]"),
            '(' => result.push_str("\\("),
            ')' => result.push_str("\\)"),
            '~' => result.push_str("\\~"),
            '`' => result.push_str("\\`"),
            '>' => result.push_str("\\>"),
            '#' => result.push_str("\\#"),
            '+' => result.push_str("\\+"),
            '-' => result.push_str("\\-"),
            '=' => result.push_str("\\="),
            '|' => result.push_str("\\|"),
            '{' => result.push_str("\\{"),
            '}' => result.push_str("\\}"),
            '.' => result.push_str("\\."),
            '!' => result.push_str("\\!"),
            _ => result.push(c),
        }
    }

    result
}

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

/// Показывает главное меню настроек режима загрузки.
///
/// Отображает меню с инлайн-кнопками для выбора типа загрузки и просмотра доступных сервисов.
///
/// # Arguments
///
/// * `bot` - Экземпляр Telegram бота
/// * `chat_id` - ID чата пользователя
/// * `db_pool` - Пул соединений с базой данных
///
/// # Returns
///
/// Возвращает `ResponseResult<Message>` с отправленным сообщением или ошибку.
///
/// # Errors
///
/// Возвращает ошибку если не удалось получить соединение с БД или отправить сообщение.
pub async fn show_main_menu(
    bot: &Bot,
    chat_id: ChatId,
    db_pool: Arc<DbPool>,
) -> ResponseResult<Message> {
    let conn = db::get_connection(&db_pool).map_err(|e| {
        RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string())))
    })?;
    let format =
        db::get_user_download_format(&conn, chat_id.0).unwrap_or_else(|_| "mp3".to_string());
    let video_quality =
        db::get_user_video_quality(&conn, chat_id.0).unwrap_or_else(|_| "best".to_string());
    let audio_bitrate =
        db::get_user_audio_bitrate(&conn, chat_id.0).unwrap_or_else(|_| "320k".to_string());

    let format_emoji = match format.as_str() {
        "mp3" => "🎵 MP3",
        "mp4" => "🎬 MP4",
        "mp4+mp3" => "🎬🎵 MP4 + MP3",
        "srt" => "📝 SRT",
        "txt" => "📄 TXT",
        _ => "🎵 MP3",
    };

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

    let keyboard = InlineKeyboardMarkup::new(vec![
        vec![InlineKeyboardButton::callback(
            format!("📥 Тип загрузки: {}", format_emoji),
            "mode:download_type",
        )],
        vec![InlineKeyboardButton::callback(
            if format == "mp4" {
                format!("🎬 Качество видео: {}", quality_emoji)
            } else {
                format!("🎵 Битрейт аудио: {}", bitrate_display)
            },
            if format == "mp4" {
                "mode:video_quality"
            } else {
                "mode:audio_bitrate"
            },
        )],
        vec![InlineKeyboardButton::callback(
            "🌐 Доступные сервисы".to_string(),
            "mode:services",
        )],
        vec![InlineKeyboardButton::callback(
            "💳 Моя подписка".to_string(),
            "mode:subscription",
        )],
    ]);

    bot.send_message(
        chat_id,
        "🎵 *Дора \\- Режимы Загрузки*\n\nВыбери, что хочешь настроить\\!",
    )
    .parse_mode(teloxide::types::ParseMode::MarkdownV2)
    .reply_markup(keyboard)
    .await
}

/// Показывает меню выбора типа загрузки.
///
/// Отображает меню с доступными форматами (MP3, MP4, SRT, TXT) и отмечает текущий выбор пользователя.
///
/// # Arguments
///
/// * `bot` - Экземпляр Telegram бота
/// * `chat_id` - ID чата пользователя
/// * `message_id` - ID сообщения для редактирования
/// * `db_pool` - Пул соединений с базой данных
/// * `url_id` - Опциональный ID URL из preview (если меню открыто из preview)
/// * `preview_msg_id` - Опциональный ID preview сообщения для удаления при изменении формата
///
/// # Returns
///
/// Возвращает `ResponseResult<()>` или ошибку при редактировании сообщения.
pub async fn show_download_type_menu(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    db_pool: Arc<DbPool>,
    url_id: Option<&str>,
    preview_msg_id: Option<MessageId>,
) -> ResponseResult<()> {
    let conn = db::get_connection(&db_pool).map_err(|e| {
        RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string())))
    })?;
    let current_format =
        db::get_user_download_format(&conn, chat_id.0).unwrap_or_else(|_| "mp3".to_string());

    // Формируем callback данные с url_id и preview_msg_id если они есть
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

    let back_callback = if url_id.is_some() {
        if let Some(preview_id) = preview_msg_id {
            format!("back:preview:{}:{}", url_id.unwrap(), preview_id.0)
        } else {
            format!("back:preview:{}", url_id.unwrap())
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
            "🔙 Назад".to_string(),
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
    edit_caption_or_text(
        bot,
        chat_id,
        message_id,
        format!(
            "Выбери формат для скачивания\\:\n\n*Текущий формат\\: {}*",
            escaped_format
        ),
        Some(keyboard),
    )
    .await?;
    Ok(())
}

/// Отправляет меню выбора типа загрузки как новое текстовое сообщение.
///
/// Используется когда нужно отправить меню вместо редактирования существующего сообщения
/// (например, когда исходное сообщение содержит медиа и не может быть отредактировано).
///
/// # Arguments
///
/// * `bot` - Экземпляр Telegram бота
/// * `chat_id` - ID чата пользователя
/// * `db_pool` - Пул соединений с базой данных
/// * `url_id` - Опциональный ID URL из preview (если меню открыто из preview)
///
/// # Returns
///
/// Возвращает `ResponseResult<()>` или ошибку при отправке сообщения.
pub async fn send_download_type_menu_as_new(
    bot: &Bot,
    chat_id: ChatId,
    db_pool: Arc<DbPool>,
    url_id: Option<&str>,
    preview_msg_id: Option<MessageId>,
) -> ResponseResult<()> {
    let conn = db::get_connection(&db_pool).map_err(|e| {
        RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string())))
    })?;
    let current_format =
        db::get_user_download_format(&conn, chat_id.0).unwrap_or_else(|_| "mp3".to_string());

    // Формируем callback данные с url_id и preview_msg_id если они есть
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

    let back_callback = if url_id.is_some() {
        if let Some(preview_id) = preview_msg_id {
            format!("back:preview:{}:{}", url_id.unwrap(), preview_id.0)
        } else {
            format!("back:preview:{}", url_id.unwrap())
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
            "🔙 Назад".to_string(),
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
    bot.send_message(
        chat_id,
        format!(
            "Выбери формат для скачивания\\:\n\n*Текущий формат\\: {}*",
            escaped_format
        ),
    )
    .parse_mode(teloxide::types::ParseMode::MarkdownV2)
    .reply_markup(keyboard)
    .await?;
    Ok(())
}

/// Показывает меню выбора качества видео.
///
/// Отображает меню с доступными качествами (1080p, 720p, 480p, 360p, best) и отмечает текущий выбор пользователя.
///
/// # Arguments
///
/// * `bot` - Экземпляр Telegram бота
/// * `chat_id` - ID чата пользователя
/// * `message_id` - ID сообщения для редактирования
/// * `db_pool` - Пул соединений с базой данных
/// * `url_id` - Опциональный ID URL из preview (если меню открыто из preview)
///
/// # Returns
///
/// Возвращает `ResponseResult<()>` или ошибку при редактировании сообщения.
pub async fn show_video_quality_menu(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    db_pool: Arc<DbPool>,
    url_id: Option<&str>,
) -> ResponseResult<()> {
    let conn = db::get_connection(&db_pool).map_err(|e| {
        RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string())))
    })?;
    let current_quality =
        db::get_user_video_quality(&conn, chat_id.0).unwrap_or_else(|_| "best".to_string());
    let send_as_document = db::get_user_send_as_document(&conn, chat_id.0).unwrap_or(0);

    let keyboard = InlineKeyboardMarkup::new(vec![
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
                "📹 Отправка: Media ✓"
            } else {
                "📄 Отправка: Document ✓"
            }
            .to_string(),
            "send_type:toggle",
        )],
        vec![InlineKeyboardButton::callback(
            "🔙 Назад".to_string(),
            if url_id.is_some() {
                format!("back:main:preview:{}", url_id.unwrap())
            } else {
                "back:main".to_string()
            },
        )],
    ]);

    let quality_display = match current_quality.as_str() {
        "1080p" => "🎬 1080p (Full HD)",
        "720p" => "🎬 720p (HD)",
        "480p" => "🎬 480p (SD)",
        "360p" => "🎬 360p (Low)",
        _ => "🎬 Best (Авто)",
    };

    let send_type_display = if send_as_document == 0 {
        "📹 Media"
    } else {
        "📄 Document"
    };

    let escaped_quality = escape_markdown(quality_display);
    let escaped_send_type = escape_markdown(send_type_display);
    edit_caption_or_text(
        bot,
        chat_id,
        message_id,
        format!(
            "Выбери качество видео\\:\n\n*Текущее качество\\: {}*\n*Тип отправки\\: {}*",
            escaped_quality, escaped_send_type
        ),
        Some(keyboard),
    )
    .await?;
    Ok(())
}

/// Показывает меню выбора битрейта аудио.
///
/// Отображает меню с доступными битрейтами (128kbps, 192kbps, 256kbps, 320kbps) и отмечает текущий выбор пользователя.
///
/// # Arguments
///
/// * `bot` - Экземпляр Telegram бота
/// * `chat_id` - ID чата пользователя
/// * `message_id` - ID сообщения для редактирования
/// * `db_pool` - Пул соединений с базой данных
/// * `url_id` - Опциональный ID URL из preview (если меню открыто из preview)
///
/// # Returns
///
/// Возвращает `ResponseResult<()>` или ошибку при редактировании сообщения.
pub async fn show_audio_bitrate_menu(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    db_pool: Arc<DbPool>,
    url_id: Option<&str>,
) -> ResponseResult<()> {
    let conn = db::get_connection(&db_pool).map_err(|e| {
        RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string())))
    })?;
    let current_bitrate =
        db::get_user_audio_bitrate(&conn, chat_id.0).unwrap_or_else(|_| "320k".to_string());
    let send_audio_as_document = db::get_user_send_audio_as_document(&conn, chat_id.0).unwrap_or(0);

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
                "🎵 Отправка: Media ✓"
            } else {
                "📄 Отправка: Document ✓"
            }
            .to_string(),
            "audio_send_type:toggle",
        )],
        vec![InlineKeyboardButton::callback(
            "🔙 Назад".to_string(),
            if url_id.is_some() {
                format!("back:main:preview:{}", url_id.unwrap())
            } else {
                "back:main".to_string()
            },
        )],
    ]);

    let send_type_display = if send_audio_as_document == 0 {
        "🎵 Media"
    } else {
        "📄 Document"
    };

    let escaped_bitrate = escape_markdown(&current_bitrate);
    let escaped_send_type = escape_markdown(send_type_display);

    edit_caption_or_text(
        bot,
        chat_id,
        message_id,
        format!(
            "Выбери битрейт для аудио\\:\n\n*Текущий битрейт\\: {}*\n*Тип отправки\\: {}*",
            escaped_bitrate, escaped_send_type
        ),
        Some(keyboard),
    )
    .await?;
    Ok(())
}

/// Показывает меню с информацией о поддерживаемых сервисах.
///
/// Отображает список доступных сервисов (YouTube, SoundCloud) и поддерживаемых форматов.
///
/// # Arguments
///
/// * `bot` - Экземпляр Telegram бота
/// * `chat_id` - ID чата пользователя
/// * `message_id` - ID сообщения для редактирования
///
/// # Returns
///
/// Возвращает `ResponseResult<()>` или ошибку при редактировании сообщения.
pub async fn show_services_menu(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
) -> ResponseResult<()> {
    let keyboard = InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::callback(
        "🔙 Назад".to_string(),
        "back:enhanced_main",
    )]]);

    let text = "🌐 *Поддерживаемые сервисы*\n\n\
        🎥 *YouTube*\n\
        • MP3 \\(Аудио\\)\n\
        • MP4 \\(Видео\\)\n\
        • SRT \\(Субтитры\\)\n\
        • TXT \\(Текстовые субтитры\\)\n\n\
        🎵 *SoundCloud*\n\
        • MP3 \\(Аудио\\)\n\n\
        📱 *VK \\(ВКонтакте\\)*\n\
        • MP3 \\(Аудио\\)\n\
        • MP4 \\(Видео\\)\n\n\
        🎬 *TikTok*\n\
        • MP3 \\(Аудио\\)\n\
        • MP4 \\(Видео\\)\n\n\
        📸 *Instagram*\n\
        • MP3 \\(Аудио из Reels\\)\n\
        • MP4 \\(Видео Reels\\)\n\n\
        🎮 *Twitch*\n\
        • MP4 \\(Клипы\\)\n\n\
        🎧 *Spotify*\n\
        • MP3 \\(Аудио\\)\n\n\
        И многие другие сервисы, которые я поддерживаю\\!\n\n\
        Просто отправь мне ссылку на трек или видео\\! ❤️‍🔥";

    edit_caption_or_text(bot, chat_id, message_id, text.to_string(), Some(keyboard)).await?;
    Ok(())
}

// Edit message to show main menu (for callbacks that need to edit existing message)
// Args: bot - telegram bot instance, chat_id - user's chat ID, message_id - ID of message to edit, db_pool - database connection pool
// Functionality: Edits existing message to show main mode menu
// url_id - Опциональный ID URL из preview (если меню открыто из preview)
// preview_msg_id - Опциональный ID preview сообщения для удаления при изменении формата
async fn edit_main_menu(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    db_pool: Arc<DbPool>,
    url_id: Option<&str>,
    _preview_msg_id: Option<MessageId>,
) -> ResponseResult<()> {
    let conn = db::get_connection(&db_pool).map_err(|e| {
        RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string())))
    })?;
    let format =
        db::get_user_download_format(&conn, chat_id.0).unwrap_or_else(|_| "mp3".to_string());
    let video_quality =
        db::get_user_video_quality(&conn, chat_id.0).unwrap_or_else(|_| "best".to_string());
    let audio_bitrate =
        db::get_user_audio_bitrate(&conn, chat_id.0).unwrap_or_else(|_| "320k".to_string());

    let format_emoji = match format.as_str() {
        "mp3" => "🎵 MP3",
        "mp4" => "🎬 MP4",
        "mp4+mp3" => "🎬🎵 MP4 + MP3",
        "srt" => "📝 SRT",
        "txt" => "📄 TXT",
        _ => "🎵 MP3",
    };

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

    // Формируем callback данные с url_id если он есть
    let mode_callback = |mode: &str| {
        if let Some(id) = url_id {
            format!("mode:{}:preview:{}", mode, id)
        } else {
            format!("mode:{}", mode)
        }
    };

    let mut keyboard_rows = vec![
        vec![InlineKeyboardButton::callback(
            format!("📥 Тип загрузки: {}", format_emoji),
            mode_callback("download_type"),
        )],
        vec![InlineKeyboardButton::callback(
            if format == "mp4" || format == "mp4+mp3" {
                format!("🎬 Качество видео: {}", quality_emoji)
            } else {
                format!("🎵 Битрейт аудио: {}", bitrate_display)
            },
            if format == "mp4" || format == "mp4+mp3" {
                mode_callback("video_quality")
            } else {
                mode_callback("audio_bitrate")
            },
        )],
        vec![InlineKeyboardButton::callback(
            "🌐 Доступные сервисы".to_string(),
            mode_callback("services"),
        )],
        vec![InlineKeyboardButton::callback(
            "💳 Моя подписка".to_string(),
            mode_callback("subscription"),
        )],
    ];

    // Добавляем кнопку "Назад" если меню открыто из preview
    if url_id.is_some() {
        keyboard_rows.push(vec![InlineKeyboardButton::callback(
            "🔙 Назад к превью".to_string(),
            format!("back:preview:{}", url_id.unwrap()),
        )]);
    }

    let keyboard = InlineKeyboardMarkup::new(keyboard_rows);

    edit_caption_or_text(
        bot,
        chat_id,
        message_id,
        "🎵 *Дора \\- Режимы Загрузки*\n\nВыбери, что хочешь настроить\\!".to_string(),
        Some(keyboard),
    )
    .await?;
    Ok(())
}

/// Отправляет главное меню настроек как новое текстовое сообщение.
///
/// Используется когда нужно отправить меню вместо редактирования существующего сообщения
/// (например, когда исходное сообщение содержит медиа и не может быть отредактировано).
///
/// # Arguments
///
/// * `bot` - Экземпляр Telegram бота
/// * `chat_id` - ID чата пользователя
/// * `db_pool` - Пул соединений с базой данных
/// * `url_id` - Опциональный ID URL из preview (если меню открыто из preview)
/// * `preview_msg_id` - Опциональный ID preview сообщения для удаления при изменении формата
///
/// # Returns
///
/// Возвращает `ResponseResult<()>` или ошибку при отправке сообщения.
pub async fn send_main_menu_as_new(
    bot: &Bot,
    chat_id: ChatId,
    db_pool: Arc<DbPool>,
    url_id: Option<&str>,
    preview_msg_id: Option<MessageId>,
) -> ResponseResult<()> {
    let conn = db::get_connection(&db_pool).map_err(|e| {
        RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string())))
    })?;
    let format =
        db::get_user_download_format(&conn, chat_id.0).unwrap_or_else(|_| "mp3".to_string());
    let video_quality =
        db::get_user_video_quality(&conn, chat_id.0).unwrap_or_else(|_| "best".to_string());
    let audio_bitrate =
        db::get_user_audio_bitrate(&conn, chat_id.0).unwrap_or_else(|_| "320k".to_string());

    let format_emoji = match format.as_str() {
        "mp3" => "🎵 MP3",
        "mp4" => "🎬 MP4",
        "mp4+mp3" => "🎬🎵 MP4 + MP3",
        "srt" => "📝 SRT",
        "txt" => "📄 TXT",
        _ => "🎵 MP3",
    };

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

    // Формируем callback данные с url_id и preview_msg_id если они есть
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

    let mut keyboard_rows = vec![
        vec![InlineKeyboardButton::callback(
            format!("📥 Тип загрузки: {}", format_emoji),
            mode_callback("download_type"),
        )],
        vec![InlineKeyboardButton::callback(
            if format == "mp4" || format == "mp4+mp3" {
                format!("🎬 Качество видео: {}", quality_emoji)
            } else {
                format!("🎵 Битрейт аудио: {}", bitrate_display)
            },
            if format == "mp4" || format == "mp4+mp3" {
                mode_callback("video_quality")
            } else {
                mode_callback("audio_bitrate")
            },
        )],
        vec![InlineKeyboardButton::callback(
            "🌐 Доступные сервисы".to_string(),
            mode_callback("services"),
        )],
        vec![InlineKeyboardButton::callback(
            "💳 Моя подписка".to_string(),
            mode_callback("subscription"),
        )],
    ];

    // Добавляем кнопку "Назад" если меню открыто из preview
    if url_id.is_some() {
        let back_callback = if let Some(preview_id) = preview_msg_id {
            format!("back:preview:{}:{}", url_id.unwrap(), preview_id.0)
        } else {
            format!("back:preview:{}", url_id.unwrap())
        };
        keyboard_rows.push(vec![InlineKeyboardButton::callback(
            "🔙 Назад к превью".to_string(),
            back_callback,
        )]);
    }

    let keyboard = InlineKeyboardMarkup::new(keyboard_rows);

    bot.send_message(
        chat_id,
        "🎵 *Дора \\- Режимы Загрузки*\n\nВыбери, что хочешь настроить\\!",
    )
    .parse_mode(teloxide::types::ParseMode::MarkdownV2)
    .reply_markup(keyboard)
    .await?;
    Ok(())
}

/// Обрабатывает callback-запросы от инлайн-клавиатур меню.
///
/// Обрабатывает нажатия на кнопки меню и обновляет настройки пользователя или переключает между меню.
///
/// # Arguments
///
/// * `bot` - Экземпляр Telegram бота
/// * `q` - Callback query для обработки
/// * `db_pool` - Пул соединений с базой данных
/// * `download_queue` - Очередь загрузок
/// * `rate_limiter` - Rate limiter
///
/// # Returns
///
/// Возвращает `ResponseResult<()>` или ошибку при обработке callback.
///
/// # Supported Callbacks
///
/// - `mode:download_type` - Переход к меню выбора формата
/// - `mode:services` - Показ информации о сервисах
/// - `back:main` - Возврат к главному меню
/// - `format:mp3|mp4|srt|txt` - Установка формата загрузки
/// - `dl:format:url_id` - Начать загрузку с указанным форматом (url_id - короткий ID из кэша)
/// - `pv:set:url_id` - Показать настройки для превью
/// - `pv:cancel:url_id` - Отменить превью
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
            // Handle audio effects callbacks first
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
                if let Err(e) =
                    handle_audio_effects_callback(bot.clone(), ae_query, Arc::clone(&db_pool)).await
                {
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
                let url_id = if is_from_preview {
                    Some(parts[3])
                } else {
                    None
                };
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
                        show_video_quality_menu(
                            &bot,
                            chat_id,
                            message_id,
                            Arc::clone(&db_pool),
                            url_id,
                        )
                        .await?;
                    }
                    "audio_bitrate" => {
                        show_audio_bitrate_menu(
                            &bot,
                            chat_id,
                            message_id,
                            Arc::clone(&db_pool),
                            url_id,
                        )
                        .await?;
                    }
                    "services" => {
                        show_services_menu(&bot, chat_id, message_id).await?;
                    }
                    "subscription" => {
                        // Удаляем старое сообщение и показываем информацию о подписке
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
                        edit_main_menu(&bot, chat_id, message_id, Arc::clone(&db_pool), None, None)
                            .await?;
                    }
                    "current" => {
                        // Show detailed current settings
                        show_current_settings_detail(
                            &bot,
                            chat_id,
                            message_id,
                            Arc::clone(&db_pool),
                        )
                        .await?;
                    }
                    "stats" => {
                        // Delete current message and show stats
                        let _ = bot.delete_message(chat_id, message_id).await;
                        let _ = crate::core::stats::show_user_stats(
                            &bot,
                            chat_id,
                            Arc::clone(&db_pool),
                        )
                        .await;
                    }
                    "history" => {
                        // Delete current message and show history
                        let _ = bot.delete_message(chat_id, message_id).await;
                        let _ =
                            crate::core::history::show_history(&bot, chat_id, Arc::clone(&db_pool))
                                .await;
                    }
                    "services" => {
                        // Edit message to show services
                        show_services_menu(&bot, chat_id, message_id).await?;
                    }
                    "subscription" => {
                        // Delete current message and show subscription info
                        let _ = bot.delete_message(chat_id, message_id).await;
                        let _ = crate::core::subscription::show_subscription_info(
                            &bot,
                            chat_id,
                            Arc::clone(&db_pool),
                        )
                        .await;
                    }
                    "help" => {
                        // Edit message to show help
                        show_help_menu(&bot, chat_id, message_id).await?;
                    }
                    _ => {}
                }
            } else if let Some(plan) = data.strip_prefix("subscribe:") {
                log::info!(
                    "🔔 Subscribe callback received: data={}, chat_id={}",
                    data,
                    chat_id.0
                );
                bot.answer_callback_query(callback_id.clone()).await?;
                // Remove "subscribe:" prefix
                log::info!("📌 Extracted plan: {}", plan);
                match plan {
                    "premium" | "vip" => {
                        log::info!(
                            "✅ Valid plan '{}', creating invoice for chat_id={}",
                            plan,
                            chat_id.0
                        );
                        // Создаем инвойс для оплаты через Telegram Stars
                        match create_subscription_invoice(&bot, chat_id, plan).await {
                            Ok(msg) => {
                                log::info!("✅ Invoice created successfully for user {} plan {}. Message ID: {}", chat_id.0, plan, msg.id.0);
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
                        bot.answer_callback_query(callback_id)
                            .text("Неизвестный план")
                            .await?;
                    }
                }
            } else if let Some(action) = data.strip_prefix("subscription:") {
                bot.answer_callback_query(callback_id.clone()).await?;
                // Remove "subscription:" prefix
                match action {
                    "cancel" => {
                        // Отменяем подписку пользователя
                        match crate::core::subscription::cancel_subscription(
                            &bot,
                            chat_id.0,
                            Arc::clone(&db_pool),
                        )
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

                                // Обновляем меню подписки
                                let _ = bot.delete_message(chat_id, message_id).await;
                                let _ = show_subscription_info(&bot, chat_id, Arc::clone(&db_pool))
                                    .await;
                            }
                            Err(e) => {
                                log::error!("Failed to cancel subscription: {}", e);
                                let _ = bot
                                    .send_message(
                                        chat_id,
                                        "❌ Не удалось отменить подписку\\. Попробуй позже или обратись к администратору\\.",
                                    )
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
            } else if let Some(quality) = data.strip_prefix("quality:") {
                let _ = bot.answer_callback_query(callback_id.clone()).await;
                // Remove "quality:" prefix
                let conn = db::get_connection(&db_pool).map_err(|e| {
                    RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string())))
                })?;
                db::set_user_video_quality(&conn, chat_id.0, quality).map_err(|e| {
                    RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string())))
                })?;

                // Get url_id from message context if available (check if we came from preview)
                // For now, we'll need to get it from the current menu's callback data
                // Since we don't have direct access, we'll check if back button has preview context
                // This is a limitation - we'd need to store url_id in quality callback data too
                // For simplicity, we'll just update the menu without url_id
                // Update the menu to show new selection
                show_video_quality_menu(&bot, chat_id, message_id, Arc::clone(&db_pool), None)
                    .await?;
            } else if data == "send_type:toggle" {
                let _ = bot.answer_callback_query(callback_id.clone()).await;
                let conn = db::get_connection(&db_pool).map_err(|e| {
                    RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string())))
                })?;

                // Получаем текущее значение и переключаем
                let current_value = db::get_user_send_as_document(&conn, chat_id.0).unwrap_or(0);
                let new_value = if current_value == 0 { 1 } else { 0 };

                db::set_user_send_as_document(&conn, chat_id.0, new_value).map_err(|e| {
                    RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string())))
                })?;

                // Обновляем меню
                show_video_quality_menu(&bot, chat_id, message_id, Arc::clone(&db_pool), None)
                    .await?;
            } else if let Some(bitrate) = data.strip_prefix("bitrate:") {
                let _ = bot.answer_callback_query(callback_id.clone()).await;
                // Remove "bitrate:" prefix
                let conn = db::get_connection(&db_pool).map_err(|e| {
                    RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string())))
                })?;
                db::set_user_audio_bitrate(&conn, chat_id.0, bitrate).map_err(|e| {
                    RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string())))
                })?;

                // Update the menu to show new selection
                show_audio_bitrate_menu(&bot, chat_id, message_id, Arc::clone(&db_pool), None)
                    .await?;
            } else if data == "audio_send_type:toggle" {
                let _ = bot.answer_callback_query(callback_id.clone()).await;
                let conn = db::get_connection(&db_pool).map_err(|e| {
                    RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string())))
                })?;

                // Получаем текущее значение и переключаем
                let current_value =
                    db::get_user_send_audio_as_document(&conn, chat_id.0).unwrap_or(0);
                let new_value = if current_value == 0 { 1 } else { 0 };

                db::set_user_send_audio_as_document(&conn, chat_id.0, new_value).map_err(|e| {
                    RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string())))
                })?;

                // Обновляем меню
                show_audio_bitrate_menu(&bot, chat_id, message_id, Arc::clone(&db_pool), None)
                    .await?;
            } else if data.starts_with("video_send_type:toggle:") {
                let _ = bot.answer_callback_query(callback_id.clone()).await;

                // Извлекаем url_id из callback data: video_send_type:toggle:url_id
                let parts: Vec<&str> = data.split(':').collect();
                if parts.len() >= 3 {
                    let url_id = parts[2];

                    let conn = db::get_connection(&db_pool).map_err(|e| {
                        RequestError::from(std::sync::Arc::new(std::io::Error::other(
                            e.to_string(),
                        )))
                    })?;

                    // Получаем текущее значение и переключаем
                    let current_value =
                        db::get_user_send_as_document(&conn, chat_id.0).unwrap_or(0);
                    let new_value = if current_value == 0 { 1 } else { 0 };

                    // Логируем изменение
                    log::info!(
                        "🔄 Video send type toggled for user {}: {} -> {} ({})",
                        chat_id.0,
                        if current_value == 0 {
                            "Media"
                        } else {
                            "Document"
                        },
                        if new_value == 0 { "Media" } else { "Document" },
                        if new_value == 0 {
                            "send_video"
                        } else {
                            "send_document"
                        }
                    );

                    db::set_user_send_as_document(&conn, chat_id.0, new_value).map_err(|e| {
                        RequestError::from(std::sync::Arc::new(std::io::Error::other(
                            e.to_string(),
                        )))
                    })?;

                    // Получаем текущую клавиатуру из сообщения и обновляем только toggle кнопку
                    if let Some(teloxide::types::MaybeInaccessibleMessage::Regular(regular_msg)) =
                        q.message.as_ref()
                    {
                        // Получаем текущую клавиатуру
                        if let Some(keyboard) = regular_msg.reply_markup() {
                            // Клонируем клавиатуру и обновляем toggle кнопку
                            let mut new_buttons = keyboard.inline_keyboard.clone();

                            // Находим и обновляем toggle кнопку (ищем кнопку с callback video_send_type:toggle)
                            for row in &mut new_buttons {
                                for button in row {
                                    if let teloxide::types::InlineKeyboardButtonKind::CallbackData(
                                        ref cb_data,
                                    ) = button.kind
                                    {
                                        if cb_data.starts_with("video_send_type:toggle:") {
                                            // Обновляем текст кнопки
                                            button.text = if new_value == 0 {
                                                "📹 Отправка: Media ✓".to_string()
                                            } else {
                                                "📄 Отправка: Document ✓".to_string()
                                            };
                                            log::debug!(
                                                "Updated toggle button text to: {}",
                                                button.text
                                            );
                                        }
                                    }
                                }
                            }

                            // Обновляем только клавиатуру, не трогая текст и изображение
                            let new_keyboard =
                                teloxide::types::InlineKeyboardMarkup::new(new_buttons);
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
                                        RequestError::from(std::sync::Arc::new(
                                            std::io::Error::other(e.to_string()),
                                        ))
                                    })?;
                                    let current_format =
                                        db::get_user_download_format(&conn, chat_id.0)
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
                                                    log::error!(
                                                        "Failed to update preview message: {:?}",
                                                        e
                                                    );
                                                    let _ = bot.send_message(chat_id, "Не удалось обновить превью. Попробуй отправить ссылку снова.").await;
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            log::error!("Failed to get preview metadata: {:?}", e);
                                            let _ = bot.send_message(chat_id, "Не удалось обновить превью. Попробуй отправить ссылку снова.").await;
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
                            log::warn!(
                                "URL not found in cache for ID: {} (expired or invalid)",
                                url_id
                            );
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
                            edit_main_menu(
                                &bot,
                                chat_id,
                                message_id,
                                Arc::clone(&db_pool),
                                None,
                                None,
                            )
                            .await?;
                        }
                        "back:enhanced_main" => {
                            edit_enhanced_main_menu(
                                &bot,
                                chat_id,
                                message_id,
                                Arc::clone(&db_pool),
                            )
                            .await?;
                        }
                        "back:start" => {
                            bot.edit_message_text(
                                chat_id,
                                message_id,
                                "Хэй\\! Я Дора, дай мне ссылку и я скачаю ❤️‍🔥",
                            )
                            .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                            .await?;
                        }
                        _ => {}
                    }
                }
            } else if data.starts_with("format:") {
                let _ = bot.answer_callback_query(callback_id.clone()).await;
                // Format: format:mp3 or format:mp3:preview:url_id or format:mp3:preview:url_id:preview_msg_id
                let parts: Vec<&str> = data.split(':').collect();
                let format = parts[1];
                let is_from_preview = parts.len() >= 4 && parts[2] == "preview";
                let url_id = if is_from_preview {
                    Some(parts[3])
                } else {
                    None
                };
                let _preview_msg_id = if is_from_preview && parts.len() >= 5 {
                    parts[4].parse::<i32>().ok().map(teloxide::types::MessageId)
                } else {
                    None
                };

                let conn = db::get_connection(&db_pool).map_err(|e| {
                    RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string())))
                })?;
                db::set_user_download_format(&conn, chat_id.0, format).map_err(|e| {
                    RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string())))
                })?;

                if is_from_preview && url_id.is_some() {
                    // Get URL from cache and return to preview menu with updated format
                    match cache::get_url(&db_pool, url_id.unwrap()).await {
                        Some(url_str) => {
                            match url::Url::parse(&url_str) {
                                Ok(url) => {
                                    let video_quality = if format == "mp4" {
                                        db::get_user_video_quality(&conn, chat_id.0).ok()
                                    } else {
                                        None
                                    };

                                    // Get metadata and send new preview, delete old preview if preview_msg_id is available
                                    match crate::telegram::preview::get_preview_metadata(
                                        &url,
                                        Some(format),
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
                                                format,
                                                video_quality.as_deref(),
                                                Arc::clone(&db_pool),
                                            )
                                            .await
                                            {
                                                Ok(_) => {
                                                    log::info!(
                                                        "Preview updated with new format: {}",
                                                        format
                                                    );
                                                }
                                                Err(e) => {
                                                    log::error!(
                                                        "Failed to send updated preview: {:?}",
                                                        e
                                                    );
                                                    let _ = bot.send_message(chat_id, "Не удалось обновить превью. Попробуй отправить ссылку снова.").await;
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            log::error!("Failed to get preview metadata: {:?}", e);
                                            let _ = bot.send_message(chat_id, "Не удалось обновить превью. Попробуй отправить ссылку снова.").await;
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
                            log::warn!(
                                "URL not found in cache for ID: {} (expired or invalid)",
                                url_id.unwrap()
                            );
                            bot.answer_callback_query(callback_id)
                                .text("Ссылка устарела, отправь её снова")
                                .await?;
                        }
                    }
                } else {
                    // Update the menu to show new selection
                    show_download_type_menu(
                        &bot,
                        chat_id,
                        message_id,
                        Arc::clone(&db_pool),
                        None,
                        None,
                    )
                    .await?;
                }
            } else if data.starts_with("dl:") {
                // Don't answer immediately - we'll answer after processing
                // Format: dl:format:url_id (старый формат)
                // Format: dl:format:quality:url_id (новый формат для видео с выбором качества)
                let parts: Vec<&str> = data.split(':').collect();

                if parts.len() >= 3 {
                    let format = parts[1];
                    let url_id = if parts.len() == 3 {
                        // Старый формат: dl:format:url_id
                        parts[2]
                    } else if parts.len() == 4 {
                        // Новый формат: dl:format:quality:url_id
                        parts[3]
                    } else {
                        log::warn!("Invalid dl callback format: {}", data);
                        bot.answer_callback_query(callback_id)
                            .text("Ошибка: неверный формат запроса")
                            .await?;
                        return Ok(());
                    };

                    // Извлекаем качество если указано (новый формат)
                    let selected_quality = if parts.len() == 4 && format == "mp4" {
                        Some(parts[2].to_string()) // quality из dl:mp4:quality:url_id
                    } else {
                        None
                    };

                    // Get URL from cache by ID
                    match cache::get_url(&db_pool, url_id).await {
                        Some(url_str) => {
                            match Url::parse(&url_str) {
                                Ok(url) => {
                                    // Get user preferences for quality/bitrate and plan
                                    let conn = db::get_connection(&db_pool).map_err(|e| {
                                        RequestError::from(std::sync::Arc::new(
                                            std::io::Error::other(e.to_string()),
                                        ))
                                    })?;
                                    let plan = match db::get_user(&conn, chat_id.0) {
                                        Ok(Some(ref user)) => user.plan.clone(),
                                        _ => "free".to_string(),
                                    };

                                    // Check rate limit
                                    if rate_limiter.is_rate_limited(chat_id, &plan).await {
                                        if let Some(remaining_time) =
                                            rate_limiter.get_remaining_time(chat_id).await
                                        {
                                            let remaining_seconds = remaining_time.as_secs();
                                            bot.answer_callback_query(callback_id)
                                                .text(format!(
                                                    "Подожди {} секунд",
                                                    remaining_seconds
                                                ))
                                                .await?;
                                        } else {
                                            bot.answer_callback_query(callback_id)
                                                .text("Подожди немного")
                                                .await?;
                                        }
                                        return Ok(());
                                    }

                                    // Игнорируем ошибки answer_callback_query (может быть "query is too old" при двойном клике)
                                    let _ = bot.answer_callback_query(callback_id.clone()).await;

                                    rate_limiter.update_rate_limit(chat_id, &plan).await;

                                    // Обрабатываем формат "mp4+mp3" - добавляем 2 задачи в очередь
                                    if format == "mp4+mp3" {
                                        // Задача 1: MP4 (видео)
                                        let video_quality = if let Some(quality) = selected_quality
                                        {
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
                                            None, // Callback doesn't have original user message
                                            true, // is_video = true
                                            "mp4".to_string(),
                                            video_quality,
                                            None, // audio_bitrate для видео не нужен
                                            &plan,
                                        );
                                        download_queue
                                            .add_task(task_mp4, Some(Arc::clone(&db_pool)))
                                            .await;

                                        // Задача 2: MP3 (аудио)
                                        let audio_bitrate = Some(
                                            db::get_user_audio_bitrate(&conn, chat_id.0)
                                                .unwrap_or_else(|_| "320k".to_string()),
                                        );
                                        let task_mp3 = DownloadTask::from_plan(
                                            url.as_str().to_string(),
                                            chat_id,
                                            None,  // Callback doesn't have original user message
                                            false, // is_video = false
                                            "mp3".to_string(),
                                            None, // video_quality для аудио не нужен
                                            audio_bitrate,
                                            &plan,
                                        );
                                        download_queue
                                            .add_task(task_mp3, Some(Arc::clone(&db_pool)))
                                            .await;

                                        log::info!("Added 2 tasks to queue for mp4+mp3: MP4 and MP3 for chat {}", chat_id.0);
                                    } else {
                                        // Обычная обработка для одного формата
                                        let video_quality = if format == "mp4" {
                                            if let Some(quality) = selected_quality {
                                                // Качество выбрано пользователем из preview
                                                Some(quality)
                                            } else {
                                                // Используем настройки пользователя
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
                                            None, // Callback doesn't have original user message
                                            is_video,
                                            format.to_string(),
                                            video_quality,
                                            audio_bitrate,
                                            &plan,
                                        );
                                        download_queue
                                            .add_task(task, Some(Arc::clone(&db_pool)))
                                            .await;
                                    }

                                    // Delete preview message
                                    if let Err(e) = bot.delete_message(chat_id, message_id).await {
                                        log::warn!("Failed to delete preview message: {:?}", e);
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
                            log::warn!(
                                "URL not found in cache for ID: {} (expired or invalid)",
                                url_id
                            );
                            bot.answer_callback_query(callback_id)
                                .text("Ссылка устарела, отправь её снова")
                                .await?;
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
                                    teloxide::types::MaybeInaccessibleMessage::Regular(msg) => {
                                        msg.photo()
                                    }
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
            } else if data.starts_with("admin:") {
                // Handle admin panel callbacks
                let _ = bot.answer_callback_query(callback_id.clone()).await;

                // Проверка прав администратора
                let is_admin = q
                    .from
                    .username
                    .as_ref()
                    .map(|u| u == "stansob")
                    .unwrap_or(false);

                if !is_admin {
                    bot.send_message(chat_id, "❌ У тебя нет прав для выполнения этой команды.")
                        .await?;
                    return Ok(());
                }

                if let Some(user_id_str) = data.strip_prefix("admin:user:") {
                    // Показываем меню управления конкретным пользователем
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
                                            "💫 Активная подписка"
                                        } else {
                                            "🔒 Нет подписки"
                                        };

                                        let expires_info =
                                            if let Some(expires) = &user.subscription_expires_at {
                                                format!("\n📅 Истекает: {}", expires)
                                            } else {
                                                String::new()
                                            };

                                        // Создаем клавиатуру с действиями
                                        use teloxide::types::{
                                            InlineKeyboardButton, InlineKeyboardMarkup,
                                        };

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
                                            vec![InlineKeyboardButton::callback(
                                                "🔙 Назад к списку",
                                                "admin:back",
                                            )],
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
                    // Изменяем план пользователя
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

                                            // Отправляем уведомление пользователю
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
                    // Возвращаемся к списку пользователей
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

                                    let button_text =
                                        format!("{} {}", plan_emoji, username_display);
                                    let callback_data = format!("admin:user:{}", user.telegram_id);

                                    current_row.push(InlineKeyboardButton::callback(
                                        button_text,
                                        callback_data,
                                    ));

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

    // Pitch buttons row
    let pitch_values = [-2, -1, 0, 1, 2];
    let pitch_row: Vec<InlineKeyboardButton> = pitch_values
        .iter()
        .map(|&value| {
            let marker = if current_pitch == value { " ✓" } else { "" };
            let label = if value > 0 {
                format!("Pitch: +{}{}", value, marker)
            } else {
                format!("Pitch: {}{}", value, marker)
            };
            InlineKeyboardButton::callback(label, format!("ae:pitch:{}:{}", session_id, value))
        })
        .collect();

    // Tempo buttons row
    let tempo_values = [0.5, 0.75, 1.0, 1.5, 2.0];
    let tempo_row: Vec<InlineKeyboardButton> = tempo_values
        .iter()
        .map(|&value| {
            let marker = if (current_tempo - value).abs() < 0.01 {
                " ✓"
            } else {
                ""
            };
            let label = format!("Tempo: {}x{}", value, marker);
            InlineKeyboardButton::callback(label, format!("ae:tempo:{}:{}", session_id, value))
        })
        .collect();

    // Action buttons row
    let action_row = vec![
        InlineKeyboardButton::callback("✅ Apply Changes", format!("ae:apply:{}", session_id)),
        InlineKeyboardButton::callback("🔄 Reset", format!("ae:reset:{}", session_id)),
    ];

    let skip_row = vec![InlineKeyboardButton::callback(
        "⏭️ Skip",
        format!("ae:skip:{}", session_id),
    )];

    // Bass buttons row (dB)
    let bass_values = [-6, -3, 0, 3, 6];
    let bass_row: Vec<InlineKeyboardButton> = bass_values
        .iter()
        .map(|&value| {
            let marker = if current_bass == value { " ✓" } else { "" };
            InlineKeyboardButton::callback(
                format!("Bass {:+}{}", value, marker),
                format!("ae:bass:{}:{:+}", session_id, value),
            )
        })
        .collect();

    // Neural morph row
    let morph_row = vec![InlineKeyboardButton::callback(
        format!(
            "🤖 Morph: {}",
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

    InlineKeyboardMarkup::new(vec![
        pitch_row, tempo_row, bass_row, morph_row, action_row, skip_row,
    ])
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
        Current: Pitch {} \\| Tempo {}x \\| Bass {} \\| Morph {}\n\n\
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
        Current: Pitch {} \\| Tempo {}x \\| Bass {} \\| Morph {}\n\n\
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
            .text("⭐ Эта функция доступна только Premium/VIP подписчикам")
            .show_alert(true)
            .await?;
        return Ok(());
    }

    match action {
        "open" => {
            let session_id = parts.get(2).ok_or("Missing session_id")?;

            let session =
                db::get_audio_effect_session(&conn, session_id)?.ok_or("Session not found")?;

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

            let mut session =
                db::get_audio_effect_session(&conn, session_id)?.ok_or("Session not found")?;

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

            let mut session =
                db::get_audio_effect_session(&conn, session_id)?.ok_or("Session not found")?;

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

            let mut session =
                db::get_audio_effect_session(&conn, session_id)?.ok_or("Session not found")?;

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

            let mut session =
                db::get_audio_effect_session(&conn, session_id)?.ok_or("Session not found")?;

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

            let session =
                db::get_audio_effect_session(&conn, session_id)?.ok_or("Session not found")?;

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
                if let Err(e) = process_audio_effects(
                    bot_clone,
                    chat_id,
                    message_id,
                    session_clone,
                    db_pool_clone,
                )
                .await
                {
                    log::error!("Failed to process audio effects: {}", e);
                }
            });
        }

        "reset" => {
            let session_id = parts.get(2).ok_or("Missing session_id")?;

            let mut session =
                db::get_audio_effect_session(&conn, session_id)?.ok_or("Session not found")?;

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

            let session =
                db::get_audio_effect_session(&conn, session_id)?.ok_or("Session not found")?;

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

            let session =
                db::get_audio_effect_session(&conn, session_id)?.ok_or("Session not found")?;

            bot.answer_callback_query(callback_id).await?;

            // Send original file
            if std::path::Path::new(&session.original_file_path).exists() {
                let file = teloxide::types::InputFile::file(&session.original_file_path);
                bot.send_audio(chat_id, file)
                    .title(format!("{} (Original)", session.title))
                    .duration(session.duration)
                    .await?;
            } else {
                bot.send_message(
                    chat_id,
                    "❌ Оригинальный файл не найден. Возможно, сессия истекла.",
                )
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
    let output_path_raw = crate::download::audio_effects::get_modified_file_path(
        &session_id,
        new_version,
        &config::DOWNLOAD_FOLDER,
    );
    let output_path = shellexpand::tilde(&output_path_raw).into_owned();
    if let Some(parent) = Path::new(&output_path).parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    // Apply effects
    let settings = session.settings();
    let result = crate::download::audio_effects::apply_audio_effects(
        &session.original_file_path,
        &output_path,
        &settings,
    )
    .await;

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
                InlineKeyboardButton::callback(
                    "📥 Get Original",
                    format!("ae:original:{}", session_id),
                ),
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
pub async fn show_enhanced_main_menu(
    bot: &Bot,
    chat_id: ChatId,
    db_pool: Arc<DbPool>,
) -> ResponseResult<Message> {
    let conn = db::get_connection(&db_pool).map_err(|e| {
        RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string())))
    })?;

    let format =
        db::get_user_download_format(&conn, chat_id.0).unwrap_or_else(|_| "mp3".to_string());
    let video_quality =
        db::get_user_video_quality(&conn, chat_id.0).unwrap_or_else(|_| "best".to_string());
    let audio_bitrate =
        db::get_user_audio_bitrate(&conn, chat_id.0).unwrap_or_else(|_| "320k".to_string());

    // Get user plan from database
    let plan = match db::get_user(&conn, chat_id.0) {
        Ok(Some(user)) => user.plan,
        _ => "free".to_string(),
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
        format!("🎬 Качество: {}", quality_display)
    } else {
        let bitrate_display = match audio_bitrate.as_str() {
            "128k" => "128 kbps",
            "192k" => "192 kbps",
            "256k" => "256 kbps",
            "320k" => "320 kbps",
            _ => "320 kbps",
        };
        format!("🎵 Битрейт: {}", bitrate_display)
    };

    // Plan display
    let plan_display = match plan.as_str() {
        "premium" => "Premium ⭐",
        "vip" => "VIP 💎",
        _ => "Free",
    };

    let text = format!(
        "Хэй\\! Я Дора ❤️‍🔥\n\n\
        Просто отправь мне ссылку, и я скачаю тебе видео или трек\\!\n\n\
        *Твои текущие настройки:*\n\
        📥 Формат: {}\n\
        {}\n\
        💎 План: {}\n\n\
        Выбери действие ниже или отправь ссылку прямо сейчас\\!",
        format_emoji, quality_line, plan_display
    );

    let keyboard = InlineKeyboardMarkup::new(vec![
        vec![
            InlineKeyboardButton::callback("⚙️ Настройки загрузки".to_string(), "main:settings"),
            InlineKeyboardButton::callback("🎬 Мои настройки".to_string(), "main:current"),
        ],
        vec![
            InlineKeyboardButton::callback("📊 Моя статистика".to_string(), "main:stats"),
            InlineKeyboardButton::callback("📜 История загрузок".to_string(), "main:history"),
        ],
        vec![
            InlineKeyboardButton::callback("🌐 Доступные сервисы".to_string(), "main:services"),
            InlineKeyboardButton::callback("💎 Подписка".to_string(), "main:subscription"),
        ],
        vec![InlineKeyboardButton::callback(
            "❓ Помощь и FAQ".to_string(),
            "main:help",
        )],
    ]);

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
    let conn = db::get_connection(&db_pool).map_err(|e| {
        RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string())))
    })?;

    let format =
        db::get_user_download_format(&conn, chat_id.0).unwrap_or_else(|_| "mp3".to_string());
    let video_quality =
        db::get_user_video_quality(&conn, chat_id.0).unwrap_or_else(|_| "best".to_string());
    let audio_bitrate =
        db::get_user_audio_bitrate(&conn, chat_id.0).unwrap_or_else(|_| "320k".to_string());

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

    let quality_line = if format == "mp4" {
        let quality_display = match video_quality.as_str() {
            "1080p" => "1080p",
            "720p" => "720p",
            "480p" => "480p",
            "360p" => "360p",
            _ => "Best",
        };
        format!("🎬 Качество: {}", quality_display)
    } else {
        let bitrate_display = match audio_bitrate.as_str() {
            "128k" => "128 kbps",
            "192k" => "192 kbps",
            "256k" => "256 kbps",
            "320k" => "320 kbps",
            _ => "320 kbps",
        };
        format!("🎵 Битрейт: {}", bitrate_display)
    };

    let plan_display = match plan.as_str() {
        "premium" => "Premium ⭐",
        "vip" => "VIP 💎",
        _ => "Free",
    };

    let text = format!(
        "Хэй\\! Я Дора ❤️‍🔥\n\n\
        Просто отправь мне ссылку, и я скачаю тебе видео или трек\\!\n\n\
        *Твои текущие настройки:*\n\
        📥 Формат: {}\n\
        {}\n\
        💎 План: {}\n\n\
        Выбери действие ниже или отправь ссылку прямо сейчас\\!",
        format_emoji, quality_line, plan_display
    );

    let keyboard = InlineKeyboardMarkup::new(vec![
        vec![
            InlineKeyboardButton::callback("⚙️ Настройки загрузки".to_string(), "main:settings"),
            InlineKeyboardButton::callback("🎬 Мои настройки".to_string(), "main:current"),
        ],
        vec![
            InlineKeyboardButton::callback("📊 Моя статистика".to_string(), "main:stats"),
            InlineKeyboardButton::callback("📜 История загрузок".to_string(), "main:history"),
        ],
        vec![
            InlineKeyboardButton::callback("🌐 Доступные сервисы".to_string(), "main:services"),
            InlineKeyboardButton::callback("💎 Подписка".to_string(), "main:subscription"),
        ],
        vec![InlineKeyboardButton::callback(
            "❓ Помощь и FAQ".to_string(),
            "main:help",
        )],
    ]);

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
    let conn = db::get_connection(&db_pool).map_err(|e| {
        RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string())))
    })?;

    let format =
        db::get_user_download_format(&conn, chat_id.0).unwrap_or_else(|_| "mp3".to_string());
    let video_quality =
        db::get_user_video_quality(&conn, chat_id.0).unwrap_or_else(|_| "best".to_string());
    let audio_bitrate =
        db::get_user_audio_bitrate(&conn, chat_id.0).unwrap_or_else(|_| "320k".to_string());
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
    let text = "❓ *Помощь и FAQ*\n\n\
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
        *Нужна помощь?*\n\
        Напиши @stansob \\(администратор\\)";

    let keyboard = InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::callback(
        "🔙 Назад в меню".to_string(),
        "back:enhanced_main",
    )]]);

    edit_caption_or_text(bot, chat_id, message_id, text.to_string(), Some(keyboard)).await
}
