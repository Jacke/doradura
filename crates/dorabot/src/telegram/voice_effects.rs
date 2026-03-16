//! Voice message effects: Backwards and BioShock Tear
//!
//! When a user sends a voice message, the bot replies with an effects menu.
//! Pressing a button downloads the voice, applies an ffmpeg filter, and sends
//! the result back as a new voice message.

use std::sync::Arc;

use teloxide::prelude::*;
use teloxide::types::{ChatId, InlineKeyboardMarkup, InputFile, Message, MessageId};

use crate::storage::cache;
use crate::storage::db::DbPool;
use crate::telegram::cb;
use crate::telegram::download_file_from_telegram;
use crate::telegram::Bot;

/// Handle an incoming voice message: cache the file_id and show the effects keyboard.
pub async fn handle_voice_message(bot: Bot, msg: Message, db_pool: Arc<DbPool>) -> ResponseResult<()> {
    let voice = match msg.voice() {
        Some(v) => v,
        None => return Ok(()),
    };

    let file_id = &voice.file.id.0;
    let file_hash = cache::store_url(&db_pool, file_id).await;
    let duration = voice.duration.seconds();

    let keyboard = build_voice_effects_keyboard(&file_hash, duration);

    bot.send_message(msg.chat.id, "Choose an effect:")
        .reply_markup(keyboard)
        .await?;

    Ok(())
}

fn build_voice_effects_keyboard(file_hash: &str, duration: u32) -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new(vec![
        vec![
            cb("⏪ Backwards", format!("vfx:rev:{file_hash}:{duration}")),
            cb("🌀 Tear", format!("vfx:tear:{file_hash}:{duration}")),
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

    // vfx:{effect}:{file_hash}:{duration}
    let parts: Vec<&str> = data.splitn(4, ':').collect();
    if parts.len() != 4 {
        log::warn!("Invalid vfx callback data: {}", data);
        return Ok(());
    }
    let effect = parts[1];
    let file_hash = parts[2];
    let duration: u32 = parts[3].parse().unwrap_or(0);

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
    let guard = match crate::core::utils::TempDirGuard::new("doradura_vfx").await {
        Ok(g) => g,
        Err(e) => {
            log::error!("Failed to create vfx temp dir: {}", e);
            let _ = bot.edit_message_text(chat_id, message_id, "Internal error.").await;
            return Ok(());
        }
    };
    let input_path = guard.path().join(format!("vfx_{file_hash}_{}_input.ogg", chat_id.0));
    let output_path = guard.path().join(format!("vfx_{file_hash}_{}_output.ogg", chat_id.0));

    if let Err(e) = download_file_from_telegram(bot, &file_id, Some(input_path.clone())).await {
        log::error!("Failed to download voice file: {}", e);
        let _ = bot
            .edit_message_text(chat_id, message_id, "Failed to download voice file.")
            .await;
        return Ok(());
    }

    let input_str = input_path.to_string_lossy().to_string();
    let output_str = output_path.to_string_lossy().to_string();

    // Build ffmpeg args — "tear" needs filter_complex for reverse reverb,
    // other effects use a simple -af chain.
    let ffmpeg_args: Vec<String> = match effect {
        "rev" => vec![
            "-i".into(),
            input_str,
            "-af".into(),
            "areverse".into(),
            "-c:a".into(),
            "libopus".into(),
            "-b:a".into(),
            "64k".into(),
            "-application".into(),
            "voip".into(),
            "-y".into(),
            output_str,
        ],
        // BioShock Infinite tear: voice through a dimensional rift.
        //
        // Key technique: **reverse reverb** — the echo arrives BEFORE the
        // sound, as if bleeding through from another timeline.
        //
        // Pipeline (filter_complex):
        //   1. Split input into [dry] and [wet]
        //   2. [wet]: reverse → heavy echo → reverse back  (= reverse reverb)
        //   3. Mix dry (60%) + reverse-reverbed (50%)
        //   4. Frequency-shift down 100 Hz (dimensional detuning)
        //   5. Chorus (multiple realities bleeding through)
        //   6. Subtle vibrato (unstable spacetime)
        //   7. Gentle tremolo (portal breathing)
        "tear" => {
            let fc = concat!(
                "[0:a]asplit=2[dry][wet];",
                "[wet]areverse,aecho=0.8:0.88:100|200:0.3|0.15,areverse[rev];",
                "[dry][rev]amix=inputs=2:weights=0.6 0.5[mixed];",
                "[mixed]afreqshift=shift=-100,",
                "chorus=0.6:0.9:55|40:0.4|0.32:0.25|0.4:2|2.3,",
                "vibrato=f=4:d=0.3,",
                "tremolo=f=2:d=0.3[out]",
            );
            vec![
                "-i".into(),
                input_str,
                "-filter_complex".into(),
                fc.into(),
                "-map".into(),
                "[out]".into(),
                "-c:a".into(),
                "libopus".into(),
                "-b:a".into(),
                "64k".into(),
                "-application".into(),
                "voip".into(),
                "-y".into(),
                output_str,
            ]
        }
        _ => {
            log::warn!("Unknown voice effect: {}", effect);
            return Ok(());
        }
    };

    // Run ffmpeg with a timeout to prevent hung processes
    const FFMPEG_VOICE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);
    let ffmpeg_result = tokio::time::timeout(
        FFMPEG_VOICE_TIMEOUT,
        tokio::process::Command::new("ffmpeg").args(&ffmpeg_args).output(),
    )
    .await;

    match ffmpeg_result {
        Ok(Ok(output)) if output.status.success() => {}
        Ok(Ok(output)) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            log::error!("ffmpeg voice effect '{}' failed: {}", effect, stderr);
            // Extract the meaningful error line for the user
            let user_msg = stderr
                .lines()
                .find(|l| l.starts_with("Error"))
                .map(|l| format!("ffmpeg error: {}", l))
                .unwrap_or_else(|| "ffmpeg processing failed.".into());
            let _ = bot.edit_message_text(chat_id, message_id, user_msg).await;
            return Ok(());
        }
        Ok(Err(e)) => {
            log::error!("Failed to run ffmpeg: {}", e);
            let _ = bot
                .edit_message_text(chat_id, message_id, "ffmpeg not available.")
                .await;
            return Ok(());
        }
        Err(_) => {
            log::error!(
                "ffmpeg voice effect '{}' timed out after {}s for chat {}",
                effect,
                FFMPEG_VOICE_TIMEOUT.as_secs(),
                chat_id.0
            );
            let _ = bot
                .edit_message_text(
                    chat_id,
                    message_id,
                    "Voice effect processing timed out. Please try again.",
                )
                .await;
            return Ok(());
        }
    }

    // Send processed voice with original duration
    let mut req = bot.send_voice(chat_id, InputFile::file(&output_path));
    if duration > 0 {
        req = req.duration(duration);
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

    // guard drops here, cleaning up the temp dir
    Ok(())
}
