use crate::core::config;
use crate::core::error::AppError;
use crate::core::error_logger::{self, ErrorType, UserContext};
use crate::core::metrics;
use crate::core::rate_limiter::RateLimiter;
use crate::core::utils::{escape_filename, sanitize_filename};
use crate::download::metadata::{
    add_cookies_args, build_telegram_safe_format, find_actual_downloaded_file, get_metadata_from_ytdlp,
    has_both_video_and_audio, probe_video_metadata,
};
use crate::download::progress::{DownloadStatus, ProgressMessage};
use crate::download::proxy::ProxyListManager;
use crate::download::send::{send_error_with_sticker, send_error_with_sticker_and_message, send_video_with_retry};
use crate::download::video::download_video_file_with_progress;
use crate::download::ytdlp_errors::sanitize_user_error_message;
use crate::storage::db::{self as db, save_download_history, DbPool};
use crate::telegram::Bot;
use chrono::{DateTime, Utc};
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;
use teloxide::prelude::*;
use teloxide::types::InputFile;
use tokio::process::Command as TokioCommand;
use tokio::time::timeout;
use url::Url;

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
    if config::proxy::PROXY_LIST.is_none() && config::proxy::PROXY_FILE.is_none() {
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

    // Парсим процент
    let parts: Vec<&str> = line.split_whitespace().collect();
    for (i, part) in parts.iter().enumerate() {
        if part.ends_with('%') {
            if let Ok(p) = part.trim_end_matches('%').parse::<f32>() {
                // Обрезаем в разумные границы, чтобы не прыгать на 100% при мусорных данных
                let clamped = p.clamp(0.0, 100.0) as u8;
                percent = Some(clamped);
            }
        }

        // Парсим размер: "of 10.00MiB"
        if *part == "of" && i + 1 < parts.len() {
            if let Some(size_bytes) = parse_size(parts[i + 1]) {
                total_size = Some(size_bytes);
            }
        }

        // Парсим скорость: "at 500.00KiB/s" или "at 2.3MiB/s"
        if *part == "at" && i + 1 < parts.len() {
            if let Some(speed) = parse_size(parts[i + 1]) {
                // Конвертируем в MB/s
                speed_mbs = Some(speed as f64 / (1024.0 * 1024.0));
            }
        }

        // Парсим ETA: "ETA 00:10" или "ETA 1:23"
        if *part == "ETA" && i + 1 < parts.len() {
            if let Some(eta) = parse_eta(parts[i + 1]) {
                eta_seconds = Some(eta);
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
    let parts: Vec<&str> = eta_str.split(':').collect();
    if parts.len() == 2 {
        if let (Ok(minutes), Ok(seconds)) = (parts[0].parse::<u64>(), parts[1].parse::<u64>()) {
            return Some(minutes * 60 + seconds);
        }
    }
    None
}

// download_audio_file and download_audio_file_with_progress moved to audio.rs
// download_video_file_with_progress moved to video.rs

// download_and_send_audio moved to audio.rs

// send_file_with_retry, send_audio_with_retry, send_video_with_retry moved to send.rs

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
                    log::info!("Successfully got metadata for video - title: '{}', artist: '{}'", meta.0, meta.1);
                    meta
                },
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
                    "--socket-timeout", "30",
                    "--retries", "2",
                ];
                add_cookies_args(&mut thumbnail_args);
                thumbnail_args.push(url.as_str());

                let command_str = format!("{} {}", ytdl_bin, thumbnail_args.join(" "));
                log::info!("[THUMBNAIL] yt-dlp command for thumbnail URL: {}", command_str);

                let thumbnail_output = timeout(
                    config::download::ytdlp_timeout(),
                    TokioCommand::new(ytdl_bin)
                        .args(&thumbnail_args)
                        .output()
                )
                .await
                .ok(); // Не критично, игнорируем ошибки

                let result = thumbnail_output
                    .and_then(|result| {
                        log::info!("[THUMBNAIL] yt-dlp thumbnail command completed");
                        result.ok()
                    })
                    .and_then(|out| {
                        log::info!("[THUMBNAIL] yt-dlp exit status: {:?}, stdout length: {}, stderr length: {}",
                            out.status, out.stdout.len(), out.stderr.len());

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
                            log::warn!("[THUMBNAIL] yt-dlp failed to get thumbnail URL, exit status: {:?}", out.status);
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

            log::info!("Video metadata received - title length: {}, artist length: {}", title.len(), artist.len());

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
            let _ = progress_msg.update(&bot_clone, DownloadStatus::Starting {
                title: display_title.as_ref().to_string(),
                file_format: Some("mp4".to_string()),
            }).await;

            // Добавляем уникальный идентификатор к имени файла для избежания конфликтов
            use std::time::{SystemTime, UNIX_EPOCH};
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0);

            let base_file_name = generate_file_name_with_ext(&title, &artist, "mp4");
            // Добавляем timestamp к имени файла (перед расширением)
            let file_name = if base_file_name.ends_with(".mp4") {
                format!("{}_{}.mp4",
                    base_file_name.trim_end_matches(".mp4"),
                    timestamp
                )
            } else {
                format!("{}_{}", base_file_name, timestamp)
            };

            log::info!("Generated filename for video: '{}' (base: '{}')", file_name, base_file_name);
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
            log::info!("[DEBUG] Checking file size before download (format: {}): {}", first_format, size_check_cmd);

            let size_check_output = timeout(
                config::download::ytdlp_timeout(),
                TokioCommand::new(ytdl_bin)
                    .args(&size_check_args)
                    .output()
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
                let mut list_formats_args: Vec<String> = vec![
                    "--list-formats".to_string(),
                    "--no-playlist".to_string(),
                ];

                let mut temp_args: Vec<&str> = vec![];
                add_cookies_args(&mut temp_args);
                for arg in temp_args {
                    list_formats_args.push(arg.to_string());
                }
                list_formats_args.push(url.as_str().to_string());

                let list_formats_output = timeout(
                    Duration::from_secs(30), // Более короткий таймаут для списка форматов
                    TokioCommand::new(ytdl_bin)
                        .args(&list_formats_args)
                        .output()
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
                                if line.contains(&format!("{}x{}", target_height, target_height)) ||
                                   (target_height == 1080 && line.contains("1920x1080")) ||
                                   (target_height == 720 && line.contains("1280x720")) ||
                                   (target_height == 480 && line.contains("854x480")) ||
                                   (target_height == 360 && line.contains("640x360")) {

                                    // Извлекаем размер (формат: ~XX.XXMiB или XX.XXMiB)
                                    if let Some(size_mb_pos) = line.find("MiB") {
                                        let before_size = &line[..size_mb_pos];
                                        if let Some(start) = before_size.rfind(|c: char| c.is_ascii_digit() || c == '.' || c == '~') {
                                            let size_str = &line[start..size_mb_pos].trim().trim_start_matches('~');
                                            if let Ok(size_mb) = size_str.parse::<f64>() {
                                                log::info!("Found format size via --list-formats: {:.2} MB for {}p", size_mb, target_height);
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
                            log::warn!("File size not available (NA) for {} quality. Will proceed with download and check size after.", quality_str);
                            log::info!("⚠️ Downloading {} video without knowing size beforehand. Will check after download.", quality_str);
                        },
                        _ => {
                            log::info!("File size not available before download (NA), will check after download");
                        }
                    }
                } else {
                    // Для локального Bot API сервера - разрешаем все форматы, даже если размер NA
                    let quality_str = video_quality.as_deref().unwrap_or("unknown");
                    log::info!("File size not available (NA) for {} quality, but local Bot API server is used (2 GB limit). Proceeding with download.", quality_str);
                }
            }

            // Step 3: Download with real-time progress updates
            let (mut progress_rx, mut download_handle) =
                download_video_file_with_progress(bot_clone.clone(), chat_id, &url, &download_path, &format_arg).await?;

            // Показываем начальный прогресс 0%
            let _ = progress_msg.update(&bot_clone, DownloadStatus::Downloading {
                title: display_title.as_ref().to_string(),
                progress: 0,
                speed_mbs: None,
                eta_seconds: None,
                current_size: None,
                total_size: None,
                file_format: Some("mp4".to_string()),
            }).await;

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
                },
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

            log::info!("Downloaded video file size (might be video-only stream, before merging): {:.2} MB", file_size as f64 / (1024.0 * 1024.0));

            // Step 3.5: Проверяем, что файл содержит и видео, и аудио дорожки
            match has_both_video_and_audio(&actual_file_path) {
                Ok(true) => {
                    log::info!("Video file verified: contains both video and audio streams");
                },
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
                    let _ = progress_msg.update(&bot_clone, DownloadStatus::Error {
                        title: display_title.as_ref().to_string(),
                        error: "Видео файл повреждён или не содержит все необходимые дорожки".to_string(),
                        file_format: Some("mp4".to_string()),
                    }).await;
                    return Err(AppError::Download("Video file missing video or audio stream".to_string()));
                },
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

                        log::info!("📝 User {} subtitle settings: download_subs={}, burn_subs={}",
                            chat_id.0, download_subs, burn_subs);

                        if download_subs && burn_subs {
                            log::info!("🔥 User requested burned subtitles - downloading subtitles and burning into video");

                            // Download subtitles first
                            let subtitle_path = format!("{}/{}_subs.srt",
                                &*config::DOWNLOAD_FOLDER,
                                safe_filename.trim_end_matches(".mp4"));

                            log::info!("📥 Downloading subtitles to: {}", subtitle_path);

                            // Download subtitles using yt-dlp
                            let ytdl_bin = &*config::YTDL_BIN;
                            let mut subtitle_args: Vec<&str> = vec![
                                "--write-subs",
                                "--write-auto-subs",
                                "--sub-lang", "en,ru",
                                "--sub-format", "srt",
                                "--convert-subs", "srt",
                                "--skip-download",
                                "--output", &subtitle_path,
                                "--no-playlist",
                            ];
                            add_cookies_args(&mut subtitle_args);
                            subtitle_args.push(url.as_str());

                            log::info!("🎬 Running yt-dlp for subtitles: {} {}", ytdl_bin, subtitle_args.join(" "));

                            let subtitle_output = TokioCommand::new(ytdl_bin)
                                .args(&subtitle_args)
                                .output()
                                .await;

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
                                        log::info!("✅ Subtitles downloaded successfully: {}", sub_file);

                                        // Burn subtitles into video
                                        let output_with_subs = format!("{}_with_subs.mp4",
                                            actual_file_path.trim_end_matches(".mp4"));

                                        log::info!("🔥 Burning subtitles into video: {} -> {}",
                                            actual_file_path, output_with_subs);

                                        match burn_subtitles_into_video(&actual_file_path, &sub_file, &output_with_subs).await {
                                            Ok(_) => {
                                                log::info!("✅ Successfully burned subtitles into video");

                                                // Delete original video and subtitle file
                                                let _ = std::fs::remove_file(&actual_file_path);
                                                let _ = std::fs::remove_file(&sub_file);

                                                output_with_subs
                                            }
                                            Err(e) => {
                                                log::error!("❌ Failed to burn subtitles: {}. Using original video.", e);
                                                // Cleanup subtitle file
                                                let _ = std::fs::remove_file(&sub_file);
                                                actual_file_path
                                            }
                                        }
                                    } else {
                                        log::warn!("⚠️ Subtitles not found after download. Using original video.");
                                        actual_file_path
                                    }
                                }
                                Ok(output) => {
                                    log::warn!("⚠️ yt-dlp failed to download subtitles: {}",
                                        String::from_utf8_lossy(&output.stderr));
                                    actual_file_path
                                }
                                Err(e) => {
                                    log::warn!("⚠️ Failed to execute yt-dlp for subtitles: {}", e);
                                    actual_file_path
                                }
                            }
                        } else {
                            actual_file_path
                        }
                    }
                    Err(_) => actual_file_path
                }
            } else {
                actual_file_path
            };

            // Step 4: Get user preference for send_as_document
            let send_as_document = if let Some(ref pool) = db_pool_clone {
                match db::get_connection(pool) {
                    Ok(conn) => {
                        let value = db::get_user_send_as_document(&conn, chat_id.0).unwrap_or(0);
                        log::info!("📊 User {} send_as_document value from DB: {} ({})",
                            chat_id.0,
                            value,
                            if value == 0 { "Media/send_video" } else { "Document/send_document" }
                        );
                        value == 1
                    }
                    Err(_) => false
                }
            } else {
                false
            };

            // Log final merged file size before sending
            let final_file_size = fs::metadata(&actual_file_path)
                .map(|m| m.len())
                .unwrap_or(0);
            log::info!("📦 Final merged video file size (before sending): {:.2} MB", final_file_size as f64 / (1024.0 * 1024.0));

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

                log::info!("📤 Sending video part {}/{} ({}): {}", part_index, total_parts, part_path, current_caption);

                // Send video with retry logic and animation
                let (sent_message, file_size) = send_video_with_retry(&bot_clone, chat_id, part_path, &mut progress_msg, &current_caption, thumbnail_url.as_deref(), send_as_document).await?;

                // Save to download history after successful send
                if let Some(ref pool) = db_pool_clone {
                    if let Ok(conn) = crate::storage::db::get_connection(pool) {
                        let file_id = sent_message.video().map(|v| v.file.id.0.clone())
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
                            title.as_str(),  // Just the title without artist
                            "mp4",
                            file_id.as_deref(),
                            author_opt,
                            Some(file_size as i64),
                            duration,
                            Some(quality),
                            None,  // audio_bitrate (N/A for mp4)
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
            let _ = progress_msg.update(&bot_clone, DownloadStatus::Success {
                title: display_title.as_ref().to_string(),
                elapsed_secs,
                file_format: Some("mp4".to_string()),
            }).await;

            // Mark the original message as completed if message_id is available
            if let Some(msg_id) = message_id {
                use teloxide::types::MessageId;
                crate::telegram::try_set_reaction(&bot_clone, chat_id, MessageId(msg_id), crate::telegram::emoji::THUMBS_UP).await;
            }

            // Step 5: Auto-clear success message after delay (оставляем только название)
            let bot_for_clear = bot_clone.clone();
            let title_for_clear = Arc::clone(&display_title);
            let mut msg_for_clear = ProgressMessage {
                chat_id: progress_msg.chat_id,
                message_id: progress_msg.message_id,
            };
            tokio::spawn(async move {
                let _ = msg_for_clear.clear_after(&bot_for_clear, config::progress::CLEAR_DELAY_SECS, title_for_clear.as_ref().to_string(), Some("mp3".to_string())).await;
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
                    log::debug!("Failed to delete expected file {} (this is OK if it doesn't exist): {}", download_path, e);
                }
            }

            Ok(())
        }.await;

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
                    "Это видео недоступно для скачивания 😢\n\n\
                Возможные причины:\n\
                • Видео удалено или приватное\n\
                • Возрастные ограничения\n\
                • Региональные ограничения\n\
                • Стрим или премьера (еще не доступны)\n\n\
                Попробуй другое видео!",
                )
            } else if error_str.contains("Signature extraction failed") {
                Some(
                    "У меня устарела версия загрузчика 😢\n\n\
                Стэн уже знает и скоро обновит!\n\
                Попробуй позже или другое видео.",
                )
            } else if error_str.contains("Sign in to confirm you're not a bot") || error_str.contains("bot detection") {
                Some(
                    "YouTube заблокировал бота 🤖\n\n\
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
            log::info!("[DEBUG] yt-dlp command for subtitles download: {}", command_str);

            let mut child = spawn_downloader_with_fallback(ytdl_bin, &args)?;
            let status = child
                .wait()
                .map_err(|e| AppError::Download(format!("downloader process failed: {}", e)))?;

            if !status.success() {
                return Err(AppError::Download(format!("downloader exited with status: {}", status)));
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
    use crate::download::audio::download_audio_file;
    use crate::download::metadata::{probe_duration_seconds, validate_cookies_file_format};
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

    // Integration-ish test: requires network and yt-dlp (or youtube-dl) + ffmpeg installed.
    // It downloads to a temp path and ensures file appears, then cleans up.
    #[test]
    #[ignore]
    fn test_download_audio_file_from_youtube() {
        if !(tool_exists("yt-dlp") || tool_exists("youtube-dl")) {
            eprintln!("skipping: no yt-dlp/youtube-dl in PATH");
            return;
        }
        if !tool_exists("ffprobe") {
            // ffmpeg suite
            eprintln!("skipping: no ffprobe in PATH");
            return;
        }
        let url = Url::parse("https://www.youtube.com/watch?v=0CAltmPaNZY").expect("Test URL should be valid");
        let tmp_dir = std::env::temp_dir();
        let dest = tmp_dir.join(format!("test_dl_{}.mp3", uuid::Uuid::new_v4()));
        let dest_str = dest.to_string_lossy().to_string();
        let res = download_audio_file(&url, &dest_str);
        match res {
            Ok(_dur_opt) => {
                assert!(std::path::Path::new(&dest_str).exists());
                let _ = fs::remove_file(&dest_str);
            }
            Err(e) => {
                let _ = fs::remove_file(&dest_str); // Cleanup on error
                panic!("Download test failed: {:?}", e);
            }
        }
    }
}
