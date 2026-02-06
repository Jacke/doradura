//! Audio download and processing module
//!
//! This module handles downloading audio files from URLs using yt-dlp,
//! tracking progress, and sending them to Telegram users.

use crate::core::config;
use crate::core::disk;
use crate::core::error::AppError;
use crate::core::error_logger::{self, ErrorType, UserContext};
use crate::core::metrics;
use crate::core::rate_limiter::RateLimiter;
use crate::core::truncate_tail_utf8;
use crate::core::utils::escape_filename;
use crate::download::cookies::report_and_wait_for_refresh;
use crate::download::downloader::{
    cleanup_partial_download, generate_file_name, parse_progress, spawn_downloader_with_fallback, ProgressInfo,
};
use crate::download::metadata::{
    add_cookies_args_with_proxy, add_no_cookies_args, get_estimated_filesize, get_metadata_from_ytdlp, get_proxy_chain,
    is_livestream, is_proxy_related_error, probe_duration_seconds,
};
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
use tokio::time::timeout;
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
        "ffmpeg:-acodec libmp3lame -b:a 320k",
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
        let postprocessor_args = format!("ffmpeg:-acodec libmp3lame -b:a {}", bitrate_str);

        // Get proxy chain for fallback: WARP → Residential → Direct
        let proxy_chain = get_proxy_chain();
        let total_proxies = proxy_chain.len();
        let mut last_error: Option<AppError> = None;

        // Try each proxy in the chain until one succeeds
        for (attempt, proxy_option) in proxy_chain.into_iter().enumerate() {
            let proxy_name = proxy_option
                .as_ref()
                .map(|p| p.name.clone())
                .unwrap_or_else(|| "Direct (no proxy)".to_string());

            log::info!(
                "📡 Audio download attempt {}/{} using [{}]",
                attempt + 1,
                total_proxies,
                proxy_name
            );

            // Clean up any partial download from previous attempt
            if attempt > 0 {
                let _ = std::fs::remove_file(&download_path_clone);
                // Comprehensive cleanup of all temp files created by yt-dlp
                cleanup_partial_download(&download_path_clone);
            }

            let mut args: Vec<&str> = vec![
                "-o",
                &download_path_clone,
                "--newline",
                "--force-overwrites", // Prevent postprocessing conflicts
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
                "2",
                "--sleep-interval",
                "3",
                "--max-sleep-interval",
                "10",
                "--limit-rate",
                "5M",
                // Exponential backoff for 403/rate-limit errors
                "--retry-sleep",
                "http:exp=1:30", // 1s -> 2s -> 4s -> ... up to 30s
                "--retry-sleep",
                "fragment:exp=1:30", // same for fragment errors
                "--retries",
                "15", // retry main request up to 15 times
            ];

            // v5.0 FALLBACK CHAIN: First try WITHOUT cookies (new yt-dlp 2026+ mode)
            // yt-dlp 2026.02.04+ automatically uses android_vr + web_safari clients
            // that don't require cookies or PO tokens for most videos
            add_no_cookies_args(&mut args, proxy_option.as_ref());

            // Use android + web_music clients (minimal BotGuard/attestation checks with WARP)
            args.push("--extractor-args");
            args.push("youtube:player_client=android,web_music;formats=missing_pot");

            // Use Deno JS runtime for YouTube challenge solving (yt-dlp 2026+)
            args.push("--js-runtimes");
            args.push("deno");

            // Impersonate browser TLS/HTTP fingerprint
            args.push("--impersonate");
            args.push("Chrome-131:Android-14");

            args.extend_from_slice(&[
                "--no-check-certificate",
                "--postprocessor-args",
                &postprocessor_args,
                &url_str,
            ]);

            let command_str = format!("{} {}", ytdl_bin, args.join(" "));
            log::debug!("yt-dlp command for audio download: {}", command_str);

            let child_result = Command::new(&ytdl_bin)
                .args(&args)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn();

            let mut child = match child_result {
                Ok(c) => c,
                Err(e) => {
                    log::error!("Failed to spawn yt-dlp: {}", e);
                    last_error = Some(AppError::Download(format!("Failed to spawn yt-dlp: {}", e)));
                    continue;
                }
            };

            let stdout = child.stdout.take();
            let stderr = child.stderr.take();

            // Use VecDeque for O(1) pop_front instead of O(n) Vec::remove(0)
            use std::collections::VecDeque;
            let stderr_lines = Arc::new(std::sync::Mutex::new(VecDeque::<String>::new()));
            let stdout_lines = Arc::new(std::sync::Mutex::new(VecDeque::<String>::new()));

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
                                lines.push_back(line_str.clone());
                                if lines.len() > 200 {
                                    lines.pop_front();
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
                            lines.push_back(line_str.clone());
                            if lines.len() > 200 {
                                lines.pop_front();
                            }
                        }
                        if let Some(progress_info) = parse_progress(&line_str) {
                            let _ = tx.send(progress_info);
                        }
                    }
                }
            }

            let status = match child.wait() {
                Ok(s) => s,
                Err(e) => {
                    log::error!("Downloader process failed: {}", e);
                    last_error = Some(AppError::Download(format!("downloader process failed: {}", e)));
                    continue;
                }
            };

            if status.success() {
                // Success! Log which proxy worked
                log::info!(
                    "✅ Audio download succeeded using [{}] (attempt {}/{})",
                    proxy_name,
                    attempt + 1,
                    total_proxies
                );
                return Ok(probe_duration_seconds(&download_path_clone));
            }

            // Download failed - check if we should try next proxy
            let stderr_text = if let Ok(mut lines) = stderr_lines.lock() {
                lines.make_contiguous().join("\n")
            } else {
                String::new()
            };
            let stdout_text = if let Ok(mut lines) = stdout_lines.lock() {
                lines.make_contiguous().join("\n")
            } else {
                String::new()
            };

            let error_type = analyze_ytdlp_error(&stderr_text);
            let error_msg = get_error_message(&error_type);

            log::error!(
                "❌ Download failed with [{}]: {:?} - {}",
                proxy_name,
                error_type,
                &stderr_text[..std::cmp::min(500, stderr_text.len())]
            );

            // Check if this is a proxy-related error that warrants trying next proxy
            let should_try_next = is_proxy_related_error(&stderr_text)
                || matches!(error_type, YtDlpErrorType::BotDetection | YtDlpErrorType::NetworkError);

            if should_try_next && attempt + 1 < total_proxies {
                log::warn!(
                    "🔄 Proxy-related error detected, will try next proxy (attempt {}/{})",
                    attempt + 2,
                    total_proxies
                );
                last_error = Some(AppError::Download(error_msg));
                continue;
            }

            // v5.0 FALLBACK CHAIN: No cookies failed, now try WITH cookies + PO Token
            // If the first attempt (without cookies) fails with bot detection or network error,
            // try again with full authentication (cookies + PO Token)
            if matches!(
                error_type,
                YtDlpErrorType::InvalidCookies | YtDlpErrorType::BotDetection | YtDlpErrorType::NetworkError
            ) {
                log::warn!(
                    "🍪 [TIER1→TIER2] No-cookies mode failed (error={:?}), trying WITH cookies + PO Token...",
                    error_type
                );
                log::warn!(
                    "🍪 [TIER1_STDERR] {}",
                    &stderr_text[..std::cmp::min(1000, stderr_text.len())]
                );

                // Log cookie file state before attempting with cookies
                crate::download::cookies::log_cookie_file_diagnostics("AUDIO_TIER2_BEFORE");

                // ATTEMPT 2: Try WITH cookies + PO Token
                // Comprehensive cleanup of all partial files
                let _ = std::fs::remove_file(&download_path_clone);
                cleanup_partial_download(&download_path_clone);

                let mut cookies_args: Vec<&str> = vec![
                    "-o",
                    &download_path_clone,
                    "--newline",
                    "--force-overwrites",
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
                ];

                // Add cookies + PO Token (full authentication)
                add_cookies_args_with_proxy(&mut cookies_args, proxy_option.as_ref());

                cookies_args.push("--extractor-args");
                cookies_args.push("youtube:player_client=web,web_safari");
                cookies_args.push("--js-runtimes");
                cookies_args.push("deno");
                cookies_args.push("--no-check-certificate");
                cookies_args.push("--postprocessor-args");
                cookies_args.push(&postprocessor_args);
                cookies_args.push(&url_str);

                log::info!("🔑 [WITH_COOKIES] Attempting audio download WITH cookies + PO Token...");

                let cookies_child = Command::new(&ytdl_bin)
                    .args(&cookies_args)
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn();

                if let Ok(child) = cookies_child {
                    // Simple wait without progress tracking for fallback
                    if let Ok(output) = child.wait_with_output() {
                        if output.status.success() {
                            log::info!("✅ [WITH_COOKIES] Audio download succeeded WITH cookies!");
                            return Ok(probe_duration_seconds(&download_path_clone));
                        } else {
                            let cookies_stderr = String::from_utf8_lossy(&output.stderr);
                            let cookies_error_type = analyze_ytdlp_error(&cookies_stderr);

                            log::error!(
                                "❌ [TIER2_FAILED] Audio with-cookies failed: error={:?} exit_code={:?}",
                                cookies_error_type,
                                output.status.code(),
                            );
                            log::error!(
                                "❌ [TIER2_STDERR] {}",
                                &cookies_stderr[..std::cmp::min(1000, cookies_stderr.len())]
                            );

                            // Log cookie file state after Tier 2 failure
                            crate::download::cookies::log_cookie_file_diagnostics("AUDIO_TIER2_AFTER_FAIL");

                            // Check if this is a cookie-specific error that needs refresh
                            if matches!(cookies_error_type, YtDlpErrorType::InvalidCookies) {
                                log::warn!("🍪 [COOKIE_INVALID] Cookies classified as invalid, attempting refresh...");

                                let url_for_report = url_str.clone();
                                let should_retry = runtime_handle.block_on(async {
                                    report_and_wait_for_refresh("InvalidCookies", &url_for_report).await
                                });

                                if should_retry {
                                    log::info!("🔄 Cookie refresh successful, will retry download");
                                    last_error = Some(AppError::Download(error_msg.clone()));
                                    std::thread::sleep(std::time::Duration::from_secs(3));
                                    continue;
                                }
                            } else if matches!(cookies_error_type, YtDlpErrorType::BotDetection) {
                                log::error!(
                                    "🤖 [BOT_DETECTED] Tier 2 got bot detection even WITH cookies. Possible IP ban or cookie rotation."
                                );
                                crate::download::cookies::log_cookie_file_diagnostics("BOT_DETECTED_WITH_COOKIES");
                            }
                        }
                    }
                }

                log::error!(
                    "💀 [BOTH_TIERS_FAILED] Both no-cookies (Tier 1) and with-cookies (Tier 2) modes failed for audio"
                );
            }

            // If PostprocessingError (ffmpeg failed):
            // Retry with --fixup never to skip problematic postprocessing
            if error_type == YtDlpErrorType::PostprocessingError {
                log::warn!("🔧 Postprocessing error detected, retrying with --fixup never...");

                // Comprehensive cleanup of all partial/corrupted files
                let _ = std::fs::remove_file(&download_path_clone);
                cleanup_partial_download(&download_path_clone);

                let mut fixup_args: Vec<&str> = vec![
                    "-o",
                    &download_path_clone,
                    "--newline",
                    "--force-overwrites",
                    "--fixup",
                    "never", // Skip postprocessors
                    "--extract-audio",
                    "--audio-format",
                    "mp3",
                    "--audio-quality",
                    "0",
                    "--add-metadata",
                    "--no-playlist",
                    "--concurrent-fragments",
                    "1",
                    "--fragment-retries",
                    "10",
                    "--socket-timeout",
                    "30",
                ];

                // Add proxy and cookies for this attempt
                add_cookies_args_with_proxy(&mut fixup_args, proxy_option.as_ref());

                fixup_args.push("--extractor-args");
                fixup_args.push("youtube:player_client=web,web_safari");
                fixup_args.push("--js-runtimes");
                fixup_args.push("deno");
                fixup_args.push("--no-check-certificate");
                fixup_args.push(&url_str);

                log::info!("🔧 [FIXUP_NEVER] Attempting audio download without postprocessing...");

                let fixup_child = Command::new(&ytdl_bin)
                    .args(&fixup_args)
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn();

                if let Ok(child) = fixup_child {
                    if let Ok(output) = child.wait_with_output() {
                        if output.status.success() {
                            log::info!("✅ [FIXUP_NEVER] Audio download succeeded without postprocessing!");
                            return Ok(probe_duration_seconds(&download_path_clone));
                        } else {
                            let fixup_stderr = String::from_utf8_lossy(&output.stderr);
                            log::warn!(
                                "❌ [FIXUP_NEVER] Failed: {}",
                                &fixup_stderr[..std::cmp::min(500, fixup_stderr.len())]
                            );
                        }
                    }
                }
            }

            // Not a proxy error or last attempt - report and return
            let error_category = match error_type {
                YtDlpErrorType::InvalidCookies => "invalid_cookies",
                YtDlpErrorType::BotDetection => "bot_detection",
                YtDlpErrorType::VideoUnavailable => "video_unavailable",
                YtDlpErrorType::NetworkError => "network",
                YtDlpErrorType::FragmentError => "fragment_error",
                YtDlpErrorType::PostprocessingError => "postprocessing_error",
                YtDlpErrorType::DiskSpaceError => "disk_space_error",
                YtDlpErrorType::Unknown => "ytdlp_unknown",
            };
            let operation = format!("audio_download:{}", error_category);
            metrics::record_error("download", &operation);

            if should_notify_admin(&error_type) {
                log::warn!("This error requires administrator attention!");
                let admin_message = format!(
                    "YTDLP ERROR (audio download)\nuser_chat_id: {}\nurl: {}\nerror_type: {:?}\nproxy: {}\nattempt: {}/{}\n\ncommand:\n{}\n\nstdout (tail):\n{}\n\nstderr (tail):\n{}",
                    user_chat_id.0,
                    url_str,
                    error_type,
                    proxy_name,
                    attempt + 1,
                    total_proxies,
                    command_str,
                    truncate_tail_utf8(&stdout_text, 6000),
                    truncate_tail_utf8(&stderr_text, 6000),
                );
                let bot_for_admin = admin_bot.clone();
                runtime_handle.spawn(async move {
                    notify_admin_text(&bot_for_admin, &admin_message).await;
                });
            }

            return Err(AppError::Download(error_msg));
        }

        // All proxies exhausted
        log::error!("❌ All {} proxies failed for audio download", total_proxies);
        Err(last_error.unwrap_or_else(|| AppError::Download("All proxies failed".to_string())))
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

        // Global timeout for entire download operation (10 minutes)
        let result: Result<(), AppError> = match timeout(config::download::global_timeout(), async {
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

            // Add unique timestamp to prevent race conditions with concurrent downloads
            use std::time::{SystemTime, UNIX_EPOCH};
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0);

            let base_file_name = generate_file_name(&title, &artist);
            // Add timestamp before extension to ensure uniqueness
            let file_name = if base_file_name.ends_with(".mp3") {
                format!("{}_{}.mp3", base_file_name.trim_end_matches(".mp3"), timestamp)
            } else {
                format!("{}_{}", base_file_name, timestamp)
            };
            let safe_filename = escape_filename(&file_name);
            let full_path = format!("{}/{}", &*config::DOWNLOAD_FOLDER, safe_filename);
            let download_path = shellexpand::tilde(&full_path).into_owned();

            // Step 1.5: Check disk space before downloading
            if let Err(e) = disk::check_disk_space_for_download() {
                log::error!("Disk space check failed: {}", e);
                send_error_with_sticker_and_message(&bot_clone, chat_id, Some("❌ Сервер перегружен. Попробуй позже."))
                    .await;
                let _ = progress_msg
                    .update(
                        &bot_clone,
                        DownloadStatus::Error {
                            title: display_title.as_ref().to_string(),
                            error: "Недостаточно места на сервере".to_string(),
                            file_format: Some("mp3".to_string()),
                        },
                    )
                    .await;
                return Err(AppError::Download("Insufficient disk space".to_string()));
            }

            // Step 1.6: Check if URL is a livestream (not supported)
            if is_livestream(&url).await {
                log::warn!("🔴 Rejected livestream URL: {}", url);
                send_error_with_sticker_and_message(
                    &bot_clone,
                    chat_id,
                    Some("❌ Прямые трансляции не поддерживаются"),
                )
                .await;
                let _ = progress_msg
                    .update(
                        &bot_clone,
                        DownloadStatus::Error {
                            title: display_title.as_ref().to_string(),
                            error: "Прямые трансляции не поддерживаются".to_string(),
                            file_format: Some("mp3".to_string()),
                        },
                    )
                    .await;
                return Err(AppError::Download("Livestreams are not supported".to_string()));
            }

            // Step 1.7: Pre-check file size before downloading
            let max_audio_size = config::validation::max_audio_size_bytes();
            if let Some(estimated_size) = get_estimated_filesize(&url).await {
                if estimated_size > max_audio_size {
                    let size_mb = estimated_size as f64 / (1024.0 * 1024.0);
                    let max_mb = max_audio_size as f64 / (1024.0 * 1024.0);
                    log::warn!("🚫 File too large: estimated {:.2} MB > max {:.2} MB", size_mb, max_mb);
                    send_error_with_sticker_and_message(
                        &bot_clone,
                        chat_id,
                        Some(&format!(
                            "❌ Файл слишком большой: ~{:.0} МБ (макс. {:.0} МБ)",
                            size_mb, max_mb
                        )),
                    )
                    .await;
                    let _ = progress_msg
                        .update(
                            &bot_clone,
                            DownloadStatus::Error {
                                title: display_title.as_ref().to_string(),
                                error: format!("Файл слишком большой: ~{:.0} МБ", size_mb),
                                file_format: Some("mp3".to_string()),
                            },
                        )
                        .await;
                    return Err(AppError::Validation(format!("File too large: ~{:.2} MB", size_mb)));
                }
            }

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
        })
        .await
        {
            Ok(inner_result) => inner_result,
            Err(_elapsed) => {
                log::error!(
                    "🚨 Audio download timed out after {} seconds",
                    config::download::GLOBAL_TIMEOUT_SECS
                );
                Err(AppError::Download("Таймаут загрузки (превышено 10 минут)".to_string()))
            }
        };

        match result {
            Ok(_) => {
                log::info!("Audio download completed successfully for chat {}", chat_id);
                timer.observe_duration();
                metrics::record_download_success("mp3", quality);
            }
            Err(e) => {
                e.track_with_operation("audio_download");
                log::error!(
                    "An error occurred during audio download for chat {} ({}): {:?}",
                    chat_id,
                    url,
                    e
                );
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
                let is_bot_blocked = {
                    let lower = error_str.to_lowercase();
                    lower.contains("sign in to confirm you're not a bot")
                        || lower.contains("sign in to confirm you’re not a bot")
                        || lower.contains("confirm you're not a bot")
                        || lower.contains("confirm you’re not a bot")
                        || lower.contains("bot detection")
                };

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
                } else if is_bot_blocked {
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
