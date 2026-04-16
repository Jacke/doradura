use crate::storage::db::{self, DbPool};
use crate::storage::SharedStorage;
use crate::telegram::Bot;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::InputFile;

/// Exports download history to TXT format
fn export_to_txt(entries: &[db::DownloadHistoryEntry]) -> String {
    let mut content = "Download History\n".to_string();
    content.push_str("=".repeat(50).as_str());
    content.push_str("\n\n");

    for (idx, entry) in entries.iter().enumerate() {
        content.push_str(&format!("{}. {}\n", idx + 1, entry.title));
        content.push_str(&format!("   URL: {}\n", entry.url));
        content.push_str(&format!("   Format: {}\n", entry.format));
        content.push_str(&format!("   Date: {}\n", entry.downloaded_at));
        content.push('\n');
    }

    content
}

/// Exports download history to CSV format
fn export_to_csv(entries: &[db::DownloadHistoryEntry]) -> String {
    let mut content = "Title,URL,Format,Date\n".to_string();

    for entry in entries {
        // Escape quotes and commas for CSV
        let title = entry.title.replace('"', "\"\"").replace('\n', " ");
        let url = entry.url.replace('"', "\"\"").replace('\n', " ");
        content.push_str(&format!(
            "\"{}\",\"{}\",\"{}\",\"{}\"\n",
            title, url, entry.format, entry.downloaded_at
        ));
    }

    content
}

/// Exports download history to JSON format
fn export_to_json(entries: &[db::DownloadHistoryEntry]) -> Result<String, serde_json::Error> {
    #[derive(serde::Serialize)]
    struct ExportEntry {
        title: String,
        url: String,
        format: String,
        downloaded_at: String,
    }

    let export_entries: Vec<ExportEntry> = entries
        .iter()
        .map(|e| ExportEntry {
            title: e.title.clone(),
            url: e.url.clone(),
            format: e.format.clone(),
            downloaded_at: e.downloaded_at.clone(),
        })
        .collect();

    serde_json::to_string_pretty(&export_entries)
}

/// Shows the export format selection menu
pub async fn show_export_menu(
    bot: &Bot,
    chat_id: ChatId,
    _db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
) -> ResponseResult<Message> {
    let entries = match shared_storage.get_all_download_history(chat_id.0).await {
        Ok(entries) => entries,
        Err(e) => {
            log::error!("Failed to get download history: {}", e);
            return bot
                .send_message(chat_id, "Failed to load history 😢 Please try again later\\.")
                .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                .await;
        }
    };

    if entries.is_empty() {
        return bot
            .send_message(
                chat_id,
                "📚 *Export History*\n\nYou have no downloads to export yet\\. Send me a link to a track or video\\!",
            )
            .parse_mode(teloxide::types::ParseMode::MarkdownV2)
            .await;
    }

    use crate::telegram::cb;
    use teloxide::types::InlineKeyboardMarkup;

    let keyboard = InlineKeyboardMarkup::new(vec![vec![
        cb("📄 TXT".to_string(), "export:txt"),
        cb("📊 CSV".to_string(), "export:csv"),
        cb("📋 JSON".to_string(), "export:json"),
    ]]);

    bot.send_message(
        chat_id,
        format!(
            "📚 *Export History*\n\nFound {} records\\.\n\nChoose an export format:",
            entries.len()
        ),
    )
    .parse_mode(teloxide::types::ParseMode::MarkdownV2)
    .reply_markup(keyboard)
    .await
}

/// Handles an export request for the chosen format
pub async fn handle_export(
    bot: &Bot,
    chat_id: ChatId,
    format: &str,
    _db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
) -> ResponseResult<()> {
    let entries = match shared_storage.get_all_download_history(chat_id.0).await {
        Ok(entries) => entries,
        Err(e) => {
            log::error!("Failed to get download history: {}", e);
            bot.send_message(chat_id, "Failed to load history 😢 Please try again later\\.")
                .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                .await?;
            return Ok(());
        }
    };

    if entries.is_empty() {
        bot.send_message(chat_id, "You have no records to export\\.")
            .parse_mode(teloxide::types::ParseMode::MarkdownV2)
            .await?;
        return Ok(());
    }

    let content = match format {
        "txt" => export_to_txt(&entries),
        "csv" => export_to_csv(&entries),
        "json" => match export_to_json(&entries) {
            Ok(content) => content,
            Err(e) => {
                log::error!("Failed to export to JSON: {}", e);
                bot.send_message(chat_id, "Error creating JSON file\\.")
                    .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                    .await?;
                return Ok(());
            }
        },
        _ => {
            bot.send_message(chat_id, "Unknown export format\\.")
                .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                .await?;
            return Ok(());
        }
    };

    // Create a temporary file
    let temp_file = format!(
        "{}/doradura_export_{}_{}.{}",
        crate::core::config::TEMP_FILES_DIR.as_str(),
        chat_id.0,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        format
    );

    if let Err(e) = fs_err::tokio::write(&temp_file, content).await {
        log::error!("Failed to write export file: {}", e);
        bot.send_message(chat_id, "Error creating export file\\.")
            .parse_mode(teloxide::types::ParseMode::MarkdownV2)
            .await?;
        return Ok(());
    }

    // Send the file
    match bot.send_document(chat_id, InputFile::file(&temp_file)).await {
        Ok(_) => {
            // Delete the temporary file
            doracore::core::utils::try_remove_file(&temp_file).await;
        }
        Err(e) => {
            log::error!("Failed to send export file: {:?}", e);
            doracore::core::utils::try_remove_file(&temp_file).await;
            bot.send_message(chat_id, "Error sending file\\.")
                .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                .await?;
        }
    }

    Ok(())
}
