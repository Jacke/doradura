//! Feedback functionality for the bot
//!
//! This module handles user feedback collection and admin notifications.

use crate::core::escape_markdown;
use crate::storage::SharedStorage;
use crate::telegram::Bot;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::ParseMode;

use crate::core::config::admin::ADMIN_USER_ID;
use crate::i18n;

const FEEDBACK_PROMPT_KIND: &str = "feedback";
const FEEDBACK_PROMPT_TTL_SECS: i64 = 300;

/// Check if user is waiting to provide feedback
pub async fn is_waiting_for_feedback(shared_storage: &Arc<SharedStorage>, user_id: i64) -> bool {
    shared_storage
        .get_prompt_session(user_id, FEEDBACK_PROMPT_KIND)
        .await
        .ok()
        .flatten()
        .is_some()
}

/// Set user feedback waiting state
pub async fn set_waiting_for_feedback(shared_storage: &Arc<SharedStorage>, user_id: i64, waiting: bool) {
    if waiting {
        let _ = shared_storage
            .upsert_prompt_session(user_id, FEEDBACK_PROMPT_KIND, "", FEEDBACK_PROMPT_TTL_SECS)
            .await;
    } else {
        let _ = shared_storage
            .delete_prompt_session(user_id, FEEDBACK_PROMPT_KIND)
            .await;
    }
}

/// Send feedback prompt to user
pub async fn send_feedback_prompt(
    bot: &Bot,
    chat_id: ChatId,
    lang: &unic_langid::LanguageIdentifier,
    shared_storage: &Arc<SharedStorage>,
) -> ResponseResult<()> {
    let message = i18n::t(lang, "feedback.prompt");

    bot.send_message(chat_id, message)
        .parse_mode(ParseMode::MarkdownV2)
        .await?;

    // Set state: waiting for feedback
    set_waiting_for_feedback(shared_storage, chat_id.0, true).await;

    Ok(())
}

/// Send feedback confirmation to user
pub async fn send_feedback_confirmation(
    bot: &Bot,
    chat_id: ChatId,
    lang: &unic_langid::LanguageIdentifier,
    shared_storage: &Arc<SharedStorage>,
) -> ResponseResult<()> {
    let message = i18n::t(lang, "feedback.sent");

    bot.send_message(chat_id, message)
        .parse_mode(ParseMode::MarkdownV2)
        .await?;

    // Clear state
    set_waiting_for_feedback(shared_storage, chat_id.0, false).await;

    Ok(())
}

/// Send feedback notification to admin
pub async fn notify_admin_feedback(
    bot: &Bot,
    user_id: i64,
    username: Option<&str>,
    first_name: &str,
    message_text: &str,
    shared_storage: &Arc<SharedStorage>,
) -> ResponseResult<()> {
    crate::core::metrics::record_user_feedback("neutral");
    log::info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    log::info!("💬 FEEDBACK RECEIVED");
    log::info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    log::info!("  • User ID: {}", user_id);
    log::info!("  • Username: {}", username.unwrap_or("N/A"));
    log::info!("  • First name: {}", first_name);
    log::info!("  • Message: {}", message_text);

    // Save feedback to database
    log::info!("💾 Saving feedback to database...");
    match shared_storage
        .save_feedback(user_id, username, first_name, message_text)
        .await
    {
        Ok(feedback_id) => {
            log::info!("✅ Feedback saved successfully with ID: {}", feedback_id);
        }
        Err(e) => {
            log::error!("❌ Failed to save feedback to database: {}", e);
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
    let lang = i18n::lang_from_code("ru"); // Admin language

    let args = doracore::fluent_args!("user_id" => user_id, "username" => escape_markdown(username.unwrap_or("N/A")), "first_name" => escape_markdown(first_name), "message" => escape_markdown(message_text));

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
