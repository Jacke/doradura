//! Info-feature callback handling — `info:` prefix.
//!
//! Non-download actions on a previewed URL: max-resolution thumbnail,
//! geo-availability check, full metadata card. Each action reads from
//! `EXTENDED_METADATA_CACHE` (populated alongside `PREVIEW_CACHE` during
//! the preview fetch); cache miss falls through to a fresh
//! `yt-dlp --dump-json` invocation.
//!
//! Callback shape: `info:{action}:{url_id}` where `action ∈
//! {menu, thumb, geo, meta}` and `url_id` is the short hash resolved
//! via `cache::get_url`.
//!
//! v0.51.0-alpha.2: scaffolding — `menu` opens the submenu, sub-actions
//! show a "coming soon" answer until alpha.3 implements them.

use std::sync::Arc;

use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};

use crate::storage::SharedStorage;
use crate::storage::db::DbPool;
use crate::telegram::Bot;

/// Entry point for `info:*` callback queries.
///
/// Parses `info:{action}:{url_id}` and dispatches. All sub-actions
/// answer the callback query so the spinner clears.
pub async fn handle_info_callback(
    bot: &Bot,
    callback_id: teloxide::types::CallbackQueryId,
    chat_id: ChatId,
    message_id: teloxide::types::MessageId,
    data: &str,
    _db_pool: Arc<DbPool>,
    _shared_storage: Arc<SharedStorage>,
) -> ResponseResult<()> {
    let parts: Vec<&str> = data.splitn(3, ':').collect();
    if parts.len() != 3 {
        let _ = bot.answer_callback_query(callback_id).await;
        return Ok(());
    }
    let action = parts[1];
    let url_id = parts[2];

    match action {
        "menu" => {
            let _ = bot.answer_callback_query(callback_id).await;
            show_info_menu(bot, chat_id, message_id, url_id).await;
        }
        "thumb" | "geo" | "meta" => {
            // alpha.3: implement. For now, a friendly placeholder so the
            // user sees the wiring is alive without seeing a stack trace.
            let _ = bot
                .answer_callback_query(callback_id)
                .text("⏳ Скоро будет готово")
                .show_alert(true)
                .await;
        }
        _ => {
            let _ = bot.answer_callback_query(callback_id).await;
        }
    }

    Ok(())
}

/// Render the Info submenu — 3 action buttons + Cancel.
///
/// Edits the existing preview message's keyboard (no new message
/// spawned — keeps the chat clean).
async fn show_info_menu(bot: &Bot, chat_id: ChatId, message_id: teloxide::types::MessageId, url_id: &str) {
    let buttons = vec![
        vec![InlineKeyboardButton::callback(
            "🖼 Скачать обложку (max-res)",
            format!("info:thumb:{}", url_id),
        )],
        vec![InlineKeyboardButton::callback(
            "🌍 Доступность по странам",
            format!("info:geo:{}", url_id),
        )],
        vec![InlineKeyboardButton::callback(
            "📋 Полные метаданные",
            format!("info:meta:{}", url_id),
        )],
        vec![InlineKeyboardButton::callback(
            "❌ Отмена",
            format!("pv:cancel:{}", url_id),
        )],
    ];
    let keyboard = InlineKeyboardMarkup::new(buttons);

    if let Err(e) = bot
        .edit_message_reply_markup(chat_id, message_id)
        .reply_markup(keyboard)
        .await
    {
        log::warn!("info: failed to edit submenu keyboard: {:?}", e);
    }
}
