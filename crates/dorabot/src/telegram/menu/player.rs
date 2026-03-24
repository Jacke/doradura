//! Player mode: real audio playback with track sending and UI cleanup.
//!
//! Callback data prefixes:
//! - `pw:play:{pl_id}` — start playing a playlist (enter player mode)
//! - `pw:play_all` — send all tracks from active playlist
//! - `pw:shuf` — toggle shuffle
//! - `pw:list` — show playlist tracks
//! - `pw:stop` — stop player and cleanup UI
//! - `pw:srch` — search in player context
//! - `pw:add` — add track to playlist (search context)

use crate::download::pipeline::{self, PipelineFormat};
use crate::download::progress::ProgressMessage;
use crate::download::search::format_duration;
use crate::download::send::send_audio_with_retry;
use crate::download::source::bot_global;
use crate::storage::db::{DbPool, PlaylistItem};
use crate::storage::SharedStorage;
use crate::telegram::notifications::notify_admin_text;
use crate::telegram::Bot;
use rand::seq::SliceRandom;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{
    BotCommand, BotCommandScope, CallbackQueryId, ChatId, InlineKeyboardButton, InlineKeyboardMarkup, InputFile,
    MessageId, Recipient,
};
use tokio::time::timeout;
use url::Url;

/// Dora 😌 sticker (headphones vibes) from doraduradoradura pack.
const PLAYER_STICKER_ID: &str = "CAACAgIAAxUAAWj-ZomiM5Mt2aK1G3b8O7JK-shMAALPFQACWGhoSMeITTonc71ENgQ";

/// Per-track download timeout (5 minutes).
const TRACK_DOWNLOAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

// ── Bot commands per-chat ─────────────────────────────────────────────────

/// Set player-mode commands for this chat (replaces global commands).
async fn set_player_commands(bot: &Bot, chat_id: ChatId) {
    let commands = vec![
        BotCommand::new("player", "open player menu"),
        BotCommand::new("playlists", "manage playlists"),
        BotCommand::new("exit", "exit player mode"),
    ];
    let scope = BotCommandScope::Chat {
        chat_id: Recipient::Id(chat_id),
    };
    let _ = bot.set_my_commands(commands).scope(scope).await;
}

/// Remove per-chat commands override (restores global defaults).
async fn restore_default_commands(bot: &Bot, chat_id: ChatId) {
    let scope = BotCommandScope::Chat {
        chat_id: Recipient::Id(chat_id),
    };
    let _ = bot.delete_my_commands().scope(scope).await;
}

// ── Player command ────────────────────────────────────────────────────────

/// Handle /player command: show playlist selector.
pub async fn handle_player_command(
    bot: &Bot,
    chat_id: ChatId,
    _db_pool: &Arc<DbPool>,
    shared_storage: &Arc<SharedStorage>,
) {
    // Check for existing session — show player menu
    if let Ok(Some(session)) = shared_storage.get_player_session(chat_id.0).await {
        if let Ok(Some(pl)) = shared_storage.get_playlist(session.playlist_id).await {
            let items = shared_storage
                .get_playlist_items(session.playlist_id)
                .await
                .unwrap_or_default();
            let msg = send_player_menu(bot, chat_id, &pl.name, &items, session.is_shuffle, None).await;
            if let Some(msg_id) = msg {
                track_message(shared_storage, chat_id.0, msg_id.0).await;
            }
            return;
        }
    }

    let playlists = shared_storage.get_user_playlists(chat_id.0).await.unwrap_or_default();

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
        let count = shared_storage.count_playlist_items(pl.id).await.unwrap_or(0);
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

/// Stop the player: delete all tracked UI messages, unpin, cleanup DB, restore commands.
pub async fn stop_player(bot: &Bot, chat_id: ChatId, db_pool: &Arc<DbPool>, shared_storage: &Arc<SharedStorage>) {
    let _ = db_pool;

    if let Ok(Some(session)) = shared_storage.get_player_session(chat_id.0).await {
        if let Some(sticker_id) = session.sticker_message_id {
            let _ = bot.unpin_chat_message(chat_id).message_id(MessageId(sticker_id)).await;
        }

        if let Ok(msg_ids) = shared_storage.get_player_messages(chat_id.0).await {
            for msg_id in msg_ids {
                let _ = bot.delete_message(chat_id, MessageId(msg_id)).await;
            }
        }

        let _ = shared_storage.delete_player_messages(chat_id.0).await;
        let _ = shared_storage.delete_player_session(chat_id.0).await;
    }

    // Restore default bot commands for this chat
    restore_default_commands(bot, chat_id).await;
}

// ── Enter player mode ─────────────────────────────────────────────────────

async fn enter_player_mode(
    bot: &Bot,
    chat_id: ChatId,
    playlist_id: i64,
    playlist_name: &str,
    items: &[PlaylistItem],
    _db_pool: &Arc<DbPool>,
    shared_storage: &Arc<SharedStorage>,
) {
    // 1. Set player-mode bot commands
    set_player_commands(bot, chat_id).await;

    // 2. Send sticker (Dora 😌)
    let sticker_msg_id = bot
        .send_sticker(
            chat_id,
            InputFile::file_id(teloxide::types::FileId(PLAYER_STICKER_ID.to_string())),
        )
        .await
        .ok()
        .map(|msg| msg.id);

    if let Some(sid) = sticker_msg_id {
        track_message(shared_storage, chat_id.0, sid.0).await;
    }

    // 3. Send banner
    let banner_msg_id = bot
        .send_message(chat_id, "🎧 Music Player by Dora")
        .await
        .ok()
        .map(|msg| msg.id);

    if let Some(bid) = banner_msg_id {
        track_message(shared_storage, chat_id.0, bid.0).await;
    }

    // 4. Send player menu (this is the message we pin — shows playlist info)
    let menu_msg = send_player_menu(bot, chat_id, playlist_name, items, false, None).await;
    if let Some(msg_id) = menu_msg {
        track_message(shared_storage, chat_id.0, msg_id.0).await;
        let _ = bot.pin_chat_message(chat_id, msg_id).disable_notification(true).await;
    }

    // 5. Create player session (sticker_message_id stores the PINNED message for unpin)
    let pinned_msg_id = menu_msg.or(banner_msg_id);
    if let Err(e) = shared_storage
        .create_player_session(chat_id.0, playlist_id, None, pinned_msg_id.map(|m| m.0))
        .await
    {
        log::error!("Failed to create player session for user {}: {}", chat_id.0, e);
        bot.send_message(chat_id, "❌ Failed to start player mode. Please try again.")
            .await
            .ok();
    }
}

// ── Player menu ───────────────────────────────────────────────────────────

fn build_menu_text(playlist_name: &str, items: &[PlaylistItem], is_shuffle: bool) -> String {
    let total = items.len();
    let cached = items.iter().filter(|i| i.file_id.is_some()).count();
    let total_duration: u32 = items.iter().filter_map(|i| i.duration_secs.map(|d| d as u32)).sum();
    let dur_str = format_duration(Some(total_duration));
    let shuffle_icon = if is_shuffle { " · 🔀" } else { "" };

    format!(
        "🎵 {}\n━━━━━━━━━━━━━━━━━━━━\n📀 {} tracks · ⏱ {}{}\n💾 {} cached · {} to download",
        playlist_name,
        total,
        dur_str,
        shuffle_icon,
        cached,
        total - cached,
    )
}

async fn send_player_menu(
    bot: &Bot,
    chat_id: ChatId,
    playlist_name: &str,
    items: &[PlaylistItem],
    is_shuffle: bool,
    old_message_id: Option<MessageId>,
) -> Option<MessageId> {
    let shuffle_label = if is_shuffle { "🔀 On" } else { "🔀 Off" };
    let text = build_menu_text(playlist_name, items, is_shuffle);

    let rows = vec![
        vec![
            InlineKeyboardButton::callback("🟢 Play All", "pw:play_all".to_string()),
            InlineKeyboardButton::callback(shuffle_label, "pw:shuf".to_string()),
        ],
        vec![
            InlineKeyboardButton::callback("➕ Add", "pw:add".to_string()),
            InlineKeyboardButton::callback("🔍 Search", "pw:srch".to_string()),
            InlineKeyboardButton::callback("📋 Tracks", "pw:list".to_string()),
        ],
        vec![InlineKeyboardButton::callback("🔴 Stop", "pw:stop".to_string())],
    ];
    let keyboard = InlineKeyboardMarkup::new(rows);

    if let Some(msg_id) = old_message_id {
        let _ = bot
            .edit_message_text(chat_id, msg_id, &text)
            .reply_markup(keyboard)
            .await;
        Some(msg_id)
    } else {
        bot.send_message(chat_id, &text)
            .reply_markup(keyboard)
            .await
            .ok()
            .map(|msg| msg.id)
    }
}

// ── Play all tracks ───────────────────────────────────────────────────────

async fn play_all_tracks(bot: &Bot, chat_id: ChatId, db_pool: &Arc<DbPool>, shared_storage: &Arc<SharedStorage>) {
    let session = match shared_storage.get_player_session(chat_id.0).await {
        Ok(Some(s)) => s,
        _ => return,
    };

    let mut items = shared_storage
        .get_playlist_items(session.playlist_id)
        .await
        .unwrap_or_default();
    if items.is_empty() {
        let _ = send_tracked_message(bot, chat_id, "Playlist is empty.", shared_storage).await;
        return;
    }

    // Shuffle if enabled
    if session.is_shuffle {
        let mut rng = rand::thread_rng();
        items.shuffle(&mut rng);
    }

    // Split into cached (have file_id) and uncached
    let (cached, uncached): (Vec<_>, Vec<_>) = items.iter().partition(|item| item.file_id.is_some());

    let total = items.len();
    let cached_count = cached.len();
    let uncached_count = uncached.len();

    // Status message
    let status_text = if uncached_count > 0 && cached_count > 0 {
        format!(
            "📨 Sending {} tracks ({} cached, {} to download)...",
            total, cached_count, uncached_count
        )
    } else if uncached_count > 0 {
        format!("📨 Downloading {} tracks...", uncached_count)
    } else {
        format!("📨 Sending {} cached tracks...", total)
    };
    let status_msg_id = send_tracked_message(bot, chat_id, &status_text, shared_storage).await;

    // Send cached tracks instantly
    let mut send_errors = 0;
    for item in &cached {
        if let Some(ref file_id) = item.file_id {
            match bot
                .send_audio(chat_id, InputFile::file_id(teloxide::types::FileId(file_id.clone())))
                .await
            {
                Ok(_) => {
                    log::info!("Player: sent cached track '{}'", item.title);
                }
                Err(e) => {
                    log::error!("Player: failed to send cached track '{}': {}", item.title, e);
                    send_errors += 1;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        }
    }

    // Download and send uncached tracks in background
    if !uncached.is_empty() {
        let bot_clone = bot.clone();
        let db_pool_clone = Arc::clone(db_pool);
        let shared_storage_clone = Arc::clone(shared_storage);
        let uncached_items: Vec<PlaylistItem> = uncached.into_iter().cloned().collect();
        let status_msg_id_clone = status_msg_id;

        tokio::spawn(async move {
            let mut sent_count = cached_count - send_errors;
            for (i, item) in uncached_items.iter().enumerate() {
                log::info!(
                    "Player: downloading track {}/{} '{}' (url: {}) for chat {}",
                    i + 1,
                    uncached_items.len(),
                    item.title,
                    item.url,
                    chat_id
                );

                let result = timeout(
                    TRACK_DOWNLOAD_TIMEOUT,
                    download_player_track(&bot_clone, chat_id, item, &db_pool_clone, &shared_storage_clone),
                )
                .await;

                match result {
                    Ok(Ok(_)) => {
                        sent_count += 1;
                        log::info!("Player: sent track '{}' ({}/{})", item.title, sent_count, total);
                        if let Some(msg_id) = status_msg_id_clone {
                            let remaining = uncached_items.len() - i - 1;
                            let update_text = if remaining > 0 {
                                format!("📨 Sent {}/{} tracks ({} downloading)...", sent_count, total, remaining)
                            } else {
                                format!("✅ Sent {}/{} tracks.", sent_count, total)
                            };
                            let _ = bot_clone.edit_message_text(chat_id, msg_id, update_text).await;
                        }
                    }
                    Ok(Err(e)) => {
                        log::error!("Player: download failed for '{}': {}", item.title, e);
                        let _ = bot_clone
                            .send_message(chat_id, format!("⚠ Failed: {}", item.title))
                            .await;
                        notify_admin_text(
                            &bot_clone,
                            &format!(
                                "⚠️ Player download failed\n\nUser: {}\nTrack: {}\nURL: {}\nError: {}",
                                chat_id.0, item.title, item.url, e
                            ),
                        )
                        .await;
                    }
                    Err(_) => {
                        log::error!(
                            "Player: download timed out for '{}' ({}s)",
                            item.title,
                            TRACK_DOWNLOAD_TIMEOUT.as_secs()
                        );
                        let _ = bot_clone
                            .send_message(chat_id, format!("⏰ Timeout: {}", item.title))
                            .await;
                        notify_admin_text(
                            &bot_clone,
                            &format!(
                                "⚠️ Player download timeout ({}s)\n\nUser: {}\nTrack: {}\nURL: {}",
                                TRACK_DOWNLOAD_TIMEOUT.as_secs(),
                                chat_id.0,
                                item.title,
                                item.url
                            ),
                        )
                        .await;
                    }
                }

                if i + 1 < uncached_items.len() {
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                }
            }
            // Final status
            if let Some(msg_id) = status_msg_id_clone {
                let final_text = if sent_count == total {
                    format!("✅ All {} tracks sent.", total)
                } else {
                    format!("✅ Done: {}/{} tracks sent.", sent_count, total)
                };
                let _ = bot_clone.edit_message_text(chat_id, msg_id, final_text).await;
            }
        });
    } else {
        // All cached — update status
        if let Some(msg_id) = status_msg_id {
            let text = if send_errors == 0 {
                format!("✅ Sent all {} tracks.", total)
            } else {
                format!("✅ Sent {}/{} tracks.", total - send_errors, total)
            };
            let _ = bot.edit_message_text(chat_id, msg_id, text).await;
        }
    }
}

// ── Download and send a single track ──────────────────────────────────────

async fn download_player_track(
    bot: &Bot,
    chat_id: ChatId,
    item: &PlaylistItem,
    _db_pool: &Arc<DbPool>,
    shared_storage: &Arc<SharedStorage>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let url_str = &item.url;
    // Skip non-video URLs (channels, playlists) — they hang yt-dlp for minutes
    if url_str.contains("/channel/")
        || url_str.contains("/playlist?")
        || url_str.contains("/user/")
        || url_str.contains("/@")
    {
        // Auto-remove invalid track from playlist
        if let Err(e) = shared_storage.remove_playlist_item(item.id).await {
            log::warn!("Failed to auto-remove invalid track {}: {}", item.id, e);
        } else {
            log::info!(
                "🗑️ Auto-removed invalid track '{}' (non-video URL) from playlist",
                item.title
            );
        }
        return Err(format!("Skipped non-video URL: {:.80}", url_str).into());
    }
    let url = Url::parse(url_str)?;

    // Check vault cache first
    if let Some(cached_fid) = crate::download::vault::check_vault_cache(shared_storage, chat_id.0, &item.url).await {
        let input = teloxide::types::InputFile::file_id(teloxide::types::FileId(cached_fid));
        if bot.send_audio(chat_id, input).await.is_ok() {
            return Ok(());
        }
        // Fall through if send fails (expired file_id)
    }

    let registry = bot_global();
    let format = PipelineFormat::Audio {
        bitrate: None,
        time_range: None,
    };

    let lang = crate::i18n::user_lang_from_storage(shared_storage, chat_id.0).await;
    let mut progress_msg = ProgressMessage::new(chat_id, lang);

    // Download phase
    let phase_result = match pipeline::download_phase(
        bot,
        chat_id,
        &url,
        &format,
        registry,
        &mut progress_msg,
        None,
        Some(shared_storage),
    )
    .await
    {
        Ok(r) => {
            // Track progress message for cleanup if Stop is pressed mid-download
            if let Some(msg_id) = progress_msg.message_id {
                track_message(shared_storage, chat_id.0, msg_id.0).await;
            }
            r
        }
        Err(e) => {
            // Track + delete progress message on error too
            if let Some(msg_id) = progress_msg.message_id {
                track_message(shared_storage, chat_id.0, msg_id.0).await;
                let _ = bot.delete_message(chat_id, msg_id).await;
            }
            return Err(format!("Download failed: {:?}", e).into());
        }
    };

    // Send audio
    let duration = phase_result.output.duration_secs.unwrap_or(0);
    let caption = phase_result.caption.as_ref();
    let artist = if phase_result.artist.is_empty() {
        None
    } else {
        Some(phase_result.artist.clone())
    };

    let send_result = send_audio_with_retry(
        bot,
        chat_id,
        &phase_result.output.file_path,
        duration,
        &mut progress_msg,
        caption,
        false,
        None,
        artist,
    )
    .await;

    // Delete progress message before handling result
    if let Some(msg_id) = progress_msg.message_id {
        let _ = bot.delete_message(chat_id, msg_id).await;
    }

    // Cleanup downloaded file
    let _ = tokio::fs::remove_file(&phase_result.output.file_path).await;

    match send_result {
        Ok((sent_msg, _file_size)) => {
            // Cache file_id for instant playback next time
            if let Some(audio) = sent_msg.audio() {
                let fid = &audio.file.id.0;
                log::info!(
                    "Player: cached file_id for item {} ('{}'): {}",
                    item.id,
                    item.title,
                    fid
                );
                let _ = shared_storage.update_playlist_item_file_id(item.id, fid).await;
            } else if let Some(doc) = sent_msg.document() {
                // Sent as document (large file fallback)
                let fid = &doc.file.id.0;
                log::info!(
                    "Player: cached file_id (doc) for item {} ('{}'): {}",
                    item.id,
                    item.title,
                    fid
                );
                let _ = shared_storage.update_playlist_item_file_id(item.id, fid).await;
            }
            // Send to vault
            let vault_fid = sent_msg
                .audio()
                .map(|a| a.file.id.0.clone())
                .or_else(|| sent_msg.document().map(|d| d.file.id.0.clone()));
            if let Some(fid) = vault_fid {
                crate::download::vault::send_to_vault_background(
                    bot.clone(),
                    Arc::clone(shared_storage),
                    chat_id.0,
                    item.url.clone(),
                    fid,
                    Some(item.title.clone()),
                    None,
                    None,
                    None,
                );
            }
            Ok(())
        }
        Err(e) => {
            log::error!("Player: send failed for '{}': {}", item.title, e);
            Err(format!("Send failed: {}", e).into())
        }
    }
}

// ── Message tracking helpers ──────────────────────────────────────────────

/// Send a message and track it for cleanup on player exit.
async fn send_tracked_message(
    bot: &Bot,
    chat_id: ChatId,
    text: &str,
    shared_storage: &Arc<SharedStorage>,
) -> Option<MessageId> {
    let msg = bot.send_message(chat_id, text).await.ok()?;
    track_message(shared_storage, chat_id.0, msg.id.0).await;
    Some(msg.id)
}

/// Track an already-sent message for cleanup on player exit.
async fn track_message(shared_storage: &Arc<SharedStorage>, user_id: i64, message_id: i32) {
    let _ = shared_storage.add_player_message(user_id, message_id).await;
}

// ── Callback handler ──────────────────────────────────────────────────────

pub async fn handle_player_callback(
    bot: &Bot,
    callback_id: CallbackQueryId,
    chat_id: ChatId,
    message_id: MessageId,
    data: &str,
    db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
    _download_queue: Arc<crate::download::queue::DownloadQueue>,
) -> Result<(), teloxide::RequestError> {
    let _ = bot.answer_callback_query(callback_id).await;

    // pw:play:{pl_id} — enter player mode
    if let Some(pl_id_str) = data.strip_prefix("pw:play:") {
        if let Ok(pl_id) = pl_id_str.parse::<i64>() {
            match shared_storage.get_playlist(pl_id).await {
                Ok(Some(pl)) if pl.user_id == chat_id.0 || pl.is_public => {
                    let items = shared_storage.get_playlist_items(pl_id).await.unwrap_or_default();
                    if items.is_empty() {
                        let _ = bot.send_message(chat_id, "Playlist is empty.").await;
                        return Ok(());
                    }
                    let _ = bot.delete_message(chat_id, message_id).await;
                    enter_player_mode(bot, chat_id, pl_id, &pl.name, &items, &db_pool, &shared_storage).await;
                }
                _ => return Ok(()),
            }
        }
        return Ok(());
    }

    // Get current session for all other callbacks
    let session = match shared_storage.get_player_session(chat_id.0).await {
        Ok(Some(s)) => s,
        _ => {
            let _ = bot.send_message(chat_id, "No active player session.").await;
            return Ok(());
        }
    };

    let items = shared_storage
        .get_playlist_items(session.playlist_id)
        .await
        .unwrap_or_default();
    let pl_name = shared_storage
        .get_playlist(session.playlist_id)
        .await
        .ok()
        .flatten()
        .map(|p| p.name)
        .unwrap_or_default();

    match data {
        "pw:play_all" => {
            play_all_tracks(bot, chat_id, &db_pool, &shared_storage).await;
        }
        "pw:shuf" => {
            if let Ok(new_shuffle) = shared_storage.toggle_player_shuffle(chat_id.0).await {
                let _ = bot.delete_message(chat_id, message_id).await;
                let new_msg = send_player_menu(bot, chat_id, &pl_name, &items, new_shuffle, None).await;
                if let Some(msg_id) = new_msg {
                    track_message(&shared_storage, chat_id.0, msg_id.0).await;
                }
            }
        }
        "pw:list" => {
            let page_items = shared_storage
                .get_playlist_items_page(session.playlist_id, 0, 20)
                .await
                .unwrap_or_default();
            let total = items.len();
            let mut text = format!("📋 {} ({} tracks)\n\n", pl_name, total);
            for item in &page_items {
                let dur = format_duration(item.duration_secs.map(|d| d as u32));
                let artist = item.artist.as_deref().unwrap_or("");
                let cached = if item.file_id.is_some() { " ✓" } else { "" };
                if artist.is_empty() {
                    text.push_str(&format!("{}.  {} ({}){}\n", item.position + 1, item.title, dur, cached));
                } else {
                    text.push_str(&format!(
                        "{}.  {} - {} ({}){}\n",
                        item.position + 1,
                        artist,
                        item.title,
                        dur,
                        cached
                    ));
                }
            }
            let _ = send_tracked_message(bot, chat_id, &text, &shared_storage).await;
        }
        "pw:srch" => {
            let _ = send_tracked_message(bot, chat_id, "🔍 Type your search query:", &shared_storage).await;
        }
        "pw:add" => {
            let _ = send_tracked_message(bot, chat_id, "🔍 Search for a track to add:", &shared_storage).await;
        }
        "pw:stop" => {
            stop_player(bot, chat_id, &db_pool, &shared_storage).await;
        }
        _ => {}
    }

    Ok(())
}
