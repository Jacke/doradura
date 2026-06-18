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

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardMarkup, InputFile, InputMedia, InputMediaPhoto};
use tokio::process::Command;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio::time::timeout;

use crate::core::escape_markdown;
use crate::i18n;
use crate::storage::SharedStorage;
use crate::telegram::Bot;
use crate::telegram::BotExt;

use super::CallbackCtx;

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

/// Target aspect ratio for the reframed clip. Dimensions are the 1080-base
/// canvas; `Original` keeps the source frame untouched (no reframe → enables a
/// stream-copy fast path in the render).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum AspectRatio {
    /// 9:16 vertical — Stories/Reels/Shorts (1080×1920). The historical default.
    R9x16,
    /// 1:1 square — classic feed (1080×1080).
    R1x1,
    /// 4:5 portrait — tallest feed post (1080×1350).
    R4x5,
    /// 16:9 landscape (1920×1080).
    R16x9,
    /// Source aspect ratio, no reframe.
    Original,
}

impl AspectRatio {
    /// All selectable ratios, in display order.
    const ALL: [AspectRatio; 5] = [
        AspectRatio::R9x16,
        AspectRatio::R1x1,
        AspectRatio::R4x5,
        AspectRatio::R16x9,
        AspectRatio::Original,
    ];

    /// Canvas dimensions, or `None` for `Original` (keep source dims, no reframe).
    fn dims(self) -> Option<(u32, u32)> {
        match self {
            AspectRatio::R9x16 => Some((1080, 1920)),
            AspectRatio::R1x1 => Some((1080, 1080)),
            AspectRatio::R4x5 => Some((1080, 1350)),
            AspectRatio::R16x9 => Some((1920, 1080)),
            AspectRatio::Original => None,
        }
    }

    /// One-char token piece for callback encoding.
    fn token(self) -> char {
        match self {
            AspectRatio::R9x16 => 't',
            AspectRatio::R1x1 => 'q',
            AspectRatio::R4x5 => 'p',
            AspectRatio::R16x9 => 'w',
            AspectRatio::Original => 'o',
        }
    }

    fn from_token(c: char) -> Option<Self> {
        match c {
            't' => Some(AspectRatio::R9x16),
            'q' => Some(AspectRatio::R1x1),
            'p' => Some(AspectRatio::R4x5),
            'w' => Some(AspectRatio::R16x9),
            'o' => Some(AspectRatio::Original),
            _ => None,
        }
    }

    /// Short human label for the config card.
    fn label(self) -> &'static str {
        match self {
            AspectRatio::R9x16 => "9:16",
            AspectRatio::R1x1 => "1:1",
            AspectRatio::R4x5 => "4:5",
            AspectRatio::R16x9 => "16:9",
            AspectRatio::Original => "orig",
        }
    }
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
    aspect: AspectRatio,
}

impl Default for StorySettings {
    /// Matches the pre-enhancement behaviour, so the one-tap path is unchanged.
    fn default() -> Self {
        Self {
            reframe: Reframe::Blur,
            seg_secs: 60,
            quality: Quality::Std,
            delivery: Delivery::Video,
            aspect: AspectRatio::R9x16,
        }
    }
}

impl StorySettings {
    /// Allowed segment lengths offered in the UI.
    const SEG_CHOICES: [u32; 3] = [15, 30, 60];

    /// Parse the compact `<mode><seg><quality><delivery><ar>` token positionally;
    /// tolerant — unknown/missing pieces fall back to [`Default`] so a malformed
    /// callback never panics or 500s. Trailing pieces are optional: legacy
    /// `<mode><seg><quality>` decodes delivery→Video, and tokens without the AR
    /// char decode aspect→9:16 (the historical canvas).
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
            Some('f') => {
                s.delivery = Delivery::Document;
                i += 1;
            }
            Some('v') => {
                s.delivery = Delivery::Video;
                i += 1;
            }
            _ => {}
        }

        // aspect ratio (optional; absent → 9:16, the historical canvas)
        if let Some(&c) = chars.get(i)
            && let Some(ar) = AspectRatio::from_token(c)
        {
            s.aspect = ar;
        }

        s
    }

    /// Encode back to the compact `<mode><seg><quality><delivery><ar>` token.
    fn encode(&self) -> String {
        let mode = if self.reframe == Reframe::Crop { 'c' } else { 'b' };
        let q = if self.quality == Quality::Max { 'm' } else { 's' };
        let d = if self.delivery == Delivery::Document { 'f' } else { 'v' };
        format!("{}{}{}{}{}", mode, self.seg_secs, q, d, self.aspect.token())
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
    fn with_aspect(mut self, a: AspectRatio) -> Self {
        self.aspect = a;
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
        // Wizard: send a grid of cropped sample frames for every aspect ratio.
        "wiz" => {
            if parts.len() < 4 {
                return Ok(());
            }
            let download_id = parts[3].parse::<i64>().unwrap_or(0);
            start_wizard(ctx, download_id).await
        }
        // Wizard pick: user chose an AR from the preview → open the card with it.
        "wpick" => {
            if parts.len() < 5 {
                return Ok(());
            }
            let download_id = parts[3].parse::<i64>().unwrap_or(0);
            let aspect = parts[4]
                .chars()
                .next()
                .and_then(AspectRatio::from_token)
                .unwrap_or(AspectRatio::R9x16);
            render_config_card(ctx, download_id, StorySettings::default().with_aspect(aspect), true).await
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
        // Aspect ratio (labels are universal ratios — no i18n needed).
        AspectRatio::ALL
            .iter()
            .map(|&ar| {
                crate::telegram::cb(
                    format!("{}{}", mark(s.aspect == ar), ar.label()),
                    cfg(s.with_aspect(ar)),
                )
            })
            .collect(),
        // Visual AR preview wizard: sends a grid of cropped sample frames.
        vec![crate::telegram::cb(
            i18n::t(lang, "stories-preview-all"),
            format!("downloads:stories:wiz:{}", id),
        )],
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

    // ── Render + segment ──
    // Fixed AR + known duration → parallel chunked re-encode (saturates the
    // box's vCPUs, ~near-instant). `Original` AR (stream-copy) or unknown
    // duration → the single-pass fallback (copy is already instant; chunking
    // needs a duration to split on).
    let output_pattern = dir.join("story_%03d.mp4");
    let parallel = settings.aspect.dims().is_some() && source_secs.is_some_and(|d| d > 0);

    let render: anyhow::Result<()> = if parallel {
        let total = if capped {
            MAX_TOTAL_SECS as u32
        } else {
            source_secs.unwrap_or(0).max(0) as u32
        };
        bot.edit_message_text(chat_id, status.id, i18n::t(&lang, "stories-rendering-parallel"))
            .await
            .ok();
        encode_segments_parallel(&input_path, &dir, settings, total).await
    } else {
        let mut cmd = build_stories_cmd(&input_path, &output_pattern, capped, settings);
        match timeout(STORIES_FFMPEG_TIMEOUT, cmd.status()).await {
            Ok(Ok(s)) if s.success() => Ok(()),
            Ok(Ok(s)) => Err(anyhow::anyhow!("ffmpeg exited {s}")),
            Ok(Err(e)) => Err(e.into()),
            Err(_) => {
                bot.delete_message(chat_id, status.id).await.ok();
                bot.send_message(chat_id, i18n::t(&lang, "stories-timeout")).await.ok();
                return Ok(());
            }
        }
    };

    if let Err(e) = render {
        log::error!("stories: render failed: {}", e);
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
                let mut req = bot.send_video(chat_id, InputFile::file(seg.clone())).caption(caption);
                // Hint the chosen canvas dims so Telegram renders the right
                // orientation. For `Original` we don't know them → omit and let
                // Telegram derive from the file.
                if let Some((w, h)) = settings.aspect.dims() {
                    req = req.width(w).height(h);
                }
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
    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-hide_banner").arg("-loglevel").arg("info").arg("-y");
    cmd.arg("-i").arg(input);
    if capped {
        cmd.arg("-t").arg(MAX_TOTAL_SECS.to_string());
    }

    match settings.aspect.dims() {
        // Original AR: no reframe → stream-copy. No re-encode = near-instant. The
        // segment muxer cuts on existing keyframes (forced keyframes aren't
        // possible with `-c copy`), so segment boundaries are approximate — an
        // accepted trade-off for the "keep original, fast" path.
        None => {
            cmd.arg("-map")
                .arg("0:v:0")
                .arg("-map")
                .arg("0:a?")
                .arg("-c")
                .arg("copy");
        }
        // Fixed AR: reframe (blur/crop) into the chosen w×h, then re-encode with
        // forced keyframes at each segment boundary for clean cuts.
        Some((w, h)) => {
            let filter = reframe_filter(settings.reframe, w, h);
            let (crf, preset, audio_bitrate) = match settings.quality {
                Quality::Std => ("20", "medium", "192k"),
                Quality::Max => ("18", "slow", "256k"),
            };
            cmd.arg("-filter_complex")
                .arg(&filter)
                .arg("-map")
                .arg("[v]")
                .arg("-map")
                .arg("0:a?")
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
                .arg("-force_key_frames")
                .arg(format!("expr:gte(t,n_forced*{})", settings.seg_secs))
                .arg("-c:a")
                .arg("aac")
                .arg("-b:a")
                .arg(audio_bitrate)
                .arg("-ar")
                .arg("44100");
        }
    }

    cmd.arg("-f")
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

/// **Per-segment parallel re-encode** — each output Story segment is encoded as
/// an independent, self-contained `ffmpeg` running **concurrently**, so a clip
/// that yields N segments uses up to N cores at once instead of one serial pass.
///
/// Why per-segment (vs chunk-then-concat): segments are the natural independent
/// unit (separate clips re-uploaded to IG), so each `-ss start -t seg` encode is
/// **exact** (no keyframe-cut drift, no concat/segment-muxer fragility) and
/// audio is continuous *within* each segment (boundaries are different stories,
/// so cross-segment seams don't matter). Produces `story_%03d.mp4` in `dir`.
async fn encode_segments_parallel(
    input: &Path,
    dir: &Path,
    settings: StorySettings,
    total_secs: u32,
) -> anyhow::Result<()> {
    let (w, h) = settings
        .aspect
        .dims()
        .context("parallel encode requires a fixed aspect ratio")?;
    let filter = reframe_filter(settings.reframe, w, h);
    // IG re-encodes on upload, so ultra presets are wasted compute — favour speed.
    let (crf, preset, abr) = match settings.quality {
        Quality::Std => ("23", "veryfast", "128k"),
        Quality::Max => ("20", "fast", "192k"),
    };
    let seg = settings.seg_secs.max(1);
    let n_segs = total_secs.div_ceil(seg).max(1);

    // Bound concurrency to the cgroup's vCPUs; give each segment a few threads so
    // a single-segment clip still uses the box, and N segments share cores cleanly.
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    let threads_per = (cores / (n_segs as usize)).clamp(2, 8);
    let max_parallel = (cores / threads_per).max(1);

    log::info!(
        "stories: parallel encode {} segment(s) × {}s, {} cores → {}-wide × {} threads, preset {}",
        n_segs,
        seg,
        cores,
        max_parallel,
        threads_per,
        preset
    );

    let sem = Arc::new(Semaphore::new(max_parallel));
    let mut set: JoinSet<Option<u32>> = JoinSet::new();
    for s in 0..n_segs {
        let sem = sem.clone();
        let input = input.to_path_buf();
        let out = dir.join(format!("story_{s:03}.mp4"));
        let filter = filter.clone();
        let (crf, preset, abr) = (crf.to_string(), preset.to_string(), abr.to_string());
        set.spawn(async move {
            let _permit = sem.acquire_owned().await.ok()?;
            let status = Command::new("ffmpeg")
                .args(["-hide_banner", "-loglevel", "error", "-y"])
                .args(["-ss", &(s * seg).to_string()])
                .arg("-i")
                .arg(&input)
                .args(["-t", &seg.to_string()])
                .args(["-threads", &threads_per.to_string()])
                .arg("-filter_complex")
                .arg(&filter)
                .args(["-map", "[v]", "-map", "0:a?"])
                .args(["-c:v", "libx264", "-profile:v", "high", "-level", "4.2"])
                .args(["-preset", &preset, "-crf", &crf])
                .args(["-pix_fmt", "yuv420p", "-r", "30"])
                .args(["-c:a", "aac", "-b:a", &abr, "-ar", "44100"])
                .arg("-movflags")
                .arg("+faststart")
                .arg(&out)
                .status()
                .await
                .ok()?;
            (status.success() && out.exists()).then_some(s)
        });
    }
    let mut ok = 0u32;
    while let Some(res) = set.join_next().await {
        if matches!(res, Ok(Some(_))) {
            ok += 1;
        }
    }
    anyhow::ensure!(ok == n_segs, "segment encode failed: {ok}/{n_segs} ok");
    Ok(())
}

/// Build the reframe `-filter_complex` graph for a target `w`×`h` canvas.
/// `Blur` = full clip over a blurred fill; `Crop` = center-crop zoom-to-fill.
fn reframe_filter(reframe: Reframe, w: u32, h: u32) -> String {
    match reframe {
        // [bg] = source scaled to *cover* the frame, cropped, heavily blurred and
        //        slightly darkened so the centred foreground pops.
        // [fg] = source scaled to *fit* inside the frame (full clip visible).
        Reframe::Blur => format!(
            "[0:v]split=2[bg][fg];\
             [bg]scale={w}:{h}:force_original_aspect_ratio=increase,crop={w}:{h},boxblur=28:2,eq=brightness=-0.07[bg];\
             [fg]scale={w}:{h}:force_original_aspect_ratio=decrease[fg];\
             [bg][fg]overlay=(W-w)/2:(H-h)/2,setsar=1[v]"
        ),
        // Zoom to fill the whole frame, cropping the overflowing edges (center).
        Reframe::Crop => format!("[0:v]scale={w}:{h}:force_original_aspect_ratio=increase,crop={w}:{h},setsar=1[v]"),
    }
}

/// Wall-clock cap for the cheap wizard ffmpeg ops (1 frame extract + crops).
const WIZARD_FFMPEG_TIMEOUT: Duration = Duration::from_secs(90);

/// Validate the download, then kick the preview wizard off detached.
async fn start_wizard(ctx: &CallbackCtx, download_id: i64) -> ResponseResult<()> {
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

    let bot = ctx.bot.clone();
    let shared_storage = ctx.shared_storage.clone();
    let chat_id = ctx.chat_id;
    tokio::spawn(async move {
        if let Err(e) = run_wizard(bot, shared_storage, chat_id, download_id, file_id).await {
            log::error!("stories wizard failed for download {}: {}", download_id, e);
        }
    });
    Ok(())
}

/// Run a quiet ffmpeg op (no progress). Returns `true` on success.
async fn run_ffmpeg_quiet(args: &[&std::ffi::OsStr]) -> bool {
    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-hide_banner").arg("-loglevel").arg("error").arg("-y");
    cmd.args(args);
    match tokio::time::timeout(WIZARD_FFMPEG_TIMEOUT, cmd.status()).await {
        Ok(Ok(st)) => st.success(),
        _ => false,
    }
}

/// Preview wizard: download the source, grab a mid-clip frame, center-crop it to
/// every aspect ratio, and send the crops as one album (a visual grid) plus a
/// row of "pick this AR" buttons. Image-only ffmpeg (1 frame + N crops) → cheap
/// and bounded, no video encode.
async fn run_wizard(
    bot: Bot,
    shared_storage: Arc<SharedStorage>,
    chat_id: ChatId,
    download_id: i64,
    file_id: String,
) -> ResponseResult<()> {
    use std::ffi::OsStr;
    let lang = i18n::user_lang_from_storage(&shared_storage, chat_id.0).await;
    let status = bot
        .send_message(chat_id, i18n::t(&lang, "stories-preview-preparing"))
        .await?;

    let (fb_mid, fb_chat) = shared_storage
        .get_download_message_info(download_id)
        .await
        .ok()
        .flatten()
        .unzip();

    let guard = match crate::core::utils::TempDirGuard::new("doradura_stories_wiz").await {
        Ok(g) => g,
        Err(e) => {
            bot.delete_message(chat_id, status.id).await.ok();
            return Err(to_req_err(e));
        }
    };
    let dir = guard.path().to_path_buf();
    let src = dir.join("source.mp4");

    if let Err(e) =
        crate::telegram::download_file_with_fallback(&bot, &file_id, fb_mid, fb_chat, Some(src.clone())).await
    {
        log::error!("stories wizard: source download failed: {}", e);
        bot.delete_message(chat_id, status.id).await.ok();
        bot.send_message(chat_id, i18n::t(&lang, "stories-download-failed"))
            .await
            .ok();
        return Ok(());
    }

    // Grab one frame at the clip midpoint.
    let dur = doracore::download::metadata::probe_duration_seconds(&src.to_string_lossy())
        .await
        .unwrap_or(0);
    let mid = (dur / 2).to_string();
    let frame = dir.join("frame.jpg");
    let ok = run_ffmpeg_quiet(&[
        OsStr::new("-ss"),
        OsStr::new(&mid),
        OsStr::new("-i"),
        src.as_os_str(),
        OsStr::new("-frames:v"),
        OsStr::new("1"),
        OsStr::new("-q:v"),
        OsStr::new("3"),
        frame.as_os_str(),
    ])
    .await;
    if !ok || !frame.exists() {
        bot.delete_message(chat_id, status.id).await.ok();
        bot.send_message(chat_id, i18n::t(&lang, "stories-preview-failed"))
            .await
            .ok();
        return Ok(());
    }

    // Center-crop the frame to each AR (Original = the untouched frame).
    let mut media: Vec<InputMedia> = Vec::new();
    for ar in AspectRatio::ALL {
        let path = match ar.dims() {
            Some((w, h)) => {
                let out = dir.join(format!("ar_{}.jpg", ar.token()));
                let vf = format!("scale={w}:{h}:force_original_aspect_ratio=increase,crop={w}:{h}");
                let cropped = run_ffmpeg_quiet(&[
                    OsStr::new("-i"),
                    frame.as_os_str(),
                    OsStr::new("-vf"),
                    OsStr::new(&vf),
                    out.as_os_str(),
                ])
                .await;
                if cropped && out.exists() { out } else { continue }
            }
            None => frame.clone(),
        };
        media.push(InputMedia::Photo(
            InputMediaPhoto::new(InputFile::file(path)).caption(ar.label()),
        ));
    }

    bot.delete_message(chat_id, status.id).await.ok();
    if media.is_empty() {
        bot.send_message(chat_id, i18n::t(&lang, "stories-preview-failed"))
            .await
            .ok();
        return Ok(());
    }
    bot.send_media_group(chat_id, media).await.ok();

    // "Pick an AR" buttons → reopen the config card pre-set to that ratio.
    let buttons: Vec<_> = AspectRatio::ALL
        .iter()
        .map(|&ar| {
            crate::telegram::cb(
                ar.label(),
                format!("downloads:stories:wpick:{}:{}", download_id, ar.token()),
            )
        })
        .collect();
    let kb = InlineKeyboardMarkup::new(vec![buttons]);
    bot.send_message(chat_id, i18n::t(&lang, "stories-pick-ar"))
        .reply_markup(kb)
        .await
        .ok();
    // `guard` drops here, removing the temp dir.
    Ok(())
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
    fn aspect_dims_and_tokens_round_trip() {
        // 9:16 canvas ratio holds.
        let (w, h) = AspectRatio::R9x16.dims().unwrap();
        assert_eq!(w * 16, h * 9);
        // 16:9 is the transpose.
        assert_eq!(AspectRatio::R16x9.dims(), Some((1920, 1080)));
        // 1:1 and 4:5.
        assert_eq!(AspectRatio::R1x1.dims(), Some((1080, 1080)));
        assert_eq!(AspectRatio::R4x5.dims(), Some((1080, 1350)));
        // Original has no fixed canvas.
        assert_eq!(AspectRatio::Original.dims(), None);
        // token round-trip for all variants.
        for ar in AspectRatio::ALL {
            assert_eq!(AspectRatio::from_token(ar.token()), Some(ar));
        }
    }

    #[test]
    fn default_settings_match_legacy() {
        let s = StorySettings::default();
        assert_eq!(s.reframe, Reframe::Blur);
        assert_eq!(s.seg_secs, 60);
        assert_eq!(s.quality, Quality::Std);
        assert_eq!(s.delivery, Delivery::Video);
        assert_eq!(s.aspect, AspectRatio::R9x16);
        // Encode now carries the trailing delivery + aspect chars.
        assert_eq!(s.encode(), "b60svt");
    }

    #[test]
    fn flags_round_trip() {
        for token in ["b60svt", "c30mfq", "b15svp", "c60mvw", "c15sfo"] {
            assert_eq!(StorySettings::parse(token).encode(), token);
        }
    }

    #[test]
    fn legacy_tokens_decode_to_defaults() {
        // Old `<mode><seg><quality>` callbacks → delivery Video + aspect 9:16.
        for token in ["b60s", "c30m", "b15s", "c60m"] {
            let s = StorySettings::parse(token);
            assert_eq!(s.delivery, Delivery::Video);
            assert_eq!(s.aspect, AspectRatio::R9x16);
        }
        // Delivery-only (no AR char) → aspect 9:16.
        assert_eq!(StorySettings::parse("b60sv").aspect, AspectRatio::R9x16);
        assert_eq!(StorySettings::parse("c30m").quality, Quality::Max);
        assert_eq!(StorySettings::parse("c30m").seg_secs, 30);
    }

    #[test]
    fn delivery_toggle_parses_and_encodes() {
        assert_eq!(StorySettings::parse("b60sf").delivery, Delivery::Document);
        assert_eq!(StorySettings::parse("b60sv").delivery, Delivery::Video);
        assert_eq!(
            StorySettings::default().with_delivery(Delivery::Document).encode(),
            "b60sft"
        );
    }

    #[test]
    fn aspect_parses_and_encodes() {
        assert_eq!(StorySettings::parse("b60svq").aspect, AspectRatio::R1x1);
        assert_eq!(StorySettings::parse("c30mfw").aspect, AspectRatio::R16x9);
        assert_eq!(StorySettings::parse("b15svo").aspect, AspectRatio::Original);
        assert_eq!(
            StorySettings::default().with_aspect(AspectRatio::R4x5).encode(),
            "b60svp"
        );
    }

    #[test]
    fn reframe_filter_uses_target_dims() {
        // Crop into 1:1 → crop=1080:1080, no blur.
        let f = reframe_filter(Reframe::Crop, 1080, 1080);
        assert!(f.contains("crop=1080:1080"));
        assert!(f.contains("force_original_aspect_ratio=increase"));
        assert!(!f.contains("boxblur"));
        // Blur into 16:9 → blurred fill at 1920:1080.
        let f = reframe_filter(Reframe::Blur, 1920, 1080);
        assert!(f.contains("crop=1920:1080"));
        assert!(f.contains("boxblur"));
        assert!(f.contains("split=2"));
    }

    #[test]
    fn original_aspect_stream_copies_without_filter() {
        let args = args_of(StorySettings::default().with_aspect(AspectRatio::Original), false);
        // Stream-copy path: -c copy, no re-encode, no filter graph.
        assert_eq!(arg_after(&args, "-c").as_deref(), Some("copy"));
        assert!(!args.iter().any(|a| a == "-filter_complex"));
        assert!(!args.iter().any(|a| a == "libx264"));
        // Still segmented.
        assert_eq!(arg_after(&args, "-f").as_deref(), Some("segment"));
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
