use teloxide::prelude::*;
use teloxide::types::InputFile;
use crate::rate_limiter::RateLimiter;
use crate::utils::escape_filename;
use crate::progress::{ProgressMessage, DownloadStatus};
use crate::config;
use crate::error::AppError;
use crate::db::{DbPool, save_download_history};
use std::sync::Arc;
use url::Url;
use std::process::{Command, Stdio};
use std::io::{BufRead, BufReader};
use tokio::process::Command as TokioCommand;
use tokio::time::timeout;
use chrono::{DateTime, Utc};
use std::fs;

/// Legacy alias for backward compatibility
/// Use AppError instead
#[deprecated(note = "Use AppError instead")]
pub type CommandError = AppError;

fn probe_duration_seconds(path: &str) -> Option<u32> {
    let output = Command::new("ffprobe")
        .args([
            "-v", "error",
            "-show_entries", "format=duration",
            "-of", "default=noprint_wrappers=1:nokey=1",
            path,
        ])
        .output()
        .ok()?;

    let duration_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if duration_str.is_empty() { return None; }
    let secs = duration_str.parse::<f32>().ok()?;
    Some(secs.round() as u32)
}

/// Получить метаданные от yt-dlp (быстрее чем HTTP парсинг)
/// Использует async команду чтобы не блокировать runtime
async fn get_metadata_from_ytdlp(url: &Url) -> Result<(String, String), AppError> {
    let ytdl_bin = &*config::YTDL_BIN;
    log::debug!("Using downloader binary: {}", ytdl_bin);
    log::debug!("Fetching metadata for URL: {}", url);

    // Получаем title используя async команду с таймаутом
    let title_output = timeout(
        config::download::ytdlp_timeout(),
        TokioCommand::new(&ytdl_bin)
            .args(["--get-title", "--no-playlist", url.as_str()])
            .output()
    )
    .await
    .map_err(|_| {
        log::error!("yt-dlp command timed out after {} seconds", config::download::YTDLP_TIMEOUT_SECS);
        AppError::Download(format!("yt-dlp command timed out"))
    })?
    .map_err(|e| {
        log::error!("Failed to execute {}: {}", ytdl_bin, e);
        AppError::Download(format!("Failed to get title: {}", e))
    })?;
    
    log::debug!("yt-dlp exit status: {:?}, stdout length: {}", title_output.status, title_output.stdout.len());
    
    if !title_output.status.success() {
        let stderr = String::from_utf8_lossy(&title_output.stderr);
        log::error!("yt-dlp failed with stderr: {}", stderr);
    }

    let title = if title_output.status.success() {
        String::from_utf8_lossy(&title_output.stdout).trim().to_string()
    } else {
        log::warn!("yt-dlp returned non-zero status, using default title");
        "Unknown Track".to_string()
    };

    // Для простоты, artist пока оставляем пустым
    // В будущем можно парсить через --print "%(artist)s"
    let artist = String::new();

    log::info!("Got metadata from yt-dlp: title='{}', artist='{}'", title, artist);
    Ok((title, artist))
}

/// Отправляет сообщение об ошибке с случайным стикером
async fn send_error_with_sticker(bot: &Bot, chat_id: ChatId) {
    // Список file_id стикеров из стикерпака doraduradoradura
    let sticker_file_ids = vec![
        "CAACAgIAAxUAAWj-ZokEQu5YpTnjl6IWPzCQZ0UUAAJCEwAC52QwSC6nTghQdw-KNgQ",
        "CAACAgIAAxUAAWj-ZomIQgQKKpbMZA0_VDzfavIiAAK1GgACt8dBSNRj5YvFS-dmNgQ",
        "CAACAgIAAxUAAWj-Zokct93wagdDXh1JbhxBIyJOAALzFwACoktASAOjHltqzx0ENgQ",
        "CAACAgIAAxUAAWj-ZomorWU-YHGN6oQ6-ikN46CJAAInFAACqlJYSGHilrVqW1AxNgQ",
        "CAACAgIAAxUAAWj-ZonVzqfhCC1-YjDNhqGioqvVAALdEwAC-_ZpSB5PRC_sd93QNgQ",
        "CAACAgIAAxkBAAIFymj-YswNosbIex7SmXJejbO_GN7-AAJMGQAC9MFQSHBzdKlbjXskNgQ",
        "CAACAgIAAxUAAWj-Zol_H6tZIPG-PPHnpNZS1QkIAAJFGwACIQtBSDwm6rS-ZojVNgQ",
        "CAACAgIAAxUAAWj-ZomOtDnC9_6jFRp84js-HQN5AALzEgACqc5ISI4uefJ9dzZPNgQ",
        "CAACAgIAAxUAAWj-ZolmPZFTqhyNqwssS4JVQY_AAALgFAACU7NBSCIDa2YqXjXyNgQ",
        "CAACAgIAAxUAAWj-ZonZTWGW2DadfQ2Mo6bHAAHy2AACjxEAAgSTSUj1H3gU_UUHdjYE",
        "CAACAgIAAxUAAWj-ZolQ6OCfECavW19ATgcCup5PAAIOFgACgbdJSMOkkJfpAbs_NgQ",
        "CAACAgIAAxUAAWj-Zol19ilXmGth6SKa-4FRrSEJAAJRFwACM9JISKFYdRXvbsb1NgQ",
        "CAACAgIAAxUAAWj-ZokRA50GUCiz_OXQUih3uljfAAIeGQACsyBISDP8m_5FL5CJNgQ",
        "CAACAgIAAxUAAWj-ZomiM5Mt2aK1G3b8O7JK-shMAALPFQACWGhoSMeITTonc71ENgQ",
        "CAACAgIAAxUAAWj-ZomSF9AsKZr6myR3lYgyc-HyAAIRGQACM9KRSG5IUy40KB2KNgQ",
    ];

    // Генерируем случайный индекс используя timestamp
    use std::time::{SystemTime, UNIX_EPOCH};
    let random_index = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(timestamp) => (timestamp.as_nanos() as usize) % sticker_file_ids.len(),
        Err(_) => 0,
    };
    let random_sticker_id = sticker_file_ids[random_index];

    // Отправляем случайный стикер
    if let Err(e) = bot.send_sticker(chat_id, InputFile::file_id(random_sticker_id)).await {
        log::error!("Failed to send error sticker: {}", e);
    }

    // Отправляем сообщение об ошибке
    if let Err(e) = bot.send_message(chat_id, "У меня не получилось, все сломалось 😢 Я написала Стэну").await {
        log::error!("Failed to send error message: {}", e);
    }
}

fn spawn_downloader_with_fallback(ytdl_bin: &str, args: &[&str]) -> Result<std::process::Child, AppError> {
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
                    .map_err(|inner| AppError::Download(format!(
                        "Failed to start downloader. Tried '{}', then '{}': {} / {}",
                        ytdl_bin, fallback, e, inner
                    )))
            } else {
                Err(AppError::Download(format!("Failed to start downloader '{}': {}", ytdl_bin, e)))
            }
        })
}

/// Парсит процент прогресса из строки вывода yt-dlp
/// Пример: "[download]  45.2% of 10.00MiB at 500.00KiB/s ETA 00:10"
fn parse_progress(line: &str) -> Option<u8> {
    if line.contains("[download]") && line.contains("%") {
        // Ищем паттерн типа "45.2%"
        let parts: Vec<&str> = line.split_whitespace().collect();
        for part in parts {
            if part.ends_with('%') {
                if let Ok(percent) = part.trim_end_matches('%').parse::<f32>() {
                    return Some(percent.min(100.0) as u8);
                }
            }
        }
    }
    None
}

fn download_audio_file(url: &Url, download_path: &str) -> Result<Option<u32>, AppError> {
    let ytdl_bin = &*config::YTDL_BIN;
    let args = [
        "-o", download_path,
        "--newline", // Выводить прогресс построчно (критично!)
        "--extract-audio",
        "--audio-format", "mp3",
        "--audio-quality", "0",
        "--add-metadata",
        "--embed-thumbnail",
        "--no-playlist",
        "--concurrent-fragments", "5",
        "--postprocessor-args", "-acodec libmp3lame -b:a 320k",
        url.as_str(),
    ];
    let mut child = spawn_downloader_with_fallback(&ytdl_bin, &args)?;
    let status = child
        .wait()
        .map_err(|e| AppError::Download(format!("downloader process failed: {}", e)))?;
    if !status.success() {
        return Err(AppError::Download(format!("downloader exited with status: {}", status)));
    }
    Ok(probe_duration_seconds(download_path))
}

/// Скачивает аудио с отслеживанием прогресса через channel
async fn download_audio_file_with_progress(
    url: &Url,
    download_path: &str,
) -> Result<(tokio::sync::mpsc::UnboundedReceiver<u8>, tokio::task::JoinHandle<Result<Option<u32>, AppError>>), AppError> {
    let ytdl_bin = config::YTDL_BIN.clone();
    let url_str = url.to_string();
    let download_path_clone = download_path.to_string();

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

    // Запускаем в blocking task, так как читаем stdout построчно
    let handle = tokio::task::spawn_blocking(move || {
        let mut child = Command::new(&ytdl_bin)
            .args([
                "-o", &download_path_clone,
                "--newline", // Выводить прогресс построчно
                "--extract-audio",
                "--audio-format", "mp3",
                "--audio-quality", "0",
                "--add-metadata",
                "--embed-thumbnail",
                "--no-playlist",
                "--concurrent-fragments", "5",
                "--postprocessor-args", "-acodec libmp3lame -b:a 320k",
                &url_str,
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| AppError::Download(format!("Failed to spawn yt-dlp: {}", e)))?;

        // Читаем stdout и stderr построчно для отслеживания прогресса
        // Прогресс может быть как в stdout, так и в stderr
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        // Объединяем оба потока
        use std::thread;
        let tx_clone = tx.clone();

        if let Some(stderr_stream) = stderr {
            thread::spawn(move || {
                let reader = BufReader::new(stderr_stream);
                for line in reader.lines() {
                    if let Ok(line_str) = line {
                        log::debug!("yt-dlp stderr: {}", line_str);
                        if let Some(progress) = parse_progress(&line_str) {
                            log::info!("Parsed progress from stderr: {}%", progress);
                            let _ = tx_clone.send(progress);
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
                    if let Some(progress) = parse_progress(&line_str) {
                        log::info!("Parsed progress from stdout: {}%", progress);
                        let _ = tx.send(progress);
                    }
                }
            }
        }

        let status = child.wait()
            .map_err(|e| AppError::Download(format!("downloader process failed: {}", e)))?;

        if !status.success() {
            return Err(AppError::Download(format!("downloader exited with status: {}", status)));
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
/// 
/// # Returns
/// 
/// Returns `Ok(())` on success or a `ResponseResult` error.
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
pub async fn download_and_send_audio(bot: Bot, chat_id: ChatId, url: Url, rate_limiter: Arc<RateLimiter>, _created_timestamp: DateTime<Utc>, db_pool: Option<Arc<DbPool>>) -> ResponseResult<()> {
    log::info!("Starting download_and_send_audio for chat {} with URL: {}", chat_id, url);
    let bot_clone = bot.clone();
    let _rate_limiter = Arc::clone(&rate_limiter);
    let db_pool_clone = db_pool.clone();

    tokio::spawn(async move {
        log::info!("Inside spawn for audio download, chat_id: {}", chat_id);
        let mut progress_msg = ProgressMessage::new(chat_id);
        let start_time = std::time::Instant::now();

        let result: Result<(), AppError> = async {
            // Step 1: Get metadata and show starting status
            let (title, artist) = match get_metadata_from_ytdlp(&url).await {
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

            let display_title: Arc<str> = if artist.trim().is_empty() {
                Arc::from(title.as_str())
            } else {
                Arc::from(format!("{} - {}", artist, title))
            };

            // Show starting status
            let _ = progress_msg.update(&bot_clone, DownloadStatus::Starting {
                title: display_title.as_ref().to_string()
            }).await;

            let file_name = generate_file_name(&title, &artist);
            let safe_filename = escape_filename(&file_name);
            let full_path = format!("~/downloads/{}", safe_filename);
            let download_path = shellexpand::tilde(&full_path).into_owned();

            // Step 2: Download with real-time progress updates
            let (mut progress_rx, mut download_handle) = download_audio_file_with_progress(&url, &download_path).await?;

            // Читаем обновления прогресса из channel
            let bot_for_progress = bot_clone.clone();
            let title_for_progress = Arc::clone(&display_title);
            let mut last_progress = 0u8;

            let duration_result = loop {
                tokio::select! {
                    // Получаем обновления прогресса
                    Some(progress) = progress_rx.recv() => {
                        // Обновляем только на значимых изменениях (каждые 10%)
                        if progress % 10 == 0 && progress != last_progress {
                            last_progress = progress;
                            let _ = progress_msg.update(&bot_for_progress, DownloadStatus::Downloading {
                                title: title_for_progress.as_ref().to_string(),
                                progress,
                            }).await;
                        }
                    }
                    // Ждем завершения загрузки
                    result = &mut download_handle => {
                        break result.map_err(|e| AppError::Download(format!("Task join error: {}", e)))??;
                    }
                }
            };

            log::debug!("Download path: {:?}", download_path);

            let duration: u32 = duration_result.unwrap_or(0);

            // Calculate elapsed time
            let elapsed_secs = start_time.elapsed().as_secs();
            log::info!("Audio downloaded in {} seconds", elapsed_secs);

            // Step 3: Validate file size before sending
            let file_size = fs::metadata(&download_path)
                .map_err(|e| AppError::Download(format!("Failed to get file metadata: {}", e)))?
                .len();
            
            if file_size > config::validation::MAX_AUDIO_SIZE_BYTES {
                let size_mb = file_size as f64 / (1024.0 * 1024.0);
                let max_mb = config::validation::MAX_AUDIO_SIZE_BYTES as f64 / (1024.0 * 1024.0);
                log::warn!("Audio file too large: {:.2} MB (max: {:.2} MB)", size_mb, max_mb);
                send_error_with_sticker(&bot_clone, chat_id).await;
                let _ = progress_msg.update(&bot_clone, DownloadStatus::Error {
                    title: display_title.as_ref().to_string(),
                    error: format!("Файл слишком большой ({:.2} MB). Максимальный размер: {:.2} MB", size_mb, max_mb),
                }).await;
                return Err(AppError::Validation(format!("File too large: {:.2} MB", size_mb)));
            }

            // Step 4: Send audio with retry logic and animation
            send_audio_with_retry(&bot_clone, chat_id, &download_path, duration, &mut progress_msg, display_title.as_ref()).await?;

            // Save to download history after successful send
            if let Some(ref pool) = db_pool_clone {
                if let Ok(conn) = crate::db::get_connection(pool) {
                    if let Err(e) = save_download_history(&conn, chat_id.0, url.as_str(), display_title.as_ref(), "mp3") {
                        log::warn!("Failed to save download history: {}", e);
                    }
                }
            }

            // Step 5: Show success status with time
            let _ = progress_msg.update(&bot_clone, DownloadStatus::Success {
                title: display_title.as_ref().to_string(),
                elapsed_secs,
            }).await;

            log::info!("Audio sent successfully to chat {}", chat_id);

            // Step 5: Auto-clear success message after delay (оставляем только название)
            let bot_for_clear = bot_clone.clone();
            let title_for_clear = Arc::clone(&display_title);
            let mut msg_for_clear = ProgressMessage {
                chat_id: progress_msg.chat_id,
                message_id: progress_msg.message_id,
            };
            tokio::spawn(async move {
                let _ = msg_for_clear.clear_after(&bot_for_clear, config::progress::CLEAR_DELAY_SECS, title_for_clear.as_ref().to_string()).await;
            });

            // Wait before cleaning up file
            tokio::time::sleep(config::download::cleanup_delay()).await;
            fs::remove_file(&download_path).map_err(|e| AppError::Download(format!("Failed to delete file: {}", e)))?;

            Ok(())
        }.await;

        match result {
            Ok(_) => {
                log::info!("Audio download completed successfully for chat {}", chat_id);
            }
            Err(e) => {
                log::error!("An error occurred during audio download for chat {}: {:?}", chat_id, e);
                // Send error sticker and message
                send_error_with_sticker(&bot_clone, chat_id).await;
                // Show error status
                let _ = progress_msg.update(&bot_clone, DownloadStatus::Error {
                    title: "Скачивание".to_string(),
                    error: e.to_string(),
                }).await;
            }
        }
    });
    log::info!("download_and_send_audio function returned, spawn task started");
    Ok(())
}

/// Generic function to send files with retry logic and animation
/// Args: bot - telegram bot instance, chat_id - user's chat ID, download_path - path to file, progress_msg - progress message handler, title - file title, file_type - type of file ("audio" or "video"), send_fn - closure that sends the file
/// Functionality: Sends file with retry logic, shows uploading animation, handles errors
async fn send_file_with_retry<F, Fut>(
    bot: &Bot,
    chat_id: ChatId,
    download_path: &str,
    progress_msg: &mut ProgressMessage,
    title: &str,
    file_type: &str,
    send_fn: F,
) -> Result<(), AppError>
where
    F: Fn(Bot, ChatId, String) -> Fut,
    Fut: std::future::Future<Output = ResponseResult<Message>>,
{
    let max_attempts = config::retry::MAX_ATTEMPTS;
    let download_path = download_path.to_string();

    // Validate file size before sending
    let file_size = fs::metadata(&download_path)
        .map_err(|e| AppError::Download(format!("Failed to get file metadata: {}", e)))?
        .len();
    
    let max_size = match file_type {
        "audio" => config::validation::MAX_AUDIO_SIZE_BYTES,
        "video" => config::validation::MAX_VIDEO_SIZE_BYTES,
        _ => config::validation::MAX_FILE_SIZE_BYTES,
    };
    
    if file_size > max_size {
        let size_mb = file_size as f64 / (1024.0 * 1024.0);
        let max_mb = max_size as f64 / (1024.0 * 1024.0);
        log::warn!("File {} too large: {:.2} MB (max: {:.2} MB)", download_path, size_mb, max_mb);
        return Err(AppError::Validation(format!(
            "Файл слишком большой ({:.2} MB). Максимальный размер: {:.2} MB",
            size_mb, max_mb
        )));
    }

    for attempt in 1..=max_attempts {
        log::info!("Attempting to send {} to chat {} (attempt {}/{})", file_type, chat_id, attempt, max_attempts);

        // Запускаем анимацию точек в отдельной задаче
        let bot_clone = bot.clone();
        let title_clone = title.to_string();
        let mut msg_clone = ProgressMessage {
            chat_id: progress_msg.chat_id,
            message_id: progress_msg.message_id,
        };

        let animation_handle = tokio::spawn(async move {
            let mut dots = 0u8;
            loop {
                let _ = msg_clone.update(&bot_clone, DownloadStatus::Uploading {
                    title: title_clone.clone(),
                    dots,
                }).await;
                dots = (dots + 1) % 4;
                tokio::time::sleep(config::animation::update_interval()).await;
            }
        });

        let response = send_fn(bot.clone(), chat_id, download_path.clone()).await;

        // Останавливаем анимацию
        animation_handle.abort();

        // Небольшая задержка, чтобы убедиться, что анимация точно остановилась
        tokio::time::sleep(config::animation::stop_delay()).await;

        match response {
            Ok(_) => {
                log::info!("Successfully sent {} to chat {} on attempt {}", file_type, chat_id, attempt);
                return Ok(());
            },
            Err(e) if attempt < max_attempts => {
                log::warn!("Attempt {}/{} failed for chat {}: {}. Retrying...",
                    attempt, max_attempts, chat_id, e);
                tokio::time::sleep(config::retry::delay()).await;
            },
            Err(e) => {
                log::error!("All {} attempts failed to send {} to chat {}: {}", max_attempts, file_type, chat_id, e);
                let error_msg = match file_type {
                    "video" => format!("У меня не получилось отправить тебе видео 🥲 попробуй как-нибудь позже. Все {} попытки не удались: {}", max_attempts, e),
                    _ => format!("Failed to send {} file after {} attempts: {}", file_type, max_attempts, e.to_string()),
                };
                return Err(AppError::Download(error_msg));
            },
        }
    }

    unreachable!()
}

/// Send audio file with retry logic
/// Args: bot - telegram bot instance, chat_id - user's chat ID, download_path - path to audio file, duration - audio duration in seconds, progress_msg - progress message handler, title - audio title
/// Functionality: Wrapper around send_file_with_retry for audio files
async fn send_audio_with_retry(
    bot: &Bot,
    chat_id: ChatId,
    download_path: &str,
    duration: u32,
    progress_msg: &mut ProgressMessage,
    title: &str,
) -> Result<(), AppError> {
    let duration = duration; // Capture duration for closure
    send_file_with_retry(
        bot,
        chat_id,
        download_path,
        progress_msg,
        title,
        "audio",
        move |bot, chat_id, path| {
            let duration = duration;
            async move {
                bot.send_audio(chat_id, InputFile::file(path))
                    .duration(duration)
                    .await
            }
        },
    ).await
}

/// Send video file with retry logic
/// Args: bot - telegram bot instance, chat_id - user's chat ID, download_path - path to video file, progress_msg - progress message handler, title - video title
/// Functionality: Wrapper around send_file_with_retry for video files
async fn send_video_with_retry(
    bot: &Bot,
    chat_id: ChatId,
    download_path: &str,
    progress_msg: &mut ProgressMessage,
    title: &str,
) -> Result<(), AppError> {
    send_file_with_retry(
        bot,
        chat_id,
        download_path,
        progress_msg,
        title,
        "video",
        move |bot, chat_id, path| async move {
            bot.send_video(chat_id, InputFile::file(path))
                .await
        },
    ).await
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
pub async fn download_and_send_video(bot: Bot, chat_id: ChatId, url: Url, rate_limiter: Arc<RateLimiter>, _created_timestamp: DateTime<Utc>, db_pool: Option<Arc<DbPool>>) -> ResponseResult<()> {
    let bot_clone = bot.clone();
    let _rate_limiter = Arc::clone(&rate_limiter);
    let db_pool_clone = db_pool.clone();

    tokio::spawn(async move {
        let mut progress_msg = ProgressMessage::new(chat_id);
        let start_time = std::time::Instant::now();

        let result: Result<(), AppError> = async {
            // Step 1: Get metadata and show starting status
            let (title, artist) = match get_metadata_from_ytdlp(&url).await {
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

            let display_title: Arc<str> = if artist.trim().is_empty() {
                Arc::from(title.as_str())
            } else {
                Arc::from(format!("{} - {}", artist, title))
            };

            // Show starting status
            let _ = progress_msg.update(&bot_clone, DownloadStatus::Starting {
                title: display_title.as_ref().to_string()
            }).await;

            let file_name = generate_file_name(&title, &artist);
            let safe_filename = escape_filename(&file_name);
            let full_path = format!("~/downloads/{}", safe_filename);
            let download_path = shellexpand::tilde(&full_path).into_owned();

            // Step 2: Show downloading status with simulated progress
            // TODO: Можно добавить real-time прогресс как для аудио
            let _ = progress_msg.update(&bot_clone, DownloadStatus::Downloading {
                title: display_title.as_ref().to_string(),
                progress: 10,
            }).await;

            let ytdl_bin = &*config::YTDL_BIN;
            let args = [
                "-o", &download_path,
                "--newline",
                "--format", "best",
                "--concurrent-fragments", "5",
                url.as_str(),
            ];
            let mut child = spawn_downloader_with_fallback(&ytdl_bin, &args)?;
            let status = child.wait().map_err(|e| AppError::Download(format!("downloader process failed: {}", e)))?;

            if !status.success() {
                return Err(AppError::Download(format!("downloader exited with status: {}", status)));
            }

            // Update to 90% after download
            let _ = progress_msg.update(&bot_clone, DownloadStatus::Downloading {
                title: display_title.as_ref().to_string(),
                progress: 90,
            }).await;

            log::debug!("Download path: {:?}", download_path);

            // Calculate elapsed time
            let elapsed_secs = start_time.elapsed().as_secs();
            log::info!("Video downloaded in {} seconds", elapsed_secs);

            // Step 3: Validate file size before sending
            let file_size = fs::metadata(&download_path)
                .map_err(|e| AppError::Download(format!("Failed to get file metadata: {}", e)))?
                .len();
            
            if file_size > config::validation::MAX_VIDEO_SIZE_BYTES {
                let size_mb = file_size as f64 / (1024.0 * 1024.0);
                let max_mb = config::validation::MAX_VIDEO_SIZE_BYTES as f64 / (1024.0 * 1024.0);
                log::warn!("Video file too large: {:.2} MB (max: {:.2} MB)", size_mb, max_mb);
                send_error_with_sticker(&bot_clone, chat_id).await;
                let _ = progress_msg.update(&bot_clone, DownloadStatus::Error {
                    title: display_title.as_ref().to_string(),
                    error: format!("Файл слишком большой ({:.2} MB). Максимальный размер: {:.2} MB", size_mb, max_mb),
                }).await;
                return Err(AppError::Validation(format!("File too large: {:.2} MB", size_mb)));
            }

            // Step 4: Send video with retry logic and animation
            send_video_with_retry(&bot_clone, chat_id, &download_path, &mut progress_msg, display_title.as_ref()).await?;

            // Save to download history after successful send
            if let Some(ref pool) = db_pool_clone {
                if let Ok(conn) = crate::db::get_connection(pool) {
                    if let Err(e) = save_download_history(&conn, chat_id.0, url.as_str(), display_title.as_ref(), "mp4") {
                        log::warn!("Failed to save download history: {}", e);
                    }
                }
            }

            // Step 5: Show success status with time
            let _ = progress_msg.update(&bot_clone, DownloadStatus::Success {
                title: display_title.as_ref().to_string(),
                elapsed_secs,
            }).await;

            // Step 5: Auto-clear success message after delay (оставляем только название)
            let bot_for_clear = bot_clone.clone();
            let title_for_clear = Arc::clone(&display_title);
            let mut msg_for_clear = ProgressMessage {
                chat_id: progress_msg.chat_id,
                message_id: progress_msg.message_id,
            };
            tokio::spawn(async move {
                let _ = msg_for_clear.clear_after(&bot_for_clear, config::progress::CLEAR_DELAY_SECS, title_for_clear.as_ref().to_string()).await;
            });

            tokio::time::sleep(config::download::cleanup_delay()).await;
            fs::remove_file(&download_path).map_err(|e| AppError::Download(format!("Failed to delete file: {}", e)))?;

            Ok(())
        }.await;

        if let Err(e) = result {
            log::error!("An error occurred during video download for chat {}: {:?}", chat_id, e);
            // Send error sticker and message
            send_error_with_sticker(&bot_clone, chat_id).await;
            // Show error status
            let _ = progress_msg.update(&bot_clone, DownloadStatus::Error {
                title: "Скачивание".to_string(),
                error: e.to_string(),
            }).await;
        }
    });
    Ok(())
}

fn generate_file_name(title: &str, artist: &str) -> String {
    if artist.trim().is_empty() && title.trim().is_empty() {
        "Unknown.mp3".to_string()
    } else if artist.trim().is_empty() {
        format!("{}.mp3", title)
    } else if title.trim().is_empty() {
        format!("{}.mp3", artist)
    } else {
        format!("{} - {}.mp3", artist, title)
    }
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
pub async fn download_and_send_subtitles(bot: Bot, chat_id: ChatId, url: Url, rate_limiter: Arc<RateLimiter>, _created_timestamp: DateTime<Utc>, subtitle_format: String, db_pool: Option<Arc<DbPool>>) -> ResponseResult<()> {
    let bot_clone = bot.clone();
    let _rate_limiter = Arc::clone(&rate_limiter);
    let db_pool_clone = db_pool.clone();

    tokio::spawn(async move {
        let mut progress_msg = ProgressMessage::new(chat_id);
        let start_time = std::time::Instant::now();

        let result: Result<(), AppError> = async {
            // Step 1: Get metadata
            let (title, _) = match get_metadata_from_ytdlp(&url).await {
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
            let _ = progress_msg.update(&bot_clone, DownloadStatus::Starting {
                title: display_title.as_ref().to_string()
            }).await;

            let file_name = format!("{}.{}", title, subtitle_format);
            let safe_filename = escape_filename(&file_name);
            let full_path = format!("~/downloads/{}", safe_filename);
            let download_path = shellexpand::tilde(&full_path).into_owned();

            // Step 2: Download subtitles
            let ytdl_bin = &*config::YTDL_BIN;
            let sub_format_flag = match subtitle_format.as_str() {
                "srt" => "--convert-subs=srt",
                "txt" => "--convert-subs=txt",
                _ => "--convert-subs=srt",
            };
            
            let args = [
                "-o", &download_path,
                "--skip-download",
                "--write-auto-subs",
                sub_format_flag,
                url.as_str(),
            ];

            let mut child = spawn_downloader_with_fallback(&ytdl_bin, &args)?;
            let status = child.wait().map_err(|e| AppError::Download(format!("downloader process failed: {}", e)))?;

            if !status.success() {
                return Err(AppError::Download(format!("downloader exited with status: {}", status)));
            }

            // Check if file exists
            if !fs::metadata(&download_path).is_ok() {
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
                    let _ = bot_clone
                        .send_document(chat_id, InputFile::file(&found))
                        .await
                        .map_err(|e| AppError::Download(format!("Failed to send document: {}", e)))?;
                } else {
                    return Err(AppError::Download(format!("Subtitle file not found")));
                }
            } else {
                // Send the file
                let _ = bot_clone
                    .send_document(chat_id, InputFile::file(&download_path))
                    .await
                    .map_err(|e| AppError::Download(format!("Failed to send document: {}", e)))?;
            }

            // Calculate elapsed time
            let elapsed_secs = start_time.elapsed().as_secs();
            log::info!("Subtitle downloaded in {} seconds", elapsed_secs);

            // Save to download history after successful send
            if let Some(ref pool) = db_pool_clone {
                if let Ok(conn) = crate::db::get_connection(pool) {
                    if let Err(e) = save_download_history(&conn, chat_id.0, url.as_str(), display_title.as_ref(), &subtitle_format) {
                        log::warn!("Failed to save download history: {}", e);
                    }
                }
            }

            // Step 3: Show success status
            let _ = progress_msg.update(&bot_clone, DownloadStatus::Success {
                title: display_title.as_ref().to_string(),
                elapsed_secs,
            }).await;

            log::info!("Subtitle sent successfully to chat {}", chat_id);

            // Step 4: Auto-clear success message
            let bot_for_clear = bot_clone.clone();
            let title_for_clear = Arc::clone(&display_title);
            let mut msg_for_clear = ProgressMessage {
                chat_id: progress_msg.chat_id,
                message_id: progress_msg.message_id,
            };
            tokio::spawn(async move {
                let _ = msg_for_clear.clear_after(&bot_for_clear, 10, title_for_clear.as_ref().to_string()).await;
            });

            // Clean up file after 10 minutes
            tokio::time::sleep(config::download::cleanup_delay()).await;
            fs::remove_file(&download_path).map_err(|e| AppError::Download(format!("Failed to delete file: {}", e)))?;

            Ok(())
        }.await;

        if let Err(e) = result {
            log::error!("An error occurred during subtitle download for chat {}: {:?}", chat_id, e);
            // Send error sticker and message
            send_error_with_sticker(&bot_clone, chat_id).await;
            // Show error status
            let _ = progress_msg.update(&bot_clone, DownloadStatus::Error {
                title: "Скачивание".to_string(),
                error: e.to_string(),
            }).await;
        }
    });
    Ok(())
}

#[cfg(test)]
mod download_tests {
    use super::*;

    fn tool_exists(bin: &str) -> bool {
        Command::new("which").arg(bin).output().map(|o| o.status.success()).unwrap_or(false)
    }

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

    // Integration-ish test: requires network and yt-dlp (or youtube-dl) + ffmpeg installed.
    // It downloads to a temp path and ensures file appears, then cleans up.
    #[test]
    #[ignore]
    fn test_download_audio_file_from_youtube() {
        if !(tool_exists("yt-dlp") || tool_exists("youtube-dl")) {
            eprintln!("skipping: no yt-dlp/youtube-dl in PATH");
            return;
        }
        if !tool_exists("ffprobe") { // ffmpeg suite
            eprintln!("skipping: no ffprobe in PATH");
            return;
        }
        let url = Url::parse("https://www.youtube.com/watch?v=0CAltmPaNZY")
            .expect("Test URL should be valid");
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
