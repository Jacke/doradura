use crate::conversion::video::{
    calculate_video_note_split, is_too_long_for_split, to_gif, to_video_notes_split, GifOptions, GIF_MAX_DURATION_SECS,
    VIDEO_NOTE_MAX_DURATION, VIDEO_NOTE_MAX_PARTS,
};
use crate::core::config;
use crate::core::error::AppError;
use crate::core::escape_markdown;
use crate::i18n;
use crate::storage::db::{self, DbPool, OutputKind, SourceKind};
use crate::storage::SharedStorage;
use crate::telegram::Bot;
use itertools::Itertools;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::ParseMode;

use super::subtitles::{download_circle_subtitles, BurnSubsResult};
use super::x264_params::video_note_dark_scene;

/// Segment of video to cut
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct CutSegment {
    pub start_secs: i64,
    pub end_secs: i64,
}

pub fn parse_command_segment(text: &str, video_duration: Option<i64>) -> Option<(i64, i64, String)> {
    let normalized = text.trim().to_lowercase();

    // Strip speed modifiers if present (e.g., "first30 2x", "full speed1.5")
    // We'll just parse the segment here, speed will be handled separately
    let segment_part = normalized.split_whitespace().next().unwrap_or(&normalized);

    // full - entire video
    if segment_part == "full" {
        let duration = video_duration?;
        let end = duration.min(60); // Max 60 seconds for video notes
        return Some((0, end, format!("00:00-{}", format_timestamp(end))));
    }

    // first<N> - first N seconds (first30, first15, etc.)
    if let Some(num_str) = segment_part.strip_prefix("first") {
        if let Ok(secs) = num_str.parse::<i64>() {
            if secs > 0 && secs <= 60 {
                return Some((0, secs, format!("00:00-{}", format_timestamp(secs))));
            }
        }
    }

    // last<N> - last N seconds (last30, last15, etc.)
    if let Some(num_str) = segment_part.strip_prefix("last") {
        if let Ok(secs) = num_str.parse::<i64>() {
            let duration = video_duration?;
            if secs > 0 && secs <= 60 && secs <= duration {
                let start = (duration - secs).max(0);
                return Some((
                    start,
                    duration,
                    format!("{}-{}", format_timestamp(start), format_timestamp(duration)),
                ));
            }
        }
    }

    // middle<N> - N seconds from the middle (middle30, middle15, etc.)
    if let Some(num_str) = segment_part.strip_prefix("middle") {
        if let Ok(secs) = num_str.parse::<i64>() {
            let duration = video_duration?;
            if secs > 0 && secs <= 60 && secs <= duration {
                let start = ((duration - secs) / 2).max(0);
                let end = start + secs;
                return Some((
                    start,
                    end,
                    format!("{}-{}", format_timestamp(start), format_timestamp(end)),
                ));
            }
        }
    }

    None
}

/// Build an ffmpeg `atempo` filter chain for a speed factor.
///
/// ffmpeg's `atempo` only accepts values in `[0.5, 2.0]`. For speeds outside that
/// range we chain two `atempo` filters so the combined multiplier equals `speed`.
/// Used by the cut/circle/ringtone filter builders — previously this 5-line
/// conditional was inlined 4 times verbatim.
fn build_atempo_filter(speed: f32) -> String {
    if speed > 2.0 {
        format!("atempo=2.0,atempo={}", speed / 2.0)
    } else if speed < 0.5 {
        format!("atempo=0.5,atempo={}", speed / 0.5)
    } else {
        format!("atempo={}", speed)
    }
}

/// Parse time range from text following a URL.
/// Accepts "HH:MM:SS-HH:MM:SS" or "MM:SS-MM:SS" after the URL.
pub fn parse_download_time_range(text: &str, url_text: &str) -> Option<(String, String, Option<f32>)> {
    let after = text.split(url_text).nth(1)?.trim();
    let mut parts = after.split_whitespace();
    let range_text = parts.next()?;
    if range_text.is_empty() {
        return None;
    }
    let normalized = range_text.replace(['—', '–', '−'], "-");
    let (start_str, end_str) = normalized.split_once('-')?;
    let start_secs = parse_timestamp_secs(start_str)?;
    let end_secs = parse_timestamp_secs(end_str)?;
    if end_secs <= start_secs {
        return None;
    }
    // Check remaining text for speed modifier (e.g., "2x", "1.5x", "speed2")
    let remaining: String = parts.join(" ");
    let speed = if remaining.is_empty() {
        None
    } else {
        parse_speed_modifier(&remaining)
    };
    Some((start_str.to_string(), end_str.to_string(), speed))
}

pub fn parse_time_range_secs(text: &str) -> Option<(i64, i64)> {
    let normalized = text.trim().replace(['—', '–', '−'], "-");
    // Strip trailing speed modifier (e.g., "2:40:53-2:42:19 2x" -> "2:40:53-2:42:19")
    let timestamp_part = normalized
        .rsplit_once(' ')
        .and_then(|(before, after)| {
            let lower = after.to_lowercase();
            if lower.ends_with('x') || lower.starts_with('x') || lower.starts_with("speed") {
                Some(before)
            } else {
                None
            }
        })
        .unwrap_or(&normalized);
    let cleaned = timestamp_part.replace(' ', "");
    let (start_str, end_str) = cleaned.split_once('-')?;
    let start = parse_timestamp_secs(start_str)?;
    let end = parse_timestamp_secs(end_str)?;
    if end <= start {
        return None;
    }
    Some((start, end))
}

pub fn parse_timestamp_secs(text: &str) -> Option<i64> {
    let parts: Vec<&str> = text.split(':').collect();
    match parts.len() {
        2 => {
            let minutes: i64 = parts[0].parse().ok()?;
            let seconds: i64 = parts[1].parse().ok()?;
            if minutes < 0 || !(0..60).contains(&seconds) {
                return None;
            }
            Some(minutes * 60 + seconds)
        }
        3 => {
            let hours: i64 = parts[0].parse().ok()?;
            let minutes: i64 = parts[1].parse().ok()?;
            let seconds: i64 = parts[2].parse().ok()?;
            if hours < 0 || minutes < 0 || !(0..60).contains(&minutes) || !(0..60).contains(&seconds) {
                return None;
            }
            Some(hours * 3600 + minutes * 60 + seconds)
        }
        _ => None,
    }
}

pub fn format_timestamp(secs: i64) -> String {
    let secs = secs.max(0);
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{h:02}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

pub fn parse_segments_spec(text: &str, video_duration: Option<i64>) -> Option<(Vec<CutSegment>, String, Option<f32>)> {
    let normalized = text.trim().replace(['—', '–', '−'], "-");

    // Extract speed modifier from anywhere in the text (e.g., "first30 2x", "1.5x full", "speed2 middle30")
    let speed = parse_speed_modifier(&normalized);

    let raw_parts: Vec<&str> = normalized
        .split([',', ';', '\n'])
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    if raw_parts.is_empty() {
        return None;
    }

    let mut segments = Vec::new();
    let mut pretty_parts = Vec::new();
    for part in raw_parts {
        // Try parsing as command first (full, first30, last30, etc.)
        if let Some((start_secs, end_secs, pretty)) = parse_command_segment(part, video_duration) {
            segments.push(CutSegment { start_secs, end_secs });
            pretty_parts.push(pretty);
        } else if let Some((start_secs, end_secs)) = parse_time_range_secs(part) {
            // Fall back to time range parsing
            segments.push(CutSegment { start_secs, end_secs });
            pretty_parts.push(format!(
                "{}-{}",
                format_timestamp(start_secs),
                format_timestamp(end_secs)
            ));
        } else {
            return None; // Invalid format
        }
    }

    Some((segments, pretty_parts.join(", "), speed))
}

pub fn parse_audio_segments_spec(text: &str, audio_duration: Option<i64>) -> Option<(Vec<CutSegment>, String)> {
    let normalized = text.trim();
    if normalized.is_empty() {
        return None;
    }

    let raw_parts: Vec<&str> = normalized
        .split([',', ';', '\n'])
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .collect();
    if raw_parts.is_empty() {
        return None;
    }

    let mut segments = Vec::new();
    let mut pretty_parts = Vec::new();
    for part in raw_parts {
        if let Some((start_secs, end_secs, pretty)) = parse_audio_command_segment(part, audio_duration) {
            segments.push(CutSegment { start_secs, end_secs });
            pretty_parts.push(pretty);
        } else if let Some((start_secs, end_secs)) = parse_time_range_secs(part) {
            segments.push(CutSegment { start_secs, end_secs });
            pretty_parts.push(format!(
                "{}-{}",
                format_timestamp(start_secs),
                format_timestamp(end_secs)
            ));
        } else {
            return None;
        }
    }

    Some((segments, pretty_parts.join(", ")))
}

pub fn parse_speed_modifier(text: &str) -> Option<f32> {
    let lower = text.to_lowercase();

    // Look for patterns like: "2x", "1.5x", "speed2", "speed1.5", "x2", "x1.5"
    for word in lower.split_whitespace() {
        // "2x", "1.5x"
        if let Some(num_str) = word.strip_suffix('x') {
            if let Ok(speed) = num_str.parse::<f32>() {
                if speed > 0.0 && speed <= 2.0 {
                    return Some(speed);
                }
            }
        }
        // "x2", "x1.5"
        if let Some(num_str) = word.strip_prefix('x') {
            if let Ok(speed) = num_str.parse::<f32>() {
                if speed > 0.0 && speed <= 2.0 {
                    return Some(speed);
                }
            }
        }
        // "speed2", "speed1.5"
        if let Some(num_str) = word.strip_prefix("speed") {
            if let Ok(speed) = num_str.parse::<f32>() {
                if speed > 0.0 && speed <= 2.0 {
                    return Some(speed);
                }
            }
        }
    }

    None
}

fn parse_audio_command_segment(text: &str, audio_duration: Option<i64>) -> Option<(i64, i64, String)> {
    let normalized = text.trim().to_lowercase();
    let segment_part = normalized.split_whitespace().next().unwrap_or(&normalized);
    let duration = audio_duration?;

    if segment_part == "full" {
        return Some((0, duration, format!("00:00-{}", format_timestamp(duration))));
    }

    if let Some(num_str) = segment_part.strip_prefix("first") {
        if let Ok(secs) = num_str.parse::<i64>() {
            if secs > 0 {
                let end = secs.min(duration);
                return Some((0, end, format!("00:00-{}", format_timestamp(end))));
            }
        }
    }

    if let Some(num_str) = segment_part.strip_prefix("last") {
        if let Ok(secs) = num_str.parse::<i64>() {
            if secs > 0 && secs <= duration {
                let start = (duration - secs).max(0);
                return Some((
                    start,
                    duration,
                    format!("{}-{}", format_timestamp(start), format_timestamp(duration)),
                ));
            }
        }
    }

    if let Some(num_str) = segment_part.strip_prefix("middle") {
        if let Ok(secs) = num_str.parse::<i64>() {
            if secs > 0 && secs <= duration {
                let start = ((duration - secs) / 2).max(0);
                let end = start + secs;
                return Some((
                    start,
                    end,
                    format!("{}-{}", format_timestamp(start), format_timestamp(end)),
                ));
            }
        }
    }

    None
}

/// Process video clip/circle creation
/// Resolved source file for a clip operation.
struct ClipSource {
    file_id: String,
    original_url: String,
    base_title: String,
    video_quality: Option<String>,
    fallback_message_id: Option<i32>,
    fallback_chat_id: Option<i64>,
}

/// Resolve the source (Download or Cut) into a `ClipSource`. Sends a user-facing
/// error message and returns `Ok(None)` when resolution fails so the caller can
/// early-return without touching `?`. `Ok(Some(_))` means good to proceed.
async fn resolve_clip_source(
    bot: &Bot,
    shared_storage: &SharedStorage,
    chat_id: ChatId,
    session: &db::VideoClipSession,
    is_ringtone: bool,
    lang: &unic_langid::LanguageIdentifier,
) -> Result<Option<ClipSource>, AppError> {
    let (file_id, original_url, base_title, video_quality) = match session.source_kind {
        SourceKind::Download => {
            let download = match shared_storage
                .get_download_history_entry(chat_id.0, session.source_id)
                .await?
            {
                Some(d) => d,
                None => {
                    bot.send_message(chat_id, i18n::t(lang, "commands.cut_file_not_found"))
                        .await
                        .ok();
                    return Ok(None);
                }
            };
            if download.format != "mp4" && !is_ringtone {
                bot.send_message(chat_id, i18n::t(lang, "commands.cut_only_mp4"))
                    .await
                    .ok();
                return Ok(None);
            }
            let fid = match download.file_id.clone() {
                Some(fid) => fid,
                None => {
                    bot.send_message(chat_id, i18n::t(lang, "commands.cut_missing_file_id"))
                        .await
                        .ok();
                    return Ok(None);
                }
            };
            (fid, download.url, download.title, download.video_quality)
        }
        SourceKind::Cut => {
            let cut = match shared_storage.get_cut_entry(chat_id.0, session.source_id).await? {
                Some(c) => c,
                None => {
                    bot.send_message(chat_id, i18n::t(lang, "commands.cut_not_found"))
                        .await
                        .ok();
                    return Ok(None);
                }
            };
            let fid = match cut.file_id.clone() {
                Some(fid) => fid,
                None => {
                    bot.send_message(chat_id, i18n::t(lang, "commands.cut_missing_file_id"))
                        .await
                        .ok();
                    return Ok(None);
                }
            };
            (
                fid,
                if !cut.original_url.is_empty() {
                    cut.original_url
                } else {
                    session.original_url.clone()
                },
                cut.title,
                cut.video_quality,
            )
        }
    };

    let message_info = match session.source_kind {
        SourceKind::Download => shared_storage
            .get_download_message_info(session.source_id)
            .await
            .ok()
            .flatten(),
        SourceKind::Cut => shared_storage
            .get_cut_message_info(session.source_id)
            .await
            .ok()
            .flatten(),
    };
    let (fallback_message_id, fallback_chat_id) = message_info.unzip();

    Ok(Some(ClipSource {
        file_id,
        original_url,
        base_title,
        video_quality,
        fallback_message_id,
        fallback_chat_id,
    }))
}

/// Convert the cut MP4 to GIF and send as animation. Sends user-facing error
/// messages on failure; caller should return immediately after this regardless
/// of outcome.
#[allow(clippy::too_many_arguments)]
async fn send_clip_as_gif(
    bot: &Bot,
    chat_id: ChatId,
    status_id: teloxide::types::MessageId,
    guard: &mut crate::core::utils::TempDirGuard,
    output_path: &std::path::Path,
    base_title: &str,
    segments_text: &str,
    url_suffix: &str,
    actual_total_len: i64,
    lang: &unic_langid::LanguageIdentifier,
) {
    let gif_title = format!("{} [gif {}]{}", base_title, segments_text, url_suffix);
    bot.edit_message_text(chat_id, status_id, "🖼️ Converting to GIF...")
        .await
        .ok();
    match to_gif(
        output_path,
        GifOptions {
            duration: Some(actual_total_len.max(1) as u64),
            start_time: None,
            width: Some(480),
            fps: Some(12),
        },
    )
    .await
    {
        Ok(gif_path) => {
            guard.track_file(gif_path.clone());
            bot.delete_message(chat_id, status_id).await.ok();
            match bot
                .send_animation(chat_id, teloxide::types::InputFile::file(&gif_path))
                .caption(&gif_title)
                .await
            {
                Ok(_) => {}
                Err(e) => {
                    log::error!("❌ Failed to send GIF: {}", e);
                    let args = doracore::fluent_args!("error" => e.to_string());
                    bot.send_message(chat_id, i18n::t_args(lang, "commands.gif_send_failed", &args))
                        .await
                        .ok();
                }
            }
        }
        Err(e) => {
            log::error!("❌ GIF conversion failed: {}", e);
            bot.delete_message(chat_id, status_id).await.ok();
            let args = doracore::fluent_args!("error" => e.to_string());
            bot.send_message(chat_id, i18n::t_args(lang, "commands.gif_conversion_failed", &args))
                .await
                .ok();
        }
    }
}

/// Build the `subtitles=…:force_style=…` filter fragment that burns a circle
/// video note's subtitles at the final 640x640 canvas size.
///
/// Returns `None` when no SRT path is provided. Path separators and `:` / `'`
/// are escaped for ffmpeg's `filter_complex` mini-language — the four-
/// backslash form is required because the string is interpreted twice:
/// once by ffmpeg's option parser and once by the filter graph parser.
fn build_circle_sub_filter(srt_path: Option<&std::path::Path>) -> Option<String> {
    let srt_path = srt_path?;
    let escaped = srt_path
        .to_string_lossy()
        .replace('\\', "\\\\\\\\")
        .replace(':', "\\\\:")
        .replace('\'', "\\\\'");
    let style = db::SubtitleStyle::circle_default();
    let force_style = style.to_force_style();
    Some(format!("subtitles='{escaped}':force_style='{force_style}'"))
}

/// Composed ffmpeg filter_complex strings plus the stream labels / encoding
/// knobs that the outer command builder needs to wire up `-map` / `-crf`.
///
/// Returned by [`build_clip_filter_plan`]. The caller uses `filter_av` for the
/// main pass, `filter_v` for the video-only retry pass, and the label / crf
/// fields to assemble the rest of the ffmpeg command line.
struct ClipFilterPlan {
    /// Full audio+video (or audio-only) `filter_complex` for the main pass.
    filter_av: String,
    /// Video-only `filter_complex` used for the retry pass when the main pass
    /// fails (e.g. audio track missing or misdetected). May be empty when
    /// there is no video stream.
    filter_v: String,
    /// `-map` label for the video output. `"[v]"` or `"[vout]"` when present,
    /// `""` when the output has no video stream.
    map_v_label: &'static str,
    /// `-map` label for the audio output. `"[a]"` or `"[aout]"`.
    map_a_label: &'static str,
    /// `-crf` value (as a string so it can be passed straight to ffmpeg).
    crf: &'static str,
}

/// Compose the ffmpeg `filter_complex` graph and associated mapping knobs for
/// a clip job.
///
/// Pure computation — no I/O, no allocation beyond the filter strings. The
/// resulting [`ClipFilterPlan`] captures every branch of the clip pipeline's
/// filter/map wiring:
///
///   * **single circle** (video note, no split) — applies
///     `scale=640:640,crop=640:640` post-filter plus optional burned-in
///     subtitles, at CRF 18 for visual quality.
///   * **multi-circle** (video note, needs split) — plain cut filter at
///     CRF 23; the circle framing is applied later by
///     `to_video_notes_split`.
///   * **ringtone** — audio-only chain with `atempo` speed adjustment.
///   * **regular cut with speed** — `setpts`/`atempo` on either the a/v or
///     audio-only chain depending on whether the source has video.
///   * **regular cut without speed** — bare `build_cut_filter` output.
///
/// Previously this logic was inlined inside `process_video_clip` as a ~120-LOC
/// match ladder; extracting it here leaves the async pipeline easier to read
/// and lets this pure string composition be reasoned about on its own.
fn build_clip_filter_plan(
    seeked_segments: &[CutSegment],
    has_video: bool,
    is_ringtone: bool,
    is_video_note: bool,
    video_note_needs_split: bool,
    circle_sub_filter: Option<&str>,
    speed: Option<f32>,
) -> ClipFilterPlan {
    // For ringtones the input is audio-only (MP3); embedded album art must be ignored
    // so that the filter_complex doesn't produce an unconnected [v] output which
    // makes ffmpeg exit with code 234 when using -f ipod.
    let has_video_for_filter = has_video && !is_ringtone;
    // Default cut filter operating on raw `[0:v]` / `[0:a]` — used by all
    // non-single-circle branches below. Single-circle path builds its own
    // pre-scaled variant to keep buffer memory in check.
    let base_filter_av = build_cut_filter(seeked_segments, has_video_for_filter, true);
    let base_filter_v = if has_video_for_filter {
        build_cut_filter(seeked_segments, true, false)
    } else {
        String::new()
    };

    // Apply speed modification if requested
    // For multi-circle video notes, don't apply circle formatting here - it will be done in split step
    let (filter_av, filter_v, map_v_label, map_a_label, crf) = if is_video_note && !video_note_needs_split {
        // Single circle: scale 4K→640² *before* the trim/concat chain so
        // downstream filters buffer 640² frames (~370 KB) instead of 4K
        // (~25 MB). On a 75 s segment that's the difference between the
        // pipeline holding ~45 GB of in-flight raw frames vs ~1.3 GB —
        // i.e. between OOM SIGKILL and a clean encode on Railway.
        //
        // Subtitle burn must still happen at 640² coordinates and AFTER
        // trim/concat (so timestamps align with the post-trim timeline),
        // so the sub filter goes into the post-chain step alongside speed
        // adjustments. Lanczos quality is identical regardless of where
        // in the chain the scale runs — same operation on the same
        // pixels, just with a smaller buffer footprint.
        let pre_scale =
            "[0:v]scale=640:640:flags=lanczos:force_original_aspect_ratio=increase,crop=640:640,format=yuv420p[v_pre]";
        let cut_av = build_cut_filter_with_input(seeked_segments, has_video_for_filter, true, "v_pre", "0:a");
        let cut_v = if has_video_for_filter {
            build_cut_filter_with_input(seeked_segments, true, false, "v_pre", "0:a")
        } else {
            String::new()
        };
        let post_v_chain = if let Some(sub_filter) = circle_sub_filter {
            sub_filter.to_string()
        } else {
            String::new()
        };

        if let Some(spd) = speed {
            let setpts_factor = 1.0 / spd;
            let atempo_filter = build_atempo_filter(spd);
            let v_chain = if post_v_chain.is_empty() {
                format!("[v]setpts={setpts_factor}*PTS[vout]")
            } else {
                format!("[v]setpts={setpts_factor}*PTS,{post_v_chain}[vout]")
            };
            let v_chain_v_only = if post_v_chain.is_empty() {
                format!("[v]setpts={setpts_factor}*PTS[vout]")
            } else {
                format!("[v]setpts={setpts_factor}*PTS,{post_v_chain}[vout]")
            };

            (
                format!("{pre_scale};{cut_av};{v_chain};[a]{atempo_filter}[aout]"),
                format!("{pre_scale};{cut_v};{v_chain_v_only}"),
                "[vout]",
                "[aout]",
                "18",
            )
        } else if post_v_chain.is_empty() {
            (
                format!("{pre_scale};{cut_av}"),
                format!("{pre_scale};{cut_v}"),
                "[v]",
                "[a]",
                "18",
            )
        } else {
            (
                format!("{pre_scale};{cut_av};[v]{post_v_chain}[vout]"),
                format!("{pre_scale};{cut_v};[v]{post_v_chain}[vout]"),
                "[vout]",
                "[a]",
                "18",
            )
        }
    } else if is_video_note && video_note_needs_split {
        // Multi-circle - create regular cut, circle formatting will be done in to_video_notes_split
        if let Some(spd) = speed {
            let setpts_factor = 1.0 / spd;
            let atempo_filter = build_atempo_filter(spd);

            (
                format!(
                    "{base_filter_av};[v]setpts={}*PTS[vout];[a]{atempo_filter}[aout]",
                    setpts_factor
                ),
                format!("{base_filter_v};[v]setpts={}*PTS[vout]", setpts_factor),
                "[vout]",
                "[aout]",
                "23",
            )
        } else {
            (base_filter_av, base_filter_v, "[v]", "[a]", "23")
        }
    } else if is_ringtone {
        let atempo_filter = speed
            .map(build_atempo_filter)
            .unwrap_or_else(|| "atempo=1.0".to_string());
        // If !has_video, base_filter_av outputs only [a]. If has_video, [v][a].
        // Ringtone uses input [a] for atempo.
        // We need to match output of base_filter

        (
            format!("{base_filter_av};{}[a]{atempo_filter}[aout]", ""), // standard [a] is output by build_cut_filter
            String::new(),
            "[v]",
            "[aout]",
            "23",
        )
    } else if let Some(spd) = speed {
        let setpts_factor = 1.0 / spd;
        let atempo_filter = build_atempo_filter(spd);

        if has_video {
            (
                format!(
                    "{base_filter_av};[v]setpts={}*PTS[vout];[a]{atempo_filter}[aout]",
                    setpts_factor
                ),
                format!("{base_filter_v};[v]setpts={}*PTS[vout]", setpts_factor),
                "[vout]",
                "[aout]",
                "23",
            )
        } else {
            (
                format!("{base_filter_av};[a]{atempo_filter}[aout]"),
                String::new(),
                "",
                "[aout]",
                "23",
            )
        }
    } else {
        (base_filter_av, base_filter_v, "[v]", "[a]", "23")
    };

    ClipFilterPlan {
        filter_av,
        filter_v,
        map_v_label,
        map_a_label,
        crf,
    }
}

/// Max output length (secs) for a clip based on its kind. Ringtones, GIFs and
/// video notes each have their own ceiling; regular cuts cap at 10 minutes.
fn compute_clip_max_len_secs(
    is_video_note: bool,
    video_note_needs_split: bool,
    is_iphone_ringtone: bool,
    is_android_ringtone: bool,
    is_gif: bool,
) -> i64 {
    if is_video_note && !video_note_needs_split {
        VIDEO_NOTE_MAX_DURATION as i64
    } else if is_video_note && video_note_needs_split {
        (VIDEO_NOTE_MAX_DURATION * VIDEO_NOTE_MAX_PARTS as u64) as i64
    } else if is_iphone_ringtone {
        crate::download::ringtone::MAX_IPHONE_DURATION_SECS as i64
    } else if is_android_ringtone {
        crate::download::ringtone::MAX_ANDROID_DURATION_SECS as i64
    } else if is_gif {
        GIF_MAX_DURATION_SECS
    } else {
        60 * 10
    }
}

/// Localized "processing your clip…" status line for the initial bot reply.
/// Keyed by output kind × speed-present.
fn pick_clip_status_message(
    lang: &unic_langid::LanguageIdentifier,
    is_video_note: bool,
    is_ringtone: bool,
    is_gif: bool,
    speed: Option<f32>,
    segments_text: &str,
) -> String {
    if let Some(spd) = speed {
        let args = doracore::fluent_args!("segments" => segments_text, "speed" => spd as f64);
        if is_video_note {
            i18n::t_args(lang, "commands.cut_status_video_note_speed", &args)
        } else if is_ringtone {
            i18n::t_args(lang, "commands.cut_status_ringtone_speed", &args)
        } else if is_gif {
            i18n::t_args(lang, "commands.cut_status_gif_speed", &args)
        } else {
            i18n::t_args(lang, "commands.cut_status_clip_speed", &args)
        }
    } else {
        let args = doracore::fluent_args!("segments" => segments_text);
        if is_video_note {
            i18n::t_args(lang, "commands.cut_status_video_note", &args)
        } else if is_ringtone {
            i18n::t_args(lang, "commands.cut_status_ringtone", &args)
        } else if is_gif {
            i18n::t_args(lang, "commands.cut_status_gif", &args)
        } else {
            i18n::t_args(lang, "commands.cut_status_clip", &args)
        }
    }
}

/// Build `(input_path, output_path)` for a clip job inside the temp dir.
/// Extension + filename pattern are determined by the output kind.
fn build_clip_output_paths(
    tmp: &std::path::Path,
    chat_id: ChatId,
    source_id: i64,
    base_title: &str,
    is_video_note: bool,
    is_iphone_ringtone: bool,
    is_ringtone: bool,
    is_gif: bool,
) -> (std::path::PathBuf, std::path::PathBuf) {
    let input_path = tmp.join(format!("input_{}_{}.mp4", chat_id.0, source_id));
    let output_path = if is_ringtone {
        let safe_title = crate::download::ringtone::sanitize_filename(base_title);
        let ext = if is_iphone_ringtone { "m4r" } else { "mp3" };
        tmp.join(format!("{}_ringtone.{}", safe_title, ext))
    } else {
        tmp.join(format!(
            "{}_{}_{}{}",
            if is_video_note {
                "circle"
            } else if is_gif {
                "gif_tmp"
            } else {
                "cut"
            },
            chat_id.0,
            uuid::Uuid::new_v4(),
            ".mp4"
        ))
    };
    (input_path, output_path)
}

pub async fn process_video_clip(
    bot: Bot,
    db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
    chat_id: ChatId,
    session: db::VideoClipSession,
    segments: Vec<CutSegment>,
    segments_text: String,
    speed: Option<f32>,
) -> Result<(), AppError> {
    use tokio::process::Command;

    let _ = db_pool;
    let lang = i18n::user_lang_from_storage(&shared_storage, chat_id.0).await;
    let total_len: i64 = segments.iter().map(|s| (s.end_secs - s.start_secs).max(0)).sum();
    let is_video_note = session.output_kind == OutputKind::VideoNote;
    let is_iphone_ringtone = session.output_kind == OutputKind::IphoneRingtone;
    let is_android_ringtone = session.output_kind == OutputKind::AndroidRingtone;
    let is_ringtone = is_iphone_ringtone || is_android_ringtone;
    let is_gif = session.output_kind == OutputKind::Gif;

    // Effective duration accounting for speed (e.g., 86s at 2x = 43s)
    let effective_len = if let Some(spd) = speed {
        (total_len as f32 / spd).ceil() as i64
    } else {
        total_len
    };

    // For video notes, determine if we need multi-circle split (using effective duration).
    // Mutable because we re-check after probing the real output file below.
    let mut video_note_needs_split =
        is_video_note && effective_len > VIDEO_NOTE_MAX_DURATION as i64 && !is_too_long_for_split(effective_len as u64);

    // Check if video note is too long for splitting (> 360s)
    if is_video_note && is_too_long_for_split(effective_len as u64) {
        let args = doracore::fluent_args!("max_minutes" => VIDEO_NOTE_MAX_PARTS as i64);
        bot.send_message(
            chat_id,
            i18n::t_args(&lang, "commands.video_note_too_long_for_split", &args),
        )
        .await
        .ok();
        return Ok(());
    }

    let max_len_secs = compute_clip_max_len_secs(
        is_video_note,
        video_note_needs_split,
        is_iphone_ringtone,
        is_android_ringtone,
        is_gif,
    );

    // For ringtones only, truncate segments to fit within limit and notify user
    // Video notes with split don't need truncation
    let (adjusted_segments, truncated) = if (is_ringtone || is_gif) && total_len > max_len_secs {
        let mut adjusted = Vec::new();
        let mut accumulated = 0i64;

        for seg in &segments {
            let seg_len = seg.end_secs - seg.start_secs;
            if accumulated >= max_len_secs {
                break;
            }

            if accumulated + seg_len <= max_len_secs {
                adjusted.push(*seg);
                accumulated += seg_len;
            } else {
                let remaining = max_len_secs - accumulated;
                adjusted.push(CutSegment {
                    start_secs: seg.start_secs,
                    end_secs: seg.start_secs + remaining,
                });
                break;
            }
        }

        (adjusted, true)
    } else if !is_video_note && !is_ringtone && !is_gif && total_len > 600 {
        // For regular cuts, reject if too long (10 min)
        bot.send_message(chat_id, i18n::t(&lang, "commands.cut_too_long"))
            .await
            .ok();
        return Ok(());
    } else {
        (segments.clone(), false)
    };

    // Calculate actual length after truncation
    let mut actual_total_len: i64 = adjusted_segments
        .iter()
        .map(|s| (s.end_secs - s.start_secs).max(0))
        .sum();

    // Effective length after speed for video note split calculations.
    // Both `mut` because we re-compute after probing the real output file below.
    let mut effective_total_len = if let Some(spd) = speed {
        (actual_total_len as f32 / spd).ceil() as i64
    } else {
        actual_total_len
    };

    // Notify user about multi-circle split
    if video_note_needs_split {
        if let Some(split_info) = calculate_video_note_split(effective_total_len as u64) {
            let args = doracore::fluent_args!("count" => split_info.num_parts as i64);
            bot.send_message(chat_id, i18n::t_args(&lang, "commands.video_note_will_split", &args))
                .await
                .ok();
        }
    }

    // Notify user if segments were truncated (ringtones and GIF)
    if truncated {
        let max_secs = if is_iphone_ringtone {
            crate::download::ringtone::MAX_IPHONE_DURATION_SECS as i64
        } else if is_gif {
            GIF_MAX_DURATION_SECS
        } else {
            crate::download::ringtone::MAX_ANDROID_DURATION_SECS as i64
        };
        let limit_text = if is_gif {
            format!("GIF ({}s)", max_secs)
        } else {
            format!("{} ({}s)", i18n::t(&lang, "commands.cut_limit_ringtone"), max_secs)
        };
        let args = doracore::fluent_args!("total" => total_len, "limit" => limit_text, "actual" => actual_total_len);
        bot.send_message(chat_id, i18n::t_args(&lang, "commands.cut_truncated", &args))
            .await
            .ok();
    }

    let ClipSource {
        file_id,
        original_url,
        base_title,
        video_quality,
        fallback_message_id,
        fallback_chat_id,
    } = match resolve_clip_source(&bot, &shared_storage, chat_id, &session, is_ringtone, &lang).await? {
        Some(src) => src,
        None => return Ok(()),
    };

    log::info!(
        "🔍 Source file info: file_id={}, message_id={:?}, chat_id={:?}",
        &file_id[..20.min(file_id.len())],
        fallback_message_id,
        fallback_chat_id
    );

    let status_msg = pick_clip_status_message(&lang, is_video_note, is_ringtone, is_gif, speed, &segments_text);

    let status = bot.send_message(chat_id, status_msg).await?;

    let mut guard = crate::core::utils::TempDirGuard::new("doradura_clip")
        .await
        .map_err(AppError::Io)?;
    log::info!("📂 Temp directory ready: {:?}", guard.path());

    let (input_path, output_path) = build_clip_output_paths(
        guard.path(),
        chat_id,
        session.source_id,
        &base_title,
        is_video_note,
        is_iphone_ringtone,
        is_ringtone,
        is_gif,
    );

    log::info!(
        "🔽 Starting download for video note: file_id={}, output_path={:?}",
        file_id,
        input_path
    );

    // Use download_file_with_fallback for Bot API -> MTProto fallback chain
    let download_result = crate::telegram::download_file_with_fallback(
        &bot,
        &file_id,
        fallback_message_id,
        fallback_chat_id,
        Some(input_path.clone()),
    )
    .await;

    match &download_result {
        Ok(path) => log::info!("✅ Download completed: {:?}", path),
        Err(e) => {
            log::error!("❌ Download failed (all fallbacks exhausted): {}", e);
            bot.delete_message(chat_id, status.id).await.ok();
            bot.send_message(
                chat_id,
                "File download failed. The file may have been deleted or is no longer accessible.",
            )
            .await
            .ok();
            return Ok(());
        }
    }
    let _ = download_result.map_err(AppError::from)?;

    // --- Subtitle handling for circles ---
    // For circles: download SRT only (don't burn yet) — subs will be burned
    // AFTER scale+crop in the ffmpeg filter chain so they render at 640x640.
    let circle_srt_path: Option<std::path::PathBuf> = if is_video_note {
        if let Some(ref sub_lang) = session.subtitle_lang {
            match download_circle_subtitles(
                &session.original_url,
                sub_lang,
                guard.path(),
                chat_id.0,
                session.source_id,
            )
            .await
            {
                BurnSubsResult::SubtitleReady(srt) => Some(srt),
                _ => None,
            }
        } else {
            None
        }
    } else {
        None
    };

    // --- Custom audio for circles ---
    let custom_audio_path = if is_video_note {
        if let Some(ref audio_fid) = session.custom_audio_file_id {
            let audio_path = guard.path().join("custom_audio.tmp");
            match crate::telegram::download_file_with_fallback(&bot, audio_fid, None, None, Some(audio_path.clone()))
                .await
            {
                Ok(_) => {
                    log::info!("✅ Custom audio downloaded: {:?}", audio_path);
                    Some(audio_path)
                }
                Err(e) => {
                    log::warn!("Failed to download custom audio: {}, using original", e);
                    None
                }
            }
        } else {
            None
        }
    } else {
        None
    };

    let actual_input_path = input_path.clone();

    // Probe file for video stream
    let probe_output = crate::core::process::run_with_timeout(
        Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=codec_type",
                "-of",
                "default=noprint_wrappers=1:nokey=1",
            ])
            .arg(&actual_input_path),
        crate::core::process::FFPROBE_TIMEOUT,
    )
    .await?;
    let has_video = !probe_output.stdout.is_empty();

    if is_video_note && !has_video {
        bot.delete_message(chat_id, status.id).await.ok();
        bot.send_message(chat_id, i18n::t(&lang, "commands.video_note_requires_video"))
            .await
            .ok();
        return Ok(());
    }

    // Fast seek: use -ss before -i to skip to near the first segment
    // Subtract 5 seconds for keyframe safety margin
    let seek_offset = adjusted_segments
        .iter()
        .map(|s| s.start_secs)
        .min()
        .unwrap_or(0)
        .saturating_sub(5)
        .max(0);

    let seeked_segments: Vec<CutSegment> = adjusted_segments
        .iter()
        .map(|s| CutSegment {
            start_secs: s.start_secs - seek_offset,
            end_secs: s.end_secs - seek_offset,
        })
        .collect();

    // Build subtitle filter fragment for post-crop burning (640x640 coordinates)
    let circle_sub_filter = build_circle_sub_filter(circle_srt_path.as_deref());

    let ClipFilterPlan {
        filter_av,
        filter_v,
        map_v_label,
        map_a_label,
        crf,
    } = build_clip_filter_plan(
        &seeked_segments,
        has_video,
        is_ringtone,
        is_video_note,
        video_note_needs_split,
        circle_sub_filter.as_deref(),
        speed,
    );

    // Probe source resolution for adaptive encoder preset.
    // Probe source resolution — useful for diagnostics but no longer
    // controls preset selection (we always use `veryslow` for video notes
    // since v0.43.4). Long 4K segments can still OOM on Railway with
    // `veryslow`; the smart-retry path (below) drops to `medium` + same
    // dark-scene params if the first pass crashes.
    let source_is_highres = if has_video && is_video_note {
        let path_str = actual_input_path.to_string_lossy().to_string();
        matches!(
            doracore::download::metadata::probe_video_metadata(&path_str).await,
            Some((_dur, _w, Some(h))) if h >= 1440
        )
    } else {
        false
    };
    if source_is_highres {
        log::info!("🎬 High-res source detected (height ≥ 1440) — primary preset=veryslow, retry preset=medium");
    }

    log::info!("🎬 Starting ffmpeg with filter: {}", filter_av);
    log::info!("🎬 Input: {:?}, Output: {:?}", actual_input_path, output_path);

    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-hide_banner").arg("-loglevel").arg("info");
    // Limit ffmpeg + filter parallelism on video-note encodes. With
    // veryslow on 4K input, x264 spawns one worker per slice and each
    // worker holds its own reference frame / motion vector buffers; on
    // a 4-core Railway worker that's enough to OOM. `-threads 2`
    // halves the per-encode RAM at minimal speed cost (we measured
    // 665 MB vs 771 MB peak on the same 60 s 4K source). The
    // `lookahead-threads=1` x264 param cuts the lookahead worker pool
    // to a single thread for the same reason. Non-video-note paths
    // inherit ffmpeg defaults — they already use ultrafast and don't
    // hit OOM.
    if is_video_note {
        cmd.arg("-threads").arg("2").arg("-filter_threads").arg("2");
    }

    // Fast seek to near the first segment (before -i for input-level seek)
    if seek_offset > 0 {
        cmd.arg("-ss").arg(format!("{}", seek_offset));
    }

    cmd.arg("-i").arg(&actual_input_path);

    // Add custom audio as second input if present (for circle with replaced audio)
    if let Some(ref audio_path) = custom_audio_path {
        cmd.arg("-i").arg(audio_path);
    }

    if is_iphone_ringtone {
        // For iPhone ringtone: AAC in MPEG-4 container (.m4r)
        // -vn strips embedded album art to avoid exit code 234 with -f ipod
        cmd.arg("-vn")
            .arg("-filter_complex")
            .arg(&filter_av)
            .arg("-map")
            .arg(map_a_label)
            .arg("-c:a")
            .arg("aac")
            .arg("-b:a")
            .arg("192k")
            .arg("-f")
            .arg("ipod");
    } else if is_android_ringtone {
        // For Android ringtone: MP3 at 192k
        cmd.arg("-vn")
            .arg("-filter_complex")
            .arg(&filter_av)
            .arg("-map")
            .arg(map_a_label)
            .arg("-c:a")
            .arg("libmp3lame")
            .arg("-b:a")
            .arg("192k")
            .arg("-f")
            .arg("mp3");
    } else if is_gif {
        // GIF: video-only cut, skip audio entirely
        cmd.arg("-filter_complex").arg(&filter_v);
        cmd.arg("-map").arg(map_v_label);
        cmd.arg("-an");
        cmd.arg("-c:v")
            .arg("libx264")
            .arg("-preset")
            .arg("ultrafast")
            .arg("-crf")
            .arg(crf)
            .arg("-movflags")
            .arg("+faststart");
    } else if custom_audio_path.is_some() && is_video_note {
        // Custom audio: use video-only filter, take audio from custom file (input 1)
        cmd.arg("-filter_complex").arg(&filter_v);
        cmd.arg("-map").arg(map_v_label);
        cmd.arg("-map").arg("1:a");
        // Video-note encoding for the custom-audio path mirrors the
        // standard-audio branch below — see that comment for full rationale.
        cmd.arg("-c:v")
            .arg("libx264")
            .arg("-preset")
            .arg("veryslow")
            .arg("-tune")
            .arg("film")
            .arg("-pix_fmt")
            .arg("yuv420p")
            .arg("-crf")
            .arg("16")
            .arg("-maxrate")
            .arg("1500k")
            .arg("-bufsize")
            .arg("3000k")
            .arg("-profile:v")
            .arg("high")
            .arg("-level")
            .arg("4.0")
            .arg("-g")
            .arg("48")
            .arg("-keyint_min")
            .arg("24")
            .arg("-x264-params")
            .arg(video_note_dark_scene().to_arg_string());
        cmd.arg("-c:a")
            .arg("aac")
            .arg("-b:a")
            .arg("96k")
            .arg("-shortest")
            .arg("-movflags")
            .arg("+faststart");
    } else {
        cmd.arg("-filter_complex").arg(&filter_av);
        if has_video {
            cmd.arg("-map").arg(map_v_label);
        }
        cmd.arg("-map").arg(map_a_label);

        if has_video {
            // **Video-note encoding (v0.43.4) — verified empirically by
            // hand-crafting `test_circles/test_small.mp4` with the exact
            // CLI below and confirming on the Telegram client that the
            // resulting circle is sharp.**
            //
            // We had dropped `-profile:v`, `-level`, `-g`, and `-keyint_min`
            // in v0.43.2 on the theory that Telegram normalises them.
            // Empirically that theory was wrong — production circles came
            // out blocky, while the same source encoded with these flags
            // (test_small.mp4) looked correct. So they go back. The most
            // likely mechanism is `-g 48 -keyint_min 24` forcing a 1-2 s
            // keyframe spacing on our intermediate, which gives Telegram's
            // fast-preset transcoder more I-frames to anchor onto.
            //
            // Non-video-notes (regular cuts) stay on `ultrafast` —
            // delivered as full-size mp4s where size > preset matters more.
            let preset = if is_video_note { "veryslow" } else { "ultrafast" };
            cmd.arg("-c:v").arg("libx264").arg("-preset").arg(preset);
            if is_video_note {
                cmd.arg("-tune").arg("film");
            }
            cmd.arg("-pix_fmt").arg("yuv420p");
            if is_video_note {
                cmd.arg("-crf")
                    .arg("16")
                    .arg("-maxrate")
                    .arg("1500k")
                    .arg("-bufsize")
                    .arg("3000k")
                    .arg("-profile:v")
                    .arg("high")
                    .arg("-level")
                    .arg("4.0")
                    .arg("-g")
                    .arg("48")
                    .arg("-keyint_min")
                    .arg("24")
                    .arg("-x264-params")
                    .arg(video_note_dark_scene().to_arg_string());
            } else {
                cmd.arg("-crf").arg(crf);
            }
        }
        let audio_bitrate = if is_video_note { "96k" } else { "192k" };
        cmd.arg("-c:a")
            .arg("aac")
            .arg("-b:a")
            .arg(audio_bitrate)
            .arg("-movflags")
            .arg("+faststart");
    }

    // Video notes use `veryslow` preset which can take 5-15 min on a 4K/8K
    // source. Regular cuts run on `ultrafast` and finish in under a minute.
    // Video notes use `veryslow` preset which can take 5-15 min on a 4K/8K
    // source. Regular cuts run on `ultrafast` and finish in under a minute.
    let ffmpeg_timeout = if is_video_note {
        std::time::Duration::from_secs(20 * 60)
    } else {
        std::time::Duration::from_secs(10 * 60)
    };
    cmd.arg("-y").arg(&output_path);
    let output = match doracore::core::process::run_with_timeout_raw(&mut cmd, ffmpeg_timeout).await {
        Ok(result) => result.map_err(AppError::from)?,
        Err(_) => {
            log::error!("❌ ffmpeg timed out after {} seconds", ffmpeg_timeout.as_secs());
            bot.delete_message(chat_id, status.id).await.ok();
            bot.send_message(
                chat_id,
                "❌ Video processing timed out (10 min limit). Try a shorter segment.",
            )
            .await
            .ok();
            return Ok(());
        }
    };

    log::info!("✅ ffmpeg processing completed with status: {}", output.status);

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        log::warn!(
            "⚠️ first ffmpeg pass failed ({}); retrying with ultrafast + audio preserved",
            output.status
        );

        // Two-stage retry — historically we dropped straight to a video-only
        // filter, which silently produced audioless circles whenever the
        // first pass was killed for memory/time reasons (very common on
        // Railway 4K/8K). Try the audio+video chain at `ultrafast` first;
        // only fall back to video-only if that *also* fails (e.g. real audio
        // mapping issue).
        let build_av_retry = || {
            let mut c = Command::new("ffmpeg");
            c.arg("-hide_banner").arg("-loglevel").arg("error");
            if seek_offset > 0 {
                c.arg("-ss").arg(format!("{}", seek_offset));
            }
            c.arg("-i").arg(&actual_input_path);
            if custom_audio_path.is_some() && is_video_note {
                if let Some(ref audio_path) = custom_audio_path {
                    c.arg("-i").arg(audio_path);
                }
                c.arg("-filter_complex")
                    .arg(&filter_v)
                    .arg("-map")
                    .arg(map_v_label)
                    .arg("-map")
                    .arg("1:a");
            } else {
                c.arg("-filter_complex").arg(&filter_av);
                if has_video {
                    c.arg("-map").arg(map_v_label);
                }
                c.arg("-map").arg(map_a_label);
            }
            // Smart retry-preset: when the first pass crashed with SIGKILL
            // on a high-res video-note (Railway OOM with `veryslow` on a
            // 4K source >30 s), drop to `medium` + the same dark-scene
            // x264 params instead of `ultrafast`. `medium` uses ~3× less
            // RAM than `veryslow` so it fits, and the resulting circle
            // is still close to the verified-good `test_small.mp4` recipe
            // — *not* the shakal output `ultrafast` produced.
            //
            // Non-video-note (regular cuts) keep `ultrafast` since size
            // matters more than preset for full-size mp4 deliveries.
            let retry_preset = if is_video_note { "medium" } else { "ultrafast" };
            c.arg("-c:v").arg("libx264").arg("-preset").arg(retry_preset);
            if is_video_note {
                c.arg("-tune").arg("film").arg("-pix_fmt").arg("yuv420p");
            }
            c.arg("-crf").arg(crf);
            if is_video_note {
                c.arg("-maxrate")
                    .arg("1500k")
                    .arg("-bufsize")
                    .arg("3000k")
                    .arg("-profile:v")
                    .arg("high")
                    .arg("-level")
                    .arg("4.0")
                    .arg("-g")
                    .arg("48")
                    .arg("-keyint_min")
                    .arg("24")
                    .arg("-x264-params")
                    .arg(video_note_dark_scene().to_arg_string());
            }
            let audio_bitrate = if is_video_note { "96k" } else { "128k" };
            c.arg("-c:a").arg("aac").arg("-b:a").arg(audio_bitrate);
            if custom_audio_path.is_some() && is_video_note {
                c.arg("-shortest");
            }
            c.arg("-movflags").arg("+faststart").arg("-y").arg(&output_path);
            c
        };
        let mut av_retry_cmd = build_av_retry();
        let av_retry_output =
            match doracore::core::process::run_with_timeout_raw(&mut av_retry_cmd, ffmpeg_timeout).await {
                Ok(result) => result.map_err(AppError::from)?,
                Err(_) => {
                    log::error!("❌ ffmpeg retry timed out after {} seconds", ffmpeg_timeout.as_secs());
                    bot.delete_message(chat_id, status.id).await.ok();
                    bot.send_message(
                        chat_id,
                        "❌ Video processing timed out (10 min limit). Try a shorter segment.",
                    )
                    .await
                    .ok();
                    return Ok(());
                }
            };

        if !av_retry_output.status.success() {
            log::warn!("⚠️ audio+video retry also failed; falling back to video-only (audioless)");
            let mut retry_cmd = Command::new("ffmpeg");
            retry_cmd.arg("-hide_banner").arg("-loglevel").arg("error");
            if seek_offset > 0 {
                retry_cmd.arg("-ss").arg(format!("{}", seek_offset));
            }
            retry_cmd
                .arg("-i")
                .arg(&actual_input_path)
                .arg("-filter_complex")
                .arg(&filter_v)
                .arg("-map")
                .arg(map_v_label)
                .arg("-c:v")
                .arg("libx264")
                .arg("-preset")
                .arg("ultrafast")
                .arg("-crf")
                .arg(crf);
            if is_video_note {
                retry_cmd.arg("-maxrate").arg("1400k").arg("-bufsize").arg("2800k");
            }
            retry_cmd.arg("-movflags").arg("+faststart").arg("-y").arg(&output_path);
            let retry_output = match doracore::core::process::run_with_timeout_raw(&mut retry_cmd, ffmpeg_timeout).await
            {
                Ok(result) => result.map_err(AppError::from)?,
                Err(_) => {
                    log::error!(
                        "❌ ffmpeg final retry timed out after {} seconds",
                        ffmpeg_timeout.as_secs()
                    );
                    bot.delete_message(chat_id, status.id).await.ok();
                    bot.send_message(
                        chat_id,
                        "❌ Video processing timed out (10 min limit). Try a shorter segment.",
                    )
                    .await
                    .ok();
                    return Ok(());
                }
            };

            if !retry_output.status.success() {
                let stderr2 = String::from_utf8_lossy(&retry_output.stderr);
                bot.delete_message(chat_id, status.id).await.ok();
                let args = doracore::fluent_args!("stderr" => stderr.to_string(), "stderr2" => stderr2.to_string());
                bot.send_message(chat_id, i18n::t_args(&lang, "commands.ffmpeg_error_dual", &args))
                    .await
                    .ok();
                return Ok(());
            }
        }
    }

    let file_size = fs_err::tokio::metadata(&output_path)
        .await
        .map(|m| m.len() as i64)
        .unwrap_or(0);

    // PROBE the ACTUAL output duration instead of trusting the theoretical
    // `actual_total_len`/`effective_total_len` values (which are computed from
    // user input and may diverge from reality when:
    //   - the source file is shorter than the requested trim range
    //   - ffmpeg atempo/setpts produces a slightly different duration
    //   - download.duration in history was wrong/stale
    //
    // Without this probe, to_video_notes_split below tries to seek past EOF
    // and produces garbage circles for the missing tail (observed: 6×60s
    // circles from a 120s source).
    //
    // The ffmpeg cut ALREADY applied the speed modifier via setpts/atempo,
    // so the probed duration IS the effective (post-speed) duration.
    if let Some(probed) = doracore::download::metadata::probe_duration_seconds(&output_path.to_string_lossy()).await {
        let probed = probed as i64;
        if probed > 0 {
            if (probed - effective_total_len).abs() > 2 {
                log::warn!(
                    "⏱ Output duration mismatch: theoretical effective={}s, probed={}s — using probed",
                    effective_total_len,
                    probed
                );
            }
            // Trust the real file: use probed as the effective duration, and
            // back-calculate the raw (speed-adjusted) length for accounting.
            effective_total_len = probed.max(1);
            actual_total_len = if let Some(spd) = speed {
                (effective_total_len as f32 * spd).round() as i64
            } else {
                effective_total_len
            };
            // Recompute split flag from the real length — the file may be
            // shorter than expected and no longer need splitting.
            video_note_needs_split = is_video_note
                && effective_total_len > VIDEO_NOTE_MAX_DURATION as i64
                && !is_too_long_for_split(effective_total_len as u64);
        }
    }

    // Build a timestamped URL linking to the start of the first segment
    let timestamped_url = if !original_url.is_empty() {
        let start_secs = adjusted_segments.first().map(|s| s.start_secs).unwrap_or(0);
        if start_secs > 0 {
            let sep = if original_url.contains('?') { "&" } else { "?" };
            format!("{}{sep}t={start_secs}", original_url)
        } else {
            original_url.clone()
        }
    } else {
        String::new()
    };

    let url_suffix = if timestamped_url.is_empty() {
        String::new()
    } else {
        format!("\n{}", timestamped_url)
    };

    if is_gif {
        send_clip_as_gif(
            &bot,
            chat_id,
            status.id,
            &mut guard,
            &output_path,
            &base_title,
            &segments_text,
            &url_suffix,
            actual_total_len,
            &lang,
        )
        .await;
        return Ok(());
    }

    let (output_kind, clip_title) = if is_video_note {
        (
            "video_note",
            format!("{} [circle {}]{}", base_title, segments_text, url_suffix),
        )
    } else if is_ringtone {
        (
            "ringtone",
            format!("{} [ringtone {}]{}", base_title, segments_text, url_suffix),
        )
    } else {
        ("clip", format!("{} [cut {}]{}", base_title, segments_text, url_suffix))
    };

    // Check output file before sending
    if !output_path.exists() {
        log::error!("❌ Output file does not exist: {:?}", output_path);
        bot.delete_message(chat_id, status.id).await.ok();
        bot.send_message(chat_id, i18n::t(&lang, "commands.output_file_missing"))
            .await
            .ok();
        return Ok(());
    }

    let output_size = fs_err::tokio::metadata(&output_path)
        .await
        .ok()
        .map(|m| m.len())
        .unwrap_or(0);
    log::info!(
        "📤 Sending {} (size: {} bytes, duration: {}s, effective: {}s)",
        if is_video_note { "video note" } else { "video" },
        output_size,
        actual_total_len,
        effective_total_len
    );

    let sent = if is_video_note && video_note_needs_split {
        // Multi-circle: split the cut video into multiple circles and send each
        // Use effective_total_len (speed-adjusted) since ffmpeg already applied speed
        match to_video_notes_split(&output_path, effective_total_len as u64, None).await {
            Ok(circle_paths) => {
                // Track circle files for automatic cleanup by guard
                for path in &circle_paths {
                    guard.track_file(path.clone());
                }
                let total_circles = circle_paths.len();
                log::info!("📤 Sending {} video notes (circles)", total_circles);

                for (i, circle_path) in circle_paths.iter().enumerate() {
                    // Calculate duration for this part (using effective/speed-adjusted length)
                    let part_duration = if i == total_circles - 1 {
                        effective_total_len - (i as i64 * VIDEO_NOTE_MAX_DURATION as i64)
                    } else {
                        VIDEO_NOTE_MAX_DURATION as i64
                    };

                    // Update status message with progress
                    let args = doracore::fluent_args!("current" => (i + 1) as i64, "total" => total_circles as i64);
                    bot.edit_message_text(
                        chat_id,
                        status.id,
                        i18n::t_args(&lang, "commands.video_note_sending_progress", &args),
                    )
                    .await
                    .ok();

                    match bot
                        .send_video_note(chat_id, teloxide::types::InputFile::file(circle_path))
                        .duration(part_duration.max(1) as u32)
                        .length(640)
                        .await
                    {
                        Ok(_) => {}
                        Err(e) => {
                            log::error!("❌ Failed to send video note {}/{}: {}", i + 1, total_circles, e);
                            bot.delete_message(chat_id, status.id).await.ok();
                            let msg = if e.to_string().to_lowercase().contains("file is too big") {
                                i18n::t(&lang, "commands.video_note_too_big")
                            } else {
                                let args = doracore::fluent_args!("error" => e.to_string());
                                i18n::t_args(&lang, "commands.video_note_send_failed", &args)
                            };
                            bot.send_message(chat_id, msg).await.ok();
                            return Ok(());
                        }
                    }
                }

                // Delete status message after successful send
                bot.delete_message(chat_id, status.id).await.ok();

                // Send clip title as separate message
                bot.send_message(chat_id, &clip_title).await.ok();

                // Skip the rest of the function since we handled everything
                // Save to history not needed for multi-circle (complex structure)
                return Ok(());
            }
            Err(e) => {
                log::error!("❌ Failed to split video into circles: {}", e);
                bot.delete_message(chat_id, status.id).await.ok();
                let args = doracore::fluent_args!("error" => e.to_string());
                bot.send_message(chat_id, i18n::t_args(&lang, "commands.video_note_split_failed", &args))
                    .await
                    .ok();
                return Ok(());
            }
        }
    } else if is_video_note {
        // Single circle
        match bot
            .send_video_note(chat_id, teloxide::types::InputFile::file(output_path.clone()))
            .duration(effective_total_len.max(1) as u32)
            .length(640)
            .await
        {
            Ok(m) => m,
            Err(e) => {
                log::error!("❌ Failed to send video note: {}", e);
                bot.delete_message(chat_id, status.id).await.ok();
                let msg = if e.to_string().to_lowercase().contains("file is too big") {
                    i18n::t(&lang, "commands.video_note_too_big")
                } else {
                    let args = doracore::fluent_args!("error" => e.to_string());
                    i18n::t_args(&lang, "commands.video_note_send_failed", &args)
                };
                bot.send_message(chat_id, msg).await.ok();
                return Ok(());
            }
        }
    } else if is_ringtone {
        let lang = i18n::user_lang_from_storage(&shared_storage, chat_id.0).await;
        match bot
            .send_document(chat_id, teloxide::types::InputFile::file(output_path.clone()))
            .caption(escape_markdown(&clip_title))
            .parse_mode(ParseMode::MarkdownV2)
            .await
        {
            Ok(m) => m,
            Err(e) => {
                bot.delete_message(chat_id, status.id).await.ok();
                let args = doracore::fluent_args!("error" => e.to_string());
                bot.send_message(chat_id, i18n::t_args(&lang, "commands.ringtone_send_failed", &args))
                    .await
                    .ok();
                return Ok(());
            }
        };
        // Send platform-specific instructions with images
        let platform = if is_iphone_ringtone {
            crate::telegram::menu::ringtone::Platform::Iphone
        } else {
            crate::telegram::menu::ringtone::Platform::Android
        };
        if let Err(e) = crate::telegram::menu::ringtone::send_ringtone_instructions(
            &bot,
            chat_id,
            platform,
            &db_pool,
            &shared_storage,
        )
        .await
        {
            log::warn!("Failed to send ringtone instructions: {}", e);
        }
        // Clean up files and return early (don't fall through to clip_title logic below)
        bot.delete_message(chat_id, status.id).await.ok();
        return Ok(());
    } else if has_video {
        match bot
            .send_video(chat_id, teloxide::types::InputFile::file(output_path.clone()))
            .caption(&clip_title)
            .await
        {
            Ok(m) => m,
            Err(e) => {
                bot.delete_message(chat_id, status.id).await.ok();
                let args = doracore::fluent_args!("error" => e.to_string());
                bot.send_message(chat_id, i18n::t_args(&lang, "commands.clip_send_failed", &args))
                    .await
                    .ok();
                return Ok(());
            }
        }
    } else {
        match bot
            .send_audio(chat_id, teloxide::types::InputFile::file(output_path.clone()))
            .caption(&clip_title)
            .await
        {
            Ok(m) => m,
            Err(e) => {
                bot.delete_message(chat_id, status.id).await.ok();
                let args = doracore::fluent_args!("error" => e.to_string());
                bot.send_message(chat_id, i18n::t_args(&lang, "commands.audio_send_failed", &args))
                    .await
                    .ok();
                return Ok(());
            }
        }
    };

    if is_video_note {
        bot.send_message(chat_id, clip_title.clone()).await.ok();
    }

    if !is_video_note && !original_url.trim().is_empty() {
        bot.send_message(chat_id, original_url.clone()).await.ok();
    }
    bot.delete_message(chat_id, status.id).await.ok();

    let sent_file_id = if is_video_note {
        sent.video_note().map(|v| v.file.id.0.clone())
    } else if is_ringtone {
        sent.document().map(|d| d.file.id.0.clone())
    } else {
        sent.video()
            .map(|v| v.file.id.0.clone())
            .or_else(|| sent.document().map(|d| d.file.id.0.clone()))
            .or_else(|| sent.audio().map(|a| a.file.id.0.clone()))
    };

    if let Some(fid) = sent_file_id {
        let segments_json = serde_json::to_string(&segments).unwrap_or_else(|_| "[]".to_string());
        if let Err(e) = shared_storage
            .create_cut(
                chat_id.0,
                &original_url,
                session.source_kind.as_str(),
                session.source_id,
                output_kind,
                &segments_json,
                &segments_text,
                &clip_title,
                Some(&fid),
                Some(file_size),
                Some(actual_total_len.max(1)),
                video_quality.as_deref(),
            )
            .await
        {
            log::error!("Failed to persist cut record for user {}: {}", chat_id.0, e);
        }
    }

    // guard drops here, cleaning up the temp dir and tracked files
    Ok(())
}

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
    let output = match doracore::core::process::run_with_timeout_raw(&mut audio_cmd, audio_timeout).await {
        Ok(result) => result.map_err(AppError::from)?,
        Err(_) => {
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
    if file_size > config::validation::max_audio_size_bytes() {
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

pub fn build_cut_filter(segments: &[CutSegment], with_video: bool, with_audio: bool) -> String {
    build_cut_filter_with_input(segments, with_video, with_audio, "0:v", "0:a")
}

/// Like [`build_cut_filter`] but lets the caller pick the upstream stream
/// labels. Used for the single-circle video-note path so we can inject a
/// `scale=640:640` step on `[0:v]` *before* the trim/concat chain runs —
/// downstream filters then buffer 640² frames instead of 4K, dropping
/// pipeline memory by ~36× and avoiding the OOM SIGKILL we observed on
/// long 4K segments at `-preset veryslow`.
pub fn build_cut_filter_with_input(
    segments: &[CutSegment],
    with_video: bool,
    with_audio: bool,
    video_input: &str,
    audio_input: &str,
) -> String {
    let mut parts = Vec::new();
    for (i, seg) in segments.iter().enumerate() {
        if with_video {
            parts.push(format!(
                "[{}]trim=start={}:end={},setpts=PTS-STARTPTS[v{}]",
                video_input, seg.start_secs, seg.end_secs, i
            ));
        }
        if with_audio {
            parts.push(format!(
                "[{}]atrim=start={}:end={},asetpts=PTS-STARTPTS[a{}]",
                audio_input, seg.start_secs, seg.end_secs, i
            ));
        }
    }

    let n = segments.len();
    let mut concat_inputs = String::new();
    for i in 0..n {
        if with_video {
            concat_inputs.push_str(&format!("[v{}]", i));
        }
        if with_audio {
            concat_inputs.push_str(&format!("[a{}]", i));
        }
    }

    let v_count = if with_video { 1 } else { 0 };
    let a_count = if with_audio { 1 } else { 0 };
    let output_labels = format!(
        "{}{}",
        if with_video { "[v]" } else { "" },
        if with_audio { "[a]" } else { "" }
    );

    parts.push(format!(
        "{}concat=n={}:v={}:a={}{}",
        concat_inputs, n, v_count, a_count, output_labels
    ));

    parts.join(";")
}

#[cfg(test)]
mod tests {
    use super::*;

    // ==================== is_cancel_text is in mod.rs ====================

    // ==================== parse_timestamp_secs tests ====================

    #[test]
    fn test_parse_timestamp_secs_mmss() {
        assert_eq!(parse_timestamp_secs("00:00"), Some(0));
        assert_eq!(parse_timestamp_secs("01:30"), Some(90));
        assert_eq!(parse_timestamp_secs("10:00"), Some(600));
        assert_eq!(parse_timestamp_secs("59:59"), Some(3599));
    }

    #[test]
    fn test_parse_timestamp_secs_hhmmss() {
        assert_eq!(parse_timestamp_secs("00:00:00"), Some(0));
        assert_eq!(parse_timestamp_secs("01:00:00"), Some(3600));
        assert_eq!(parse_timestamp_secs("01:30:45"), Some(5445));
        assert_eq!(parse_timestamp_secs("10:15:30"), Some(36930));
    }

    #[test]
    fn test_parse_timestamp_secs_invalid() {
        assert_eq!(parse_timestamp_secs(""), None);
        assert_eq!(parse_timestamp_secs("invalid"), None);
        assert_eq!(parse_timestamp_secs("1:2:3:4"), None);
        assert_eq!(parse_timestamp_secs("00:60"), None); // 60 seconds invalid
        assert_eq!(parse_timestamp_secs("00:-1"), None);
    }

    // ==================== format_timestamp tests ====================

    #[test]
    fn test_format_timestamp_mmss() {
        assert_eq!(format_timestamp(0), "00:00");
        assert_eq!(format_timestamp(30), "00:30");
        assert_eq!(format_timestamp(90), "01:30");
        assert_eq!(format_timestamp(3599), "59:59");
    }

    #[test]
    fn test_format_timestamp_hhmmss() {
        assert_eq!(format_timestamp(3600), "01:00:00");
        assert_eq!(format_timestamp(5445), "01:30:45");
        assert_eq!(format_timestamp(36000), "10:00:00");
    }

    #[test]
    fn test_format_timestamp_negative() {
        // Negative values should be treated as 0
        assert_eq!(format_timestamp(-10), "00:00");
    }

    // ==================== parse_time_range_secs tests ====================

    #[test]
    fn test_parse_time_range_secs_valid() {
        assert_eq!(parse_time_range_secs("00:00-00:30"), Some((0, 30)));
        assert_eq!(parse_time_range_secs("01:00-02:00"), Some((60, 120)));
        assert_eq!(parse_time_range_secs("00:10-01:30:00"), Some((10, 5400)));
    }

    #[test]
    fn test_parse_time_range_secs_special_dashes() {
        // Em dash, en dash, minus sign
        assert_eq!(parse_time_range_secs("00:00\u{2014}00:30"), Some((0, 30)));
        assert_eq!(parse_time_range_secs("00:00\u{2013}00:30"), Some((0, 30)));
        assert_eq!(parse_time_range_secs("00:00\u{2212}00:30"), Some((0, 30)));
    }

    #[test]
    fn test_parse_time_range_secs_with_spaces() {
        assert_eq!(parse_time_range_secs("  00:00 - 00:30  "), Some((0, 30)));
    }

    #[test]
    fn test_parse_time_range_secs_invalid() {
        assert_eq!(parse_time_range_secs("00:30-00:00"), None); // End before start
        assert_eq!(parse_time_range_secs("00:00-00:00"), None); // Same time
        assert_eq!(parse_time_range_secs("invalid"), None);
        assert_eq!(parse_time_range_secs("00:00"), None); // No range
    }

    // ==================== parse_command_segment tests ====================

    #[test]
    fn test_parse_command_segment_full() {
        let result = parse_command_segment("full", Some(120));
        assert!(result.is_some());
        let (start, end, text) = result.unwrap();
        assert_eq!(start, 0);
        assert_eq!(end, 60); // Capped at 60 for video notes
        assert_eq!(text, "00:00-01:00");
    }

    #[test]
    fn test_parse_command_segment_first() {
        let result = parse_command_segment("first30", Some(120));
        assert!(result.is_some());
        let (start, end, _) = result.unwrap();
        assert_eq!(start, 0);
        assert_eq!(end, 30);

        let result = parse_command_segment("first15", Some(120));
        assert!(result.is_some());
        let (start, end, _) = result.unwrap();
        assert_eq!(start, 0);
        assert_eq!(end, 15);
    }

    #[test]
    fn test_parse_command_segment_last() {
        let result = parse_command_segment("last30", Some(120));
        assert!(result.is_some());
        let (start, end, _) = result.unwrap();
        assert_eq!(start, 90);
        assert_eq!(end, 120);
    }

    #[test]
    fn test_parse_command_segment_middle() {
        let result = parse_command_segment("middle30", Some(120));
        assert!(result.is_some());
        let (start, end, _) = result.unwrap();
        assert_eq!(start, 45); // (120-30)/2 = 45
        assert_eq!(end, 75);
    }

    #[test]
    fn test_parse_command_segment_with_speed() {
        // Speed modifier should be stripped for segment parsing
        let result = parse_command_segment("first30 2x", Some(120));
        assert!(result.is_some());
        let (start, end, _) = result.unwrap();
        assert_eq!(start, 0);
        assert_eq!(end, 30);
    }

    #[test]
    fn test_parse_command_segment_invalid() {
        assert!(parse_command_segment("full", None).is_none()); // No duration
        assert!(parse_command_segment("first0", Some(120)).is_none()); // Zero seconds
        assert!(parse_command_segment("first61", Some(120)).is_none()); // Over 60 limit
        assert!(parse_command_segment("invalid", Some(120)).is_none());
    }

    // ==================== parse_speed_modifier tests ====================

    #[test]
    fn test_parse_speed_modifier_suffix_x() {
        assert_eq!(parse_speed_modifier("2x"), Some(2.0));
        assert_eq!(parse_speed_modifier("1.5x"), Some(1.5));
        assert_eq!(parse_speed_modifier("0.5x"), Some(0.5));
    }

    #[test]
    fn test_parse_speed_modifier_prefix_x() {
        assert_eq!(parse_speed_modifier("x2"), Some(2.0));
        assert_eq!(parse_speed_modifier("x1.5"), Some(1.5));
    }

    #[test]
    fn test_parse_speed_modifier_speed_prefix() {
        assert_eq!(parse_speed_modifier("speed2"), Some(2.0));
        assert_eq!(parse_speed_modifier("speed1.5"), Some(1.5));
    }

    #[test]
    fn test_parse_speed_modifier_in_text() {
        assert_eq!(parse_speed_modifier("first30 2x"), Some(2.0));
        assert_eq!(parse_speed_modifier("full speed1.5"), Some(1.5));
    }

    #[test]
    fn test_parse_speed_modifier_invalid() {
        assert_eq!(parse_speed_modifier(""), None);
        assert_eq!(parse_speed_modifier("fast"), None);
        assert_eq!(parse_speed_modifier("3x"), None); // Over 2.0 limit
        assert_eq!(parse_speed_modifier("0x"), None); // Zero not allowed
    }

    // ==================== parse_segments_spec tests ====================

    #[test]
    fn test_parse_segments_spec_time_ranges() {
        let result = parse_segments_spec("00:00-00:30", None);
        assert!(result.is_some());
        let (segments, text, speed) = result.unwrap();
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].start_secs, 0);
        assert_eq!(segments[0].end_secs, 30);
        assert_eq!(text, "00:00-00:30");
        assert!(speed.is_none());
    }

    #[test]
    fn test_parse_segments_spec_multiple_ranges() {
        let result = parse_segments_spec("00:00-00:10, 00:30-00:40", None);
        assert!(result.is_some());
        let (segments, text, _) = result.unwrap();
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].start_secs, 0);
        assert_eq!(segments[0].end_secs, 10);
        assert_eq!(segments[1].start_secs, 30);
        assert_eq!(segments[1].end_secs, 40);
        assert_eq!(text, "00:00-00:10, 00:30-00:40");
    }

    #[test]
    fn test_parse_segments_spec_command() {
        let result = parse_segments_spec("first30", Some(120));
        assert!(result.is_some());
        let (segments, _, _) = result.unwrap();
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].start_secs, 0);
        assert_eq!(segments[0].end_secs, 30);
    }

    #[test]
    fn test_parse_segments_spec_with_speed() {
        let result = parse_segments_spec("first30 2x", Some(120));
        assert!(result.is_some());
        let (_, _, speed) = result.unwrap();
        assert_eq!(speed, Some(2.0));
    }

    #[test]
    fn test_parse_segments_spec_invalid() {
        assert!(parse_segments_spec("", None).is_none());
        assert!(parse_segments_spec("invalid", None).is_none());
    }

    // ==================== parse_audio_segments_spec tests ====================

    #[test]
    fn test_parse_audio_segments_spec_time_range() {
        let result = parse_audio_segments_spec("00:00-01:00", None);
        assert!(result.is_some());
        let (segments, text) = result.unwrap();
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].start_secs, 0);
        assert_eq!(segments[0].end_secs, 60);
        assert_eq!(text, "00:00-01:00");
    }

    #[test]
    fn test_parse_audio_segments_spec_full() {
        let result = parse_audio_segments_spec("full", Some(300));
        assert!(result.is_some());
        let (segments, _) = result.unwrap();
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].start_secs, 0);
        assert_eq!(segments[0].end_secs, 300); // Full duration, no cap
    }

    #[test]
    fn test_parse_audio_segments_spec_first() {
        let result = parse_audio_segments_spec("first60", Some(300));
        assert!(result.is_some());
        let (segments, _) = result.unwrap();
        assert_eq!(segments[0].start_secs, 0);
        assert_eq!(segments[0].end_secs, 60);
    }

    // ==================== parse_audio_command_segment tests ====================

    #[test]
    fn test_parse_audio_command_segment_full() {
        let result = parse_audio_command_segment("full", Some(300));
        assert!(result.is_some());
        let (start, end, _) = result.unwrap();
        assert_eq!(start, 0);
        assert_eq!(end, 300);
    }

    #[test]
    fn test_parse_audio_command_segment_first() {
        let result = parse_audio_command_segment("first60", Some(300));
        assert!(result.is_some());
        let (start, end, _) = result.unwrap();
        assert_eq!(start, 0);
        assert_eq!(end, 60);
    }

    #[test]
    fn test_parse_audio_command_segment_last() {
        let result = parse_audio_command_segment("last60", Some(300));
        assert!(result.is_some());
        let (start, end, _) = result.unwrap();
        assert_eq!(start, 240);
        assert_eq!(end, 300);
    }

    #[test]
    fn test_parse_audio_command_segment_middle() {
        let result = parse_audio_command_segment("middle60", Some(300));
        assert!(result.is_some());
        let (start, end, _) = result.unwrap();
        assert_eq!(start, 120); // (300-60)/2 = 120
        assert_eq!(end, 180);
    }

    #[test]
    fn test_parse_audio_command_segment_no_duration() {
        assert!(parse_audio_command_segment("full", None).is_none());
    }

    // ==================== build_cut_filter tests ====================

    #[test]
    fn test_build_cut_filter_single_segment_video_audio() {
        let segments = vec![CutSegment {
            start_secs: 0,
            end_secs: 30,
        }];
        let filter = build_cut_filter(&segments, true, true);
        assert!(filter.contains("[0:v]trim=start=0:end=30"));
        assert!(filter.contains("[0:a]atrim=start=0:end=30"));
        assert!(filter.contains("concat=n=1:v=1:a=1[v][a]"));
    }

    #[test]
    fn test_build_cut_filter_video_only() {
        let segments = vec![CutSegment {
            start_secs: 10,
            end_secs: 40,
        }];
        let filter = build_cut_filter(&segments, true, false);
        assert!(filter.contains("[0:v]trim=start=10:end=40"));
        assert!(!filter.contains("[0:a]atrim"));
        assert!(filter.contains("concat=n=1:v=1:a=0[v]"));
    }

    #[test]
    fn test_build_cut_filter_audio_only() {
        let segments = vec![CutSegment {
            start_secs: 0,
            end_secs: 60,
        }];
        let filter = build_cut_filter(&segments, false, true);
        assert!(!filter.contains("[0:v]trim"));
        assert!(filter.contains("[0:a]atrim=start=0:end=60"));
        assert!(filter.contains("concat=n=1:v=0:a=1[a]"));
    }

    #[test]
    fn test_build_cut_filter_multiple_segments() {
        let segments = vec![
            CutSegment {
                start_secs: 0,
                end_secs: 10,
            },
            CutSegment {
                start_secs: 30,
                end_secs: 40,
            },
        ];
        let filter = build_cut_filter(&segments, true, true);
        assert!(filter.contains("[0:v]trim=start=0:end=10"));
        assert!(filter.contains("[0:v]trim=start=30:end=40"));
        assert!(filter.contains("[v0][a0][v1][a1]concat=n=2"));
    }

    // ==================== CutSegment serialization tests ====================

    #[test]
    fn test_cut_segment_serialize() {
        let segment = CutSegment {
            start_secs: 10,
            end_secs: 30,
        };
        let json = serde_json::to_string(&segment).unwrap();
        assert!(json.contains("\"start_secs\":10"));
        assert!(json.contains("\"end_secs\":30"));
    }

    // ==================== parse_download_time_range tests ====================

    #[test]
    fn test_parse_download_time_range_basic() {
        let text = "https://youtu.be/abc123 00:01:00-00:02:30";
        let url = "https://youtu.be/abc123";
        let result = parse_download_time_range(text, url);
        assert_eq!(result, Some(("00:01:00".to_string(), "00:02:30".to_string(), None)));
    }

    #[test]
    fn test_parse_download_time_range_mmss() {
        let text = "https://youtu.be/abc123 01:00-02:30";
        let url = "https://youtu.be/abc123";
        let result = parse_download_time_range(text, url);
        assert_eq!(result, Some(("01:00".to_string(), "02:30".to_string(), None)));
    }

    #[test]
    fn test_parse_download_time_range_em_dash() {
        let text = "https://youtu.be/abc123 01:00\u{2014}02:30";
        let url = "https://youtu.be/abc123";
        let result = parse_download_time_range(text, url);
        assert_eq!(result, Some(("01:00".to_string(), "02:30".to_string(), None)));
    }

    #[test]
    fn test_parse_download_time_range_en_dash() {
        let text = "https://youtu.be/abc123 01:00\u{2013}02:30";
        let url = "https://youtu.be/abc123";
        let result = parse_download_time_range(text, url);
        assert_eq!(result, Some(("01:00".to_string(), "02:30".to_string(), None)));
    }

    #[test]
    fn test_parse_download_time_range_no_range() {
        let text = "https://youtu.be/abc123";
        let url = "https://youtu.be/abc123";
        let result = parse_download_time_range(text, url);
        assert_eq!(result, None);
    }

    #[test]
    fn test_parse_download_time_range_invalid_order() {
        let text = "https://youtu.be/abc123 02:30-01:00";
        let url = "https://youtu.be/abc123";
        let result = parse_download_time_range(text, url);
        assert_eq!(result, None);
    }

    #[test]
    fn test_parse_download_time_range_equal_times() {
        let text = "https://youtu.be/abc123 01:00-01:00";
        let url = "https://youtu.be/abc123";
        let result = parse_download_time_range(text, url);
        assert_eq!(result, None);
    }

    #[test]
    fn test_parse_download_time_range_extra_text_after() {
        let text = "https://youtu.be/abc123 00:10-00:30 some extra text";
        let url = "https://youtu.be/abc123";
        let result = parse_download_time_range(text, url);
        assert_eq!(result, Some(("00:10".to_string(), "00:30".to_string(), None)));
    }

    #[test]
    fn test_parse_download_time_range_with_speed() {
        let text = "https://youtu.be/abc123 2:48:45-2:49:59 2x";
        let url = "https://youtu.be/abc123";
        let result = parse_download_time_range(text, url);
        assert_eq!(result, Some(("2:48:45".to_string(), "2:49:59".to_string(), Some(2.0))));
    }

    #[test]
    fn test_parse_download_time_range_with_speed_1_5x() {
        let text = "https://youtu.be/abc123 00:10-00:30 1.5x";
        let url = "https://youtu.be/abc123";
        let result = parse_download_time_range(text, url);
        assert_eq!(result, Some(("00:10".to_string(), "00:30".to_string(), Some(1.5))));
    }

    #[test]
    fn test_parse_download_time_range_garbage_after_url() {
        let text = "https://youtu.be/abc123 hello world";
        let url = "https://youtu.be/abc123";
        let result = parse_download_time_range(text, url);
        assert_eq!(result, None);
    }
}
