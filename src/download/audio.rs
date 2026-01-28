//! Audio download and processing module
//!
//! This module handles downloading audio files from URLs using yt-dlp,
//! tracking progress, and sending them to Telegram users.

use crate::core::config;
use crate::core::error::AppError;
use crate::core::error_logger::{self, ErrorType, UserContext};
use crate::core::metrics;
use crate::core::rate_limiter::RateLimiter;
use crate::core::truncate_tail_utf8;
use crate::core::utils::escape_filename;
use crate::download::downloader::{generate_file_name, parse_progress, spawn_downloader_with_fallback, ProgressInfo};
use crate::download::metadata::{add_cookies_args, get_metadata_from_ytdlp, probe_duration_seconds};
use crate::download::progress::{DownloadStatus, ProgressMessage};
use crate::download::send::{send_audio_with_retry, send_error_with_sticker, send_error_with_sticker_and_message};
use crate::download::ytdlp_errors::{
    analyze_ytdlp_error, get_error_message, sanitize_user_error_message, should_notify_admin, YtDlpErrorType,
};
use crate::storage::db::{self as db, save_download_history, DbPool};
use crate::telegram::notifications::notify_admin_text;
use crate::telegram::Bot;
use chrono::{DateTime, Utc};
use std::fs;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::Arc;
use teloxide::prelude::*;
use url::Url;

/// Downloads audio file without progress tracking
///
/// This is a simpler version that downloads audio synchronously.
/// For most use cases, prefer `download_audio_file_with_progress`.
#[allow(dead_code)]
pub fn download_audio_file(url: &Url, download_path: &str) -> Result<Option<u32>, AppError> {
    let ytdl_bin = &*config::YTDL_BIN;
    let args = [
        "-o",
        download_path,
        "--newline",
        "--extract-audio",
        "--audio-format",
        "mp3",
        "--audio-quality",
        "0",
        "--add-metadata",
        "--embed-thumbnail",
        "--no-playlist",
        "--concurrent-fragments",
        "1",
        "--postprocessor-args",
        "-acodec libmp3lame -b:a 320k",
        url.as_str(),
    ];
    let mut child = spawn_downloader_with_fallback(ytdl_bin, &args)?;
    let status = child
        .wait()
        .map_err(|e| AppError::Download(format!("downloader process failed: {}", e)))?;
    if !status.success() {
        return Err(AppError::Download(format!("downloader exited with status: {}", status)));
    }
    Ok(probe_duration_seconds(download_path))
}

/// Downloads audio with real-time progress tracking via channel
///
/// Returns a receiver for progress updates and a join handle for the download task.
/// The download runs in a blocking task to read stdout line by line.
pub async fn download_audio_file_with_progress(
    admin_bot: Bot,
    user_chat_id: ChatId,
    url: &Url,
    download_path: &str,
    bitrate: Option<String>,
) -> Result<
    (
        tokio::sync::mpsc::UnboundedReceiver<ProgressInfo>,
        tokio::task::JoinHandle<Result<Option<u32>, AppError>>,
    ),
    AppError,
> {
    let ytdl_bin = config::YTDL_BIN.clone();
    let url_str = url.to_string();
    let download_path_clone = download_path.to_string();
    let bitrate_str = bitrate.unwrap_or_else(|| "320k".to_string());
    let runtime_handle = tokio::runtime::Handle::current();

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

    let handle = tokio::task::spawn_blocking(move || {
        let postprocessor_args = format!("-acodec libmp3lame -b:a {}", bitrate_str);

        let mut args: Vec<&str> = vec![
            "-o",
            &download_path_clone,
            "--newline",
            "--extract-audio",
            "--audio-format",
            "mp3",
            "--audio-quality",
            "0",
            "--add-metadata",
            "--embed-thumbnail",
            "--no-playlist",
            "--concurrent-fragments",
            "1",
            "--fragment-retries",
            "10",
            "--socket-timeout",
            "30",
            "--http-chunk-size",
            "2097152",
            "--sleep-requests",
            "1",
            "--sleep-interval",
            "2",
            "--max-sleep-interval",
            "5",
        ];
        add_cookies_args(&mut args);

        // Use default web client with cookies (not Android which requires PO Token)
        args.push("--extractor-args");
        args.push("youtube:player_client=default,web_safari,web_embedded");

        // Use Node.js for YouTube n-challenge solving
        args.push("--js-runtimes");
        args.push("node");

        args.extend_from_slice(&[
            "--no-check-certificate",
            "--postprocessor-args",
            &postprocessor_args,
            &url_str,
        ]);

        let command_str = format!("{} {}", ytdl_bin, args.join(" "));
        log::info!("[DEBUG] yt-dlp command for audio download: {}", command_str);

        let mut child = Command::new(&ytdl_bin)
            .args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| AppError::Download(format!("Failed to spawn yt-dlp: {}", e)))?;

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        let stderr_lines = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let stdout_lines = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));

        use std::thread;
        let tx_clone = tx.clone();
        let stderr_lines_clone = Arc::clone(&stderr_lines);
        let stdout_lines_clone = Arc::clone(&stdout_lines);

        if let Some(stderr_stream) = stderr {
            thread::spawn(move || {
                let reader = BufReader::new(stderr_stream);
                for line in reader.lines() {
                    if let Ok(line_str) = line {
                        log::debug!("yt-dlp stderr: {}", line_str);
                        if let Ok(mut lines) = stderr_lines_clone.lock() {
                            lines.push(line_str.clone());
                            if lines.len() > 200 {
                                lines.remove(0);
                            }
                        }
                        if let Some(progress_info) = parse_progress(&line_str) {
                            log::info!("Parsed progress from stderr: {}%", progress_info.percent);
                            let _ = tx_clone.send(progress_info);
                        }
                    }
                }
            });
        }

        if let Some(stdout_stream) = stdout {
            let reader = BufReader::new(stdout_stream);
            for line in reader.lines() {
                if let Ok(line_str) = line {
                    log::debug!("yt-dlp stdout: {}", line_str);
                    if let Ok(mut lines) = stdout_lines_clone.lock() {
                        lines.push(line_str.clone());
                        if lines.len() > 200 {
                            lines.remove(0);
                        }
                    }
                    if let Some(progress_info) = parse_progress(&line_str) {
                        let _ = tx.send(progress_info);
                    }
                }
            }
        }

        let status = child
            .wait()
            .map_err(|e| AppError::Download(format!("downloader process failed: {}", e)))?;

        if !status.success() {
            let stderr_text = if let Ok(lines) = stderr_lines.lock() {
                lines.join("\n")
            } else {
                String::new()
            };
            let stdout_text = if let Ok(lines) = stdout_lines.lock() {
                lines.join("\n")
            } else {
                String::new()
            };

            if !stderr_text.is_empty() {
                let error_type = analyze_ytdlp_error(&stderr_text);

                let error_category = match error_type {
                    YtDlpErrorType::InvalidCookies => "invalid_cookies",
                    YtDlpErrorType::BotDetection => "bot_detection",
                    YtDlpErrorType::VideoUnavailable => "video_unavailable",
                    YtDlpErrorType::NetworkError => "network",
                    YtDlpErrorType::FragmentError => "fragment_error",
                    YtDlpErrorType::Unknown => "ytdlp_unknown",
                };
                let operation = format!("audio_download:{}", error_category);
                metrics::record_error("download", &operation);

                log::error!("yt-dlp download failed, error type: {:?}", error_type);
                log::error!("yt-dlp stderr: {}", stderr_text);

                if should_notify_admin(&error_type) {
                    log::warn!("This error requires administrator attention!");
                    let admin_message = format!(
                        "YTDLP ERROR (audio download)\nuser_chat_id: {}\nurl: {}\nerror_type: {:?}\n\ncommand:\n{}\n\nstdout (tail):\n{}\n\nstderr (tail):\n{}",
                        user_chat_id.0,
                        url_str,
                        error_type,
                        command_str,
                        truncate_tail_utf8(&stdout_text, 6000),
                        truncate_tail_utf8(&stderr_text, 6000),
                    );
                    let bot_for_admin = admin_bot.clone();
                    runtime_handle.spawn(async move {
                        notify_admin_text(&bot_for_admin, &admin_message).await;
                    });
                }

                return Err(AppError::Download(get_error_message(&error_type)));
            } else {
                metrics::record_error("download", "audio_download");
                return Err(AppError::Download(format!("downloader exited with status: {}", status)));
            }
        }

        Ok(probe_duration_seconds(&download_path_clone))
    });

    Ok((rx, handle))
}

/// Download audio file and send it to user
///
/// Downloads audio from URL using yt-dlp, shows progress updates, validates file size,
/// and sends the file to the user via Telegram.
///
/// # Arguments
///
/// * `bot` - Telegram bot instance
/// * `chat_id` - User's chat ID
/// * `url` - URL to download from
/// * `rate_limiter` - Rate limiter instance (unused but kept for API consistency)
/// * `_created_timestamp` - Timestamp when task was created (unused)
/// * `db_pool` - Optional database pool for user preferences and history
/// * `audio_bitrate` - Optional bitrate (defaults to 320k)
/// * `message_id` - Optional message ID to mark as completed
///
/// # Behavior
///
/// 1. Fetches metadata (title, artist) from yt-dlp
/// 2. Shows starting status message
/// 3. Downloads audio with real-time progress updates
/// 4. Validates file size (max 49 MB)
/// 5. Sends audio file with retry logic
/// 6. Shows success message
/// 7. Cleans up temporary file after delay
pub async fn download_and_send_audio(
    bot: Bot,
    chat_id: ChatId,
    url: Url,
    rate_limiter: Arc<RateLimiter>,
    _created_timestamp: DateTime<Utc>,
    db_pool: Option<Arc<DbPool>>,
    audio_bitrate: Option<String>,
    message_id: Option<i32>,
) -> ResponseResult<()> {
    log::info!(
        "Starting download_and_send_audio for chat {} with URL: {}",
        chat_id,
        url
    );
    let bot_clone = bot.clone();
    let _rate_limiter = Arc::clone(&rate_limiter);
    let db_pool_clone = db_pool.clone();

    tokio::spawn(async move {
        log::info!("Inside spawn for audio download, chat_id: {}", chat_id);
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
        metrics::record_format_request("mp3", &user_plan);

        // Start metrics timer for this download
        let quality = audio_bitrate.as_deref().unwrap_or("default");
        let timer = metrics::DOWNLOAD_DURATION_SECONDS
            .with_label_values(&["mp3", quality])
            .start_timer();

        let result: Result<(), AppError> = async {
            // Step 1: Get metadata and show starting status
            let (title, artist) = match get_metadata_from_ytdlp(Some(&bot_clone), Some(chat_id), &url).await {
                Ok(meta) => meta,
                Err(e) => {
                    log::error!("Failed to get metadata: {:?}", e);
                    if e.to_string().contains("timed out") {
                        log::warn!("yt-dlp timed out, sending error message to user");
                        send_error_with_sticker(&bot_clone, chat_id).await;
                    }
                    return Err(e);
                }
            };

            let display_title: Arc<str> = if artist.trim().is_empty() {
                Arc::from(title.as_str())
            } else {
                Arc::from(format!("{} - {}", artist, title))
            };

            let caption: Arc<str> = Arc::from(crate::core::utils::format_media_caption(&title, &artist));

            log::info!("Display title for audio: '{}'", display_title);
            log::info!("Formatted caption for audio: '{}'", caption);

            // Show starting status
            let _ = progress_msg
                .update(
                    &bot_clone,
                    DownloadStatus::Starting {
                        title: display_title.as_ref().to_string(),
                        file_format: Some("mp3".to_string()),
                    },
                )
                .await;

            let file_name = generate_file_name(&title, &artist);
            let safe_filename = escape_filename(&file_name);
            let full_path = format!("{}/{}", &*config::DOWNLOAD_FOLDER, safe_filename);
            let download_path = shellexpand::tilde(&full_path).into_owned();

            // Step 2: Download with real-time progress updates
            let (mut progress_rx, mut download_handle) = download_audio_file_with_progress(
                bot_clone.clone(),
                chat_id,
                &url,
                &download_path,
                audio_bitrate.clone(),
            )
            .await?;

            // Show initial 0% progress
            let _ = progress_msg
                .update(
                    &bot_clone,
                    DownloadStatus::Downloading {
                        title: display_title.as_ref().to_string(),
                        progress: 0,
                        speed_mbs: None,
                        eta_seconds: None,
                        current_size: None,
                        total_size: None,
                        file_format: Some("mp3".to_string()),
                    },
                )
                .await;

            // Read progress updates from channel
            let bot_for_progress = bot_clone.clone();
            let title_for_progress = Arc::clone(&display_title);
            let mut last_progress = 0u8;

            let duration_result = loop {
                tokio::select! {
                    Some(progress_info) = progress_rx.recv() => {
                        // Don't let progress go backwards, filter false early 100%
                        let mut safe_progress = progress_info.percent.clamp(last_progress, 100);
                        if safe_progress == 100 && last_progress < 90 {
                            safe_progress = last_progress;
                        }

                        let progress_diff = safe_progress.saturating_sub(last_progress);

                        if progress_diff >= 5 {
                            last_progress = safe_progress;
                            log::info!("Updating progress UI: {}%", safe_progress);
                            let _ = progress_msg.update(&bot_for_progress, DownloadStatus::Downloading {
                                title: title_for_progress.as_ref().to_string(),
                                progress: safe_progress,
                                speed_mbs: progress_info.speed_mbs,
                                eta_seconds: progress_info.eta_seconds,
                                current_size: progress_info.current_size,
                                total_size: progress_info.total_size,
                                file_format: Some("mp3".to_string()),
                            }).await;
                        }
                    }
                    result = &mut download_handle => {
                        // Draw progress to 100% after successful download
                        if last_progress < 100 {
                            let _ = progress_msg.update(&bot_for_progress, DownloadStatus::Downloading {
                                title: title_for_progress.as_ref().to_string(),
                                progress: 100,
                                speed_mbs: None,
                                eta_seconds: None,
                                current_size: None,
                                total_size: None,
                                file_format: Some("mp3".to_string()),
                            }).await;
                            let _ = last_progress; // Suppress unused warning
                        }
                        break result.map_err(|e| AppError::Download(format!("Task join error: {}", e)))??;
                    }
                }
            };

            log::debug!("Download path: {:?}", download_path);

            let duration: u32 = duration_result.unwrap_or(0);

            let elapsed_secs = start_time.elapsed().as_secs();
            log::info!("Audio downloaded in {} seconds", elapsed_secs);

            // Step 3: Validate file size before sending
            let file_size = fs::metadata(&download_path)
                .map_err(|e| AppError::Download(format!("Failed to get file metadata: {}", e)))?
                .len();

            let max_audio_size = config::validation::max_audio_size_bytes();
            if file_size > max_audio_size {
                let size_mb = file_size as f64 / (1024.0 * 1024.0);
                let max_mb = max_audio_size as f64 / (1024.0 * 1024.0);
                log::warn!("Audio file too large: {:.2} MB (max: {:.2} MB)", size_mb, max_mb);
                send_error_with_sticker(&bot_clone, chat_id).await;
                let _ = progress_msg
                    .update(
                        &bot_clone,
                        DownloadStatus::Error {
                            title: display_title.as_ref().to_string(),
                            error: format!(
                                "Файл слишком большой ({:.2} MB). Максимальный размер: {:.2} MB",
                                size_mb, max_mb
                            ),
                            file_format: Some("mp3".to_string()),
                        },
                    )
                    .await;
                return Err(AppError::Validation(format!("Файл слишком большой: {:.2} MB", size_mb)));
            }

            // Step 4: Get user preference for send_audio_as_document
            let send_audio_as_document = if let Some(ref pool) = db_pool_clone {
                match db::get_connection(pool) {
                    Ok(conn) => db::get_user_send_audio_as_document(&conn, chat_id.0).unwrap_or(0) == 1,
                    Err(e) => {
                        log::warn!(
                            "Failed to get db connection for send_audio_as_document preference: {}",
                            e
                        );
                        false
                    }
                }
            } else {
                false
            };

            // Step 5: Send audio with retry logic and get the sent message
            let (sent_message, file_size) = send_audio_with_retry(
                &bot_clone,
                chat_id,
                &download_path,
                duration,
                &mut progress_msg,
                caption.as_ref(),
                send_audio_as_document,
            )
            .await?;

            // Update progress message to Success immediately after sending
            let elapsed_secs = start_time.elapsed().as_secs();
            let _ = progress_msg
                .update(
                    &bot_clone,
                    DownloadStatus::Success {
                        title: display_title.as_ref().to_string(),
                        elapsed_secs,
                        file_format: Some("mp3".to_string()),
                    },
                )
                .await;

            // Add audio effects button for Premium/VIP users
            // Copy file BEFORE it gets deleted
            log::info!(
                "Audio effects: checking if we should add button (db_pool exists: {})",
                db_pool_clone.is_some()
            );
            if let Some(ref pool) = db_pool_clone {
                log::info!("Audio effects: db_pool exists, getting connection");
                if let Ok(conn) = crate::storage::db::get_connection(pool) {
                    log::info!("Audio effects: got DB connection");
                    // TODO: Re-enable premium check after testing
                    // if crate::storage::db::is_premium_or_vip(&conn, chat_id.0).unwrap_or(false) {
                    if true {
                        // Temporarily enabled for all users for testing
                        log::info!("Audio effects: premium check passed (testing mode)");
                        use crate::download::audio_effects::{self, AudioEffectSession};
                        use crate::storage::db;

                        let session_id = uuid::Uuid::new_v4().to_string();
                        let session_file_path_raw =
                            audio_effects::get_original_file_path(&session_id, &config::DOWNLOAD_FOLDER);
                        let session_file_path = shellexpand::tilde(&session_file_path_raw).into_owned();

                        log::info!(
                            "Audio effects: attempting to copy file from '{}' to '{}'",
                            download_path,
                            session_file_path
                        );
                        log::info!(
                            "Audio effects: checking if source file exists: {}",
                            std::path::Path::new(&download_path).exists()
                        );

                        // Copy file synchronously before it gets deleted
                        match std::fs::copy(&download_path, &session_file_path) {
                            Ok(bytes) => {
                                log::info!(
                                    "Audio effects: successfully copied {} bytes to {}",
                                    bytes,
                                    session_file_path
                                );
                                let session = AudioEffectSession::new(
                                    session_id.clone(),
                                    chat_id.0,
                                    session_file_path,
                                    sent_message.id.0,
                                    display_title.as_ref().to_string(),
                                    duration,
                                );

                                match db::create_audio_effect_session(&conn, &session) {
                                    Ok(_) => {
                                        log::info!("Audio effects: session created in DB with id {}", session_id);
                                        let bot_for_button = bot_clone.clone();
                                        let session_id_clone = session_id.clone();
                                        tokio::spawn(async move {
                                            use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};

                                            let keyboard = InlineKeyboardMarkup::new(vec![vec![
                                                InlineKeyboardButton::callback(
                                                    "Edit Audio",
                                                    format!("ae:open:{}", session_id_clone),
                                                ),
                                                InlineKeyboardButton::callback(
                                                    "Cut Audio",
                                                    format!("ac:open:{}", session_id_clone),
                                                ),
                                            ]]);

                                            log::info!(
                                                "Audio effects: attempting to add button to message {}",
                                                sent_message.id.0
                                            );
                                            if let Err(e) = bot_for_button
                                                .edit_message_reply_markup(chat_id, sent_message.id)
                                                .reply_markup(keyboard)
                                                .await
                                            {
                                                log::warn!("Failed to add audio effects button: {}", e);
                                            } else {
                                                log::info!(
                                                    "Added audio effects button to message {} for session {}",
                                                    sent_message.id.0,
                                                    session_id_clone
                                                );
                                            }
                                        });
                                    }
                                    Err(e) => {
                                        log::warn!("Failed to create audio effect session in DB: {}", e);
                                    }
                                }
                            }
                            Err(e) => {
                                log::warn!(
                                    "Failed to copy file for audio effects session: {} (source: {}, dest: {})",
                                    e,
                                    download_path,
                                    session_file_path
                                );
                            }
                        }
                    } else {
                        log::info!("Audio effects: user is not premium/vip");
                    }
                } else {
                    log::warn!("Audio effects: failed to get DB connection");
                }
            } else {
                log::warn!("Audio effects: db_pool is None");
            }

            // Save to download history after successful send
            if let Some(ref pool) = db_pool_clone {
                if let Ok(conn) = crate::storage::db::get_connection(pool) {
                    let file_id = sent_message
                        .audio()
                        .map(|a| a.file.id.0.clone())
                        .or_else(|| sent_message.document().map(|d| d.file.id.0.clone()));

                    let author_opt = if !artist.trim().is_empty() {
                        Some(artist.as_str())
                    } else {
                        None
                    };

                    let bitrate = audio_bitrate.as_deref().unwrap_or("320k");

                    match save_download_history(
                        &conn,
                        chat_id.0,
                        url.as_str(),
                        title.as_str(),
                        "mp3",
                        file_id.as_deref(),
                        author_opt,
                        Some(file_size as i64),
                        Some(duration as i64),
                        None, // video_quality (N/A for mp3)
                        Some(bitrate),
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

            log::info!("Audio sent successfully to chat {}", chat_id);

            // Step 5: Auto-clear success message after delay
            let bot_for_clear = bot_clone.clone();
            let title_for_clear = Arc::clone(&display_title);
            let mut msg_for_clear = ProgressMessage {
                chat_id: progress_msg.chat_id,
                message_id: progress_msg.message_id,
            };
            tokio::spawn(async move {
                let _ = msg_for_clear
                    .clear_after(
                        &bot_for_clear,
                        config::progress::CLEAR_DELAY_SECS,
                        title_for_clear.as_ref().to_string(),
                        Some("mp3".to_string()),
                    )
                    .await;
            });

            // Wait before cleaning up file
            tokio::time::sleep(config::download::cleanup_delay()).await;
            if let Err(e) = fs::remove_file(&download_path) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    return Err(AppError::Download(format!("Failed to delete file: {}", e)))?;
                }
            }

            Ok(())
        }
        .await;

        match result {
            Ok(_) => {
                log::info!("Audio download completed successfully for chat {}", chat_id);
                timer.observe_duration();
                metrics::record_download_success("mp3", quality);
            }
            Err(e) => {
                e.track_with_operation("audio_download");
                log::error!("An error occurred during audio download for chat {}: {:?}", chat_id, e);
                timer.observe_duration();
                let error_type = if e.to_string().contains("too large") {
                    "file_too_large"
                } else if e.to_string().contains("timed out") {
                    "timeout"
                } else {
                    "other"
                };
                metrics::record_download_failure("mp3", error_type);

                let user_ctx = UserContext::new(chat_id.0, None);
                let err_type = match error_type {
                    "file_too_large" => ErrorType::FileTooLarge,
                    "timeout" => ErrorType::Timeout,
                    _ => ErrorType::DownloadFailed,
                };
                error_logger::log_error(
                    err_type,
                    &e.to_string(),
                    &user_ctx,
                    Some(url.as_str()),
                    Some(r#"{"format":"mp3"}"#),
                );

                let error_str = e.to_string();
                let user_error = sanitize_user_error_message(&error_str);
                let custom_message = if error_str.contains("Only images are available") {
                    Some(
                        "Это видео недоступно для скачивания\n\n\
                    Возможные причины:\n\
                    - Видео удалено или приватное\n\
                    - Возрастные ограничения\n\
                    - Региональные ограничения\n\
                    - Стрим или премьера (еще не доступны)\n\n\
                    Попробуй другое видео!",
                    )
                } else if error_str.contains("Signature extraction failed") {
                    Some(
                        "У меня устарела версия загрузчика\n\n\
                    Стэн уже знает и скоро обновит!\n\
                    Попробуй позже или другое видео.",
                    )
                } else if error_str.contains("Sign in to confirm you're not a bot")
                    || error_str.contains("bot detection")
                {
                    Some(
                        "YouTube заблокировал бота\n\n\
                    Нужно настроить cookies.\n\
                    Стэн уже знает и разбирается!\n\n\
                    Попробуй позже.",
                    )
                } else {
                    None
                };

                let display_error = custom_message.unwrap_or(user_error.as_str());

                send_error_with_sticker_and_message(&bot_clone, chat_id, custom_message).await;
                let _ = progress_msg
                    .update(
                        &bot_clone,
                        DownloadStatus::Error {
                            title: "Скачивание".to_string(),
                            file_format: Some("mp3".to_string()),
                            error: display_error.to_string(),
                        },
                    )
                    .await;
            }
        }
    });
    log::info!("download_and_send_audio function returned, spawn task started");
    Ok(())
}
