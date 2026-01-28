//! Video download and processing module
//!
//! This module handles downloading video files from URLs using yt-dlp,
//! tracking progress, and sending them to Telegram users.

use crate::core::config;
use crate::core::error::AppError;
use crate::core::error_logger::{self, ErrorType, UserContext};
use crate::core::metrics;
use crate::core::rate_limiter::RateLimiter;
use crate::core::truncate_tail_utf8;
use crate::core::utils::escape_filename;
use crate::download::downloader::{
    burn_subtitles_into_video, generate_file_name_with_ext, parse_progress, split_video_into_parts, ProgressInfo,
};
use crate::download::metadata::{
    add_cookies_args, build_telegram_safe_format, find_actual_downloaded_file, get_metadata_from_ytdlp,
    has_both_video_and_audio, probe_video_metadata,
};
use crate::download::progress::{DownloadStatus, ProgressMessage};
use crate::download::send::{send_error_with_sticker, send_error_with_sticker_and_message, send_video_with_retry};
use crate::download::ytdlp_errors::{
    analyze_ytdlp_error, get_error_message, sanitize_user_error_message, should_notify_admin, YtDlpErrorType,
};
use crate::storage::db::{self as db, save_download_history, save_video_timestamps, DbPool};
use crate::telegram::cache::PREVIEW_CACHE;
use crate::telegram::notifications::notify_admin_text;
use crate::telegram::Bot;
use chrono::{DateTime, Utc};
use std::fs;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;
use teloxide::prelude::*;
use tokio::process::Command as TokioCommand;
use tokio::time::timeout;
use url::Url;

/// Downloads video with real-time progress tracking via channel
///
/// Returns a receiver for progress updates and a join handle for the download task.
/// The download runs in a blocking task to read stdout line by line.
pub async fn download_video_file_with_progress(
    admin_bot: Bot,
    user_chat_id: ChatId,
    url: &Url,
    download_path: &str,
    format_arg: &str,
) -> Result<
    (
        tokio::sync::mpsc::UnboundedReceiver<ProgressInfo>,
        tokio::task::JoinHandle<Result<(), AppError>>,
    ),
    AppError,
> {
    let ytdl_bin = config::YTDL_BIN.clone();
    let url_str = url.to_string();
    let download_path_clone = download_path.to_string();
    let format_arg_clone = format_arg.to_string();
    let runtime_handle = tokio::runtime::Handle::current();

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

    let handle = tokio::task::spawn_blocking(move || {
        let mut args: Vec<&str> = vec![
            "-o",
            &download_path_clone,
            "--newline",
            "--format",
            &format_arg_clone,
            "--merge-output-format",
            "mp4",
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
            // Exponential backoff for 403/rate-limit errors
            "--retry-sleep",
            "http:exp=1:30", // 1s -> 2s -> 4s -> ... up to 30s
            "--retry-sleep",
            "fragment:exp=1:30", // same for fragment errors
            "--retries",
            "5", // retry main request up to 5 times
            "--postprocessor-args",
            "ffmpeg:-movflags +faststart",
        ];
        add_cookies_args(&mut args);

        // Use web client with PO Token support (bgutil plugin provides tokens automatically)
        // web client provides better quality formats than android_sdkless
        args.push("--extractor-args");
        args.push("youtube:player_client=web,web_safari,android_sdkless");

        // Use Node.js for YouTube n-challenge solving and BotGuard token generation
        args.push("--js-runtimes");
        args.push("node");

        args.extend_from_slice(&["--no-check-certificate", &url_str]);

        let command_str = format!("{} {}", ytdl_bin, args.join(" "));
        log::info!("[DEBUG] yt-dlp command for video download: {}", command_str);

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
                let operation = format!("video_download:{}", error_category);
                metrics::record_error("download", &operation);

                log::error!("yt-dlp download failed, error type: {:?}", error_type);
                log::error!("yt-dlp stderr: {}", stderr_text);

                if should_notify_admin(&error_type) {
                    log::warn!("This error requires administrator attention!");
                    let admin_message = format!(
                        "YTDLP ERROR (video download)\nuser_chat_id: {}\nurl: {}\nerror_type: {:?}\n\ncommand:\n{}\n\nstdout (tail):\n{}\n\nstderr (tail):\n{}",
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
                metrics::record_error("download", "video_download");
                return Err(AppError::Download(format!("downloader exited with status: {}", status)));
            }
        }

        Ok(())
    });

    Ok((rx, handle))
}

/// Download video file and send it to user
///
/// Downloads video from URL using yt-dlp, shows progress updates, validates file size,
/// and sends the file to the user via Telegram.
///
/// # Arguments
///
/// * `bot` - Telegram bot instance
/// * `chat_id` - User's chat ID
/// * `url` - URL to download from
/// * `rate_limiter` - Rate limiter instance (unused but kept for API consistency)
/// * `_created_timestamp` - Timestamp when task was created (unused)
///
/// # Returns
///
/// Returns `Ok(())` on success or a `ResponseResult` error.
///
/// # Behavior
///
/// Similar to [`download_and_send_audio`], but for video files.
pub async fn download_and_send_video(
    bot: Bot,
    chat_id: ChatId,
    url: Url,
    rate_limiter: Arc<RateLimiter>,
    _created_timestamp: DateTime<Utc>,
    db_pool: Option<Arc<DbPool>>,
    video_quality: Option<String>,
    message_id: Option<i32>,
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
        metrics::record_format_request("mp4", &user_plan);

        // Start metrics timer for video download
        let quality = video_quality.as_deref().unwrap_or("default");
        let timer = metrics::DOWNLOAD_DURATION_SECONDS
            .with_label_values(&["mp4", quality])
            .start_timer();

        let result: Result<(), AppError> = async {
            // Step 1: Get metadata and show starting status
            let (title, artist) = match get_metadata_from_ytdlp(Some(&bot_clone), Some(chat_id), &url).await {
                Ok(meta) => {
                    log::info!(
                        "Successfully got metadata for video - title: '{}', artist: '{}'",
                        meta.0,
                        meta.1
                    );
                    meta
                }
                Err(e) => {
                    log::error!("Failed to get metadata for video from URL {}: {:?}", url, e);
                    // Проверяем, является ли это ошибкой таймаута
                    if e.to_string().contains("timed out") {
                        log::warn!("yt-dlp timed out, sending error message to user");
                        send_error_with_sticker(&bot_clone, chat_id).await;
                    }
                    return Err(e);
                }
            };

            // Получаем thumbnail URL для preview изображения
            log::info!("[THUMBNAIL] Starting to get thumbnail URL for video");
            let thumbnail_url = {
                let ytdl_bin = &*config::YTDL_BIN;
                let mut thumbnail_args: Vec<&str> = vec![
                    "--get-thumbnail",
                    "--no-playlist",
                    "--socket-timeout",
                    "30",
                    "--retries",
                    "2",
                ];
                add_cookies_args(&mut thumbnail_args);
                thumbnail_args.push(url.as_str());

                let command_str = format!("{} {}", ytdl_bin, thumbnail_args.join(" "));
                log::info!("[THUMBNAIL] yt-dlp command for thumbnail URL: {}", command_str);

                let thumbnail_output = timeout(
                    config::download::ytdlp_timeout(),
                    TokioCommand::new(ytdl_bin).args(&thumbnail_args).output(),
                )
                .await
                .ok(); // Не критично, игнорируем ошибки

                let result = thumbnail_output
                    .and_then(|result| {
                        log::info!("[THUMBNAIL] yt-dlp thumbnail command completed");
                        result.ok()
                    })
                    .and_then(|out| {
                        log::info!(
                            "[THUMBNAIL] yt-dlp exit status: {:?}, stdout length: {}, stderr length: {}",
                            out.status,
                            out.stdout.len(),
                            out.stderr.len()
                        );

                        if !out.stderr.is_empty() {
                            let stderr_str = String::from_utf8_lossy(&out.stderr);
                            log::debug!("[THUMBNAIL] yt-dlp stderr: {}", stderr_str);
                        }

                        if out.status.success() {
                            let url_str = String::from_utf8_lossy(&out.stdout).trim().to_string();
                            log::info!("[THUMBNAIL] yt-dlp stdout (thumbnail URL): '{}'", url_str);
                            if url_str.is_empty() {
                                log::warn!("[THUMBNAIL] Thumbnail URL is empty");
                                None
                            } else {
                                Some(url_str)
                            }
                        } else {
                            log::warn!(
                                "[THUMBNAIL] yt-dlp failed to get thumbnail URL, exit status: {:?}",
                                out.status
                            );
                            None
                        }
                    });

                if result.is_none() {
                    log::warn!("[THUMBNAIL] Failed to get thumbnail URL from yt-dlp (timeout or error)");
                }

                result
            };

            if let Some(ref thumb_url) = thumbnail_url {
                log::info!("[THUMBNAIL] Successfully got thumbnail URL for video: {}", thumb_url);
            } else {
                log::warn!("[THUMBNAIL] Thumbnail URL not available for video - will send without thumbnail preview");
            }

            log::info!(
                "Video metadata received - title length: {}, artist length: {}",
                title.len(),
                artist.len()
            );

            let display_title: Arc<str> = if artist.trim().is_empty() {
                Arc::from(title.as_str())
            } else {
                Arc::from(format!("{} - {}", artist, title))
            };

            // Создаём отформатированный caption для Telegram с MarkdownV2
            let caption: Arc<str> = Arc::from(crate::core::utils::format_media_caption(&title, &artist));

            log::info!("Display title for video: '{}'", display_title);
            log::info!("Formatted caption for video: '{}'", caption);

            // Show starting status
            let _ = progress_msg
                .update(
                    &bot_clone,
                    DownloadStatus::Starting {
                        title: display_title.as_ref().to_string(),
                        file_format: Some("mp4".to_string()),
                    },
                )
                .await;

            // Добавляем уникальный идентификатор к имени файла для избежания конфликтов
            use std::time::{SystemTime, UNIX_EPOCH};
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0);

            let base_file_name = generate_file_name_with_ext(&title, &artist, "mp4");
            // Добавляем timestamp к имени файла (перед расширением)
            let file_name = if base_file_name.ends_with(".mp4") {
                format!("{}_{}.mp4", base_file_name.trim_end_matches(".mp4"), timestamp)
            } else {
                format!("{}_{}", base_file_name, timestamp)
            };

            log::info!(
                "Generated filename for video: '{}' (base: '{}')",
                file_name,
                base_file_name
            );
            let safe_filename = escape_filename(&file_name);
            let full_path = format!("{}/{}", &*config::DOWNLOAD_FOLDER, safe_filename);
            let download_path = shellexpand::tilde(&full_path).into_owned();

            // Step 2: Determine video quality format with fallback chain
            // Используем bestvideo[height<=X]+bestaudio для автоматического объединения video-only и audio-only форматов
            // YouTube SABR streaming возвращает только отдельные форматы, yt-dlp объединит их автоматически с помощью ffmpeg
            // Добавляем fallback на best для случаев когда доступны готовые комбинированные форматы
            // Синтаксис "format1/format2/format3" позволяет yt-dlp выбрать первый доступный формат
            let format_arg = match video_quality.as_deref() {
                Some("1080p") => build_telegram_safe_format(Some(1080)),
                Some("720p") => build_telegram_safe_format(Some(720)),
                Some("480p") => build_telegram_safe_format(Some(480)),
                Some("360p") => build_telegram_safe_format(Some(360)),
                _ => build_telegram_safe_format(None), // приоритет avc1/mp4a без ограничения по высоте
            };

            log::info!("Using Telegram-safe video format chain: {}", format_arg);

            // Step 2.5: Check estimated file size before downloading
            // Пытаемся получить размер файла для выбранного формата
            // Проблема: YouTube часто возвращает "NA" для размера, и fallback цепочка может выбрать другой формат
            // Поэтому проверяем размер для первого формата в цепочке (без fallback)
            // Если размер недоступен или слишком большой - предупреждаем пользователя
            let ytdl_bin = &*config::YTDL_BIN;

            // Получаем первый формат из цепочки для проверки (без fallback)
            let first_format = match video_quality.as_deref() {
                Some("1080p") => "bestvideo[height<=1080]+bestaudio",
                Some("720p") => "bestvideo[height<=720]+bestaudio",
                Some("480p") => "bestvideo[height<=480]+bestaudio",
                Some("360p") => "bestvideo[height<=360]+bestaudio",
                _ => "bestvideo+bestaudio",
            };

            let mut size_check_args: Vec<String> = vec![
                "--print".to_string(),
                "%(filesize)s".to_string(),
                "--format".to_string(),
                first_format.to_string(),
                "--no-playlist".to_string(),
                "--skip-download".to_string(),
            ];

            let mut temp_args: Vec<&str> = vec![];
            add_cookies_args(&mut temp_args);
            for arg in temp_args {
                size_check_args.push(arg.to_string());
            }
            size_check_args.push(url.as_str().to_string());

            let size_check_cmd = format!("{} {}", ytdl_bin, size_check_args.join(" "));
            log::info!(
                "[DEBUG] Checking file size before download (format: {}): {}",
                first_format,
                size_check_cmd
            );

            let size_check_output = timeout(
                config::download::ytdlp_timeout(),
                TokioCommand::new(ytdl_bin).args(&size_check_args).output(),
            )
            .await;

            let mut size_available = false;
            if let Ok(Ok(output)) = size_check_output {
                if output.status.success() {
                    let size_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    if !size_str.is_empty() && size_str != "NA" {
                        if let Ok(file_size) = size_str.parse::<u64>() {
                            size_available = true;
                            let size_mb = file_size as f64 / (1024.0 * 1024.0);
                            log::info!("Estimated video file size for {}: {:.2} MB", first_format, size_mb);
                        }
                    }
                }
            }

            // Если размер недоступен (NA) - проверяем через --list-formats для получения точных размеров
            // YouTube часто не предоставляет размер через --print для объединенных форматов
            // Но через --list-formats мы можем увидеть размеры отдельных форматов
            if !size_available {
                log::info!("File size NA via --print, trying to get sizes via --list-formats");

                // Получаем список форматов с размерами
                let mut list_formats_args: Vec<String> = vec!["--list-formats".to_string(), "--no-playlist".to_string()];

                let mut temp_args: Vec<&str> = vec![];
                add_cookies_args(&mut temp_args);
                for arg in temp_args {
                    list_formats_args.push(arg.to_string());
                }
                list_formats_args.push(url.as_str().to_string());

                let list_formats_output = timeout(
                    Duration::from_secs(30), // Более короткий таймаут для списка форматов
                    TokioCommand::new(ytdl_bin).args(&list_formats_args).output(),
                )
                .await;

                // Парсим вывод и ищем форматы с размерами для запрошенного качества
                if let Ok(Ok(output)) = list_formats_output {
                    if output.status.success() {
                        let formats_output = String::from_utf8_lossy(&output.stdout);

                        // Ищем размеры для форматов в зависимости от запрошенного качества
                        let target_height = match video_quality.as_deref() {
                            Some("1080p") => 1080,
                            Some("720p") => 720,
                            Some("480p") => 480,
                            Some("360p") => 360,
                            _ => 0,
                        };

                        if target_height > 0 {
                            // Парсим строки вида: "137     mp4   1920x1080   24    |  154.58MiB  1786k https"
                            for line in formats_output.lines() {
                                // Ищем строки с нужным разрешением
                                if line.contains(&format!("{}x{}", target_height, target_height))
                                    || (target_height == 1080 && line.contains("1920x1080"))
                                    || (target_height == 720 && line.contains("1280x720"))
                                    || (target_height == 480 && line.contains("854x480"))
                                    || (target_height == 360 && line.contains("640x360"))
                                {
                                    // Извлекаем размер (формат: ~XX.XXMiB или XX.XXMiB)
                                    if let Some(size_mb_pos) = line.find("MiB") {
                                        let before_size = &line[..size_mb_pos];
                                        if let Some(start) = before_size
                                            .rfind(|c: char| c.is_ascii_digit() || c == '.' || c == '~')
                                        {
                                            let size_str = &line[start..size_mb_pos].trim().trim_start_matches('~');
                                            if let Ok(size_mb) = size_str.parse::<f64>() {
                                                log::info!(
                                                    "Found format size via --list-formats: {:.2} MB for {}p",
                                                    size_mb,
                                                    target_height
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // Если размер все еще недоступен - проверяем нужно ли блокировать
                // НО: не блокируем если используется локальный Bot API сервер (лимит 2 GB)
                let is_local_bot_api = std::env::var("BOT_API_URL")
                    .map(|url| !url.contains("api.telegram.org"))
                    .unwrap_or(false);

                if !is_local_bot_api {
                    // Для стандартного API предупреждаем о возможном превышении лимита
                    match video_quality.as_deref() {
                        Some("1080p") | Some("720p") => {
                            let quality_str = video_quality.as_deref().unwrap_or("unknown");
                            log::warn!(
                                "File size not available (NA) for {} quality. Will proceed with download and check size after.",
                                quality_str
                            );
                            log::info!(
                                "Warning: Downloading {} video without knowing size beforehand. Will check after download.",
                                quality_str
                            );
                        }
                        _ => {
                            log::info!("File size not available before download (NA), will check after download");
                        }
                    }
                } else {
                    // Для локального Bot API сервера - разрешаем все форматы, даже если размер NA
                    let quality_str = video_quality.as_deref().unwrap_or("unknown");
                    log::info!(
                        "File size not available (NA) for {} quality, but local Bot API server is used (2 GB limit). Proceeding with download.",
                        quality_str
                    );
                }
            }

            // Step 3: Download with real-time progress updates
            let (mut progress_rx, mut download_handle) =
                download_video_file_with_progress(bot_clone.clone(), chat_id, &url, &download_path, &format_arg).await?;

            // Показываем начальный прогресс 0%
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
                        file_format: Some("mp4".to_string()),
                    },
                )
                .await;

            // Читаем обновления прогресса из channel
            let bot_for_progress = bot_clone.clone();
            let title_for_progress = Arc::clone(&display_title);
            let mut last_progress = 0u8;

            loop {
                tokio::select! {
                    // Получаем обновления прогресса
                    Some(progress_info) = progress_rx.recv() => {
                        log::debug!("Received progress update: {}% (speed: {:?} MB/s, eta: {:?}s, total_size: {:?})",
                            progress_info.percent, progress_info.speed_mbs, progress_info.eta_seconds, progress_info.total_size);

                        // Сначала обновляем UI, чтобы пользователь видел прогресс
                        // Обновляем при значимых изменениях (разница >= 5%)
                        // Не даём прогрессу откатываться назад и игнорируем ранние ложные 100%
                        let mut safe_progress = progress_info
                            .percent
                            .clamp(last_progress, 100);
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
                                file_format: Some("mp4".to_string()),
                            }).await;
                        }

                        // Размер файла больше не проверяется - пользователь сам решает что качать
                    }
                    // Ждем завершения загрузки
                    result = &mut download_handle => {
                        // Дорисовываем прогресс до 100% после успешной загрузки
                        if last_progress < 100 {
                            let _ = progress_msg.update(&bot_for_progress, DownloadStatus::Downloading {
                                title: title_for_progress.as_ref().to_string(),
                                progress: 100,
                                speed_mbs: None,
                                eta_seconds: None,
                                current_size: None,
                                total_size: None,
                                file_format: Some("mp4".to_string()),
                            }).await;
                            let _ = last_progress; // Suppress unused warning
                        }
                        result.map_err(|e| AppError::Download(format!("Task join error: {}", e)))??;
                        break;
                    }
                }
            }

            log::debug!("Download path: {:?}", download_path);

            // Calculate elapsed time
            let elapsed_secs = start_time.elapsed().as_secs();
            log::info!("Video downloaded in {} seconds", elapsed_secs);

            // Step 2.5: Find actual downloaded file (yt-dlp may add suffixes like (1).mp4)
            let actual_file_path = match find_actual_downloaded_file(&download_path) {
                Ok(path) => {
                    log::info!("Using actual downloaded file: {}", path);
                    path
                }
                Err(e) => {
                    log::error!("Failed to find actual downloaded file: {:?}", e);
                    return Err(e);
                }
            };

            // Step 3: Get file size info (no validation, just logging)
            // NOTE: This might be incomplete if ffmpeg is still merging video+audio streams
            let file_size = fs::metadata(&actual_file_path)
                .map_err(|e| AppError::Download(format!("Failed to get file metadata: {}", e)))?
                .len();

            log::info!(
                "Downloaded video file size (might be video-only stream, before merging): {:.2} MB",
                file_size as f64 / (1024.0 * 1024.0)
            );

            // Step 3.5: Проверяем, что файл содержит и видео, и аудио дорожки
            match has_both_video_and_audio(&actual_file_path) {
                Ok(true) => {
                    log::info!("Video file verified: contains both video and audio streams");
                }
                Ok(false) => {
                    log::error!("Video file is missing video or audio stream!");
                    log::error!("This can cause black screen or playback issues in Telegram");

                    // Попробуем получить детальную информацию о файле
                    let _ = Command::new("ffprobe")
                        .args(["-v", "error", "-show_streams", &actual_file_path])
                        .output()
                        .map(|output| {
                            log::error!("File streams info: {}", String::from_utf8_lossy(&output.stdout));
                        });

                    send_error_with_sticker(&bot_clone, chat_id).await;
                    let _ = progress_msg
                        .update(
                            &bot_clone,
                            DownloadStatus::Error {
                                title: display_title.as_ref().to_string(),
                                error: "Видео файл повреждён или не содержит все необходимые дорожки".to_string(),
                                file_format: Some("mp4".to_string()),
                            },
                        )
                        .await;
                    return Err(AppError::Download(
                        "Video file missing video or audio stream".to_string(),
                    ));
                }
                Err(e) => {
                    log::warn!("Failed to verify video streams: {}. Continuing anyway...", e);
                }
            }

            // Step 3.7: Check if we need to burn subtitles into video
            let actual_file_path = if let Some(ref pool) = db_pool_clone {
                match db::get_connection(pool) {
                    Ok(conn) => {
                        let download_subs = db::get_user_download_subtitles(&conn, chat_id.0).unwrap_or(false);
                        let burn_subs = db::get_user_burn_subtitles(&conn, chat_id.0).unwrap_or(false);

                        log::info!(
                            "User {} subtitle settings: download_subs={}, burn_subs={}",
                            chat_id.0,
                            download_subs,
                            burn_subs
                        );

                        if download_subs && burn_subs {
                            log::info!(
                                "User requested burned subtitles - downloading subtitles and burning into video"
                            );

                            // Download subtitles first
                            let subtitle_path = format!(
                                "{}/{}_subs.srt",
                                &*config::DOWNLOAD_FOLDER,
                                safe_filename.trim_end_matches(".mp4")
                            );

                            log::info!("Downloading subtitles to: {}", subtitle_path);

                            // Download subtitles using yt-dlp
                            let ytdl_bin = &*config::YTDL_BIN;
                            let mut subtitle_args: Vec<&str> = vec![
                                "--write-subs",
                                "--write-auto-subs",
                                "--sub-lang",
                                "en,ru",
                                "--sub-format",
                                "srt",
                                "--convert-subs",
                                "srt",
                                "--skip-download",
                                "--output",
                                &subtitle_path,
                                "--no-playlist",
                            ];
                            add_cookies_args(&mut subtitle_args);
                            subtitle_args.push(url.as_str());

                            log::info!(
                                "Running yt-dlp for subtitles: {} {}",
                                ytdl_bin,
                                subtitle_args.join(" ")
                            );

                            let subtitle_output = TokioCommand::new(ytdl_bin).args(&subtitle_args).output().await;

                            match subtitle_output {
                                Ok(output) if output.status.success() => {
                                    // Find the actual subtitle file (yt-dlp may add language suffix)
                                    let subtitle_file = std::fs::read_dir(&*config::DOWNLOAD_FOLDER)
                                        .ok()
                                        .and_then(|entries| {
                                            entries
                                                .filter_map(Result::ok)
                                                .find(|entry| {
                                                    let name = entry.file_name();
                                                    let name_str = name.to_string_lossy();
                                                    name_str.contains(safe_filename.trim_end_matches(".mp4"))
                                                        && name_str.ends_with(".srt")
                                                })
                                                .map(|entry| entry.path().display().to_string())
                                        });

                                    if let Some(sub_file) = subtitle_file {
                                        log::info!("Subtitles downloaded successfully: {}", sub_file);

                                        // Burn subtitles into video
                                        let output_with_subs =
                                            format!("{}_with_subs.mp4", actual_file_path.trim_end_matches(".mp4"));

                                        log::info!(
                                            "Burning subtitles into video: {} -> {}",
                                            actual_file_path,
                                            output_with_subs
                                        );

                                        match burn_subtitles_into_video(&actual_file_path, &sub_file, &output_with_subs)
                                            .await
                                        {
                                            Ok(_) => {
                                                log::info!("Successfully burned subtitles into video");

                                                // Delete original video and subtitle file
                                                let _ = std::fs::remove_file(&actual_file_path);
                                                let _ = std::fs::remove_file(&sub_file);

                                                output_with_subs
                                            }
                                            Err(e) => {
                                                log::error!(
                                                    "Failed to burn subtitles: {}. Using original video.",
                                                    e
                                                );
                                                // Cleanup subtitle file
                                                let _ = std::fs::remove_file(&sub_file);
                                                actual_file_path
                                            }
                                        }
                                    } else {
                                        log::warn!("Subtitles not found after download. Using original video.");
                                        actual_file_path
                                    }
                                }
                                Ok(output) => {
                                    log::warn!(
                                        "yt-dlp failed to download subtitles: {}",
                                        String::from_utf8_lossy(&output.stderr)
                                    );
                                    actual_file_path
                                }
                                Err(e) => {
                                    log::warn!("Failed to execute yt-dlp for subtitles: {}", e);
                                    actual_file_path
                                }
                            }
                        } else {
                            actual_file_path
                        }
                    }
                    Err(_) => actual_file_path,
                }
            } else {
                actual_file_path
            };

            // Step 4: Get user preference for send_as_document
            let send_as_document = if let Some(ref pool) = db_pool_clone {
                match db::get_connection(pool) {
                    Ok(conn) => {
                        let value = db::get_user_send_as_document(&conn, chat_id.0).unwrap_or(0);
                        log::info!(
                            "User {} send_as_document value from DB: {} ({})",
                            chat_id.0,
                            value,
                            if value == 0 {
                                "Media/send_video"
                            } else {
                                "Document/send_document"
                            }
                        );
                        value == 1
                    }
                    Err(_) => false,
                }
            } else {
                false
            };

            // Log final merged file size before sending
            let final_file_size = fs::metadata(&actual_file_path).map(|m| m.len()).unwrap_or(0);
            log::info!(
                "Final merged video file size (before sending): {:.2} MB",
                final_file_size as f64 / (1024.0 * 1024.0)
            );

            // Step 5: Send video (with splitting if necessary and Local Bot API is used)
            let is_local_bot_api = std::env::var("BOT_API_URL")
                .map(|url| !url.contains("api.telegram.org"))
                .unwrap_or(false);

            // Use splitting only if it's Local Bot API and file is > 1.9GB
            // For standard API, yt-dlp already ensures the file is small enough or it fails earlier
            let target_part_size = 1900 * 1024 * 1024; // 1.9 GB

            let video_parts = if is_local_bot_api && final_file_size > target_part_size {
                log::info!("Video size exceeds 1.9GB and Local Bot API is used - splitting into parts");
                split_video_into_parts(&actual_file_path, target_part_size).await?
            } else {
                vec![actual_file_path.clone()]
            };

            let mut first_part_db_id = None;
            let total_parts = video_parts.len();

            for (idx, part_path) in video_parts.iter().enumerate() {
                let part_index = (idx + 1) as i32;
                let current_caption = if total_parts > 1 {
                    format!("{} (Part {}/{})", caption, part_index, total_parts)
                } else {
                    caption.as_ref().to_string()
                };

                log::info!(
                    "Sending video part {}/{} ({}): {}",
                    part_index,
                    total_parts,
                    part_path,
                    current_caption
                );

                // Send video with retry logic and animation
                let (sent_message, file_size) = send_video_with_retry(
                    &bot_clone,
                    chat_id,
                    part_path,
                    &mut progress_msg,
                    &current_caption,
                    thumbnail_url.as_deref(),
                    send_as_document,
                )
                .await?;

                // Save to download history after successful send
                if let Some(ref pool) = db_pool_clone {
                    if let Ok(conn) = crate::storage::db::get_connection(pool) {
                        let file_id = sent_message
                            .video()
                            .map(|v| v.file.id.0.clone())
                            .or_else(|| sent_message.document().map(|d| d.file.id.0.clone()));

                        let author_opt = if !artist.trim().is_empty() {
                            Some(artist.as_str())
                        } else {
                            None
                        };

                        let duration = probe_video_metadata(part_path).map(|(d, _, _)| d as i64);

                        let db_id = save_download_history(
                            &conn,
                            chat_id.0,
                            url.as_str(),
                            title.as_str(), // Just the title without artist
                            "mp4",
                            file_id.as_deref(),
                            author_opt,
                            Some(file_size as i64),
                            duration,
                            Some(quality),
                            None, // audio_bitrate (N/A for mp4)
                            first_part_db_id,
                            if total_parts > 1 { Some(part_index) } else { None },
                        );

                        match db_id {
                            Ok(id) => {
                                // Save message_id for MTProto file_reference refresh
                                let sent_msg_id = sent_message.id.0;
                                if let Err(e) = db::update_download_message_id(&conn, id, sent_msg_id, chat_id.0) {
                                    log::warn!("Failed to save message_id for download {}: {}", id, e);
                                }

                                // Save video timestamps (only for single-part videos or first part)
                                if total_parts == 1 || first_part_db_id.is_none() {
                                    if let Some(metadata) = PREVIEW_CACHE.get(url.as_str()).await {
                                        if !metadata.timestamps.is_empty() {
                                            if let Err(e) = save_video_timestamps(&conn, id, &metadata.timestamps) {
                                                log::warn!("Failed to save video timestamps for download {}: {}", id, e);
                                            } else {
                                                log::debug!(
                                                    "Saved {} timestamps for download {}",
                                                    metadata.timestamps.len(),
                                                    id
                                                );
                                            }
                                        }
                                    }
                                }

                                if first_part_db_id.is_none() && total_parts > 1 {
                                    first_part_db_id = Some(id);
                                }
                                if total_parts == 1 {
                                    let bot_for_button = bot_clone.clone();
                                    let message_id = sent_message.id;
                                    tokio::spawn(async move {
                                        use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};
                                        let keyboard = InlineKeyboardMarkup::new(vec![vec![
                                            InlineKeyboardButton::callback(
                                                "✂️ Cut Video",
                                                format!("downloads:clip:{}", id),
                                            ),
                                        ]]);
                                        if let Err(e) = bot_for_button
                                            .edit_message_reply_markup(chat_id, message_id)
                                            .reply_markup(keyboard)
                                            .await
                                        {
                                            log::warn!("Failed to add video cut button: {}", e);
                                        }
                                    });
                                }
                            }
                            Err(e) => log::warn!("Failed to save download history for part {}: {}", part_index, e),
                        }
                    }
                }
            }

            // Сразу после успешной отправки всех частей обновляем сообщение прогресса до Success
            // чтобы убрать застрявшее состояние "Uploading: 99%"
            let _ = progress_msg
                .update(
                    &bot_clone,
                    DownloadStatus::Success {
                        title: display_title.as_ref().to_string(),
                        elapsed_secs,
                        file_format: Some("mp4".to_string()),
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

            // Step 5: Auto-clear success message after delay (оставляем только название)
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

            tokio::time::sleep(config::download::cleanup_delay()).await;

            // Cleanup all parts if splitting was performed
            if total_parts > 1 {
                for part_path in &video_parts {
                    if let Err(e) = fs::remove_file(part_path) {
                        log::warn!("Failed to delete video part {}: {}", part_path, e);
                    }
                }
            }

            // Удаляем фактический файл, который был скачан и (возможно) разделен
            if let Err(e) = fs::remove_file(&actual_file_path) {
                log::warn!("Failed to delete actual file {}: {}", actual_file_path, e);
            }
            // Также пытаемся удалить исходный путь на случай если он отличается
            if actual_file_path != download_path {
                if let Err(e) = fs::remove_file(&download_path) {
                    log::debug!(
                        "Failed to delete expected file {} (this is OK if it doesn't exist): {}",
                        download_path,
                        e
                    );
                }
            }

            Ok(())
        }
        .await;

        // Record metrics based on result
        match &result {
            Ok(_) => {
                log::info!("Video download completed successfully for chat {}", chat_id);
                timer.observe_duration();
                metrics::record_download_success("mp4", quality);
            }
            Err(e) => {
                e.track_with_operation("video_download");
                timer.observe_duration();
                let error_type = if e.to_string().contains("too large") {
                    "file_too_large"
                } else if e.to_string().contains("timed out") {
                    "timeout"
                } else {
                    "other"
                };
                metrics::record_download_failure("mp4", error_type);

                // Log error to database
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
                    Some(r#"{"format":"mp4"}"#),
                );
            }
        }

        if let Err(e) = result {
            log::error!("An error occurred during video download for chat {}: {:?}", chat_id, e);

            // Определяем тип ошибки и формируем полезное сообщение
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
            } else if error_str.contains("Sign in to confirm you're not a bot") || error_str.contains("bot detection") {
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

            // Send error sticker and message
            send_error_with_sticker_and_message(&bot_clone, chat_id, custom_message).await;
            // Show error status
            let _ = progress_msg
                .update(
                    &bot_clone,
                    DownloadStatus::Error {
                        title: "Скачивание".to_string(),
                        error: display_error.to_string(),
                        file_format: Some("mp4".to_string()),
                    },
                )
                .await;
        }
    });
    Ok(())
}
