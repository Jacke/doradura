//! Admin functionality for the Telegram bot
//!
//! This module contains all admin-related commands and utilities:
//! - User management (/users, /setplan, /admin)
//! - Database backup operations
//! - Markdown escaping utilities

use anyhow::Result;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup, ParseMode};

use crate::core::config::admin::ADMIN_USERNAME;
use crate::storage::backup::{create_backup, list_backups};
use crate::storage::db::{get_all_users, get_connection, update_user_plan, DbPool};

/// Maximum message length for Telegram (with margin)
const MAX_MESSAGE_LENGTH: usize = 4000;

/// Check if user is admin
pub fn is_admin(username: Option<&str>) -> bool {
    username.map(|u| u == ADMIN_USERNAME.as_str()).unwrap_or(false)
}

/// Escapes special characters for MarkdownV2 format
///
/// # Arguments
/// * `text` - Text to escape
///
/// # Returns
/// Escaped text safe for MarkdownV2 parsing
pub fn escape_markdown(text: &str) -> String {
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

/// Handle /backup command - create database backup
///
/// # Arguments
/// * `bot` - Bot instance
/// * `chat_id` - Chat ID where to send response
/// * `username` - Username of the requester
pub async fn handle_backup_command(bot: &Bot, chat_id: ChatId, username: Option<&str>) -> Result<()> {
    if !is_admin(username) {
        bot.send_message(chat_id, "❌ У тебя нет прав для выполнения этой команды.")
            .await?;
        return Ok(());
    }

    match create_backup("database.sqlite") {
        Ok(backup_path) => {
            let backups = list_backups().unwrap_or_default();
            bot.send_message(
                chat_id,
                format!(
                    "✅ Бэкап создан успешно!\n\n📁 Путь: {}\n📊 Всего бэкапов: {}",
                    backup_path.display(),
                    backups.len()
                ),
            )
            .await?;
        }
        Err(e) => {
            bot.send_message(chat_id, format!("❌ Ошибка при создании бэкапа: {}", e))
                .await?;
        }
    }

    Ok(())
}

/// Handle /users command - show list of all users
///
/// # Arguments
/// * `bot` - Bot instance
/// * `chat_id` - Chat ID where to send response
/// * `username` - Username of the requester
/// * `db_pool` - Database connection pool
pub async fn handle_users_command(
    bot: &Bot,
    chat_id: ChatId,
    username: Option<&str>,
    db_pool: Arc<DbPool>,
) -> Result<()> {
    log::debug!(
        "Users command: username={:?}, is_admin={}",
        username,
        is_admin(username)
    );

    if !is_admin(username) {
        log::warn!("User {:?} tried to access /users command without permission", username);
        bot.send_message(chat_id, "❌ У тебя нет прав для выполнения этой команды.")
            .await?;
        return Ok(());
    }

    let conn = get_connection(&db_pool)?;
    let users = get_all_users(&conn)?;

    log::debug!("Found {} users in database", users.len());

    if users.is_empty() {
        bot.send_message(
            chat_id,
            "👥 *Список пользователей*\n\nВ базе данных пока нет пользователей\\.",
        )
        .parse_mode(ParseMode::MarkdownV2)
        .await?;
        return Ok(());
    }

    // Calculate statistics
    let free_count = users.iter().filter(|u| u.plan == "free").count();
    let premium_count = users.iter().filter(|u| u.plan == "premium").count();
    let vip_count = users.iter().filter(|u| u.plan == "vip").count();
    let with_subscription = users.iter().filter(|u| u.telegram_charge_id.is_some()).count();

    let total_users = escape_markdown(&users.len().to_string());
    let free_escaped = escape_markdown(&free_count.to_string());
    let premium_escaped = escape_markdown(&premium_count.to_string());
    let vip_escaped = escape_markdown(&vip_count.to_string());
    let subs_escaped = escape_markdown(&with_subscription.to_string());

    let mut text = format!(
        "👥 *Список пользователей* \\(всего\\: {}\\)\n\n\
        📊 Статистика:\n\
        • 🌟 Free: {}\n\
        • ⭐ Premium: {}\n\
        • 👑 VIP: {}\n\
        • 💫 Активных подписок: {}\n\n\
        ━━━━━━━━━━━━━━━━━━━━\n\n",
        total_users, free_escaped, premium_escaped, vip_escaped, subs_escaped
    );

    let mut users_added = 0;

    for (idx, user) in users.iter().enumerate() {
        let username_str = user
            .username
            .as_ref()
            .map(|u| {
                let escaped = escape_markdown(u);
                format!("@{}", escaped)
            })
            .unwrap_or_else(|| {
                let id_escaped = escape_markdown(&user.telegram_id.to_string());
                format!("ID\\: {}", id_escaped)
            });

        let plan_emoji = match user.plan.as_str() {
            "premium" => "⭐",
            "vip" => "👑",
            _ => "🌟",
        };

        let sub_icon = if user.telegram_charge_id.is_some() { " 💫" } else { "" };

        let plan_escaped = escape_markdown(&user.plan);
        let idx_escaped = escape_markdown(&(idx + 1).to_string());
        let user_line = format!(
            "{}\\. {} {} {}{}\n",
            idx_escaped, username_str, plan_emoji, plan_escaped, sub_icon
        );

        // Check if adding this line would exceed the limit
        if text.len() + user_line.len() > MAX_MESSAGE_LENGTH {
            let remaining = escape_markdown(&(users.len() - users_added).to_string());
            text.push_str(&format!("\n\\.\\.\\. и еще {} пользователей", remaining));
            break;
        }

        text.push_str(&user_line);
        users_added += 1;
    }

    log::debug!(
        "Sending users list with {} users (text length: {})",
        users_added,
        text.len()
    );

    match bot.send_message(chat_id, &text).parse_mode(ParseMode::MarkdownV2).await {
        Ok(_) => {
            log::debug!("Successfully sent users list");
        }
        Err(e) => {
            log::error!("Failed to send users list: {:?}", e);
            // Try sending without Markdown if there was a formatting error
            let text_plain = text.replace("\\", "").replace("*", "");
            bot.send_message(
                chat_id,
                format!(
                    "❌ Ошибка при отправке списка. Попробую без форматирования:\n\n{}",
                    text_plain
                ),
            )
            .await?;
        }
    }

    Ok(())
}

/// Handle /setplan command - change user's subscription plan
///
/// # Arguments
/// * `bot` - Bot instance
/// * `chat_id` - Chat ID where to send response
/// * `username` - Username of the requester
/// * `message_text` - Full message text with command arguments
/// * `db_pool` - Database connection pool
pub async fn handle_setplan_command(
    bot: &Bot,
    chat_id: ChatId,
    username: Option<&str>,
    message_text: &str,
    db_pool: Arc<DbPool>,
) -> Result<()> {
    if !is_admin(username) {
        bot.send_message(chat_id, "❌ У тебя нет прав для выполнения этой команды.")
            .await?;
        return Ok(());
    }

    // Parse command: /setplan <user_id> <plan>
    let parts: Vec<&str> = message_text.split_whitespace().collect();
    if parts.len() != 3 {
        bot.send_message(
            chat_id,
            "❌ Неверный формат команды. Используй: /setplan <user_id> <plan>\nПример: /setplan 123456789 premium",
        )
        .await?;
        return Ok(());
    }

    let user_id = match parts[1].parse::<i64>() {
        Ok(id) => id,
        Err(_) => {
            bot.send_message(
                chat_id,
                "❌ Неверный формат user_id. Используй: /setplan <user_id> <plan>",
            )
            .await?;
            return Ok(());
        }
    };

    let plan = parts[2];
    if !["free", "premium", "vip"].contains(&plan) {
        bot.send_message(chat_id, "❌ Неверный план. Используй: free, premium или vip")
            .await?;
        return Ok(());
    }

    let conn = get_connection(&db_pool)?;
    update_user_plan(&conn, user_id, plan)?;

    let (plan_emoji, plan_name) = match plan {
        "premium" => ("⭐", "Premium"),
        "vip" => ("👑", "VIP"),
        _ => ("🌟", "Free"),
    };

    // Send message to admin
    bot.send_message(
        chat_id,
        format!("✅ План пользователя {} изменен на {} {}", user_id, plan_emoji, plan),
    )
    .await?;

    // Send notification to the user whose plan was changed
    let user_chat_id = ChatId(user_id);
    bot.send_message(
        user_chat_id,
        format!(
            "💳 *Изменение плана подписки*\n\n\
            Твой план был изменен администратором\\.\n\n\
            *Новый план:* {} {}\n\n\
            Изменения вступят в силу немедленно\\! 🎉",
            plan_emoji, plan_name
        ),
    )
    .parse_mode(ParseMode::MarkdownV2)
    .await?;

    Ok(())
}

/// Handle /admin command - show admin control panel
///
/// # Arguments
/// * `bot` - Bot instance
/// * `chat_id` - Chat ID where to send response
/// * `username` - Username of the requester
/// * `db_pool` - Database connection pool
pub async fn handle_admin_command(
    bot: &Bot,
    chat_id: ChatId,
    username: Option<&str>,
    db_pool: Arc<DbPool>,
) -> Result<()> {
    if !is_admin(username) {
        bot.send_message(chat_id, "❌ У тебя нет прав для выполнения этой команды.")
            .await?;
        return Ok(());
    }

    let conn = get_connection(&db_pool)?;
    let users = get_all_users(&conn)?;

    // Create inline keyboard with users (2 per row)
    let mut keyboard_rows = Vec::new();
    let mut current_row = Vec::new();

    for user in users.iter().take(20) {
        // Show first 20 users
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

        let button_text = format!("{} {}", plan_emoji, username_display);
        let callback_data = format!("admin:user:{}", user.telegram_id);

        current_row.push(InlineKeyboardButton::callback(button_text, callback_data));

        // Every 2 buttons create a new row
        if current_row.len() == 2 {
            keyboard_rows.push(current_row.clone());
            current_row.clear();
        }
    }

    // Add remaining buttons if any
    if !current_row.is_empty() {
        keyboard_rows.push(current_row);
    }

    let keyboard = InlineKeyboardMarkup::new(keyboard_rows);

    bot.send_message(
        chat_id,
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
    .parse_mode(ParseMode::MarkdownV2)
    .reply_markup(keyboard)
    .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escape_markdown_basic() {
        assert_eq!(escape_markdown("hello"), "hello");
        assert_eq!(escape_markdown("hello_world"), "hello\\_world");
        assert_eq!(escape_markdown("hello*world"), "hello\\*world");
    }

    #[test]
    fn test_escape_markdown_complex() {
        let input = "Test: [link](url) *bold* _italic_ `code`";
        let expected = "Test: \\[link\\]\\(url\\) \\*bold\\* \\_italic\\_ \\`code\\`";
        assert_eq!(escape_markdown(input), expected);
    }

    #[test]
    fn test_escape_markdown_all_special_chars() {
        let input = r"\*[]()~`>#+-=|{}.!";
        let expected = r"\\\*\[\]\(\)\~\`\>\#\+\-\=\|\{\}\.\!";
        assert_eq!(escape_markdown(input), expected);
    }

    #[test]
    fn test_is_admin() {
        // Test with default admin username (from config)
        let admin_username = crate::core::config::admin::ADMIN_USERNAME.as_str();
        assert!(is_admin(Some(admin_username)));
        assert!(!is_admin(Some("other_user")));
        assert!(!is_admin(None));
    }
}
