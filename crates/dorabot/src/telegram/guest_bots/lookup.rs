//! Path A/C/B dispatcher for guest_message replies.
//!
//! Lookup precedence:
//!   - **Path C** — global `popular_files` cache. Hit = sub-second reply.
//!   - **Path A** — caller's personal `download_history`. Same UX as C but
//!     scoped to the user who summoned the bot.
//!   - **Path B** — `InlineQueryResultArticle` with deep-link into DM so
//!     the full download pipeline can run with the user's attention.
//!
//! Hits on Path A also write through to the global cache so subsequent
//! guest_query for the same URL pulls Path C (faster, no per-user lookup).

use std::sync::Arc;

use anyhow::Result;
use doracore::storage::cache;
use doracore::storage::db::{DbPool, PopularFileEntry};

use crate::storage::SharedStorage;

use super::intent::{GuestFormat, ParsedIntent};
use super::reply;

pub async fn resolve_and_reply(
    bot_token: &str,
    bot_username: &str,
    query_id: &str,
    caller_user_id: i64,
    intent: &ParsedIntent,
    shared: &SharedStorage,
    db_pool: &Arc<DbPool>,
) -> Result<()> {
    let format = intent.format;
    let db_format = format.db_key();

    // Path C — global popular cache (hot path).
    if let Some(entry) = shared.lookup_popular_file(&intent.url, db_format).await.ok().flatten() {
        log::info!(
            "guest_message Path C hit: url={} format={} hits={}",
            intent.url,
            db_format,
            entry.hits
        );
        return send_cached(bot_token, query_id, format, &entry).await;
    }

    // Path A — personal history of the caller.
    if let Some((file_id, title, author, _duration, _size)) = shared
        .lookup_personal_file(caller_user_id, &intent.url, db_format)
        .await
        .ok()
        .flatten()
    {
        log::info!("guest_message Path A hit: url={} caller={}", intent.url, caller_user_id);
        // Write through to global cache so the next viral hit goes Path C.
        let _ = shared
            .upsert_popular_file(
                &intent.url,
                db_format,
                &file_id,
                title.as_deref(),
                author.as_deref(),
                None,
                None,
            )
            .await;
        let entry = PopularFileEntry {
            url: intent.url.clone(),
            format: db_format.to_string(),
            file_id,
            title,
            author,
            duration: None,
            file_size: None,
            hits: 1,
        };
        return send_cached(bot_token, query_id, format, &entry).await;
    }

    // Path B — fallback article with deep-link into DM.
    log::info!(
        "guest_message Path B (no cache): url={} format={:?}",
        intent.url,
        format
    );
    let url_id = cache::store_url(db_pool, Some(shared), &intent.url).await;
    let payload = encode_deeplink_payload(&url_id, format);
    let deep_link = format!("https://t.me/{}?start={}", bot_username, payload);

    let title = match format {
        GuestFormat::Mp3 => "🎵 Скачать MP3",
        GuestFormat::Mp4 => "🎬 Скачать MP4",
        GuestFormat::Auto => "🔽 Скачать",
    };
    let description = "Открой меня — скачаю за ~30 секунд и пришлю в личку.";

    reply::answer_article_with_deeplink(bot_token, query_id, title, description, &deep_link, "🔽 Открыть в боте").await
}

async fn send_cached(bot_token: &str, query_id: &str, format: GuestFormat, entry: &PopularFileEntry) -> Result<()> {
    match format {
        GuestFormat::Mp3 => {
            reply::answer_cached_audio(
                bot_token,
                query_id,
                &entry.file_id,
                entry.title.as_deref(),
                entry.author.as_deref(),
            )
            .await
        }
        GuestFormat::Mp4 | GuestFormat::Auto => {
            reply::answer_cached_video(bot_token, query_id, &entry.file_id, entry.title.as_deref()).await
        }
    }
}

/// Deep-link `start` payload: Telegram caps at 64 chars and only allows
/// `[A-Za-z0-9_-]`. Our URL cache returns short alphanumeric IDs already
/// (see `doracore::storage::cache`), so we just suffix the format code.
fn encode_deeplink_payload(url_id: &str, format: GuestFormat) -> String {
    let fmt = match format {
        GuestFormat::Mp3 => "a",
        GuestFormat::Mp4 => "v",
        GuestFormat::Auto => "p", // "pick" — show preview card on landing
    };
    format!("dl_{}_{}", url_id, fmt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deeplink_payload_fits_telegram_limit() {
        // Real-world URL cache IDs are 8-12 alphanum chars; the prefix
        // adds 5 ("dl_") + 2 ("_a"). Even with a generous 20-char id we
        // stay well under Telegram's 64-byte cap.
        let p = encode_deeplink_payload("a".repeat(20).as_str(), GuestFormat::Mp3);
        assert!(p.len() <= 64);
        assert!(p.starts_with("dl_"));
        assert!(p.ends_with("_a"));
    }

    #[test]
    fn deeplink_format_suffixes() {
        assert!(encode_deeplink_payload("x", GuestFormat::Mp3).ends_with("_a"));
        assert!(encode_deeplink_payload("x", GuestFormat::Mp4).ends_with("_v"));
        assert!(encode_deeplink_payload("x", GuestFormat::Auto).ends_with("_p"));
    }
}
