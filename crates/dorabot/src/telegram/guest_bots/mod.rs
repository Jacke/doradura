//! Bot API 10.0 "Guest Bots" handler — alpha.29.
//!
//! When a user @-mentions our bot in a chat we aren't a member of, Telegram
//! sends an `Update` containing a `guest_message` field with a one-shot
//! `guest_query_id`. We have ~60s to respond once via `answerGuestQuery`
//! using an `InlineQueryResult` (file_id-cached audio/video or article).
//!
//! teloxide master (pinned in alpha.26) doesn't expose `guest_message` yet,
//! so we intercept raw JSON updates and dispatch through this module
//! without going through the teloxide dispatcher.
//!
//! Lookup precedence (fastest → slowest):
//!   1. **Path C** — global popular_files cache (any user once downloaded it)
//!   2. **Path A** — this caller's personal download_history
//!   3. **Path B** — fallback `InlineQueryResultArticle` with a deep-link
//!      `https://t.me/{bot_username}?start=dl_<urlid>_<format>` that
//!      bounces the user into DM where the full download pipeline runs.
//!
//! All three paths share the same `answerGuestQuery` POST helper in
//! [`reply`]; the difference is purely which `InlineQueryResult` payload
//! is constructed in [`lookup`].

pub mod intent;
pub mod lookup;
pub mod rate_limit;
pub mod reply;

use std::sync::Arc;

use serde_json::Value;

use crate::storage::SharedStorage;
use crate::storage::db::DbPool;

/// Top-level entry from the raw-update intercept hook.
///
/// Returns `true` if this update was a guest_message that we handled (so the
/// caller can skip teloxide's normal dispatch path). Returns `false` if the
/// update isn't a guest_message — caller should hand it to teloxide as usual.
pub async fn try_handle_guest_update(
    raw: &Value,
    bot_token: &str,
    bot_username: &str,
    db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
) -> bool {
    let Some(gm) = raw.get("guest_message") else {
        return false;
    };

    let Some(query_id) = gm.get("guest_query_id").and_then(|v| v.as_str()) else {
        log::warn!("guest_message without guest_query_id — skipping");
        return true; // we ate it; nothing useful for teloxide
    };

    let caller_user_id = gm
        .get("from")
        .or_else(|| gm.get("guest_bot_caller_user"))
        .and_then(|v| v.get("id"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let caller_chat_id = gm
        .get("guest_bot_caller_chat")
        .and_then(|v| v.get("id"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    // Anti-spam: bail before any DB work if this (chat, user) is throttled.
    if !rate_limit::check(caller_chat_id, caller_user_id) {
        log::info!(
            "guest_message rate-limited: chat={} user={}",
            caller_chat_id,
            caller_user_id
        );
        return true;
    }

    let mention_text = gm.get("text").and_then(|v| v.as_str()).unwrap_or("");
    let reply_text = gm
        .get("reply_to_message")
        .and_then(|m| m.get("text"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let Some(parsed) = intent::parse(mention_text, reply_text) else {
        // No URL anywhere — politely tell the caller in-place.
        if let Err(e) = reply::answer_article_text(
            bot_token,
            query_id,
            "🤔 Не нашёл ссылки",
            "Прикрепи ссылку (YouTube, Instagram, TikTok, …) или ответь этим mention на сообщение со ссылкой.",
        )
        .await
        {
            log::warn!("answer_article_text failed: {e:?}");
        }
        return true;
    };

    // Path C → Path A → Path B.
    let result = lookup::resolve_and_reply(
        bot_token,
        bot_username,
        query_id,
        caller_user_id,
        &parsed,
        &shared_storage,
        &db_pool,
    )
    .await;
    if let Err(e) = result {
        log::warn!("guest_message dispatch error: {e:?}");
    }
    true
}
