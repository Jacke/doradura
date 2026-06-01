//! Inline query handler — top-UX upgrade (v0.51.0-alpha.34).
//!
//! Three dispatch modes, picked from the query text:
//!
//! 1. **URL** (`@bot https://yt.be/x`) → `url_mode`: canonicalize the URL, ask
//!    `popular_files` for *every* format we've ever cached for it (mp3 / mp4 /
//!    m4r / video_note / gif / cut), and emit a Cached* result per row plus a
//!    deep-link Article fallback (with YouTube thumbnail when applicable).
//!
//! 2. **Empty** (`@bot ` then space) → `recents_mode`: fetch the caller's last
//!    15 entries from `download_history` and surface them as Cached* results
//!    — a "your recents" picker mirroring how @gif handles its empty state.
//!
//! 3. **Free text** (`@bot Дора Дорадура`) → `personal_text_search`: search
//!    the caller's own `download_history` by title/author (existing
//!    `get_download_history_filtered` does AND/OR semantics with the `" - "`
//!    convention), pin matches at the top, append Vlipsy reaction GIFs as a
//!    fallback so the original `@bot reaction face` UX still works.
//!
//! Every response carries a persistent **"🔽 Открыть Doradura"** button above
//! the results via the new `InlineQueryResultsButton::StartParameter` API —
//! the conversion funnel from inline-in-group to DM.

use crate::storage::{DbPool, SharedStorage};
use crate::vlipsy::VlipsyClient;
use doracore::download::url_canonical::canonicalize_url;
use doracore::storage::cache;
use doracore::storage::db::{DownloadHistoryEntry, PopularFileEntry};
use lazy_regex::{Lazy, Regex, lazy_regex};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::LazyLock;
use std::time::Instant;
use teloxide::prelude::*;
use teloxide::types::{
    FileId, InlineQueryResult, InlineQueryResultArticle, InlineQueryResultCachedAudio, InlineQueryResultCachedGif,
    InlineQueryResultCachedVideo, InlineQueryResultVideo, InlineQueryResultsButton, InlineQueryResultsButtonKind,
    InputMessageContent, InputMessageContentText,
};
use tokio::sync::RwLock;

const RESULTS_PER_PAGE: u32 = 10;
const PERSONAL_RESULT_LIMIT: usize = 20;
const RECENTS_LIMIT: i32 = 15;
const VLIPSY_FALLBACK_LIMIT: usize = 10;

/// URL regex — mirrors `telegram::commands::URL_REGEX` so inline detection
/// stays in sync with text-message detection.
static URL_REGEX: Lazy<Regex> = lazy_regex!(r"https?://[^\s]+");

/// Per-user rate limiter for inline queries.
static INLINE_RATE: LazyLock<RwLock<HashMap<u64, Instant>>> = LazyLock::new(|| RwLock::new(HashMap::new()));

/// Minimum milliseconds between inline query responses for the same user.
const INLINE_COOLDOWN_MS: u128 = 500;

/// Handle inline queries — see module docs for the three-way dispatch.
pub async fn handle_inline_query(
    bot: crate::telegram::Bot,
    query: InlineQuery,
    shared_storage: Arc<SharedStorage>,
    db_pool: Arc<DbPool>,
    bot_username: &str,
) -> ResponseResult<()> {
    // Per-user rate limit: reject requests faster than INLINE_COOLDOWN_MS.
    {
        let mut rates = INLINE_RATE.write().await;
        rates.retain(|_, ts| ts.elapsed().as_millis() < 60_000);
        if let Some(last) = rates.get(&query.from.id.0)
            && last.elapsed().as_millis() < INLINE_COOLDOWN_MS
        {
            let _ = bot.answer_inline_query(query.id.clone(), vec![]).await;
            return Ok(());
        }
        rates.insert(query.from.id.0, Instant::now());
    }

    let user_id = query.from.id.0 as i64;
    let raw = query.query.clone();
    let trimmed = raw.trim();

    // ── Dispatch ─────────────────────────────────────────────────────────
    let (results, next_offset, is_personal) = if let Some(m) = URL_REGEX.find(trimmed) {
        let url = m.as_str().trim_end_matches(['.', ',', ')', ']']).to_string();
        let results = url_mode(&shared_storage, &db_pool, bot_username, &url).await;
        (results, String::new(), true)
    } else if trimmed.is_empty() {
        let results = recents_mode(&shared_storage, &db_pool, bot_username, user_id).await;
        (results, String::new(), true)
    } else {
        let offset: u32 = query.offset.parse().unwrap_or(0);
        let (results, next) = personal_text_search(&shared_storage, &bot, user_id, trimmed, offset).await;
        // Personal results may include Vlipsy fallback; mark as personal so
        // Telegram doesn't share the cached response across users.
        (results, next, true)
    };

    // Persistent conversion-funnel button above all results.
    let funnel = InlineQueryResultsButton {
        text: "🔽 Открыть Doradura".to_string(),
        kind: InlineQueryResultsButtonKind::StartParameter("from_inline".to_string()),
    };

    bot.answer_inline_query(query.id, results)
        .cache_time(60)
        .is_personal(is_personal)
        .next_offset(next_offset)
        .button(funnel)
        .await?;
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────
// Common entry type — lets URL mode (PopularFileEntry) and personal modes
// (DownloadHistoryEntry) share one mapping function.
// ─────────────────────────────────────────────────────────────────────────

struct InlineEntry<'a> {
    format: &'a str,
    file_id: &'a str,
    title: &'a str,
    author: Option<&'a str>,
    duration: Option<i64>,
    file_size: Option<i64>,
    video_quality: Option<&'a str>,
    audio_bitrate: Option<&'a str>,
}

impl<'a> InlineEntry<'a> {
    fn from_popular(p: &'a PopularFileEntry) -> Self {
        Self {
            format: p.format.as_str(),
            file_id: p.file_id.as_str(),
            title: p.title.as_deref().unwrap_or("Doradura"),
            author: p.author.as_deref(),
            duration: p.duration,
            file_size: p.file_size,
            // popular_files doesn't cache per-quality variants today, so
            // these stay None in URL mode — captions will gracefully skip.
            video_quality: None,
            audio_bitrate: None,
        }
    }

    fn from_history(h: &'a DownloadHistoryEntry) -> Self {
        Self {
            format: h.format.as_str(),
            file_id: h.file_id.as_deref().unwrap_or(""),
            title: h.title.as_str(),
            author: h.author.as_deref(),
            duration: h.duration,
            file_size: h.file_size,
            video_quality: h.video_quality.as_deref(),
            audio_bitrate: h.audio_bitrate.as_deref(),
        }
    }
}

fn make_audio_result(entry: &InlineEntry<'_>, id: String) -> InlineQueryResult {
    let mut r = InlineQueryResultCachedAudio::new(id, FileId(entry.file_id.to_string()));
    r.caption = Some(caption_audio(
        entry.title,
        entry.author,
        entry.duration,
        entry.audio_bitrate,
    ));
    InlineQueryResult::CachedAudio(r)
}

fn make_video_result(entry: &InlineEntry<'_>, id: String, title_override: Option<String>) -> InlineQueryResult {
    let display_title = title_override.unwrap_or_else(|| display_title_video(entry));
    let mut r = InlineQueryResultCachedVideo::new(id, FileId(entry.file_id.to_string()), display_title);
    r.description = Some(desc_video(entry.video_quality, entry.file_size, entry.duration));
    InlineQueryResult::CachedVideo(r)
}

fn make_gif_result(entry: &InlineEntry<'_>, id: String) -> InlineQueryResult {
    let mut r = InlineQueryResultCachedGif::new(id, FileId(entry.file_id.to_string()));
    if !entry.title.is_empty() {
        r.caption = Some(format!("🖼 {}", entry.title));
    }
    InlineQueryResult::CachedGif(r)
}

/// Build the right Cached* variant for an entry, or `None` if the format
/// can't be represented inline (subtitles, unknown).
fn entry_to_inline_result(entry: &InlineEntry<'_>, seed: usize) -> Option<InlineQueryResult> {
    if entry.file_id.is_empty() {
        return None;
    }
    let id = make_id(entry.format, entry.file_id, seed);
    match entry.format {
        "mp3" | "m4r" => Some(make_audio_result(entry, id)),
        "mp4" | "cut" => Some(make_video_result(entry, id, None)),
        // video_note inline can only be rendered as a video (Bot API has no
        // CachedVideoNote variant). We mark it with ⭕ in the title.
        "video_note" => {
            let title = format!("⭕ {}", entry.title);
            Some(make_video_result(entry, id, Some(title)))
        }
        "gif" => Some(make_gif_result(entry, id)),
        _ => None,
    }
}

/// Unique-per-response ID (1-64 bytes). Combines format + file_id prefix +
/// caller-provided seed so we never collide within a single answer.
fn make_id(format: &str, file_id: &str, seed: usize) -> String {
    let fid = &file_id[..file_id.len().min(20)];
    let mut id = format!("{}_{}_{}", format, fid, seed);
    if id.len() > 64 {
        id.truncate(64);
    }
    id
}

fn display_title_video(entry: &InlineEntry<'_>) -> String {
    if let Some(author) = entry.author {
        format!("{} — {}", author, entry.title)
    } else {
        entry.title.to_string()
    }
}

// ─────────────────────────────────────────────────────────────────────────
// URL mode
// ─────────────────────────────────────────────────────────────────────────

async fn url_mode(
    shared: &SharedStorage,
    db_pool: &Arc<DbPool>,
    bot_username: &str,
    raw_url: &str,
) -> Vec<InlineQueryResult> {
    // FIX (latent bug): popular_files is keyed on canonical URLs by the
    // writer (`pipeline::save_to_history_and_cache`) but the previous reader
    // looked up the raw URL → any `?si=…` tracking variant of a YouTube link
    // would miss the cache. Canonicalize first so reader matches writer.
    let canonical = canonicalize_url(raw_url);

    let cached = shared
        .lookup_popular_file_all_formats(&canonical)
        .await
        .unwrap_or_default();

    let mut out: Vec<InlineQueryResult> = Vec::with_capacity(cached.len() + 1);
    for (seed, entry) in cached.iter().enumerate() {
        let inline = InlineEntry::from_popular(entry);
        if let Some(r) = entry_to_inline_result(&inline, seed) {
            out.push(r);
        }
    }

    // Always append the deep-link article so the user can request a fresh
    // download / a format we haven't cached.
    out.push(build_article(db_pool, shared, bot_username, raw_url).await);
    out
}

// ─────────────────────────────────────────────────────────────────────────
// Recents mode (empty query)
// ─────────────────────────────────────────────────────────────────────────

async fn recents_mode(
    shared: &SharedStorage,
    db_pool: &Arc<DbPool>,
    bot_username: &str,
    user_id: i64,
) -> Vec<InlineQueryResult> {
    let history = shared
        .get_download_history(user_id, Some(RECENTS_LIMIT))
        .await
        .unwrap_or_default();

    let mut out: Vec<InlineQueryResult> = Vec::new();
    for (seed, entry) in history.iter().enumerate() {
        if entry.file_id.is_none() {
            continue;
        }
        let inline = InlineEntry::from_history(entry);
        if let Some(r) = entry_to_inline_result(&inline, seed) {
            out.push(r);
        }
    }

    if out.is_empty() {
        // First-time user with no history — invite them to start.
        out.push(build_empty_recents_article(bot_username));
        // Touch db_pool to keep parity if cache layer ever needs it for
        // first-run analytics; intentional no-op today.
        let _ = db_pool;
    }
    out
}

// ─────────────────────────────────────────────────────────────────────────
// Free-text mode: personal history search + Vlipsy fallback
// ─────────────────────────────────────────────────────────────────────────

async fn personal_text_search(
    shared: &SharedStorage,
    bot: &crate::telegram::Bot,
    user_id: i64,
    query_text: &str,
    offset: u32,
) -> (Vec<InlineQueryResult>, String) {
    // 1. Personal history matches — uses existing AND/OR semantics from
    //    download_history::HistorySearch (e.g. "Author - Title" → AND).
    let personal = shared
        .get_download_history_filtered(user_id, None, Some(query_text), None, None)
        .await
        .unwrap_or_default();

    let mut out: Vec<InlineQueryResult> = Vec::new();
    for (seed, entry) in personal.iter().take(PERSONAL_RESULT_LIMIT).enumerate() {
        if entry.file_id.is_none() {
            continue;
        }
        let inline = InlineEntry::from_history(entry);
        if let Some(r) = entry_to_inline_result(&inline, seed) {
            out.push(r);
        }
    }

    // 2. Vlipsy fallback — runs only if Vlipsy is configured. When personal
    //    matches exist, Vlipsy reactions sit below as bonus content; when
    //    nothing matched personal, Vlipsy is the answer (preserving the
    //    original `@bot funny face` UX).
    let (vlipsy_results, next_offset) = vlipsy_search(bot, query_text, offset).await;
    let personal_empty = out.is_empty();
    let vlipsy_cap = if personal_empty {
        RESULTS_PER_PAGE as usize
    } else {
        VLIPSY_FALLBACK_LIMIT
    };
    for r in vlipsy_results.into_iter().take(vlipsy_cap) {
        out.push(r);
    }

    if out.is_empty() {
        // No personal hits AND Vlipsy returned nothing — show a hint that
        // bounces the user to DM so they can start downloading.
        out.push(build_no_match_article(query_text));
    }

    (out, next_offset)
}

/// Best-effort Vlipsy lookup. Returns (results, next_offset). On any error
/// returns empty so the caller can fall through to the no-match article.
async fn vlipsy_search(_bot: &crate::telegram::Bot, query: &str, offset: u32) -> (Vec<InlineQueryResult>, String) {
    let client = match VlipsyClient::new() {
        Some(c) => c,
        None => return (Vec::new(), String::new()),
    };

    let api_result = if query.is_empty() {
        client.trending(RESULTS_PER_PAGE, offset).await
    } else {
        client.search(query, RESULTS_PER_PAGE, offset).await
    };

    let response = match api_result {
        Ok(r) => r,
        Err(e) => {
            log::error!("Vlipsy inline query error: {}", e);
            return (Vec::new(), String::new());
        }
    };

    let mp4_mime: mime::Mime = "video/mp4".parse().unwrap();
    let mut out: Vec<InlineQueryResult> = Vec::new();
    for vlip in &response.results {
        if let Some(mp4_url) = vlip.mp4_url() {
            let title = vlip.display_title().to_string();
            let thumb_url = vlip.thumb_url().unwrap_or(mp4_url);
            let Ok(video_url) = mp4_url.parse() else { continue };
            let Ok(thumb) = thumb_url.parse() else { continue };
            let result = InlineQueryResultVideo::new(vlip.id.clone(), video_url, mp4_mime.clone(), thumb, title);
            out.push(InlineQueryResult::Video(result));
        }
    }

    let next = if out.len() == RESULTS_PER_PAGE as usize {
        (offset + RESULTS_PER_PAGE).to_string()
    } else {
        String::new()
    };
    (out, next)
}

// ─────────────────────────────────────────────────────────────────────────
// Caption / description helpers
// ─────────────────────────────────────────────────────────────────────────

/// `🎵 Author — Title · 320kbps · 3:42` — segments with `None` are skipped
/// so a partially-populated entry still renders cleanly.
pub(crate) fn caption_audio(title: &str, author: Option<&str>, duration: Option<i64>, bitrate: Option<&str>) -> String {
    let head = match author {
        Some(a) if !a.trim().is_empty() => format!("🎵 {} — {}", a, title),
        _ => format!("🎵 {}", title),
    };
    let mut extras: Vec<String> = Vec::new();
    if let Some(b) = bitrate
        && !b.trim().is_empty()
    {
        extras.push(b.to_string());
    }
    if let Some(d) = duration
        && d > 0
    {
        extras.push(duration_short(d));
    }
    if extras.is_empty() {
        head
    } else {
        format!("{} · {}", head, extras.join(" · "))
    }
}

/// `1080p · 24.0 MB · 3:42` — same skip-None behaviour as `caption_audio`.
pub(crate) fn desc_video(quality: Option<&str>, size: Option<i64>, duration: Option<i64>) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(q) = quality
        && !q.trim().is_empty()
    {
        parts.push(q.to_string());
    }
    if let Some(s) = size
        && s > 0
    {
        parts.push(doracore::core::format_bytes_i64(s));
    }
    if let Some(d) = duration
        && d > 0
    {
        parts.push(duration_short(d));
    }
    if parts.is_empty() {
        "Doradura".to_string()
    } else {
        parts.join(" · ")
    }
}

/// `3:42` for under an hour, `1:23:45` otherwise. Thin wrapper over the
/// shared formatter so call sites stay readable.
pub(crate) fn duration_short(secs: i64) -> String {
    doracore::core::format_media_duration_i64(secs)
}

// ─────────────────────────────────────────────────────────────────────────
// Article fallback (URL mode + empty-recents + no-match)
// ─────────────────────────────────────────────────────────────────────────

async fn build_article(
    db_pool: &Arc<DbPool>,
    shared: &SharedStorage,
    bot_username: &str,
    url: &str,
) -> InlineQueryResult {
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
    article.reply_markup = Some(teloxide::types::InlineKeyboardMarkup::new(vec![vec![
        teloxide::types::InlineKeyboardButton::url("🔽 Открыть в боте", deep_link.parse().expect("valid t.me URL")),
    ]]));

    // YouTube preview thumbnail when the URL looks like a YouTube watch.
    if let Some(thumb) = doracore::core::share::youtube_thumbnail_url(url)
        && let Ok(parsed) = thumb.parse()
    {
        article.thumbnail_url = Some(parsed);
    }
    InlineQueryResult::Article(article)
}

fn build_empty_recents_article(bot_username: &str) -> InlineQueryResult {
    let deep_link = format!("https://t.me/{}?start=from_inline", bot_username);
    let mut article = InlineQueryResultArticle::new(
        "recents_empty",
        "🔽 Открой Doradura",
        InputMessageContent::Text(InputMessageContentText::new(format!(
            "🔽 Открой бота — твои загрузки появятся здесь:\n{}",
            deep_link
        ))),
    );
    article.description = Some("Скачай первый трек — и он появится в этом списке".to_string());
    article.reply_markup = Some(teloxide::types::InlineKeyboardMarkup::new(vec![vec![
        teloxide::types::InlineKeyboardButton::url("🔽 Открыть в боте", deep_link.parse().expect("valid t.me URL")),
    ]]));
    InlineQueryResult::Article(article)
}

fn build_no_match_article(query_text: &str) -> InlineQueryResult {
    let preview: String = query_text.chars().take(40).collect();
    let mut article = InlineQueryResultArticle::new(
        "nomatch",
        "🔍 Ничего не нашёл",
        InputMessageContent::Text(InputMessageContentText::new(format!(
            "🔍 По запросу «{}» ничего не нашёл в твоих скачиваниях.\nОткрой Doradura, чтобы скачать.",
            preview
        ))),
    );
    article.description = Some(format!("Запрос: {}", preview));
    InlineQueryResult::Article(article)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Caption / description formatting ─────────────────────────────────

    #[test]
    fn caption_audio_full_fields() {
        let s = caption_audio("Дорадура", Some("Дора"), Some(222), Some("320k"));
        assert!(s.contains("Дора — Дорадура"));
        assert!(s.contains("320k"));
        assert!(s.contains("3:42"));
        assert!(s.starts_with("🎵"));
    }

    #[test]
    fn caption_audio_title_only() {
        let s = caption_audio("Unknown Track", None, None, None);
        assert_eq!(s, "🎵 Unknown Track");
    }

    #[test]
    fn caption_audio_skips_empty_author_and_bitrate() {
        let s = caption_audio("Track", Some("   "), Some(60), Some("  "));
        assert!(s.contains("🎵 Track"));
        assert!(!s.contains("   "));
    }

    #[test]
    fn desc_video_full_fields() {
        let s = desc_video(Some("1080p"), Some(24 * 1024 * 1024), Some(222));
        assert!(s.contains("1080p"));
        assert!(s.contains("MB"));
        assert!(s.contains("3:42"));
    }

    #[test]
    fn desc_video_partial() {
        let s = desc_video(None, Some(0), Some(45));
        // 0-size and None-quality skipped → just duration.
        assert_eq!(s, "0:45");
    }

    #[test]
    fn desc_video_empty_falls_back() {
        let s = desc_video(None, None, None);
        assert_eq!(s, "Doradura");
    }

    #[test]
    fn duration_short_routes_to_shared_helper() {
        assert_eq!(duration_short(45), "0:45");
        assert_eq!(duration_short(3_600 + 23 * 60 + 45), "1:23:45");
    }

    // ── Format → result-variant routing ──────────────────────────────────

    fn dummy(format: &'static str) -> InlineEntry<'static> {
        InlineEntry {
            format,
            file_id: "FILEABCDE123",
            title: "Track",
            author: Some("Artist"),
            duration: Some(180),
            file_size: Some(2_000_000),
            video_quality: Some("720p"),
            audio_bitrate: Some("256k"),
        }
    }

    #[test]
    fn mp3_routes_to_cached_audio() {
        let r = entry_to_inline_result(&dummy("mp3"), 0).unwrap();
        assert!(matches!(r, InlineQueryResult::CachedAudio(_)));
    }

    #[test]
    fn m4r_routes_to_cached_audio() {
        let r = entry_to_inline_result(&dummy("m4r"), 0).unwrap();
        assert!(matches!(r, InlineQueryResult::CachedAudio(_)));
    }

    #[test]
    fn mp4_routes_to_cached_video() {
        let r = entry_to_inline_result(&dummy("mp4"), 0).unwrap();
        assert!(matches!(r, InlineQueryResult::CachedVideo(_)));
    }

    #[test]
    fn cut_routes_to_cached_video() {
        let r = entry_to_inline_result(&dummy("cut"), 0).unwrap();
        assert!(matches!(r, InlineQueryResult::CachedVideo(_)));
    }

    #[test]
    fn video_note_routes_to_cached_video_with_circle_icon() {
        let r = entry_to_inline_result(&dummy("video_note"), 0).unwrap();
        let v = match r {
            InlineQueryResult::CachedVideo(v) => v,
            _ => panic!("expected CachedVideo"),
        };
        assert!(v.title.starts_with("⭕"));
    }

    #[test]
    fn gif_routes_to_cached_gif() {
        let r = entry_to_inline_result(&dummy("gif"), 0).unwrap();
        assert!(matches!(r, InlineQueryResult::CachedGif(_)));
    }

    #[test]
    fn unknown_format_is_dropped() {
        assert!(entry_to_inline_result(&dummy("srt"), 0).is_none());
        assert!(entry_to_inline_result(&dummy("txt"), 0).is_none());
    }

    #[test]
    fn empty_file_id_is_dropped() {
        let mut entry = dummy("mp3");
        entry.file_id = "";
        assert!(entry_to_inline_result(&entry, 0).is_none());
    }

    // ── Unique-id generation ─────────────────────────────────────────────

    #[test]
    fn make_id_under_64_bytes() {
        let id = make_id("mp3", "AgADAQADtKgxG_zDuVTSXl2vJaXMx_yFw", 7);
        assert!(id.len() <= 64);
        assert!(id.starts_with("mp3_"));
        assert!(id.ends_with("_7"));
    }

    #[test]
    fn make_id_is_unique_across_seeds() {
        let a = make_id("mp4", "FILE", 0);
        let b = make_id("mp4", "FILE", 1);
        assert_ne!(a, b);
    }

    // ── URL canonicalization is applied in URL mode ──────────────────────

    #[test]
    fn url_canonicalization_strips_tracking_params() {
        // Sanity check that the canonicalizer we delegate to actually rewrites
        // youtu.be tracking variants — guards against regressions where the
        // dep gets reshuffled.
        let raw = "https://youtu.be/jNQXAC9IVRw?si=abc";
        let canonical = canonicalize_url(raw);
        assert!(!canonical.contains("si=abc"));
        assert!(canonical.contains("jNQXAC9IVRw"));
    }
}
