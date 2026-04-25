use crate::core::config::admin::{ADMIN_IDS, ADMIN_USER_ID};
use crate::storage::db::{self as db, DbPool};
use crate::telegram::Bot;
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

fn admin_chat_ids() -> Vec<ChatId> {
    if !ADMIN_IDS.is_empty() {
        return ADMIN_IDS.iter().copied().map(ChatId).collect();
    }
    if *ADMIN_USER_ID != 0 {
        return vec![ChatId(*ADMIN_USER_ID)];
    }
    Vec::new()
}

/// Sends a plain-text message to the configured admins (uses `ADMIN_IDS` or `ADMIN_USER_ID`).
pub async fn notify_admin_text(bot: &Bot, text: &str) {
    let admin_chat_ids = admin_chat_ids();
    if admin_chat_ids.is_empty() {
        log::warn!("ADMIN_IDS/ADMIN_USER_ID not configured; admin notification skipped");
        return;
    }

    for chat_id in admin_chat_ids {
        send_plain_text_chunks(bot, chat_id, text).await;
    }
}

/// Sends a notification to admin about video processing error
pub async fn notify_admin_video_error(bot: &Bot, user_id: i64, username: Option<&str>, error: &str, context: &str) {
    let admin_chat_ids = admin_chat_ids();
    if admin_chat_ids.is_empty() {
        log::warn!("ADMIN_IDS/ADMIN_USER_ID not configured; admin notification skipped");
        return;
    }

    let username_str = username.unwrap_or("unknown");
    let message = format!(
        "⚠️ *Video processing error*\n\n\
        👤 User: @{} (ID: {})\n\
        📝 Context: {}\n\n\
        ❌ Error:\n```\n{}\n```",
        username_str, user_id, context, error
    );

    for chat_id in admin_chat_ids {
        send_plain_text_chunks(bot, chat_id, &message).await;
    }
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
    let admin_chat_ids = admin_chat_ids();
    if admin_chat_ids.is_empty() {
        log::warn!("ADMIN_IDS/ADMIN_USER_ID not configured; admin notification skipped");
        return;
    }

    // Look up username from DB
    let username = db::get_connection(&db_pool)
        .ok()
        .and_then(|conn| db::get_user(&conn, user_id).ok().flatten())
        .and_then(|u| u.username);

    // Short op ID for quick identification
    let op = &task_id[..8.min(task_id.len())];

    for chat_id in admin_chat_ids {
        let escaped_error = crate::telegram::admin::escape_markdown(error_message);
        let escaped_url = crate::telegram::admin::escape_markdown(url);
        let user_display = match &username {
            Some(name) => format!("@{} \\({}\\)", crate::telegram::admin::escape_markdown(name), user_id),
            None => format!("`{}`", user_id),
        };

        let error_lower = error_message.to_lowercase();
        let cookie_hint = if error_lower.contains("blocked")
            || error_lower.contains("bot")
            || error_lower.contains("403")
            || error_lower.contains("sign in")
        {
            "\n\n🍪 *Cookies may need refresh\\!* Use /update\\_cookies"
        } else {
            ""
        };

        let message = format!(
            "⚠️ *Task error*\n\n\
            Op: `{}`\n\
            User: {}\n\
            URL: {}\n\
            Error: {}{}\n\n\
            The task will be retried automatically\\.",
            op, user_display, escaped_url, escaped_error, cookie_hint
        );

        match bot
            .send_message(chat_id, &message)
            .parse_mode(teloxide::types::ParseMode::MarkdownV2)
            .await
        {
            Err(e) => {
                log::error!("Failed to send admin notification: {}", e);
            }
            _ => {
                log::info!("Admin notification sent for task {}", task_id);
            }
        }

        if let Some(details) = details {
            let details_message = format!("Details for op={} (user {}):\n{}", op, user_id, details);
            send_plain_text_chunks(&bot, chat_id, &details_message).await;
        }
    }
}

/// Sends a notification to admins that the bot has started/restarted.
///
/// # Arguments
///
/// * `bot` - Bot instance used to send messages
/// * `bot_username` - The bot's username
pub async fn notify_admin_startup(bot: &Bot, bot_username: Option<&str>) {
    let admin_chat_ids = admin_chat_ids();
    if admin_chat_ids.is_empty() {
        log::warn!("ADMIN_IDS/ADMIN_USER_ID not configured; startup notification skipped");
        return;
    }

    // Get commit info from environment variable (set by Docker build or CI)
    let commit_info = std::env::var("GIT_COMMIT")
        .or_else(|_| std::env::var("RAILWAY_GIT_COMMIT_SHA"))
        .map(|sha| {
            // Shorten to 7 characters like git does
            if sha.len() > 7 { sha[..7].to_string() } else { sha }
        })
        .unwrap_or_else(|_| "unknown".to_string());

    // Get branch info if available
    let branch_info = std::env::var("GIT_BRANCH")
        .or_else(|_| std::env::var("RAILWAY_GIT_BRANCH"))
        .unwrap_or_else(|_| "main".to_string());

    let bot_name = bot_username.unwrap_or("doradura");
    let version = env!("CARGO_PKG_VERSION");

    let message = format!(
        "🚀 *Bot Restarted*\n\n\
        🤖 Bot: @{}\n\
        📦 Version: {}\n\
        🔖 Commit: `{}`\n\
        🌿 Branch: {}\n\
        ⏰ Time: {}",
        bot_name,
        version,
        commit_info,
        branch_info,
        chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
    );

    for chat_id in admin_chat_ids {
        if let Err(e) = bot
            .send_message(chat_id, &message)
            .parse_mode(teloxide::types::ParseMode::MarkdownV2)
            .await
        {
            // Fallback to plain text if markdown fails
            let plain_message = format!(
                "🚀 Bot Restarted\n\n\
                Bot: @{}\n\
                Version: {}\n\
                Commit: {}\n\
                Branch: {}\n\
                Time: {}",
                bot_name,
                version,
                commit_info,
                branch_info,
                chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
            );
            if let Err(e2) = bot.send_message(chat_id, &plain_message).await {
                log::error!("Failed to send startup notification: {} / {}", e, e2);
            }
        }
    }

    log::info!("Admin notified about bot startup (commit: {})", commit_info);
}

/// Sends a notification to admins about a new user registration.
///
/// # Arguments
///
/// * `bot` - Bot instance used to send messages
/// * `user_id` - Telegram ID of the new user
/// * `username` - Username of the new user (if available)
/// * `first_name` - First name of the user (if available)
/// * `language` - Detected/selected language code
/// * `first_message` - The first message/action from the user (if available)
pub async fn notify_admin_new_user(
    bot: &Bot,
    user_id: i64,
    username: Option<&str>,
    first_name: Option<&str>,
    language: Option<&str>,
    first_message: Option<&str>,
) {
    let admin_chat_ids = admin_chat_ids();
    if admin_chat_ids.is_empty() {
        return;
    }

    let username_display = username.map_or_else(|| "—".to_string(), |u| format!("@{}", u));
    let first_name_display = first_name.unwrap_or("—");
    let language_display = language.unwrap_or("—");

    let mut message = format!(
        "🆕 *New user*\n\n\
        👤 {}\n\
        📛 Name: {}\n\
        🆔 ID: `{}`\n\
        🌐 Language: {}",
        username_display, first_name_display, user_id, language_display
    );

    if let Some(msg) = first_message {
        // Truncate long messages
        let truncated = if msg.chars().count() > 200 {
            format!("{}...", msg.chars().take(200).collect::<String>())
        } else {
            msg.to_string()
        };
        message.push_str(&format!("\n\n💬 First message:\n{}", truncated));
    }

    for chat_id in admin_chat_ids {
        send_plain_text_chunks(bot, chat_id, &message).await;
    }

    log::info!(
        "Admin notified about new user: {} (@{})",
        user_id,
        username.unwrap_or("none")
    );
}
