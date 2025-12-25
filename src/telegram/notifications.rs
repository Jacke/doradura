use crate::core::config::admin::ADMIN_USERNAME;
use crate::core::config::admin::ADMIN_USER_ID;
use crate::storage::db::DbPool;
use std::sync::Arc;
use teloxide::prelude::*;

const MAX_MESSAGE_LENGTH: usize = 4000;

fn split_message_for_telegram(text: &str, max_len: usize) -> Vec<String> {
    if text.len() <= max_len {
        return vec![text.to_string()];
    }

    let mut parts = Vec::new();
    let mut start = 0;

    while start < text.len() {
        let mut end = (start + max_len).min(text.len());
        while end < text.len() && !text.is_char_boundary(end) {
            end -= 1;
        }
        if end == start {
            end = (start + max_len).min(text.len());
            while end < text.len() && !text.is_char_boundary(end) {
                end += 1;
            }
        }

        parts.push(text[start..end].to_string());
        start = end;
    }

    parts
}

async fn send_plain_text_chunks(bot: &Bot, chat_id: ChatId, text: &str) {
    for part in split_message_for_telegram(text, MAX_MESSAGE_LENGTH) {
        if let Err(e) = bot.send_message(chat_id, part).await {
            log::error!("Failed to send admin message chunk: {}", e);
            break;
        }
    }
}

/// Sends a plain-text message to the configured admin (uses `ADMIN_USER_ID`).
pub async fn notify_admin_text(bot: &Bot, text: &str) {
    let admin_id = *ADMIN_USER_ID;
    if admin_id == 0 {
        log::warn!("ADMIN_USER_ID not configured; admin notification skipped");
        return;
    }

    send_plain_text_chunks(bot, ChatId(admin_id), text).await;
}

/// Sends a notification to admin about video processing error
pub async fn notify_admin_video_error(bot: &Bot, user_id: i64, username: Option<&str>, error: &str, context: &str) {
    let admin_id = *ADMIN_USER_ID;
    if admin_id == 0 {
        log::warn!("ADMIN_USER_ID not configured; admin notification skipped");
        return;
    }

    let username_str = username.unwrap_or("unknown");
    let message = format!(
        "⚠️ *Ошибка обработки видео*\n\n\
        👤 User: @{} (ID: {})\n\
        📝 Context: {}\n\n\
        ❌ Error:\n```\n{}\n```",
        username_str, user_id, context, error
    );

    send_plain_text_chunks(bot, ChatId(admin_id), &message).await;
}

/// Sends a notification to the administrator about a task failure.
///
/// # Arguments
///
/// * `bot` - Bot instance used to send messages
/// * `db_pool` - Connection pool used to find the administrator ChatId
/// * `task_id` - Task ID
/// * `user_id` - ID of the user whose task failed
/// * `url` - Task URL
/// * `error_message` - Error message
pub async fn notify_admin_task_failed(
    bot: Bot,
    db_pool: Arc<DbPool>,
    task_id: &str,
    user_id: i64,
    url: &str,
    error_message: &str,
    details: Option<&str>,
) {
    // Prefer direct admin id configuration; fallback to DB lookup by username.
    let admin_chat_id = if *ADMIN_USER_ID != 0 {
        Some(ChatId(*ADMIN_USER_ID))
    } else {
        match crate::storage::db::get_connection(&db_pool) {
            Ok(conn) => match crate::storage::db::get_all_users(&conn) {
                Ok(users) => users
                    .iter()
                    .find(|u| u.username.as_deref() == Some(ADMIN_USERNAME.as_str()))
                    .map(|u| teloxide::types::ChatId(u.telegram_id)),
                Err(e) => {
                    log::error!("Failed to get users for admin notification: {}", e);
                    None
                }
            },
            Err(e) => {
                log::error!("Failed to get DB connection for admin notification: {}", e);
                None
            }
        }
    };

    if let Some(chat_id) = admin_chat_id {
        // Escape special characters for MarkdownV2
        let escaped_error = crate::telegram::admin::escape_markdown(error_message);
        let escaped_url = crate::telegram::admin::escape_markdown(url);

        let message = format!(
            "⚠️ *Ошибка задачи*\n\n\
            Task ID: `{}`\n\
            User ID: `{}`\n\
            URL: {}\n\
            Ошибка: {}\n\n\
            Задача будет повторена автоматически\\.",
            task_id, user_id, escaped_url, escaped_error
        );

        if let Err(e) = bot
            .send_message(chat_id, &message)
            .parse_mode(teloxide::types::ParseMode::MarkdownV2)
            .await
        {
            log::error!("Failed to send admin notification: {}", e);
        } else {
            log::info!("Admin notification sent for task {}", task_id);
        }

        if let Some(details) = details {
            let details_message = format!("Details for task {} (user {}):\n{}", task_id, user_id, details);
            send_plain_text_chunks(&bot, chat_id, &details_message).await;
        }
    } else {
        log::warn!(
            "Admin user '{}' not found in database. Notification not sent for task {}",
            ADMIN_USERNAME.as_str(),
            task_id
        );
    }
}
