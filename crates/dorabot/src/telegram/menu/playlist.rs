//! Playlist management UI: create, view, edit, delete playlists and their tracks.
//!
//! Callback data prefixes:
//! - `pl:list:{page}` — list playlists
//! - `pl:view:{pl_id}:{page}` — view tracks in playlist
//! - `pl:new` — create new playlist (enters name session)
//! - `pl:ren:{pl_id}` — rename playlist (enters name session)
//! - `pl:del:{pl_id}` — confirm delete
//! - `pl:delok:{pl_id}` — execute delete
//! - `pl:play:{pl_id}` — start player for playlist
//! - `pl:pub:{pl_id}:{0|1}` — toggle public
//! - `pl:share:{pl_id}` — show share link
//! - `pl:add:{pl_id}` — add submenu
//! - `pl:addf:{pl_id}:{src}` — add from source (y=yt, s=sc, h=history)
//! - `pl:rm:{pl_id}:{item_id}` — remove track
//! - `pl:mv:{pl_id}:{item_id}:{d}` — move track (d=u|d)
//! - `pl:imp:{pl_id}` — import from URL (enters session)
//! - `pl:clone:{token}` — clone shared playlist

use crate::core::types::Plan;
use crate::download::search::format_duration;
use crate::storage::db::{self, DbPool};
use crate::storage::SharedStorage;
use crate::telegram::Bot;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{CallbackQueryId, ChatId, InlineKeyboardButton, InlineKeyboardMarkup, MessageId};

const TRACKS_PER_PAGE: usize = 8;
const PLAYLISTS_PER_PAGE: usize = 5;
const SESSION_TTL_SECS: u64 = 300; // 5 minutes
const PLAYLIST_NAME_PROMPT_KIND: &str = "playlist_name";
const PLAYLIST_IMPORT_PROMPT_KIND: &str = "playlist_import_url";

/// Truncate a string to `max_chars` characters safely (no panic on multi-byte).
fn truncate_str(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

// ── Plan limits ───────────────────────────────────────────────────────────

fn max_playlists(plan: Plan) -> i64 {
    match plan {
        Plan::Free => 3,
        Plan::Premium => 10,
        Plan::Vip => 100,
    }
}

pub fn max_tracks_per_playlist(plan: Plan) -> i64 {
    match plan {
        Plan::Free => 50,
        Plan::Premium => 200,
        Plan::Vip => 1000,
    }
}

// ── Name input session ────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum NameAction {
    Create,
    Rename(i64),
}

#[derive(Debug, Clone)]
pub struct PlaylistNameSession {
    pub action: NameAction,
}

fn encode_playlist_name_session(session: &PlaylistNameSession) -> String {
    match session.action {
        NameAction::Create => "create".to_string(),
        NameAction::Rename(playlist_id) => format!("rename:{playlist_id}"),
    }
}

fn decode_playlist_name_session(payload: &str) -> Option<PlaylistNameSession> {
    if payload == "create" {
        Some(PlaylistNameSession {
            action: NameAction::Create,
        })
    } else {
        payload
            .strip_prefix("rename:")
            .and_then(|v| v.parse::<i64>().ok())
            .map(|playlist_id| PlaylistNameSession {
                action: NameAction::Rename(playlist_id),
            })
    }
}

pub async fn is_waiting_for_playlist_name(shared_storage: &Arc<SharedStorage>, user_id: i64) -> bool {
    get_playlist_name_session(shared_storage, user_id).await.is_some()
}

pub async fn get_playlist_name_session(
    shared_storage: &Arc<SharedStorage>,
    user_id: i64,
) -> Option<PlaylistNameSession> {
    let payload = shared_storage
        .get_prompt_session(user_id, PLAYLIST_NAME_PROMPT_KIND)
        .await
        .ok()
        .flatten()?;
    decode_playlist_name_session(&payload)
}

pub async fn set_playlist_name_session(
    shared_storage: &Arc<SharedStorage>,
    user_id: i64,
    session: PlaylistNameSession,
) {
    let _ = shared_storage
        .upsert_prompt_session(
            user_id,
            PLAYLIST_NAME_PROMPT_KIND,
            &encode_playlist_name_session(&session),
            SESSION_TTL_SECS as i64,
        )
        .await;
}

pub async fn clear_playlist_name_session(shared_storage: &Arc<SharedStorage>, user_id: i64) {
    let _ = shared_storage
        .delete_prompt_session(user_id, PLAYLIST_NAME_PROMPT_KIND)
        .await;
}

pub async fn is_waiting_for_import_url(shared_storage: &Arc<SharedStorage>, user_id: i64) -> bool {
    get_import_playlist_id(shared_storage, user_id).await.is_some()
}

pub async fn get_import_playlist_id(shared_storage: &Arc<SharedStorage>, user_id: i64) -> Option<i64> {
    shared_storage
        .get_prompt_session(user_id, PLAYLIST_IMPORT_PROMPT_KIND)
        .await
        .ok()
        .flatten()
        .and_then(|payload| payload.parse::<i64>().ok())
}

pub async fn set_import_url_session(shared_storage: &Arc<SharedStorage>, user_id: i64, playlist_id: i64) {
    let _ = shared_storage
        .upsert_prompt_session(
            user_id,
            PLAYLIST_IMPORT_PROMPT_KIND,
            &playlist_id.to_string(),
            SESSION_TTL_SECS as i64,
        )
        .await;
}

pub async fn clear_import_url_session(shared_storage: &Arc<SharedStorage>, user_id: i64) {
    let _ = shared_storage
        .delete_prompt_session(user_id, PLAYLIST_IMPORT_PROMPT_KIND)
        .await;
}

// ── Handle text input for playlist name ───────────────────────────────────

pub async fn handle_playlist_name_input(
    bot: &Bot,
    chat_id: ChatId,
    db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
    text: &str,
) {
    let session = match get_playlist_name_session(&shared_storage, chat_id.0).await {
        Some(s) => s,
        None => return,
    };
    clear_playlist_name_session(&shared_storage, chat_id.0).await;

    let text = text.trim();
    if text.eq_ignore_ascii_case("cancel") {
        let _ = bot.send_message(chat_id, "Cancelled.").await;
        return;
    }

    if text.is_empty() || text.len() > 100 {
        let _ = bot
            .send_message(chat_id, "Playlist name must be 1-100 characters.")
            .await;
        return;
    }

    match session.action {
        NameAction::Create => {
            // Check plan limits
            let plan = shared_storage
                .get_user(chat_id.0)
                .await
                .ok()
                .flatten()
                .map(|u| u.plan)
                .unwrap_or(Plan::Free);
            let count = shared_storage.count_user_playlists(chat_id.0).await.unwrap_or(0);
            if count >= max_playlists(plan) {
                let _ = bot
                    .send_message(
                        chat_id,
                        format!(
                            "Playlist limit reached ({}/{}). Upgrade your plan for more.",
                            count,
                            max_playlists(plan)
                        ),
                    )
                    .await;
                return;
            }

            match shared_storage.create_playlist(chat_id.0, text, None).await {
                Ok(id) => {
                    let _ = bot
                        .send_message(chat_id, format!("📋 Playlist \"{}\" created!", text))
                        .await;
                    // Show the new playlist
                    let _ = show_playlist_view(bot, chat_id, id, 0, &db_pool, &shared_storage).await;
                }
                Err(e) => {
                    log::error!("Failed to create playlist: {}", e);
                    let _ = bot.send_message(chat_id, "Failed to create playlist.").await;
                }
            }
        }
        NameAction::Rename(pl_id) => {
            if let Err(e) = shared_storage.rename_playlist(pl_id, text).await {
                log::error!("Failed to rename playlist: {}", e);
                let _ = bot.send_message(chat_id, "Failed to rename playlist.").await;
            } else {
                let _ = bot
                    .send_message(chat_id, format!("✏️ Playlist renamed to \"{}\"", text))
                    .await;
                let _ = show_playlist_view(bot, chat_id, pl_id, 0, &db_pool, &shared_storage).await;
            }
        }
    }
}

// ── /playlists command ────────────────────────────────────────────────────

pub async fn handle_playlists_command(
    bot: &Bot,
    chat_id: ChatId,
    db_pool: &Arc<DbPool>,
    shared_storage: &Arc<SharedStorage>,
) {
    let _ = show_playlists_list(bot, chat_id, 0, db_pool, shared_storage).await;
}

async fn show_playlists_list(
    bot: &Bot,
    chat_id: ChatId,
    page: usize,
    db_pool: &Arc<DbPool>,
    shared_storage: &Arc<SharedStorage>,
) -> Result<(), teloxide::RequestError> {
    let _ = db_pool;
    let playlists = shared_storage.get_user_playlists(chat_id.0).await.unwrap_or_default();
    let plan = shared_storage
        .get_user(chat_id.0)
        .await
        .ok()
        .flatten()
        .map(|u| u.plan)
        .unwrap_or(Plan::Free);

    if playlists.is_empty() {
        let rows = vec![vec![InlineKeyboardButton::callback(
            "+ New Playlist",
            "pl:new".to_string(),
        )]];
        let keyboard = InlineKeyboardMarkup::new(rows);
        let _ = bot
            .send_message(chat_id, "📋 No playlists yet.")
            .reply_markup(keyboard)
            .await;
        return Ok(());
    }

    let total = playlists.len();
    let start = page * PLAYLISTS_PER_PAGE;
    let page_playlists = &playlists[start..total.min(start + PLAYLISTS_PER_PAGE)];

    let mut text = format!("📋 My Playlists ({}/{})\n\n", total, max_playlists(plan));

    let mut rows: Vec<Vec<InlineKeyboardButton>> = Vec::new();

    for (i, pl) in page_playlists.iter().enumerate() {
        let count = shared_storage.count_playlist_items(pl.id).await.unwrap_or(0);
        let public_icon = if pl.is_public { " 🌍" } else { "" };
        text.push_str(&format!(
            "{}. {} — {} tracks{}\n",
            start + i + 1,
            pl.name,
            count,
            public_icon
        ));
        rows.push(vec![
            InlineKeyboardButton::callback(format!("📂 {}", pl.name), format!("pl:view:{}:0", pl.id)),
            InlineKeyboardButton::callback("▶", format!("pw:play:{}", pl.id)),
        ]);
    }

    // Navigation
    let mut nav_row: Vec<InlineKeyboardButton> = Vec::new();
    if page > 0 {
        nav_row.push(InlineKeyboardButton::callback("◀", format!("pl:list:{}", page - 1)));
    }
    nav_row.push(InlineKeyboardButton::callback("+ New Playlist", "pl:new".to_string()));
    if start + PLAYLISTS_PER_PAGE < total {
        nav_row.push(InlineKeyboardButton::callback("▶", format!("pl:list:{}", page + 1)));
    }
    rows.push(nav_row);

    let keyboard = InlineKeyboardMarkup::new(rows);
    bot.send_message(chat_id, text).reply_markup(keyboard).await?;
    Ok(())
}

// ── Playlist detail view ──────────────────────────────────────────────────

async fn show_playlist_view(
    bot: &Bot,
    chat_id: ChatId,
    playlist_id: i64,
    page: usize,
    db_pool: &Arc<DbPool>,
    shared_storage: &Arc<SharedStorage>,
) -> Result<(), teloxide::RequestError> {
    let _ = db_pool;
    let pl = match shared_storage.get_playlist(playlist_id).await {
        Ok(Some(pl)) => pl,
        _ => {
            let _ = bot.send_message(chat_id, "Playlist not found.").await;
            return Ok(());
        }
    };

    let total_items = shared_storage.count_playlist_items(playlist_id).await.unwrap_or(0);
    let offset = (page * TRACKS_PER_PAGE) as i64;
    let items = shared_storage
        .get_playlist_items_page(playlist_id, offset, TRACKS_PER_PAGE as i64)
        .await
        .unwrap_or_default();

    let public_icon = if pl.is_public { " 🌍" } else { "" };
    let mut text = format!("📋 {}{} ({} tracks)\n\n", pl.name, public_icon, total_items);

    if items.is_empty() {
        text.push_str("No tracks yet. Add some!\n");
    } else {
        for item in &items {
            let dur = format_duration(item.duration_secs.map(|d| d as u32));
            let artist = item.artist.as_deref().unwrap_or("");
            if artist.is_empty() {
                text.push_str(&format!("{}. {} ({})\n", item.position + 1, item.title, dur));
            } else {
                text.push_str(&format!(
                    "{}. {} - {} ({})\n",
                    item.position + 1,
                    artist,
                    item.title,
                    dur
                ));
            }
        }
    }

    let mut rows: Vec<Vec<InlineKeyboardButton>> = Vec::new();

    // Track management buttons (first few tracks only)
    for item in items.iter().take(5) {
        rows.push(vec![
            InlineKeyboardButton::callback("🗑", format!("pl:rm:{}:{}", playlist_id, item.id)),
            InlineKeyboardButton::callback("⬆", format!("pl:mv:{}:{}:u", playlist_id, item.id)),
            InlineKeyboardButton::callback("⬇", format!("pl:mv:{}:{}:d", playlist_id, item.id)),
        ]);
    }

    // Action buttons
    rows.push(vec![
        InlineKeyboardButton::callback("➕ Add", format!("pl:add:{}", playlist_id)),
        InlineKeyboardButton::callback("▶ Play", format!("pw:play:{}", playlist_id)),
    ]);

    let mut bottom_row = vec![
        InlineKeyboardButton::callback("✏️ Rename", format!("pl:ren:{}", playlist_id)),
        InlineKeyboardButton::callback("🗑 Delete", format!("pl:del:{}", playlist_id)),
    ];
    if pl.is_public {
        bottom_row.push(InlineKeyboardButton::callback(
            "🔒 Private",
            format!("pl:pub:{}:0", playlist_id),
        ));
    } else {
        bottom_row.push(InlineKeyboardButton::callback(
            "🌍 Public",
            format!("pl:pub:{}:1", playlist_id),
        ));
    }
    rows.push(bottom_row);

    // Share + Import
    rows.push(vec![
        InlineKeyboardButton::callback("🔗 Share", format!("pl:share:{}", playlist_id)),
        InlineKeyboardButton::callback("📥 Import URL", format!("pl:imp:{}", playlist_id)),
    ]);

    // Pagination
    let total_pages = (total_items as usize).div_ceil(TRACKS_PER_PAGE).max(1);
    if total_pages > 1 {
        let mut nav_row: Vec<InlineKeyboardButton> = Vec::new();
        if page > 0 {
            nav_row.push(InlineKeyboardButton::callback(
                "◀",
                format!("pl:view:{}:{}", playlist_id, page - 1),
            ));
        }
        nav_row.push(InlineKeyboardButton::callback("📋 Back", "pl:list:0".to_string()));
        if page + 1 < total_pages {
            nav_row.push(InlineKeyboardButton::callback(
                "▶",
                format!("pl:view:{}:{}", playlist_id, page + 1),
            ));
        }
        rows.push(nav_row);
    } else {
        rows.push(vec![InlineKeyboardButton::callback("📋 Back", "pl:list:0".to_string())]);
    }

    let keyboard = InlineKeyboardMarkup::new(rows);
    bot.send_message(chat_id, text).reply_markup(keyboard).await?;
    Ok(())
}

// ── Add submenu ───────────────────────────────────────────────────────────

async fn show_add_menu(bot: &Bot, chat_id: ChatId, playlist_id: i64) -> Result<(), teloxide::RequestError> {
    let rows = vec![
        vec![
            InlineKeyboardButton::callback("🔍 YouTube", format!("pl:addf:{}:y", playlist_id)),
            InlineKeyboardButton::callback("🔍 SoundCloud", format!("pl:addf:{}:s", playlist_id)),
        ],
        vec![InlineKeyboardButton::callback(
            "📜 From History",
            format!("pl:addf:{}:h", playlist_id),
        )],
        vec![InlineKeyboardButton::callback(
            "◀ Back",
            format!("pl:view:{}:0", playlist_id),
        )],
    ];
    let keyboard = InlineKeyboardMarkup::new(rows);
    bot.send_message(chat_id, "➕ Add tracks to playlist:")
        .reply_markup(keyboard)
        .await?;
    Ok(())
}

// ── Share token generation ────────────────────────────────────────────────

fn generate_share_token() -> String {
    use rand::Rng;
    const CHARSET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut rng = rand::thread_rng();
    (0..8)
        .map(|_| {
            let idx = rng.gen_range(0..CHARSET.len());
            CHARSET[idx] as char
        })
        .collect()
}

// ── Ownership check ──────────────────────────────────────────────────────

/// Verify the user owns the playlist. Returns the playlist if owned.
async fn verify_ownership(shared_storage: &Arc<SharedStorage>, playlist_id: i64, user_id: i64) -> Option<db::Playlist> {
    match shared_storage.get_playlist(playlist_id).await {
        Ok(Some(pl)) if pl.user_id == user_id => Some(pl),
        _ => None,
    }
}

// ── Callback handler ──────────────────────────────────────────────────────

pub async fn handle_playlist_callback(
    bot: &Bot,
    callback_id: CallbackQueryId,
    chat_id: ChatId,
    message_id: MessageId,
    data: &str,
    db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
) -> Result<(), teloxide::RequestError> {
    let _ = bot.answer_callback_query(callback_id).await;

    let conn = match db::get_connection(&db_pool) {
        Ok(c) => c,
        Err(_) => return Ok(()),
    };

    // pl:new — create
    if data == "pl:new" {
        set_playlist_name_session(
            &shared_storage,
            chat_id.0,
            PlaylistNameSession {
                action: NameAction::Create,
            },
        )
        .await;
        let _ = bot
            .send_message(chat_id, "📝 Enter a name for your new playlist (or type \"cancel\"):")
            .await;
        return Ok(());
    }

    // pl:list:{page}
    if let Some(page_str) = data.strip_prefix("pl:list:") {
        let page = page_str.parse::<usize>().unwrap_or(0);
        let _ = bot.delete_message(chat_id, message_id).await;
        let _ = show_playlists_list(bot, chat_id, page, &db_pool, &shared_storage).await;
        return Ok(());
    }

    // pl:view:{pl_id}:{page}
    if let Some(rest) = data.strip_prefix("pl:view:") {
        let parts: Vec<&str> = rest.splitn(2, ':').collect();
        if parts.len() == 2 {
            if let (Ok(pl_id), Ok(page)) = (parts[0].parse::<i64>(), parts[1].parse::<usize>()) {
                // Allow viewing own playlists or public playlists
                match shared_storage.get_playlist(pl_id).await {
                    Ok(Some(pl)) if pl.user_id == chat_id.0 || pl.is_public => {}
                    _ => return Ok(()),
                }
                let _ = bot.delete_message(chat_id, message_id).await;
                let _ = show_playlist_view(bot, chat_id, pl_id, page, &db_pool, &shared_storage).await;
            }
        }
        return Ok(());
    }

    // pl:ren:{pl_id}
    if let Some(pl_id_str) = data.strip_prefix("pl:ren:") {
        if let Ok(pl_id) = pl_id_str.parse::<i64>() {
            if verify_ownership(&shared_storage, pl_id, chat_id.0).await.is_none() {
                return Ok(());
            }
            set_playlist_name_session(
                &shared_storage,
                chat_id.0,
                PlaylistNameSession {
                    action: NameAction::Rename(pl_id),
                },
            )
            .await;
            let _ = bot
                .send_message(chat_id, "✏️ Enter new playlist name (or \"cancel\"):")
                .await;
        }
        return Ok(());
    }

    // pl:del:{pl_id} — confirm
    if let Some(pl_id_str) = data.strip_prefix("pl:del:") {
        if let Ok(pl_id) = pl_id_str.parse::<i64>() {
            let pl = match verify_ownership(&shared_storage, pl_id, chat_id.0).await {
                Some(p) => p,
                None => return Ok(()),
            };
            let pl_name = pl.name;
            let rows = vec![vec![
                InlineKeyboardButton::callback("✅ Yes, delete", format!("pl:delok:{}", pl_id)),
                InlineKeyboardButton::callback("❌ Cancel", format!("pl:view:{}:0", pl_id)),
            ]];
            let keyboard = InlineKeyboardMarkup::new(rows);
            let _ = bot
                .send_message(
                    chat_id,
                    format!("🗑 Delete playlist \"{}\"? This cannot be undone.", pl_name),
                )
                .reply_markup(keyboard)
                .await;
        }
        return Ok(());
    }

    // pl:delok:{pl_id}
    if let Some(pl_id_str) = data.strip_prefix("pl:delok:") {
        if let Ok(pl_id) = pl_id_str.parse::<i64>() {
            if verify_ownership(&shared_storage, pl_id, chat_id.0).await.is_none() {
                return Ok(());
            }
            let _ = shared_storage.delete_playlist(pl_id).await;
            let _ = bot.delete_message(chat_id, message_id).await;
            let _ = bot.send_message(chat_id, "🗑 Playlist deleted.").await;
            let _ = show_playlists_list(bot, chat_id, 0, &db_pool, &shared_storage).await;
        }
        return Ok(());
    }

    // pl:pub:{pl_id}:{0|1}
    if let Some(rest) = data.strip_prefix("pl:pub:") {
        let parts: Vec<&str> = rest.splitn(2, ':').collect();
        if parts.len() == 2 {
            if let (Ok(pl_id), Ok(val)) = (parts[0].parse::<i64>(), parts[1].parse::<i32>()) {
                if verify_ownership(&shared_storage, pl_id, chat_id.0).await.is_none() {
                    return Ok(());
                }
                let _ = shared_storage.set_playlist_public(pl_id, val != 0).await;
                let _ = bot.delete_message(chat_id, message_id).await;
                let _ = show_playlist_view(bot, chat_id, pl_id, 0, &db_pool, &shared_storage).await;
            }
        }
        return Ok(());
    }

    // pl:share:{pl_id}
    if let Some(pl_id_str) = data.strip_prefix("pl:share:") {
        if let Ok(pl_id) = pl_id_str.parse::<i64>() {
            let pl = match verify_ownership(&shared_storage, pl_id, chat_id.0).await {
                Some(p) => p,
                None => return Ok(()),
            };
            let token = if let Some(t) = pl.share_token {
                t
            } else {
                let t = generate_share_token();
                let _ = shared_storage.set_playlist_share_token(pl_id, &t).await;
                let _ = shared_storage.set_playlist_public(pl_id, true).await;
                t
            };

            // Get cached bot username for deep link
            let bot_username = crate::core::copyright::get_bot_username().unwrap_or("bot");
            let link = format!("https://t.me/{}?start=pl_{}", bot_username, token);
            let count = shared_storage.count_playlist_items(pl_id).await.unwrap_or(0);

            let text = format!(
                "🔗 Share Link\n━━━━━━━━━━━━━━\n{}\n{} tracks\n\n{}",
                pl.name, count, link
            );
            let _ = bot.send_message(chat_id, text).await;
        }
        return Ok(());
    }

    // pl:add:{pl_id}
    if let Some(pl_id_str) = data.strip_prefix("pl:add:") {
        if let Ok(pl_id) = pl_id_str.parse::<i64>() {
            if verify_ownership(&shared_storage, pl_id, chat_id.0).await.is_none() {
                return Ok(());
            }
            let _ = bot.delete_message(chat_id, message_id).await;
            let _ = show_add_menu(bot, chat_id, pl_id).await;
        }
        return Ok(());
    }

    // pl:addf:{pl_id}:{src}
    if let Some(rest) = data.strip_prefix("pl:addf:") {
        let parts: Vec<&str> = rest.splitn(2, ':').collect();
        if parts.len() == 2 {
            if let Ok(pl_id) = parts[0].parse::<i64>() {
                if verify_ownership(&shared_storage, pl_id, chat_id.0).await.is_none() {
                    return Ok(());
                }
                let _ = bot.delete_message(chat_id, message_id).await;
                match parts[1] {
                    "y" | "s" => {
                        // Prompt for search — will be handled by search module
                        let _ = bot.send_message(chat_id, "🔍 Type your search query:").await;
                        // Set search context
                        use super::search::{set_search_session, SearchContext, SearchSession};
                        // We store the context for future search handling
                        let _ = set_search_session(
                            &shared_storage,
                            chat_id.0,
                            &SearchSession {
                                query: String::new(),
                                results: vec![],
                                source: if parts[1] == "y" {
                                    crate::download::search::SearchSource::YouTube
                                } else {
                                    crate::download::search::SearchSource::SoundCloud
                                },
                                context: SearchContext::AddToPlaylist { playlist_id: pl_id },
                            },
                        )
                        .await;
                    }
                    "h" => {
                        // Show download history for adding
                        let _ = show_history_for_add(bot, chat_id, pl_id, 0, &db_pool).await;
                    }
                    _ => {}
                }
            }
        }
        return Ok(());
    }

    // pl:rm:{pl_id}:{item_id}
    if let Some(rest) = data.strip_prefix("pl:rm:") {
        let parts: Vec<&str> = rest.splitn(2, ':').collect();
        if parts.len() == 2 {
            if let (Ok(pl_id), Ok(item_id)) = (parts[0].parse::<i64>(), parts[1].parse::<i64>()) {
                if verify_ownership(&shared_storage, pl_id, chat_id.0).await.is_none() {
                    return Ok(());
                }
                let _ = shared_storage.remove_playlist_item(item_id).await;
                let _ = bot.delete_message(chat_id, message_id).await;
                let _ = show_playlist_view(bot, chat_id, pl_id, 0, &db_pool, &shared_storage).await;
            }
        }
        return Ok(());
    }

    // pl:mv:{pl_id}:{item_id}:{d}
    if let Some(rest) = data.strip_prefix("pl:mv:") {
        let parts: Vec<&str> = rest.splitn(3, ':').collect();
        if parts.len() == 3 {
            if let (Ok(pl_id), Ok(item_id)) = (parts[0].parse::<i64>(), parts[1].parse::<i64>()) {
                if verify_ownership(&shared_storage, pl_id, chat_id.0).await.is_none() {
                    return Ok(());
                }
                let direction = match parts[2] {
                    "u" => -1,
                    "d" => 1,
                    _ => 0,
                };
                if direction != 0 {
                    let _ = shared_storage.reorder_playlist_item(item_id, direction).await;
                }
                let _ = bot.delete_message(chat_id, message_id).await;
                let _ = show_playlist_view(bot, chat_id, pl_id, 0, &db_pool, &shared_storage).await;
            }
        }
        return Ok(());
    }

    // pl:imp:{pl_id}
    if let Some(pl_id_str) = data.strip_prefix("pl:imp:") {
        if let Ok(pl_id) = pl_id_str.parse::<i64>() {
            if verify_ownership(&shared_storage, pl_id, chat_id.0).await.is_none() {
                return Ok(());
            }
            set_import_url_session(&shared_storage, chat_id.0, pl_id).await;
            let _ = bot
                .send_message(
                    chat_id,
                    "📥 Send a playlist URL (YouTube or Spotify) or type \"cancel\":",
                )
                .await;
        }
        return Ok(());
    }

    // pl:hadd:{pl_id}:{entry_id}:{page} — add from download history
    if let Some(rest) = data.strip_prefix("pl:hadd:") {
        let parts: Vec<&str> = rest.splitn(3, ':').collect();
        if parts.len() >= 2 {
            if let (Ok(pl_id), Ok(entry_id)) = (parts[0].parse::<i64>(), parts[1].parse::<i64>()) {
                if verify_ownership(&shared_storage, pl_id, chat_id.0).await.is_none() {
                    return Ok(());
                }
                if let Ok(Some(entry)) = db::get_download_history_entry(&conn, chat_id.0, entry_id) {
                    let source = crate::download::search::source_name_from_url(&entry.url);
                    let _ = shared_storage
                        .add_playlist_item(
                            pl_id,
                            &entry.title,
                            entry.author.as_deref(),
                            &entry.url,
                            entry.duration.map(|d| d as i32),
                            entry.file_id.as_deref(),
                            source,
                        )
                        .await;
                    let _ = bot
                        .send_message(chat_id, format!("➕ Added \"{}\" to playlist", entry.title))
                        .await;
                }
            }
        }
        return Ok(());
    }

    // pl:clone:{token}
    if let Some(token) = data.strip_prefix("pl:clone:") {
        handle_clone_playlist(bot, chat_id, token, &db_pool, &shared_storage).await;
        return Ok(());
    }

    Ok(())
}

// ── Clone shared playlist ─────────────────────────────────────────────────

pub async fn handle_clone_playlist(
    bot: &Bot,
    chat_id: ChatId,
    token: &str,
    db_pool: &Arc<DbPool>,
    shared_storage: &Arc<SharedStorage>,
) {
    let _ = db_pool;

    let source_pl = match shared_storage.get_playlist_by_share_token(token).await {
        Ok(Some(pl)) => pl,
        _ => {
            let _ = bot.send_message(chat_id, "Playlist not found or link expired.").await;
            return;
        }
    };

    // Check plan limits
    let plan = shared_storage
        .get_user(chat_id.0)
        .await
        .ok()
        .flatten()
        .map(|u| u.plan)
        .unwrap_or(Plan::Free);
    let count = shared_storage.count_user_playlists(chat_id.0).await.unwrap_or(0);
    if count >= max_playlists(plan) {
        let _ = bot
            .send_message(chat_id, "Playlist limit reached. Upgrade your plan for more.")
            .await;
        return;
    }

    // Create new playlist
    let new_name = format!("{} (copy)", source_pl.name);
    let new_pl_id = match shared_storage
        .create_playlist(chat_id.0, &new_name, source_pl.description.as_deref())
        .await
    {
        Ok(id) => id,
        Err(e) => {
            log::error!("Failed to clone playlist: {}", e);
            let _ = bot.send_message(chat_id, "Failed to clone playlist.").await;
            return;
        }
    };

    // Copy items in a transaction
    let items = shared_storage
        .get_playlist_items(source_pl.id)
        .await
        .unwrap_or_default();
    for item in &items {
        let _ = shared_storage
            .add_playlist_item(
                new_pl_id,
                &item.title,
                item.artist.as_deref(),
                &item.url,
                item.duration_secs,
                item.file_id.as_deref(),
                &item.source,
            )
            .await;
    }

    let _ = bot
        .send_message(
            chat_id,
            format!("📥 Cloned \"{}\" with {} tracks!", source_pl.name, items.len()),
        )
        .await;
}

// ── History for add ───────────────────────────────────────────────────────

async fn show_history_for_add(
    bot: &Bot,
    chat_id: ChatId,
    playlist_id: i64,
    page: usize,
    db_pool: &Arc<DbPool>,
) -> Result<(), teloxide::RequestError> {
    let conn = match db::get_connection(db_pool) {
        Ok(c) => c,
        Err(_) => return Ok(()),
    };

    // Get recent downloads
    let history = db::get_download_history(&conn, chat_id.0, Some(10)).unwrap_or_default();

    if history.is_empty() {
        let _ = bot.send_message(chat_id, "No download history.").await;
        return Ok(());
    }

    let mut text = String::from("📜 Recent Downloads\n\n");
    let mut rows: Vec<Vec<InlineKeyboardButton>> = Vec::new();

    for (i, entry) in history.iter().enumerate() {
        text.push_str(&format!("{}. {}\n", page * 10 + i + 1, entry.title));
        rows.push(vec![InlineKeyboardButton::callback(
            format!("➕ {}", truncate_str(&entry.title, 30)),
            format!("pl:hadd:{}:{}:{}", playlist_id, entry.id, page),
        )]);
    }

    rows.push(vec![InlineKeyboardButton::callback(
        "◀ Back",
        format!("pl:view:{}:0", playlist_id),
    )]);

    let keyboard = InlineKeyboardMarkup::new(rows);
    bot.send_message(chat_id, text).reply_markup(keyboard).await?;
    Ok(())
}
