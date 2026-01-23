use crate::core::escape_markdown;
use crate::storage::db::{self, DbPool};
use crate::telegram::Bot;
use chrono::NaiveDateTime;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{CallbackQueryId, ChatId, InlineKeyboardButton, InlineKeyboardMarkup, MessageId};
use teloxide::RequestError;
use url::Url;

/// Форматирует дату для отображения
fn format_date(date_str: &str) -> String {
    // Парсим дату из SQLite формата (YYYY-MM-DD HH:MM:SS)
    if let Ok(dt) = NaiveDateTime::parse_from_str(date_str, "%Y-%m-%d %H:%M:%S") {
        use chrono::Datelike;
        use chrono::Timelike;
        // Форматируем в русский формат
        format!(
            "{} {}, {:02}:{:02}",
            match dt.month() {
                1 => "янв",
                2 => "фев",
                3 => "мар",
                4 => "апр",
                5 => "май",
                6 => "июн",
                7 => "июл",
                8 => "авг",
                9 => "сен",
                10 => "окт",
                11 => "ноя",
                12 => "дек",
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

/// Количество записей на одной странице истории
const ITEMS_PER_PAGE: usize = 5;

/// Показывает историю загрузок пользователя с пагинацией
pub async fn show_history(bot: &Bot, chat_id: ChatId, db_pool: Arc<DbPool>) -> ResponseResult<Message> {
    show_history_page(bot, chat_id, db_pool, 0).await
}

/// Показывает конкретную страницу истории загрузок
pub async fn show_history_page(
    bot: &Bot,
    chat_id: ChatId,
    db_pool: Arc<DbPool>,
    page: usize,
) -> ResponseResult<Message> {
    let lang = crate::i18n::user_lang_from_pool(&db_pool, chat_id.0);

    let conn = db::get_connection(&db_pool)
        .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

    // Получаем все записи истории для подсчета страниц
    let all_entries = match db::get_download_history(&conn, chat_id.0, None) {
        Ok(entries) => entries,
        Err(e) => {
            log::error!("Failed to get download history: {}", e);
            return bot
                .send_message(chat_id, crate::i18n::t(&lang, "history.load_failed"))
                .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                .await;
        }
    };

    if all_entries.is_empty() {
        return bot
            .send_message(
                chat_id,
                "📚 *История загрузок*\n\nУ тебя пока нет загрузок\\. Отправь мне ссылку на трек или видео\\!",
            )
            .parse_mode(teloxide::types::ParseMode::MarkdownV2)
            .await;
    }

    let total_items = all_entries.len();
    let total_pages = total_items.div_ceil(ITEMS_PER_PAGE);
    let current_page = page.min(total_pages - 1);

    let start_idx = current_page * ITEMS_PER_PAGE;
    let end_idx = (start_idx + ITEMS_PER_PAGE).min(total_items);
    let entries = &all_entries[start_idx..end_idx];

    let mut text = format!(
        "📚 *История загрузок*\n_Страница {} из {}_\n\n",
        current_page + 1,
        total_pages
    );
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

        text.push_str(&format!(
            "*{}*\\. {} {}\n📅 {}\n",
            start_idx + idx + 1,
            format_emoji,
            escaped_title,
            escaped_date
        ));

        // Сохраняем URL в кэше и получаем короткий ID
        let url_id = crate::storage::cache::store_url(&db_pool, &entry.url).await;
        let callback_data = format!("history:repeat:{}:{}", entry.id, url_id);
        let delete_callback = format!("history:delete:{}", entry.id);

        // Укороченное название для трека (первые 20 символов)
        let short_title = if entry.title.len() > 20 {
            format!("{}...", &entry.title.chars().take(20).collect::<String>())
        } else {
            entry.title.clone()
        };

        // Одна строка с двумя кнопками: повтор и удаление
        keyboard_rows.push(vec![
            InlineKeyboardButton::callback(format!("🔄 {}", short_title), callback_data),
            InlineKeyboardButton::callback("🗑️".to_string(), delete_callback),
        ]);

        // Добавляем визуальный разделитель в текст между записями (кроме последней)
        if idx < entries.len() - 1 {
            text.push_str("───────────────\n");
        } else {
            text.push('\n');
        }
    }

    // Кнопки навигации
    let mut nav_buttons = Vec::new();

    if current_page > 0 {
        nav_buttons.push(InlineKeyboardButton::callback(
            "⬅️".to_string(),
            format!("history:page:{}", current_page - 1),
        ));
    }

    // Показываем номер страницы как неактивную кнопку (callback не будет обрабатываться)
    if total_pages > 1 {
        nav_buttons.push(InlineKeyboardButton::callback(
            format!("{}/{}", current_page + 1, total_pages),
            format!("history:page:{}", current_page), // Клик на текущую страницу не делает ничего
        ));
    }

    if current_page < total_pages - 1 {
        nav_buttons.push(InlineKeyboardButton::callback(
            "➡️".to_string(),
            format!("history:page:{}", current_page + 1),
        ));
    }

    if !nav_buttons.is_empty() {
        keyboard_rows.push(nav_buttons);
    }

    keyboard_rows.push(vec![InlineKeyboardButton::callback(
        "🔙 В главное меню".to_string(),
        "back:start",
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
    download_queue: Arc<crate::download::queue::DownloadQueue>,
    rate_limiter: Arc<crate::core::rate_limiter::RateLimiter>,
) -> ResponseResult<()> {
    let lang = crate::i18n::user_lang_from_pool(&db_pool, chat_id.0);

    let parts: Vec<&str> = data.splitn(3, ':').collect();
    if parts.len() < 3 {
        bot.answer_callback_query(callback_id)
            .text(crate::i18n::t(&lang, "history.invalid_format"))
            .await?;
        return Ok(());
    }

    let action = parts[1];

    match action {
        "page" => {
            // Формат: history:page:page_number
            let page_str = parts[2];

            match page_str.parse::<usize>() {
                Ok(page) => {
                    // Получаем текущую страницу из сообщения для проверки
                    // Если это та же страница, просто отвечаем на callback
                    bot.answer_callback_query(callback_id.clone()).await?;

                    // Удаляем текущее сообщение
                    if let Err(e) = bot.delete_message(chat_id, message_id).await {
                        log::warn!("Failed to delete history message: {:?}", e);
                    }

                    // Показываем новую страницу
                    show_history_page(bot, chat_id, db_pool, page).await?;
                }
                Err(e) => {
                    log::error!("Failed to parse page number: {}", e);
                    bot.answer_callback_query(callback_id)
                        .text(crate::i18n::t(&lang, "history.invalid_page"))
                        .await?;
                }
            }
        }
        "repeat" => {
            // Формат: history:repeat:entry_id:url_id
            let entry_id_str = parts[2].split(':').next().unwrap_or("");
            let url_id = parts[2].split_once(':').map(|x| x.1).unwrap_or("");

            // Сначала пробуем отправить по file_id, если он есть
            let mut file_sent = false;
            if let Ok(entry_id) = entry_id_str.parse::<i64>() {
                if let Ok(conn) = db::get_connection(&db_pool) {
                    if let Ok(Some(entry)) = db::get_download_history_entry(&conn, chat_id.0, entry_id) {
                        if let Some(file_id) = entry.file_id {
                            log::info!("Found file_id for history entry {}: {}", entry_id, file_id);

                            let result = match entry.format.as_str() {
                                "mp3" => {
                                    bot.send_audio(
                                        chat_id,
                                        teloxide::types::InputFile::file_id(teloxide::types::FileId(file_id.clone())),
                                    )
                                    .await
                                }
                                "mp4" => {
                                    bot.send_video(
                                        chat_id,
                                        teloxide::types::InputFile::file_id(teloxide::types::FileId(file_id.clone())),
                                    )
                                    .await
                                }
                                _ => {
                                    bot.send_document(
                                        chat_id,
                                        teloxide::types::InputFile::file_id(teloxide::types::FileId(file_id)),
                                    )
                                    .await
                                }
                            };

                            match result {
                                Ok(_) => {
                                    log::info!("Successfully resent file using file_id for entry {}", entry_id);
                                    bot.answer_callback_query(callback_id.clone())
                                        .text(crate::i18n::t(&lang, "history.file_sent"))
                                        .await?;
                                    file_sent = true;

                                    // Удаляем сообщение истории
                                    if let Err(e) = bot.delete_message(chat_id, message_id).await {
                                        log::warn!("Failed to delete history message: {:?}", e);
                                    }
                                }
                                Err(e) => {
                                    log::warn!(
                                        "Failed to resend file using file_id: {}. Falling back to re-download.",
                                        e
                                    );
                                }
                            }
                        }
                    }
                }
            }

            if file_sent {
                return Ok(());
            }

            // Получаем URL из кэша (fallback)
            match crate::storage::cache::get_url(&db_pool, url_id).await {
                Some(url_str) => {
                    // URL найден в кэше
                    match Url::parse(&url_str) {
                        Ok(url) => {
                            // Получаем план пользователя для rate limiting
                            let conn = db::get_connection(&db_pool).map_err(|e| {
                                RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string())))
                            })?;
                            let plan = match db::get_user(&conn, chat_id.0) {
                                Ok(Some(ref user)) => user.plan.clone(),
                                _ => "free".to_string(),
                            };

                            // Проверяем rate limit
                            if rate_limiter.is_rate_limited(chat_id, &plan).await {
                                if let Some(remaining_time) = rate_limiter.get_remaining_time(chat_id).await {
                                    let remaining_seconds = remaining_time.as_secs();
                                    let mut args = fluent_templates::fluent_bundle::FluentArgs::new();
                                    args.set("seconds", remaining_seconds);
                                    bot.answer_callback_query(callback_id)
                                        .text(crate::i18n::t_args(&lang, "commands.wait_seconds", &args))
                                        .await?;
                                } else {
                                    bot.answer_callback_query(callback_id)
                                        .text(crate::i18n::t(&lang, "commands.wait"))
                                        .await?;
                                }
                                return Ok(());
                            }

                            bot.answer_callback_query(callback_id.clone()).await?;

                            // Получаем формат из истории
                            let format = match entry_id_str.parse::<i64>() {
                                Ok(id) => match db::get_download_history_entry(&conn, chat_id.0, id) {
                                    Ok(Some(entry)) => entry.format,
                                    _ => "mp3".to_string(),
                                },
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
                            let task = crate::download::queue::DownloadTask::from_plan(
                                url.as_str().to_string(),
                                chat_id,
                                None, // Callback doesn't have original user message
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
                                .text(crate::i18n::t(&lang, "commands.invalid_url"))
                                .await?;
                        }
                    }
                }
                None => {
                    log::warn!("URL not found in cache for id: {} (expired or invalid)", url_id);
                    bot.answer_callback_query(callback_id)
                        .text(crate::i18n::t(&lang, "commands.link_expired"))
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
                        .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

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
                                .text(crate::i18n::t(&lang, "history.entry_not_found"))
                                .await?;
                        }
                        Err(e) => {
                            log::error!("Failed to delete history entry: {}", e);
                            bot.answer_callback_query(callback_id)
                                .text(crate::i18n::t(&lang, "history.delete_failed"))
                                .await?;
                        }
                    }
                }
                Err(e) => {
                    log::error!("Failed to parse entry ID: {}", e);
                    bot.answer_callback_query(callback_id)
                        .text(crate::i18n::t(&lang, "history.invalid_id"))
                        .await?;
                }
            }
        }
        _ => {
            bot.answer_callback_query(callback_id)
                .text(crate::i18n::t(&lang, "history.unknown_action"))
                .await?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escape_markdown_special_chars() {
        assert_eq!(escape_markdown("test_underscore"), "test\\_underscore");
        assert_eq!(escape_markdown("bold*text"), "bold\\*text");
        assert_eq!(escape_markdown("[link]"), "\\[link\\]");
        assert_eq!(escape_markdown("(parens)"), "\\(parens\\)");
        assert_eq!(escape_markdown("~strike~"), "\\~strike\\~");
        assert_eq!(escape_markdown("`code`"), "\\`code\\`");
        assert_eq!(escape_markdown(">quote"), "\\>quote");
        assert_eq!(escape_markdown("#hash"), "\\#hash");
        assert_eq!(escape_markdown("+plus"), "\\+plus");
        assert_eq!(escape_markdown("-dash"), "\\-dash");
        assert_eq!(escape_markdown("=equals"), "\\=equals");
        assert_eq!(escape_markdown("|pipe"), "\\|pipe");
        assert_eq!(escape_markdown("{brace}"), "\\{brace\\}");
        assert_eq!(escape_markdown("period."), "period\\.");
        assert_eq!(escape_markdown("exclaim!"), "exclaim\\!");
    }

    #[test]
    fn test_escape_markdown_backslash() {
        assert_eq!(escape_markdown("back\\slash"), "back\\\\slash");
    }

    #[test]
    fn test_escape_markdown_normal_text() {
        assert_eq!(escape_markdown("normal text"), "normal text");
        assert_eq!(escape_markdown("Hello World"), "Hello World");
        assert_eq!(escape_markdown("12345"), "12345");
    }

    #[test]
    fn test_escape_markdown_empty_string() {
        assert_eq!(escape_markdown(""), "");
    }

    #[test]
    fn test_escape_markdown_multiple_special_chars() {
        assert_eq!(
            escape_markdown("Hello [World]! How are you?"),
            "Hello \\[World\\]\\! How are you?"
        );
    }

    #[test]
    fn test_escape_markdown_unicode() {
        assert_eq!(escape_markdown("Привет мир"), "Привет мир");
        assert_eq!(escape_markdown("🎵 Music"), "🎵 Music");
    }

    #[test]
    fn test_format_date_valid() {
        assert_eq!(format_date("2024-01-15 10:30:00"), "янв 15, 10:30");
        assert_eq!(format_date("2024-06-01 00:00:00"), "июн 1, 00:00");
        assert_eq!(format_date("2024-12-31 23:59:00"), "дек 31, 23:59");
    }

    #[test]
    fn test_format_date_all_months() {
        assert!(format_date("2024-01-01 12:00:00").starts_with("янв"));
        assert!(format_date("2024-02-01 12:00:00").starts_with("фев"));
        assert!(format_date("2024-03-01 12:00:00").starts_with("мар"));
        assert!(format_date("2024-04-01 12:00:00").starts_with("апр"));
        assert!(format_date("2024-05-01 12:00:00").starts_with("май"));
        assert!(format_date("2024-06-01 12:00:00").starts_with("июн"));
        assert!(format_date("2024-07-01 12:00:00").starts_with("июл"));
        assert!(format_date("2024-08-01 12:00:00").starts_with("авг"));
        assert!(format_date("2024-09-01 12:00:00").starts_with("сен"));
        assert!(format_date("2024-10-01 12:00:00").starts_with("окт"));
        assert!(format_date("2024-11-01 12:00:00").starts_with("ноя"));
        assert!(format_date("2024-12-01 12:00:00").starts_with("дек"));
    }

    #[test]
    fn test_format_date_invalid() {
        assert_eq!(format_date("not a date"), "not a date");
        assert_eq!(format_date(""), "");
        assert_eq!(format_date("2024-13-01 12:00:00"), "2024-13-01 12:00:00");
    }

    #[test]
    fn test_format_date_midnight() {
        assert_eq!(format_date("2024-01-01 00:00:00"), "янв 1, 00:00");
    }

    #[test]
    fn test_format_date_end_of_day() {
        assert_eq!(format_date("2024-01-01 23:59:00"), "янв 1, 23:59");
    }

    #[test]
    fn test_items_per_page_constant() {
        assert_eq!(ITEMS_PER_PAGE, 5);
    }
}
