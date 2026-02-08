use crate::core::config;
use crate::core::error::AppError;
use crate::core::error_logger::{self, ErrorType, UserContext};
use crate::core::metrics;
use crate::core::rate_limiter::RateLimiter;
use crate::core::utils::{escape_filename, sanitize_filename};
use crate::download::metadata::{add_cookies_args, get_metadata_from_ytdlp, probe_video_metadata};
use crate::download::progress::{DownloadStatus, ProgressMessage};
use crate::download::proxy::ProxyListManager;
use crate::download::send::send_error_with_sticker;
use crate::download::ytdlp_errors::sanitize_user_error_message;
use crate::storage::db::{self as db, DbPool};
use crate::telegram::Bot;
use chrono::{DateTime, Utc};
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::InputFile;
use tokio::process::Command as TokioCommand;
use url::Url;

/// Cleans up all partial/temporary files created by yt-dlp for a download path
///
/// yt-dlp creates various intermediate files during download:
/// - `.part` - partial download
/// - `.ytdl` - download state
/// - `.temp.{ext}` - temporary merge files
/// - `.f{N}.{ext}` - format-specific fragments
/// - `.info.json` - metadata cache
///
/// This function removes all of them to prevent disk space leaks.
pub fn cleanup_partial_download(base_path: &str) {
    let base = Path::new(base_path);
    let parent = base.parent().unwrap_or(Path::new("."));
    let filename = base.file_name().and_then(|n| n.to_str()).unwrap_or("");

    // Remove exact known patterns
    let patterns = [".part", ".ytdl", ".temp.mp4", ".temp.webm", ".info.json"];
    for pattern in patterns {
        let path = format!("{}{}", base_path, pattern);
        if let Err(e) = fs::remove_file(&path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                log::debug!("Failed to remove {}: {}", path, e);
            }
        }
    }

    // Remove fragment files (.f{N}.{ext}) using glob pattern
    // These are created when yt-dlp downloads separate audio/video streams
    if let Ok(entries) = fs::read_dir(parent) {
        let base_name = filename
            .trim_end_matches(".mp4")
            .trim_end_matches(".mp3")
            .trim_end_matches(".webm");
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                // Match patterns like: basename.f123.mp4, basename.f456.webm, etc.
                if name.starts_with(base_name)
                    && (name.contains(".f") || name.ends_with(".part") || name.ends_with(".ytdl"))
                {
                    let path = entry.path();
                    if let Err(e) = fs::remove_file(&path) {
                        if e.kind() != std::io::ErrorKind::NotFound {
                            log::debug!("Failed to remove fragment {}: {}", path.display(), e);
                        }
                    }
                }
            }
        }
    }

    log::debug!("Cleaned up partial files for: {}", base_path);
}

/// Legacy alias for backward compatibility
/// Use AppError instead
#[deprecated(note = "Use AppError instead")]
pub type CommandError = AppError;

// extract_retry_after, is_timeout_or_network_error, truncate_tail_utf8 are now imported from crate::core

// UploadProgress, ProgressReader, input_file_with_progress, read_log_tail, log_bot_api_speed_for_file moved to send.rs

// is_local_bot_api is now in crate::core::config::bot_api

// validate_cookies_file_format and add_cookies_args moved to metadata.rs

/// Selects a proxy from configured sources and returns the proxy URL if available
///
/// Returns the proxy URL if configured and healthy, None otherwise
#[allow(dead_code)]
async fn get_proxy_for_download() -> Option<String> {
    // Skip if proxy system is not configured
    if config::proxy::WARP_PROXY.is_none() && config::proxy::PROXY_FILE.is_none() {
        return None;
    }

    // Create proxy manager with configured strategy
    let manager = ProxyListManager::new(config::proxy::get_selection_strategy());

    // Select a proxy for this download
    if let Some(proxy) = manager.select().await {
        log::debug!("Selected proxy for download: {}", proxy);
        Some(proxy.to_string())
    } else {
        log::warn!("Proxy configured but no healthy proxies available");
        None
    }
}

// probe_duration_seconds, has_both_video_and_audio, probe_video_metadata,
// build_telegram_safe_format, find_actual_downloaded_file, get_metadata_from_ytdlp
// moved to metadata.rs

// send_error_with_sticker, send_error_with_sticker_and_message moved to send.rs

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
                        AppError::Download(format!(
                            "Failed to start downloader. Tried '{}', then '{}': {} / {}",
                            ytdl_bin, fallback, e, inner
                        ))
                    })
            } else {
                Err(AppError::Download(format!(
                    "Failed to start downloader '{}': {}",
                    ytdl_bin, e
                )))
            }
        })
}

/// Структура для хранения данных прогресса загрузки
#[derive(Debug, Clone)]
pub struct ProgressInfo {
    pub percent: u8,
    pub speed_mbs: Option<f64>,
    pub eta_seconds: Option<u64>,
    pub current_size: Option<u64>,
    pub total_size: Option<u64>,
}

/// Parses progress from yt-dlp output line
/// Example: "[download]  45.2% of 10.00MiB at 500.00KiB/s ETA 00:10"
pub fn parse_progress(line: &str) -> Option<ProgressInfo> {
    // Проверяем базовые требования
    if !line.contains("[download]") {
        return None;
    }

    // Для отладки: логируем все строки с [download]
    if !line.contains("%") {
        // Это может быть другое сообщение, например "[download] Destination: ..."
        log::trace!("Download line without percent: {}", line);
        return None;
    }

    let mut percent = None;
    let mut speed_mbs = None;
    let mut eta_seconds = None;
    let mut current_size = None;
    let mut total_size = None;

    // Парсим без аллокации Vec - используем peek iterator
    let mut parts = line.split_whitespace().peekable();
    while let Some(part) = parts.next() {
        // Парсим процент
        if part.ends_with('%') {
            if let Ok(p) = part.trim_end_matches('%').parse::<f32>() {
                // Обрезаем в разумные границы, чтобы не прыгать на 100% при мусорных данных
                let clamped = p.clamp(0.0, 100.0) as u8;
                percent = Some(clamped);
            }
        }

        // Парсим размер: "of 10.00MiB"
        if part == "of" {
            if let Some(&next) = parts.peek() {
                if let Some(size_bytes) = parse_size(next) {
                    total_size = Some(size_bytes);
                }
            }
        }

        // Парсим скорость: "at 500.00KiB/s" или "at 2.3MiB/s"
        if part == "at" {
            if let Some(&next) = parts.peek() {
                if let Some(speed) = parse_size(next) {
                    // Конвертируем в MB/s
                    speed_mbs = Some(speed as f64 / (1024.0 * 1024.0));
                }
            }
        }

        // Парсим ETA: "ETA 00:10" или "ETA 1:23"
        if part == "ETA" {
            if let Some(&next) = parts.peek() {
                if let Some(eta) = parse_eta(next) {
                    eta_seconds = Some(eta);
                }
            }
        }
    }

    // Если есть процент, возвращаем ProgressInfo
    if let Some(p) = percent {
        // Вычисляем текущий размер на основе процента
        if let Some(total) = total_size {
            current_size = Some((total as f64 * (p as f64 / 100.0)) as u64);
        }

        log::debug!(
            "Progress parsed successfully: {}% (speed: {:?} MB/s, eta: {:?}s)",
            p,
            speed_mbs,
            eta_seconds
        );

        Some(ProgressInfo {
            percent: p,
            speed_mbs,
            eta_seconds,
            current_size,
            total_size,
        })
    } else {
        log::debug!("Could not parse percent from line: {}", line);
        None
    }
}

/// Парсит размер из строки типа "10.00MiB" или "500.00KiB"
fn parse_size(size_str: &str) -> Option<u64> {
    let size_str = size_str.trim_end_matches("/s"); // Убираем "/s" если есть
    if size_str.ends_with("MiB") {
        if let Ok(mb) = size_str.trim_end_matches("MiB").parse::<f64>() {
            return Some((mb * 1024.0 * 1024.0) as u64);
        }
    } else if size_str.ends_with("KiB") {
        if let Ok(kb) = size_str.trim_end_matches("KiB").parse::<f64>() {
            return Some((kb * 1024.0) as u64);
        }
    } else if size_str.ends_with("GiB") {
        if let Ok(gb) = size_str.trim_end_matches("GiB").parse::<f64>() {
            return Some((gb * 1024.0 * 1024.0 * 1024.0) as u64);
        }
    }
    None
}

/// Парсит ETA из строки типа "00:10" или "1:23"
fn parse_eta(eta_str: &str) -> Option<u64> {
    // Use split_once to avoid Vec allocation
    let (minutes_str, seconds_str) = eta_str.split_once(':')?;
    let minutes: u64 = minutes_str.parse().ok()?;
    let seconds: u64 = seconds_str.parse().ok()?;
    Some(minutes * 60 + seconds)
}

// download_and_send_audio moved to audio.rs

// send_file_with_retry, send_audio_with_retry, send_video_with_retry moved to send.rs
pub fn generate_file_name(title: &str, artist: &str) -> String {
    generate_file_name_with_ext(title, artist, "mp3")
}

pub fn generate_file_name_with_ext(title: &str, artist: &str, extension: &str) -> String {
    let title_trimmed = title.trim();
    let artist_trimmed = artist.trim();

    log::debug!(
        "Generating filename: title='{}' (len={}), artist='{}' (len={}), ext='{}'",
        title,
        title.len(),
        artist,
        artist.len(),
        extension
    );

    let filename = if artist_trimmed.is_empty() && title_trimmed.is_empty() {
        log::warn!("Both title and artist are empty, using 'Unknown.{}'", extension);
        format!("Unknown.{}", extension)
    } else if artist_trimmed.is_empty() {
        log::debug!("Using title only: '{}.{}'", title_trimmed, extension);
        format!("{}.{}", title_trimmed, extension)
    } else if title_trimmed.is_empty() {
        log::debug!("Using artist only: '{}.{}'", artist_trimmed, extension);
        format!("{}.{}", artist_trimmed, extension)
    } else {
        log::debug!("Using both: '{} - {}.{}'", artist_trimmed, title_trimmed, extension);
        format!("{} - {}.{}", artist_trimmed, title_trimmed, extension)
    };

    // Заменяем пробелы на подчеркивания перед возвратом
    sanitize_filename(&filename)
}

/// Download subtitles file (SRT or TXT format) and send it to user
///
/// Downloads subtitles from URL using yt-dlp and sends them as a document.
///
/// # Arguments
///
/// * `bot` - Telegram bot instance
/// * `chat_id` - User's chat ID
/// * `url` - URL to download subtitles from
/// * `rate_limiter` - Rate limiter instance (unused but kept for API consistency)
/// * `_created_timestamp` - Timestamp when task was created (unused)
/// * `subtitle_format` - Subtitle format ("srt" or "txt")
///
/// # Returns
///
/// Returns `Ok(())` on success or a `ResponseResult` error.
pub async fn download_and_send_subtitles(
    bot: Bot,
    chat_id: ChatId,
    url: Url,
    rate_limiter: Arc<RateLimiter>,
    _created_timestamp: DateTime<Utc>,
    subtitle_format: String,
    db_pool: Option<Arc<DbPool>>,
    message_id: Option<i32>,
    _alert_manager: Option<Arc<crate::core::alerts::AlertManager>>,
) -> ResponseResult<()> {
    let bot_clone = bot.clone();
    let _rate_limiter = Arc::clone(&rate_limiter);
    let db_pool_clone = db_pool.clone();

    tokio::spawn(async move {
        let mut progress_msg = ProgressMessage::new(chat_id);
        let start_time = std::time::Instant::now();

        // Get user plan for metrics
        let user_plan = if let Some(ref pool) = db_pool_clone {
            if let Ok(conn) = db::get_connection(pool) {
                db::get_user(&conn, chat_id.0)
                    .ok()
                    .flatten()
                    .map(|u| u.plan)
                    .unwrap_or_else(|| "free".to_string())
            } else {
                "free".to_string()
            }
        } else {
            "free".to_string()
        };

        // Record format request for metrics
        let format = subtitle_format.as_str();
        metrics::record_format_request(format, &user_plan);

        // Start metrics timer for subtitles download
        let timer = metrics::DOWNLOAD_DURATION_SECONDS
            .with_label_values(&[format, "default"])
            .start_timer();

        let result: Result<(), AppError> = async {
            // Step 1: Get metadata
            let (title, _) = match get_metadata_from_ytdlp(Some(&bot_clone), Some(chat_id), &url).await {
                Ok(meta) => meta,
                Err(e) => {
                    log::error!("Failed to get metadata: {:?}", e);
                    // Проверяем, является ли это ошибкой таймаута
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

            // Логируем полную команду для отладки
            let command_str = format!("{} {}", ytdl_bin, args.join(" "));
            log::debug!("yt-dlp command for subtitles download: {}", command_str);

            // Run blocking download in spawn_blocking to avoid blocking async runtime
            let ytdl_bin_owned = ytdl_bin.to_string();
            let args_owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
            let download_result = tokio::task::spawn_blocking(move || {
                let args_refs: Vec<&str> = args_owned.iter().map(|s| s.as_str()).collect();
                let mut child = spawn_downloader_with_fallback(&ytdl_bin_owned, &args_refs)?;
                let status = child
                    .wait()
                    .map_err(|e| AppError::Download(format!("downloader process failed: {}", e)))?;
                Ok::<_, AppError>(status)
            })
            .await
            .map_err(|e| AppError::Download(format!("spawn_blocking failed: {}", e)))??;

            if !download_result.success() {
                return Err(AppError::Download(format!(
                    "downloader exited with status: {}",
                    download_result
                )));
            }

            // Check if file exists
            if fs::metadata(&download_path).is_err() {
                // Try to find the actual filename that was downloaded
                let parent_dir = shellexpand::tilde("~/downloads/").into_owned();
                let dir_entries = fs::read_dir(&parent_dir)
                    .map_err(|e| AppError::Download(format!("Failed to read downloads dir: {}", e)))?;
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
                        .map_err(|e| AppError::Download(format!("Failed to send document: {}", e)))?;

                    // NOTE: Subtitles are not saved to download_history as they won't appear in /downloads
                    // (We only save mp3/mp4 with file_id for the /downloads command)
                    // Subtitle tracking is intentionally disabled per requirements
                } else {
                    return Err(AppError::Download("Subtitle file not found".to_string()));
                }
            } else {
                // Send the file
                let _sent_message = bot_clone
                    .send_document(chat_id, InputFile::file(&download_path))
                    .await
                    .map_err(|e| AppError::Download(format!("Failed to send document: {}", e)))?;

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
                    crate::telegram::emoji::THUMBS_UP,
                )
                .await;
            }

            log::info!("Subtitle sent successfully to chat {}", chat_id);

            // Step 4: Auto-clear success message
            let bot_for_clear = bot_clone.clone();
            let title_for_clear = Arc::clone(&display_title);
            let mut msg_for_clear = ProgressMessage {
                chat_id: progress_msg.chat_id,
                message_id: progress_msg.message_id,
            };
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
            if let Err(e) = fs::remove_file(&download_path) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    return Err(AppError::Download(format!("Failed to delete file: {}", e)))?;
                }
                // File doesn't exist - that's fine, it was probably deleted manually
            }

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

                // Log error to database
                let user_ctx = UserContext::new(chat_id.0, None);
                let err_type = if error_type == "timeout" {
                    ErrorType::Timeout
                } else {
                    ErrorType::DownloadFailed
                };
                error_logger::log_error(
                    err_type,
                    &e.to_string(),
                    &user_ctx,
                    Some(url.as_str()),
                    Some(&format!(r#"{{"format":"{}"}}"#, format)),
                );
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
                        title: "Скачивание".to_string(),
                        error: user_error,
                        file_format: Some(subtitle_format.clone()),
                    },
                )
                .await;
        }
    });
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
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// burn_subtitles_into_video("input.mp4", "subtitles.srt", "output.mp4").await?;
/// # Ok(())
/// # }
/// ```
///
/// Splits a large video file into playable segments using ffmpeg.
/// This is used when the file exceeds Telegram's upload limits.
pub async fn split_video_into_parts(path: &str, target_part_size_bytes: u64) -> Result<Vec<String>, AppError> {
    log::info!("Checking if video needs splitting: {}", path);
    let file_size = fs::metadata(path)
        .map_err(|e| AppError::Download(format!("Failed to get file size: {}", e)))?
        .len();

    if file_size <= target_part_size_bytes {
        log::info!(
            "Video size {} is within limit {}, no splitting needed",
            file_size,
            target_part_size_bytes
        );
        return Ok(vec![path.to_string()]);
    }

    let metadata =
        probe_video_metadata(path).ok_or_else(|| AppError::Download(format!("Failed to probe video: {}", path)))?;
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

    let output = TokioCommand::new("ffmpeg")
        .args([
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
        ])
        .output()
        .await
        .map_err(|e| AppError::Download(format!("Failed to execute ffmpeg split: {}", e)))?;

    if !output.status.success() {
        return Err(AppError::Download(format!(
            "ffmpeg split failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    // Find all created parts
    let mut parts = Vec::new();
    let parent_dir = Path::new(path).parent().unwrap_or_else(|| Path::new("."));
    let file_stem = Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();

    for entry in fs::read_dir(parent_dir).map_err(|e| AppError::Download(e.to_string()))? {
        let entry = entry.map_err(|e| AppError::Download(e.to_string()))?;
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with(&file_stem) && name.contains("_part_") && name.ends_with(".mp4") {
            parts.push(entry.path().to_string_lossy().to_string());
        }
    }
    parts.sort();

    log::info!("Successfully split video into {} parts", parts.len());
    Ok(parts)
}

/// # use doradura::core::error::AppError;
/// # use doradura::download::downloader::burn_subtitles_into_video;
/// # async fn run() -> Result<(), AppError> {
/// burn_subtitles_into_video("video.mp4", "subtitles.srt", "video_with_subs.mp4").await?;
/// # Ok(())
/// # }
/// ```
pub async fn burn_subtitles_into_video(
    video_path: &str,
    subtitle_path: &str,
    output_path: &str,
) -> Result<(), AppError> {
    log::info!(
        "🔥 Burning subtitles into video: {} + {} -> {}",
        video_path,
        subtitle_path,
        output_path
    );

    // Проверяем наличие исходных файлов
    if !std::path::Path::new(video_path).exists() {
        return Err(AppError::Download(format!("Video file not found: {}", video_path)));
    }
    if !std::path::Path::new(subtitle_path).exists() {
        return Err(AppError::Download(format!(
            "Subtitle file not found: {}",
            subtitle_path
        )));
    }

    // Escape путь к субтитрам для ffmpeg filter
    // Важно: ffmpeg требует экранирования специальных символов в пути
    let escaped_subtitle_path = subtitle_path
        .replace("\\", "\\\\")
        .replace(":", "\\:")
        .replace("'", "\\'");

    // Команда ffmpeg для вшивания субтитров
    // Используем фильтр subtitles для наложения субтитров на видео
    // -c:v libx264 - используем H.264 кодек для видео
    // -c:a copy - копируем аудио без перекодирования
    // -preset fast - быстрая скорость кодирования
    let mut cmd = TokioCommand::new("ffmpeg");
    cmd.arg("-i")
        .arg(video_path)
        .arg("-vf")
        .arg(format!("subtitles='{}'", escaped_subtitle_path))
        .arg("-c:v")
        .arg("libx264")
        .arg("-c:a")
        .arg("copy")
        .arg("-preset")
        .arg("fast")
        .arg("-y") // Перезаписывать выходной файл если существует
        .arg(output_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    log::info!(
        "🎬 Running ffmpeg command: ffmpeg -i {} -vf subtitles='{}' -c:v libx264 -c:a copy -preset fast -y {}",
        video_path,
        escaped_subtitle_path,
        output_path
    );

    let output = cmd
        .output()
        .await
        .map_err(|e| AppError::Download(format!("Failed to execute ffmpeg: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        log::error!("❌ ffmpeg failed to burn subtitles: {}", stderr);
        return Err(AppError::Download(format!(
            "ffmpeg failed to burn subtitles: {}",
            stderr
        )));
    }

    // Проверяем что выходной файл был создан
    if !std::path::Path::new(output_path).exists() {
        return Err(AppError::Download(format!(
            "Output video file was not created: {}",
            output_path
        )));
    }

    log::info!("✅ Successfully burned subtitles into video: {}", output_path);
    Ok(())
}

// ==================== Audio Effects Integration ====================

#[cfg(test)]
mod download_tests {
    use super::*;
    use crate::core::{extract_retry_after, is_timeout_or_network_error, truncate_tail_utf8};
    use crate::download::metadata::{build_telegram_safe_format, probe_duration_seconds, validate_cookies_file_format};
    use crate::download::send::{read_log_tail, UploadProgress};
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

    #[test]
    fn test_probe_duration_seconds_handles_missing_file() {
        assert_eq!(probe_duration_seconds("/no/such/file.mp3"), None);
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
        let text = "Привет мир"; // Russian text (each Cyrillic char is 2 bytes)
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
        let _ = std::fs::remove_file(&temp_file);
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
        let _ = std::fs::remove_file(&temp_file);
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
        let _ = std::fs::remove_file(&temp_file);
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
        let _ = std::fs::remove_file(&temp_file);
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
        let _ = std::fs::remove_file(&temp_file);

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
        let _ = std::fs::remove_file(&temp_file);

        // Should only contain the tail
        assert!(result.len() <= 60); // Allow some margin for line boundaries
                                     // Should not contain the first lines
        assert!(!result.contains("Line number 0"));
    }
}
