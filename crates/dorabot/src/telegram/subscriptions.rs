//! Telegram UI for content subscriptions (subscribe, manage, notifications).
//!
//! Handles `/subscriptions` command and `cw:` callback prefix.

use crate::core::config;
use crate::storage::db::{self, DbPool};
use crate::storage::get_connection;
use crate::telegram::cb;
use crate::telegram::Bot;
use crate::watcher::db as watcher_db;
use crate::watcher::traits::WatchNotification;
use crate::watcher::WatcherRegistry;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{CallbackQueryId, ChatId, InlineKeyboardMarkup, MessageId};
use tokio::sync::mpsc;

/// Max subscriptions by plan.
fn max_subscriptions_for_plan(plan: &str) -> u32 {
    match plan {
        "vip" => *config::watcher::MAX_SUBS_VIP,
        "premium" => *config::watcher::MAX_SUBS_PREMIUM,
        _ => *config::watcher::MAX_SUBS_FREE,
    }
}

// ─── /subscriptions command ───

/// Handle the /subscriptions command: show list of user's subscriptions.
pub async fn handle_subscriptions_command(bot: &Bot, chat_id: ChatId, db_pool: &Arc<DbPool>) {
    let conn = match get_connection(db_pool) {
        Ok(c) => c,
        Err(e) => {
            log::error!("DB error in /subscriptions: {}", e);
            let _ = bot.send_message(chat_id, "Database error").await;
            return;
        }
    };

    let subs = match watcher_db::get_user_subscriptions(&conn, chat_id.0) {
        Ok(s) => s,
        Err(e) => {
            log::error!("Failed to get subscriptions: {}", e);
            let _ = bot.send_message(chat_id, "Failed to load subscriptions").await;
            return;
        }
    };

    // Get user plan for limit display
    let plan = db::get_user(&conn, chat_id.0)
        .ok()
        .flatten()
        .map(|u| u.plan.to_string())
        .unwrap_or_else(|| "free".to_string());
    let max_subs = max_subscriptions_for_plan(&plan);
    drop(conn);

    if subs.is_empty() {
        let text = format!(
            "🔔 You have no active subscriptions.\n\n\
             Send an Instagram profile link to subscribe to updates.\n\
             Limit: 0/{} subscriptions",
            max_subs
        );
        let _ = bot.send_message(chat_id, text).await;
        return;
    }

    let mut text = format!("🔔 Your Subscriptions ({}/{})\n", subs.len(), max_subs);

    let mut buttons: Vec<Vec<teloxide::types::InlineKeyboardButton>> = Vec::new();

    for (i, sub) in subs.iter().enumerate() {
        let source_emoji = match sub.source_type.as_str() {
            "instagram" => "📸",
            _ => "🔗",
        };

        let types: Vec<&str> = {
            let mut t = Vec::new();
            if sub.watch_mask & 1 != 0 {
                t.push("Posts");
            }
            if sub.watch_mask & 2 != 0 {
                t.push("Stories");
            }
            t
        };
        let types_str = types.join(" + ");

        let last_check = sub.last_checked_at.as_deref().unwrap_or("never");

        text.push_str(&format!(
            "\n{}. {} {} — {}\n   Last check: {}",
            i + 1,
            source_emoji,
            sub.display_name,
            types_str,
            last_check,
        ));

        if let Some(ref err) = sub.last_error {
            text.push_str(&format!("\n   ⚠️ {}", &err[..err.len().min(50)]));
        }

        buttons.push(vec![cb(
            format!("Manage {}", sub.display_name),
            format!("cw:manage:{}", sub.id),
        )]);
    }

    let keyboard = InlineKeyboardMarkup::new(buttons);
    let _ = bot.send_message(chat_id, text).reply_markup(keyboard).await;
}

// ─── Subscribe flow (triggered from ig:sub:<username>) ───

/// Show subscribe confirmation dialog.
pub async fn show_subscribe_confirm(
    bot: &Bot,
    chat_id: ChatId,
    username: &str,
    db_pool: &Arc<DbPool>,
    registry: &WatcherRegistry,
) {
    let watcher = match registry.get("instagram") {
        Some(w) => w,
        None => {
            let _ = bot.send_message(chat_id, "Instagram watcher not available").await;
            return;
        }
    };

    // Check subscription limit
    let conn = match get_connection(db_pool) {
        Ok(c) => c,
        Err(_) => {
            let _ = bot.send_message(chat_id, "Database error").await;
            return;
        }
    };

    let plan = db::get_user(&conn, chat_id.0)
        .ok()
        .flatten()
        .map(|u| u.plan.to_string())
        .unwrap_or_else(|| "free".to_string());
    let current_count = watcher_db::count_user_subscriptions(&conn, chat_id.0).unwrap_or(0);
    let max_subs = max_subscriptions_for_plan(&plan);

    // Check if already subscribed
    if let Ok(Some(existing)) = watcher_db::has_subscription(&conn, chat_id.0, "instagram", username) {
        if existing.is_active {
            let _ = bot
                .send_message(chat_id, format!("You're already subscribed to @{}!", username))
                .await;
            return;
        }
    }
    drop(conn);

    if current_count >= max_subs {
        let _ = bot
            .send_message(
                chat_id,
                format!(
                    "You've reached the subscription limit ({}/{}).\n\
                     Upgrade your plan or unsubscribe from an existing one.",
                    current_count, max_subs
                ),
            )
            .await;
        return;
    }

    // Resolve the profile to validate it exists
    let (display_name, _meta) = match watcher.resolve_source(username).await {
        Ok(r) => r,
        Err(e) => {
            let _ = bot.send_message(chat_id, format!("Cannot subscribe: {}", e)).await;
            return;
        }
    };

    let default_mask = watcher.default_watch_mask();
    let text = format!("Subscribe to {} updates?", display_name);

    let has_cookies = crate::download::cookies::load_instagram_cookie_header().is_some();
    let effective_mask = if has_cookies { default_mask } else { 1 }; // Posts only without cookies

    let mut toggle_row = vec![cb(
        format!("{}Posts", if effective_mask & 1 != 0 { "✅ " } else { "☐ " }),
        format!("cw:ptog:{}:1:{}", username, effective_mask),
    )];

    if has_cookies {
        toggle_row.push(cb(
            format!("{}Stories", if effective_mask & 2 != 0 { "✅ " } else { "☐ " }),
            format!("cw:ptog:{}:2:{}", username, effective_mask),
        ));
    }

    let keyboard = InlineKeyboardMarkup::new(vec![
        toggle_row,
        vec![
            cb("✔ Confirm", format!("cw:ok:instagram:{}:{}", username, effective_mask)),
            cb("✖ Cancel", "cw:cancel".to_string()),
        ],
    ]);

    let _ = bot.send_message(chat_id, text).reply_markup(keyboard).await;
}

// ─── Callback handler (cw: prefix) ───

/// Handle all `cw:` callbacks.
pub async fn handle_subscription_callback(
    bot: &Bot,
    callback_id: &CallbackQueryId,
    chat_id: ChatId,
    message_id: MessageId,
    data: &str,
    db_pool: Arc<DbPool>,
    registry: &WatcherRegistry,
) {
    let _ = bot.answer_callback_query(callback_id.clone()).await;

    let parts: Vec<&str> = data.split(':').collect();
    if parts.len() < 2 {
        return;
    }

    match parts[1] {
        "ok" => {
            // cw:ok:ig:<username>:<mask>
            if parts.len() >= 5 {
                let source_type = parts[2];
                let source_id = parts[3];
                let mask: u32 = parts[4].parse().unwrap_or(3);
                handle_confirm_subscribe(
                    bot,
                    chat_id,
                    message_id,
                    source_type,
                    source_id,
                    mask,
                    &db_pool,
                    registry,
                )
                .await;
            }
        }
        "cancel" => {
            let _ = bot.delete_message(chat_id, message_id).await;
        }
        "ptog" => {
            // cw:ptog:<username>:<bit>:<current_mask>
            if parts.len() >= 5 {
                let username = parts[2];
                let bit: u32 = parts[3].parse().unwrap_or(0);
                let current_mask: u32 = parts[4].parse().unwrap_or(3);
                let new_mask = current_mask ^ bit;
                // Don't allow mask=0
                let new_mask = if new_mask == 0 { bit } else { new_mask };
                update_toggle_keyboard(bot, chat_id, message_id, username, new_mask).await;
            }
        }
        "manage" => {
            // cw:manage:<id>
            if parts.len() >= 3 {
                if let Ok(sub_id) = parts[2].parse::<i64>() {
                    show_manage_subscription(bot, chat_id, message_id, sub_id, &db_pool).await;
                }
            }
        }
        "unsub" => {
            // cw:unsub:<id>
            if parts.len() >= 3 {
                if let Ok(sub_id) = parts[2].parse::<i64>() {
                    handle_unsubscribe(bot, chat_id, message_id, sub_id, &db_pool).await;
                }
            }
        }
        "tog" => {
            // cw:tog:<id>:<bit>
            if parts.len() >= 4 {
                if let (Ok(sub_id), Ok(bit)) = (parts[2].parse::<i64>(), parts[3].parse::<u32>()) {
                    handle_toggle_content_type(bot, chat_id, message_id, sub_id, bit, &db_pool).await;
                }
            }
        }
        "list" => {
            let _ = bot.delete_message(chat_id, message_id).await;
            handle_subscriptions_command(bot, chat_id, &db_pool).await;
        }
        _ => {}
    }
}

async fn handle_confirm_subscribe(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    source_type: &str,
    source_id: &str,
    mask: u32,
    db_pool: &Arc<DbPool>,
    registry: &WatcherRegistry,
) {
    let watcher = match registry.get(source_type) {
        Some(w) => w,
        None => {
            let _ = bot
                .edit_message_text(chat_id, message_id, "Watcher not available")
                .await;
            return;
        }
    };

    // Resolve source again to get display_name and meta
    let (display_name, meta) = match watcher.resolve_source(source_id).await {
        Ok(r) => r,
        Err(e) => {
            let _ = bot
                .edit_message_text(chat_id, message_id, format!("Failed: {}", e))
                .await;
            return;
        }
    };

    let conn = match get_connection(db_pool) {
        Ok(c) => c,
        Err(_) => {
            let _ = bot.edit_message_text(chat_id, message_id, "Database error").await;
            return;
        }
    };

    match watcher_db::upsert_subscription(
        &conn,
        chat_id.0,
        source_type,
        source_id,
        &display_name,
        mask,
        meta.as_ref(),
    ) {
        Ok(_id) => {
            let types: Vec<&str> = {
                let mut t = Vec::new();
                if mask & 1 != 0 {
                    t.push("Posts");
                }
                if mask & 2 != 0 {
                    t.push("Stories");
                }
                t
            };

            let text = format!(
                "🔔 Subscribed to {} ({})\n\nYou'll be notified when new content appears.",
                display_name,
                types.join(" + "),
            );

            let keyboard = InlineKeyboardMarkup::new(vec![vec![cb("📋 My Subscriptions", "cw:list".to_string())]]);

            let _ = bot
                .edit_message_text(chat_id, message_id, text)
                .reply_markup(keyboard)
                .await;

            log::info!(
                "User {} subscribed to {}:{} (mask={})",
                chat_id.0,
                source_type,
                source_id,
                mask
            );
        }
        Err(e) => {
            let _ = bot
                .edit_message_text(chat_id, message_id, format!("Failed to subscribe: {}", e))
                .await;
        }
    }
}

async fn update_toggle_keyboard(bot: &Bot, chat_id: ChatId, message_id: MessageId, username: &str, new_mask: u32) {
    let has_cookies = crate::download::cookies::load_instagram_cookie_header().is_some();

    let mut toggle_row = vec![cb(
        format!("{}Posts", if new_mask & 1 != 0 { "✅ " } else { "☐ " }),
        format!("cw:ptog:{}:1:{}", username, new_mask),
    )];

    if has_cookies {
        toggle_row.push(cb(
            format!("{}Stories", if new_mask & 2 != 0 { "✅ " } else { "☐ " }),
            format!("cw:ptog:{}:2:{}", username, new_mask),
        ));
    }

    let keyboard = InlineKeyboardMarkup::new(vec![
        toggle_row,
        vec![
            cb("✔ Confirm", format!("cw:ok:instagram:{}:{}", username, new_mask)),
            cb("✖ Cancel", "cw:cancel".to_string()),
        ],
    ]);

    let _ = bot
        .edit_message_reply_markup(chat_id, message_id)
        .reply_markup(keyboard)
        .await;
}

async fn show_manage_subscription(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    sub_id: i64,
    db_pool: &Arc<DbPool>,
) {
    let conn = match get_connection(db_pool) {
        Ok(c) => c,
        Err(_) => return,
    };

    let sub = match watcher_db::get_subscription(&conn, sub_id) {
        Ok(Some(s)) => s,
        _ => {
            let _ = bot
                .edit_message_text(chat_id, message_id, "Subscription not found")
                .await;
            return;
        }
    };

    let source_emoji = match sub.source_type.as_str() {
        "instagram" => "📸",
        _ => "🔗",
    };

    let text = format!(
        "{} {} — Manage Subscription\n\n\
         Source: {}\n\
         Status: {}\n\
         Last check: {}\n\
         Errors: {}",
        source_emoji,
        sub.display_name,
        sub.source_type,
        if sub.is_active { "Active" } else { "Inactive" },
        sub.last_checked_at.as_deref().unwrap_or("never"),
        sub.consecutive_errors,
    );

    let keyboard = InlineKeyboardMarkup::new(vec![
        vec![
            cb(
                format!("{}Posts", if sub.watch_mask & 1 != 0 { "✅ " } else { "☐ " }),
                format!("cw:tog:{}:1", sub_id),
            ),
            cb(
                format!("{}Stories", if sub.watch_mask & 2 != 0 { "✅ " } else { "☐ " }),
                format!("cw:tog:{}:2", sub_id),
            ),
        ],
        vec![
            cb("🔕 Unsubscribe", format!("cw:unsub:{}", sub_id)),
            cb("🔙 Back", "cw:list".to_string()),
        ],
    ]);

    let _ = bot
        .edit_message_text(chat_id, message_id, text)
        .reply_markup(keyboard)
        .await;
}

async fn handle_unsubscribe(bot: &Bot, chat_id: ChatId, message_id: MessageId, sub_id: i64, db_pool: &Arc<DbPool>) {
    let conn = match get_connection(db_pool) {
        Ok(c) => c,
        Err(_) => return,
    };

    // Get sub info before deactivating
    let display_name = watcher_db::get_subscription(&conn, sub_id)
        .ok()
        .flatten()
        .map(|s| s.display_name)
        .unwrap_or_default();

    match watcher_db::deactivate_subscription(&conn, sub_id) {
        Ok(()) => {
            let text = format!("🔕 Unsubscribed from {}", display_name);
            let keyboard = InlineKeyboardMarkup::new(vec![vec![cb("📋 My Subscriptions", "cw:list".to_string())]]);
            let _ = bot
                .edit_message_text(chat_id, message_id, text)
                .reply_markup(keyboard)
                .await;
            log::info!("User {} unsubscribed from sub_id={}", chat_id.0, sub_id);
        }
        Err(e) => {
            let _ = bot
                .edit_message_text(chat_id, message_id, format!("Failed: {}", e))
                .await;
        }
    }
}

async fn handle_toggle_content_type(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    sub_id: i64,
    bit: u32,
    db_pool: &Arc<DbPool>,
) {
    let conn = match get_connection(db_pool) {
        Ok(c) => c,
        Err(_) => return,
    };

    let sub = match watcher_db::get_subscription(&conn, sub_id) {
        Ok(Some(s)) => s,
        _ => return,
    };

    let new_mask = sub.watch_mask ^ bit;
    // Don't allow mask=0
    let new_mask = if new_mask == 0 { bit } else { new_mask };

    if let Err(e) = watcher_db::update_watch_mask(&conn, sub_id, new_mask) {
        log::error!("Failed to toggle content type: {}", e);
        return;
    }
    drop(conn);

    // Refresh the manage view
    show_manage_subscription(bot, chat_id, message_id, sub_id, db_pool).await;
}

// ─── Notification dispatcher ───

/// Start the notification dispatcher that receives WatchNotifications
/// and sends formatted Telegram messages.
pub fn start_notification_dispatcher(
    bot: Bot,
    db_pool: Arc<DbPool>,
    mut rx: mpsc::UnboundedReceiver<WatchNotification>,
) {
    tokio::spawn(async move {
        while let Some(notification) = rx.recv().await {
            let chat_id = ChatId(notification.user_id);

            match notification.update.content_type.as_str() {
                "post" => {
                    let text = format!(
                        "📸 New post by {}\n{}",
                        notification.display_name, notification.update.url,
                    );

                    let keyboard = InlineKeyboardMarkup::new(vec![vec![cb(
                        "🔕 Unsubscribe",
                        format!("cw:unsub:{}", notification.subscription_id),
                    )]]);

                    if let Err(e) = bot.send_message(chat_id, &text).reply_markup(keyboard).await {
                        handle_send_error(e, notification.user_id, &db_pool);
                    }
                }
                "story" => {
                    let text = format!("📱 {}\n", notification.update.description,);

                    let keyboard = InlineKeyboardMarkup::new(vec![vec![cb(
                        "🔕 Unsubscribe",
                        format!("cw:unsub:{}", notification.subscription_id),
                    )]]);

                    if let Err(e) = bot.send_message(chat_id, &text).reply_markup(keyboard).await {
                        handle_send_error(e, notification.user_id, &db_pool);
                    }
                }
                _ => {
                    log::warn!(
                        "Unknown notification content type: {}",
                        notification.update.content_type
                    );
                }
            }
        }
        log::warn!("Notification dispatcher channel closed");
    });
}

/// Handle send errors — deactivate subscriptions if bot is blocked.
fn handle_send_error(err: teloxide::RequestError, user_id: i64, db_pool: &Arc<DbPool>) {
    let err_str = err.to_string();
    if err_str.contains("Forbidden") || err_str.contains("blocked") || err_str.contains("deactivated") {
        log::warn!("Bot blocked by user {}, deactivating all subscriptions", user_id);
        if let Ok(conn) = get_connection(db_pool) {
            let _ = watcher_db::deactivate_all_for_user(&conn, user_id);
        }
    } else {
        log::error!("Failed to send notification to {}: {}", user_id, err);
    }
}
