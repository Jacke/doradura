//! Vault menu: setup, manage, and disconnect user's private vault channel.
//!
//! Callback prefix: `vault:`

use crate::storage::SharedStorage;
use crate::storage::db::DbPool;
use crate::telegram::{Bot, BotExt};
use anyhow::Context;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{CallbackQueryId, ChatId, InlineKeyboardButton, InlineKeyboardMarkup, MessageId};

const VAULT_SETUP_PROMPT_KIND: &str = "vault_setup";
const VAULT_SETUP_TTL_SECS: i64 = 300;

pub async fn is_waiting_for_vault_setup(shared_storage: &Arc<SharedStorage>, user_id: i64) -> bool {
    shared_storage
        .get_prompt_session(user_id, VAULT_SETUP_PROMPT_KIND)
        .await
        .ok()
        .flatten()
        .is_some()
}

async fn set_waiting_for_vault_setup(shared_storage: &Arc<SharedStorage>, user_id: i64, waiting: bool) {
    if waiting {
        if let Err(e) = shared_storage
            .upsert_prompt_session(user_id, VAULT_SETUP_PROMPT_KIND, "", VAULT_SETUP_TTL_SECS)
            .await
        {
            log::error!("Failed to set vault setup prompt for user {}: {}", user_id, e);
        }
    } else if let Err(e) = shared_storage
        .delete_prompt_session(user_id, VAULT_SETUP_PROMPT_KIND)
        .await
    {
        log::error!("Failed to clear vault setup prompt for user {}: {}", user_id, e);
    }
}

pub async fn handle_vault_callback(
    bot: &Bot,
    _callback_id: CallbackQueryId,
    chat_id: ChatId,
    message_id: MessageId,
    data: &str,
    db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
) -> Result<(), teloxide::RequestError> {
    let suffix = data.strip_prefix("vault:").unwrap_or(data);

    match suffix {
        "menu" => show_vault_menu(bot, chat_id, message_id, &db_pool, &shared_storage).await,
        "setup" => {
            set_waiting_for_vault_setup(&shared_storage, chat_id.0, true).await;
            let text = "\
\u{1f5c4} *Vault Setup*\n\n\
Steps:\n\
1\\. Create a private Telegram channel\n\
2\\. Add this bot as admin \\(post messages permission\\)\n\
3\\. Forward any message from that channel here\n\n\
_Or send the channel @username or t\\.me/username link_";
            let kb = InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::callback(
                "\u{274c} Cancel",
                "vault:menu".to_string(),
            )]]);
            let _ = bot
                .edit_message_text(chat_id, message_id, text)
                .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                .reply_markup(kb)
                .await;
        }
        "disable" => {
            crate::core::log_if_err(
                shared_storage.deactivate_user_vault(chat_id.0).await,
                "Failed to deactivate vault",
            );
            show_vault_menu(bot, chat_id, message_id, &db_pool, &shared_storage).await;
        }
        "enable" => {
            crate::core::log_if_err(
                shared_storage.activate_user_vault(chat_id.0).await,
                "Failed to activate vault",
            );
            show_vault_menu(bot, chat_id, message_id, &db_pool, &shared_storage).await;
        }
        "clear" => {
            crate::core::log_if_err(shared_storage.clear_vault_cache(chat_id.0).await, "vault clear_cache");
            let _ = bot
                .edit_message_text(chat_id, message_id, "\u{1f5d1} Cache cleared.")
                .reply_markup(InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::callback(
                    "\u{25c0} Back",
                    "vault:menu".to_string(),
                )]]))
                .await;
        }
        "disconnect" => {
            crate::core::log_if_err(shared_storage.clear_vault_cache(chat_id.0).await, "vault clear_cache");
            crate::core::log_if_err(shared_storage.delete_user_vault(chat_id.0).await, "vault delete");
            let _ = bot
                .edit_message_text(chat_id, message_id, "\u{2705} Vault disconnected.")
                .reply_markup(InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::callback(
                    "\u{25c0} Back",
                    "vault:menu".to_string(),
                )]]))
                .await;
        }
        _ => {}
    }

    Ok(())
}

async fn show_vault_menu(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    db_pool: &Arc<DbPool>,
    shared_storage: &Arc<SharedStorage>,
) {
    set_waiting_for_vault_setup(shared_storage, chat_id.0, false).await;

    let _ = db_pool;
    let vault = shared_storage.get_user_vault(chat_id.0).await.ok().flatten();

    match vault {
        Some(v) => {
            let (count, total_bytes) = shared_storage.get_vault_cache_stats(chat_id.0).await.unwrap_or((0, 0));
            let size_mb = total_bytes as f64 / (1024.0 * 1024.0);
            let channel_name = v.channel_title.as_deref().unwrap_or("Unknown");
            let status = if v.is_active {
                "\u{2705} Active"
            } else {
                "\u{23f8} Paused"
            };

            let text = format!(
                "\u{1f5c4} *Vault*\n\nChannel: {}\nStatus: {}\n\u{1f4ca} {} tracks, {:.1} MB cached",
                channel_name, status, count, size_mb
            );

            let mut rows = Vec::new();
            if v.is_active {
                rows.push(vec![InlineKeyboardButton::callback(
                    "\u{23f8} Disable",
                    "vault:disable".to_string(),
                )]);
            } else {
                rows.push(vec![InlineKeyboardButton::callback(
                    "\u{25b6} Enable",
                    "vault:enable".to_string(),
                )]);
            }
            rows.push(vec![
                InlineKeyboardButton::callback("\u{1f5d1} Clear Cache", "vault:clear".to_string()),
                InlineKeyboardButton::callback("\u{274c} Disconnect", "vault:disconnect".to_string()),
            ]);
            rows.push(vec![InlineKeyboardButton::callback(
                "\u{25c0} Back",
                "main:menu".to_string(),
            )]);

            let kb = InlineKeyboardMarkup::new(rows);
            let _ = bot.edit_message_text(chat_id, message_id, text).reply_markup(kb).await;
        }
        None => {
            let text = "\u{1f5c4} *Vault*\n\n\
                Your vault is a private Telegram channel where the bot stores your downloads\\.\n\n\
                \u{2705} Instant repeat playback via cached file\\_id\n\
                \u{2705} Browseable music library\n\
                \u{2705} You own the channel and its content";
            let kb = InlineKeyboardMarkup::new(vec![
                vec![InlineKeyboardButton::callback(
                    "\u{2699} Setup Vault",
                    "vault:setup".to_string(),
                )],
                vec![InlineKeyboardButton::callback("\u{25c0} Back", "main:menu".to_string())],
            ]);
            let _ = bot
                .edit_message_text(chat_id, message_id, text)
                .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                .reply_markup(kb)
                .await;
        }
    }
}

/// Handle vault setup input: forwarded message or channel link.
pub async fn handle_vault_setup_input(
    bot: &Bot,
    msg: &teloxide::types::Message,
    db_pool: &Arc<DbPool>,
    shared_storage: &Arc<SharedStorage>,
) {
    set_waiting_for_vault_setup(shared_storage, msg.chat.id.0, false).await;
    let chat_id = msg.chat.id;

    // Try forwarded message first
    if let Some(teloxide::types::MessageOrigin::Channel { chat, .. }) = msg.forward_origin() {
        let channel_id = chat.id.0;
        let channel_title = chat.title().map(|s| s.to_string());
        if let Err(reason) = verify_and_save_vault(
            bot,
            db_pool,
            shared_storage,
            chat_id.0,
            channel_id,
            channel_title.as_deref(),
        )
        .await
        {
            let _ = bot.send_message(chat_id, reason.to_string()).await;
        }
        return;
    }

    // Try text: @username or t.me/username
    let text = msg.text().unwrap_or("").trim();
    let username = if let Some(u) = text.strip_prefix("@") {
        u.to_string()
    } else if let Some(u) = text
        .strip_prefix("https://t.me/")
        .or_else(|| text.strip_prefix("t.me/"))
    {
        u.trim_end_matches('/').to_string()
    } else {
        let _ = bot
            .send_message(
                chat_id,
                "\u{274c} Please forward a message from your channel, or send @username / t.me/username",
            )
            .await;
        return;
    };

    if username.is_empty() {
        let _ = bot.send_message(chat_id, "\u{274c} Invalid channel username").await;
        return;
    }

    // Resolve @username via raw Telegram Bot API call
    let chat_result = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .unwrap_or_default()
        .get(format!(
            "https://api.telegram.org/bot{}/getChat",
            std::env::var("TELOXIDE_TOKEN")
                .or_else(|_| std::env::var("TELEGRAM_BOT_TOKEN"))
                .unwrap_or_default()
        ))
        .query(&[("chat_id", format!("@{}", username))])
        .send()
        .await;

    match chat_result {
        Ok(resp) => {
            if let Ok(body) = resp.json::<serde_json::Value>().await {
                if body["ok"].as_bool() == Some(true) {
                    let channel_id = body["result"]["id"].as_i64().unwrap_or(0);
                    let channel_title = body["result"]["title"].as_str().map(|s| s.to_string());
                    if channel_id == 0 {
                        let _ = bot.send_message(chat_id, "\u{274c} Could not resolve channel").await;
                        return;
                    }
                    if let Err(reason) = verify_and_save_vault(
                        bot,
                        db_pool,
                        shared_storage,
                        chat_id.0,
                        channel_id,
                        channel_title.as_deref(),
                    )
                    .await
                    {
                        let _ = bot.send_message(chat_id, reason.to_string()).await;
                    }
                } else {
                    let _ = bot
                        .send_message(
                            chat_id,
                            "\u{274c} Channel not found. Make sure the bot is added as admin.",
                        )
                        .await;
                }
            }
        }
        Err(_) => {
            let _ = bot.send_message(chat_id, "\u{274c} Failed to resolve channel").await;
        }
    }
}

async fn verify_and_save_vault(
    bot: &Bot,
    db_pool: &Arc<DbPool>,
    shared_storage: &Arc<SharedStorage>,
    user_id: i64,
    channel_id: i64,
    channel_title: Option<&str>,
) -> anyhow::Result<()> {
    let channel_chat_id = ChatId(channel_id);

    // Verify bot is admin: send a test message and delete it
    let test_msg = bot
        .send_message(channel_chat_id, "\u{2705} Vault connection test (will be deleted)")
        .await
        .map_err(|e| {
            anyhow::anyhow!(
                "\u{274c} Cannot post to channel: {}. Make sure the bot is admin with post permissions.",
                e
            )
        })?;

    bot.try_delete(channel_chat_id, test_msg.id).await;

    // Save vault
    let _ = db_pool;
    shared_storage
        .set_user_vault(user_id, channel_id, channel_title)
        .await
        .with_context(|| "DB error")?;

    let title = channel_title.unwrap_or("your channel");
    let _ = bot
        .send_message(
            ChatId(user_id),
            format!(
                "\u{2705} Vault connected to \"{}\"! Your downloads will now be cached there.",
                title
            ),
        )
        .await;

    Ok(())
}
