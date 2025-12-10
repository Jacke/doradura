use crate::core::config::admin::ADMIN_USERNAME;
use crate::storage::db::DbPool;
use std::sync::Arc;
use teloxide::prelude::*;

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
) {
    // Try to find the admin's ChatId by username
    let admin_chat_id = match crate::storage::db::get_connection(&db_pool) {
        Ok(conn) => {
            // Look for a user with username = ADMIN_USERNAME
            match crate::storage::db::get_all_users(&conn) {
                Ok(users) => users
                    .iter()
                    .find(|u| u.username.as_deref() == Some(ADMIN_USERNAME.as_str()))
                    .map(|u| teloxide::types::ChatId(u.telegram_id)),
                Err(e) => {
                    log::error!("Failed to get users for admin notification: {}", e);
                    None
                }
            }
        }
        Err(e) => {
            log::error!("Failed to get DB connection for admin notification: {}", e);
            None
        }
    };

    if let Some(chat_id) = admin_chat_id {
        // Escape special characters for MarkdownV2
        let escaped_error = error_message
            .replace("_", "\\_")
            .replace("*", "\\*")
            .replace("[", "\\[")
            .replace("]", "\\]")
            .replace("(", "\\(")
            .replace(")", "\\)")
            .replace("~", "\\~")
            .replace("`", "\\`")
            .replace(">", "\\>")
            .replace("#", "\\#")
            .replace("+", "\\+")
            .replace("-", "\\-")
            .replace("=", "\\=")
            .replace("|", "\\|")
            .replace("{", "\\{")
            .replace("}", "\\}")
            .replace(".", "\\.")
            .replace("!", "\\!");
        let escaped_url = url.replace("_", "\\_").replace(".", "\\.");

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
    } else {
        log::warn!(
            "Admin user '{}' not found in database. Notification not sent for task {}",
            ADMIN_USERNAME.as_str(),
            task_id
        );
    }
}
