//! Inline query handler.
//!
//! Two modes triggered by the query text:
//!   - **URL detected** (`@bot https://yt.be/x`) → return cached file_ids
//!     from popular_files (V48) as `InlineQueryResultCached*`, plus an
//!     `InlineQueryResultArticle` deep-link fallback that bounces into DM
//!     when nothing is cached. This is the alpha.30 inline-mode counterpart
//!     to the alpha.29 Guest Bots flow — works in **private DMs** where
//!     guest_message isn't applicable.
//!   - **Free text** (`@bot reaction face`) → search Vlipsy for video
//!     reactions. Original behaviour, untouched.

use crate::storage::{DbPool, SharedStorage};
use crate::vlipsy::VlipsyClient;
use doracore::storage::cache;
use lazy_regex::{Lazy, Regex, lazy_regex};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::LazyLock;
use std::time::Instant;
use teloxide::prelude::*;
use teloxide::types::{
    FileId, InlineQueryResult, InlineQueryResultArticle, InlineQueryResultCachedAudio, InlineQueryResultCachedVideo,
    InlineQueryResultVideo, InputMessageContent, InputMessageContentText,
};
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};
use tokio::sync::RwLock;

const RESULTS_PER_PAGE: u32 = 10;

/// URL regex — mirrors `telegram::commands::URL_REGEX` so inline detection
/// stays in sync with text-message detection.
static URL_REGEX: Lazy<Regex> = lazy_regex!(r"https?://[^\s]+");

/// Per-user rate limiter for inline queries.
static INLINE_RATE: LazyLock<RwLock<HashMap<u64, Instant>>> = LazyLock::new(|| RwLock::new(HashMap::new()));

/// Minimum milliseconds between inline query responses for the same user.
const INLINE_COOLDOWN_MS: u128 = 500;

/// Handle inline queries — see module docs for the URL-vs-text split.
///
/// `shared_storage` + `db_pool` + `bot_username` are needed for the URL
/// branch (popular_files lookup + deep-link generation). The Vlipsy fallback
/// ignores them.
pub async fn handle_inline_query(
    bot: crate::telegram::Bot,
    query: InlineQuery,
    shared_storage: Arc<SharedStorage>,
    db_pool: Arc<DbPool>,
    bot_username: &str,
) -> ResponseResult<()> {
    // Per-user rate limit: reject requests faster than INLINE_COOLDOWN_MS
    {
        let mut rates = INLINE_RATE.write().await;
        // Evict entries older than 60 seconds to prevent unbounded map growth
        rates.retain(|_, ts| ts.elapsed().as_millis() < 60_000);
        if let Some(last) = rates.get(&query.from.id.0)
            && last.elapsed().as_millis() < INLINE_COOLDOWN_MS
        {
            let _ = bot.answer_inline_query(query.id.clone(), vec![]).await;
            return Ok(());
        }
        rates.insert(query.from.id.0, Instant::now());
    }

    // URL-mode short-circuit (alpha.30) — if query contains a URL, return
    // cached file_ids / deep-link instead of Vlipsy search results.
    if let Some(url) = URL_REGEX
        .find(&query.query)
        .map(|m| m.as_str().trim_end_matches(['.', ',', ')', ']']).to_string())
    {
        let results = build_url_results(&shared_storage, &db_pool, bot_username, &url).await;
        bot.answer_inline_query(query.id, results)
            .cache_time(60)
            .is_personal(true)
            .await?;
        return Ok(());
    }

    let client = match VlipsyClient::new() {
        Some(c) => c,
        None => {
            // Vlipsy not configured — return empty results
            bot.answer_inline_query(query.id, Vec::<InlineQueryResult>::new())
                .cache_time(300)
                .await?;
            return Ok(());
        }
    };

    let offset: u32 = query.offset.parse().unwrap_or(0);
    let search_query = query.query.trim().to_string();

    let api_result = if search_query.is_empty() {
        client.trending(RESULTS_PER_PAGE, offset).await
    } else {
        client.search(&search_query, RESULTS_PER_PAGE, offset).await
    };

    let response = match api_result {
        Ok(r) => r,
        Err(e) => {
            log::error!("Vlipsy inline query error: {}", e);
            bot.answer_inline_query(query.id, Vec::<InlineQueryResult>::new())
                .cache_time(60)
                .await?;
            return Ok(());
        }
    };

    let mut results: Vec<InlineQueryResult> = Vec::new();
    let mp4_mime: mime::Mime = "video/mp4".parse().unwrap();

    for vlip in &response.results {
        if let Some(mp4_url) = vlip.mp4_url() {
            let title = vlip.display_title().to_string();
            let thumb_url = vlip.thumb_url().unwrap_or(mp4_url);

            let Ok(video_url) = mp4_url.parse() else { continue };
            let Ok(thumb) = thumb_url.parse() else { continue };

            let result = InlineQueryResultVideo::new(vlip.id.clone(), video_url, mp4_mime.clone(), thumb, title);

            results.push(InlineQueryResult::Video(result));
        }
    }

    // Calculate next offset for pagination
    let next_offset = if results.len() == RESULTS_PER_PAGE as usize {
        (offset + RESULTS_PER_PAGE).to_string()
    } else {
        String::new()
    };

    bot.answer_inline_query(query.id, results)
        .cache_time(300)
        .is_personal(false)
        .next_offset(next_offset)
        .await?;

    Ok(())
}

/// Build inline results for a URL query. Layout:
///   - If popular_files has (url, mp3) → InlineQueryResultCachedAudio
///   - If popular_files has (url, mp4) → InlineQueryResultCachedVideo
///   - Always: InlineQueryResultArticle with deep-link "open in DM" fallback
///
/// Order matters — cached results appear first so the user can tap-to-send
/// in one motion; the article is the "I'll wait for download" path.
async fn build_url_results(
    shared: &SharedStorage,
    db_pool: &Arc<DbPool>,
    bot_username: &str,
    url: &str,
) -> Vec<InlineQueryResult> {
    let mut out: Vec<InlineQueryResult> = Vec::new();

    // Cached MP3 (Path C — global popular cache).
    if let Some(entry) = shared.lookup_popular_file(url, "mp3").await.ok().flatten() {
        let title = entry.title.clone().unwrap_or_else(|| "🎵 Audio".to_string());
        let id = format!("u_mp3_{}", &entry.file_id[..entry.file_id.len().min(10)]);
        let mut r = InlineQueryResultCachedAudio::new(id, FileId(entry.file_id.clone()));
        if let Some(author) = entry.author.as_deref() {
            r.caption = Some(format!("🎵 {} — {}", author, title));
        } else {
            r.caption = Some(format!("🎵 {}", title));
        }
        out.push(InlineQueryResult::CachedAudio(r));
    }

    // Cached MP4 (Path C — global popular cache).
    if let Some(entry) = shared.lookup_popular_file(url, "mp4").await.ok().flatten() {
        let title = entry.title.clone().unwrap_or_else(|| "🎬 Video".to_string());
        let id = format!("u_mp4_{}", &entry.file_id[..entry.file_id.len().min(10)]);
        let mut r = InlineQueryResultCachedVideo::new(id, FileId(entry.file_id.clone()), title.clone());
        r.description = Some("MP4 · из кэша Doradura".to_string());
        out.push(InlineQueryResult::CachedVideo(r));
    }

    // Always include deep-link article — gives the user an option to wait
    // for a fresh download (or pick a different format) by bouncing to DM.
    let url_id = cache::store_url(db_pool, Some(shared), url).await;
    let deep_link = format!("https://t.me/{}?start=dl_{}_p", bot_username, url_id);
    let id_article = format!("u_dl_{}", &url_id[..url_id.len().min(10)]);
    let mut article = InlineQueryResultArticle::new(
        id_article,
        "🔽 Скачать в боте",
        InputMessageContent::Text(InputMessageContentText::new(format!(
            "🔽 Открой бота — скачаю и пришлю в личку:\n{}",
            deep_link
        ))),
    );
    article.description = Some("MP3 / MP4 / circle / ringtone — выбор форматов".to_string());
    article.reply_markup = Some(InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::url(
        "🔽 Открыть в боте",
        deep_link.parse().expect("valid t.me URL"),
    )]]));
    out.push(InlineQueryResult::Article(article));

    out
}
