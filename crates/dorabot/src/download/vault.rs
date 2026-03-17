//! Vault cache: check for cached file_ids and send downloads to user's vault channel.

use crate::storage::SharedStorage;
use crate::telegram::Bot;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{ChatId, FileId, InputFile};

/// Check vault cache for a cached file_id. Returns `Some(file_id)` if found.
pub async fn check_vault_cache(shared_storage: &Arc<SharedStorage>, user_id: i64, url: &str) -> Option<String> {
    let vault = shared_storage.get_user_vault(user_id).await.ok()??;
    if !vault.is_active {
        return None;
    }
    shared_storage
        .get_vault_cached_file_id(user_id, url)
        .await
        .ok()
        .flatten()
}

/// Fire-and-forget: send audio to user's vault channel and save cache entry.
pub fn send_to_vault_background(
    bot: Bot,
    shared_storage: Arc<SharedStorage>,
    user_id: i64,
    url: String,
    file_id: String,
    title: Option<String>,
    artist: Option<String>,
    duration_secs: Option<i32>,
    file_size: Option<i64>,
) {
    tokio::spawn(async move {
        let vault = match shared_storage.get_user_vault(user_id).await {
            Ok(Some(v)) if v.is_active => v,
            _ => return,
        };

        // Already cached? Skip sending again.
        if shared_storage
            .get_vault_cached_file_id(user_id, &url)
            .await
            .ok()
            .flatten()
            .is_some()
        {
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
                let _ = shared_storage
                    .save_vault_cache_entry(
                        user_id,
                        &url,
                        title.as_deref(),
                        artist.as_deref(),
                        duration_secs,
                        &file_id,
                        Some(msg_id),
                        file_size,
                    )
                    .await;
            }
            Err(e) => {
                let err_str = e.to_string();
                if err_str.contains("Forbidden")
                    || err_str.contains("chat not found")
                    || err_str.contains("not enough rights")
                {
                    log::warn!("Vault send failed (deactivating): {}", err_str);
                    let _ = shared_storage.deactivate_user_vault(user_id).await;
                } else {
                    log::warn!("Vault send failed: {}", err_str);
                }
            }
        }
    });
}
