//! Player mode: Now Playing widget with playback controls.
//!
//! Callback data prefixes:
//! - `pw:next` — next track
//! - `pw:prev` — previous track
//! - `pw:shuf` — toggle shuffle
//! - `pw:list` — show playlist tracks
//! - `pw:stop` — stop player
//! - `pw:srch` — search in player context
//! - `pw:play:{pl_id}` — start playing a playlist

use crate::download::search::format_duration;
use crate::storage::db::{self, DbPool, PlaylistItem};
use crate::telegram::Bot;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{CallbackQueryId, ChatId, InlineKeyboardButton, InlineKeyboardMarkup, InputFile, MessageId};

// ── Player command ────────────────────────────────────────────────────────

/// Handle /player command: show playlist selector or start playing.
pub async fn handle_player_command(bot: &Bot, chat_id: ChatId, db_pool: &Arc<DbPool>) {
    let conn = match db::get_connection(db_pool) {
        Ok(c) => c,
        Err(_) => {
            let _ = bot.send_message(chat_id, "Database error").await;
            return;
        }
    };

    // Check for existing session
    if let Ok(Some(session)) = db::get_player_session(&conn, chat_id.0) {
        // Resume existing session
        if let Ok(Some(item)) = db::get_playlist_item_at_position(&conn, session.playlist_id, session.current_position)
        {
            let total = db::count_playlist_items(&conn, session.playlist_id).unwrap_or(0);
            let pl_name = db::get_playlist(&conn, session.playlist_id)
                .ok()
                .flatten()
                .map(|p| p.name)
                .unwrap_or_default();
            let _ = send_now_playing(
                bot,
                chat_id,
                &item,
                &pl_name,
                session.current_position,
                total as i32,
                session.is_shuffle,
                None,
            )
            .await;
            return;
        }
    }

    let playlists = db::get_user_playlists(&conn, chat_id.0).unwrap_or_default();

    if playlists.is_empty() {
        let rows = vec![vec![InlineKeyboardButton::callback(
            "+ Create Playlist",
            "pl:new".to_string(),
        )]];
        let keyboard = InlineKeyboardMarkup::new(rows);
        let _ = bot
            .send_message(chat_id, "No playlists yet. Create one first!")
            .reply_markup(keyboard)
            .await;
        return;
    }

    // Show playlist selector
    let mut rows: Vec<Vec<InlineKeyboardButton>> = Vec::new();
    for pl in &playlists {
        let count = db::count_playlist_items(&conn, pl.id).unwrap_or(0);
        let label = format!("▶ {} ({} tracks)", pl.name, count);
        rows.push(vec![InlineKeyboardButton::callback(
            label,
            format!("pw:play:{}", pl.id),
        )]);
    }

    let keyboard = InlineKeyboardMarkup::new(rows);
    let _ = bot
        .send_message(chat_id, "🎵 Select a playlist to play:")
        .reply_markup(keyboard)
        .await;
}

/// Stop the player and delete session.
pub async fn stop_player(bot: &Bot, chat_id: ChatId, db_pool: &Arc<DbPool>) {
    if let Ok(conn) = db::get_connection(db_pool) {
        if let Ok(Some(session)) = db::get_player_session(&conn, chat_id.0) {
            // Try to delete the Now Playing message
            if let Some(msg_id) = session.player_message_id {
                let _ = bot.delete_message(chat_id, MessageId(msg_id)).await;
            }
        }
        let _ = db::delete_player_session(&conn, chat_id.0);
    }
    let _ = bot.send_message(chat_id, "⏹ Player stopped.").await;
}

// ── Now Playing widget ────────────────────────────────────────────────────

async fn send_now_playing(
    bot: &Bot,
    chat_id: ChatId,
    item: &PlaylistItem,
    playlist_name: &str,
    position: i32,
    total: i32,
    is_shuffle: bool,
    old_message_id: Option<MessageId>,
) -> Result<Option<MessageId>, teloxide::RequestError> {
    let artist = item.artist.as_deref().unwrap_or("Unknown");
    let dur = format_duration(item.duration_secs.map(|d| d as u32));

    let text = format!(
        "🎵 Now Playing\n━━━━━━━━━━━━━━\n{} - {}\n{}  ·  {} ({}/{})",
        artist,
        item.title,
        dur,
        playlist_name,
        position + 1,
        total,
    );

    let shuffle_label = if is_shuffle { "🔀 On" } else { "🔀 Off" };

    let rows = vec![
        vec![
            InlineKeyboardButton::callback("⏮", "pw:prev".to_string()),
            InlineKeyboardButton::callback("⏭", "pw:next".to_string()),
            InlineKeyboardButton::callback(shuffle_label, "pw:shuf".to_string()),
        ],
        vec![
            InlineKeyboardButton::callback("📋 Playlist", "pw:list".to_string()),
            InlineKeyboardButton::callback("🔍 Search", "pw:srch".to_string()),
            InlineKeyboardButton::callback("⏹ Stop", "pw:stop".to_string()),
        ],
    ];
    let keyboard = InlineKeyboardMarkup::new(rows);

    // Edit existing message or send new one
    if let Some(msg_id) = old_message_id {
        let _ = bot
            .edit_message_text(chat_id, msg_id, &text)
            .reply_markup(keyboard)
            .await;
        Ok(Some(msg_id))
    } else {
        let msg = bot.send_message(chat_id, &text).reply_markup(keyboard).await?;
        Ok(Some(msg.id))
    }
}

// ── Play a track ──────────────────────────────────────────────────────────

async fn play_track(
    bot: &Bot,
    chat_id: ChatId,
    item: &PlaylistItem,
    playlist_name: &str,
    position: i32,
    total: i32,
    is_shuffle: bool,
    db_pool: &Arc<DbPool>,
    download_queue: &Arc<crate::download::queue::DownloadQueue>,
    old_now_playing_id: Option<MessageId>,
) -> Option<MessageId> {
    if let Some(ref file_id) = item.file_id {
        // Cached — send via file_id
        let _ = bot
            .send_audio(chat_id, InputFile::file_id(teloxide::types::FileId(file_id.clone())))
            .await;
    } else {
        // Not cached — add to download queue
        let task = crate::download::queue::DownloadTask::new(
            item.url.clone(),
            chat_id,
            None,
            false,
            "mp3".to_string(),
            None,
            None,
        );
        download_queue.add_task(task, Some(db_pool.clone())).await;
    }

    // Update Now Playing
    match send_now_playing(
        bot,
        chat_id,
        item,
        playlist_name,
        position,
        total,
        is_shuffle,
        old_now_playing_id,
    )
    .await
    {
        Ok(msg_id) => {
            // Save player message id
            if let Ok(conn) = db::get_connection(db_pool) {
                let _ = db::update_player_position(&conn, chat_id.0, position, msg_id.map(|m| m.0));
            }
            msg_id
        }
        Err(_) => None,
    }
}

// ── Callback handler ──────────────────────────────────────────────────────

pub async fn handle_player_callback(
    bot: &Bot,
    callback_id: CallbackQueryId,
    chat_id: ChatId,
    message_id: MessageId,
    data: &str,
    db_pool: Arc<DbPool>,
    download_queue: Arc<crate::download::queue::DownloadQueue>,
) -> Result<(), teloxide::RequestError> {
    let _ = bot.answer_callback_query(callback_id).await;

    let conn = match db::get_connection(&db_pool) {
        Ok(c) => c,
        Err(_) => return Ok(()),
    };

    // pw:play:{pl_id} — start playing
    if let Some(pl_id_str) = data.strip_prefix("pw:play:") {
        if let Ok(pl_id) = pl_id_str.parse::<i64>() {
            // Verify ownership or public access
            match db::get_playlist(&conn, pl_id) {
                Ok(Some(pl)) if pl.user_id == chat_id.0 || pl.is_public => {}
                _ => return Ok(()),
            }
            let items_count = db::count_playlist_items(&conn, pl_id).unwrap_or(0);
            if items_count == 0 {
                let _ = bot.send_message(chat_id, "Playlist is empty.").await;
                return Ok(());
            }

            // Create player session
            let _ = db::create_player_session(&conn, chat_id.0, pl_id, None);

            // Play first track
            if let Ok(Some(item)) = db::get_playlist_item_at_position(&conn, pl_id, 0) {
                let pl_name = db::get_playlist(&conn, pl_id)
                    .ok()
                    .flatten()
                    .map(|p| p.name)
                    .unwrap_or_default();
                let _ = bot.delete_message(chat_id, message_id).await;
                play_track(
                    bot,
                    chat_id,
                    &item,
                    &pl_name,
                    0,
                    items_count as i32,
                    false,
                    &db_pool,
                    &download_queue,
                    None,
                )
                .await;
            }
        }
        return Ok(());
    }

    // Get current session
    let session = match db::get_player_session(&conn, chat_id.0) {
        Ok(Some(s)) => s,
        _ => {
            let _ = bot.send_message(chat_id, "No active player session.").await;
            return Ok(());
        }
    };

    let total = db::count_playlist_items(&conn, session.playlist_id).unwrap_or(0) as i32;
    let pl_name = db::get_playlist(&conn, session.playlist_id)
        .ok()
        .flatten()
        .map(|p| p.name)
        .unwrap_or_default();

    match data {
        "pw:next" => {
            let next_pos = if session.is_shuffle {
                use rand::Rng;
                rand::thread_rng().gen_range(0..total)
            } else if session.current_position + 1 >= total {
                0 // wrap around
            } else {
                session.current_position + 1
            };
            if let Ok(Some(item)) = db::get_playlist_item_at_position(&conn, session.playlist_id, next_pos) {
                play_track(
                    bot,
                    chat_id,
                    &item,
                    &pl_name,
                    next_pos,
                    total,
                    session.is_shuffle,
                    &db_pool,
                    &download_queue,
                    Some(message_id),
                )
                .await;
            }
        }
        "pw:prev" => {
            let prev_pos = if session.current_position <= 0 {
                (total - 1).max(0)
            } else {
                session.current_position - 1
            };
            if let Ok(Some(item)) = db::get_playlist_item_at_position(&conn, session.playlist_id, prev_pos) {
                play_track(
                    bot,
                    chat_id,
                    &item,
                    &pl_name,
                    prev_pos,
                    total,
                    session.is_shuffle,
                    &db_pool,
                    &download_queue,
                    Some(message_id),
                )
                .await;
            }
        }
        "pw:shuf" => {
            if let Ok(new_shuffle) = db::toggle_player_shuffle(&conn, chat_id.0) {
                if let Ok(Some(item)) =
                    db::get_playlist_item_at_position(&conn, session.playlist_id, session.current_position)
                {
                    let _ = send_now_playing(
                        bot,
                        chat_id,
                        &item,
                        &pl_name,
                        session.current_position,
                        total,
                        new_shuffle,
                        Some(message_id),
                    )
                    .await;
                }
            }
        }
        "pw:list" => {
            // Show playlist items
            let items = db::get_playlist_items_page(&conn, session.playlist_id, 0, 10).unwrap_or_default();
            let mut text = format!("📋 {} ({} tracks)\n\n", pl_name, total);
            for item in &items {
                let marker = if item.position == session.current_position {
                    "▶ "
                } else {
                    "  "
                };
                let dur = format_duration(item.duration_secs.map(|d| d as u32));
                let artist = item.artist.as_deref().unwrap_or("");
                if artist.is_empty() {
                    text.push_str(&format!("{}{}.  {} ({})\n", marker, item.position + 1, item.title, dur));
                } else {
                    text.push_str(&format!(
                        "{}{}.  {} - {} ({})\n",
                        marker,
                        item.position + 1,
                        artist,
                        item.title,
                        dur
                    ));
                }
            }
            let _ = bot.send_message(chat_id, text).await;
        }
        "pw:srch" => {
            let _ = bot.send_message(chat_id, "🔍 Type your search query:").await;
            // Player mode intercept in commands.rs will handle the text
        }
        "pw:stop" => {
            stop_player(bot, chat_id, &db_pool).await;
        }
        _ => {}
    }

    Ok(())
}
