//! Music search UI: results display with download/add-to-playlist buttons.
//!
//! Callback data prefixes:
//! - `sr:p:{src}:{page}` — paginate (src = y|s)
//! - `sr:dl:{idx}` — download result[idx]
//! - `sr:add:{pl_id}:{idx}` — add result[idx] to playlist
//! - `sr:src:{src}` — switch source

use crate::download::search::{format_duration, search, SearchResult, SearchSource};
use crate::i18n;
use crate::storage::db::{self, DbPool};
use crate::telegram::Bot;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use teloxide::prelude::*;
use teloxide::types::{CallbackQueryId, ChatId, InlineKeyboardButton, InlineKeyboardMarkup, MessageId};
use tokio::sync::RwLock;

const RESULTS_PER_PAGE: usize = 5;
const SESSION_TTL_SECS: u64 = 600; // 10 minutes

// ── Session state ─────────────────────────────────────────────────────────

/// Context for where search results will be used.
#[derive(Debug, Clone)]
pub enum SearchContext {
    /// Standalone search (just download).
    Standalone,
    /// Player mode search (add to active playlist).
    PlayerMode { playlist_id: i64 },
    /// Adding to a specific playlist.
    AddToPlaylist { playlist_id: i64 },
}

/// In-memory search session for a user.
#[derive(Debug, Clone)]
pub struct SearchSession {
    pub query: String,
    pub results: Vec<SearchResult>,
    pub source: SearchSource,
    pub context: SearchContext,
    pub created_at: Instant,
}

static SEARCH_SESSIONS: std::sync::LazyLock<Arc<RwLock<HashMap<i64, SearchSession>>>> =
    std::sync::LazyLock::new(|| Arc::new(RwLock::new(HashMap::new())));

/// Store a search session for a user.
pub async fn set_search_session(user_id: i64, session: SearchSession) {
    let mut sessions = SEARCH_SESSIONS.write().await;
    sessions.insert(user_id, session);
}

/// Get a search session for a user (returns None if expired).
pub async fn get_search_session(user_id: i64) -> Option<SearchSession> {
    let sessions = SEARCH_SESSIONS.read().await;
    let session = sessions.get(&user_id)?;
    if session.created_at.elapsed().as_secs() > SESSION_TTL_SECS {
        drop(sessions);
        clear_search_session(user_id).await;
        return None;
    }
    Some(session.clone())
}

/// Remove a search session for a user.
pub async fn clear_search_session(user_id: i64) {
    let mut sessions = SEARCH_SESSIONS.write().await;
    sessions.remove(&user_id);
}

// ── Perform search and show results ───────────────────────────────────────

/// Handle text input as a music search (called from player mode intercept).
pub async fn handle_player_search(bot: &Bot, chat_id: ChatId, text: &str, db_pool: Arc<DbPool>, playlist_id: i64) {
    handle_standalone_search(bot, chat_id, text, db_pool, SearchContext::PlayerMode { playlist_id }).await;
}

/// Handle standalone search (from /search or menu).
pub async fn handle_standalone_search(
    bot: &Bot,
    chat_id: ChatId,
    text: &str,
    db_pool: Arc<DbPool>,
    context: SearchContext,
) {
    let _lang = i18n::user_lang_from_pool(&db_pool, chat_id.0);
    let source = SearchSource::YouTube;

    let status_msg = bot.send_message(chat_id, "Searching...").await;

    match search(source, text, RESULTS_PER_PAGE as u8, Some(&db_pool)).await {
        Ok(results) => {
            if let Ok(msg) = &status_msg {
                let _ = bot.delete_message(chat_id, msg.id).await;
            }
            if results.is_empty() {
                let _ = bot
                    .send_message(chat_id, "🔍 No results found. Try a different query!")
                    .await;
                return;
            }
            let session = SearchSession {
                query: text.to_string(),
                results: results.clone(),
                source,
                context: context.clone(),
                created_at: Instant::now(),
            };
            set_search_session(chat_id.0, session).await;
            let _ = show_search_results(bot, chat_id, &results, text, source, 0, &context).await;
        }
        Err(e) => {
            if let Ok(msg) = &status_msg {
                let _ = bot.delete_message(chat_id, msg.id).await;
            }
            log::error!("Search error: {}", e);
            let _ = bot.send_message(chat_id, format!("Search failed: {}", e)).await;
        }
    }
}

// ── Display search results ────────────────────────────────────────────────

async fn show_search_results(
    bot: &Bot,
    chat_id: ChatId,
    results: &[SearchResult],
    query: &str,
    source: SearchSource,
    page: usize,
    context: &SearchContext,
) -> Result<(), teloxide::RequestError> {
    let start = page * RESULTS_PER_PAGE;
    let page_results = &results[start..results.len().min(start + RESULTS_PER_PAGE)];

    let mut text = format!(
        "🔍 \"{}\" — {} ({}-{} of {})\n\n",
        query,
        source.label(),
        start + 1,
        start + page_results.len(),
        results.len()
    );

    for (i, r) in page_results.iter().enumerate() {
        let idx = start + i + 1;
        let artist = if r.artist.is_empty() { "" } else { &r.artist };
        let dur = format_duration(r.duration_secs);
        if artist.is_empty() {
            text.push_str(&format!("{}. {} ({})\n", idx, r.title, dur));
        } else {
            text.push_str(&format!("{}. {} - {} ({})\n", idx, artist, r.title, dur));
        }
    }

    let mut rows: Vec<Vec<InlineKeyboardButton>> = Vec::new();

    // Download buttons row
    let mut dl_row: Vec<InlineKeyboardButton> = Vec::new();
    for (i, _) in page_results.iter().enumerate() {
        let global_idx = start + i;
        dl_row.push(InlineKeyboardButton::callback(
            format!("{} ⬇", global_idx + 1),
            format!("sr:dl:{}", global_idx),
        ));
    }
    rows.push(dl_row);

    // Add-to-playlist buttons row (if in playlist context)
    match context {
        SearchContext::PlayerMode { playlist_id } | SearchContext::AddToPlaylist { playlist_id } => {
            let mut add_row: Vec<InlineKeyboardButton> = Vec::new();
            for (i, _) in page_results.iter().enumerate() {
                let global_idx = start + i;
                add_row.push(InlineKeyboardButton::callback(
                    format!("{} ➕", global_idx + 1),
                    format!("sr:add:{}:{}", playlist_id, global_idx),
                ));
            }
            rows.push(add_row);
        }
        SearchContext::Standalone => {}
    }

    // Navigation row
    let mut nav_row: Vec<InlineKeyboardButton> = Vec::new();
    if page > 0 {
        nav_row.push(InlineKeyboardButton::callback(
            "◀",
            format!("sr:p:{}:{}", source.code(), page - 1),
        ));
    }
    // Switch source button
    let other_source = match source {
        SearchSource::YouTube => SearchSource::SoundCloud,
        SearchSource::SoundCloud => SearchSource::YouTube,
    };
    nav_row.push(InlineKeyboardButton::callback(
        format!("{} 🔄", other_source.label()),
        format!("sr:src:{}", other_source.code()),
    ));
    if start + RESULTS_PER_PAGE < results.len() {
        nav_row.push(InlineKeyboardButton::callback(
            "▶",
            format!("sr:p:{}:{}", source.code(), page + 1),
        ));
    }
    rows.push(nav_row);

    let keyboard = InlineKeyboardMarkup::new(rows);

    bot.send_message(chat_id, text).reply_markup(keyboard).await?;

    Ok(())
}

// ── Callback handler ──────────────────────────────────────────────────────

pub async fn handle_search_callback(
    bot: &Bot,
    callback_id: CallbackQueryId,
    chat_id: ChatId,
    message_id: MessageId,
    data: &str,
    db_pool: Arc<DbPool>,
    download_queue: Arc<crate::download::queue::DownloadQueue>,
) -> Result<(), teloxide::RequestError> {
    let _ = bot.answer_callback_query(callback_id).await;

    let session = match get_search_session(chat_id.0).await {
        Some(s) => s,
        None => {
            let _ = bot
                .send_message(chat_id, "Search session expired. Please search again.")
                .await;
            return Ok(());
        }
    };

    // sr:dl:{idx} — download
    if let Some(idx_str) = data.strip_prefix("sr:dl:") {
        if let Ok(idx) = idx_str.parse::<usize>() {
            if let Some(result) = session.results.get(idx) {
                let _ = bot.delete_message(chat_id, message_id).await;

                // Add to download queue as mp3
                let task = crate::download::queue::DownloadTask::new(
                    result.url.clone(),
                    chat_id,
                    None,
                    false,
                    "mp3".to_string(),
                    None,
                    None,
                );
                download_queue.add_task(task, Some(db_pool.clone())).await;

                let _ = bot
                    .send_message(chat_id, format!("⬇ Downloading: {}", result.title))
                    .await;
            }
        }
        return Ok(());
    }

    // sr:add:{pl_id}:{idx} — add to playlist
    if let Some(rest) = data.strip_prefix("sr:add:") {
        let parts: Vec<&str> = rest.splitn(2, ':').collect();
        if parts.len() == 2 {
            if let (Ok(pl_id), Ok(idx)) = (parts[0].parse::<i64>(), parts[1].parse::<usize>()) {
                if let Some(result) = session.results.get(idx) {
                    if let Ok(conn) = db::get_connection(&db_pool) {
                        // Verify ownership
                        match db::get_playlist(&conn, pl_id) {
                            Ok(Some(pl)) if pl.user_id == chat_id.0 => {}
                            _ => return Ok(()),
                        }
                        let _ = db::add_playlist_item(
                            &conn,
                            pl_id,
                            &result.title,
                            Some(&result.artist),
                            &result.url,
                            result.duration_secs.map(|d| d as i32),
                            None,
                            session.source.source_name(),
                        );
                        let _ = bot
                            .send_message(chat_id, format!("➕ Added \"{}\" to playlist", result.title))
                            .await;
                    }
                }
            }
        }
        return Ok(());
    }

    // sr:p:{src}:{page} — paginate
    if let Some(rest) = data.strip_prefix("sr:p:") {
        let parts: Vec<&str> = rest.splitn(2, ':').collect();
        if parts.len() == 2 {
            if let Ok(page) = parts[1].parse::<usize>() {
                let _ = bot.delete_message(chat_id, message_id).await;
                let _ = show_search_results(
                    bot,
                    chat_id,
                    &session.results,
                    &session.query,
                    session.source,
                    page,
                    &session.context,
                )
                .await;
            }
        }
        return Ok(());
    }

    // sr:src:{src} — switch source
    if let Some(src_code) = data.strip_prefix("sr:src:") {
        if let Some(new_source) = SearchSource::from_code(src_code) {
            let _ = bot.delete_message(chat_id, message_id).await;

            let status_msg = bot.send_message(chat_id, "Searching...").await;

            match search(new_source, &session.query, RESULTS_PER_PAGE as u8, Some(&db_pool)).await {
                Ok(results) => {
                    if let Ok(msg) = &status_msg {
                        let _ = bot.delete_message(chat_id, msg.id).await;
                    }
                    if results.is_empty() {
                        let _ = bot.send_message(chat_id, "No results found.").await;
                        return Ok(());
                    }
                    let new_session = SearchSession {
                        query: session.query.clone(),
                        results: results.clone(),
                        source: new_source,
                        context: session.context.clone(),
                        created_at: Instant::now(),
                    };
                    set_search_session(chat_id.0, new_session).await;
                    let _ =
                        show_search_results(bot, chat_id, &results, &session.query, new_source, 0, &session.context)
                            .await;
                }
                Err(e) => {
                    if let Ok(msg) = &status_msg {
                        let _ = bot.delete_message(chat_id, msg.id).await;
                    }
                    let _ = bot.send_message(chat_id, format!("Search failed: {}", e)).await;
                }
            }
        }
        return Ok(());
    }

    Ok(())
}
