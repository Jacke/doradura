// ── Re-export shared download utilities from doracore ─────────────────────────
pub use doracore::download::downloader::{
    cleanup_partial_download, generate_file_name, generate_file_name_with_ext, parse_progress,
};

use crate::core::config;
use crate::core::error::AppError;
use crate::core::error_logger::{self, ErrorType, UserContext};
use crate::core::metrics;
use crate::core::process::{FFMPEG_TIMEOUT, run_with_timeout};
use crate::core::utils::escape_filename;
use crate::download::context::DownloadContext;
use crate::download::error::DownloadError;
use crate::download::metadata::{add_cookies_args, get_metadata_from_ytdlp, probe_video_metadata};
use crate::download::progress::{DownloadStatus, ProgressBarStyle, ProgressMessage};
use crate::download::send::send_error_with_sticker;
use crate::download::ytdlp_errors::sanitize_user_error_message;
use crate::storage::db::{self as db};
use fs_err as fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;
use teloxide::prelude::*;
use teloxide::types::InputFile;
use tokio::process::Command as TokioCommand;

pub fn spawn_downloader_with_fallback(ytdl_bin: &str, args: &[&str]) -> Result<std::process::Child, AppError> {
    Command::new(ytdl_bin)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .or_else(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                let fallback = "youtube-dl";
                Command::new(fallback)
                    .args(args)
                    .stdin(Stdio::null())
                    .stdout(Stdio::inherit())
                    .stderr(Stdio::inherit())
                    .spawn()
                    .map_err(|inner| {
                        AppError::Download(DownloadError::Process(format!(
                            "Failed to start downloader. Tried '{}', then '{}': {} / {}",
                            ytdl_bin, fallback, e, inner
                        )))
                    })
            } else {
                Err(AppError::Download(DownloadError::Process(format!(
                    "Failed to start downloader '{}': {}",
                    ytdl_bin, e
                ))))
            }
        })
}

/// Download subtitles file (SRT or TXT format) and send it to user
///
/// Downloads subtitles from URL using yt-dlp and sends them as a document.
///
/// # Arguments
///
/// * `ctx` - Shared download context (bot, chat_id, url, etc.)
/// * `subtitle_format` - Subtitle format ("srt" or "txt")
///
/// # Returns
///
/// Returns `Ok(())` on success or a `ResponseResult` error.
pub async fn download_and_send_subtitles(ctx: DownloadContext, subtitle_format: String) -> ResponseResult<()> {
    let DownloadContext {
        bot,
        chat_id,
        url,
        rate_limiter,
        db_pool,
        shared_storage,
        message_id,
        alert_manager: _alert_manager,
        created_timestamp: _created_timestamp,
    } = ctx;
    let bot_clone = bot.clone();
    let _rate_limiter = Arc::clone(&rate_limiter);
    let db_pool_clone = db_pool.clone();
    let shared_storage_clone = shared_storage.clone();

    // Run inline (awaited) so queue_processor waits for actual completion.
    // Previously this was tokio::spawn(...), causing the queue permit to be
    // released ~50ms into the job and multiple subtitle downloads to run in
    // parallel despite max_concurrent=1.
    async move {
        let lang = if let Some(storage) = shared_storage_clone.as_ref() {
            crate::i18n::user_lang_from_storage(storage, chat_id.0).await
        } else {
            db_pool_clone
                .as_ref()
                .map(|pool| crate::i18n::user_lang_from_pool(pool, chat_id.0))
                .unwrap_or_else(|| crate::i18n::lang_from_code("ru"))
        };
        let mut progress_msg = ProgressMessage::new(chat_id, lang);
        if let Some(storage) = shared_storage_clone.as_ref() {
            if let Ok(style_str) = storage.get_user_progress_bar_style(chat_id.0).await {
                progress_msg.style = ProgressBarStyle::parse(&style_str);
            }
        } else if let Some(ref pool) = db_pool_clone
            && let Ok(conn) = db::get_connection(pool)
            && let Ok(style_str) = db::get_user_progress_bar_style(&conn, chat_id.0)
        {
            progress_msg.style = ProgressBarStyle::parse(&style_str);
        }
        let start_time = std::time::Instant::now();

        // Get user plan for metrics
        let user_plan = if let Some(storage) = shared_storage_clone.as_ref() {
            storage
                .get_user(chat_id.0)
                .await
                .ok()
                .flatten()
                .map(|u| u.plan)
                .unwrap_or_default()
        } else if let Some(ref pool) = db_pool_clone {
            match db::get_connection(pool) {
                Ok(conn) => db::get_user(&conn, chat_id.0)
                    .ok()
                    .flatten()
                    .map(|u| u.plan)
                    .unwrap_or_default(),
                _ => crate::core::types::Plan::default(),
            }
        } else {
            crate::core::types::Plan::default()
        };

        // Record format request for metrics
        let format = subtitle_format.as_str();
        metrics::record_format_request(format, user_plan.as_str());

        // Start metrics timer for subtitles download
        let timer = metrics::DOWNLOAD_DURATION_SECONDS
            .with_label_values(&[format, "default"])
            .start_timer();

        let result: Result<(), AppError> = async {
            // Step 1: Get metadata
            let notifier_bot = bot_clone.clone();
            let notifier: crate::download::metadata::ErrorNotifyFn = Box::new(move |text| {
                let bot = notifier_bot.clone();
                Box::pin(async move {
                    crate::telegram::notifications::notify_admin_text(&bot, &text).await;
                })
            });
            let (title, _) = match get_metadata_from_ytdlp(&url, Some(&notifier)).await {
                Ok(meta) => meta,
                Err(e) => {
                    log::error!("Failed to get metadata: {:?}", e);
                    // Check if this is a timeout error
                    if e.to_string().contains("timed out") {
                        log::warn!("yt-dlp timed out, sending error message to user");
                        send_error_with_sticker(&bot_clone, chat_id).await;
                    }
                    return Err(e);
                }
            };
            let display_title: Arc<str> = Arc::from(title.as_str());

            // Show starting status
            let _ = progress_msg
                .update(
                    &bot_clone,
                    DownloadStatus::Starting {
                        title: display_title.as_ref().to_string(),
                        file_format: Some(subtitle_format.clone()),
                        artist: None,
                    },
                )
                .await;

            let file_name = format!("{}.{}", title, subtitle_format);
            let safe_filename = escape_filename(&file_name);
            let full_path = format!("{}/{}", &*config::DOWNLOAD_FOLDER, safe_filename);
            let download_path = shellexpand::tilde(&full_path).into_owned();

            // Step 2: Download subtitles
            let ytdl_bin = &*config::YTDL_BIN;
            let sub_format_flag = match subtitle_format.as_str() {
                "srt" => "--convert-subs=srt",
                "txt" => "--convert-subs=txt",
                _ => "--convert-subs=srt",
            };

            let mut args: Vec<&str> = vec![
                "-o",
                &download_path,
                "--skip-download",
                "--write-auto-subs",
                sub_format_flag,
            ];
            add_cookies_args(&mut args);
            args.push(url.as_str());

            // Log the full command for debugging
            let command_str = format!("{} {}", ytdl_bin, args.join(" "));
            log::debug!("yt-dlp command for subtitles download: {}", command_str);

            // Run blocking download in spawn_blocking to avoid blocking async runtime
            let ytdl_bin_owned = ytdl_bin.to_string();
            let args_owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
            let download_result = tokio::task::spawn_blocking(move || {
                let args_refs: Vec<&str> = args_owned.iter().map(|s| s.as_str()).collect();
                let mut child = spawn_downloader_with_fallback(&ytdl_bin_owned, &args_refs)?;
                let status = child.wait().map_err(|e| {
                    AppError::Download(DownloadError::Process(format!("downloader process failed: {}", e)))
                })?;
                Ok::<_, AppError>(status)
            })
            .await
            .map_err(|e| AppError::Download(DownloadError::Process(format!("spawn_blocking failed: {}", e))))??;

            if !download_result.success() {
                return Err(AppError::Download(DownloadError::Process(format!(
                    "downloader exited with status: {}",
                    download_result
                ))));
            }

            // Check if file exists
            if fs::metadata(&download_path).is_err() {
                // Try to find the actual filename that was downloaded
                let parent_dir = shellexpand::tilde("~/downloads/").into_owned();
                let dir_entries = fs::read_dir(&parent_dir).map_err(|e| {
                    AppError::Download(DownloadError::FileNotFound(format!(
                        "Failed to read downloads dir: {}",
                        e
                    )))
                })?;
                let mut found_file: Option<String> = None;

                for entry in dir_entries {
                    if let Ok(entry) = entry {
                        let file_name = entry.file_name().to_string_lossy().to_string();
                        if file_name.ends_with(&format!(".{}", subtitle_format)) {
                            found_file = Some(entry.path().to_string_lossy().to_string());
                            break;
                        }
                    }
                }

                if let Some(found) = found_file {
                    // Send the found file
                    let _sent_message = bot_clone
                        .send_document(chat_id, InputFile::file(&found))
                        .await
                        .map_err(|e| {
                            AppError::Download(DownloadError::SendFailed(format!("Failed to send document: {}", e)))
                        })?;

                    // NOTE: Subtitles are not saved to download_history as they won't appear in /downloads
                    // (We only save mp3/mp4 with file_id for the /downloads command)
                    // Subtitle tracking is intentionally disabled per requirements
                } else {
                    return Err(AppError::Download(DownloadError::FileNotFound(
                        "Subtitle file not found".to_string(),
                    )));
                }
            } else {
                // Send the file
                let _sent_message = bot_clone
                    .send_document(chat_id, InputFile::file(&download_path))
                    .await
                    .map_err(|e| {
                        AppError::Download(DownloadError::SendFailed(format!("Failed to send document: {}", e)))
                    })?;

                // NOTE: Subtitles are not saved to download_history as they won't appear in /downloads
                // (We only save mp3/mp4 with file_id for the /downloads command)
                // Subtitle tracking is intentionally disabled per requirements
            }

            // Calculate elapsed time
            let elapsed_secs = start_time.elapsed().as_secs();
            log::info!("Subtitle downloaded in {} seconds", elapsed_secs);

            // Step 3: Show success status
            let _ = progress_msg
                .update(
                    &bot_clone,
                    DownloadStatus::Success {
                        title: display_title.as_ref().to_string(),
                        elapsed_secs,
                        file_format: Some(subtitle_format.clone()),
                    },
                )
                .await;

            // Mark the original message as completed if message_id is available
            if let Some(msg_id) = message_id {
                use teloxide::types::MessageId;
                crate::telegram::try_set_reaction(
                    &bot_clone,
                    chat_id,
                    MessageId(msg_id),
                    crate::telegram::success_reaction_for_format(Some(&subtitle_format)),
                )
                .await;
            }

            log::info!("Subtitle sent successfully to chat {}", chat_id);

            // Step 4: Auto-clear success message
            let bot_for_clear = bot_clone.clone();
            let title_for_clear = Arc::clone(&display_title);
            let mut msg_for_clear = progress_msg.clone_for_clear();
            let subtitle_format_clone = subtitle_format.clone();
            tokio::spawn(async move {
                let _ = msg_for_clear
                    .clear_after(
                        &bot_for_clear,
                        10,
                        title_for_clear.as_ref().to_string(),
                        Some(subtitle_format_clone),
                    )
                    .await;
            });

            // Clean up file after 10 minutes
            tokio::time::sleep(config::download::cleanup_delay()).await;
            if let Err(e) = fs::remove_file(&download_path)
                && e.kind() != std::io::ErrorKind::NotFound
            {
                return Err(AppError::Download(DownloadError::Other(format!(
                    "Failed to delete file: {}",
                    e
                ))))?;
            }
            // File doesn't exist - that's fine, it was probably deleted manually

            Ok(())
        }
        .await;

        // Record metrics based on result
        match &result {
            Ok(_) => {
                log::info!("Subtitle download completed successfully for chat {}", chat_id);
                timer.observe_duration();
                metrics::record_download_success(format, "default");
            }
            Err(e) => {
                e.track_with_operation("subtitle_download");
                timer.observe_duration();
                let error_type = if e.to_string().contains("timed out") {
                    "timeout"
                } else {
                    "other"
                };
                metrics::record_download_failure(format, error_type);

                // Log error to database (offload to blocking thread pool)
                let err_type = if error_type == "timeout" {
                    ErrorType::Timeout
                } else {
                    ErrorType::DownloadFailed
                };
                let err_msg = e.to_string();
                let url_str = url.to_string();
                let ctx_str = format!(r#"{{"format":"{}"}}"#, format);
                let user_ctx = UserContext::new(chat_id.0, None);
                tokio::task::spawn_blocking(move || {
                    error_logger::log_error(err_type, &err_msg, &user_ctx, Some(&url_str), Some(&ctx_str));
                });
            }
        }

        if let Err(e) = result {
            log::error!(
                "An error occurred during subtitle download for chat {}: {:?}",
                chat_id,
                e
            );
            let user_error = sanitize_user_error_message(&e.to_string());
            // Send error sticker and message
            send_error_with_sticker(&bot_clone, chat_id).await;
            // Show error status
            let _ = progress_msg
                .update(
                    &bot_clone,
                    DownloadStatus::Error {
                        title: "Downloading".to_string(),
                        error: user_error,
                        file_format: Some(subtitle_format.clone()),
                    },
                )
                .await;
        }
    }
    .await;
    Ok(())
}

// ==================== Subtitle Burning ====================

/// Burns (hardcodes) subtitles into a video file using ffmpeg
///
/// # Arguments
///
/// * `video_path` - Path to the source video file
/// * `subtitle_path` - Path to the subtitle file (SRT format)
/// * `output_path` - Path where the output video with burned subtitles will be saved
///
/// # Returns
///
/// Returns `Ok(())` on success or an `AppError` on failure.
///
/// # Example
///
/// ```no_run
/// # use doradura::download::downloader::burn_subtitles_into_video;
/// # use doradura::storage::db::SubtitleStyle;
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let style = SubtitleStyle::default();
/// burn_subtitles_into_video("input.mp4", "subtitles.srt", "output.mp4", &style).await?;
/// # Ok(())
/// # }
/// ```
///
/// Splits a large video file into playable segments using ffmpeg.
/// This is used when the file exceeds Telegram's upload limits.
pub async fn split_video_into_parts(path: &str, target_part_size_bytes: u64) -> Result<Vec<String>, AppError> {
    let split_start = std::time::Instant::now();
    log::info!("Checking if video needs splitting: {}", path);
    let file_size = fs::metadata(path)
        .map_err(|e| AppError::Download(DownloadError::Other(format!("Failed to get file size: {}", e))))?
        .len();

    if file_size <= target_part_size_bytes {
        log::info!(
            "Video size {} is within limit {}, no splitting needed",
            file_size,
            target_part_size_bytes
        );
        doracore::core::metrics::VIDEO_ENCODING_DURATION_SECONDS
            .with_label_values(&["split"])
            .observe(split_start.elapsed().as_secs_f64());
        return Ok(vec![path.to_string()]);
    }

    let metadata = probe_video_metadata(path)
        .await
        .ok_or_else(|| AppError::Download(DownloadError::Ffmpeg(format!("Failed to probe video: {}", path))))?;
    let duration = metadata.0 as f64;

    // Use slightly smaller parts to be safe (e.g. 5% buffer)
    let safe_target = (target_part_size_bytes as f64 * 0.95) as u64;
    let num_parts = (file_size as f64 / safe_target as f64).ceil() as u64;
    let segment_duration = duration / num_parts as f64;

    log::info!(
        "Splitting video (size: {} MB, duration: {}s) into {} parts, ~{:.2}s each",
        file_size / 1024 / 1024,
        duration,
        num_parts,
        segment_duration
    );

    let output_pattern = format!("{}_part_%03d.mp4", path.trim_end_matches(".mp4"));

    let mut cmd = TokioCommand::new("ffmpeg");
    cmd.args([
        "-i",
        path,
        "-f",
        "segment",
        "-segment_time",
        &segment_duration.to_string(),
        "-c",
        "copy", // Use stream copy for speed
        "-map",
        "0",
        "-reset_timestamps",
        "1",
        &output_pattern,
    ]);
    let output = run_with_timeout(&mut cmd, FFMPEG_TIMEOUT).await?;

    if !output.status.success() {
        doracore::core::metrics::VIDEO_ENCODING_DURATION_SECONDS
            .with_label_values(&["split"])
            .observe(split_start.elapsed().as_secs_f64());
        return Err(AppError::Download(DownloadError::Ffmpeg(format!(
            "ffmpeg split failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ))));
    }

    // Find all created parts
    let mut parts = Vec::new();
    let parent_dir = Path::new(path).parent().unwrap_or_else(|| Path::new("."));
    let file_stem = Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();

    for entry in fs::read_dir(parent_dir).map_err(|e| AppError::Download(DownloadError::Other(e.to_string())))? {
        let entry = entry.map_err(|e| AppError::Download(DownloadError::Other(e.to_string())))?;
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with(&file_stem) && name.contains("_part_") && name.ends_with(".mp4") {
            parts.push(entry.path().to_string_lossy().to_string());
        }
    }
    parts.sort();

    log::info!("Successfully split video into {} parts", parts.len());
    doracore::core::metrics::VIDEO_ENCODING_DURATION_SECONDS
        .with_label_values(&["split"])
        .observe(split_start.elapsed().as_secs_f64());
    Ok(parts)
}

/// Cleans overlapping timestamps in SRT subtitles (common with YouTube auto-captions).
///
/// YouTube auto-generated captions produce overlapping entries where each new line
/// starts before the previous one ends, causing ffmpeg/libass to stack them on screen.
/// This function:
/// 1. Removes consecutive entries with identical text (YouTube "builds up" captions)
/// 2. Trims end times so entry N-1 ends when entry N starts (no overlap)
/// 3. Re-numbers entries sequentially
pub async fn clean_srt_overlaps(path: &str) {
    let content = match fs_err::tokio::read_to_string(path).await {
        Ok(c) => c,
        Err(e) => {
            log::warn!("Could not read SRT for cleanup: {e}");
            return;
        }
    };

    let mut entries: Vec<(String, String, String)> = Vec::new(); // (start, end, text)

    // Parse SRT blocks separated by blank lines
    for block in content.split("\n\n") {
        let block = block.trim();
        if block.is_empty() {
            continue;
        }
        let lines: Vec<&str> = block.lines().collect();
        // SRT format: index, timestamp line, text lines
        if lines.len() < 3 {
            continue;
        }
        // Find the timestamp line (contains " --> ")
        let ts_idx = match lines.iter().position(|l| l.contains(" --> ")) {
            Some(i) => i,
            None => continue,
        };
        let ts_parts: Vec<&str> = lines[ts_idx].split(" --> ").collect();
        if ts_parts.len() != 2 {
            continue;
        }
        let start = ts_parts[0].trim().to_string();
        let end = ts_parts[1].trim().to_string();
        let text: String = lines[ts_idx + 1..].join("\n");
        entries.push((start, end, text));
    }

    if entries.is_empty() {
        return;
    }

    // Step 1: Remove consecutive entries with identical text
    let mut deduped: Vec<(String, String, String)> = Vec::with_capacity(entries.len());
    for entry in entries {
        if let Some(last) = deduped.last()
            && last.2.trim() == entry.2.trim()
        {
            continue; // skip duplicate text
        }
        deduped.push(entry);
    }

    // Step 2: Trim overlapping end times — if entry N starts before N-1 ends,
    // set N-1's end to N's start
    for i in 1..deduped.len() {
        let next_start = deduped[i].0.clone();
        let prev_end = &deduped[i - 1].1;
        if prev_end > &next_start {
            deduped[i - 1].1 = next_start;
        }
    }

    // Step 3: Write back as SRT
    let mut out = String::new();
    for (i, (start, end, text)) in deduped.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&format!("{}\n{} --> {}\n{}\n", i + 1, start, end, text));
    }

    if let Err(e) = fs_err::tokio::write(path, &out).await {
        log::warn!("Could not write cleaned SRT: {e}");
    } else {
        log::info!("🧹 Cleaned SRT overlaps: {path}");
    }
}

/// # use doradura::core::error::AppError;
/// # use doradura::download::downloader::burn_subtitles_into_video;
/// # async fn run() -> Result<(), AppError> {
/// let style = db::SubtitleStyle::default();
/// burn_subtitles_into_video("video.mp4", "subtitles.srt", "video_with_subs.mp4", &style).await?;
/// # Ok(())
/// # }
/// ```
pub async fn burn_subtitles_into_video(
    video_path: &str,
    subtitle_path: &str,
    output_path: &str,
    style: &db::SubtitleStyle,
) -> Result<(), AppError> {
    let encoding_start = std::time::Instant::now();
    log::info!(
        "🔥 Burning subtitles into video: {} + {} -> {}",
        video_path,
        subtitle_path,
        output_path
    );

    // Verify source files exist
    if !std::path::Path::new(video_path).exists() {
        doracore::core::metrics::VIDEO_ENCODING_DURATION_SECONDS
            .with_label_values(&["burn_subtitles"])
            .observe(encoding_start.elapsed().as_secs_f64());
        return Err(AppError::Download(DownloadError::FileNotFound(format!(
            "Video file not found: {}",
            video_path
        ))));
    }
    if !std::path::Path::new(subtitle_path).exists() {
        doracore::core::metrics::VIDEO_ENCODING_DURATION_SECONDS
            .with_label_values(&["burn_subtitles"])
            .observe(encoding_start.elapsed().as_secs_f64());
        return Err(AppError::Download(DownloadError::FileNotFound(format!(
            "Subtitle file not found: {}",
            subtitle_path
        ))));
    }

    // Clean overlapping timestamps from YouTube auto-captions
    clean_srt_overlaps(subtitle_path).await;

    // Escape subtitle path for ffmpeg filter syntax.
    // ffmpeg filter strings interpret: \ ' : [ ] ; , = as special chars.
    // All must be escaped with backslash to prevent filter injection.
    let escaped_subtitle_path = subtitle_path
        .replace('\\', "\\\\")
        .replace('\'', "\\'")
        .replace(':', "\\:")
        .replace('[', "\\[")
        .replace(']', "\\]")
        .replace(';', "\\;")
        .replace(',', "\\,")
        .replace('=', "\\=");

    let force_style = style.to_force_style();
    let vf_filter = format!("subtitles='{}':force_style='{}'", escaped_subtitle_path, force_style);
    let mut cmd = TokioCommand::new("ffmpeg");
    cmd.arg("-i")
        .arg(video_path)
        .arg("-vf")
        .arg(&vf_filter)
        .arg("-c:v")
        .arg("libx264")
        .arg("-c:a")
        .arg("copy")
        .arg("-preset")
        .arg("ultrafast")
        .arg("-y") // Overwrite output file if it exists
        .arg(output_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // 10 minutes — re-encoding a full video with libx264 is much slower than
    // stream-copy operations (splitting, thumbnails) that use FFMPEG_TIMEOUT (120s).
    const BURN_TIMEOUT: Duration = Duration::from_secs(600);

    log::info!(
        "🎬 Running ffmpeg command: ffmpeg -i {} -vf '{}' -c:v libx264 -c:a copy -preset ultrafast -y {}",
        video_path,
        vf_filter,
        output_path
    );

    let output = run_with_timeout(&mut cmd, BURN_TIMEOUT).await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        log::error!("❌ ffmpeg failed to burn subtitles: {}", stderr);
        doracore::core::metrics::VIDEO_ENCODING_DURATION_SECONDS
            .with_label_values(&["burn_subtitles"])
            .observe(encoding_start.elapsed().as_secs_f64());
        return Err(AppError::Download(DownloadError::Ffmpeg(format!(
            "ffmpeg failed to burn subtitles: {}",
            stderr
        ))));
    }

    // Verify that the output file was created
    if !std::path::Path::new(output_path).exists() {
        doracore::core::metrics::VIDEO_ENCODING_DURATION_SECONDS
            .with_label_values(&["burn_subtitles"])
            .observe(encoding_start.elapsed().as_secs_f64());
        return Err(AppError::Download(DownloadError::FileNotFound(format!(
            "Output video file was not created: {}",
            output_path
        ))));
    }

    log::info!("✅ Successfully burned subtitles into video: {}", output_path);
    doracore::core::metrics::VIDEO_ENCODING_DURATION_SECONDS
        .with_label_values(&["burn_subtitles"])
        .observe(encoding_start.elapsed().as_secs_f64());
    Ok(())
}

// ==================== Audio Effects Integration ====================

#[cfg(test)]
mod download_tests {
    use super::*;
    use crate::core::{extract_retry_after, is_timeout_or_network_error, truncate_tail_utf8};
    use crate::download::metadata::{build_telegram_safe_format, probe_duration_seconds, validate_cookies_file_format};
    use crate::download::send::{UploadProgress, read_log_tail};
    use std::path::PathBuf;

    fn tool_exists(bin: &str) -> bool {
        Command::new("which")
            .arg(bin)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    // ==================== extract_retry_after Tests ====================

    #[test]
    fn test_extract_retry_after_standard_format() {
        assert_eq!(extract_retry_after("Retry after 30s"), Some(30));
        assert_eq!(extract_retry_after("retry after 60s"), Some(60));
        assert_eq!(extract_retry_after("RETRY AFTER 120s"), Some(120));
    }

    #[test]
    fn test_extract_retry_after_alternative_format() {
        assert_eq!(extract_retry_after("retry_after: 45"), Some(45));
        assert_eq!(extract_retry_after("retry_after:30"), Some(30));
        assert_eq!(extract_retry_after("RETRY_AFTER: 90"), Some(90));
    }

    #[test]
    fn test_extract_retry_after_no_match() {
        assert_eq!(extract_retry_after("no retry info here"), None);
        assert_eq!(extract_retry_after(""), None);
        assert_eq!(extract_retry_after("random error message"), None);
    }

    #[test]
    fn test_extract_retry_after_embedded_in_message() {
        assert_eq!(
            extract_retry_after("Error: Too many requests. Retry after 15s please wait"),
            Some(15)
        );
    }

    // ==================== is_timeout_or_network_error Tests ====================

    #[test]
    fn test_is_timeout_or_network_error_timeout() {
        assert!(is_timeout_or_network_error("Request timeout"));
        assert!(is_timeout_or_network_error("Connection timed out"));
        assert!(is_timeout_or_network_error("TIMEOUT ERROR"));
    }

    #[test]
    fn test_is_timeout_or_network_error_network() {
        assert!(is_timeout_or_network_error("Network error occurred"));
        assert!(is_timeout_or_network_error("Error sending request"));
    }

    #[test]
    fn test_is_timeout_or_network_error_negative() {
        assert!(!is_timeout_or_network_error("File not found"));
        assert!(!is_timeout_or_network_error("Permission denied"));
        assert!(!is_timeout_or_network_error("Invalid URL"));
    }

    // ==================== UploadProgress Tests ====================

    #[test]
    fn test_upload_progress_new() {
        let progress = UploadProgress::new();
        assert_eq!(progress.bytes_sent(), 0);
    }

    #[test]
    fn test_upload_progress_add_bytes() {
        let progress = UploadProgress::new();
        progress.add_bytes(1000);
        assert_eq!(progress.bytes_sent(), 1000);

        progress.add_bytes(500);
        assert_eq!(progress.bytes_sent(), 1500);
    }

    #[test]
    fn test_upload_progress_clone() {
        let progress = UploadProgress::new();
        progress.add_bytes(100);

        let cloned = progress.clone();
        // Both should share the same Arc
        assert_eq!(cloned.bytes_sent(), 100);

        progress.add_bytes(50);
        assert_eq!(cloned.bytes_sent(), 150);
    }

    // ==================== Existing Tests ====================

    #[tokio::test]
    async fn test_probe_duration_seconds_handles_missing_file() {
        assert_eq!(probe_duration_seconds("/no/such/file.mp3").await, None);
    }

    #[test]
    fn test_spawn_downloader_fails_without_tools() {
        if tool_exists("yt-dlp") || tool_exists("youtube-dl") {
            // Tools present; skip this specific negative test.
            return;
        }
        let res = spawn_downloader_with_fallback("youtube-dl", &["--version"]);
        assert!(res.is_err());
    }

    // ==================== truncate_tail_utf8 Tests ====================

    #[test]
    fn test_truncate_tail_utf8_short_string() {
        let text = "Hello, World!";
        assert_eq!(truncate_tail_utf8(text, 100), text);
    }

    #[test]
    fn test_truncate_tail_utf8_exact_length() {
        let text = "Hello";
        assert_eq!(truncate_tail_utf8(text, 5), "Hello");
    }

    #[test]
    fn test_truncate_tail_utf8_truncates() {
        let text = "Hello, World! This is a test.";
        let result = truncate_tail_utf8(text, 10);
        // Should contain ellipsis and last 10 bytes
        assert!(result.starts_with("…\n"));
        assert!(result.len() <= 15); // ellipsis + newline + ~10 bytes
    }

    #[test]
    fn test_truncate_tail_utf8_respects_boundaries() {
        // UTF-8 string with multi-byte characters
        let text = "Hello 🌍 world"; // emoji are 4 bytes each in UTF-8
        let result = truncate_tail_utf8(text, 6);
        // Should not break in the middle of a UTF-8 character
        assert!(result.is_char_boundary(0));
        for (i, _) in result.char_indices() {
            assert!(result.is_char_boundary(i));
        }
    }

    #[test]
    fn test_truncate_tail_utf8_empty_string() {
        let text = "";
        assert_eq!(truncate_tail_utf8(text, 10), "");
    }

    // ==================== validate_cookies_file_format Tests ====================

    #[test]
    fn test_validate_cookies_file_format_valid() {
        use std::io::Write;
        let temp_dir = std::env::temp_dir();
        let temp_file = temp_dir.join(format!("test_cookies_{}.txt", std::process::id()));

        let mut file = std::fs::File::create(&temp_file).unwrap();
        writeln!(file, "# Netscape HTTP Cookie File").unwrap();
        writeln!(file, "# This is a generated file").unwrap();
        writeln!(file, ".youtube.com\tTRUE\t/\tTRUE\t0\tSID\tabc123").unwrap();
        drop(file);

        let result = validate_cookies_file_format(temp_file.to_str().unwrap());
        let _ = fs_err::remove_file(&temp_file);
        assert!(result);
    }

    #[test]
    fn test_validate_cookies_file_format_missing_header() {
        use std::io::Write;
        let temp_dir = std::env::temp_dir();
        let temp_file = temp_dir.join(format!("test_cookies_no_header_{}.txt", std::process::id()));

        let mut file = std::fs::File::create(&temp_file).unwrap();
        writeln!(file, "# Some other file format").unwrap();
        writeln!(file, ".youtube.com\tTRUE\t/\tTRUE\t0\tSID\tabc123").unwrap();
        drop(file);

        let result = validate_cookies_file_format(temp_file.to_str().unwrap());
        let _ = fs_err::remove_file(&temp_file);
        assert!(!result);
    }

    #[test]
    fn test_validate_cookies_file_format_no_cookies() {
        use std::io::Write;
        let temp_dir = std::env::temp_dir();
        let temp_file = temp_dir.join(format!("test_cookies_empty_{}.txt", std::process::id()));

        let mut file = std::fs::File::create(&temp_file).unwrap();
        writeln!(file, "# Netscape HTTP Cookie File").unwrap();
        writeln!(file, "# No actual cookies").unwrap();
        drop(file);

        let result = validate_cookies_file_format(temp_file.to_str().unwrap());
        let _ = fs_err::remove_file(&temp_file);
        assert!(!result);
    }

    #[test]
    fn test_validate_cookies_file_format_nonexistent() {
        let result = validate_cookies_file_format("/nonexistent/cookies.txt");
        assert!(!result);
    }

    #[test]
    fn test_validate_cookies_file_format_http_cookie_header() {
        use std::io::Write;
        let temp_dir = std::env::temp_dir();
        let temp_file = temp_dir.join(format!("test_cookies_http_{}.txt", std::process::id()));

        let mut file = std::fs::File::create(&temp_file).unwrap();
        writeln!(file, "# HTTP Cookie File").unwrap();
        writeln!(file, ".example.com\tTRUE\t/\tFALSE\t0\ttest\tvalue").unwrap();
        drop(file);

        let result = validate_cookies_file_format(temp_file.to_str().unwrap());
        let _ = fs_err::remove_file(&temp_file);
        assert!(result);
    }

    // ==================== build_telegram_safe_format Tests ====================

    #[test]
    fn test_build_telegram_safe_format_default() {
        let format = build_telegram_safe_format(None);
        // Should contain avc1 codec preference for telegram compatibility
        assert!(format.contains("avc1"));
        assert!(format.contains("mp4a"));
        // Should have fallbacks
        assert!(format.contains("/best"));
    }

    #[test]
    fn test_build_telegram_safe_format_1080p() {
        let format = build_telegram_safe_format(Some(1080));
        assert!(format.contains("[height<=1080]"));
        assert!(format.contains("avc1"));
    }

    #[test]
    fn test_build_telegram_safe_format_720p() {
        let format = build_telegram_safe_format(Some(720));
        assert!(format.contains("[height<=720]"));
        assert!(format.contains("avc1"));
    }

    #[test]
    fn test_build_telegram_safe_format_480p() {
        let format = build_telegram_safe_format(Some(480));
        assert!(format.contains("[height<=480]"));
    }

    #[test]
    fn test_build_telegram_safe_format_custom_height() {
        let format = build_telegram_safe_format(Some(144));
        // Custom height should be first in the chain
        assert!(format.contains("[height<=144]"));
    }

    #[test]
    fn test_build_telegram_safe_format_has_fallbacks() {
        let format = build_telegram_safe_format(Some(720));
        // Should have progressive fallback to lower qualities
        assert!(format.contains("[height<=720]"));
        assert!(format.contains("[height<=480]"));
        assert!(format.contains("[height<=360]"));
        // Final fallback
        assert!(format.contains("best[ext=mp4]"));
        assert!(format.ends_with("/best"));
    }

    // ==================== read_log_tail Tests ====================

    #[test]
    fn test_read_log_tail_nonexistent_file() {
        let result = read_log_tail(&PathBuf::from("/nonexistent/log.txt"), 1024);
        assert!(result.is_err());
    }

    #[test]
    fn test_read_log_tail_small_file() {
        use std::io::Write;
        let temp_dir = std::env::temp_dir();
        let temp_file = temp_dir.join(format!("test_log_small_{}.txt", std::process::id()));

        let mut file = std::fs::File::create(&temp_file).unwrap();
        writeln!(file, "Line 1").unwrap();
        writeln!(file, "Line 2").unwrap();
        drop(file);

        let result = read_log_tail(&temp_file, 1024).unwrap();
        let _ = fs_err::remove_file(&temp_file);

        assert!(result.contains("Line 1"));
        assert!(result.contains("Line 2"));
    }

    #[test]
    fn test_read_log_tail_truncates_large_file() {
        use std::io::Write;
        let temp_dir = std::env::temp_dir();
        let temp_file = temp_dir.join(format!("test_log_large_{}.txt", std::process::id()));

        let mut file = std::fs::File::create(&temp_file).unwrap();
        // Write more than max_bytes
        for i in 0..100 {
            writeln!(file, "Line number {}", i).unwrap();
        }
        drop(file);

        let result = read_log_tail(&temp_file, 50).unwrap();
        let _ = fs_err::remove_file(&temp_file);

        // Should only contain the tail
        assert!(result.len() <= 60); // Allow some margin for line boundaries
        // Should not contain the first lines
        assert!(!result.contains("Line number 0"));
    }
}
