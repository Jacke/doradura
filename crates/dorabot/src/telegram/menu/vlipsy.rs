//! In-bot Vlipsy search: button-based search, results display, and clip sending.
//!
//! Flow: menu button → search prompt → user types query → results with pagination → select → send clip.
//!
//! Callback data prefixes:
//! - `vl:s:{id}` — select clip by ID
//! - `vl:p:{page}:{query}` — paginate results
//! - `vl:search` — initiate search prompt

use crate::i18n;
use crate::storage::db::DbPool;
use crate::storage::SharedStorage;
use crate::telegram::Bot;
use crate::vlipsy::VlipsyClient;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{
    CallbackQueryId, ChatId, InlineKeyboardButton, InlineKeyboardMarkup, InputFile, MessageId, ParseMode,
};
use unic_langid::LanguageIdentifier;

const RESULTS_PER_PAGE: u32 = 5;

// ── State tracking ──────────────────────────────────────────────────────────

const VLIPSY_SEARCH_PROMPT_KIND: &str = "vlipsy_search";
const VLIPSY_SEARCH_TTL_SECS: i64 = 300;

pub async fn is_waiting_for_vlipsy_search(shared_storage: &Arc<SharedStorage>, user_id: i64) -> bool {
    shared_storage
        .get_prompt_session(user_id, VLIPSY_SEARCH_PROMPT_KIND)
        .await
        .ok()
        .flatten()
        .is_some()
}

pub async fn set_waiting_for_vlipsy_search(shared_storage: &Arc<SharedStorage>, user_id: i64, waiting: bool) {
    if waiting {
        let _ = shared_storage
            .upsert_prompt_session(user_id, VLIPSY_SEARCH_PROMPT_KIND, "", VLIPSY_SEARCH_TTL_SECS)
            .await;
    } else {
        let _ = shared_storage
            .delete_prompt_session(user_id, VLIPSY_SEARCH_PROMPT_KIND)
            .await;
    }
}

// ── Search prompt ───────────────────────────────────────────────────────────

pub async fn send_search_prompt(
    bot: &Bot,
    chat_id: ChatId,
    lang: &LanguageIdentifier,
    shared_storage: &Arc<SharedStorage>,
) -> Result<(), teloxide::RequestError> {
    if !crate::vlipsy::is_available() {
        bot.send_message(chat_id, i18n::t(lang, "vlipsy.unavailable"))
            .parse_mode(ParseMode::MarkdownV2)
            .await?;
        return Ok(());
    }

    set_waiting_for_vlipsy_search(shared_storage, chat_id.0, true).await;

    bot.send_message(chat_id, i18n::t(lang, "vlipsy.search_prompt"))
        .parse_mode(ParseMode::MarkdownV2)
        .await?;

    Ok(())
}

// ── Handle user's search text ───────────────────────────────────────────────

pub async fn handle_search_text(
    bot: &Bot,
    chat_id: ChatId,
    text: &str,
    lang: &LanguageIdentifier,
    _db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
) {
    set_waiting_for_vlipsy_search(&shared_storage, chat_id.0, false).await;

    let text = text.trim();
    if text.eq_ignore_ascii_case("cancel") {
        let _ = crate::telegram::show_enhanced_main_menu(bot, chat_id, _db_pool, shared_storage).await;
        return;
    }

    // Send "searching..." status
    let status_msg = bot
        .send_message(chat_id, i18n::t(lang, "vlipsy.searching"))
        .parse_mode(ParseMode::MarkdownV2)
        .await;

    let client = match VlipsyClient::new() {
        Some(c) => c,
        None => {
            let _ = bot
                .send_message(chat_id, i18n::t(lang, "vlipsy.unavailable"))
                .parse_mode(ParseMode::MarkdownV2)
                .await;
            return;
        }
    };

    let result = client.search(text, RESULTS_PER_PAGE, 0).await;

    // Delete status message
    if let Ok(msg) = &status_msg {
        let _ = bot.delete_message(chat_id, msg.id).await;
    }

    match result {
        Ok(response) => {
            if response.results.is_empty() {
                let _ = bot
                    .send_message(chat_id, i18n::t(lang, "vlipsy.no_results"))
                    .parse_mode(ParseMode::MarkdownV2)
                    .await;
            } else {
                let total_pages = response
                    .total
                    .map(|t| ((t as f64) / RESULTS_PER_PAGE as f64).ceil() as u32)
                    .unwrap_or(1);
                let _ = show_results_page(bot, chat_id, &response.results, text, 0, total_pages, lang).await;
            }
        }
        Err(e) => {
            log::error!("Vlipsy search error: {}", e);
            let _ = bot
                .send_message(chat_id, i18n::t(lang, "vlipsy.no_results"))
                .parse_mode(ParseMode::MarkdownV2)
                .await;
        }
    }
}

// ── Display results page ────────────────────────────────────────────────────

async fn show_results_page(
    bot: &Bot,
    chat_id: ChatId,
    results: &[crate::vlipsy::Vlip],
    query: &str,
    page: u32,
    total_pages: u32,
    lang: &LanguageIdentifier,
) -> Result<(), teloxide::RequestError> {
    let mut rows: Vec<Vec<InlineKeyboardButton>> = Vec::new();

    for vlip in results {
        let title = vlip.display_title();
        let source_label = vlip
            .source
            .as_deref()
            .map(|s| format!("{} — {}", title, s))
            .unwrap_or_else(|| title.to_string());

        // Truncate label to fit button display
        let label = if source_label.len() > 45 {
            format!("{}...", &source_label[..42])
        } else {
            source_label
        };

        let cb_data = format!("vl:s:{}", vlip.id);
        rows.push(vec![InlineKeyboardButton::callback(label, cb_data)]);
    }

    // Pagination row
    let mut nav_row: Vec<InlineKeyboardButton> = Vec::new();
    if page > 0 {
        let prev_data = build_page_callback(page - 1, query);
        nav_row.push(InlineKeyboardButton::callback(
            i18n::t(lang, "vlipsy.prev_page"),
            prev_data,
        ));
    }
    if page + 1 < total_pages {
        let next_data = build_page_callback(page + 1, query);
        nav_row.push(InlineKeyboardButton::callback(
            i18n::t(lang, "vlipsy.next_page"),
            next_data,
        ));
    }
    if !nav_row.is_empty() {
        rows.push(nav_row);
    }

    let keyboard = InlineKeyboardMarkup::new(rows);

    // Build title with query and page info
    let mut title_args = fluent_templates::fluent_bundle::FluentArgs::new();
    title_args.set("query", query.to_string());
    let title_text = i18n::t_args(lang, "vlipsy.result_title", &title_args);

    let mut page_args = fluent_templates::fluent_bundle::FluentArgs::new();
    page_args.set("page", (page + 1) as i64);
    page_args.set("total_pages", total_pages as i64);
    let page_text = i18n::t_args(lang, "vlipsy.page_info", &page_args);

    let text = format!("{}\n{}", title_text, page_text);

    bot.send_message(chat_id, text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(keyboard)
        .await?;

    Ok(())
}

/// Build a pagination callback, safely truncating the query to fit 64-byte limit.
fn build_page_callback(page: u32, query: &str) -> String {
    let prefix = format!("vl:p:{}:", page);
    let max_query_bytes = 64 - prefix.len();

    // Truncate query at UTF-8 boundary
    let truncated = truncate_utf8(query, max_query_bytes);
    format!("{}{}", prefix, truncated)
}

fn truncate_utf8(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

// ── Callback handler ────────────────────────────────────────────────────────

pub async fn handle_vlipsy_callback(
    bot: &Bot,
    callback_id: CallbackQueryId,
    chat_id: ChatId,
    message_id: MessageId,
    data: &str,
    db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
) -> Result<(), teloxide::RequestError> {
    let _ = db_pool;
    let lang = i18n::user_lang_from_storage(&shared_storage, chat_id.0).await;

    if data == "vl:search" {
        let _ = bot.answer_callback_query(callback_id).await;
        send_search_prompt(bot, chat_id, &lang, &shared_storage).await?;
        return Ok(());
    }

    if let Some(clip_id) = data.strip_prefix("vl:s:") {
        let _ = bot.answer_callback_query(callback_id).await;
        send_clip(bot, chat_id, clip_id, &lang).await;
        return Ok(());
    }

    if let Some(rest) = data.strip_prefix("vl:p:") {
        let _ = bot.answer_callback_query(callback_id).await;

        // Parse "page:query"
        if let Some(colon_pos) = rest.find(':') {
            let page: u32 = rest[..colon_pos].parse().unwrap_or(0);
            let query = &rest[colon_pos + 1..];
            let offset = page * RESULTS_PER_PAGE;

            // Delete old results message
            let _ = bot.delete_message(chat_id, message_id).await;

            let client = match VlipsyClient::new() {
                Some(c) => c,
                None => return Ok(()),
            };

            match client.search(query, RESULTS_PER_PAGE, offset).await {
                Ok(response) => {
                    let total_pages = response
                        .total
                        .map(|t| ((t as f64) / RESULTS_PER_PAGE as f64).ceil() as u32)
                        .unwrap_or(1);
                    let _ = show_results_page(bot, chat_id, &response.results, query, page, total_pages, &lang).await;
                }
                Err(e) => {
                    log::error!("Vlipsy pagination error: {}", e);
                    let _ = bot
                        .send_message(chat_id, i18n::t(&lang, "vlipsy.no_results"))
                        .parse_mode(ParseMode::MarkdownV2)
                        .await;
                }
            }
        }
        return Ok(());
    }

    Ok(())
}

// ── Send clip ───────────────────────────────────────────────────────────────

async fn send_clip(bot: &Bot, chat_id: ChatId, clip_id: &str, lang: &LanguageIdentifier) {
    let client = match VlipsyClient::new() {
        Some(c) => c,
        None => {
            let _ = bot
                .send_message(chat_id, i18n::t(lang, "vlipsy.unavailable"))
                .parse_mode(ParseMode::MarkdownV2)
                .await;
            return;
        }
    };

    // Send "sending..." status
    let status_msg = bot
        .send_message(chat_id, i18n::t(lang, "vlipsy.sending"))
        .parse_mode(ParseMode::MarkdownV2)
        .await;

    match client.get_vlip(clip_id).await {
        Ok(resp) => {
            if let Some(vlip) = resp.vlip {
                if let Some(mp4_url) = vlip.mp4_url() {
                    let url: url::Url = match mp4_url.parse() {
                        Ok(u) => u,
                        Err(_) => {
                            log::error!("Invalid MP4 URL from Vlipsy: {}", mp4_url);
                            return;
                        }
                    };

                    match bot
                        .send_video(chat_id, InputFile::url(url))
                        .caption(vlip.display_title())
                        .await
                    {
                        Ok(_) => {
                            log::info!("Sent Vlipsy clip {} to chat {}", clip_id, chat_id);
                        }
                        Err(e) => {
                            log::error!("Failed to send Vlipsy clip: {}", e);
                        }
                    }
                } else {
                    log::error!("No MP4 URL for Vlipsy clip {}", clip_id);
                }
            }
        }
        Err(e) => {
            log::error!("Failed to fetch Vlipsy clip {}: {}", clip_id, e);
        }
    }

    // Delete status message
    if let Ok(msg) = &status_msg {
        let _ = bot.delete_message(chat_id, msg.id).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::db;
    use crate::storage::SharedStorage;
    use std::sync::Arc;
    async fn test_shared_storage() -> Arc<SharedStorage> {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let (_file, path) = tmp.keep().unwrap();
        let db_pool = Arc::new(db::create_pool(path.to_str().unwrap()).unwrap());
        let conn = db::get_connection(&db_pool).unwrap();
        conn.execute(
            "CREATE TABLE IF NOT EXISTS users (
                telegram_id INTEGER PRIMARY KEY
            )",
            [],
        )
        .unwrap();
        SharedStorage::from_sqlite_pool(db_pool).await.unwrap()
    }

    #[test]
    fn test_build_page_callback_short_query() {
        let cb = build_page_callback(2, "funny");
        assert_eq!(cb, "vl:p:2:funny");
        assert!(cb.len() <= 64);
    }

    #[test]
    fn test_build_page_callback_long_query() {
        let long_query = "a".repeat(100);
        let cb = build_page_callback(0, &long_query);
        assert!(cb.len() <= 64, "Callback too long: {} bytes", cb.len());
        assert!(cb.starts_with("vl:p:0:"));
    }

    #[test]
    fn test_build_page_callback_utf8_safety() {
        // Cyrillic query that could be cut mid-character
        let query = "смешные реакции из фильмов и сериалов для всех";
        let cb = build_page_callback(1, query);
        assert!(cb.len() <= 64, "Callback too long: {} bytes", cb.len());
        // Verify it's valid UTF-8
        assert!(std::str::from_utf8(cb.as_bytes()).is_ok());
    }

    #[test]
    fn test_truncate_utf8_ascii() {
        assert_eq!(truncate_utf8("hello", 3), "hel");
        assert_eq!(truncate_utf8("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_utf8_cyrillic() {
        let s = "Привет"; // 12 bytes (2 bytes per char)
        assert_eq!(truncate_utf8(s, 4), "Пр"); // 2 chars = 4 bytes
        assert_eq!(truncate_utf8(s, 5), "Пр"); // 5 is mid-char, rounds down to 4
    }

    #[tokio::test]
    async fn test_state_tracking() {
        let shared_storage = test_shared_storage().await;
        let user_id = 999_999_999;
        let conn = db::get_connection(&shared_storage.sqlite_pool()).unwrap();
        conn.execute("INSERT INTO users (telegram_id) VALUES (?1)", [user_id])
            .unwrap();
        assert!(!is_waiting_for_vlipsy_search(&shared_storage, user_id).await);

        set_waiting_for_vlipsy_search(&shared_storage, user_id, true).await;
        assert!(is_waiting_for_vlipsy_search(&shared_storage, user_id).await);

        set_waiting_for_vlipsy_search(&shared_storage, user_id, false).await;
        assert!(!is_waiting_for_vlipsy_search(&shared_storage, user_id).await);
    }
}
