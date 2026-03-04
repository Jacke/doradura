//! Playlist import from external URLs (YouTube, Spotify) using yt-dlp.

use crate::download::search::{append_proxy_args, source_name_from_url, YtdlpFlatEntry};
use crate::storage::db::{self, DbPool};
use crate::telegram::Bot;
use std::sync::Arc;
use std::time::Duration;
use teloxide::prelude::*;
use teloxide::types::ChatId;
use tokio::process::Command as TokioCommand;
use tokio::time::timeout;

const IMPORT_TIMEOUT_SECS: u64 = 120;

/// Handle a URL import into a playlist.
pub async fn handle_import_url(bot: &Bot, chat_id: ChatId, url: &str, playlist_id: i64, db_pool: Arc<DbPool>) {
    let status_msg = bot.send_message(chat_id, "📥 Importing playlist...").await;

    let ytdl_bin = &*crate::core::config::YTDL_BIN;
    let mut args: Vec<String> = vec![
        "--flat-playlist".to_string(),
        "--dump-json".to_string(),
        "--no-warnings".to_string(),
        "--no-check-certificate".to_string(),
    ];

    append_proxy_args(&mut args);

    args.push(url.to_string());

    log::info!("Importing playlist from: {}", url);

    let output = match timeout(
        Duration::from_secs(IMPORT_TIMEOUT_SECS),
        TokioCommand::new(ytdl_bin).args(&args).output(),
    )
    .await
    {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => {
            let _ = bot.send_message(chat_id, format!("Import failed: {}", e)).await;
            return;
        }
        Err(_) => {
            let _ = bot.send_message(chat_id, "Import timed out.").await;
            return;
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        log::warn!("yt-dlp import failed: {}", stderr);
        let _ = bot.send_message(chat_id, "Import failed. Check the URL.").await;
        return;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let entries: Vec<YtdlpFlatEntry> = stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();

    if entries.is_empty() {
        let _ = bot.send_message(chat_id, "No tracks found in this URL.").await;
        return;
    }

    let conn = match db::get_connection(&db_pool) {
        Ok(c) => c,
        Err(_) => {
            let _ = bot.send_message(chat_id, "Database error").await;
            return;
        }
    };

    // Check plan limits
    let plan = db::get_user(&conn, chat_id.0)
        .ok()
        .flatten()
        .map(|u| u.plan)
        .unwrap_or(crate::core::types::Plan::Free);
    let max_tracks = crate::telegram::menu::playlist::max_tracks_per_playlist(plan);
    let current_count = db::count_playlist_items(&conn, playlist_id).unwrap_or(0);
    let available = (max_tracks - current_count).max(0) as usize;

    let to_import = entries.len().min(available);

    let mut added = 0;
    // Wrap in a transaction for atomic bulk insert
    let _ = conn.execute_batch("BEGIN");
    for entry in entries.iter().take(to_import) {
        let title = entry.title.as_deref().unwrap_or("Unknown");
        if title.is_empty() {
            continue;
        }
        let url = entry.webpage_url.as_deref().or(entry.url.as_deref()).unwrap_or("");
        if url.is_empty() {
            continue;
        }

        let source = source_name_from_url(url);

        if db::add_playlist_item(
            &conn,
            playlist_id,
            title,
            entry.artist(),
            url,
            entry.duration.map(|d| d as i32),
            None,
            source,
        )
        .is_ok()
        {
            added += 1;
        }
    }
    let _ = conn.execute_batch("COMMIT");

    // Delete status message
    if let Ok(msg) = &status_msg {
        let _ = bot.delete_message(chat_id, msg.id).await;
    }

    let mut text = format!("📥 Imported {} tracks!", added);
    if to_import < entries.len() {
        text.push_str(&format!(
            "\n⚠️ {} tracks skipped (plan limit: {} tracks)",
            entries.len() - to_import,
            max_tracks
        ));
    }

    let _ = bot.send_message(chat_id, text).await;
}

/// Check if a URL looks like a playlist that should be imported.
pub fn is_playlist_url(url: &str) -> bool {
    url.contains("open.spotify.com/playlist/")
        || url.contains("youtube.com/playlist?list=")
        || url.contains("music.youtube.com/playlist?list=")
}
