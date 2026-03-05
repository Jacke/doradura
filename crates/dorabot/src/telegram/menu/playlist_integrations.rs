//! Playlist Integrations: import playlists from Spotify, SoundCloud, YouTube, Yandex Music.
//!
//! Callback prefix: `pi:`
//! - `pi:list:{page}` — list synced playlists
//! - `pi:new` — start import (ask for URL)
//! - `pi:view:{id}:{page}` — view tracks in a playlist
//! - `pi:play:{id}` — play all tracks
//! - `pi:resync:{id}` — re-sync from source
//! - `pi:del:{id}` — confirm delete
//! - `pi:delok:{id}` — execute delete
//! - `pi:dl:{id}:{track_id}` — download single track
//! - `pi:retry:{id}` — retry not_found tracks

use crate::download::pipeline::{self, PipelineFormat};
use crate::download::playlist_sync::{self, resolver::Platform};
use crate::download::progress::ProgressMessage;
use crate::download::search::format_duration;
use crate::download::send::send_audio_with_retry;
use crate::download::source::SourceRegistry;
use crate::storage::db::{self, DbPool};
use crate::telegram::notifications::notify_admin_text;
use crate::telegram::Bot;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use teloxide::prelude::*;
use teloxide::types::{
    CallbackQueryId, ChatId, FileId, InlineKeyboardButton, InlineKeyboardMarkup, InputFile, MessageId,
};
use tokio::sync::RwLock;
use tokio::time::timeout;
use url::Url;

const TRACKS_PER_PAGE: i64 = 10;
const PLAYLISTS_PER_PAGE: usize = 8;
const TRACK_DOWNLOAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

// ── State: waiting for import URL ───────────────────────────────────────

static IMPORT_URL_STATES: std::sync::LazyLock<Arc<RwLock<HashMap<i64, Instant>>>> =
    std::sync::LazyLock::new(|| Arc::new(RwLock::new(HashMap::new())));

pub async fn is_waiting_for_import_url(user_id: i64) -> bool {
    let states = IMPORT_URL_STATES.read().await;
    if let Some(ts) = states.get(&user_id) {
        ts.elapsed().as_secs() < 300
    } else {
        false
    }
}

async fn set_waiting_for_import_url(user_id: i64, waiting: bool) {
    let mut states = IMPORT_URL_STATES.write().await;
    if waiting {
        states.insert(user_id, Instant::now());
    } else {
        states.remove(&user_id);
    }
}

// ── /playlist_integrations command ──────────────────────────────────────

pub async fn handle_playlist_integrations_command(bot: &Bot, chat_id: ChatId, db_pool: &Arc<DbPool>) {
    show_playlist_list(bot, chat_id, 0, db_pool, None).await;
}

// ── URL input handler (called from message handler) ─────────────────────

pub async fn handle_import_url_input(bot: &Bot, chat_id: ChatId, text: &str, db_pool: Arc<DbPool>) {
    set_waiting_for_import_url(chat_id.0, false).await;

    let url = text.trim();

    let platform = match playlist_sync::detect_platform(url) {
        Some(p) => p,
        None => {
            let _ = bot
                .send_message(chat_id, "Unsupported URL. Supported platforms:\n• Spotify (playlists, albums)\n• SoundCloud (sets, likes)\n• YouTube (playlists)\n• Yandex Music (playlists, albums)")
                .await;
            return;
        }
    };

    // Check for duplicate
    if let Ok(conn) = db::get_connection(&db_pool) {
        if let Ok(Some(existing)) = db::get_synced_playlist_by_url(&conn, chat_id.0, url) {
            let text = format!(
                "{} \"{}\" is already imported ({} tracks).\n\nUse Re-sync to update it.",
                platform.icon(),
                existing.name,
                existing.track_count
            );
            let kb = InlineKeyboardMarkup::new(vec![
                vec![InlineKeyboardButton::callback(
                    "🔄 Re-sync",
                    format!("pi:resync:{}", existing.id),
                )],
                vec![InlineKeyboardButton::callback("◀ Back", "pi:list:0".to_string())],
            ]);
            let _ = bot.send_message(chat_id, text).reply_markup(kb).await;
            return;
        }
    }

    let progress_msg = bot
        .send_message(
            chat_id,
            format!("{} Importing {} playlist...", platform.icon(), platform.label()),
        )
        .await;

    let progress_msg_id = progress_msg.as_ref().ok().map(|m| m.id);
    let url_owned = url.to_string();
    let bot_clone = bot.clone();
    let db_pool_clone = db_pool.clone();

    // Progress callback for Spotify (per-track YouTube search)
    let bot_progress = bot.clone();
    let chat_id_progress = chat_id;
    let msg_id_progress = progress_msg_id;
    let last_update = Arc::new(std::sync::Mutex::new(Instant::now()));

    let progress_fn: Option<playlist_sync::resolver::ProgressFn> = if platform == Platform::Spotify {
        let last_update = last_update.clone();
        Some(Arc::new(move |current: usize, total: usize, title: &str| {
            let mut last = last_update.lock().unwrap();
            if last.elapsed().as_secs() < 3 && current > 1 && current < total {
                return;
            }
            *last = Instant::now();
            let msg_text = if total > 0 {
                format!("🔍 {}/{} — Searching: {}", current, total, title)
            } else {
                format!("🔍 {} — Searching: {}", current, title)
            };
            if let Some(mid) = msg_id_progress {
                let bot = bot_progress.clone();
                tokio::spawn(async move {
                    let _ = bot.edit_message_text(chat_id_progress, mid, msg_text).await;
                });
            }
        }))
    } else {
        None
    };

    let result = playlist_sync::import_playlist(&url_owned, db_pool_clone.clone(), progress_fn).await;

    match result {
        Ok(resolved) => {
            let matched = resolved
                .tracks
                .iter()
                .filter(|t| t.status == playlist_sync::resolver::TrackStatus::Matched)
                .count();
            let not_found = resolved
                .tracks
                .iter()
                .filter(|t| t.status == playlist_sync::resolver::TrackStatus::NotFound)
                .count();
            let total = resolved.tracks.len();
            let name = resolved.name.clone();
            let platform = resolved.platform;

            let playlist_id = match db::get_connection(&db_pool_clone) {
                Ok(conn) => match playlist_sync::save_resolved_playlist(&conn, chat_id.0, &url_owned, &resolved) {
                    Ok(id) => id,
                    Err(e) => {
                        let _ = bot_clone.send_message(chat_id, format!("Failed to save: {}", e)).await;
                        return;
                    }
                },
                Err(_) => {
                    let _ = bot_clone.send_message(chat_id, "Database error").await;
                    return;
                }
            };

            let mut summary = format!(
                "✅ Imported \"{}\" from {}\n📊 {} matched",
                name,
                platform.label(),
                matched
            );
            if not_found > 0 {
                summary.push_str(&format!(" | ⚠️ {} not found", not_found));
            }

            let kb = InlineKeyboardMarkup::new(vec![
                vec![
                    InlineKeyboardButton::callback("📋 View Tracks", format!("pi:view:{}:0", playlist_id)),
                    InlineKeyboardButton::callback("▶ Play All", format!("pi:play:{}", playlist_id)),
                ],
                vec![InlineKeyboardButton::callback(
                    "◀ Back to List",
                    "pi:list:0".to_string(),
                )],
            ]);

            if let Some(mid) = progress_msg_id {
                let _ = bot_clone
                    .edit_message_text(chat_id, mid, &summary)
                    .reply_markup(kb)
                    .await;
            } else {
                let _ = bot_clone.send_message(chat_id, &summary).reply_markup(kb).await;
            }

            if total > 50 {
                notify_admin_text(
                    &bot_clone,
                    &format!(
                        "📥 Large playlist import: user {} imported \"{}\" ({}) — {} tracks ({} matched, {} not found)",
                        chat_id.0,
                        name,
                        platform.label(),
                        total,
                        matched,
                        not_found
                    ),
                )
                .await;
            }
        }
        Err(e) => {
            let error_text = format!("❌ Import failed: {}", e);
            if let Some(mid) = progress_msg_id {
                let _ = bot_clone.edit_message_text(chat_id, mid, &error_text).await;
            } else {
                let _ = bot_clone.send_message(chat_id, &error_text).await;
            }
        }
    }
}

// ── Callback handler ────────────────────────────────────────────────────

pub async fn handle_playlist_integrations_callback(
    bot: &Bot,
    callback_id: CallbackQueryId,
    chat_id: ChatId,
    message_id: MessageId,
    data: &str,
    db_pool: Arc<DbPool>,
    _download_queue: Arc<crate::download::queue::DownloadQueue>,
) -> Result<(), teloxide::RequestError> {
    let _ = bot.answer_callback_query(callback_id).await;

    let suffix = &data[3..]; // strip "pi:"

    if let Some(page_str) = suffix.strip_prefix("list:") {
        let page = page_str.parse::<usize>().unwrap_or(0);
        show_playlist_list(bot, chat_id, page, &db_pool, Some(message_id)).await;
    } else if suffix == "new" {
        set_waiting_for_import_url(chat_id.0, true).await;
        let text = "📥 Send a playlist URL:\n\n• Spotify: open.spotify.com/playlist/...\n• SoundCloud: soundcloud.com/.../sets/...\n• YouTube: youtube.com/playlist?list=...\n• Yandex Music: music.yandex.ru/.../playlists/...";
        let kb = InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::callback(
            "❌ Cancel",
            "pi:list:0".to_string(),
        )]]);
        let _ = bot.edit_message_text(chat_id, message_id, text).reply_markup(kb).await;
    } else if let Some(rest) = suffix.strip_prefix("view:") {
        let parts: Vec<&str> = rest.splitn(2, ':').collect();
        if parts.len() == 2 {
            let id = parts[0].parse::<i64>().unwrap_or(0);
            let page = parts[1].parse::<i64>().unwrap_or(0);
            show_tracks_view(bot, chat_id, message_id, id, page, &db_pool).await;
        }
    } else if let Some(id_str) = suffix.strip_prefix("play:") {
        let id = id_str.parse::<i64>().unwrap_or(0);
        play_all(bot, chat_id, message_id, id, &db_pool).await;
    } else if let Some(id_str) = suffix.strip_prefix("resync:") {
        let id = id_str.parse::<i64>().unwrap_or(0);
        resync_playlist(bot, chat_id, message_id, id, &db_pool).await;
    } else if let Some(id_str) = suffix.strip_prefix("del:") {
        let id = id_str.parse::<i64>().unwrap_or(0);
        confirm_delete(bot, chat_id, message_id, id, &db_pool).await;
    } else if let Some(id_str) = suffix.strip_prefix("delok:") {
        let id = id_str.parse::<i64>().unwrap_or(0);
        execute_delete(bot, chat_id, message_id, id, &db_pool).await;
    } else if let Some(rest) = suffix.strip_prefix("dl:") {
        let parts: Vec<&str> = rest.splitn(2, ':').collect();
        if parts.len() == 2 {
            let pl_id = parts[0].parse::<i64>().unwrap_or(0);
            let track_id = parts[1].parse::<i64>().unwrap_or(0);
            download_single_track(bot, chat_id, pl_id, track_id, &db_pool).await;
        }
    } else if let Some(id_str) = suffix.strip_prefix("retry:") {
        let id = id_str.parse::<i64>().unwrap_or(0);
        retry_not_found(bot, chat_id, message_id, id, &db_pool).await;
    }

    Ok(())
}

// ── Playlist list ───────────────────────────────────────────────────────

async fn show_playlist_list(
    bot: &Bot,
    chat_id: ChatId,
    page: usize,
    db_pool: &Arc<DbPool>,
    edit_msg: Option<MessageId>,
) {
    set_waiting_for_import_url(chat_id.0, false).await;

    let conn = match db::get_connection(db_pool) {
        Ok(c) => c,
        Err(_) => {
            let _ = bot.send_message(chat_id, "Database error").await;
            return;
        }
    };

    let playlists = db::get_user_synced_playlists(&conn, chat_id.0).unwrap_or_default();

    if playlists.is_empty() {
        let text = "🎵 Playlist Integrations\n\nNo imported playlists yet.\nImport from Spotify, SoundCloud, YouTube, or Yandex Music!";
        let kb = InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::callback(
            "📥 Import New",
            "pi:new".to_string(),
        )]]);
        if let Some(mid) = edit_msg {
            let _ = bot.edit_message_text(chat_id, mid, text).reply_markup(kb).await;
        } else {
            let _ = bot.send_message(chat_id, text).reply_markup(kb).await;
        }
        return;
    }

    let total_pages = playlists.len().div_ceil(PLAYLISTS_PER_PAGE);
    let page = page.min(total_pages.saturating_sub(1));
    let start = page * PLAYLISTS_PER_PAGE;
    let page_items = &playlists[start..(start + PLAYLISTS_PER_PAGE).min(playlists.len())];

    let mut text = "🎵 Playlist Integrations\n\n".to_string();
    for (i, pl) in page_items.iter().enumerate() {
        let icon = Platform::from_db_name(&pl.source_platform)
            .map(|p| p.icon())
            .unwrap_or("🎵");
        text.push_str(&format!(
            "{}. {} {} — {} tracks\n",
            start + i + 1,
            icon,
            pl.name,
            pl.track_count
        ));
    }

    let mut rows: Vec<Vec<InlineKeyboardButton>> = Vec::new();
    for chunk in page_items.chunks(2) {
        let mut row = Vec::new();
        for pl in chunk {
            let icon = Platform::from_db_name(&pl.source_platform)
                .map(|p| p.icon())
                .unwrap_or("🎵");
            let label = format!("{} {}", icon, truncate_name(&pl.name, 18));
            row.push(InlineKeyboardButton::callback(label, format!("pi:view:{}:0", pl.id)));
        }
        rows.push(row);
    }

    if total_pages > 1 {
        let mut nav = Vec::new();
        if page > 0 {
            nav.push(InlineKeyboardButton::callback("◀", format!("pi:list:{}", page - 1)));
        }
        nav.push(InlineKeyboardButton::callback(
            format!("{}/{}", page + 1, total_pages),
            "pi:noop".to_string(),
        ));
        if page + 1 < total_pages {
            nav.push(InlineKeyboardButton::callback("▶", format!("pi:list:{}", page + 1)));
        }
        rows.push(nav);
    }

    rows.push(vec![InlineKeyboardButton::callback(
        "📥 Import New",
        "pi:new".to_string(),
    )]);

    let kb = InlineKeyboardMarkup::new(rows);
    if let Some(mid) = edit_msg {
        let _ = bot.edit_message_text(chat_id, mid, &text).reply_markup(kb).await;
    } else {
        let _ = bot.send_message(chat_id, &text).reply_markup(kb).await;
    }
}

// ── Track view ──────────────────────────────────────────────────────────

async fn show_tracks_view(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    playlist_id: i64,
    page: i64,
    db_pool: &Arc<DbPool>,
) {
    let conn = match db::get_connection(db_pool) {
        Ok(c) => c,
        Err(_) => return,
    };

    let pl = match db::get_synced_playlist(&conn, playlist_id) {
        Ok(Some(p)) => p,
        _ => return,
    };

    let icon = Platform::from_db_name(&pl.source_platform)
        .map(|p| p.icon())
        .unwrap_or("🎵");
    let platform_label = Platform::from_db_name(&pl.source_platform)
        .map(|p| p.label())
        .unwrap_or("Unknown");

    let total_tracks = db::count_synced_tracks(&conn, playlist_id).unwrap_or(0);
    let total_pages = ((total_tracks as i64) + TRACKS_PER_PAGE - 1) / TRACKS_PER_PAGE;
    let page = page.min(total_pages.saturating_sub(1)).max(0);

    let tracks =
        db::get_synced_tracks_page(&conn, playlist_id, page * TRACKS_PER_PAGE, TRACKS_PER_PAGE).unwrap_or_default();

    let mut text = format!(
        "{} {} ({})\n📊 {} matched",
        icon, pl.name, platform_label, pl.matched_count
    );
    if pl.not_found_count > 0 {
        text.push_str(&format!(" | ⚠️ {} not found", pl.not_found_count));
    }
    text.push_str("\n\n");

    for track in &tracks {
        let status_icon = if track.import_status == "not_found" {
            "⚠️ "
        } else {
            ""
        };
        let duration = format_duration(track.duration_secs.map(|d| d as u32));
        let artist = track.artist.as_deref().unwrap_or("");
        if artist.is_empty() {
            text.push_str(&format!(
                "{}{}. {} ({})\n",
                status_icon,
                track.position + 1,
                track.title,
                duration
            ));
        } else {
            text.push_str(&format!(
                "{}{}. {} — {} ({})\n",
                status_icon,
                track.position + 1,
                artist,
                track.title,
                duration
            ));
        }
    }

    let mut rows: Vec<Vec<InlineKeyboardButton>> = Vec::new();

    for track in &tracks {
        if track.import_status == "matched" || track.file_id.is_some() {
            rows.push(vec![InlineKeyboardButton::callback(
                format!("🎵 {}", truncate_name(&track.title, 25)),
                format!("pi:dl:{}:{}", playlist_id, track.id),
            )]);
        }
    }

    if total_pages > 1 {
        let mut nav = Vec::new();
        if page > 0 {
            nav.push(InlineKeyboardButton::callback(
                "◀",
                format!("pi:view:{}:{}", playlist_id, page - 1),
            ));
        }
        nav.push(InlineKeyboardButton::callback(
            format!("{}/{}", page + 1, total_pages),
            format!("pi:view:{}:{}", playlist_id, page),
        ));
        if page + 1 < total_pages {
            nav.push(InlineKeyboardButton::callback(
                "▶",
                format!("pi:view:{}:{}", playlist_id, page + 1),
            ));
        }
        rows.push(nav);
    }

    rows.push(vec![
        InlineKeyboardButton::callback("▶ Play All", format!("pi:play:{}", playlist_id)),
        InlineKeyboardButton::callback("🔄 Re-sync", format!("pi:resync:{}", playlist_id)),
    ]);

    if pl.not_found_count > 0 {
        rows.push(vec![InlineKeyboardButton::callback(
            format!("🔍 Retry {} not found", pl.not_found_count),
            format!("pi:retry:{}", playlist_id),
        )]);
    }

    rows.push(vec![
        InlineKeyboardButton::callback("🗑 Delete", format!("pi:del:{}", playlist_id)),
        InlineKeyboardButton::callback("◀ Back", "pi:list:0".to_string()),
    ]);

    let kb = InlineKeyboardMarkup::new(rows);
    let _ = bot.edit_message_text(chat_id, message_id, &text).reply_markup(kb).await;
}

// ── Play All ────────────────────────────────────────────────────────────

async fn play_all(bot: &Bot, chat_id: ChatId, message_id: MessageId, playlist_id: i64, db_pool: &Arc<DbPool>) {
    let conn = match db::get_connection(db_pool) {
        Ok(c) => c,
        Err(_) => return,
    };

    let pl = match db::get_synced_playlist(&conn, playlist_id) {
        Ok(Some(p)) => p,
        _ => return,
    };

    let tracks = db::get_synced_tracks(&conn, playlist_id).unwrap_or_default();
    let playable: Vec<_> = tracks
        .into_iter()
        .filter(|t| t.import_status == "matched" || t.file_id.is_some())
        .collect();

    if playable.is_empty() {
        let _ = bot
            .edit_message_text(chat_id, message_id, "No playable tracks in this playlist.")
            .await;
        return;
    }

    let total = playable.len();
    let _ = bot
        .edit_message_text(chat_id, message_id, format!("▶ Playing \"{}\" — 0/{}", pl.name, total))
        .await;

    let bot_clone = bot.clone();
    let db_pool_clone = db_pool.clone();
    let pl_name = pl.name.clone();
    let msg_id = message_id;

    tokio::spawn(async move {
        let mut sent = 0;
        for track in &playable {
            sent += 1;
            let _ = bot_clone
                .edit_message_text(
                    chat_id,
                    msg_id,
                    format!("▶ Playing {}/{} — {}", sent, total, track.title),
                )
                .await;

            // Try cached file_id first
            if let Some(ref fid) = track.file_id {
                let input = InputFile::file_id(FileId(fid.clone()));
                let _ = bot_clone.send_audio(chat_id, input).await;
                continue;
            }

            // Download via pipeline
            let url_str = match track.resolved_url.as_deref().or(track.source_url.as_deref()) {
                Some(u) => u,
                None => continue,
            };

            let url = match Url::parse(url_str) {
                Ok(u) => u,
                Err(_) => continue,
            };

            let registry = SourceRegistry::global();
            let format = PipelineFormat::Audio {
                bitrate: None,
                time_range: None,
            };
            let lang = crate::i18n::user_lang_from_pool(&db_pool_clone, chat_id.0);
            let mut progress_msg = ProgressMessage::new(chat_id, lang);

            let result = timeout(
                TRACK_DOWNLOAD_TIMEOUT,
                pipeline::download_phase(&bot_clone, chat_id, &url, &format, registry, &mut progress_msg, None),
            )
            .await;

            // Cleanup progress message
            if let Some(pmid) = progress_msg.message_id {
                let _ = bot_clone.delete_message(chat_id, pmid).await;
            }

            match result {
                Ok(Ok(phase_result)) => {
                    let duration = phase_result.output.duration_secs.unwrap_or(0);
                    let caption = phase_result.caption.as_ref();
                    let artist = if phase_result.artist.is_empty() {
                        None
                    } else {
                        Some(phase_result.artist.clone())
                    };

                    let send_result = send_audio_with_retry(
                        &bot_clone,
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

                    if let Ok((sent_msg, _)) = send_result {
                        if let Some(audio) = sent_msg.audio() {
                            if let Ok(conn) = db::get_connection(&db_pool_clone) {
                                let _ = db::update_synced_track_file_id(&conn, track.id, &audio.file.id.0);
                            }
                        }
                    }
                    let _ = tokio::fs::remove_file(&phase_result.output.file_path).await;
                }
                Ok(Err(e)) => log::error!("Download failed for {}: {:?}", track.title, e),
                Err(_) => log::error!("Download timed out for {}", track.title),
            }
        }

        let _ = bot_clone
            .edit_message_text(
                chat_id,
                msg_id,
                format!("✅ Finished playing \"{}\" — {}/{} tracks", pl_name, sent, total),
            )
            .await;
    });
}

// ── Re-sync ─────────────────────────────────────────────────────────────

async fn resync_playlist(bot: &Bot, chat_id: ChatId, message_id: MessageId, playlist_id: i64, db_pool: &Arc<DbPool>) {
    let conn = match db::get_connection(db_pool) {
        Ok(c) => c,
        Err(_) => return,
    };

    let pl = match db::get_synced_playlist(&conn, playlist_id) {
        Ok(Some(p)) => p,
        _ => return,
    };

    let _ = bot
        .edit_message_text(chat_id, message_id, format!("🔄 Re-syncing \"{}\"...", pl.name))
        .await;

    let url = pl.source_url.clone();
    let db_pool_clone = db_pool.clone();
    let bot_clone = bot.clone();

    let result = playlist_sync::import_playlist(&url, db_pool_clone.clone(), None).await;

    match result {
        Ok(resolved) => {
            if let Ok(conn) = db::get_connection(&db_pool_clone) {
                let _ = db::delete_synced_tracks(&conn, playlist_id);

                let matched = resolved
                    .tracks
                    .iter()
                    .filter(|t| t.status == playlist_sync::resolver::TrackStatus::Matched)
                    .count() as i32;
                let not_found = resolved
                    .tracks
                    .iter()
                    .filter(|t| t.status == playlist_sync::resolver::TrackStatus::NotFound)
                    .count() as i32;

                for (i, track) in resolved.tracks.iter().enumerate() {
                    let _ = db::add_synced_track(
                        &conn,
                        playlist_id,
                        i as i32,
                        &track.title,
                        track.artist.as_deref(),
                        track.duration_secs,
                        track.external_id.as_deref(),
                        track.source_url.as_deref(),
                        track.resolved_url.as_deref(),
                        track.status.as_str(),
                    );
                }

                let _ = db::update_synced_playlist_counts(
                    &conn,
                    playlist_id,
                    resolved.tracks.len() as i32,
                    matched,
                    not_found,
                );

                let summary = format!(
                    "✅ Re-synced \"{}\"\n📊 {} matched | ⚠️ {} not found",
                    resolved.name, matched, not_found
                );
                let kb = InlineKeyboardMarkup::new(vec![vec![
                    InlineKeyboardButton::callback("📋 View", format!("pi:view:{}:0", playlist_id)),
                    InlineKeyboardButton::callback("◀ Back", "pi:list:0".to_string()),
                ]]);
                let _ = bot_clone
                    .edit_message_text(chat_id, message_id, summary)
                    .reply_markup(kb)
                    .await;
            }
        }
        Err(e) => {
            let text = format!("❌ Re-sync failed: {}", e);
            let kb = InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::callback(
                "◀ Back",
                format!("pi:view:{}:0", playlist_id),
            )]]);
            let _ = bot_clone
                .edit_message_text(chat_id, message_id, &text)
                .reply_markup(kb)
                .await;
        }
    }
}

// ── Delete ──────────────────────────────────────────────────────────────

async fn confirm_delete(bot: &Bot, chat_id: ChatId, message_id: MessageId, playlist_id: i64, db_pool: &Arc<DbPool>) {
    let conn = match db::get_connection(db_pool) {
        Ok(c) => c,
        Err(_) => return,
    };

    let pl = match db::get_synced_playlist(&conn, playlist_id) {
        Ok(Some(p)) => p,
        _ => return,
    };

    let text = format!(
        "🗑 Delete \"{}\" ({} tracks)?\n\nThis cannot be undone.",
        pl.name, pl.track_count
    );
    let kb = InlineKeyboardMarkup::new(vec![
        vec![InlineKeyboardButton::callback(
            "🗑 Yes, Delete",
            format!("pi:delok:{}", playlist_id),
        )],
        vec![InlineKeyboardButton::callback(
            "◀ Cancel",
            format!("pi:view:{}:0", playlist_id),
        )],
    ]);
    let _ = bot.edit_message_text(chat_id, message_id, text).reply_markup(kb).await;
}

async fn execute_delete(bot: &Bot, chat_id: ChatId, message_id: MessageId, playlist_id: i64, db_pool: &Arc<DbPool>) {
    if let Ok(conn) = db::get_connection(db_pool) {
        let _ = db::delete_synced_playlist(&conn, playlist_id);
    }
    show_playlist_list(bot, chat_id, 0, db_pool, Some(message_id)).await;
}

// ── Download single track ───────────────────────────────────────────────

async fn download_single_track(bot: &Bot, chat_id: ChatId, _playlist_id: i64, track_id: i64, db_pool: &Arc<DbPool>) {
    let conn = match db::get_connection(db_pool) {
        Ok(c) => c,
        Err(_) => return,
    };

    let track = match db::get_synced_track(&conn, track_id) {
        Ok(Some(t)) => t,
        _ => return,
    };

    // Try cached file_id first
    if let Some(ref fid) = track.file_id {
        let input = InputFile::file_id(FileId(fid.clone()));
        let _ = bot.send_audio(chat_id, input).await;
        return;
    }

    let url_str = match track.resolved_url.as_deref().or(track.source_url.as_deref()) {
        Some(u) => u,
        None => {
            let _ = bot.send_message(chat_id, "No downloadable URL for this track").await;
            return;
        }
    };

    let url = match Url::parse(url_str) {
        Ok(u) => u,
        Err(_) => {
            let _ = bot.send_message(chat_id, "Invalid track URL").await;
            return;
        }
    };

    let registry = SourceRegistry::global();
    let format = PipelineFormat::Audio {
        bitrate: None,
        time_range: None,
    };
    let lang = crate::i18n::user_lang_from_pool(db_pool, chat_id.0);
    let mut progress_msg = ProgressMessage::new(chat_id, lang);

    let result = timeout(
        TRACK_DOWNLOAD_TIMEOUT,
        pipeline::download_phase(bot, chat_id, &url, &format, registry, &mut progress_msg, None),
    )
    .await;

    // Cleanup progress message
    if let Some(pmid) = progress_msg.message_id {
        let _ = bot.delete_message(chat_id, pmid).await;
    }

    match result {
        Ok(Ok(phase_result)) => {
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

            match send_result {
                Ok((sent_msg, _)) => {
                    if let Some(audio) = sent_msg.audio() {
                        if let Ok(conn) = db::get_connection(db_pool) {
                            let _ = db::update_synced_track_file_id(&conn, track_id, &audio.file.id.0);
                        }
                    }
                }
                Err(e) => {
                    let _ = bot.send_message(chat_id, format!("Failed to send: {}", e)).await;
                }
            }
            let _ = tokio::fs::remove_file(&phase_result.output.file_path).await;
        }
        Ok(Err(e)) => {
            let _ = bot.send_message(chat_id, format!("Download failed: {:?}", e)).await;
        }
        Err(_) => {
            let _ = bot.send_message(chat_id, "Download timed out").await;
        }
    }
}

// ── Retry not found ─────────────────────────────────────────────────────

async fn retry_not_found(bot: &Bot, chat_id: ChatId, message_id: MessageId, playlist_id: i64, db_pool: &Arc<DbPool>) {
    let conn = match db::get_connection(db_pool) {
        Ok(c) => c,
        Err(_) => return,
    };

    let tracks = db::get_synced_tracks(&conn, playlist_id).unwrap_or_default();
    // Collect owned data for the spawn
    let not_found: Vec<(i64, String, Option<String>)> = tracks
        .iter()
        .filter(|t| t.import_status == "not_found")
        .map(|t| (t.id, t.title.clone(), t.artist.clone()))
        .collect();

    if not_found.is_empty() {
        let _ = bot.edit_message_text(chat_id, message_id, "No tracks to retry.").await;
        return;
    }

    let total = not_found.len();
    let _ = bot
        .edit_message_text(chat_id, message_id, format!("🔍 Retrying {} tracks...", total))
        .await;

    let bot_clone = bot.clone();
    let db_pool_clone = db_pool.clone();
    let msg_id = message_id;

    tokio::spawn(async move {
        let mut found: i32 = 0;
        for (i, (track_id, title, artist)) in not_found.iter().enumerate() {
            let search_query = if let Some(ref art) = artist {
                format!("{} - {}", art, title)
            } else {
                title.clone()
            };

            let result = crate::download::search::search(
                crate::download::search::SearchSource::YouTube,
                &search_query,
                1,
                Some(&db_pool_clone),
            )
            .await;

            if let Ok(results) = result {
                if let Some(first) = results.first() {
                    if let Ok(conn) = db::get_connection(&db_pool_clone) {
                        let _ = db::update_synced_track_status(&conn, *track_id, "matched", Some(&first.url));
                    }
                    found += 1;
                }
            }

            if (i + 1) % 3 == 0 || i + 1 == total {
                let _ = bot_clone
                    .edit_message_text(
                        chat_id,
                        msg_id,
                        format!("🔍 {}/{} — found {} so far", i + 1, total, found),
                    )
                    .await;
            }
        }

        // Update playlist counts
        if let Ok(conn) = db::get_connection(&db_pool_clone) {
            if let Ok(Some(pl)) = db::get_synced_playlist(&conn, playlist_id) {
                let new_matched = pl.matched_count + found;
                let new_not_found = pl.not_found_count - found;
                let _ =
                    db::update_synced_playlist_counts(&conn, playlist_id, pl.track_count, new_matched, new_not_found);
            }
        }

        let text = format!("✅ Retry complete: found {} of {} tracks", found, total);
        let kb = InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::callback(
            "📋 View Tracks",
            format!("pi:view:{}:0", playlist_id),
        )]]);
        let _ = bot_clone
            .edit_message_text(chat_id, msg_id, text)
            .reply_markup(kb)
            .await;
    });
}

// ── Helpers ─────────────────────────────────────────────────────────────

fn truncate_name(name: &str, max_len: usize) -> String {
    if name.chars().count() <= max_len {
        name.to_string()
    } else {
        let truncated: String = name.chars().take(max_len - 1).collect();
        format!("{}…", truncated)
    }
}
