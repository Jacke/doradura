//! Vlipsy custom preview with repeat toggle.
//!
//! Short video clips (3-10s) get a specialised preview instead of yt-dlp metadata.
//! Buttons: Video | Circle | MP3 | Repeat toggle (1x-4x) | Cancel.
//!
//! Callback data prefixes:
//! - `vp:video:{repeat}:{url_id}` — download as video
//! - `vp:circle:{repeat}:{url_id}` — download as video_note (circle)
//! - `vp:mp3:{repeat}:{url_id}` — download as MP3
//! - `vp:rep:{current_repeat}:{url_id}` — cycle repeat 1→2→3→4→1

use crate::core::config;
use crate::core::escape_markdown;
use crate::download::source::vlipsy::scrape_clip_page;
use crate::storage::cache;
use crate::storage::db::DbPool;
use crate::storage::SharedStorage;
use crate::telegram::Bot;
use reqwest::Client;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardMarkup, InputFile, MessageId, ParseMode};
use url::Url;

/// Build the Vlipsy preview keyboard.
///
/// Layout:
/// ```text
/// [📹 Video]  [⭕ Circle]  [🎵 MP3]
/// [🔁 {repeat}x]
/// [❌ Cancel]
/// ```
fn build_vlipsy_keyboard(url_id: &str, repeat: u8) -> InlineKeyboardMarkup {
    let row1 = vec![
        crate::telegram::cb("📹 Video", format!("vp:video:{}:{}", repeat, url_id)),
        crate::telegram::cb("⭕ Circle", format!("vp:circle:{}:{}", repeat, url_id)),
        crate::telegram::cb("🎵 MP3", format!("vp:mp3:{}:{}", repeat, url_id)),
    ];
    let row2 = vec![crate::telegram::cb(
        format!("🔁 {}x", repeat),
        format!("vp:rep:{}:{}", repeat, url_id),
    )];
    let row3 = vec![crate::telegram::cb("❌ Cancel", format!("pv:cancel:{}", url_id))];
    InlineKeyboardMarkup::new(vec![row1, row2, row3])
}

/// Send a custom Vlipsy preview: title, duration, thumbnail + buttons.
pub async fn send_vlipsy_preview(
    bot: &Bot,
    chat_id: ChatId,
    url: &Url,
    url_id: &str,
    processing_msg_id: MessageId,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let http = Client::builder()
        .user_agent("doradura/0.14")
        .timeout(std::time::Duration::from_secs(15))
        .build()?;

    let info = scrape_clip_page(&http, url).await?;

    // Build preview text (MarkdownV2)
    let escaped_title = escape_markdown(&info.title);
    let mut text = format!("🎬 *{}*\n", escaped_title);
    if let Some(dur) = info.duration_secs {
        text.push_str(&format!("⏱️ Duration: {}s\n", dur));
    }

    let keyboard = build_vlipsy_keyboard(url_id, 1);

    // Delete "processing..." message
    let _ = bot.delete_message(chat_id, processing_msg_id).await;

    // Try to send with thumbnail
    if let Some(thumb_url) = &info.thumbnail_url {
        if let Ok(response) = reqwest::get(thumb_url).await {
            if response.status().is_success() {
                if let Ok(bytes) = response.bytes().await {
                    let bytes_vec = bytes.to_vec();
                    let result = bot
                        .send_photo(chat_id, InputFile::memory(bytes_vec))
                        .caption(&text)
                        .parse_mode(ParseMode::MarkdownV2)
                        .reply_markup(keyboard.clone())
                        .await;
                    if result.is_ok() {
                        return Ok(());
                    }
                }
            }
        }
    }

    // Fallback: text-only preview
    bot.send_message(chat_id, &text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(keyboard)
        .await?;

    Ok(())
}

/// Handle `vp:` callbacks from the Vlipsy preview keyboard.
pub async fn handle_vlipsy_callback(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    data: &str,
    db_pool: &Arc<DbPool>,
    shared_storage: &Arc<SharedStorage>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Parse: "vp:{action}:{param}:{url_id}"
    let parts: Vec<&str> = data.splitn(4, ':').collect();
    if parts.len() < 4 {
        log::warn!("Invalid vp callback: {}", data);
        return Ok(());
    }
    let action = parts[1];
    let param = parts[2];
    let url_id = parts[3];

    match action {
        "rep" => handle_repeat_toggle(bot, chat_id, message_id, param, url_id).await,
        "video" | "circle" | "mp3" => {
            let repeat: u8 = param.parse().unwrap_or(1);
            handle_download(
                bot,
                chat_id,
                message_id,
                action,
                repeat,
                url_id,
                db_pool,
                shared_storage,
            )
            .await
        }
        _ => {
            log::warn!("Unknown vp action: {}", action);
            Ok(())
        }
    }
}

/// Cycle repeat: 1→2→3→4→1, edit the keyboard.
async fn handle_repeat_toggle(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    current: &str,
    url_id: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let current_repeat: u8 = current.parse().unwrap_or(1);
    let next_repeat = match current_repeat {
        1 => 2,
        2 => 3,
        3 => 4,
        _ => 1,
    };
    let keyboard = build_vlipsy_keyboard(url_id, next_repeat);
    // Try editing caption (photo message) first, fall back to text message
    let caption_result = bot
        .edit_message_reply_markup(chat_id, message_id)
        .reply_markup(keyboard)
        .await;
    if let Err(e) = caption_result {
        log::warn!("Failed to edit vlipsy keyboard: {:?}", e);
    }
    Ok(())
}

/// Download the clip, apply repeat + format conversion, send to user.
async fn handle_download(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    action: &str,
    repeat: u8,
    url_id: &str,
    db_pool: &Arc<DbPool>,
    shared_storage: &Arc<SharedStorage>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Resolve URL from cache
    let url_str = match cache::get_url(db_pool, Some(shared_storage.as_ref()), url_id).await {
        Some(u) => u,
        None => {
            log::warn!("Vlipsy URL expired for url_id={}", url_id);
            let _ = bot.send_message(chat_id, "Link expired, please send it again").await;
            return Ok(());
        }
    };

    let url = Url::parse(&url_str)?;

    // Delete preview message
    let _ = bot.delete_message(chat_id, message_id).await;

    // Send status
    let status_msg = bot.send_message(chat_id, "⏳ Processing...").await?;

    let bot_clone = bot.clone();
    let action = action.to_string();
    let status_msg_id = status_msg.id;

    tokio::spawn(async move {
        let result = download_process_send(&bot_clone, chat_id, &url, &action, repeat).await;

        // Delete status message
        let _ = bot_clone.delete_message(chat_id, status_msg_id).await;

        if let Err(e) = result {
            log::error!("Vlipsy download failed: {:?}", e);
            let _ = bot_clone
                .send_message(chat_id, "❌ Download failed, please try again")
                .await;
        }
    });

    Ok(())
}

/// Core: download MP4 → optional repeat → convert → send.
async fn download_process_send(
    bot: &Bot,
    chat_id: ChatId,
    url: &Url,
    action: &str,
    repeat: u8,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let http = Client::builder()
        .user_agent("doradura/0.14")
        .timeout(std::time::Duration::from_secs(120))
        .build()?;

    let info = scrape_clip_page(&http, url).await?;

    // Download MP4
    let download_folder = shellexpand::tilde(&*config::DOWNLOAD_FOLDER).into_owned();
    let session_id = uuid::Uuid::new_v4();
    let input_path = format!("{}/vlipsy_{}.mp4", download_folder, session_id);

    // Ensure directory exists
    if let Some(parent) = std::path::Path::new(&input_path).parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let resp = http.get(&info.mp4_url).send().await?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {} downloading MP4", resp.status()).into());
    }
    let bytes = resp.bytes().await?;
    tokio::fs::write(&input_path, &bytes).await?;

    log::info!(
        "Vlipsy downloaded {} ({:.1} KB) for {}",
        input_path,
        bytes.len() as f64 / 1024.0,
        action
    );

    // Apply repeat if > 1
    let repeated_path = if repeat > 1 {
        let out = format!("{}/vlipsy_{}_repeat.mp4", download_folder, session_id);
        let loop_count = (repeat - 1).to_string();
        let status = tokio::process::Command::new("ffmpeg")
            .args(["-stream_loop", &loop_count, "-i", &input_path, "-c", "copy", "-y", &out])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await?;
        if !status.success() {
            log::error!("ffmpeg stream_loop failed");
            return Err("ffmpeg repeat failed".into());
        }
        out
    } else {
        input_path.clone()
    };

    let result = match action {
        "video" => send_as_video(bot, chat_id, &repeated_path, &info.title).await,
        "circle" => send_as_circle(bot, chat_id, &repeated_path, &download_folder, &session_id).await,
        "mp3" => send_as_mp3(bot, chat_id, &repeated_path, &info.title, &download_folder, &session_id).await,
        _ => Err("Unknown action".into()),
    };

    // Cleanup temp files
    let _ = tokio::fs::remove_file(&input_path).await;
    if repeat > 1 {
        let _ = tokio::fs::remove_file(&repeated_path).await;
    }

    result
}

/// Send file as a regular Telegram video.
async fn send_as_video(
    bot: &Bot,
    chat_id: ChatId,
    path: &str,
    title: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    bot.send_video(chat_id, InputFile::file(path)).caption(title).await?;
    Ok(())
}

/// Crop to 640x640 square and send as video_note (circle).
async fn send_as_circle(
    bot: &Bot,
    chat_id: ChatId,
    input_path: &str,
    download_folder: &str,
    session_id: &uuid::Uuid,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let circle_path = format!("{}/vlipsy_{}_circle.mp4", download_folder, session_id);
    let status = tokio::process::Command::new("ffmpeg")
        .args([
            "-i",
            input_path,
            "-vf",
            "scale=640:640:force_original_aspect_ratio=increase,crop=640:640,format=yuv420p",
            "-c:v",
            "libx264",
            "-preset",
            "fast",
            "-c:a",
            "aac",
            "-y",
            &circle_path,
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await?;

    if !status.success() {
        return Err("ffmpeg circle crop failed".into());
    }

    bot.send_video_note(chat_id, InputFile::file(&circle_path)).await?;

    let _ = tokio::fs::remove_file(&circle_path).await;
    Ok(())
}

/// Convert to MP3 and send as audio.
async fn send_as_mp3(
    bot: &Bot,
    chat_id: ChatId,
    input_path: &str,
    title: &str,
    download_folder: &str,
    session_id: &uuid::Uuid,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mp3_path = format!("{}/vlipsy_{}.mp3", download_folder, session_id);
    let status = tokio::process::Command::new("ffmpeg")
        .args([
            "-i",
            input_path,
            "-vn",
            "-acodec",
            "libmp3lame",
            "-ab",
            "192k",
            "-y",
            &mp3_path,
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await?;

    if !status.success() {
        return Err("ffmpeg mp3 conversion failed".into());
    }

    bot.send_audio(chat_id, InputFile::file(&mp3_path)).title(title).await?;

    let _ = tokio::fs::remove_file(&mp3_path).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_vlipsy_keyboard_repeat_1() {
        let kb = build_vlipsy_keyboard("abc123", 1);
        let rows = &kb.inline_keyboard;
        assert_eq!(rows.len(), 3, "Should have 3 rows");
        // Row 1: Video, Circle, MP3
        assert_eq!(rows[0].len(), 3);
        assert!(rows[0][0].text.contains("Video"));
        assert!(rows[0][1].text.contains("Circle"));
        assert!(rows[0][2].text.contains("MP3"));
        // Row 2: Repeat
        assert_eq!(rows[1].len(), 1);
        assert!(rows[1][0].text.contains("1x"));
        // Row 3: Cancel
        assert_eq!(rows[2].len(), 1);
        assert!(rows[2][0].text.contains("Cancel"));
    }

    #[test]
    fn test_build_vlipsy_keyboard_repeat_3() {
        let kb = build_vlipsy_keyboard("xyz", 3);
        assert!(kb.inline_keyboard[1][0].text.contains("3x"));
    }

    #[test]
    fn test_callback_data_within_64_bytes() {
        // Worst case: longest action + max repeat + typical url_id
        let cases = [
            format!("vp:circle:4:{}", "a".repeat(50)),
            format!("vp:video:4:{}", "a".repeat(50)),
            format!("vp:mp3:4:{}", "a".repeat(50)),
            format!("vp:rep:4:{}", "a".repeat(50)),
        ];
        for cb in &cases {
            assert!(cb.len() <= 64, "Callback too long ({} bytes): {}", cb.len(), cb);
        }
    }

    #[test]
    fn test_repeat_cycle() {
        // Verify the cycle logic
        let cycle = |n: u8| match n {
            1 => 2,
            2 => 3,
            3 => 4,
            _ => 1,
        };
        assert_eq!(cycle(1), 2);
        assert_eq!(cycle(2), 3);
        assert_eq!(cycle(3), 4);
        assert_eq!(cycle(4), 1);
    }

    #[test]
    fn test_cancel_reuses_pv_cancel() {
        let kb = build_vlipsy_keyboard("test_id", 1);
        let cancel_btn = &kb.inline_keyboard[2][0];
        if let teloxide::types::InlineKeyboardButtonKind::CallbackData(ref data) = cancel_btn.kind {
            assert!(
                data.starts_with("pv:cancel:"),
                "Cancel should reuse pv:cancel prefix: {}",
                data
            );
        } else {
            panic!("Cancel button should be callback data");
        }
    }
}
