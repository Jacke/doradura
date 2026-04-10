use crate::storage::db::DbPool;
use crate::storage::SharedStorage;
use crate::telegram::admin;
use crate::telegram::Bot;
use crate::telegram::BotExt;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{CallbackQueryId, InlineKeyboardMarkup};

use super::admin_users;

/// Handles admin-related callback queries: `admin:`, `analytics:`, `metrics:`, and `au:` prefixes.
///
/// Returns `Ok(true)` if the callback was handled, `Ok(false)` if it was not recognized.
pub async fn handle_admin_callback(
    bot: &Bot,
    callback_id: &CallbackQueryId,
    chat_id: ChatId,
    message_id: teloxide::types::MessageId,
    data: &str,
    from: &teloxide::types::User,
    db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
) -> ResponseResult<bool> {
    if data.starts_with("analytics:") {
        let _ = bot.answer_callback_query(callback_id.clone()).await;

        let is_admin = i64::try_from(from.id.0).ok().map(admin::is_admin).unwrap_or(false);

        if !is_admin {
            bot.send_message(chat_id, "❌ You don't have permission to execute this command.")
                .await?;
            return Ok(true);
        }

        match data {
            "analytics:refresh" => {
                use crate::telegram::analytics::generate_analytics_dashboard;
                let dashboard = generate_analytics_dashboard(&db_pool, &shared_storage).await;

                let keyboard = InlineKeyboardMarkup::new(vec![
                    vec![
                        crate::telegram::cb("🔄 Refresh", "analytics:refresh"),
                        crate::telegram::cb("📊 Details", "analytics:details"),
                    ],
                    vec![crate::telegram::cb("🔙 Close", "analytics:close")],
                ]);

                bot.edit_md_kb(chat_id, message_id, dashboard, keyboard).await?;
            }
            "analytics:details" => {
                let details_text = "📊 *Detailed Metrics*\n\nSelect a category:";
                let keyboard = InlineKeyboardMarkup::new(vec![
                    vec![crate::telegram::cb("⚡ Performance", "metrics:performance")],
                    vec![crate::telegram::cb("💰 Business", "metrics:business")],
                    vec![crate::telegram::cb("👥 Engagement", "metrics:engagement")],
                    vec![crate::telegram::cb("🔙 Back", "analytics:refresh")],
                ]);

                bot.edit_md_kb(chat_id, message_id, details_text, keyboard).await?;
            }
            "analytics:close" => {
                let _ = bot.delete_message(chat_id, message_id).await;
            }
            _ => {}
        }

        return Ok(true);
    }

    if data.starts_with("metrics:") {
        let _ = bot.answer_callback_query(callback_id.clone()).await;

        let is_admin = i64::try_from(from.id.0).ok().map(admin::is_admin).unwrap_or(false);

        if !is_admin {
            bot.send_message(chat_id, "❌ You don't have permission to execute this command.")
                .await?;
            return Ok(true);
        }

        let category = data.strip_prefix("metrics:").unwrap_or("");

        use crate::telegram::analytics::generate_metrics_report;
        let metrics_text = generate_metrics_report(&db_pool, &shared_storage, Some(category.to_string())).await;

        let keyboard = InlineKeyboardMarkup::new(vec![vec![crate::telegram::cb(
            "🔙 To main dashboard",
            "analytics:refresh",
        )]]);

        bot.edit_md_kb(chat_id, message_id, metrics_text, keyboard).await?;

        return Ok(true);
    }

    if data.starts_with("au:") {
        let _ = bot.answer_callback_query(callback_id.clone()).await;
        let is_admin = i64::try_from(from.id.0).ok().map(admin::is_admin).unwrap_or(false);
        if !is_admin {
            bot.send_message(chat_id, "❌ You don't have permission to execute this command.")
                .await?;
            return Ok(true);
        }
        if let Err(e) = admin_users::handle_callback(bot, chat_id, message_id, &shared_storage, data).await {
            log::error!("Admin users callback error: {}", e);
        }
        return Ok(true);
    }

    if data.starts_with("admin:") {
        let _ = bot.answer_callback_query(callback_id.clone()).await;

        let is_admin = i64::try_from(from.id.0).ok().map(admin::is_admin).unwrap_or(false);

        if !is_admin {
            bot.send_message(chat_id, "❌ You don't have permission to execute this command.")
                .await?;
            return Ok(true);
        }

        // Handle browser/cookie manager callbacks
        if data.starts_with("admin:browser_") {
            if let Err(e) =
                admin::handle_browser_callback(bot, callback_id.to_string(), chat_id, message_id, data).await
            {
                log::error!("Failed to handle browser callback: {}", e);
            }
            return Ok(true);
        }

        if data == "admin:update_ytdlp" {
            if let Err(e) = admin::handle_update_ytdlp_callback(bot, chat_id, message_id).await {
                log::error!("Failed to handle update_ytdlp callback: {}", e);
            }
            return Ok(true);
        }

        if data == "admin:check_ytdlp_version" {
            if let Err(e) = admin::handle_check_ytdlp_version_callback(bot, chat_id, message_id).await {
                log::error!("Failed to handle check_ytdlp_version callback: {}", e);
            }
            return Ok(true);
        }

        if data == "admin:test_cookies" {
            if let Err(e) = admin::handle_test_cookies_callback(bot, chat_id, message_id).await {
                log::error!("Failed to handle test_cookies callback: {}", e);
            }
            return Ok(true);
        }

        return Ok(true);
    }

    Ok(false)
}
