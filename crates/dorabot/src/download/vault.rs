//! Vault cache: check for cached file_ids and send downloads to user's vault channel.

use crate::storage::db::{self, DbPool};
use crate::telegram::Bot;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{ChatId, FileId, InputFile};

/// Check vault cache for a cached file_id. Returns `Some(file_id)` if found.
pub fn check_vault_cache(db_pool: &Arc<DbPool>, user_id: i64, url: &str) -> Option<String> {
    let conn = db::get_connection(db_pool).ok()?;
    let vault = db::get_user_vault(&conn, user_id).ok()??;
    if !vault.is_active {
        return None;
    }
    db::get_vault_cached_file_id(&conn, user_id, url)
}

/// Fire-and-forget: send audio to user's vault channel and save cache entry.
pub fn send_to_vault_background(
    bot: Bot,
    db_pool: Arc<DbPool>,
    user_id: i64,
    url: String,
    file_id: String,
    title: Option<String>,
    artist: Option<String>,
    duration_secs: Option<i32>,
    file_size: Option<i64>,
) {
    tokio::spawn(async move {
        let conn = match db::get_connection(&db_pool) {
            Ok(c) => c,
            Err(_) => return,
        };
        let vault = match db::get_user_vault(&conn, user_id) {
            Ok(Some(v)) if v.is_active => v,
            _ => return,
        };

        // Already cached? Skip sending again.
        if db::get_vault_cached_file_id(&conn, user_id, &url).is_some() {
            return;
        }

        let caption = match (&artist, &title) {
            (Some(a), Some(t)) if !a.is_empty() => format!("{} \u{2014} {}", a, t),
            (_, Some(t)) => t.clone(),
            _ => String::new(),
        };

        let channel_id = ChatId(vault.channel_id);
        let input = InputFile::file_id(FileId(file_id.clone()));
        let result = if caption.is_empty() {
            bot.send_audio(channel_id, input).await
        } else {
            bot.send_audio(channel_id, input).caption(&caption).await
        };

        match result {
            Ok(msg) => {
                let msg_id = msg.id.0 as i64;
                let _ = db::save_vault_cache_entry(
                    &conn,
                    user_id,
                    &url,
                    title.as_deref(),
                    artist.as_deref(),
                    duration_secs,
                    &file_id,
                    Some(msg_id),
                    file_size,
                );
            }
            Err(e) => {
                let err_str = e.to_string();
                if err_str.contains("Forbidden")
                    || err_str.contains("chat not found")
                    || err_str.contains("not enough rights")
                {
                    log::warn!("Vault send failed (deactivating): {}", err_str);
                    let _ = db::deactivate_user_vault(&conn, user_id);
                } else {
                    log::warn!("Vault send failed: {}", err_str);
                }
            }
        }
    });
}
