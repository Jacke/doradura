use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup, MessageId, ChatId, CallbackQueryId};
use teloxide::RequestError;
use crate::db::{self, DbPool};
use chrono::NaiveDateTime;
use std::sync::Arc;
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

/// Форматирует дату для отображения
fn format_date(date_str: &str) -> String {
    // Парсим дату из SQLite формата (YYYY-MM-DD HH:MM:SS)
    if let Ok(dt) = NaiveDateTime::parse_from_str(date_str, "%Y-%m-%d %H:%M:%S") {
        use chrono::Datelike;
        use chrono::Timelike;
        // Форматируем в русский формат
        format!("{} {}, {:02}:{:02}", 
            match dt.month() {
                1 => "янв", 2 => "фев", 3 => "мар", 4 => "апр",
                5 => "май", 6 => "июн", 7 => "июл", 8 => "авг",
                9 => "сен", 10 => "окт", 11 => "ноя", 12 => "дек",
                _ => "???",
            },
            dt.day(),
            dt.hour(),
            dt.minute()
        )
    } else {
        date_str.to_string()
    }
}

/// Показывает историю загрузок пользователя
pub async fn show_history(bot: &Bot, chat_id: ChatId, db_pool: Arc<DbPool>) -> ResponseResult<Message> {
    let conn = db::get_connection(&db_pool)
        .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))))?;
    
    let entries = match db::get_download_history(&conn, chat_id.0, Some(20)) {
        Ok(entries) => entries,
        Err(e) => {
            log::error!("Failed to get download history: {}", e);
            return bot.send_message(chat_id, "У меня не получилось загрузить историю 😢 Попробуй позже\\.")
                .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                .await;
        }
    };
    
    if entries.is_empty() {
        return bot.send_message(chat_id, "📚 *История загрузок*\n\nУ тебя пока нет загрузок\\. Отправь мне ссылку на трек или видео\\!")
            .parse_mode(teloxide::types::ParseMode::MarkdownV2)
            .await;
    }
    
    let mut text = "📚 *История загрузок*\n\n".to_string();
    let mut keyboard_rows = Vec::new();
    
    for (idx, entry) in entries.iter().enumerate() {
        let format_emoji = match entry.format.as_str() {
            "mp3" => "🎵",
            "mp4" => "🎬",
            "srt" => "📝",
            "txt" => "📄",
            _ => "📦",
        };
        
        let escaped_title = escape_markdown(&entry.title);
        let formatted_date = format_date(&entry.downloaded_at);
        let escaped_date = escape_markdown(&formatted_date);
        
        text.push_str(&format!("{} *{}*\\. {}\n📅 {}\n\n", 
            idx + 1, 
            format_emoji,
            escaped_title,
            escaped_date
        ));
        
        // Сохраняем URL в кэше и получаем короткий ID
        let url_id = crate::cache::store_url(&db_pool, &entry.url).await;
        let callback_data = format!("history:repeat:{}:{}", entry.id, url_id);
        let delete_callback = format!("history:delete:{}", entry.id);
        
        keyboard_rows.push(vec![
            InlineKeyboardButton::callback(
                format!("🔄 Повторить {}", idx + 1),
                callback_data
            ),
            InlineKeyboardButton::callback(
                format!("🗑️ Удалить {}", idx + 1),
                delete_callback
            ),
        ]);
    }
    
    keyboard_rows.push(vec![InlineKeyboardButton::callback(
        "🔙 Назад".to_string(),
        "back:start"
    )]);
    
    let keyboard = InlineKeyboardMarkup::new(keyboard_rows);
    
    bot.send_message(chat_id, text)
        .parse_mode(teloxide::types::ParseMode::MarkdownV2)
        .reply_markup(keyboard)
        .await
}

/// Обрабатывает callback для истории загрузок
pub async fn handle_history_callback(
    bot: &Bot,
    callback_id: CallbackQueryId,
    chat_id: ChatId,
    message_id: MessageId,
    data: &str,
    db_pool: Arc<DbPool>,
    download_queue: Arc<crate::queue::DownloadQueue>,
    rate_limiter: Arc<crate::rate_limiter::RateLimiter>,
) -> ResponseResult<()> {
    let parts: Vec<&str> = data.splitn(3, ':').collect();
    if parts.len() < 3 {
        bot.answer_callback_query(callback_id)
            .text("Ошибка: неверный формат данных")
            .await?;
        return Ok(());
    }
    
    let action = parts[1];
    
    match action {
        "repeat" => {
            // Формат: history:repeat:entry_id:url_id
            let entry_id_str = parts[2].split(':').next().unwrap_or("");
            let url_id = parts[2].splitn(2, ':').nth(1).unwrap_or("");
            
            // Получаем URL из кэша
            match crate::cache::get_url(&db_pool, url_id).await {
                Some(url_str) => {
                    // URL найден в кэше
                    match Url::parse(&url_str) {
                                Ok(url) => {
                                    // Получаем план пользователя для rate limiting
                                    let conn = db::get_connection(&db_pool)
                                        .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))))?;
                                    let plan = match db::get_user(&conn, chat_id.0) {
                                        Ok(Some(ref user)) => user.plan.clone(),
                                        _ => "free".to_string(),
                                    };
                                    
                                    // Проверяем rate limit
                                    if rate_limiter.is_rate_limited(chat_id, &plan).await {
                                        if let Some(remaining_time) = rate_limiter.get_remaining_time(chat_id).await {
                                            let remaining_seconds = remaining_time.as_secs();
                                            bot.answer_callback_query(callback_id)
                                                .text(&format!("Подожди {} секунд", remaining_seconds))
                                                .await?;
                                        } else {
                                            bot.answer_callback_query(callback_id)
                                                .text("Подожди немного")
                                                .await?;
                                        }
                                        return Ok(());
                                    }
                                    
                                    bot.answer_callback_query(callback_id.clone()).await?;
                                    
                                    // Получаем формат из истории
                                    let format = match entry_id_str.parse::<i64>() {
                                        Ok(id) => {
                                            match db::get_download_history_entry(&conn, chat_id.0, id) {
                                                Ok(Some(entry)) => entry.format,
                                                _ => "mp3".to_string(),
                                            }
                                        }
                                        Err(_) => "mp3".to_string(),
                                    };
                                    
                                    rate_limiter.update_rate_limit(chat_id, &plan).await;
                                    
                                    // Get user preferences for quality/bitrate
                                    let video_quality = if format == "mp4" {
                                        match db::get_user_video_quality(&conn, chat_id.0) {
                                            Ok(q) => Some(q),
                                            Err(_) => Some("best".to_string()),
                                        }
                                    } else {
                                        None
                                    };
                                    let audio_bitrate = if format == "mp3" {
                                        match db::get_user_audio_bitrate(&conn, chat_id.0) {
                                            Ok(b) => Some(b),
                                            Err(_) => Some("320k".to_string()),
                                        }
                                    } else {
                                        None
                                    };
                                    
                                    // Добавляем задачу в очередь
                                    let is_video = format == "mp4";
                                    let task = crate::queue::DownloadTask::from_plan(
                                        url.as_str().to_string(),
                                        chat_id,
                                        is_video,
                                        format.clone(),
                                        video_quality,
                                        audio_bitrate,
                                        &plan,
                                    );
                                    download_queue.add_task(task, Some(Arc::clone(&db_pool))).await;
                                    
                                    // Удаляем сообщение истории
                                    if let Err(e) = bot.delete_message(chat_id, message_id).await {
                                        log::warn!("Failed to delete history message: {:?}", e);
                                    }
                                }
                                Err(e) => {
                                    log::error!("Failed to parse URL: {}", e);
                                    bot.answer_callback_query(callback_id)
                                        .text("Ошибка: неверная ссылка")
                                        .await?;
                                }
                            }
                        }
                None => {
                    log::warn!("URL not found in cache for id: {} (expired or invalid)", url_id);
                    bot.answer_callback_query(callback_id)
                        .text("Ссылка устарела, попробуйте снова")
                        .await?;
                }
            }
        }
        "delete" => {
            // Формат: history:delete:entry_id
            let entry_id_str = parts[2];
            
            match entry_id_str.parse::<i64>() {
                Ok(entry_id) => {
                    let conn = db::get_connection(&db_pool)
                        .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))))?;
                    
                    match db::delete_download_history_entry(&conn, chat_id.0, entry_id) {
                        Ok(true) => {
                            bot.answer_callback_query(callback_id.clone()).await?;
                            
                            // Обновляем сообщение истории
                            show_history(bot, chat_id, db_pool).await?;
                            
                            // Удаляем старое сообщение
                            if let Err(e) = bot.delete_message(chat_id, message_id).await {
                                log::warn!("Failed to delete old history message: {:?}", e);
                            }
                        }
                        Ok(false) => {
                            bot.answer_callback_query(callback_id)
                                .text("Запись не найдена")
                                .await?;
                        }
                        Err(e) => {
                            log::error!("Failed to delete history entry: {}", e);
                            bot.answer_callback_query(callback_id)
                                .text("Ошибка при удалении")
                                .await?;
                        }
                    }
                }
                Err(e) => {
                    log::error!("Failed to parse entry ID: {}", e);
                    bot.answer_callback_query(callback_id)
                        .text("Ошибка: неверный ID записи")
                        .await?;
                }
            }
        }
        _ => {
            bot.answer_callback_query(callback_id)
                .text("Неизвестное действие")
                .await?;
        }
    }
    
    Ok(())
}

