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
use crate::storage::db::{self as db, save_download_history, DbPool};
use crate::telegram::Bot;
use std::fs;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::Message;
use url::Url;

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
) -> Result<DownloadPhaseResult, PipelineError> {
    let file_format_str = format.label().to_string();

    // ── Step 1: Resolve source ──
    let source = registry.resolve(url).ok_or_else(|| {
        PipelineError::Operational(AppError::Download(DownloadError::Other(
            "Unsupported URL — no download source found".to_string(),
        )))
    })?;
    log::info!("Pipeline: resolved source '{}' for URL: {}", source.name(), url);

    // ── Step 2: Get metadata ──
    let MediaMetadata { title, artist } = match source.get_metadata(url).await {
        Ok(meta) => meta,
        Err(e) => {
            log::error!("Pipeline: failed to get metadata: {:?}", e);
            if e.to_string().contains("timed out") {
                send_error_with_sticker(bot, chat_id).await;
            }
            return Err(PipelineError::Metadata(e));
        }
    };

    let display_title: Arc<str> = if artist.trim().is_empty() {
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
    if source.is_livestream(url).await {
        log::warn!("Pipeline: rejected livestream URL: {}", url);
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
    if !has_time_range {
        if let Some(estimated_size) = source.estimate_size(url).await {
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
    } // end !has_time_range

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
    message_id: Option<i32>,
    _alert_manager: Option<&Arc<crate::core::alerts::AlertManager>>,
    registry: &SourceRegistry,
    progress_msg: &mut ProgressMessage,
) -> Result<PipelineResult, PipelineError> {
    let start_time = std::time::Instant::now();
    let file_format_str = format.label().to_string();

    let phase = download_phase(bot, chat_id, url, format, registry, progress_msg, message_id).await?;
    let DownloadPhaseResult {
        output: download_output,
        title,
        artist,
        display_title,
        caption,
    } = phase;

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

    let send_as_document = if let Some(pool) = db_pool {
        match db::get_connection(pool) {
            Ok(conn) => match format {
                PipelineFormat::Audio { .. } => db::get_user_send_audio_as_document(&conn, chat_id.0).unwrap_or(0) == 1,
                PipelineFormat::Video { .. } => db::get_user_send_as_document(&conn, chat_id.0).unwrap_or(0) == 1,
            },
            Err(_) => false,
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
    if let Some(pool) = db_pool {
        if let Ok(conn) = db::get_connection(pool) {
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

            match save_download_history(
                &conn,
                chat_id.0,
                url.as_str(),
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
            ) {
                Ok(db_id) => {
                    let sent_msg_id = sent_message.id.0;
                    if let Err(e) = db::update_download_message_id(&conn, db_id, sent_msg_id, chat_id.0) {
                        log::warn!("Failed to save message_id for download {}: {}", db_id, e);
                    }
                }
                Err(e) => {
                    log::warn!("Failed to save download history: {}", e);
                }
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
        let mut msg_for_clear = ProgressMessage {
            chat_id: progress_msg.chat_id,
            message_id: progress_msg.message_id,
            lang: progress_msg.lang.clone(),
            style: progress_msg.style,
            source_badge: progress_msg.source_badge.clone(),
        };
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
        let truncated_error = if error_str.len() > 300 {
            format!("{}...", &error_str[..300])
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
