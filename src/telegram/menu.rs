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
        RequestError::from(std::sync::Arc::new(std::io::Error::new(
            std::io::ErrorKind::Other,
            e.to_string(),
        )))
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
        RequestError::from(std::sync::Arc::new(std::io::Error::new(
            std::io::ErrorKind::Other,
            e.to_string(),
        )))
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
    bot.edit_message_text(
        chat_id,
        message_id,
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
        RequestError::from(std::sync::Arc::new(std::io::Error::new(
            std::io::ErrorKind::Other,
            e.to_string(),
        )))
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
        RequestError::from(std::sync::Arc::new(std::io::Error::new(
            std::io::ErrorKind::Other,
            e.to_string(),
        )))
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
    bot.edit_message_text(
        chat_id,
        message_id,
        format!(
            "Выбери качество видео\\:\n\n*Текущее качество\\: {}*\n*Тип отправки\\: {}*",
            escaped_quality, escaped_send_type
        ),
    )
    .parse_mode(teloxide::types::ParseMode::MarkdownV2)
    .reply_markup(keyboard)
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
        RequestError::from(std::sync::Arc::new(std::io::Error::new(
            std::io::ErrorKind::Other,
            e.to_string(),
        )))
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
    bot.edit_message_text(
        chat_id,
        message_id,
        format!(
            "Выбери битрейт для аудио\\:\n\n*Текущий битрейт\\: {}*\n*Тип отправки\\: {}*",
            escaped_bitrate, escaped_send_type
        ),
    )
    .parse_mode(teloxide::types::ParseMode::MarkdownV2)
    .reply_markup(keyboard)
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
        "back:main",
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

    bot.edit_message_text(chat_id, message_id, text)
        .parse_mode(teloxide::types::ParseMode::MarkdownV2)
        .reply_markup(keyboard)
        .await?;
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
        RequestError::from(std::sync::Arc::new(std::io::Error::new(
            std::io::ErrorKind::Other,
            e.to_string(),
        )))
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

    bot.edit_message_text(
        chat_id,
        message_id,
        "🎵 *Дора \\- Режимы Загрузки*\n\nВыбери, что хочешь настроить\\!",
    )
    .parse_mode(teloxide::types::ParseMode::MarkdownV2)
    .reply_markup(keyboard)
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
        RequestError::from(std::sync::Arc::new(std::io::Error::new(
            std::io::ErrorKind::Other,
            e.to_string(),
        )))
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
    if let Some(data) = q.data {
        let chat_id = q.message.as_ref().map(|m| m.chat().id);
        let message_id = q.message.as_ref().map(|m| m.id());

        if let (Some(chat_id), Some(message_id)) = (chat_id, message_id) {
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
                    parts[4]
                        .parse::<i32>()
                        .ok()
                        .map(|id| teloxide::types::MessageId(id))
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
            } else if data.starts_with("subscribe:") {
                log::info!("🔔 Subscribe callback received: data={}, chat_id={}", data, chat_id.0);
                bot.answer_callback_query(callback_id.clone()).await?;
                let plan = &data[10..]; // Remove "subscribe:" prefix
                log::info!("📌 Extracted plan: {}", plan);
                match plan {
                    "premium" | "vip" => {
                        log::info!("✅ Valid plan '{}', creating invoice for chat_id={}", plan, chat_id.0);
                        // Создаем инвойс для оплаты через Telegram Stars
                        match create_subscription_invoice(&bot, chat_id, plan).await {
                            Ok(msg) => {
                                log::info!("✅ Invoice created successfully for user {} plan {}. Message ID: {}", chat_id.0, plan, msg.id.0);
                            }
                            Err(e) => {
                                log::error!("❌ Failed to create invoice for user {} plan {}: {:?}", chat_id.0, plan, e);
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
            } else if data.starts_with("subscription:") {
                bot.answer_callback_query(callback_id.clone()).await?;
                let action = &data[13..]; // Remove "subscription:" prefix
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
                                let _ = show_subscription_info(&bot, chat_id, Arc::clone(&db_pool)).await;
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
            } else if data.starts_with("quality:") {
                let _ = bot.answer_callback_query(callback_id.clone()).await;
                let quality = &data[8..]; // Remove "quality:" prefix
                let conn = db::get_connection(&db_pool).map_err(|e| {
                    RequestError::from(std::sync::Arc::new(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        e.to_string(),
                    )))
                })?;
                db::set_user_video_quality(&conn, chat_id.0, quality).map_err(|e| {
                    RequestError::from(std::sync::Arc::new(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        e.to_string(),
                    )))
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
                    RequestError::from(std::sync::Arc::new(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        e.to_string(),
                    )))
                })?;

                // Получаем текущее значение и переключаем
                let current_value = db::get_user_send_as_document(&conn, chat_id.0).unwrap_or(0);
                let new_value = if current_value == 0 { 1 } else { 0 };

                db::set_user_send_as_document(&conn, chat_id.0, new_value).map_err(|e| {
                    RequestError::from(std::sync::Arc::new(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        e.to_string(),
                    )))
                })?;

                // Обновляем меню
                show_video_quality_menu(&bot, chat_id, message_id, Arc::clone(&db_pool), None)
                    .await?;
            } else if data.starts_with("bitrate:") {
                let _ = bot.answer_callback_query(callback_id.clone()).await;
                let bitrate = &data[8..]; // Remove "bitrate:" prefix
                let conn = db::get_connection(&db_pool).map_err(|e| {
                    RequestError::from(std::sync::Arc::new(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        e.to_string(),
                    )))
                })?;
                db::set_user_audio_bitrate(&conn, chat_id.0, bitrate).map_err(|e| {
                    RequestError::from(std::sync::Arc::new(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        e.to_string(),
                    )))
                })?;

                // Update the menu to show new selection
                show_audio_bitrate_menu(&bot, chat_id, message_id, Arc::clone(&db_pool), None)
                    .await?;
            } else if data == "audio_send_type:toggle" {
                let _ = bot.answer_callback_query(callback_id.clone()).await;
                let conn = db::get_connection(&db_pool).map_err(|e| {
                    RequestError::from(std::sync::Arc::new(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        e.to_string(),
                    )))
                })?;

                // Получаем текущее значение и переключаем
                let current_value =
                    db::get_user_send_audio_as_document(&conn, chat_id.0).unwrap_or(0);
                let new_value = if current_value == 0 { 1 } else { 0 };

                db::set_user_send_audio_as_document(&conn, chat_id.0, new_value).map_err(|e| {
                    RequestError::from(std::sync::Arc::new(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        e.to_string(),
                    )))
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
                        RequestError::from(std::sync::Arc::new(std::io::Error::new(
                            std::io::ErrorKind::Other,
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
                        RequestError::from(std::sync::Arc::new(std::io::Error::new(
                            std::io::ErrorKind::Other,
                            e.to_string(),
                        )))
                    })?;

                    // Получаем текущую клавиатуру из сообщения и обновляем только toggle кнопку
                    if let Some(msg) = q.message.as_ref() {
                        if let teloxide::types::MaybeInaccessibleMessage::Regular(regular_msg) = msg
                        {
                            // Получаем текущую клавиатуру
                            if let Some(keyboard) = regular_msg.reply_markup() {
                                // Клонируем клавиатуру и обновляем toggle кнопку
                                let mut new_buttons = keyboard.inline_keyboard.clone();

                                // Находим и обновляем toggle кнопку (ищем кнопку с callback video_send_type:toggle)
                                for row in &mut new_buttons {
                                    for button in row {
                                        if let teloxide::types::InlineKeyboardButtonKind::CallbackData(ref cb_data) = button.kind {
                                            if cb_data.starts_with("video_send_type:toggle:") {
                                                // Обновляем текст кнопки
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
                }
            } else if data.starts_with("back:") {
                let _ = bot.answer_callback_query(callback_id.clone()).await;
                if data.starts_with("back:preview:") {
                    // Format: back:preview:url_id or back:preview:url_id:preview_msg_id
                    let parts: Vec<&str> = data.split(':').collect();
                    let url_id = parts[2];
                    let preview_msg_id = if parts.len() >= 4 {
                        parts[3]
                            .parse::<i32>()
                            .ok()
                            .map(|id| teloxide::types::MessageId(id))
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
                                            std::io::Error::new(
                                                std::io::ErrorKind::Other,
                                                e.to_string(),
                                            ),
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

                                    // Delete settings menu
                                    if let Err(e) = bot.delete_message(chat_id, message_id).await {
                                        log::warn!("Failed to delete settings menu: {:?}", e);
                                    }

                                    // Get metadata and send new preview, delete old preview if preview_msg_id is available
                                    match crate::telegram::preview::get_preview_metadata(
                                        &url,
                                        Some(&current_format),
                                        video_quality.as_deref(),
                                    )
                                    .await
                                    {
                                        Ok(metadata) => {
                                            let _ = crate::telegram::preview::send_preview(
                                                &bot,
                                                chat_id,
                                                &url,
                                                &metadata,
                                                &current_format,
                                                video_quality.as_deref(),
                                                preview_msg_id,
                                                Arc::clone(&db_pool),
                                            )
                                            .await;
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
                        parts[4]
                            .parse::<i32>()
                            .ok()
                            .map(|id| teloxide::types::MessageId(id))
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
                let preview_msg_id = if is_from_preview && parts.len() >= 5 {
                    parts[4]
                        .parse::<i32>()
                        .ok()
                        .map(|id| teloxide::types::MessageId(id))
                } else {
                    None
                };

                let conn = db::get_connection(&db_pool).map_err(|e| {
                    RequestError::from(std::sync::Arc::new(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        e.to_string(),
                    )))
                })?;
                db::set_user_download_format(&conn, chat_id.0, format).map_err(|e| {
                    RequestError::from(std::sync::Arc::new(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        e.to_string(),
                    )))
                })?;

                if is_from_preview && url_id.is_some() {
                    // Delete settings menu
                    if let Err(e) = bot.delete_message(chat_id, message_id).await {
                        log::warn!("Failed to delete settings menu: {:?}", e);
                    }

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
                                            // Send new preview with updated format, delete old preview
                                            match crate::telegram::preview::send_preview(
                                                &bot,
                                                chat_id,
                                                &url,
                                                &metadata,
                                                format,
                                                video_quality.as_deref(),
                                                preview_msg_id,
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
                                            std::io::Error::new(
                                                std::io::ErrorKind::Other,
                                                e.to_string(),
                                            ),
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
                                                .text(&format!(
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
                                            None, // Callback doesn't have original user message
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
            } else if data.starts_with("export:") {
                // Handle export callbacks
                let _ = bot.answer_callback_query(callback_id.clone()).await;
                let format = &data[7..]; // Remove "export:" prefix
                handle_export(&bot, chat_id, format, Arc::clone(&db_pool)).await?;
            } else if data.starts_with("admin:") {
                // Handle admin panel callbacks
                let _ = bot.answer_callback_query(callback_id.clone()).await;

                // Проверка прав администратора
                let is_admin = q.from.username.as_ref()
                    .map(|u| u == "stansob")
                    .unwrap_or(false);

                if !is_admin {
                    bot.send_message(chat_id, "❌ У тебя нет прав для выполнения этой команды.").await?;
                    return Ok(());
                }

                if data.starts_with("admin:user:") {
                    // Показываем меню управления конкретным пользователем
                    let user_id_str = &data[11..]; // Remove "admin:user:" prefix

                    if let Ok(user_id) = user_id_str.parse::<i64>() {
                        match db::get_connection(&db_pool) {
                            Ok(conn) => {
                                match db::get_user(&conn, user_id) {
                                    Ok(Some(user)) => {
                            let username_display = user.username.as_ref()
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

                            let expires_info = if let Some(expires) = &user.subscription_expires_at {
                                format!("\n📅 Истекает: {}", expires)
                            } else {
                                String::new()
                            };

                            // Создаем клавиатуру с действиями
                            use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};

                            let keyboard = InlineKeyboardMarkup::new(vec![
                                vec![
                                    InlineKeyboardButton::callback(
                                        "🌟 Set Free",
                                        format!("admin:setplan:{}:free", user_id)
                                    ),
                                ],
                                vec![
                                    InlineKeyboardButton::callback(
                                        "⭐ Set Premium",
                                        format!("admin:setplan:{}:premium", user_id)
                                    ),
                                ],
                                vec![
                                    InlineKeyboardButton::callback(
                                        "👑 Set VIP",
                                        format!("admin:setplan:{}:vip", user_id)
                                    ),
                                ],
                                vec![
                                    InlineKeyboardButton::callback(
                                        "🔙 Назад к списку",
                                        "admin:back"
                                    ),
                                ],
                            ]);

                            let _ = bot.edit_message_text(
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
                                )
                            )
                            .parse_mode(teloxide::types::ParseMode::Markdown)
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
                                            let _ = bot.send_message(
                                                user_chat_id,
                                                format!(
                                                    "💳 *Изменение плана подписки*\n\n\
                                                    Твой план был изменен администратором.\n\n\
                                                    *Новый план:* {} {}\n\n\
                                                    Изменения вступят в силу немедленно! 🎉",
                                                    plan_emoji,
                                                    plan_name
                                                )
                                            )
                                            .parse_mode(teloxide::types::ParseMode::Markdown)
                                            .await;

                                            let _ = bot.edit_message_text(
                                                chat_id,
                                                message_id,
                                                format!("✅ План пользователя {} изменен на {} {}", user_id, plan_emoji, new_plan)
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
                        Ok(conn) => {
                            match db::get_all_users(&conn) {
                                Ok(users) => {

                    use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};

                    let mut keyboard_rows = Vec::new();
                    let mut current_row = Vec::new();

                    for user in users.iter().take(20) {
                        let username_display = user.username.as_ref()
                            .map(|u| format!("@{}", u))
                            .unwrap_or_else(|| format!("ID:{}", user.telegram_id));

                        let plan_emoji = match user.plan.as_str() {
                            "premium" => "⭐",
                            "vip" => "👑",
                            _ => "🌟",
                        };

                        let button_text = format!("{} {}", plan_emoji, username_display);
                        let callback_data = format!("admin:user:{}", user.telegram_id);

                        current_row.push(InlineKeyboardButton::callback(
                            button_text,
                            callback_data
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

                    let _ = bot.edit_message_text(
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
                        )
                    )
                    .parse_mode(teloxide::types::ParseMode::Markdown)
                    .reply_markup(keyboard)
                    .await;
                                }
                                Err(e) => {
                                    log::error!("Failed to get users: {}", e);
                                }
                            }
                        }
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
