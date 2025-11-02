use teloxide::prelude::*;
use teloxide::types::InputFile;
use teloxide::RequestError;
use crate::db::{self, DbPool};
use std::sync::Arc;

/// Экспортирует историю в TXT формат
fn export_to_txt(entries: &[db::DownloadHistoryEntry]) -> String {
    let mut content = "История загрузок\n".to_string();
    content.push_str("=".repeat(50).as_str());
    content.push_str("\n\n");
    
    for (idx, entry) in entries.iter().enumerate() {
        content.push_str(&format!("{}. {}\n", idx + 1, entry.title));
        content.push_str(&format!("   URL: {}\n", entry.url));
        content.push_str(&format!("   Формат: {}\n", entry.format));
        content.push_str(&format!("   Дата: {}\n", entry.downloaded_at));
        content.push_str("\n");
    }
    
    content
}

/// Экспортирует историю в CSV формат
fn export_to_csv(entries: &[db::DownloadHistoryEntry]) -> String {
    let mut content = "Название,URL,Формат,Дата\n".to_string();
    
    for entry in entries {
        // Экранируем кавычки и запятые в CSV
        let title = entry.title.replace('"', "\"\"").replace('\n', " ");
        let url = entry.url.replace('"', "\"\"").replace('\n', " ");
        content.push_str(&format!("\"{}\",\"{}\",\"{}\",\"{}\"\n", 
            title, url, entry.format, entry.downloaded_at));
    }
    
    content
}

/// Экспортирует историю в JSON формат
fn export_to_json(entries: &[db::DownloadHistoryEntry]) -> Result<String, serde_json::Error> {
    #[derive(serde::Serialize)]
    struct ExportEntry {
        title: String,
        url: String,
        format: String,
        downloaded_at: String,
    }
    
    let export_entries: Vec<ExportEntry> = entries.iter().map(|e| ExportEntry {
        title: e.title.clone(),
        url: e.url.clone(),
        format: e.format.clone(),
        downloaded_at: e.downloaded_at.clone(),
    }).collect();
    
    serde_json::to_string_pretty(&export_entries)
}

/// Показывает меню выбора формата экспорта
pub async fn show_export_menu(bot: &Bot, chat_id: ChatId, db_pool: Arc<DbPool>) -> ResponseResult<Message> {
    let conn = db::get_connection(&db_pool)
        .map_err(|e| RequestError::from(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;
    
    let entries = match db::get_all_download_history(&conn, chat_id.0) {
        Ok(entries) => entries,
        Err(e) => {
            log::error!("Failed to get download history: {}", e);
            return bot.send_message(chat_id, "У меня не получилось загрузить историю 😢 Попробуй позже\\.")
                .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                .await;
        }
    };
    
    if entries.is_empty() {
        return bot.send_message(chat_id, "📚 *Экспорт истории*\n\nУ тебя пока нет загрузок для экспорта\\. Отправь мне ссылку на трек или видео\\!")
            .parse_mode(teloxide::types::ParseMode::MarkdownV2)
            .await;
    }
    
    use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};
    
    let keyboard = InlineKeyboardMarkup::new(vec![
        vec![
            InlineKeyboardButton::callback("📄 TXT".to_string(), "export:txt"),
            InlineKeyboardButton::callback("📊 CSV".to_string(), "export:csv"),
            InlineKeyboardButton::callback("📋 JSON".to_string(), "export:json"),
        ],
    ]);
    
    bot.send_message(chat_id, format!("📚 *Экспорт истории*\n\nНайдено записей: {}\n\nВыбери формат экспорта:", entries.len()))
        .parse_mode(teloxide::types::ParseMode::MarkdownV2)
        .reply_markup(keyboard)
        .await
}

/// Обрабатывает запрос на экспорт в выбранном формате
pub async fn handle_export(
    bot: &Bot,
    chat_id: ChatId,
    format: &str,
    db_pool: Arc<DbPool>,
) -> ResponseResult<()> {
    let conn = db::get_connection(&db_pool)
        .map_err(|e| RequestError::from(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;
    
    let entries = match db::get_all_download_history(&conn, chat_id.0) {
        Ok(entries) => entries,
        Err(e) => {
            log::error!("Failed to get download history: {}", e);
            bot.send_message(chat_id, "У меня не получилось загрузить историю 😢 Попробуй позже\\.")
                .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                .await?;
            return Ok(());
        }
    };
    
    if entries.is_empty() {
        bot.send_message(chat_id, "У тебя нет записей для экспорта\\.")
            .parse_mode(teloxide::types::ParseMode::MarkdownV2)
            .await?;
        return Ok(());
    }
    
    let content = match format {
        "txt" => export_to_txt(&entries),
        "csv" => export_to_csv(&entries),
        "json" => {
            match export_to_json(&entries) {
                Ok(content) => content,
                Err(e) => {
                    log::error!("Failed to export to JSON: {}", e);
                    bot.send_message(chat_id, "Ошибка при создании JSON файла\\.")
                        .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                        .await?;
                    return Ok(());
                }
            }
        }
        _ => {
            bot.send_message(chat_id, "Неизвестный формат экспорта\\.")
                .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                .await?;
            return Ok(());
        }
    };
    
    // Создаем временный файл
    let temp_file = format!("/tmp/doradura_export_{}_{}.{}", 
        chat_id.0, 
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default().as_secs(),
        format);
    
    if let Err(e) = std::fs::write(&temp_file, content) {
        log::error!("Failed to write export file: {}", e);
        bot.send_message(chat_id, "Ошибка при создании файла экспорта\\.")
            .parse_mode(teloxide::types::ParseMode::MarkdownV2)
            .await?;
        return Ok(());
    }
    
    // Отправляем файл
    match bot.send_document(chat_id, InputFile::file(&temp_file))
        .await
    {
        Ok(_) => {
            // Удаляем временный файл
            let _ = std::fs::remove_file(&temp_file);
        }
        Err(e) => {
            log::error!("Failed to send export file: {:?}", e);
            let _ = std::fs::remove_file(&temp_file);
            bot.send_message(chat_id, "Ошибка при отправке файла\\.")
                .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                .await?;
        }
    }
    
    Ok(())
}

