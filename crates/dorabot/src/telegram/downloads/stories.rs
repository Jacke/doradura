//! Instagram Stories: reformat a downloaded clip into a vertical 9:16 canvas
//! (1080×1920) and split it into short story segments, each sent as a separate
//! playable portrait video.
//!
//! Flow: `downloads:stories:{download_id}` button → a small config card lets the
//! user pick the reframe mode (blurred fill vs crop-to-fill), the segment length
//! (15/30/60 s) and the quality (standard vs maximum). Pressing "Create" resolves
//! the MP4 download → downloads the source file (Bot API → MTProto fallback) → one
//! ffmpeg pass that reframes to 9:16 and cuts the result into segments via the
//! `segment` muxer with forced keyframes at each boundary → sends every segment as
//! a portrait video.
//!
//! Self-contained on purpose: the existing `process_video_clip` pipeline is
//! heavily specialised for circles/ringtones/GIFs, so threading a Stories
//! `OutputKind` through it would touch many fragile branches. This module only
//! reuses the shared download/ffmpeg/send helpers.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardMarkup, InputFile};
use tokio::process::Command;

use crate::core::escape_markdown;
use crate::i18n;
use crate::storage::SharedStorage;
use crate::telegram::Bot;
use crate::telegram::BotExt;

use super::CallbackCtx;

/// Story canvas — Instagram's native 9:16 portrait resolution.
const STORY_W: u32 = 1080;
const STORY_H: u32 = 1920;
/// Hard ceiling on source length we'll process — keeps the encode bounded so a
/// long clip can't run away on a shared box. Trim-from-start past this, with a
/// warning. 20 min stays within [`STORIES_FFMPEG_TIMEOUT`] even at preset slow.
const MAX_TOTAL_SECS: i64 = 1200; // 20 min
/// ffmpeg wall-clock timeout for the transform + segment pass.
const STORIES_FFMPEG_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// How the source is fit into the 9:16 frame.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Reframe {
    /// Whole clip visible (letterbox), gaps filled with a blurred copy.
    Blur,
    /// Zoom to fill the frame, cropping the edges — the "native" Stories look.
    Crop,
}

/// Encode quality preset.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Quality {
    /// CRF 20, preset medium, AAC 192k — fast, good.
    Std,
    /// CRF 18, preset slow, AAC 256k — slower, best.
    Max,
}

/// How each segment is delivered to the chat.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Delivery {
    /// `sendVideo` — playable inline, but Telegram may re-compress.
    Video,
    /// `sendDocument` — the raw .mp4 file, untouched (best for re-uploading to
    /// Instagram without Telegram's recompression).
    Document,
}

/// Resolved Story render settings, encoded into callback data as a compact
/// `<mode><seg><quality><delivery>` token (e.g. `b60sv`, `c30mf`). The trailing
/// delivery char is optional on parse so legacy `<mode><seg><quality>` tokens
/// (e.g. `b60s`) still decode — to `Delivery::Video`, the old behaviour.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct StorySettings {
    reframe: Reframe,
    seg_secs: u32,
    quality: Quality,
    delivery: Delivery,
}

impl Default for StorySettings {
    /// Matches the pre-enhancement behaviour, so the one-tap path is unchanged.
    fn default() -> Self {
        Self {
            reframe: Reframe::Blur,
            seg_secs: 60,
            quality: Quality::Std,
            delivery: Delivery::Video,
        }
    }
}

impl StorySettings {
    /// Allowed segment lengths offered in the UI.
    const SEG_CHOICES: [u32; 3] = [15, 30, 60];

    /// Parse the compact `<mode><seg><quality><delivery>` token positionally;
    /// tolerant — unknown/missing pieces fall back to [`Default`] so a malformed
    /// callback never panics or 500s. A missing trailing delivery char (legacy
    /// `<mode><seg><quality>` tokens) decodes to [`Delivery::Video`].
    fn parse(flags: &str) -> Self {
        let mut s = Self::default();
        let chars: Vec<char> = flags.chars().collect();
        let mut i = 0;

        // mode
        match chars.get(i) {
            Some('c') => {
                s.reframe = Reframe::Crop;
                i += 1;
            }
            Some('b') => {
                s.reframe = Reframe::Blur;
                i += 1;
            }
            _ => {}
        }

        // segment length digits
        let mut seg_str = String::new();
        while let Some(c) = chars.get(i).filter(|c| c.is_ascii_digit()) {
            seg_str.push(*c);
            i += 1;
        }
        if let Ok(seg) = seg_str.parse::<u32>()
            && Self::SEG_CHOICES.contains(&seg)
        {
            s.seg_secs = seg;
        }

        // quality
        match chars.get(i) {
            Some('m') => {
                s.quality = Quality::Max;
                i += 1;
            }
            Some('s') => {
                s.quality = Quality::Std;
                i += 1;
            }
            _ => {}
        }

        // delivery (optional; absent in legacy tokens → Video)
        match chars.get(i) {
            Some('f') => s.delivery = Delivery::Document,
            Some('v') => s.delivery = Delivery::Video,
            _ => {}
        }

        s
    }

    /// Encode back to the compact `<mode><seg><quality><delivery>` token.
    fn encode(&self) -> String {
        let mode = if self.reframe == Reframe::Crop { 'c' } else { 'b' };
        let q = if self.quality == Quality::Max { 'm' } else { 's' };
        let d = if self.delivery == Delivery::Document { 'f' } else { 'v' };
        format!("{}{}{}{}", mode, self.seg_secs, q, d)
    }

    fn with_reframe(mut self, r: Reframe) -> Self {
        self.reframe = r;
        self
    }
    fn with_seg(mut self, seg: u32) -> Self {
        self.seg_secs = seg;
        self
    }
    fn with_quality(mut self, q: Quality) -> Self {
        self.quality = q;
        self
    }
    fn with_delivery(mut self, d: Delivery) -> Self {
        self.delivery = d;
        self
    }
}

/// Convert any `Display` error into a `teloxide::RequestError` for `?`.
fn to_req_err(e: impl std::fmt::Display) -> teloxide::RequestError {
    teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string())))
}

pub(super) async fn handle(ctx: &CallbackCtx, action: &str, parts: &[&str]) -> ResponseResult<()> {
    if action != "stories" || parts.len() < 3 {
        return Ok(());
    }

    // Two shapes share the `stories` action:
    //   downloads:stories:{id}              → open the config card (entry)
    //   downloads:stories:cfg:{id}:{flags}  → re-render the card after a toggle
    //   downloads:stories:go:{id}:{flags}   → run the render with {flags}
    match parts[2] {
        "cfg" => {
            if parts.len() < 5 {
                return Ok(());
            }
            let download_id = parts[3].parse::<i64>().unwrap_or(0);
            let settings = StorySettings::parse(parts[4]);
            render_config_card(ctx, download_id, settings, true).await
        }
        "go" => {
            if parts.len() < 5 {
                return Ok(());
            }
            let download_id = parts[3].parse::<i64>().unwrap_or(0);
            let settings = StorySettings::parse(parts[4]);
            start_render(ctx, download_id, settings).await
        }
        // Entry: numeric download id → open card with defaults.
        id_str => {
            let download_id = id_str.parse::<i64>().unwrap_or(0);
            render_config_card(ctx, download_id, StorySettings::default(), false).await
        }
    }
}

/// Show (or live-update) the Stories config card.
async fn render_config_card(
    ctx: &CallbackCtx,
    download_id: i64,
    settings: StorySettings,
    edit_in_place: bool,
) -> ResponseResult<()> {
    let Some(download) = ctx
        .shared_storage
        .get_download_history_entry(ctx.chat_id.0, download_id)
        .await
        .map_err(to_req_err)?
    else {
        return Ok(());
    };

    let lang = i18n::user_lang_from_storage(&ctx.shared_storage, ctx.chat_id.0).await;

    // Guard rails: Stories only make sense for video.
    if download.format != "mp4" {
        ctx.bot
            .send_md(ctx.chat_id, i18n::t(&lang, "stories-only-mp4"))
            .await
            .ok();
        return Ok(());
    }
    if download.file_id.is_none() {
        ctx.bot
            .send_md(ctx.chat_id, i18n::t(&lang, "stories-no-file-id"))
            .await
            .ok();
        return Ok(());
    }

    let esc_title = escape_markdown(&download.title);
    let title = i18n::t_args(
        &lang,
        "stories-config-title",
        &doracore::fluent_args!("title" => esc_title.as_str()),
    );
    let keyboard = build_config_keyboard(&lang, download_id, settings);

    if edit_in_place {
        // Ignore "message is not modified" / already-gone errors.
        ctx.bot
            .edit_md_kb(ctx.chat_id, ctx.message_id, title, keyboard)
            .await
            .ok();
    } else {
        ctx.bot.send_md_kb(ctx.chat_id, title, keyboard).await?;
        ctx.bot.try_delete(ctx.chat_id, ctx.message_id).await;
    }
    Ok(())
}

/// Build the toggle keyboard. Every button carries the *resulting* flags so the
/// card is fully stateless — no DB, no session.
fn build_config_keyboard(lang: &unic_langid::LanguageIdentifier, id: i64, s: StorySettings) -> InlineKeyboardMarkup {
    let mark = |active: bool| if active { "● " } else { "○ " };
    let cfg = |next: StorySettings| format!("downloads:stories:cfg:{}:{}", id, next.encode());

    let rows = vec![
        // Reframe mode.
        vec![
            crate::telegram::cb(
                format!(
                    "{}{}",
                    mark(s.reframe == Reframe::Blur),
                    i18n::t(lang, "stories-mode-blur")
                ),
                cfg(s.with_reframe(Reframe::Blur)),
            ),
            crate::telegram::cb(
                format!(
                    "{}{}",
                    mark(s.reframe == Reframe::Crop),
                    i18n::t(lang, "stories-mode-crop")
                ),
                cfg(s.with_reframe(Reframe::Crop)),
            ),
        ],
        // Segment length.
        StorySettings::SEG_CHOICES
            .iter()
            .map(|&seg| crate::telegram::cb(format!("{}{}s", mark(s.seg_secs == seg), seg), cfg(s.with_seg(seg))))
            .collect(),
        // Quality.
        vec![
            crate::telegram::cb(
                format!(
                    "{}{}",
                    mark(s.quality == Quality::Std),
                    i18n::t(lang, "stories-quality-std")
                ),
                cfg(s.with_quality(Quality::Std)),
            ),
            crate::telegram::cb(
                format!(
                    "{}{}",
                    mark(s.quality == Quality::Max),
                    i18n::t(lang, "stories-quality-max")
                ),
                cfg(s.with_quality(Quality::Max)),
            ),
        ],
        // Delivery: playable video vs raw file (document).
        vec![
            crate::telegram::cb(
                format!(
                    "{}{}",
                    mark(s.delivery == Delivery::Video),
                    i18n::t(lang, "stories-delivery-video")
                ),
                cfg(s.with_delivery(Delivery::Video)),
            ),
            crate::telegram::cb(
                format!(
                    "{}{}",
                    mark(s.delivery == Delivery::Document),
                    i18n::t(lang, "stories-delivery-file")
                ),
                cfg(s.with_delivery(Delivery::Document)),
            ),
        ],
        // Run.
        vec![crate::telegram::cb(
            i18n::t(lang, "stories-create"),
            format!("downloads:stories:go:{}:{}", id, s.encode()),
        )],
        // Cancel.
        vec![crate::telegram::cb(
            i18n::t(lang, "stories-cancel"),
            "downloads:cancel".to_string(),
        )],
    ];
    InlineKeyboardMarkup::new(rows)
}

/// Validate the download, then kick the heavy work off detached so the callback
/// returns immediately.
async fn start_render(ctx: &CallbackCtx, download_id: i64, settings: StorySettings) -> ResponseResult<()> {
    let Some(download) = ctx
        .shared_storage
        .get_download_history_entry(ctx.chat_id.0, download_id)
        .await
        .map_err(to_req_err)?
    else {
        return Ok(());
    };

    let lang = i18n::user_lang_from_storage(&ctx.shared_storage, ctx.chat_id.0).await;

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

    // Replace the config card with progress feedback.
    ctx.bot.try_delete(ctx.chat_id, ctx.message_id).await;

    let bot = ctx.bot.clone();
    let shared_storage = ctx.shared_storage.clone();
    let chat_id = ctx.chat_id;
    let title = download.title.clone();
    tokio::spawn(async move {
        if let Err(e) = run_stories(bot, shared_storage, chat_id, download_id, file_id, title, settings).await {
            log::error!("stories: processing failed for download {}: {}", download_id, e);
        }
    });

    Ok(())
}

/// Download the source MP4, render it to vertical 9:16 with the chosen reframe
/// mode + quality, split into [`StorySettings::seg_secs`] segments and send each
/// as a portrait video.
#[allow(clippy::too_many_arguments)]
async fn run_stories(
    bot: Bot,
    shared_storage: Arc<SharedStorage>,
    chat_id: ChatId,
    download_id: i64,
    file_id: String,
    title: String,
    settings: StorySettings,
) -> ResponseResult<()> {
    let lang = i18n::user_lang_from_storage(&shared_storage, chat_id.0).await;
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
    let mut cmd = build_stories_cmd(&input_path, &output_pattern, capped, settings);

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

        // Video → playable inline (Telegram may recompress). Document → the raw
        // .mp4 untouched (best for re-uploading to Instagram). Portrait video
        // omits an explicit thumbnail (Telegram auto-generates one matching the
        // actual frame orientation; see download/send.rs notes).
        let send_result = match settings.delivery {
            Delivery::Video => {
                let mut req = bot
                    .send_video(chat_id, InputFile::file(seg.clone()))
                    .caption(caption)
                    .width(STORY_W)
                    .height(STORY_H);
                if let Some(d) = dur {
                    req = req.duration(d);
                }
                req.await.map(|_| ())
            }
            Delivery::Document => bot
                .send_document(chat_id, InputFile::file(seg.clone()))
                .caption(caption)
                .await
                .map(|_| ()),
        };

        match send_result {
            Ok(()) => sent += 1,
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

/// Build the ffmpeg command: reframe the clip into the 9:16 frame (blurred fill
/// or crop-to-fill), then cut into [`StorySettings::seg_secs`]-long MP4 segments
/// with keyframes forced at each boundary so the segment muxer splits cleanly.
fn build_stories_cmd(
    input: &std::path::Path,
    output_pattern: &std::path::Path,
    capped: bool,
    settings: StorySettings,
) -> Command {
    let filter = match settings.reframe {
        // [bg] = source scaled to *cover* the frame, cropped, heavily blurred and
        //        slightly darkened so the centred foreground pops.
        // [fg] = source scaled to *fit* inside the frame (full clip visible).
        Reframe::Blur => format!(
            "[0:v]split=2[bg][fg];\
             [bg]scale={w}:{h}:force_original_aspect_ratio=increase,crop={w}:{h},boxblur=28:2,eq=brightness=-0.07[bg];\
             [fg]scale={w}:{h}:force_original_aspect_ratio=decrease[fg];\
             [bg][fg]overlay=(W-w)/2:(H-h)/2,setsar=1[v]",
            w = STORY_W,
            h = STORY_H,
        ),
        // Zoom to fill the whole frame, cropping the overflowing edges.
        Reframe::Crop => format!(
            "[0:v]scale={w}:{h}:force_original_aspect_ratio=increase,crop={w}:{h},setsar=1[v]",
            w = STORY_W,
            h = STORY_H,
        ),
    };

    let (crf, preset, audio_bitrate) = match settings.quality {
        Quality::Std => ("20", "medium", "192k"),
        Quality::Max => ("18", "slow", "256k"),
    };

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
        .arg(preset)
        .arg("-crf")
        .arg(crf)
        .arg("-pix_fmt")
        .arg("yuv420p")
        .arg("-r")
        .arg("30")
        // Force a keyframe at every segment boundary for clean cuts.
        .arg("-force_key_frames")
        .arg(format!("expr:gte(t,n_forced*{})", settings.seg_secs))
        .arg("-c:a")
        .arg("aac")
        .arg("-b:a")
        .arg(audio_bitrate)
        .arg("-ar")
        .arg("44100")
        .arg("-f")
        .arg("segment")
        .arg("-segment_time")
        .arg(settings.seg_secs.to_string())
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

    fn args_of(settings: StorySettings, capped: bool) -> Vec<String> {
        let cmd = build_stories_cmd(
            std::path::Path::new("/tmp/in.mp4"),
            std::path::Path::new("/tmp/story_%03d.mp4"),
            capped,
            settings,
        );
        cmd.as_std()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    fn arg_after(args: &[String], flag: &str) -> Option<String> {
        args.iter().position(|a| a == flag).map(|i| args[i + 1].clone())
    }

    #[test]
    fn story_canvas_is_9_by_16() {
        assert_eq!(STORY_W * 16, STORY_H * 9);
    }

    #[test]
    fn default_settings_match_legacy() {
        let s = StorySettings::default();
        assert_eq!(s.reframe, Reframe::Blur);
        assert_eq!(s.seg_secs, 60);
        assert_eq!(s.quality, Quality::Std);
        assert_eq!(s.delivery, Delivery::Video);
        // Encode now carries the trailing delivery char.
        assert_eq!(s.encode(), "b60sv");
    }

    #[test]
    fn flags_round_trip() {
        for token in ["b60sv", "c30mf", "b15sv", "c60mv", "c15sf"] {
            assert_eq!(StorySettings::parse(token).encode(), token);
        }
    }

    #[test]
    fn legacy_tokens_without_delivery_decode_to_video() {
        // Old `<mode><seg><quality>` callbacks must still work → Video.
        for token in ["b60s", "c30m", "b15s", "c60m"] {
            assert_eq!(StorySettings::parse(token).delivery, Delivery::Video);
        }
        assert_eq!(StorySettings::parse("c30m").quality, Quality::Max);
        assert_eq!(StorySettings::parse("c30m").seg_secs, 30);
    }

    #[test]
    fn delivery_toggle_parses_and_encodes() {
        assert_eq!(StorySettings::parse("b60sf").delivery, Delivery::Document);
        assert_eq!(StorySettings::parse("b60sv").delivery, Delivery::Video);
        assert_eq!(
            StorySettings::default().with_delivery(Delivery::Document).encode(),
            "b60sf"
        );
    }

    #[test]
    fn flags_parse_is_tolerant() {
        // Junk falls back to defaults; unknown segment ignored.
        assert_eq!(StorySettings::parse(""), StorySettings::default());
        assert_eq!(StorySettings::parse("x99y").seg_secs, 60);
        assert_eq!(StorySettings::parse("c45s").seg_secs, 60); // 45 not allowed
        assert_eq!(StorySettings::parse("c45s").reframe, Reframe::Crop);
    }

    #[test]
    fn blur_filter_has_blur_and_split() {
        let args = args_of(StorySettings::default(), false);
        let filter = args.iter().find(|a| a.contains("overlay")).expect("filter present");
        assert!(filter.contains("1080") && filter.contains("1920"));
        assert!(filter.contains("boxblur"));
        assert!(filter.contains("split=2"));
    }

    #[test]
    fn crop_filter_crops_without_blur() {
        let s = StorySettings::default().with_reframe(Reframe::Crop);
        let args = args_of(s, false);
        let filter = args
            .iter()
            .position(|a| a == "-filter_complex")
            .map(|i| args[i + 1].clone())
            .expect("filter present");
        assert!(filter.contains("crop=1080:1920"));
        assert!(filter.contains("force_original_aspect_ratio=increase"));
        assert!(!filter.contains("boxblur"));
        assert!(!filter.contains("split"));
    }

    #[test]
    fn std_quality_args() {
        let args = args_of(StorySettings::default().with_quality(Quality::Std), false);
        assert_eq!(arg_after(&args, "-crf").as_deref(), Some("20"));
        assert_eq!(arg_after(&args, "-preset").as_deref(), Some("medium"));
        assert_eq!(arg_after(&args, "-b:a").as_deref(), Some("192k"));
    }

    #[test]
    fn max_quality_args() {
        let args = args_of(StorySettings::default().with_quality(Quality::Max), false);
        assert_eq!(arg_after(&args, "-crf").as_deref(), Some("18"));
        assert_eq!(arg_after(&args, "-preset").as_deref(), Some("slow"));
        assert_eq!(arg_after(&args, "-b:a").as_deref(), Some("256k"));
    }

    #[test]
    fn segment_length_propagates() {
        for seg in StorySettings::SEG_CHOICES {
            let args = args_of(StorySettings::default().with_seg(seg), false);
            assert_eq!(
                arg_after(&args, "-segment_time").as_deref(),
                Some(seg.to_string().as_str())
            );
            let kf = arg_after(&args, "-force_key_frames").expect("force_key_frames present");
            assert!(kf.contains(&format!("n_forced*{}", seg)));
        }
    }

    #[test]
    fn segment_muxer_configured() {
        let args = args_of(StorySettings::default(), false);
        assert_eq!(arg_after(&args, "-f").as_deref(), Some("segment"));
        assert_eq!(arg_after(&args, "-reset_timestamps").as_deref(), Some("1"));
    }

    #[test]
    fn capped_adds_duration_limit() {
        let args = args_of(StorySettings::default(), true);
        assert_eq!(
            arg_after(&args, "-t").as_deref(),
            Some(MAX_TOTAL_SECS.to_string().as_str())
        );
    }

    #[test]
    fn uncapped_has_no_duration_limit() {
        let args = args_of(StorySettings::default(), false);
        assert!(!args.iter().any(|a| a == "-t"));
    }
}
