//! "🔁 Loop to audio" feature — loops a downloaded video under an uploaded
//! audio track so the output MP4's duration matches the audio.
//!
//! ffmpeg `-stream_loop -1` loops the video input infinitely; `-shortest`
//! stops at the audio's end, making output duration = audio duration.
//! Re-encoding via libx264 is mandatory — `-c:v copy` with `-stream_loop`
//! fails when slice boundaries are not on keyframes, producing corrupted
//! output.
//!
//! Lifecycle:
//!   1. Callback `downloads:loop:{id}` creates a `VideoClipSession` with
//!      `output_kind = OutputKind::Loop` (reusing `start_session_from_download`).
//!   2. User uploads audio → the intercept in `commands/mod.rs` stores the
//!      `file_id` in `custom_audio_file_id`, deletes the session row, and
//!      spawns `process_loop_to_audio`.
//!   3. This module downloads both files, probes durations, runs ffmpeg,
//!      and sends the result via `bot.send_video`.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use teloxide::prelude::*;
use teloxide::types::{ChatId, InputFile};
use tokio::process::Command as TokioCommand;

use crate::storage::SharedStorage;
use crate::storage::db::DbPool;
use crate::telegram::Bot;
use doracore::storage::db::VideoClipSession;

/// Maximum audio duration we accept as loop target (seconds).
///
/// 10 min at libx264 veryfast crf 23 480p ≈ 25-45 MB which fits under
/// Telegram's 50 MB `sendVideo` ceiling. Anything longer risks being
/// silently demoted to `send_document`.
const MAX_AUDIO_DURATION_SECS: u32 = 600;

/// Minimum audio duration (avoid 0-length outputs).
const MIN_AUDIO_DURATION_SECS: u32 = 1;

/// Minimum video slice duration (avoid degenerate loops).
const MIN_VIDEO_DURATION_SECS: u32 = 1;

/// Timeout for the ffmpeg loop-and-mux command. 10 minutes matches the
/// subtitle-burn re-encode timeout used elsewhere in the codebase.
const LOOP_FFMPEG_TIMEOUT: Duration = Duration::from_secs(600);

/// Process a loop-to-audio session end to end: download both inputs,
/// probe durations, run ffmpeg, send the result.
///
/// Always returns `Ok(())`: user-visible failures are reported via bot
/// messages and the metrics counter. Returning `Err` only when something
/// truly unexpected happens would make the caller log noise, which we
/// avoid by handling everything in-function.
pub async fn process_loop_to_audio(
    bot: Bot,
    chat_id: ChatId,
    session: VideoClipSession,
    audio_file_id: String,
    db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
) -> anyhow::Result<()> {
    let _ = db_pool; // reserved for future DB logging

    // ── 1. Scoped temp dir (auto-cleans on drop) ────────────────────────
    let guard = doracore::core::utils::TempDirGuard::new("doradura_loop").await?;
    let video_path = guard.path().join("source.mp4");
    let audio_path = guard.path().join("audio.bin");
    let output_path = guard.path().join("output.mp4");

    // ── 2. Resolve source video's Telegram file_id ──────────────────────
    let download = match shared_storage
        .get_download_history_entry(chat_id.0, session.source_download_id)
        .await
    {
        Ok(Some(d)) => d,
        _ => {
            metrics_inc("download_failed");
            send_error(&bot, chat_id, &shared_storage, "loop.failed").await;
            return Ok(());
        }
    };
    let Some(video_file_id) = download.file_id else {
        metrics_inc("download_failed");
        send_error(&bot, chat_id, &shared_storage, "loop.failed").await;
        return Ok(());
    };

    // ── 3. Pull both files from Telegram ────────────────────────────────
    if let Err(e) =
        crate::telegram::download_file_with_fallback(&bot, &video_file_id, None, None, Some(video_path.clone())).await
    {
        log::warn!("Loop: failed to download source video: {}", e);
        metrics_inc("download_failed");
        send_error(&bot, chat_id, &shared_storage, "loop.failed").await;
        return Ok(());
    }
    if let Err(e) =
        crate::telegram::download_file_with_fallback(&bot, &audio_file_id, None, None, Some(audio_path.clone())).await
    {
        log::warn!("Loop: failed to download user audio: {}", e);
        metrics_inc("download_failed");
        send_error(&bot, chat_id, &shared_storage, "loop.failed").await;
        return Ok(());
    }

    // ── 4. Probe audio duration (rejection gates) ───────────────────────
    let Some(audio_path_str) = audio_path.to_str() else {
        metrics_inc("ffmpeg_failed");
        send_error(&bot, chat_id, &shared_storage, "loop.failed").await;
        return Ok(());
    };
    let Some(audio_dur) = doracore::download::metadata::probe_duration_seconds(audio_path_str).await else {
        log::warn!("Loop: ffprobe audio failed");
        metrics_inc("ffmpeg_failed");
        send_error(&bot, chat_id, &shared_storage, "loop.failed").await;
        return Ok(());
    };
    if audio_dur < MIN_AUDIO_DURATION_SECS {
        metrics_inc("audio_too_short");
        send_error(&bot, chat_id, &shared_storage, "loop.audio_too_short").await;
        return Ok(());
    }
    if audio_dur > MAX_AUDIO_DURATION_SECS {
        metrics_inc("audio_too_long");
        send_error(&bot, chat_id, &shared_storage, "loop.audio_too_long").await;
        return Ok(());
    }

    // ── 5. Probe video slice duration (rejection gate) ──────────────────
    let Some(video_path_str) = video_path.to_str() else {
        metrics_inc("ffmpeg_failed");
        send_error(&bot, chat_id, &shared_storage, "loop.failed").await;
        return Ok(());
    };
    let Some(video_dur) = doracore::download::metadata::probe_duration_seconds(video_path_str).await else {
        log::warn!("Loop: ffprobe video failed");
        metrics_inc("ffmpeg_failed");
        send_error(&bot, chat_id, &shared_storage, "loop.failed").await;
        return Ok(());
    };
    if video_dur < MIN_VIDEO_DURATION_SECS {
        metrics_inc("video_too_short");
        send_error(&bot, chat_id, &shared_storage, "loop.video_too_short").await;
        return Ok(());
    }

    // ── 6. Run ffmpeg loop-and-mux ──────────────────────────────────────
    let mut cmd = build_loop_ffmpeg_command(&video_path, &audio_path, &output_path);
    let out = match doracore::core::process::run_with_timeout(&mut cmd, LOOP_FFMPEG_TIMEOUT).await {
        Ok(o) => o,
        Err(e) => {
            log::error!("Loop: ffmpeg runner error: {:?}", e);
            metrics_inc("ffmpeg_failed");
            send_error(&bot, chat_id, &shared_storage, "loop.failed").await;
            return Ok(());
        }
    };
    if !out.status.success() {
        log::error!("Loop: ffmpeg non-zero exit: {}", String::from_utf8_lossy(&out.stderr));
        metrics_inc("ffmpeg_failed");
        send_error(&bot, chat_id, &shared_storage, "loop.failed").await;
        return Ok(());
    }

    // ── 7. Probe output (uses rotation-aware helper) ────────────────────
    let Some(output_path_str) = output_path.to_str() else {
        metrics_inc("send_failed");
        send_error(&bot, chat_id, &shared_storage, "loop.failed").await;
        return Ok(());
    };
    let (out_dur, width, height) = doracore::download::metadata::probe_video_metadata(output_path_str)
        .await
        .unwrap_or((audio_dur, None, None));

    // ── 8. Send the final MP4 ───────────────────────────────────────────
    // `bot.send_video` direct call — no ProgressMessage required. Mirrors
    // the file_id cache-hit path in `download/pipeline.rs`.
    let input = InputFile::file(&output_path);
    let mut req = bot
        .send_video(chat_id, input)
        .supports_streaming(true)
        .duration(out_dur);
    if let Some(w) = width {
        req = req.width(w);
    }
    if let Some(h) = height {
        req = req.height(h);
    }
    if let Err(e) = req.await {
        log::error!("Loop: send_video failed: {}", e);
        metrics_inc("send_failed");
        send_error(&bot, chat_id, &shared_storage, "loop.failed").await;
        return Ok(());
    }

    metrics_inc("success");
    log::info!(
        "Loop: delivered to chat {} (source {}s, audio {}s, out {}s)",
        chat_id,
        video_dur,
        audio_dur,
        out_dur
    );
    Ok(())
}

/// Build the ffmpeg invocation. Extracted so it's unit-testable without
/// spawning a subprocess.
fn build_loop_ffmpeg_command(video: &Path, audio: &Path, output: &Path) -> TokioCommand {
    let mut cmd = TokioCommand::new("ffmpeg");
    cmd.kill_on_drop(true);
    cmd.arg("-v").arg("error");
    cmd.arg("-stream_loop").arg("-1"); // loop input 0 infinitely
    cmd.arg("-i").arg(video);
    cmd.arg("-i").arg(audio);
    cmd.arg("-map").arg("0:v:0"); // video only from input 0
    cmd.arg("-map").arg("1:a:0"); // audio only from input 1
    cmd.arg("-c:v").arg("libx264"); // mandatory re-encode for loop boundary integrity
    cmd.arg("-preset").arg("veryfast");
    cmd.arg("-crf").arg("23");
    cmd.arg("-pix_fmt").arg("yuv420p"); // broad Telegram player compat
    cmd.arg("-c:a").arg("aac");
    cmd.arg("-b:a").arg("192k");
    cmd.arg("-shortest"); // stop at audio end → out dur = audio dur
    cmd.arg("-movflags").arg("+faststart"); // streamable (Telegram requires)
    cmd.arg("-y");
    cmd.arg(output);
    cmd
}

fn metrics_inc(outcome: &'static str) {
    doracore::core::metrics::LOOP_TO_AUDIO_TOTAL
        .with_label_values(&[outcome])
        .inc();
}

async fn send_error(bot: &Bot, chat_id: ChatId, shared_storage: &Arc<SharedStorage>, key: &str) {
    let lang = crate::i18n::user_lang_from_storage(shared_storage, chat_id.0).await;
    let text = crate::i18n::t(&lang, key);
    let _ = bot.send_message(chat_id, text).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    use std::path::PathBuf;

    #[test]
    fn build_loop_ffmpeg_command_has_required_flags() {
        let cmd = build_loop_ffmpeg_command(
            &PathBuf::from("/tmp/v.mp4"),
            &PathBuf::from("/tmp/a.mp3"),
            &PathBuf::from("/tmp/o.mp4"),
        );
        let args: Vec<&OsStr> = cmd.as_std().get_args().collect();

        // Core loop semantics
        assert!(args.iter().any(|a| *a == "-stream_loop"), "missing -stream_loop");
        assert!(args.iter().any(|a| *a == "-shortest"), "missing -shortest");

        // Two -map args: one for video, one for audio
        let map_count = args.iter().filter(|a| **a == "-map").count();
        assert_eq!(map_count, 2, "expected exactly 2 -map args, got {}", map_count);
        assert!(args.iter().any(|a| *a == "0:v:0"), "missing video map target");
        assert!(args.iter().any(|a| *a == "1:a:0"), "missing audio map target");

        // Re-encoding (not stream copy)
        assert!(args.iter().any(|a| *a == "libx264"), "missing libx264 codec");
        assert!(args.iter().any(|a| *a == "aac"), "missing aac codec");

        // Telegram compatibility
        assert!(args.iter().any(|a| *a == "+faststart"), "missing +faststart movflags");
        assert!(args.iter().any(|a| *a == "yuv420p"), "missing yuv420p pix_fmt");
    }

    #[test]
    fn build_loop_ffmpeg_command_input_order() {
        let cmd = build_loop_ffmpeg_command(
            &PathBuf::from("/tmp/video.mp4"),
            &PathBuf::from("/tmp/audio.mp3"),
            &PathBuf::from("/tmp/out.mp4"),
        );
        let args: Vec<String> = cmd
            .as_std()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();

        // -stream_loop -1 must come BEFORE the first -i (it applies to the
        // next input). Find the positions.
        let stream_loop_pos = args.iter().position(|a| a == "-stream_loop").unwrap();
        let first_i_pos = args.iter().position(|a| a == "-i").unwrap();
        assert!(
            stream_loop_pos < first_i_pos,
            "-stream_loop must come before the first -i"
        );

        // Video must be the first input (so -stream_loop applies to it).
        let video_pos = args.iter().position(|a| a == "/tmp/video.mp4").unwrap();
        let audio_pos = args.iter().position(|a| a == "/tmp/audio.mp3").unwrap();
        assert!(video_pos < audio_pos, "video input must come before audio input");
    }
}
