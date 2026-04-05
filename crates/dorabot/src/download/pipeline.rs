//! Unified download pipeline orchestrator.
//!
//! Source-agnostic pipeline that works with any `DownloadSource` backend
//! (yt-dlp, direct HTTP, or custom implementations). The pipeline handles:
//!   resolve source → get metadata → pre-checks → download with progress → post-validate
//!   → send to Telegram → success message → cleanup
//!
//! Provides two entry points:
//! - `execute()` — full pipeline for simple cases (audio)
//! - `download_phase()` — download-only for callers needing custom post-processing (video)

use crate::core::config;
use crate::core::disk;
use crate::core::error::AppError;
use crate::core::error_logger::{self, ErrorType, UserContext};
use crate::core::metrics;
use crate::core::utils::format_media_caption;
use crate::download::builder::DownloadConfigBuilder;
use crate::download::downloader::cleanup_partial_download;
use crate::download::error::DownloadError;
use crate::download::progress::{DownloadStatus, ProgressMessage};
use crate::download::send::{
    send_audio_with_retry, send_error_with_sticker, send_error_with_sticker_and_message, send_video_with_retry,
};
use crate::download::source::{DownloadOutput, DownloadSource, MediaMetadata, SourceProgress, SourceRegistry};
use crate::storage::db::{self as db, DbPool};
use crate::storage::SharedStorage;
use crate::telegram::Bot;
use std::fs;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::Message;
use url::Url;

/// Apply speed modification to a downloaded media file using ffmpeg.
/// Replaces the original file in-place.
pub(crate) async fn apply_speed_to_file(file_path: &str, speed: f32) -> Result<String, String> {
    if (speed - 1.0).abs() < 0.01 {
        return Ok(file_path.to_string());
    }
    let speed_start = std::time::Instant::now();
    let spd = speed as f64;
    let setpts_factor = 1.0 / spd;
    // Parser caps speed at 0.5..=2.0, so single atempo stage is always valid
    let atempo = format!("atempo={}", spd);

    let path = std::path::Path::new(file_path);
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("mp4");
    let tmp_path = path
        .with_extension(format!("speed.{}", ext))
        .to_string_lossy()
        .to_string();

    let mut cmd = tokio::process::Command::new("ffmpeg");
    cmd.args(["-y", "-i", file_path]);

    if ext == "mp3" || ext == "m4a" || ext == "ogg" || ext == "opus" {
        cmd.args(["-af", &atempo]);
    } else {
        let filter = format!("[0:v]setpts={}*PTS[v];[0:a]{}[a]", setpts_factor, atempo);
        cmd.args([
            "-filter_complex",
            &filter,
            "-map",
            "[v]",
            "-map",
            "[a]",
            "-c:v",
            "libx264",
            "-preset",
            "fast",
            "-crf",
            "23",
            "-c:a",
            "aac",
            "-b:a",
            "192k",
        ]);
    }
    cmd.arg(&tmp_path);

    let output = cmd.output().await.map_err(|e| format!("ffmpeg speed failed: {}", e))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        log::error!("ffmpeg speed error: {}", stderr);
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Err(format!("ffmpeg speed error: {}", stderr));
    }

    tokio::fs::rename(&tmp_path, file_path)
        .await
        .map_err(|e| format!("rename failed: {}", e))?;
    log::info!(
        "⏱️ [SPEED_FILTER] {}x applied in {:.1}s",
        speed,
        speed_start.elapsed().as_secs_f64()
    );
    Ok(file_path.to_string())
}

/// Sanitize a user-controlled string before including it in a log message.
///
/// Replaces newline and carriage-return characters with their escaped
/// representations to prevent log-injection attacks where a malicious URL
/// could forge additional log lines.
fn sanitize_for_log(s: &str) -> String {
    s.replace('\n', "\\n").replace('\r', "\\r")
}

/// The format a pipeline download should produce.
#[derive(Debug, Clone)]
pub enum PipelineFormat {
    Audio {
        bitrate: Option<String>,
        time_range: Option<(String, String)>,
    },
    Video {
        quality: Option<String>,
        time_range: Option<(String, String)>,
    },
}

impl PipelineFormat {
    /// Returns the file extension for this format.
    pub fn extension(&self) -> &str {
        match self {
            PipelineFormat::Audio { .. } => "mp3",
            PipelineFormat::Video { .. } => "mp4",
        }
    }

    /// Returns the format label for metrics and status messages.
    pub fn label(&self) -> &str {
        match self {
            PipelineFormat::Audio { .. } => "mp3",
            PipelineFormat::Video { .. } => "mp4",
        }
    }

    /// Returns the quality label for metrics.
    pub fn quality_label(&self) -> &str {
        match self {
            PipelineFormat::Audio { ref bitrate, .. } => bitrate.as_deref().unwrap_or("default"),
            PipelineFormat::Video { ref quality, .. } => quality.as_deref().unwrap_or("default"),
        }
    }

    /// Returns the max file size for this format.
    pub fn max_file_size(&self) -> u64 {
        match self {
            PipelineFormat::Audio { .. } => config::validation::max_audio_size_bytes(),
            PipelineFormat::Video { .. } => config::validation::max_video_size_bytes(),
        }
    }

    /// Returns the time_range regardless of variant.
    pub fn time_range(&self) -> &Option<(String, String)> {
        match self {
            PipelineFormat::Audio { ref time_range, .. } => time_range,
            PipelineFormat::Video { ref time_range, .. } => time_range,
        }
    }
}

/// Result of the pipeline's download+send operation, returned for post-processing.
pub struct PipelineResult {
    /// The sent Telegram message
    pub sent_message: Message,
    /// File size in bytes
    pub file_size: u64,
    /// Duration in seconds
    pub duration: u32,
    /// Title string
    pub title: String,
    /// Artist string
    pub artist: String,
    /// Display title (artist - title or just title)
    pub display_title: Arc<str>,
    /// Path to downloaded file (before cleanup)
    pub download_path: String,
    /// The download output details
    pub output: DownloadOutput,
}

/// Result of the download phase (before send).
///
/// Contains everything needed for callers to handle sending and post-processing.
/// Use with `download_phase()` for custom post-download logic (e.g., video subtitle burning).
pub struct DownloadPhaseResult {
    /// Downloaded file output details
    pub output: DownloadOutput,
    /// Title from metadata
    pub title: String,
    /// Artist from metadata
    pub artist: String,
    /// Display title (artist - title or just title)
    pub display_title: Arc<str>,
    /// Caption for Telegram
    pub caption: Arc<str>,
}

/// Execute the download phase only: resolve → metadata → pre-checks → download with progress.
///
/// Returns the download result and metadata. The caller handles sending, history,
/// and post-processing. For a complete pipeline (download + send + history), use `execute()`.
///
/// The caller creates and passes `progress_msg` so it can continue updating it after
/// the download phase completes (e.g., for send progress, error states).
pub async fn download_phase(
    bot: &Bot,
    chat_id: ChatId,
    url: &Url,
    format: &PipelineFormat,
    registry: &SourceRegistry,
    progress_msg: &mut ProgressMessage,
    message_id: Option<i32>,
    shared_storage: Option<&Arc<SharedStorage>>,
) -> Result<DownloadPhaseResult, PipelineError> {
    let pipeline_start = std::time::Instant::now();
    let file_format_str = format.label().to_string();

    // ── Step 1: Resolve source ──
    let source = registry.resolve(url).ok_or_else(|| {
        PipelineError::Operational(AppError::Download(DownloadError::Other(
            "Unsupported URL — no download source found".to_string(),
        )))
    })?;
    log::info!(
        "Pipeline: resolved source '{}' for URL: {}",
        source.name(),
        sanitize_for_log(url.as_str())
    );

    // ── Experimental flag (used in multiple steps below) ──
    let is_experimental = if let Some(s) = shared_storage {
        s.get_user_experimental_features(chat_id.0).await.unwrap_or(false)
    } else {
        false
    };

    // ── Step 2: Get metadata ──
    // Experimental fast-path: preview cache already has title/artist from the preview fetch —
    // skip the redundant yt-dlp --dump-json call (~6s saved).
    let MediaMetadata { title, artist } = {
        let from_cache = if is_experimental {
            crate::telegram::cache::PREVIEW_CACHE.get(url.as_str()).await.map(|pm| {
                log::info!("Pipeline: title/artist from preview cache (experimental, skipping yt-dlp metadata)");
                MediaMetadata {
                    title: pm.title.clone(),
                    artist: pm.artist.clone(),
                }
            })
        } else {
            None
        };

        match from_cache {
            Some(meta) => meta,
            None => match source.get_metadata(url).await {
                Ok(meta) => meta,
                Err(e) => {
                    log::error!("Pipeline: failed to get metadata: {:?}", e);
                    if e.to_string().contains("timed out") {
                        send_error_with_sticker(bot, chat_id).await;
                    }
                    return Err(PipelineError::Metadata(e));
                }
            },
        }
    };

    // Sanitize metadata: strip "NA" placeholders and newlines from yt-dlp
    let (title, artist) = sanitize_metadata(title, artist);

    let display_title: Arc<str> = if artist.is_empty() {
        Arc::from(title.as_str())
    } else {
        Arc::from(format!("{} - {}", artist, title))
    };
    let caption: Arc<str> = Arc::from(format_media_caption(&title, &artist));

    // ── Step 3: Show starting status ──
    let _ = progress_msg
        .update(
            bot,
            DownloadStatus::Starting {
                title: display_title.as_ref().to_string(),
                file_format: Some(file_format_str.clone()),
                artist: Some(artist.clone()),
            },
        )
        .await;

    // Set ⚡ reaction when download begins
    if let Some(msg_id) = message_id {
        use teloxide::types::MessageId;
        crate::telegram::try_set_reaction(bot, chat_id, MessageId(msg_id), crate::telegram::emoji::ZAP).await;
    }

    // ── Step 4: Pre-checks ──
    // Disk space
    if let Err(e) = disk::check_disk_space_for_download() {
        log::error!("Pipeline: disk space check failed: {}", e);
        send_error_with_sticker_and_message(bot, chat_id, Some("❌ Server overloaded. Try again later.")).await;
        let _ = progress_msg
            .update(
                bot,
                DownloadStatus::Error {
                    title: display_title.as_ref().to_string(),
                    error: "Not enough disk space on server".to_string(),
                    file_format: Some(file_format_str.clone()),
                },
            )
            .await;
        return Err(PipelineError::PreCheck("Insufficient disk space".to_string()));
    }

    // Livestream check
    // Experimental fast-path: read is_live from cached info JSON (~0ms) instead of a
    // separate yt-dlp --print call (~6.5s). Falls back to full check on cache miss.
    let is_live = if is_experimental {
        match doracore::download::metadata::check_is_live_from_cache(url) {
            Some(live) => {
                log::info!(
                    "Pipeline: is_live={} from cached info JSON (experimental, skipping yt-dlp check)",
                    live
                );
                live
            }
            None => source.is_livestream(url).await,
        }
    } else {
        source.is_livestream(url).await
    };
    if is_live {
        log::warn!("Pipeline: rejected livestream URL: {}", sanitize_for_log(url.as_str()));
        send_error_with_sticker_and_message(bot, chat_id, Some("❌ Live streams are not supported")).await;
        let _ = progress_msg
            .update(
                bot,
                DownloadStatus::Error {
                    title: display_title.as_ref().to_string(),
                    error: "Live streams are not supported".to_string(),
                    file_format: Some(file_format_str.clone()),
                },
            )
            .await;
        return Err(PipelineError::PreCheck("Livestreams are not supported".to_string()));
    }

    // File size pre-check (skip when time_range is set — partial downloads are much smaller)
    let max_size = format.max_file_size();
    let has_time_range = format.time_range().is_some();
    if !has_time_range && matches!(format, PipelineFormat::Video { .. }) {
        // Try PREVIEW_CACHE first (avoids a separate yt-dlp call + PO Token generation)
        let cached_size = crate::telegram::cache::PREVIEW_CACHE
            .get(url.as_str())
            .await
            .and_then(|meta| {
                // Use quality-specific size from video_formats if available
                let quality = match &format {
                    PipelineFormat::Video { quality, .. } => quality.as_deref(),
                    _ => None,
                };
                if let Some(q) = quality {
                    meta.video_formats
                        .as_ref()
                        .and_then(|fmts| fmts.iter().find(|f| f.quality == q).and_then(|f| f.size_bytes))
                        .or(meta.filesize)
                } else {
                    meta.filesize
                }
            });

        let estimated_size = if let Some(size) = cached_size {
            log::info!("📊 Size from preview cache: {:.1} MB", size as f64 / (1024.0 * 1024.0));
            Some(size)
        } else {
            source.estimate_size(url).await
        };

        if let Some(estimated_size) = estimated_size {
            if estimated_size > max_size {
                let size_mb = estimated_size as f64 / (1024.0 * 1024.0);
                let max_mb = max_size as f64 / (1024.0 * 1024.0);
                log::warn!(
                    "Pipeline: file too large: estimated {:.2} MB > max {:.2} MB",
                    size_mb,
                    max_mb
                );
                let msg = match format {
                    PipelineFormat::Audio { .. } => {
                        format!("❌ File too large: ~{:.0} MB (max {:.0} MB)", size_mb, max_mb)
                    }
                    PipelineFormat::Video { .. } => {
                        format!("❌ Video too large: ~{:.0} MB (max {:.0} MB)", size_mb, max_mb)
                    }
                };
                send_error_with_sticker_and_message(bot, chat_id, Some(&msg)).await;
                let _ = progress_msg
                    .update(
                        bot,
                        DownloadStatus::Error {
                            title: display_title.as_ref().to_string(),
                            error: msg.clone(),
                            file_format: Some(file_format_str.clone()),
                        },
                    )
                    .await;
                return Err(PipelineError::PreCheck(format!("File too large: ~{:.2} MB", size_mb)));
            }
        }
    } // end !has_time_range && video

    // ── Step 5: Build download request ──
    let mut builder = DownloadConfigBuilder::new(url.clone())
        .format(format.extension())
        .max_file_size(max_size);

    match format {
        PipelineFormat::Audio { ref bitrate, .. } => {
            if let Some(ref br) = bitrate {
                builder = builder.audio_bitrate(br);
            }
        }
        PipelineFormat::Video { ref quality, .. } => {
            if let Some(ref q) = quality {
                builder = builder.video_quality(q);
            }
        }
    }

    if let Some((ref start, ref end)) = *format.time_range() {
        builder = builder.time_range(start, end);
    }

    // Experimental: 16 concurrent fragments (2× vs previous 8) for faster segmented downloads.
    let concurrent_fragments = if is_experimental { 16u8 } else { 1u8 };
    builder = builder.concurrent_fragments(concurrent_fragments);

    let request = builder.build(&title, &artist);

    // ── Step 6: Download with progress ──
    let _ = progress_msg
        .update(
            bot,
            DownloadStatus::Downloading {
                title: display_title.as_ref().to_string(),
                progress: 0,
                speed_mbs: None,
                eta_seconds: None,
                current_size: None,
                total_size: None,
                file_format: Some(file_format_str.clone()),
                update_count: 0,
                artist: Some(artist.clone()),
            },
        )
        .await;

    let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel::<SourceProgress>();
    let source_clone: Arc<dyn DownloadSource> = Arc::clone(&source);
    let request_clone = request.clone();

    let mut download_handle = tokio::spawn(async move { source_clone.download(&request_clone, progress_tx).await });

    let bot_for_progress = bot.clone();
    let title_for_progress = Arc::clone(&display_title);
    let file_format_for_progress = file_format_str.clone();
    let artist_for_progress = Some(artist.clone());
    let mut last_progress = 0u8;
    let mut download_update_count = 0u32;

    let download_output = loop {
        tokio::select! {
            Some(sp) = progress_rx.recv() => {
                let mut safe_progress = sp.percent.clamp(last_progress, 100);
                if safe_progress == 100 && last_progress < 90 {
                    safe_progress = last_progress;
                }
                let diff = safe_progress.saturating_sub(last_progress);
                if diff >= 5 {
                    last_progress = safe_progress;
                    download_update_count += 1;

                    // Check disk space every ~25% of download to abort early if disk fills up
                    if download_update_count.is_multiple_of(5) {
                        if let Ok(info) = crate::core::disk::get_disk_space(&config::DOWNLOAD_FOLDER) {
                            if !info.has_enough_space() {
                                log::error!(
                                    "Pipeline: disk space critical during download ({:.2} GB free), aborting",
                                    info.available_gb()
                                );
                                download_handle.abort();
                                return Err(PipelineError::Operational(AppError::Download(
                                    DownloadError::DiskSpace(format!(
                                        "Disk full during download: {:.2} GB free",
                                        info.available_gb()
                                    )),
                                )));
                            }
                        }
                    }

                    let _ = progress_msg.update(
                        &bot_for_progress,
                        DownloadStatus::Downloading {
                            title: title_for_progress.as_ref().to_string(),
                            progress: safe_progress,
                            speed_mbs: sp.speed_bytes_sec.map(|b| b / (1024.0 * 1024.0)),
                            eta_seconds: sp.eta_seconds,
                            current_size: sp.downloaded_bytes,
                            total_size: sp.total_bytes,
                            file_format: Some(file_format_for_progress.clone()),
                            update_count: download_update_count,
                            artist: artist_for_progress.clone(),
                        },
                    ).await;
                }
            }
            result = &mut download_handle => {
                // Draw 100% before completing
                if last_progress < 100 {
                    let _ = progress_msg.update(
                        &bot_for_progress,
                        DownloadStatus::Downloading {
                            title: title_for_progress.as_ref().to_string(),
                            progress: 100,
                            speed_mbs: None,
                            eta_seconds: None,
                            current_size: None,
                            total_size: None,
                            file_format: Some(file_format_for_progress.clone()),
                            update_count: download_update_count,
                            artist: artist_for_progress.clone(),
                        },
                    ).await;
                }
                let output = result
                    .map_err(|e| PipelineError::Operational(AppError::Download(DownloadError::Other(format!("Task join error: {}", e)))))?
                    .map_err(PipelineError::Operational)?;
                break output;
            }
        }
    };

    log::info!(
        "Pipeline: {} downloaded ({:.2} MB)",
        format.label(),
        download_output.file_size as f64 / (1024.0 * 1024.0)
    );
    log::info!(
        "⏱️ [PIPELINE_DOWNLOAD] done in {:.1}s (chat {})",
        pipeline_start.elapsed().as_secs_f64(),
        chat_id.0
    );

    Ok(DownloadPhaseResult {
        output: download_output,
        title,
        artist,
        display_title,
        caption,
    })
}

/// Execute the unified download pipeline (full flow: download + send + history + cleanup).
///
/// This is the main entry point for simple cases (e.g., audio) where no custom
/// post-processing is needed between download and send. For video (which needs
/// subtitle burning, stream verification, splitting), use `download_phase()` instead.
pub async fn execute(
    bot: &Bot,
    chat_id: ChatId,
    url: &Url,
    format: &PipelineFormat,
    db_pool: Option<&Arc<DbPool>>,
    shared_storage: Option<&Arc<SharedStorage>>,
    message_id: Option<i32>,
    _alert_manager: Option<&Arc<crate::core::alerts::AlertManager>>,
    registry: &SourceRegistry,
    progress_msg: &mut ProgressMessage,
) -> Result<PipelineResult, PipelineError> {
    let start_time = std::time::Instant::now();
    let file_format_str = format.label().to_string();
    let canonical_url = doracore::download::url_canonical::canonicalize_url(url.as_str());

    // ── Vault cache lookup (audio only) ──
    if matches!(format, PipelineFormat::Audio { .. }) {
        if let Some(shared_storage) = shared_storage {
            if let Some(cached_fid) =
                crate::download::vault::check_vault_cache(shared_storage, chat_id.0, &canonical_url).await
            {
                log::info!(
                    "Pipeline: vault cache hit for {} (chat {})",
                    sanitize_for_log(url.as_str()),
                    chat_id
                );
                let input = teloxide::types::InputFile::file_id(teloxide::types::FileId(cached_fid));
                match bot.send_audio(chat_id, input).await {
                    Ok(sent_message) => {
                        let file_size = sent_message.audio().map(|a| a.file.size).unwrap_or(0) as u64;
                        let duration = sent_message.audio().map(|a| a.duration.seconds()).unwrap_or(0);
                        return Ok(PipelineResult {
                            sent_message,
                            file_size,
                            duration,
                            title: String::new(),
                            artist: String::new(),
                            display_title: Arc::from("(cached)"),
                            download_path: String::new(),
                            output: DownloadOutput {
                                file_path: String::new(),
                                file_size: 0,
                                duration_secs: Some(duration),
                                mime_hint: None,
                                additional_files: None,
                            },
                        });
                    }
                    Err(e) => {
                        log::warn!("Pipeline: vault cache send failed, falling through: {}", e);
                    }
                }
            }
        }
    }

    // ── Cross-user file_id dedup (skip re-download if someone already downloaded this) ──
    // Only for full downloads (no time_range = no cuts), where we can guarantee identical output.
    if format.time_range().is_none() {
        let (vq, ab) = match format {
            PipelineFormat::Audio { ref bitrate, .. } => (None, bitrate.as_deref()),
            PipelineFormat::Video { ref quality, .. } => (quality.as_deref(), None),
        };
        let cached_fid = if let Some(storage) = shared_storage {
            storage
                .find_cached_file_id(&canonical_url, format.label(), vq, ab)
                .await
                .ok()
                .flatten()
        } else if let Some(pool) = db_pool {
            if let Ok(conn) = db::get_connection(pool) {
                db::find_cached_file_id(&conn, &canonical_url, format.label(), vq, ab)
                    .ok()
                    .flatten()
            } else {
                None
            }
        } else {
            None
        };
        if let Some(cached_fid) = cached_fid {
            log::info!(
                "Pipeline: cross-user file_id cache hit for {} (chat {})",
                sanitize_for_log(url.as_str()),
                chat_id
            );
            let input = teloxide::types::InputFile::file_id(teloxide::types::FileId(cached_fid.clone()));
            let send_result = match format {
                PipelineFormat::Audio { .. } => bot.send_audio(chat_id, input).await,
                PipelineFormat::Video { .. } => bot.send_video(chat_id, input).await,
            };
            match send_result {
                Ok(sent_message) => {
                    let (file_size, duration) = match format {
                        PipelineFormat::Audio { .. } => (
                            sent_message.audio().map(|a| a.file.size).unwrap_or(0) as u64,
                            sent_message.audio().map(|a| a.duration.seconds()).unwrap_or(0),
                        ),
                        PipelineFormat::Video { .. } => (
                            sent_message.video().map(|v| v.file.size).unwrap_or(0) as u64,
                            sent_message.video().map(|v| v.duration.seconds()).unwrap_or(0),
                        ),
                    };
                    return Ok(PipelineResult {
                        sent_message,
                        file_size,
                        duration,
                        title: String::new(),
                        artist: String::new(),
                        display_title: Arc::from("(cached)"),
                        download_path: String::new(),
                        output: DownloadOutput {
                            file_path: String::new(),
                            file_size: 0,
                            duration_secs: Some(duration),
                            mime_hint: None,
                            additional_files: None,
                        },
                    });
                }
                Err(e) => {
                    log::warn!(
                        "Pipeline: file_id cache send failed (file_id may be expired), falling through: {}",
                        e
                    );
                }
            }
        }
    }

    let phase = download_phase(
        bot,
        chat_id,
        url,
        format,
        registry,
        progress_msg,
        message_id,
        shared_storage,
    )
    .await?;
    let DownloadPhaseResult {
        output: mut download_output,
        title,
        artist,
        display_title,
        caption,
    } = phase;

    // ── Speed post-processing (only when time_range is set — speed is always paired with it) ──
    if format.time_range().is_some() {
        let speed = if let Some(storage) = shared_storage {
            storage
                .get_preview_context(chat_id.0, url.as_str())
                .await
                .ok()
                .flatten()
                .and_then(|ctx| ctx.speed)
        } else {
            None
        };
        if let Some(speed) = speed {
            match apply_speed_to_file(&download_output.file_path, speed).await {
                Ok(_) => {
                    if let Ok(meta) = tokio::fs::metadata(&download_output.file_path).await {
                        download_output.file_size = meta.len();
                    }
                    if let Some(dur) = download_output.duration_secs {
                        download_output.duration_secs = Some(((dur as f64) / speed as f64).round() as u32);
                    }
                }
                Err(e) => {
                    log::warn!(
                        "Speed filter failed in audio pipeline, sending at original speed: {}",
                        e
                    );
                }
            }
        }
    }

    let max_size = format.max_file_size();

    // ── Step 7: Post-validate file size ──
    if download_output.file_size > max_size {
        let size_mb = download_output.file_size as f64 / (1024.0 * 1024.0);
        let max_mb = max_size as f64 / (1024.0 * 1024.0);
        log::warn!(
            "Pipeline: file too large after download: {:.2} MB (max: {:.2} MB)",
            size_mb,
            max_mb
        );
        send_error_with_sticker(bot, chat_id).await;
        let _ = progress_msg
            .update(
                bot,
                DownloadStatus::Error {
                    title: display_title.as_ref().to_string(),
                    error: format!("File too large ({:.2} MB). Maximum size: {:.2} MB", size_mb, max_mb),
                    file_format: Some(file_format_str.clone()),
                },
            )
            .await;
        return Err(PipelineError::Operational(AppError::Validation(format!(
            "File too large: {:.2} MB",
            size_mb
        ))));
    }

    // ── Step 8: Send to Telegram ──
    let duration = download_output.duration_secs.unwrap_or(0);

    let send_as_document = if let Some(storage) = shared_storage {
        match format {
            PipelineFormat::Audio { .. } => storage
                .get_user_send_audio_as_document(chat_id.0)
                .await
                .map(|value| value == 1)
                .unwrap_or(false),
            PipelineFormat::Video { .. } => storage
                .get_user_send_as_document(chat_id.0)
                .await
                .map(|value| value == 1)
                .unwrap_or(false),
        }
    } else {
        false
    };

    // Verify downloaded file exists before attempting send
    if !std::path::Path::new(&download_output.file_path).exists() {
        return Err(PipelineError::Operational(AppError::Download(
            DownloadError::FileNotFound(format!("Downloaded file not found: {}", download_output.file_path)),
        )));
    }

    // Check if primary file is a photo (Instagram photos, etc.)
    let is_photo = download_output
        .mime_hint
        .as_deref()
        .map(|m| m.starts_with("image/"))
        .unwrap_or(false);

    let (sent_message, file_size) = if is_photo {
        // Send photo via send_photo
        use teloxide::types::InputFile;
        let photo_file = InputFile::file(&download_output.file_path);
        let msg = bot
            .send_photo(chat_id, photo_file)
            .caption(caption.as_ref())
            .await
            .map_err(|e| {
                PipelineError::Operational(AppError::Download(DownloadError::SendFailed(format!(
                    "Failed to send photo: {}",
                    e
                ))))
            })?;
        let size = download_output.file_size;
        (msg, size)
    } else {
        match format {
            PipelineFormat::Audio { .. } => send_audio_with_retry(
                bot,
                chat_id,
                &download_output.file_path,
                duration,
                progress_msg,
                caption.as_ref(),
                send_as_document,
                message_id,
                Some(artist.clone()),
            )
            .await
            .map_err(PipelineError::Operational)?,
            PipelineFormat::Video { .. } => {
                send_video_with_retry(
                    bot,
                    chat_id,
                    &download_output.file_path,
                    progress_msg,
                    &display_title,
                    None, // thumbnail URL — video.rs handles this via download_phase()
                    send_as_document,
                    message_id,
                    Some(artist.clone()),
                )
                .await
                .map_err(PipelineError::Operational)?
            }
        }
    };

    // Send additional carousel items (Instagram multi-item posts)
    if let Some(ref extras) = download_output.additional_files {
        if !extras.is_empty() {
            use teloxide::types::{InputFile, InputMedia, InputMediaPhoto, InputMediaVideo};
            let media_group: Vec<InputMedia> = extras
                .iter()
                .filter_map(|item| {
                    if !std::path::Path::new(&item.file_path).exists() {
                        return None;
                    }
                    let file = InputFile::file(&item.file_path);
                    if item.mime_type.starts_with("video/") {
                        Some(InputMedia::Video(InputMediaVideo::new(file)))
                    } else {
                        Some(InputMedia::Photo(InputMediaPhoto::new(file)))
                    }
                })
                .collect();

            if !media_group.is_empty() {
                // Telegram send_media_group requires 2-10 items
                // If only 1 extra, send individually; otherwise batch
                if media_group.len() == 1 {
                    let item = &extras[0];
                    let file = InputFile::file(&item.file_path);
                    if item.mime_type.starts_with("video/") {
                        let _ = bot.send_video(chat_id, file).await;
                    } else {
                        let _ = bot.send_photo(chat_id, file).await;
                    }
                } else {
                    match bot.send_media_group(chat_id, media_group).await {
                        Ok(_) => {
                            log::info!("Pipeline: sent {} additional carousel items", extras.len());
                        }
                        Err(e) => {
                            log::warn!("Pipeline: failed to send carousel media group: {}", e);
                        }
                    }
                }
            }
        }
    }

    // ── Step 9: Success message ──
    let elapsed_secs = start_time.elapsed().as_secs();
    let _ = progress_msg
        .update(
            bot,
            DownloadStatus::Success {
                title: display_title.as_ref().to_string(),
                elapsed_secs,
                file_format: Some(file_format_str.clone()),
            },
        )
        .await;

    // ── Step 10: Save to download history ──
    if let Some(storage) = shared_storage {
        let file_id = match format {
            PipelineFormat::Audio { .. } => sent_message
                .audio()
                .map(|a| a.file.id.0.clone())
                .or_else(|| sent_message.document().map(|d| d.file.id.0.clone())),
            PipelineFormat::Video { .. } => sent_message
                .video()
                .map(|v| v.file.id.0.clone())
                .or_else(|| sent_message.document().map(|d| d.file.id.0.clone())),
        };

        let author_opt = if !artist.trim().is_empty() {
            Some(artist.as_str())
        } else {
            None
        };

        let (video_quality_opt, audio_bitrate_opt) = match format {
            PipelineFormat::Audio { ref bitrate, .. } => (None, bitrate.as_deref().or(Some("320k"))),
            PipelineFormat::Video { ref quality, .. } => (quality.as_deref(), None),
        };

        match storage
            .save_download_history(
                chat_id.0,
                &canonical_url,
                title.as_str(),
                format.label(),
                file_id.as_deref(),
                author_opt,
                Some(file_size as i64),
                Some(duration as i64),
                video_quality_opt,
                audio_bitrate_opt,
                None,
                None,
            )
            .await
        {
            Ok(db_id) => {
                let sent_msg_id = sent_message.id.0;
                if let Err(e) = storage.update_download_message_id(db_id, sent_msg_id, chat_id.0).await {
                    log::warn!("Failed to save message_id for download {}: {}", db_id, e);
                }
                // Auto-categorize in background if user has categories and API key is set
                let storage_c = Arc::clone(storage);
                let title_c = title.clone();
                let artist_c = artist.clone();
                let user_id_c = chat_id.0;
                tokio::spawn(async move {
                    let Ok(cats) = storage_c.get_user_categories(user_id_c).await else {
                        return;
                    };
                    if cats.is_empty() {
                        return;
                    }
                    let Some(category) = crate::core::categorizer::suggest_category(&cats, &title_c, &artist_c).await
                    else {
                        return;
                    };
                    if let Err(e) = storage_c.set_download_category(user_id_c, db_id, Some(&category)).await {
                        log::warn!("Failed to auto-set category for download {}: {}", db_id, e);
                    } else {
                        log::info!("auto-categorized download {} → '{}'", db_id, category);
                    }
                });
            }
            Err(e) => {
                log::warn!("Failed to save download history: {}", e);
            }
        }
    }

    // ── Step 10b: Send to vault (audio only, fire-and-forget) ──
    if matches!(format, PipelineFormat::Audio { .. }) {
        if let Some(shared_storage) = shared_storage {
            let file_id_for_vault = sent_message
                .audio()
                .map(|a| a.file.id.0.clone())
                .or_else(|| sent_message.document().map(|d| d.file.id.0.clone()));
            if let Some(fid) = file_id_for_vault {
                crate::download::vault::send_to_vault_background(
                    bot.clone(),
                    Arc::clone(shared_storage),
                    chat_id.0,
                    canonical_url.clone(),
                    fid,
                    Some(title.clone()),
                    if artist.is_empty() { None } else { Some(artist.clone()) },
                    Some(duration as i32),
                    Some(file_size as i64),
                );
            }
        }
    }

    // ── Step 11: Mark original message as completed ──
    if let Some(msg_id) = message_id {
        use teloxide::types::MessageId;
        let reaction = crate::telegram::success_reaction_for_format(Some(&file_format_str));
        crate::telegram::try_set_reaction(bot, chat_id, MessageId(msg_id), reaction).await;
    }

    log::info!("Pipeline: {} sent successfully to chat {}", format.label(), chat_id);

    // ── Step 12: Auto-clear success message ──
    {
        let bot_for_clear = bot.clone();
        let title_for_clear = Arc::clone(&display_title);
        let file_format_clear = file_format_str.clone();
        let mut msg_for_clear = progress_msg.clone_for_clear();
        tokio::spawn(async move {
            let _ = msg_for_clear
                .clear_after(
                    &bot_for_clear,
                    config::progress::CLEAR_DELAY_SECS,
                    title_for_clear.as_ref().to_string(),
                    Some(file_format_clear),
                )
                .await;
        });
    }

    Ok(PipelineResult {
        sent_message,
        file_size,
        duration,
        title,
        artist,
        display_title,
        download_path: download_output.file_path.clone(),
        output: download_output,
    })
}

/// Schedule file cleanup after a delay.
///
/// Also cleans up any additional files (e.g., from Instagram carousel downloads).
pub fn schedule_cleanup(download_path: String) {
    schedule_cleanup_with_extras(download_path, Vec::new());
}

/// Schedule file cleanup for the primary download path plus additional file paths.
pub fn schedule_cleanup_with_extras(download_path: String, extra_paths: Vec<String>) {
    tokio::spawn(async move {
        tokio::time::sleep(config::download::cleanup_delay()).await;
        if let Err(e) = fs::remove_file(&download_path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                log::warn!("Failed to delete file: {}", e);
            }
        }
        cleanup_partial_download(&download_path);
        for path in &extra_paths {
            if let Err(e) = fs::remove_file(path) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    log::warn!("Failed to delete extra file: {}", e);
                }
            }
        }
    });
}

/// Errors that can occur during pipeline execution.
///
/// Separated into categories to allow callers to handle them differently
/// (e.g., pre-check errors don't need admin alerts).
#[derive(Debug)]
pub enum PipelineError {
    /// Failed during metadata fetching
    Metadata(AppError),
    /// Failed pre-checks (disk space, livestream, size)
    PreCheck(String),
    /// Operational failure during download or send
    Operational(AppError),
}

impl PipelineError {
    /// Convert to AppError for compatibility with existing error handling.
    pub fn into_app_error(self) -> AppError {
        match self {
            PipelineError::Metadata(e) => e,
            PipelineError::PreCheck(msg) => AppError::Download(DownloadError::Other(msg)),
            PipelineError::Operational(e) => e,
        }
    }

    /// Whether this error warrants an admin alert.
    pub fn is_critical(&self) -> bool {
        match self {
            PipelineError::Operational(e) => {
                let s = e.to_string();
                s.contains("Signature extraction failed")
                    || s.contains("confirm you're not a bot")
                    || s.contains("bot detection")
                    || s.contains("Only images are available")
            }
            _ => false,
        }
    }
}

impl std::fmt::Display for PipelineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PipelineError::Metadata(e) => write!(f, "Metadata error: {}", e),
            PipelineError::PreCheck(msg) => write!(f, "Pre-check failed: {}", msg),
            PipelineError::Operational(e) => write!(f, "Download error: {}", e),
        }
    }
}

/// Handle pipeline errors: send error sticker, update progress message,
/// record metrics, and send admin alerts for critical errors.
pub async fn handle_pipeline_error(
    bot: &Bot,
    chat_id: ChatId,
    url: &Url,
    error: &PipelineError,
    format: &PipelineFormat,
    alert_manager: Option<&Arc<crate::core::alerts::AlertManager>>,
    message_id: Option<i32>,
) {
    // Set 😢 reaction on error
    if let Some(msg_id) = message_id {
        use teloxide::types::MessageId;
        crate::telegram::try_set_reaction(bot, chat_id, MessageId(msg_id), crate::telegram::emoji::SAD).await;
    }

    let error_str = error.to_string();

    // Determine custom error message
    let custom_message = if error_str.contains("Only images are available") {
        Some(
            "This video is not available for download\n\n\
            Possible reasons:\n\
            - Video deleted or private\n\
            - Age restrictions\n\
            - Regional restrictions\n\
            - Stream or premiere (not yet available)\n\n\
            Try a different video!",
        )
    } else if error_str.contains("Signature extraction failed") {
        Some(
            "My downloader version is outdated\n\n\
            Stan already knows and will update soon!\n\
            Try again later or try a different video.",
        )
    } else if error_str.to_lowercase().contains("bot detection") || error_str.contains("confirm you're not a bot") {
        Some(
            "YouTube has blocked the bot\n\n\
            Cookies need to be configured.\n\
            Stan already knows and is working on it!\n\n\
            Try again later.",
        )
    } else {
        None
    };

    // Send admin alert for critical errors
    if error.is_critical() {
        if let Some(alert_mgr) = alert_manager {
            let context = crate::core::alerts::DownloadContext::with_live_status().await;
            if let Err(alert_err) = alert_mgr
                .alert_download_failure(chat_id.0, url.as_str(), &error_str, 3, Some(&context))
                .await
            {
                log::error!("Failed to send critical error alert: {}", alert_err);
            }
        }
    }

    send_error_with_sticker_and_message(bot, chat_id, custom_message).await;

    // Notify admin about every download error with categorized details
    let admin_id = *config::admin::ADMIN_USER_ID;
    if admin_id != 0 {
        use crate::download::ytdlp_errors;
        let yt_error = ytdlp_errors::analyze_ytdlp_error(&error_str);
        let (category_emoji, category_name) = match &yt_error {
            ytdlp_errors::YtDlpErrorType::InvalidCookies => ("🍪", "COOKIES EXPIRED"),
            ytdlp_errors::YtDlpErrorType::BotDetection => ("🤖", "BOT DETECTION / 403"),
            ytdlp_errors::YtDlpErrorType::VideoUnavailable => ("🚫", "VIDEO UNAVAILABLE"),
            ytdlp_errors::YtDlpErrorType::NetworkError => ("🌐", "NETWORK ERROR"),
            ytdlp_errors::YtDlpErrorType::FragmentError => ("📦", "FRAGMENT ERROR"),
            ytdlp_errors::YtDlpErrorType::PostprocessingError => ("🎬", "FFMPEG / POSTPROCESS"),
            ytdlp_errors::YtDlpErrorType::DiskSpaceError => ("💾", "DISK FULL"),
            ytdlp_errors::YtDlpErrorType::Unknown => ("❓", "UNKNOWN"),
        };
        let truncated_error = if error_str.chars().count() > 300 {
            format!("{}...", error_str.chars().take(300).collect::<String>())
        } else {
            error_str.clone()
        };
        let recommendation = ytdlp_errors::get_fix_recommendations(&yt_error);
        let admin_msg = format!(
            "{} <b>{}</b>\n\n\
             User: {}\n\
             Format: {}\n\
             URL: {}\n\n\
             <pre>{}</pre>\n\n\
             {}",
            category_emoji,
            category_name,
            chat_id.0,
            format.label(),
            url,
            truncated_error,
            recommendation
        );
        let _ = bot
            .send_message(ChatId(admin_id), &admin_msg)
            .parse_mode(teloxide::types::ParseMode::Html)
            .await;
    }

    // Record metrics
    let error_type = if error_str.contains("too large") {
        "file_too_large"
    } else if error_str.contains("timed out") {
        "timeout"
    } else {
        "other"
    };
    metrics::record_download_failure(format.label(), error_type);

    // Log to error logger (offload to blocking thread pool)
    let err_type = match error_type {
        "file_too_large" => ErrorType::FileTooLarge,
        "timeout" => ErrorType::Timeout,
        _ => ErrorType::DownloadFailed,
    };
    let err_msg = error_str.clone();
    let url_str = url.to_string();
    let ctx_str = format!(r#"{{"format":"{}"}}"#, format.label());
    let user_ctx = UserContext::new(chat_id.0, None);
    tokio::task::spawn_blocking(move || {
        error_logger::log_error(err_type, &err_msg, &user_ctx, Some(&url_str), Some(&ctx_str));
    });
}

/// Sanitize metadata from yt-dlp: strip "NA" placeholders and newlines.
///
/// yt-dlp returns "NA" for missing fields (common with SoundCloud) and may include
/// newlines in titles (e.g. playlist descriptions mixed into track names).
///
/// When artist is empty/NA and title contains " - ", tries to parse artist from title
/// (common SoundCloud pattern: "Artist1, Artist2 - Track Name [tags]").
fn sanitize_metadata(title: String, artist: String) -> (String, String) {
    // Take only first line — yt-dlp --print on playlists outputs one line per track
    let title = title.lines().next().unwrap_or("").trim().to_string();
    let artist = artist.lines().next().unwrap_or("").trim().to_string();

    // yt-dlp returns "NA" for unavailable fields
    let artist = if artist == "NA" { String::new() } else { artist };

    // Truncate excessively long metadata
    const MAX_TITLE_LEN: usize = 200;
    const MAX_ARTIST_LEN: usize = 100;
    let title = if title.len() > MAX_TITLE_LEN {
        format!("{}...", &title[..title.floor_char_boundary(MAX_TITLE_LEN)])
    } else {
        title
    };
    let artist = if artist.len() > MAX_ARTIST_LEN {
        format!("{}...", &artist[..artist.floor_char_boundary(MAX_ARTIST_LEN)])
    } else {
        artist
    };

    // If artist is still empty, try to parse "Artist - Title" from the title
    if artist.is_empty() {
        if let Some(pos) = title.find(" - ") {
            let artist_part = title[..pos].trim();
            let title_part = title[pos + 3..].trim();
            if !artist_part.is_empty() && !title_part.is_empty() && artist_part.len() <= 80 {
                return (title_part.to_string(), artist_part.to_string());
            }
        }
    }

    (title, artist)
}

#[cfg(test)]
mod tests {
    use super::sanitize_metadata;

    #[test]
    fn clean_metadata_passes_through() {
        let (title, artist) = sanitize_metadata("Song Title".into(), "Artist Name".into());
        assert_eq!(title, "Song Title");
        assert_eq!(artist, "Artist Name");
    }

    #[test]
    fn na_artist_becomes_empty() {
        let (title, artist) = sanitize_metadata("Track".into(), "NA".into());
        assert_eq!(title, "Track");
        assert_eq!(artist, "");
    }

    #[test]
    fn multiline_title_takes_first_line() {
        let (title, artist) = sanitize_metadata("Track1\nTrack2\nTrack3".into(), "Artist".into());
        assert_eq!(title, "Track1");
        assert_eq!(artist, "Artist");
    }

    #[test]
    fn multiline_artist_takes_first_line() {
        let (_, artist) = sanitize_metadata("T".into(), "Art\nist".into());
        assert_eq!(artist, "Art");
    }

    #[test]
    fn na_artist_with_newlines() {
        let (title, artist) = sanitize_metadata(
            "pale fortress\nkareful - ready or not\nKAREFUL & MANNEQUIN".into(),
            "NA\n".into(),
        );
        // First line only: title="pale fortress", artist="NA" → empty
        // No " - " in "pale fortress" → no split
        assert_eq!(title, "pale fortress");
        assert_eq!(artist, "");
    }

    #[test]
    fn soundcloud_artist_title_split() {
        let (title, artist) = sanitize_metadata(
            "pale fortress, kareful - ready or not jumpstyle - slowed".into(),
            "NA".into(),
        );
        assert_eq!(artist, "pale fortress, kareful");
        assert_eq!(title, "ready or not jumpstyle - slowed");
    }

    #[test]
    fn no_split_when_artist_present() {
        let (title, artist) = sanitize_metadata("pale fortress, kareful - ready or not".into(), "Real Artist".into());
        assert_eq!(artist, "Real Artist");
        assert_eq!(title, "pale fortress, kareful - ready or not");
    }

    #[test]
    fn whitespace_trimmed() {
        let (title, artist) = sanitize_metadata("  Song  ".into(), "  Artist  ".into());
        assert_eq!(title, "Song");
        assert_eq!(artist, "Artist");
    }

    #[test]
    fn carriage_return_stripped() {
        // \r\n is a line break → first line only → "Song"
        let (title, _) = sanitize_metadata("Song\r\nTitle".into(), "A".into());
        assert_eq!(title, "Song");
    }

    #[test]
    fn empty_artist_stays_empty() {
        let (_, artist) = sanitize_metadata("T".into(), "".into());
        assert_eq!(artist, "");
    }

    #[test]
    fn na_is_case_sensitive() {
        // Only exact "NA" is filtered, not "Na" or "na"
        let (_, artist) = sanitize_metadata("T".into(), "Na".into());
        assert_eq!(artist, "Na");

        let (_, artist) = sanitize_metadata("T".into(), "na".into());
        assert_eq!(artist, "na");
    }

    #[test]
    fn long_title_truncated() {
        let long_title = "A".repeat(250);
        let (title, _) = sanitize_metadata(long_title, "Artist".into());
        assert!(title.len() <= 203); // 200 + "..."
        assert!(title.ends_with("..."));
    }

    #[test]
    fn long_artist_truncated() {
        let long_artist = "B".repeat(150);
        let (_, artist) = sanitize_metadata("Title".into(), long_artist);
        assert!(artist.len() <= 103); // 100 + "..."
        assert!(artist.ends_with("..."));
    }
}
