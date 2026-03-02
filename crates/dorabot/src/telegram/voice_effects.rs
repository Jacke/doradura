//! Voice message effects: Backwards and BioShock Tear
//!
//! When a user sends a voice message, the bot replies with an effects menu.
//! Pressing a button downloads the voice, applies an ffmpeg filter, and sends
//! the result back as a new voice message.

use std::path::PathBuf;
use std::sync::Arc;

use teloxide::prelude::*;
use teloxide::types::{ChatId, InlineKeyboardMarkup, InputFile, Message, MessageId};

use crate::download::metadata::probe_duration_seconds;
use crate::storage::cache;
use crate::storage::db::DbPool;
use crate::telegram::admin::download_file_from_telegram;
use crate::telegram::cb;
use crate::telegram::Bot;

/// Handle an incoming voice message: cache the file_id and show the effects keyboard.
pub async fn handle_voice_message(bot: Bot, msg: Message, db_pool: Arc<DbPool>) -> ResponseResult<()> {
    let voice = match msg.voice() {
        Some(v) => v,
        None => return Ok(()),
    };

    let file_id = &voice.file.id.0;
    let file_hash = cache::store_url(&db_pool, file_id).await;

    let keyboard = build_voice_effects_keyboard(&file_hash);

    bot.send_message(msg.chat.id, "Choose an effect:")
        .reply_markup(keyboard)
        .await?;

    Ok(())
}

fn build_voice_effects_keyboard(file_hash: &str) -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new(vec![
        vec![
            cb("⏪ Backwards", format!("vfx:rev:{file_hash}")),
            cb("🌀 Tear", format!("vfx:tear:{file_hash}")),
        ],
        vec![cb("❌ Cancel", "vfx:cancel")],
    ])
}

/// Route a `vfx:*` callback to the appropriate handler.
pub async fn handle_voice_effect_callback(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    data: &str,
    db_pool: &DbPool,
) -> Result<(), teloxide::RequestError> {
    // vfx:cancel
    if data == "vfx:cancel" {
        bot.delete_message(chat_id, message_id).await?;
        return Ok(());
    }

    // vfx:{effect}:{file_hash}
    let parts: Vec<&str> = data.splitn(3, ':').collect();
    if parts.len() != 3 {
        log::warn!("Invalid vfx callback data: {}", data);
        return Ok(());
    }
    let effect = parts[1];
    let file_hash = parts[2];

    let file_id = match cache::get_url(db_pool, file_hash).await {
        Some(id) => id,
        None => {
            log::warn!("Voice file_id not found in cache for hash: {}", file_hash);
            let _ = bot
                .edit_message_text(chat_id, message_id, "Voice message expired, please send it again.")
                .await;
            return Ok(());
        }
    };

    // Show processing indicator
    let _ = bot.edit_message_text(chat_id, message_id, "⏳ Processing...").await;

    // Download voice file
    let input_path = PathBuf::from(format!("/tmp/vfx_{file_hash}_input.ogg"));
    let output_path = PathBuf::from(format!("/tmp/vfx_{file_hash}_{effect}_output.ogg"));

    if let Err(e) = download_file_from_telegram(bot, &file_id, Some(input_path.clone())).await {
        log::error!("Failed to download voice file: {}", e);
        let _ = bot
            .edit_message_text(chat_id, message_id, "Failed to download voice file.")
            .await;
        cleanup(&[&input_path, &output_path]);
        return Ok(());
    }

    // Build ffmpeg filter
    let filter = match effect {
        "rev" => "areverse".to_string(),
        "tear" => "aphaser=type=t:speed=0.4:decay=0.6,aecho=0.8:0.9:40|60:0.4|0.3,flanger=delay=3:depth=4:speed=0.3:type=triangular".to_string(),
        _ => {
            log::warn!("Unknown voice effect: {}", effect);
            cleanup(&[&input_path]);
            return Ok(());
        }
    };

    // Run ffmpeg
    let input_str = input_path.to_string_lossy().to_string();
    let output_str = output_path.to_string_lossy().to_string();

    let ffmpeg_result = tokio::process::Command::new("ffmpeg")
        .args([
            "-i",
            &input_str,
            "-af",
            &filter,
            "-c:a",
            "libopus",
            "-b:a",
            "64k",
            "-application",
            "voip",
            "-y",
            &output_str,
        ])
        .output()
        .await;

    match ffmpeg_result {
        Ok(output) if output.status.success() => {}
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            log::error!("ffmpeg failed: {}", stderr);
            let _ = bot
                .edit_message_text(chat_id, message_id, "ffmpeg processing failed.")
                .await;
            cleanup(&[&input_path, &output_path]);
            return Ok(());
        }
        Err(e) => {
            log::error!("Failed to run ffmpeg: {}", e);
            let _ = bot
                .edit_message_text(chat_id, message_id, "ffmpeg not available.")
                .await;
            cleanup(&[&input_path, &output_path]);
            return Ok(());
        }
    }

    // Get duration
    let duration = probe_duration_seconds(&output_str);

    // Send processed voice
    let mut req = bot.send_voice(chat_id, InputFile::file(&output_path));
    if let Some(dur) = duration {
        req = req.duration(dur);
    }

    match req.await {
        Ok(_) => {
            log::info!("Voice effect '{}' sent to chat {}", effect, chat_id);
        }
        Err(e) => {
            log::error!("Failed to send processed voice: {}", e);
        }
    }

    // Delete the processing message
    let _ = bot.delete_message(chat_id, message_id).await;

    cleanup(&[&input_path, &output_path]);
    Ok(())
}

fn cleanup(paths: &[&PathBuf]) {
    for path in paths {
        let _ = std::fs::remove_file(path);
    }
}
