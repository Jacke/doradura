//! Lyrics callback handler.
//!
//! Callback data protocol:
//!   `lyr:{audio_session_id}`          — first tap: fetch & store lyrics, show section picker
//!   `lyr:s:{lyrics_session_id}:{idx}` — show the section at index `idx` (or "all")

use crate::lyrics::{self, LyricsSection};
use crate::storage::SharedStorage;
use crate::telegram::Bot;
use crate::telegram::BotExt;
use doracore::download::url_canonical::canonicalize_url;
use std::collections::HashMap;
use std::sync::Arc;
use teloxide::RequestError;
use teloxide::prelude::*;
use teloxide::types::InlineKeyboardMarkup;
use uuid::Uuid;

const MAX_MSG_LEN: usize = 4000;

/// Prompt-session kind for "user is pasting a corrected lyrics URL". Payload is
/// the audio-effect session id whose lyrics are being corrected.
const LYRICS_FIX_PROMPT_KIND: &str = "lyrics_fix";
/// How long the "paste a lyrics link" prompt stays armed.
const LYRICS_FIX_TTL_SECS: i64 = 600;

pub(crate) async fn handle_lyrics_callback(
    bot: Bot,
    q: CallbackQuery,
    shared_storage: Arc<SharedStorage>,
) -> ResponseResult<()> {
    let data = q.data.clone().unwrap_or_default();
    let chat_id = match q.message.as_ref().map(|m| m.chat().id) {
        Some(id) => id,
        None => {
            bot.answer_callback_query(q.id).await?;
            return Ok(());
        }
    };
    let user_id = q.from.id.0 as i64;

    // ── Correction: arm "paste a lyrics URL" prompt ───────────────────────────
    if let Some(audio_session_id) = data.strip_prefix("lyr:fix:") {
        bot.answer_callback_query(q.id).await?;
        return arm_lyrics_fix(&bot, chat_id, user_id, audio_session_id, &shared_storage).await;
    }

    // ── Section display ───────────────────────────────────────────────────────
    if let Some(rest) = data.strip_prefix("lyr:s:") {
        bot.answer_callback_query(q.id).await?;
        return handle_show_section(&bot, chat_id, rest, &shared_storage).await;
    }

    // ── Initial fetch ─────────────────────────────────────────────────────────
    if let Some(audio_session_id) = data.strip_prefix("lyr:") {
        bot.answer_callback_query(q.id).text("🎵 Fetching lyrics…").await?;
        return handle_fetch_lyrics(&bot, chat_id, user_id, audio_session_id, &shared_storage).await;
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
    shared_storage: &Arc<SharedStorage>,
) -> ResponseResult<()> {
    // Get artist + title from the audio effects session (display_title = "Artist - Song")
    let ae_session = shared_storage
        .get_audio_effect_session(audio_session_id)
        .await
        .map_err(db_err)?;
    let (artist, song_title, source_url) = match ae_session {
        Some(ref s) => {
            let (a, t) = lyrics::parse_artist_title(&s.title);
            (a.to_string(), t.to_string(), s.source_url.clone())
        }
        None => {
            bot.send_message(chat_id, "❌ Session expired. Download the track again.")
                .await?;
            return Ok(());
        }
    };

    // 1) A canonical correction for this source video wins over the auto-match.
    if let Some(src) = source_url.as_deref() {
        let key = canonicalize_url(src);
        if let Ok(Some(ovr)) = shared_storage.get_lyrics_override(&key).await {
            log::info!("lyrics: using override for {}", key);
            let (sections, has_structure) = lyrics::parse_sections(&ovr.lyrics_text);
            let lyr = lyrics::LyricsResult {
                artist: ovr.artist.unwrap_or(artist),
                artist_id: None,
                title: ovr.title.unwrap_or(song_title),
                album: None,
                release_date: None,
                thumbnail_url: None,
                sections,
                has_structure,
            };
            return present_lyrics(bot, chat_id, user_id, audio_session_id, lyr, shared_storage).await;
        }
    }

    // 2) Auto-match cascade.
    match lyrics::fetch_lyrics(&artist, &song_title, None).await {
        None => {
            let display = format!("{} – {}", artist, song_title);
            let kb = fix_keyboard(audio_session_id);
            bot.send_md_kb(
                chat_id,
                format!(
                    "❌ Lyrics not found for *{}*\n\nWrong song? Tap below and send the correct Genius/LRCLIB link\\.",
                    escape_md(&display)
                ),
                kb,
            )
            .await?;
        }
        Some(lyr) => {
            present_lyrics(bot, chat_id, user_id, audio_session_id, lyr, shared_storage).await?;
        }
    }

    Ok(())
}

/// Render fetched lyrics: full text (unstructured) or a section picker
/// (structured). Always offers a "wrong song / fix" affordance keyed by the
/// audio session so the user can supply a corrected lyrics link.
async fn present_lyrics(
    bot: &Bot,
    chat_id: ChatId,
    user_id: i64,
    audio_session_id: &str,
    lyr: lyrics::LyricsResult,
    shared_storage: &Arc<SharedStorage>,
) -> ResponseResult<()> {
    let artist = lyr.artist.clone();
    let song_title = lyr.title.clone();

    if !lyr.has_structure || lyr.sections.len() <= 1 {
        let text = lyr.all_text();
        let header = format!("🎵 {} – {}\n\n", artist, song_title);
        send_chunked(bot, chat_id, &format!("{}{}", header, text)).await?;
        bot.send_md_kb(
            chat_id,
            "Wrong lyrics? Send the correct Genius/LRCLIB link\\.".to_string(),
            fix_keyboard(audio_session_id),
        )
        .await?;
        return Ok(());
    }

    let session_id = Uuid::new_v4().to_string();
    let sections_json = serde_json::to_string(&lyr.sections).map_err(json_err)?;
    match shared_storage
        .create_lyrics_session(
            &session_id,
            user_id,
            &artist,
            &song_title,
            &sections_json,
            lyr.has_structure,
        )
        .await
    {
        Ok(_) => {
            send_section_picker(
                bot,
                chat_id,
                &artist,
                &song_title,
                &session_id,
                audio_session_id,
                &lyr.sections,
            )
            .await
        }
        Err(e) => {
            log::error!("Failed to persist lyrics session, falling back to full text: {}", e);
            let text = lyr.all_text();
            let header = format!("🎵 {} – {}\n\n", artist, song_title);
            send_chunked(bot, chat_id, &format!("{}{}", header, text)).await
        }
    }
}

/// One-button keyboard: "wrong song / fix lyrics" → arms the correction prompt.
fn fix_keyboard(audio_session_id: &str) -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new(vec![vec![crate::telegram::cb(
        "❌ Wrong song / fix lyrics",
        format!("lyr:fix:{}", audio_session_id),
    )]])
}

// ── Section picker ────────────────────────────────────────────────────────────

async fn send_section_picker(
    bot: &Bot,
    chat_id: ChatId,
    artist: &str,
    title: &str,
    session_id: &str,
    audio_session_id: &str,
    sections: &[LyricsSection],
) -> ResponseResult<()> {
    let display = escape_md(&format!("{} – {}", artist, title));
    let header = format!("🎵 *{}*\nChoose a section:", display);
    let keyboard = build_section_keyboard(session_id, audio_session_id, sections);
    bot.send_md_kb(chat_id, header, keyboard).await?;
    Ok(())
}

fn build_section_keyboard(
    session_id: &str,
    audio_session_id: &str,
    sections: &[LyricsSection],
) -> InlineKeyboardMarkup {
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
    rows.push(vec![crate::telegram::cb(
        "❌ Wrong song / fix lyrics",
        format!("lyr:fix:{}", audio_session_id),
    )]);

    InlineKeyboardMarkup::new(rows)
}

// ── Show a selected section ───────────────────────────────────────────────────

async fn handle_show_section(
    bot: &Bot,
    chat_id: ChatId,
    rest: &str,
    shared_storage: &Arc<SharedStorage>,
) -> ResponseResult<()> {
    // rest = "{lyrics_session_id}:{idx_or_all}"
    let (session_id, idx_str) = match rest.rsplit_once(':') {
        Some(pair) => pair,
        None => return Ok(()),
    };

    let row = shared_storage.get_lyrics_session(session_id).await.map_err(db_err)?;

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
            artist: artist.clone(),
            artist_id: None,
            title: title.clone(),
            album: None,
            release_date: None,
            thumbnail_url: None,
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

// ── Correction flow ───────────────────────────────────────────────────────────

/// Arm the "paste a corrected lyrics URL" prompt for this audio session.
async fn arm_lyrics_fix(
    bot: &Bot,
    chat_id: ChatId,
    user_id: i64,
    audio_session_id: &str,
    shared_storage: &Arc<SharedStorage>,
) -> ResponseResult<()> {
    if let Err(e) = shared_storage
        .upsert_prompt_session(user_id, LYRICS_FIX_PROMPT_KIND, audio_session_id, LYRICS_FIX_TTL_SECS)
        .await
    {
        log::error!("arm_lyrics_fix: failed to set prompt session: {}", e);
    }
    bot.send_message(
        chat_id,
        "🔗 Send the correct lyrics link (a genius.com or lrclib.net URL). Send \"cancel\" to abort.",
    )
    .await?;
    Ok(())
}

/// Whether `user_id` has an armed lyrics-fix prompt (drives the text-input
/// interceptor in `commands`).
pub async fn pending_lyrics_fix(shared_storage: &Arc<SharedStorage>, user_id: i64) -> Option<String> {
    shared_storage
        .get_prompt_session(user_id, LYRICS_FIX_PROMPT_KIND)
        .await
        .ok()
        .flatten()
}

/// Consume a pasted lyrics URL: fetch from the provider, persist a canonical
/// override keyed by the source video, and show the corrected lyrics.
pub async fn handle_lyrics_fix_input(
    bot: &Bot,
    chat_id: ChatId,
    user_id: i64,
    text: &str,
    shared_storage: &Arc<SharedStorage>,
) -> ResponseResult<()> {
    let Some(audio_session_id) = pending_lyrics_fix(shared_storage, user_id).await else {
        return Ok(());
    };
    let _ = shared_storage
        .delete_prompt_session(user_id, LYRICS_FIX_PROMPT_KIND)
        .await;

    let url = text.trim();
    if url.eq_ignore_ascii_case("cancel") {
        bot.send_message(chat_id, "Cancelled.").await?;
        return Ok(());
    }
    if lyrics::providers::provider_for_url(url).is_none() {
        bot.send_message(chat_id, "❌ Unsupported link. Send a genius.com or lrclib.net URL.")
            .await?;
        return Ok(());
    }

    let status = bot.send_message(chat_id, "🔎 Fetching lyrics from your link…").await?;
    let Some((provider, lyr)) = lyrics::providers::fetch_from_url(url).await else {
        bot.edit_message_text(chat_id, status.id, "❌ Couldn't read lyrics from that link.")
            .await
            .ok();
        return Ok(());
    };
    bot.try_delete(chat_id, status.id).await;

    // Persist a global override keyed by the source video, so future downloads
    // of the same video skip the wrong auto-match.
    if let Ok(Some(sess)) = shared_storage.get_audio_effect_session(&audio_session_id).await
        && let Some(src) = sess.source_url.as_deref()
    {
        let key = canonicalize_url(src);
        let snapshot = lyr.all_text();
        let artist = (!lyr.artist.trim().is_empty()).then_some(lyr.artist.as_str());
        let title = (!lyr.title.trim().is_empty()).then_some(lyr.title.as_str());
        if let Err(e) = shared_storage
            .upsert_lyrics_override(&key, provider.id(), url, artist, title, &snapshot, Some(user_id))
            .await
        {
            log::error!("handle_lyrics_fix_input: save override failed: {}", e);
        } else {
            log::info!("lyrics override saved for {}", key);
        }
    }

    present_lyrics(bot, chat_id, user_id, &audio_session_id, lyr, shared_storage).await
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
