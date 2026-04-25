use super::is_admin;
use crate::storage::SharedStorage;
use crate::storage::db::DbPool;
use crate::telegram::Bot;
use anyhow::Result;
use std::sync::Arc;
use std::time::Duration;
use teloxide::prelude::*;

/// Handle /send command - send a message to a specific user on behalf of the bot
///
/// Format: `/send <telegram_id> <message text>`
pub async fn handle_send_command(
    bot: &Bot,
    chat_id: ChatId,
    user_id: i64,
    message_text: &str,
    db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
) -> Result<()> {
    if !is_admin(user_id) {
        bot.send_message(chat_id, "Access denied.").await?;
        return Ok(());
    }

    let parts: Vec<&str> = message_text.splitn(3, char::is_whitespace).collect();
    if parts.len() < 3 || parts[2].trim().is_empty() {
        bot.send_message(
            chat_id,
            "Usage: /send <telegram_id> <message>\n\nExample: /send 123456789 Hello!",
        )
        .await?;
        return Ok(());
    }

    let target_id = match parts[1].parse::<i64>() {
        Ok(id) => id,
        Err(_) => {
            bot.send_message(chat_id, "Invalid telegram_id. Must be a number.")
                .await?;
            return Ok(());
        }
    };

    let text = parts[2].trim();

    // Check that user exists in DB
    let _ = db_pool;
    if shared_storage.get_user(target_id).await?.is_none() {
        bot.send_message(chat_id, format!("User {} not found in database.", target_id))
            .await?;
        return Ok(());
    }

    match bot.send_message(ChatId(target_id), text).await {
        Ok(_) => {
            bot.send_message(chat_id, format!("Message sent to {}.", target_id))
                .await?;
        }
        Err(e) => {
            let err_str = e.to_string();
            if err_str.contains("Forbidden") || err_str.contains("bot was blocked") {
                bot.send_message(chat_id, format!("User {} has blocked the bot.", target_id))
                    .await?;
            } else {
                bot.send_message(chat_id, format!("Failed to send: {}", e)).await?;
            }
        }
    }

    Ok(())
}

/// Handle /broadcast command - send a message to all users
///
/// Format: `/broadcast <message text>`
/// Rate-limited to ~28 msg/sec to stay under Telegram's 30/sec limit.
pub async fn handle_broadcast_command(
    bot: &Bot,
    chat_id: ChatId,
    user_id: i64,
    message_text: &str,
    db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
) -> Result<()> {
    if !is_admin(user_id) {
        bot.send_message(chat_id, "Access denied.").await?;
        return Ok(());
    }

    let text = match message_text.strip_prefix("/broadcast") {
        Some(t) if !t.trim().is_empty() => t.trim(),
        _ => {
            bot.send_message(
                chat_id,
                "Usage: /broadcast <message>\n\nExample: /broadcast New feature available!",
            )
            .await?;
            return Ok(());
        }
    };

    let _ = db_pool;
    let users = shared_storage.get_all_users().await?;
    let total = users.len();

    bot.send_message(chat_id, format!("Broadcasting to {} users...", total))
        .await?;

    let mut sent: u32 = 0;
    let mut blocked: u32 = 0;
    let mut failed: u32 = 0;

    for user in &users {
        let tid = user.telegram_id();
        // Skip admin — they already see the message
        if tid == user_id {
            continue;
        }

        match bot.send_message(ChatId(tid), text).await {
            Ok(_) => sent += 1,
            Err(e) => {
                let err_str = e.to_string();
                if err_str.contains("Forbidden")
                    || err_str.contains("bot was blocked")
                    || err_str.contains("chat not found")
                {
                    blocked += 1;
                } else {
                    failed += 1;
                    log::warn!("Broadcast failed for user {}: {}", tid, e);
                }
            }
        }

        // Rate limit: ~28 msg/sec (under Telegram's 30/sec cap)
        tokio::time::sleep(Duration::from_millis(35)).await;
    }

    bot.send_message(
        chat_id,
        format!(
            "Broadcast complete.\nSent: {}\nBlocked/unavailable: {}\nFailed: {}",
            sent, blocked, failed
        ),
    )
    .await?;

    Ok(())
}
