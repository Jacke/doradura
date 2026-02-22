//! Lyrics callback handler.
//!
//! Callback data protocol:
//!   `lyr:{audio_session_id}`          — first tap: fetch & store lyrics, show section picker
//!   `lyr:s:{lyrics_session_id}:{idx}` — show the section at index `idx` (or "all")

use crate::lyrics::{self, LyricsSection};
use crate::storage::db::{self, DbPool};
use crate::telegram::Bot;
use std::collections::HashMap;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{CallbackQueryId, InlineKeyboardMarkup, ParseMode};
use teloxide::RequestError;
use uuid::Uuid;

const MAX_MSG_LEN: usize = 4000;

pub(crate) async fn handle_lyrics_callback(bot: Bot, q: CallbackQuery, db_pool: Arc<DbPool>) -> ResponseResult<()> {
    let data = q.data.clone().unwrap_or_default();
    let chat_id = match q.message.as_ref().map(|m| m.chat().id) {
        Some(id) => id,
        None => {
            bot.answer_callback_query(q.id).await?;
            return Ok(());
        }
    };
    let user_id = q.from.id.0 as i64;

    // ── Section display ───────────────────────────────────────────────────────
    if let Some(rest) = data.strip_prefix("lyr:s:") {
        bot.answer_callback_query(q.id).await?;
        return handle_show_section(&bot, chat_id, rest, &db_pool).await;
    }

    // ── Initial fetch ─────────────────────────────────────────────────────────
    if let Some(audio_session_id) = data.strip_prefix("lyr:") {
        bot.answer_callback_query(q.id).text("🎵 Fetching lyrics…").await?;
        return handle_fetch_lyrics(&bot, chat_id, user_id, audio_session_id, &db_pool).await;
    }

    bot.answer_callback_query(q.id).await?;
    Ok(())
}

// ── First tap: fetch and display ─────────────────────────────────────────────

async fn handle_fetch_lyrics(
    bot: &Bot,
    chat_id: ChatId,
    user_id: i64,
    audio_session_id: &str,
    db_pool: &Arc<DbPool>,
) -> ResponseResult<()> {
    let conn = db::get_connection(db_pool).map_err(db_err)?;

    // Get artist + title from the audio effects session (display_title = "Artist - Song")
    let ae_session = db::get_audio_effect_session(&conn, audio_session_id).map_err(db_err)?;
    let (artist, song_title) = match ae_session {
        Some(ref s) => {
            let (a, t) = lyrics::parse_artist_title(&s.title);
            (a.to_string(), t.to_string())
        }
        None => {
            bot.send_message(chat_id, "❌ Session expired. Download the track again.")
                .await?;
            return Ok(());
        }
    };

    match lyrics::fetch_lyrics(&artist, &song_title).await {
        None => {
            let display = format!("{} – {}", artist, song_title);
            bot.send_message(chat_id, format!("❌ Lyrics not found for *{}*", escape_md(&display)))
                .parse_mode(ParseMode::MarkdownV2)
                .await?;
        }
        Some(lyr) => {
            if !lyr.has_structure || lyr.sections.len() <= 1 {
                // No structure detected — send the full text directly
                let text = lyr.all_text();
                let header = format!("🎵 {} – {}\n\n", artist, song_title);
                send_chunked(bot, chat_id, &format!("{}{}", header, text)).await?;
            } else {
                // Save session and show section picker keyboard
                let session_id = Uuid::new_v4().to_string();
                let sections_json = serde_json::to_string(&lyr.sections).map_err(json_err)?;
                db::create_lyrics_session(
                    &conn,
                    &session_id,
                    user_id,
                    &artist,
                    &song_title,
                    &sections_json,
                    lyr.has_structure,
                )
                .map_err(db_err)?;
                send_section_picker(bot, chat_id, &artist, &song_title, &session_id, &lyr.sections).await?;
            }
        }
    }

    Ok(())
}

// ── Section picker ────────────────────────────────────────────────────────────

async fn send_section_picker(
    bot: &Bot,
    chat_id: ChatId,
    artist: &str,
    title: &str,
    session_id: &str,
    sections: &[LyricsSection],
) -> ResponseResult<()> {
    let display = escape_md(&format!("{} – {}", artist, title));
    let header = format!("🎵 *{}*\nChoose a section:", display);
    let keyboard = build_section_keyboard(session_id, sections);
    bot.send_message(chat_id, header)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(keyboard)
        .await?;
    Ok(())
}

fn build_section_keyboard(session_id: &str, sections: &[LyricsSection]) -> InlineKeyboardMarkup {
    // Label duplicates: "Chorus", "Chorus (2)", etc.
    let mut total: HashMap<String, usize> = HashMap::new();
    for s in sections {
        *total.entry(s.name.clone()).or_insert(0) += 1;
    }
    let mut seen: HashMap<String, usize> = HashMap::new();

    let buttons: Vec<teloxide::types::InlineKeyboardButton> = sections
        .iter()
        .enumerate()
        .map(|(idx, s)| {
            let occ = seen.entry(s.name.clone()).or_insert(0);
            *occ += 1;
            let label = if total.get(&s.name).copied().unwrap_or(1) > 1 {
                format!("{} ({})", s.name, occ)
            } else {
                s.name.clone()
            };
            crate::telegram::cb(label, format!("lyr:s:{}:{}", session_id, idx))
        })
        .collect();

    // Rows of 3 section buttons + a final "All Lyrics" row
    let mut rows: Vec<Vec<teloxide::types::InlineKeyboardButton>> = buttons.chunks(3).map(|c| c.to_vec()).collect();

    rows.push(vec![crate::telegram::cb(
        "📄 All Lyrics",
        format!("lyr:s:{}:all", session_id),
    )]);

    InlineKeyboardMarkup::new(rows)
}

// ── Show a selected section ───────────────────────────────────────────────────

async fn handle_show_section(bot: &Bot, chat_id: ChatId, rest: &str, db_pool: &Arc<DbPool>) -> ResponseResult<()> {
    // rest = "{lyrics_session_id}:{idx_or_all}"
    let (session_id, idx_str) = match rest.rsplit_once(':') {
        Some(pair) => pair,
        None => return Ok(()),
    };

    let conn = db::get_connection(db_pool).map_err(db_err)?;
    let row = db::get_lyrics_session(&conn, session_id).map_err(db_err)?;

    let (artist, title, sections_json, has_structure) = match row {
        Some(r) => r,
        None => {
            bot.send_message(chat_id, "❌ Session expired. Tap 🎵 Lyrics again.")
                .await?;
            return Ok(());
        }
    };

    let sections: Vec<LyricsSection> = serde_json::from_str(&sections_json).unwrap_or_default();
    if sections.is_empty() {
        return Ok(());
    }

    if idx_str == "all" {
        let lyr = lyrics::LyricsResult {
            sections,
            has_structure,
        };
        let header = format!("🎵 {} – {}\n\n", artist, title);
        send_chunked(bot, chat_id, &format!("{}{}", header, lyr.all_text())).await?;
    } else if let Ok(idx) = idx_str.parse::<usize>() {
        match sections.get(idx) {
            Some(sec) => {
                let text = format!("🎵 {} – {}\n[{}]\n\n{}", artist, title, sec.name, sec.text());
                send_chunked(bot, chat_id, &text).await?;
            }
            None => {
                bot.send_message(chat_id, "❌ Section not found.").await?;
            }
        }
    }

    Ok(())
}

// ── Utilities ─────────────────────────────────────────────────────────────────

/// Split long text into ≤MAX_MSG_LEN chunks, breaking on newlines.
async fn send_chunked(bot: &Bot, chat_id: ChatId, text: &str) -> ResponseResult<()> {
    if text.len() <= MAX_MSG_LEN {
        bot.send_message(chat_id, text).await?;
        return Ok(());
    }
    let mut chunk = String::new();
    for line in text.lines() {
        if chunk.len() + line.len() + 1 > MAX_MSG_LEN {
            bot.send_message(chat_id, &chunk).await?;
            chunk.clear();
        }
        if !chunk.is_empty() {
            chunk.push('\n');
        }
        chunk.push_str(line);
    }
    if !chunk.is_empty() {
        bot.send_message(chat_id, &chunk).await?;
    }
    Ok(())
}

fn escape_md(s: &str) -> String {
    crate::core::escape_markdown(s)
}

fn db_err(e: impl std::fmt::Display) -> RequestError {
    RequestError::from(Arc::new(std::io::Error::other(e.to_string())))
}

fn json_err(e: impl std::fmt::Display) -> RequestError {
    RequestError::from(Arc::new(std::io::Error::other(e.to_string())))
}

// Suppress unused import warning for CallbackQueryId (used in the function signature above)
const _: fn(CallbackQueryId) = |_| {};
