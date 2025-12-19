//! Feedback functionality for the bot
//!
//! This module handles user feedback collection and admin notifications.

use std::collections::HashMap;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::ParseMode;
use tokio::sync::Mutex;

use crate::core::config::admin::ADMIN_USER_ID;
use crate::i18n;

/// Escapes special characters for MarkdownV2.
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

/// State management for feedback collection
/// Maps user_id -> waiting for feedback
static FEEDBACK_STATES: once_cell::sync::Lazy<Arc<Mutex<HashMap<i64, bool>>>> =
    once_cell::sync::Lazy::new(|| Arc::new(Mutex::new(HashMap::new())));

/// Check if user is waiting to provide feedback
pub async fn is_waiting_for_feedback(user_id: i64) -> bool {
    let states = FEEDBACK_STATES.lock().await;
    states.get(&user_id).copied().unwrap_or(false)
}

/// Set user feedback waiting state
pub async fn set_waiting_for_feedback(user_id: i64, waiting: bool) {
    let mut states = FEEDBACK_STATES.lock().await;
    if waiting {
        states.insert(user_id, true);
    } else {
        states.remove(&user_id);
    }
}

/// Send feedback prompt to user
pub async fn send_feedback_prompt(
    bot: &Bot,
    chat_id: ChatId,
    lang: &unic_langid::LanguageIdentifier,
) -> ResponseResult<()> {
    let message = i18n::t(lang, "feedback.prompt");

    bot.send_message(chat_id, message)
        .parse_mode(ParseMode::MarkdownV2)
        .await?;

    // Set state: waiting for feedback
    set_waiting_for_feedback(chat_id.0, true).await;

    Ok(())
}

/// Send feedback confirmation to user
pub async fn send_feedback_confirmation(
    bot: &Bot,
    chat_id: ChatId,
    lang: &unic_langid::LanguageIdentifier,
) -> ResponseResult<()> {
    let message = i18n::t(lang, "feedback.sent");

    bot.send_message(chat_id, message)
        .parse_mode(ParseMode::MarkdownV2)
        .await?;

    // Clear state
    set_waiting_for_feedback(chat_id.0, false).await;

    Ok(())
}

/// Send feedback notification to admin
pub async fn notify_admin_feedback(
    bot: &Bot,
    user_id: i64,
    username: Option<&str>,
    first_name: &str,
    message_text: &str,
    db_pool: std::sync::Arc<crate::storage::db::DbPool>,
) -> ResponseResult<()> {
    log::info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    log::info!("💬 FEEDBACK RECEIVED");
    log::info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    log::info!("  • User ID: {}", user_id);
    log::info!("  • Username: {}", username.unwrap_or("N/A"));
    log::info!("  • First name: {}", first_name);
    log::info!("  • Message: {}", message_text);

    // Save feedback to database
    log::info!("💾 Saving feedback to database...");
    match crate::storage::db::get_connection(&db_pool) {
        Ok(conn) => match crate::storage::db::save_feedback(&conn, user_id, username, first_name, message_text) {
            Ok(feedback_id) => {
                log::info!("✅ Feedback saved successfully with ID: {}", feedback_id);
            }
            Err(e) => {
                log::error!("❌ Failed to save feedback to database: {}", e);
            }
        },
        Err(e) => {
            log::error!("❌ Failed to get database connection: {}", e);
        }
    }

    // Get admin user ID from config
    let admin_id = *ADMIN_USER_ID;

    if admin_id == 0 {
        log::warn!("⚠️ ADMIN_USER_ID not configured (value is 0)");
        log::warn!("💡 Set ADMIN_USER_ID in your .env file to receive feedback notifications");
        log::warn!("💡 To get your user ID, send any message to @userinfobot");
        log::info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        return Ok(());
    }

    log::info!("📤 Sending notification to admin (ID: {})...", admin_id);

    // Create notification message
    use fluent_templates::fluent_bundle::FluentArgs;
    let lang = i18n::lang_from_code("ru"); // Admin language

    let mut args = FluentArgs::new();
    args.set("user_id", user_id);
    args.set("username", escape_markdown(username.unwrap_or("N/A")));
    args.set("first_name", escape_markdown(first_name));
    args.set("message", escape_markdown(message_text));

    let notification = i18n::t_args(&lang, "feedback.admin_notification", &args);

    // Send notification to admin
    match bot
        .send_message(ChatId(admin_id), notification)
        .parse_mode(ParseMode::MarkdownV2)
        .await
    {
        Ok(_) => {
            log::info!("✅ Feedback notification sent successfully to admin {}", admin_id);
            log::info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        }
        Err(e) => {
            log::error!("❌ Failed to send feedback notification to admin: {:?}", e);
            log::error!("   Check that ADMIN_USER_ID ({}) is correct", admin_id);
            log::error!("   The admin must have started a chat with the bot first");
            log::info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
            return Err(e);
        }
    }

    Ok(())
}
