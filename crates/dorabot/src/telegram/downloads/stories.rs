//! Instagram Stories: reformat a downloaded clip into a vertical 9:16 canvas
//! (1080×1920) with a blurred fill background and split it into ≤60 s story
//! segments, each sent as a separate playable video.
//!
//! Flow: `downloads:stories:{download_id}` button → resolve the MP4 download →
//! download the source file (Bot API → MTProto fallback) → one ffmpeg pass that
//! (a) scales the clip to fit the 9:16 frame, (b) fills the letterbox area with
//! a blurred, slightly darkened copy of the same frame, and (c) cuts the result
//! into 60 s segments via the `segment` muxer with forced keyframes at each
//! boundary → send every segment as a portrait video.
//!
//! Self-contained on purpose: the existing `process_video_clip` pipeline is
//! heavily specialised for circles/ringtones/GIFs, so threading a Stories
//! `OutputKind` through it would touch many fragile branches. This module only
//! reuses the shared download/ffmpeg/send helpers.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use teloxide::prelude::*;
use teloxide::types::InputFile;
use tokio::process::Command;

use crate::i18n;
use crate::storage::SharedStorage;
use crate::telegram::Bot;
use crate::telegram::BotExt;

use super::CallbackCtx;

/// Story canvas — Instagram's native 9:16 portrait resolution.
const STORY_W: u32 = 1080;
const STORY_H: u32 = 1920;
/// Per-segment length. 60 s is the current Instagram Stories per-card limit.
const STORY_SEGMENT_SECS: u32 = 60;
/// Hard ceiling on source length we'll process — keeps the encode bounded.
const MAX_TOTAL_SECS: i64 = 600; // 10 min
/// ffmpeg wall-clock timeout for the transform + segment pass.
const STORIES_FFMPEG_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// Convert any `Display` error into a `teloxide::RequestError` for `?`.
fn to_req_err(e: impl std::fmt::Display) -> teloxide::RequestError {
    teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string())))
}

pub(super) async fn handle(ctx: &CallbackCtx, action: &str, parts: &[&str]) -> ResponseResult<()> {
    // Only one action lives here: `downloads:stories:{download_id}`.
    if action != "stories" || parts.len() < 3 {
        return Ok(());
    }
    let download_id = parts[2].parse::<i64>().unwrap_or(0);

    let Some(download) = ctx
        .shared_storage
        .get_download_history_entry(ctx.chat_id.0, download_id)
        .await
        .map_err(to_req_err)?
    else {
        return Ok(());
    };

    let lang = crate::i18n::user_lang_from_storage(&ctx.shared_storage, ctx.chat_id.0).await;

    if download.format != "mp4" {
        ctx.bot
            .send_md(ctx.chat_id, i18n::t(&lang, "stories-only-mp4"))
            .await
            .ok();
        return Ok(());
    }
    let Some(file_id) = download.file_id.clone() else {
        ctx.bot
            .send_md(ctx.chat_id, i18n::t(&lang, "stories-no-file-id"))
            .await
            .ok();
        return Ok(());
    };

    // Heavy work runs detached so the callback returns immediately.
    let bot = ctx.bot.clone();
    let shared_storage = ctx.shared_storage.clone();
    let chat_id = ctx.chat_id;
    let title = download.title.clone();
    tokio::spawn(async move {
        if let Err(e) = run_stories(bot, shared_storage, chat_id, download_id, file_id, title).await {
            log::error!("stories: processing failed for download {}: {}", download_id, e);
        }
    });

    Ok(())
}

/// Download the source MP4, render it to vertical 9:16 with a blurred fill
/// background, split into 60 s segments and send each as a portrait video.
async fn run_stories(
    bot: Bot,
    shared_storage: Arc<SharedStorage>,
    chat_id: ChatId,
    download_id: i64,
    file_id: String,
    title: String,
) -> ResponseResult<()> {
    let lang = crate::i18n::user_lang_from_storage(&shared_storage, chat_id.0).await;
    let status = bot.send_message(chat_id, i18n::t(&lang, "stories-preparing")).await?;

    // MTProto fallback coordinates for large files.
    let (fallback_message_id, fallback_chat_id) = shared_storage
        .get_download_message_info(download_id)
        .await
        .ok()
        .flatten()
        .unzip();

    let guard = match crate::core::utils::TempDirGuard::new("doradura_stories").await {
        Ok(g) => g,
        Err(e) => {
            bot.delete_message(chat_id, status.id).await.ok();
            return Err(to_req_err(e));
        }
    };
    let dir = guard.path().to_path_buf();
    let input_path = dir.join("source.mp4");

    // ── Download source ──
    if let Err(e) = crate::telegram::download_file_with_fallback(
        &bot,
        &file_id,
        fallback_message_id,
        fallback_chat_id,
        Some(input_path.clone()),
    )
    .await
    {
        log::error!("stories: source download failed: {}", e);
        bot.delete_message(chat_id, status.id).await.ok();
        bot.send_message(chat_id, i18n::t(&lang, "stories-download-failed"))
            .await
            .ok();
        return Ok(());
    }

    // Cap overly long sources so the encode stays bounded.
    let source_secs = doracore::download::metadata::probe_duration_seconds(&input_path.to_string_lossy())
        .await
        .map(|d| d as i64);
    let capped = matches!(source_secs, Some(d) if d > MAX_TOTAL_SECS);

    // ── Render + segment in a single ffmpeg pass ──
    let output_pattern = dir.join("story_%03d.mp4");
    let mut cmd = build_stories_cmd(&input_path, &output_pattern, capped);

    let outcome = crate::core::progress_pulse::run_ffmpeg_with_progress(
        &bot,
        chat_id,
        status.id,
        &mut cmd,
        STORIES_FFMPEG_TIMEOUT,
        "📱 Render Stories",
    )
    .await;

    let output = match outcome {
        doracore::core::process::PulseOutcome::Done(o) => o,
        doracore::core::process::PulseOutcome::Io(e) => {
            bot.delete_message(chat_id, status.id).await.ok();
            return Err(to_req_err(e));
        }
        doracore::core::process::PulseOutcome::Timeout => {
            bot.delete_message(chat_id, status.id).await.ok();
            bot.send_message(chat_id, i18n::t(&lang, "stories-timeout")).await.ok();
            return Ok(());
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        log::error!("stories: ffmpeg failed: {}", stderr.lines().last().unwrap_or(""));
        bot.delete_message(chat_id, status.id).await.ok();
        bot.send_message(chat_id, i18n::t(&lang, "stories-cut-failed"))
            .await
            .ok();
        return Ok(());
    }

    // ── Collect produced segments ──
    let mut segments: Vec<PathBuf> = Vec::new();
    if let Ok(mut entries) = fs_err::tokio::read_dir(&dir).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let p = entry.path();
            if let Some(name) = p.file_name().and_then(|n| n.to_str())
                && name.starts_with("story_")
                && name.ends_with(".mp4")
            {
                segments.push(p);
            }
        }
    }
    segments.sort();

    if segments.is_empty() {
        bot.delete_message(chat_id, status.id).await.ok();
        bot.send_message(chat_id, i18n::t(&lang, "stories-no-segments"))
            .await
            .ok();
        return Ok(());
    }

    let total = segments.len();

    // ── Send each segment as a portrait video ──
    let mut sent = 0usize;
    for (idx, seg) in segments.iter().enumerate() {
        let dur = doracore::download::metadata::probe_duration_seconds(&seg.to_string_lossy()).await;
        let caption = i18n::t_args(
            &lang,
            "stories-caption",
            &doracore::fluent_args!("title" => title.as_str(), "index" => idx as i64 + 1, "total" => total as i64),
        );

        // Portrait video: omit explicit thumbnail (Telegram auto-generates one
        // matching the actual frame orientation; see download/send.rs notes).
        let mut req = bot
            .send_video(chat_id, InputFile::file(seg.clone()))
            .caption(caption)
            .width(STORY_W)
            .height(STORY_H);
        if let Some(d) = dur {
            req = req.duration(d);
        }

        match req.await {
            Ok(_) => sent += 1,
            Err(e) => log::warn!("stories: failed to send segment {}/{}: {}", idx + 1, total, e),
        }
    }

    bot.delete_message(chat_id, status.id).await.ok();

    if sent == 0 {
        bot.send_message(chat_id, i18n::t(&lang, "stories-send-failed"))
            .await
            .ok();
    } else {
        let mut done = i18n::t_args(&lang, "stories-done", &doracore::fluent_args!("count" => sent as i64));
        if capped {
            done.push('\n');
            done.push_str(&i18n::t_args(
                &lang,
                "stories-capped",
                &doracore::fluent_args!("minutes" => MAX_TOTAL_SECS / 60),
            ));
        }
        bot.send_message(chat_id, done).await.ok();
    }

    // `guard` drops here, removing the temp directory and all segments.
    drop(guard);
    Ok(())
}

/// Build the ffmpeg command: scale the clip into the 9:16 frame, fill the
/// letterbox area with a blurred + darkened copy of the same frame, then cut
/// into [`STORY_SEGMENT_SECS`]-long MP4 segments with keyframes forced at each
/// boundary so the segment muxer splits cleanly.
fn build_stories_cmd(input: &std::path::Path, output_pattern: &std::path::Path, capped: bool) -> Command {
    // [bg] = source scaled to *cover* the frame, cropped, heavily blurred and
    //        slightly darkened so the centred foreground pops.
    // [fg] = source scaled to *fit* inside the frame (full clip visible).
    let filter = format!(
        "[0:v]split=2[bg][fg];\
         [bg]scale={w}:{h}:force_original_aspect_ratio=increase,crop={w}:{h},boxblur=28:2,eq=brightness=-0.07[bg];\
         [fg]scale={w}:{h}:force_original_aspect_ratio=decrease[fg];\
         [bg][fg]overlay=(W-w)/2:(H-h)/2,setsar=1[v]",
        w = STORY_W,
        h = STORY_H,
    );

    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-hide_banner").arg("-loglevel").arg("info").arg("-y");
    cmd.arg("-i").arg(input);
    if capped {
        cmd.arg("-t").arg(MAX_TOTAL_SECS.to_string());
    }
    cmd.arg("-filter_complex")
        .arg(&filter)
        .arg("-map")
        .arg("[v]")
        .arg("-map")
        .arg("0:a?")
        // High-quality H.264, broadly compatible with mobile playback.
        .arg("-c:v")
        .arg("libx264")
        .arg("-profile:v")
        .arg("high")
        .arg("-level")
        .arg("4.2")
        .arg("-preset")
        .arg("medium")
        .arg("-crf")
        .arg("20")
        .arg("-pix_fmt")
        .arg("yuv420p")
        .arg("-r")
        .arg("30")
        // Force a keyframe at every segment boundary for clean cuts.
        .arg("-force_key_frames")
        .arg(format!("expr:gte(t,n_forced*{})", STORY_SEGMENT_SECS))
        .arg("-c:a")
        .arg("aac")
        .arg("-b:a")
        .arg("192k")
        .arg("-ar")
        .arg("44100")
        .arg("-f")
        .arg("segment")
        .arg("-segment_time")
        .arg(STORY_SEGMENT_SECS.to_string())
        .arg("-reset_timestamps")
        .arg("1")
        .arg("-segment_format")
        .arg("mp4")
        .arg(output_pattern);
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn story_canvas_is_9_by_16() {
        // 1080×1920 reduces to 9:16.
        assert_eq!(STORY_W * 16, STORY_H * 9);
    }

    #[test]
    fn segment_length_matches_instagram_limit() {
        assert_eq!(STORY_SEGMENT_SECS, 60);
    }

    #[test]
    fn filter_references_canvas_dimensions() {
        let cmd = build_stories_cmd(
            std::path::Path::new("/tmp/in.mp4"),
            std::path::Path::new("/tmp/story_%03d.mp4"),
            false,
        );
        let args: Vec<String> = cmd
            .as_std()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let filter = args.iter().find(|a| a.contains("overlay")).expect("filter present");
        assert!(filter.contains("1080"));
        assert!(filter.contains("1920"));
        assert!(filter.contains("boxblur"));
    }

    #[test]
    fn capped_adds_duration_limit() {
        let cmd = build_stories_cmd(
            std::path::Path::new("/tmp/in.mp4"),
            std::path::Path::new("/tmp/story_%03d.mp4"),
            true,
        );
        let args: Vec<String> = cmd
            .as_std()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(args.iter().any(|a| a == "-t"));
        assert!(args.iter().any(|a| a == &MAX_TOTAL_SECS.to_string()));
    }

    #[test]
    fn uncapped_has_no_duration_limit() {
        let cmd = build_stories_cmd(
            std::path::Path::new("/tmp/in.mp4"),
            std::path::Path::new("/tmp/story_%03d.mp4"),
            false,
        );
        let args: Vec<String> = cmd
            .as_std()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(!args.iter().any(|a| a == "-t"));
    }

    #[test]
    fn segment_muxer_configured() {
        let cmd = build_stories_cmd(
            std::path::Path::new("/tmp/in.mp4"),
            std::path::Path::new("/tmp/story_%03d.mp4"),
            false,
        );
        let args: Vec<String> = cmd
            .as_std()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let f_idx = args.iter().position(|a| a == "-f").expect("-f present");
        assert_eq!(args[f_idx + 1], "segment");
        let st_idx = args
            .iter()
            .position(|a| a == "-segment_time")
            .expect("-segment_time present");
        assert_eq!(args[st_idx + 1], STORY_SEGMENT_SECS.to_string());
    }
}
