//! Audio-cut pipeline — `process_audio_cut`.
//!
//! Cuts user-selected segments out of a previously-uploaded audio file and
//! sends the result back to the chat. Reuses `build_cut_filter` from the
//! parent module for the ffmpeg filter graph; everything else (status
//! message, temp dir, send-as-audio/document toggle, GH #8 progress
//! pulses) is local. Extracted from `circle/mod.rs` (Phase 2 split).

use std::sync::Arc;

use teloxide::prelude::*;

use crate::core::error::AppError;
use crate::i18n;
use crate::storage::SharedStorage;
use crate::storage::db::DbPool;
use crate::telegram::Bot;

use super::CutSegment;
use super::build_cut_filter;

pub async fn process_audio_cut(
    bot: Bot,
    db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
    chat_id: ChatId,
    session: crate::download::audio_effects::AudioEffectSession,
    segments: Vec<CutSegment>,
    segments_text: String,
) -> Result<(), AppError> {
    use tokio::process::Command;

    let _ = db_pool;
    let lang = i18n::user_lang_from_storage(&shared_storage, chat_id.0).await;
    let total_len: i64 = segments.iter().map(|s| (s.end_secs - s.start_secs).max(0)).sum();
    if total_len <= 0 {
        bot.send_message(chat_id, i18n::t(&lang, "commands.empty_cut"))
            .await
            .ok();
        return Ok(());
    }

    let input_path = std::path::PathBuf::from(&session.original_file_path);
    if !input_path.exists() {
        bot.send_message(chat_id, i18n::t(&lang, "commands.audio_source_missing"))
            .await
            .ok();
        return Ok(());
    }

    let args = doracore::fluent_args!("segments" => segments_text.as_str());
    let status = bot
        .send_message(chat_id, i18n::t_args(&lang, "commands.audio_cut_processing", &args))
        .await?;

    let guard = crate::core::utils::TempDirGuard::new("doradura_audio_cut")
        .await
        .map_err(AppError::Io)?;

    let output_path = guard
        .path()
        .join(format!("cut_audio_{}_{}.mp3", chat_id.0, uuid::Uuid::new_v4()));

    // Fast seek for audio cuts
    let audio_seek_offset = segments
        .iter()
        .map(|s| s.start_secs)
        .min()
        .unwrap_or(0)
        .saturating_sub(5)
        .max(0);

    let seeked_audio_segments: Vec<CutSegment> = segments
        .iter()
        .map(|s| CutSegment {
            start_secs: s.start_secs - audio_seek_offset,
            end_secs: s.end_secs - audio_seek_offset,
        })
        .collect();

    let filter = build_cut_filter(&seeked_audio_segments, false, true);

    let mut audio_cmd = Command::new("ffmpeg");
    audio_cmd.arg("-hide_banner").arg("-loglevel").arg("info");
    if audio_seek_offset > 0 {
        audio_cmd.arg("-ss").arg(format!("{}", audio_seek_offset));
    }
    let audio_timeout = std::time::Duration::from_secs(5 * 60); // 5 minutes for audio
    audio_cmd
        .arg("-i")
        .arg(&input_path)
        .arg("-filter_complex")
        .arg(&filter)
        .arg("-map")
        .arg("[a]")
        .arg("-q:a")
        .arg("0")
        .arg("-y")
        .arg(&output_path);
    // Pulses every 3s (GH #8) — moved to shared helper in alpha.20.
    let audio_outcome = crate::core::progress_pulse::run_ffmpeg_with_progress(
        &bot,
        chat_id,
        status.id,
        &mut audio_cmd,
        audio_timeout,
        "🎵 Cutting audio",
    )
    .await;

    let output = match audio_outcome {
        doracore::core::process::PulseOutcome::Done(o) => o,
        doracore::core::process::PulseOutcome::Io(e) => return Err(AppError::Io(e)),
        doracore::core::process::PulseOutcome::Timeout => {
            log::error!("❌ Audio ffmpeg timed out after {} seconds", audio_timeout.as_secs());
            bot.delete_message(chat_id, status.id).await.ok();
            bot.send_message(chat_id, "❌ Audio processing timed out. Try a shorter segment.")
                .await
                .ok();
            return Ok(());
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bot.delete_message(chat_id, status.id).await.ok();
        let args = doracore::fluent_args!("stderr" => stderr.to_string());
        bot.send_message(chat_id, i18n::t_args(&lang, "commands.ffmpeg_error_single", &args))
            .await
            .ok();
        return Ok(());
    }

    if !output_path.exists() {
        bot.delete_message(chat_id, status.id).await.ok();
        bot.send_message(chat_id, i18n::t(&lang, "commands.output_file_missing"))
            .await
            .ok();
        return Ok(());
    }

    let file_size = fs_err::tokio::metadata(&output_path)
        .await
        .map(|m| m.len())
        .unwrap_or(0);
    let limits = doracore::core::upload_limits::UploadLimits::from_env();
    if limits
        .check(doracore::core::upload_limits::UploadKind::Audio, file_size)
        .is_err()
    {
        bot.delete_message(chat_id, status.id).await.ok();
        bot.send_message(chat_id, i18n::t(&lang, "commands.audio_too_large_for_telegram"))
            .await
            .ok();
        return Ok(());
    }

    let caption = format!("{} [cut {}]", session.title, segments_text);
    let send_as_document = shared_storage
        .get_user_send_audio_as_document(chat_id.0)
        .await
        .unwrap_or(0);

    let send_res = if send_as_document == 0 {
        bot.send_audio(chat_id, teloxide::types::InputFile::file(output_path.clone()))
            .caption(caption)
            .duration(total_len.max(1) as u32)
            .await
    } else {
        bot.send_document(chat_id, teloxide::types::InputFile::file(output_path.clone()))
            .caption(caption)
            .await
    };

    if let Err(e) = send_res {
        bot.delete_message(chat_id, status.id).await.ok();
        let args = doracore::fluent_args!("error" => e.to_string());
        bot.send_message(chat_id, i18n::t_args(&lang, "commands.audio_send_failed", &args))
            .await
            .ok();
        return Ok(());
    }

    bot.delete_message(chat_id, status.id).await.ok();
    // guard drops here, cleaning up the temp dir
    Ok(())
}
