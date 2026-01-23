//! Telegram file sending functionality with progress tracking and retry logic.
//!
//! This module provides utilities for sending files to Telegram with:
//! - Progress tracking during uploads
//! - Automatic retry logic with rate limit handling
//! - Support for both audio and video files
//! - Error sticker notifications for failed operations

use crate::core::config;
use crate::core::error::AppError;
use crate::core::metrics;
use crate::core::{extract_retry_after, is_timeout_or_network_error, BOT_API_RESPONSE_REGEX, BOT_API_START_REGEX};
use crate::download::metadata::probe_video_metadata;
use crate::download::progress::{DownloadStatus, ProgressMessage};
use crate::download::thumbnail::{
    compress_thumbnail_jpeg, convert_webp_to_jpeg, detect_image_format, generate_thumbnail_from_video, ImageFormat,
};
use crate::telegram::Bot;
use rand::Rng;
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use teloxide::prelude::*;
use teloxide::types::{InputFile, ParseMode};
use teloxide::RequestError;
use tokio::fs::File as TokioFile;
use tokio::io::{AsyncRead, ReadBuf};

const DEFAULT_BOT_API_LOG_PATH: &str = "bot-api-data/logs/telegram-bot-api.log";
const DEFAULT_BOT_API_LOG_TAIL_BYTES: u64 = 4 * 1024 * 1024;

/// Reads the tail of a log file up to `max_bytes`.
pub(crate) fn read_log_tail(path: &PathBuf, max_bytes: u64) -> Result<String, std::io::Error> {
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    if len > max_bytes {
        file.seek(SeekFrom::End(-(max_bytes as i64)))?;
    } else {
        file.seek(SeekFrom::Start(0))?;
    }
    let mut buf = String::new();
    file.read_to_string(&mut buf)?;
    Ok(buf)
}

/// Logs Bot API upload speed by parsing the local Bot API server logs.
///
/// This function is only useful when running with a local Bot API server.
/// It parses the log file to extract upload timing information and logs
/// the calculated upload speed.
fn log_bot_api_speed_for_file(download_path: &str) {
    let bot_api_url = match config::bot_api::local_url() {
        Some(url) => url,
        None => return,
    };

    let file_name = match Path::new(download_path).file_name().and_then(|name| name.to_str()) {
        Some(name) => name.to_string(),
        None => return,
    };

    let log_path = std::env::var("BOT_API_LOG_PATH").unwrap_or_else(|_| DEFAULT_BOT_API_LOG_PATH.to_string());
    let log_path = PathBuf::from(log_path);
    let tail_bytes = std::env::var("BOT_API_LOG_TAIL_BYTES")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_BOT_API_LOG_TAIL_BYTES);

    let content = match read_log_tail(&log_path, tail_bytes) {
        Ok(data) => data,
        Err(e) => {
            log::warn!("Local Bot API log read failed: {} ({})", log_path.display(), e);
            return;
        }
    };

    // Use pre-compiled lazy regexes from crate::core
    let start_re = &*BOT_API_START_REGEX;
    let response_re = &*BOT_API_RESPONSE_REGEX;

    #[derive(Clone)]
    struct Entry {
        method: String,
        name: String,
        size: u64,
        start_time: f64,
        response_time: Option<f64>,
    }

    let mut entries: HashMap<String, Entry> = HashMap::new();
    for line in content.lines() {
        if let Some(caps) = start_re.captures(line) {
            let time = caps.get(1).and_then(|v| v.as_str().parse::<f64>().ok());
            let query_id = caps.get(2).map(|v| v.as_str().to_string());
            let method = caps.get(3).map(|v| v.as_str().to_string());
            let name = caps.get(4).map(|v| v.as_str().to_string());
            let size = caps.get(5).and_then(|v| v.as_str().parse::<u64>().ok());

            if let (Some(time), Some(query_id), Some(method), Some(name), Some(size)) =
                (time, query_id, method, name, size)
            {
                entries.insert(
                    query_id,
                    Entry {
                        method,
                        name,
                        size,
                        start_time: time,
                        response_time: None,
                    },
                );
            }
        }

        if let Some(caps) = response_re.captures(line) {
            let time = caps.get(1).and_then(|v| v.as_str().parse::<f64>().ok());
            let query_id = caps.get(2).map(|v| v.as_str().to_string());
            if let (Some(time), Some(query_id)) = (time, query_id) {
                if let Some(entry) = entries.get_mut(&query_id) {
                    entry.response_time = Some(time);
                }
            }
        }
    }

    let mut best: Option<Entry> = None;
    for entry in entries.values() {
        if entry.name != file_name {
            continue;
        }
        if entry.response_time.is_none() {
            continue;
        }
        let replace = match &best {
            Some(current) => entry.response_time.unwrap_or(0.0) > current.response_time.unwrap_or(0.0),
            None => true,
        };
        if replace {
            best = Some(entry.clone());
        }
    }

    if let Some(entry) = best {
        if let Some(response_time) = entry.response_time {
            let duration = response_time - entry.start_time;
            if duration > 0.0 {
                let size_mb = entry.size as f64 / (1024.0 * 1024.0);
                let speed_mbs = size_mb / duration;
                log::info!(
                    "Local Bot API speed: method={}, file={}, size={:.1} MB, duration={:.1}s, speed={:.2} MB/s, api_url={}",
                    entry.method,
                    entry.name,
                    size_mb,
                    duration,
                    speed_mbs,
                    bot_api_url
                );
            }
        }
    }
}

/// Tracks the number of bytes sent during an upload operation.
///
/// This struct uses atomic operations for thread-safe progress tracking,
/// allowing the upload progress to be monitored from a separate task.
#[derive(Clone)]
pub struct UploadProgress {
    bytes_sent: Arc<AtomicU64>,
}

impl UploadProgress {
    /// Creates a new `UploadProgress` instance with zero bytes sent.
    pub fn new() -> Self {
        Self {
            bytes_sent: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Adds the specified number of bytes to the total bytes sent counter.
    pub fn add_bytes(&self, bytes: usize) {
        self.bytes_sent.fetch_add(bytes as u64, Ordering::Relaxed);
    }

    /// Returns the total number of bytes sent so far.
    pub fn bytes_sent(&self) -> u64 {
        self.bytes_sent.load(Ordering::Relaxed)
    }
}

impl Default for UploadProgress {
    fn default() -> Self {
        Self::new()
    }
}

/// An `AsyncRead` wrapper that tracks read progress.
///
/// This struct wraps any `AsyncRead` implementation and reports the number
/// of bytes read to an `UploadProgress` instance.
pub struct ProgressReader<R> {
    inner: R,
    progress: UploadProgress,
}

impl<R> ProgressReader<R> {
    /// Creates a new `ProgressReader` wrapping the given reader.
    pub fn new(inner: R, progress: UploadProgress) -> Self {
        Self { inner, progress }
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for ProgressReader<R> {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let poll = Pin::new(&mut self.inner).poll_read(cx, buf);
        if let Poll::Ready(Ok(())) = &poll {
            let after = buf.filled().len();
            if after > before {
                self.progress.add_bytes(after - before);
            }
        }
        poll
    }
}

/// Creates an `InputFile` with progress tracking for Telegram uploads.
///
/// # Arguments
///
/// * `path` - Path to the file to upload
/// * `progress` - Progress tracker to update during upload
///
/// # Returns
///
/// Returns an `InputFile` configured with the progress-tracking reader,
/// or a `RequestError` if the file cannot be opened.
pub async fn input_file_with_progress(path: &str, progress: UploadProgress) -> Result<InputFile, RequestError> {
    log::info!("Upload wrapper: opening file for upload: {}", path);
    let file = TokioFile::open(path)
        .await
        .map_err(|err| RequestError::Io(Arc::new(err)))?;
    let reader = ProgressReader::new(file, progress);
    let file_name = Path::new(path).file_name().and_then(|name| name.to_str());
    let mut input_file = InputFile::read(reader);
    if let Some(name) = file_name {
        log::info!("Upload wrapper: using file name {}", name);
        input_file = input_file.file_name(name.to_string());
    }
    Ok(input_file)
}

/// Sends an error sticker to the user.
///
/// This function sends a random sticker from a predefined set along with
/// a default error message.
pub async fn send_error_with_sticker(bot: &Bot, chat_id: ChatId) {
    send_error_with_sticker_and_message(bot, chat_id, None).await;
}

/// Sends an error sticker with an optional custom message to the user.
///
/// # Arguments
///
/// * `bot` - The Telegram bot instance
/// * `chat_id` - The chat ID to send the sticker to
/// * `custom_message` - Optional custom error message (uses default if None)
pub async fn send_error_with_sticker_and_message(bot: &Bot, chat_id: ChatId, custom_message: Option<&str>) {
    // List of sticker file_ids from doraduradoradura sticker pack
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

    // Generate random index using proper random number generator
    let random_index = rand::thread_rng().gen_range(0..sticker_file_ids.len());
    let random_sticker_id = sticker_file_ids[random_index];

    // Send random sticker
    if let Err(e) = bot
        .send_sticker(
            chat_id,
            InputFile::file_id(teloxide::types::FileId(random_sticker_id.to_string())),
        )
        .await
    {
        log::error!("Failed to send error sticker: {}", e);
    }

    // Send error message
    let error_text = custom_message.unwrap_or("У меня не получилось, все сломалось. Я написала Стэну");
    if let Err(e) = bot.send_message(chat_id, error_text).await {
        log::error!("Failed to send error message: {}", e);
    }
}

/// Generic function to send files with retry logic and animation.
///
/// This function handles the complexity of sending files to Telegram with:
/// - Progress tracking and UI updates
/// - Automatic retry on transient failures
/// - Rate limit handling with backoff
/// - Timeout handling for large files
///
/// # Arguments
///
/// * `bot` - Telegram bot instance
/// * `chat_id` - User's chat ID
/// * `download_path` - Path to file to send
/// * `progress_msg` - Progress message handler for UI updates
/// * `title` - File title for display
/// * `file_type` - Type of file ("audio" or "video")
/// * `send_fn` - Closure that performs the actual send operation
///
/// # Type Parameters
///
/// * `F` - Closure type that takes (Bot, ChatId, String, UploadProgress) and returns Fut
/// * `Fut` - Future type that resolves to ResponseResult<Message>
///
/// # Returns
///
/// Returns a tuple of (Message, file_size) on success, or an AppError on failure.
pub async fn send_file_with_retry<F, Fut>(
    bot: &Bot,
    chat_id: ChatId,
    download_path: &str,
    progress_msg: &mut ProgressMessage,
    title: &str,
    file_type: &str,
    send_fn: F,
) -> Result<(Message, u64), AppError>
where
    F: Fn(Bot, ChatId, String, UploadProgress) -> Fut,
    Fut: std::future::Future<Output = ResponseResult<Message>>,
{
    let max_attempts = config::retry::MAX_ATTEMPTS;
    let download_path = download_path.to_string();
    let mut timeout_retry_used = false;

    // Validate file size before sending
    let file_size = fs::metadata(&download_path)
        .map_err(|e| AppError::Download(format!("Failed to get file metadata: {}", e)))?
        .len();

    let max_size = match file_type {
        "audio" => config::validation::max_audio_size_bytes(),
        "video" => config::validation::max_video_size_bytes(),
        _ => config::validation::MAX_FILE_SIZE_BYTES,
    };

    if file_size > max_size {
        let size_mb = file_size as f64 / (1024.0 * 1024.0);
        let max_mb = max_size as f64 / (1024.0 * 1024.0);
        log::warn!(
            "File {} too large: {:.2} MB (max: {:.2} MB)",
            download_path,
            size_mb,
            max_mb
        );
        return Err(AppError::Validation(format!(
            "Файл слишком большой ({:.2} MB). Максимальный размер: {:.2} MB",
            size_mb, max_mb
        )));
    }

    log::info!(
        "Preparing upload for {}: file_size={} bytes, max_size={} bytes, path={}",
        file_type,
        file_size,
        max_size,
        download_path
    );

    // Send chat action "Uploading document..." before sending file
    use teloxide::types::ChatAction;
    if let Err(e) = bot.send_chat_action(chat_id, ChatAction::UploadDocument).await {
        log::warn!("Failed to send chat action: {}", e);
        // Not critical, continue with file upload
    }

    for attempt in 1..=max_attempts {
        log::info!(
            "Attempting to send {} to chat {} (attempt {}/{})",
            file_type,
            chat_id,
            attempt,
            max_attempts
        );

        // Start progress animation in a separate task
        let bot_clone = bot.clone();
        let title_clone = title.to_string();
        let mut msg_clone = ProgressMessage {
            chat_id: progress_msg.chat_id,
            message_id: progress_msg.message_id,
        };

        let file_size_clone = file_size;
        let file_type_clone = file_type.to_string();
        let upload_start = std::time::Instant::now();
        let bot_for_action = bot.clone();
        let upload_progress = UploadProgress::new();
        let upload_progress_clone = upload_progress.clone();
        let progress_handle = tokio::spawn(async move {
            let mut update_count = 0u32;
            let mut last_progress = 0u8;
            let mut last_eta = Option::<u64>::None;
            let mut consecutive_99_updates = 0u32;
            let mut last_action_time = std::time::Instant::now();
            let mut logged_complete = false;

            loop {
                let elapsed = upload_start.elapsed();
                let elapsed_secs = elapsed.as_secs();
                let elapsed_secs_f64 = elapsed.as_secs_f64();

                // Send ChatAction every 4 seconds to maintain "uploading" status
                // Telegram shows ChatAction only for 5 seconds, so we need to repeat
                if last_action_time.elapsed().as_secs() >= 4 {
                    if let Err(e) = bot_for_action
                        .send_chat_action(chat_id, ChatAction::UploadDocument)
                        .await
                    {
                        log::debug!("Failed to send chat action during upload: {}", e);
                        // Not critical, continue
                    }
                    last_action_time = std::time::Instant::now();
                }

                let actual_uploaded = upload_progress_clone.bytes_sent();
                let (progress, eta_seconds, current_size, speed_mbs) = if actual_uploaded > 0 {
                    let progress = ((actual_uploaded as f64 / file_size_clone as f64) * 100.0) as u8;
                    let progress = progress.min(99);
                    let speed_mbs = if elapsed_secs_f64 > 0.0 {
                        Some(actual_uploaded as f64 / (1024.0 * 1024.0) / elapsed_secs_f64)
                    } else {
                        None
                    };
                    let remaining_bytes = file_size_clone.saturating_sub(actual_uploaded);
                    let eta_seconds = match speed_mbs {
                        Some(speed) if speed > 0.0 && remaining_bytes > 0 => {
                            Some((remaining_bytes as f64 / (speed * 1024.0 * 1024.0)) as u64)
                        }
                        _ => None,
                    };
                    (
                        progress,
                        eta_seconds,
                        Some(actual_uploaded.min(file_size_clone)),
                        speed_mbs,
                    )
                } else {
                    // Calculate estimated progress based on time and file size
                    // Assume average upload speed: 5-10 MB/s for large files, 10-20 MB/s for small files
                    let estimated_speed_mbps = if file_size_clone > 50 * 1024 * 1024 {
                        // For large files (>50MB) - slower
                        5.0 + (update_count as f64 * 0.1).min(5.0) // from 5 to 10 MB/s
                    } else {
                        // For small files - faster
                        10.0 + (update_count as f64 * 0.2).min(10.0) // from 10 to 20 MB/s
                    };

                    let estimated_uploaded = (estimated_speed_mbps * 1024.0 * 1024.0 * elapsed_secs as f64) as u64;
                    let progress = if estimated_uploaded >= file_size_clone {
                        99 // Maximum 99% until actual send completes
                    } else {
                        ((estimated_uploaded as f64 / file_size_clone as f64) * 100.0) as u8
                    };

                    // Calculate ETA
                    let remaining_bytes = file_size_clone.saturating_sub(estimated_uploaded);
                    let eta_seconds = if estimated_speed_mbps > 0.0 && remaining_bytes > 0 {
                        Some((remaining_bytes as f64 / (estimated_speed_mbps * 1024.0 * 1024.0)) as u64)
                    } else {
                        None
                    };

                    (
                        progress,
                        eta_seconds,
                        Some(estimated_uploaded.min(file_size_clone)),
                        None,
                    )
                };

                if actual_uploaded >= file_size_clone && !logged_complete {
                    log::info!(
                        "Upload stream finished locally: sent={} bytes, total={} bytes, elapsed={}s",
                        actual_uploaded,
                        file_size_clone,
                        elapsed_secs
                    );
                    logged_complete = true;
                }

                // Check if progress or ETA changed
                let progress_changed = progress != last_progress;
                let eta_changed = eta_seconds != last_eta;

                // If progress reached 99% and not changing - don't update as often
                if progress >= 99 {
                    consecutive_99_updates += 1;
                    // After 3 updates at 99% - update only every 5 seconds
                    if consecutive_99_updates > 3 && !progress_changed && !eta_changed {
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        continue;
                    }
                } else {
                    consecutive_99_updates = 0;
                }

                // Update UI only if progress or ETA changed, or it's the first update
                if progress_changed || eta_changed || update_count == 0 {
                    // Determine file format based on file_type
                    let file_format = match file_type_clone.as_str() {
                        "video" => Some("mp4".to_string()),
                        "audio" => Some("mp3".to_string()),
                        _ => None,
                    };

                    let _ = msg_clone
                        .update(
                            &bot_clone,
                            DownloadStatus::Uploading {
                                title: title_clone.clone(),
                                dots: 0,                          // Don't use dots, use progress
                                progress: Some(progress.min(99)), // Don't show 100% until complete
                                speed_mbs,
                                eta_seconds,
                                current_size,
                                total_size: Some(file_size_clone),
                                file_format,
                            },
                        )
                        .await;

                    log::info!(
                        "Upload status: progress={}%, sent={:?}, total={} bytes, speed_mbs={:?}, eta={:?}s, elapsed={}s",
                        progress.min(99),
                        current_size,
                        file_size_clone,
                        speed_mbs,
                        eta_seconds,
                        elapsed_secs
                    );

                    last_progress = progress;
                    last_eta = eta_seconds;
                }

                update_count += 1;

                // If too much time passed, slow down updates
                if elapsed_secs > 600 {
                    // Wait longer before next update
                    tokio::time::sleep(Duration::from_secs(5)).await;
                } else if progress >= 99 && consecutive_99_updates > 3 {
                    // If progress is 99% and there were several updates - update less often
                    tokio::time::sleep(Duration::from_secs(2)).await;
                } else {
                    tokio::time::sleep(config::animation::update_interval()).await;
                }
            }
        });

        // Log request details for debugging (especially for local Bot API)
        let is_local_api = std::env::var("BOT_API_URL").is_ok();
        if is_local_api {
            log::info!(
                "[LOCAL API] Starting Telegram upload request: type={}, attempt={}, chat_id={}, file_size={}MB, path={}",
                file_type,
                attempt,
                chat_id,
                file_size / (1024 * 1024),
                download_path
            );
        } else {
            log::info!(
                "Starting Telegram upload request: type={}, attempt={}, path={}",
                file_type,
                attempt,
                download_path
            );
        }
        let request_start = std::time::Instant::now();
        let response = send_fn(bot.clone(), chat_id, download_path.clone(), upload_progress).await;
        log_bot_api_speed_for_file(&download_path);

        // Detailed logging for local API
        if is_local_api {
            log::info!(
                "[LOCAL API] Telegram upload request finished: type={}, attempt={}, chat_id={}, elapsed={}s, result={}",
                file_type,
                attempt,
                chat_id,
                request_start.elapsed().as_secs(),
                if response.is_ok() { "ok" } else { "err" }
            );
        } else {
            log::info!(
                "Telegram upload request finished: type={}, attempt={}, elapsed={}s, result={}",
                file_type,
                attempt,
                request_start.elapsed().as_secs(),
                if response.is_ok() { "ok" } else { "err" }
            );
        }

        // Stop progress tracking
        progress_handle.abort();
        log::info!(
            "Upload progress tracker stopped: type={}, attempt={}",
            file_type,
            attempt
        );

        // Small delay to ensure animation has stopped
        tokio::time::sleep(config::animation::stop_delay()).await;

        match response {
            Ok(msg) => {
                log::info!(
                    "Successfully sent {} to chat {} on attempt {}",
                    file_type,
                    chat_id,
                    attempt
                );

                // Clear progress message to remove remaining "99%" progress
                // This is important because background task may have left message in Uploading state
                // Will be updated to Success/Completed in the main function
                log::debug!("File sent successfully, progress message will be updated by caller");

                return Ok((msg, file_size));
            }
            Err(e) if attempt < max_attempts => {
                let error_str = e.to_string();

                // Check rate limiting
                if let Some(retry_after_secs) = extract_retry_after(&error_str) {
                    log::warn!(
                        "Rate limit hit when sending {} to chat {}: Retry after {}s. Waiting...",
                        file_type,
                        chat_id,
                        retry_after_secs
                    );
                    // Wait specified time + small delay for reliability
                    tokio::time::sleep(Duration::from_secs(retry_after_secs + 1)).await;
                    // Continue loop for retry
                    continue;
                }

                // Check if this is a timeout error
                // If it's timeout or network error, file may already be sent
                let is_timeout_or_network = is_timeout_or_network_error(&error_str);

                if is_timeout_or_network {
                    // For large files (>50MB) don't retry on first timeout,
                    // as file is likely already uploaded and being processed.
                    // Telegram may process large videos for 5-15 minutes after upload.
                    if file_size > 50 * 1024 * 1024 && attempt == 1 {
                        if is_local_api {
                            log::warn!(
                                "[LOCAL API] Attempt {}/{} failed for chat {} with timeout for large file ({}MB). File is likely uploaded and processing server-side. PREVENTING RETRY to avoid duplicates. Error: {}",
                                attempt,
                                max_attempts,
                                chat_id,
                                file_size / (1024 * 1024),
                                e
                            );
                        } else {
                            log::warn!(
                                "Attempt {}/{} failed for chat {} with timeout for large file ({}MB): {}. File is likely uploaded and processing server-side. Sending notification to user.",
                                attempt,
                                max_attempts,
                                chat_id,
                                file_size / (1024 * 1024),
                                e
                            );
                        }
                        metrics::record_error("telegram_api", "send_file_timeout");

                        // Send notification to user
                        let notification_msg = match file_type {
                            "video" => "Видео успешно загружено на сервер Telegram и обрабатывается.\n\nОно появится в чате через несколько минут.\n\nОбработка больших файлов может занять до 10-15 минут.",
                            _ => "File uploaded to Telegram and is being processed. It will appear in chat shortly.",
                        };

                        // Send notification and return it as "successful" message
                        match bot.send_message(chat_id, notification_msg).await {
                            Ok(sent_msg) => {
                                log::info!("Sent processing notification to user for chat {}", chat_id);
                                return Ok((sent_msg, file_size));
                            }
                            Err(send_err) => {
                                log::error!("Failed to send processing notification: {}", send_err);
                                // Even if notification failed, don't retry file upload
                                return Err(AppError::Download(format!(
                                    "File uploaded but processing notification failed: {}",
                                    send_err
                                )));
                            }
                        }
                    }

                    if timeout_retry_used {
                        log::warn!(
                            "Attempt {}/{} failed for chat {} with timeout/network error after retry: {}. Skipping further retries to avoid duplicates.",
                            attempt,
                            max_attempts,
                            chat_id,
                            e
                        );
                        metrics::record_error("telegram_api", "send_file");
                        let error_msg = match file_type {
                            "video" => format!(
                                "У меня не получилось отправить тебе видео, попробуй как-нибудь позже. Ошибка: {}",
                                e
                            ),
                            _ => format!("Failed to send {} file after timeout/network retry: {}", file_type, e),
                        };
                        return Err(AppError::Download(error_msg));
                    }

                    log::warn!(
                        "Attempt {}/{} failed for chat {} with timeout/network error: {}. This may indicate the file was actually sent but response timed out. Will retry once more to confirm.",
                        attempt,
                        max_attempts,
                        chat_id,
                        e
                    );
                    timeout_retry_used = true;
                    // For timeout/network errors use longer delay
                    tokio::time::sleep(Duration::from_secs(5)).await;
                } else {
                    log::warn!(
                        "Attempt {}/{} failed for chat {}: {}. Retrying...",
                        attempt,
                        max_attempts,
                        chat_id,
                        e
                    );
                    tokio::time::sleep(config::retry::delay()).await;
                }
            }
            Err(e) => {
                log::error!(
                    "All {} attempts failed to send {} to chat {}: {}",
                    max_attempts,
                    file_type,
                    chat_id,
                    e
                );

                // Record telegram error metric
                metrics::record_error("telegram_api", "send_file");

                let error_msg = match file_type {
                    "video" => format!("У меня не получилось отправить тебе видео, попробуй как-нибудь позже. Все {} попытки не удались: {}", max_attempts, e),
                    _ => format!("Failed to send {} file after {} attempts: {}", file_type, max_attempts, e),
                };
                return Err(AppError::Download(error_msg));
            }
        }
    }

    unreachable!()
}

/// Send audio file with retry logic.
///
/// # Arguments
///
/// * `bot` - Telegram bot instance
/// * `chat_id` - User's chat ID
/// * `download_path` - Path to audio file
/// * `duration` - Audio duration in seconds
/// * `progress_msg` - Progress message handler
/// * `caption` - Formatted caption with MarkdownV2
/// * `send_as_document` - If true, send as document instead of audio
///
/// # Returns
///
/// Returns a tuple of (Message, file_size) on success, or an AppError on failure.
#[allow(dead_code)]
pub async fn send_audio_with_retry(
    bot: &Bot,
    chat_id: ChatId,
    download_path: &str,
    duration: u32,
    progress_msg: &mut ProgressMessage,
    caption: &str,
    send_as_document: bool,
) -> Result<(Message, u64), AppError> {
    if send_as_document {
        log::info!("User preference: sending audio as document");
        let caption_clone = caption.to_string();
        send_file_with_retry(
            bot,
            chat_id,
            download_path,
            progress_msg,
            "",
            "audio",
            move |bot, chat_id, path, progress| {
                let caption_clone = caption_clone.clone();
                async move {
                    let input_file = input_file_with_progress(&path, progress).await?;
                    bot.send_document(chat_id, input_file)
                        .caption(&caption_clone)
                        .parse_mode(ParseMode::MarkdownV2)
                        .await
                }
            },
        )
        .await
    } else {
        let caption_clone = caption.to_string();
        send_file_with_retry(
            bot,
            chat_id,
            download_path,
            progress_msg,
            "",
            "audio",
            move |bot, chat_id, path, progress| {
                let duration = duration;
                let caption_clone = caption_clone.clone();
                async move {
                    let input_file = input_file_with_progress(&path, progress).await?;
                    bot.send_audio(chat_id, input_file)
                        .caption(&caption_clone)
                        .parse_mode(ParseMode::MarkdownV2)
                        .duration(duration)
                        .await
                }
            },
        )
        .await
    }
}

/// Send video file with retry logic and fallback to send_document for large files.
///
/// # Arguments
///
/// * `bot` - Telegram bot instance
/// * `chat_id` - User's chat ID
/// * `download_path` - Path to video file
/// * `progress_msg` - Progress message handler
/// * `title` - Video title
/// * `thumbnail_url` - Optional URL for video thumbnail
/// * `send_as_document` - If true, send as document instead of video
///
/// # Behavior
///
/// - Tries to send as video (send_video) with metadata
/// - If file > 50 MB and send_video fails, falls back to send_document
/// - Uses send_file_with_retry for retry logic
/// - Optionally includes thumbnail preview image
///
/// # Returns
///
/// Returns a tuple of (Message, file_size) on success, or an AppError on failure.
pub async fn send_video_with_retry(
    bot: &Bot,
    chat_id: ChatId,
    download_path: &str,
    progress_msg: &mut ProgressMessage,
    title: &str,
    thumbnail_url: Option<&str>,
    send_as_document: bool,
) -> Result<(Message, u64), AppError> {
    // Get video metadata for correct Telegram sending
    let video_metadata = probe_video_metadata(download_path);

    log::info!("Video metadata for {}: {:?}", download_path, video_metadata);

    let duration = video_metadata.map(|(d, _, _)| d);
    let width = video_metadata.and_then(|(_, w, _)| w);
    let height = video_metadata.and_then(|(_, _, h)| h);

    // Check file size
    let file_size = fs::metadata(download_path)
        .map_err(|e| AppError::Download(format!("Failed to get file metadata: {}", e)))?
        .len();

    let standard_limit = 50 * 1024 * 1024; // 50 MB - standard limit for send_video
    let use_document_fallback = file_size > standard_limit || send_as_document;

    if send_as_document {
        log::info!("User preference: sending video as document");
    } else if use_document_fallback {
        log::info!(
            "File size ({:.2} MB) exceeds standard send_video limit (50 MB), will use send_document fallback",
            file_size as f64 / (1024.0 * 1024.0)
        );
    }

    // Download thumbnail if available, otherwise generate from video
    let thumbnail_bytes = if let Some(thumb_url) = thumbnail_url {
        log::info!("[THUMBNAIL] Starting thumbnail download from URL: {}", thumb_url);
        match reqwest::get(thumb_url).await {
            Ok(response) => {
                log::info!("[THUMBNAIL] Thumbnail HTTP response status: {}", response.status());

                // Check Content-Type
                if let Some(content_type) = response.headers().get("content-type") {
                    let content_type_str = content_type.to_str().unwrap_or("unknown");
                    log::info!("[THUMBNAIL] Thumbnail Content-Type: {}", content_type_str);
                }

                if response.status().is_success() {
                    match response.bytes().await {
                        Ok(bytes) => {
                            let bytes_vec = bytes.to_vec();
                            log::info!(
                                "[THUMBNAIL] Successfully downloaded thumbnail: {} bytes ({} KB)",
                                bytes_vec.len(),
                                bytes_vec.len() as f64 / 1024.0
                            );

                            // Check file format by magic bytes
                            let format = detect_image_format(&bytes_vec);
                            log::info!("[THUMBNAIL] Detected image format: {:?}", format);

                            // Check size (Telegram requires <= 200 KB)
                            if bytes_vec.len() > 200 * 1024 {
                                log::warn!("[THUMBNAIL] Thumbnail size ({} KB) exceeds Telegram limit (200 KB). May cause issues.",
                                    bytes_vec.len() as f64 / 1024.0);
                            }

                            // Check format (Telegram requires JPEG or PNG)
                            match format {
                                ImageFormat::Jpeg | ImageFormat::Png => {
                                    log::info!("[THUMBNAIL] Thumbnail format is valid (JPEG/PNG), will use it");
                                    Some(bytes_vec)
                                }
                                ImageFormat::WebP => {
                                    log::warn!("[THUMBNAIL] Thumbnail is WebP format, Telegram may not support it properly. Trying anyway...");
                                    Some(bytes_vec)
                                }
                                ImageFormat::Unknown => {
                                    log::warn!("[THUMBNAIL] Unknown thumbnail format, may cause black screen. First bytes: {:?}",
                                        bytes_vec.iter().take(10).collect::<Vec<_>>());
                                    Some(bytes_vec)
                                }
                            }
                        }
                        Err(e) => {
                            log::warn!("[THUMBNAIL] Failed to get thumbnail bytes: {}", e);
                            None
                        }
                    }
                } else {
                    log::warn!(
                        "[THUMBNAIL] Thumbnail request failed with status: {}",
                        response.status()
                    );
                    None
                }
            }
            Err(e) => {
                log::warn!("[THUMBNAIL] Failed to download thumbnail: {}", e);
                None
            }
        }
    } else {
        log::info!("[THUMBNAIL] No thumbnail URL provided");
        None
    };

    // If thumbnail from URL is not available, generate from video
    let thumbnail_bytes = thumbnail_bytes.or_else(|| {
        log::info!("[THUMBNAIL] Thumbnail URL not available, trying to generate from video file");
        generate_thumbnail_from_video(download_path)
    });

    // Create temporary file for thumbnail if available
    // This is needed for proper thumbnail transmission to Telegram with file name
    // Convert WebP to JPEG if needed, as Telegram works better with JPEG
    let temp_thumb_path: Option<std::path::PathBuf> = if let Some(ref thumb_bytes) = thumbnail_bytes {
        let format = detect_image_format(thumb_bytes);

        // Convert WebP to JPEG if needed (Telegram works better with JPEG)
        let (final_bytes, file_ext) = if format == ImageFormat::WebP {
            log::info!("[THUMBNAIL] Converting WebP thumbnail to JPEG for better Telegram compatibility");
            // Try to use ffmpeg to convert WebP to JPEG
            match convert_webp_to_jpeg(thumb_bytes) {
                Ok(jpeg_bytes) => {
                    log::info!(
                        "[THUMBNAIL] Successfully converted WebP to JPEG: {} bytes",
                        jpeg_bytes.len()
                    );
                    (jpeg_bytes, "jpg")
                }
                Err(e) => {
                    log::warn!("[THUMBNAIL] Failed to convert WebP to JPEG: {}. Using original.", e);
                    (thumb_bytes.clone(), "webp")
                }
            }
        } else {
            let ext = match format {
                ImageFormat::Jpeg => "jpg",
                ImageFormat::Png => "png",
                ImageFormat::Unknown => "jpg",
                _ => "jpg",
            };
            (thumb_bytes.clone(), ext)
        };

        // Check size - if larger than 200KB, compress
        let final_bytes = if final_bytes.len() > 200 * 1024 {
            log::warn!(
                "[THUMBNAIL] Thumbnail too large ({} KB), trying to compress",
                final_bytes.len() as f64 / 1024.0
            );
            compress_thumbnail_jpeg(&final_bytes).unwrap_or(final_bytes)
        } else {
            final_bytes
        };

        let temp_dir = std::env::temp_dir();
        let temp_path = temp_dir.join(format!(
            "thumb_{}.{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis(),
            file_ext
        ));

        // Get absolute path (canonicalize only works for existing files)
        let abs_path = if temp_path.exists() {
            temp_path.canonicalize().unwrap_or_else(|_| temp_path.clone())
        } else {
            // If file not created yet, get absolute path through parent
            temp_dir
                .canonicalize()
                .map(|canon_dir| canon_dir.join(temp_path.file_name().unwrap_or_default()))
                .unwrap_or_else(|_| temp_path.clone())
        };

        if fs::write(&abs_path, &final_bytes).is_ok() {
            log::info!(
                "[THUMBNAIL] Saved thumbnail to temporary file: {:?} ({} bytes)",
                abs_path,
                final_bytes.len()
            );
            Some(abs_path)
        } else {
            log::warn!("[THUMBNAIL] Failed to save thumbnail to temporary file");
            None
        }
    } else {
        None
    };

    // Clone values for use in closure
    let duration_clone = duration;
    // If user chose to send as document, send as document immediately
    if send_as_document {
        log::info!("User preference: sending video as document (skip send_video)");
        let title_for_doc = title.to_string();
        return send_file_with_retry(
            bot,
            chat_id,
            download_path,
            progress_msg,
            title,
            "video",
            move |bot, chat_id, path, progress| {
                let title_for_doc = title_for_doc.clone();
                async move {
                    let input_file = input_file_with_progress(&path, progress).await?;
                    bot.send_document(chat_id, input_file)
                        .caption(&title_for_doc)
                        .parse_mode(ParseMode::MarkdownV2)
                        .await
                }
            },
        )
        .await;
    }

    let width_clone = width;
    let height_clone = height;
    let thumbnail_bytes_clone = thumbnail_bytes.clone();
    let temp_thumb_path_clone = temp_thumb_path.clone();
    let title_clone = title.to_string();

    // Try to send as video
    let result = send_file_with_retry(
        bot,
        chat_id,
        download_path,
        progress_msg,
        title,
        "video",
        move |bot, chat_id, path, progress| {
            let duration_clone = duration_clone;
            let width_clone = width_clone;
            let height_clone = height_clone;
            let thumbnail_bytes_clone = thumbnail_bytes_clone.clone();
            let temp_thumb_path_clone = temp_thumb_path_clone.clone();
            let title_clone = title_clone.clone();

            async move {
                let input_file = input_file_with_progress(&path, progress).await?;
                let mut video_msg = bot
                    .send_video(chat_id, input_file)
                    .caption(&title_clone)
                    .parse_mode(ParseMode::MarkdownV2);

                // Add metadata for correct Telegram playback
                if let Some(dur) = duration_clone {
                    video_msg = video_msg.duration(dur);
                }
                if let Some(w) = width_clone {
                    video_msg = video_msg.width(w);
                }
                if let Some(h) = height_clone {
                    video_msg = video_msg.height(h);
                }

                // Add thumbnail if available
                // IMPORTANT: Use absolute path and ensure file exists
                if let Some(thumb_path) = temp_thumb_path_clone {
                    // Check that file exists before sending
                    if thumb_path.exists() {
                        let abs_path_str = thumb_path.to_str().unwrap_or("thumb.jpg");
                        log::info!(
                            "[THUMBNAIL] Adding thumbnail from file: {} (exists: {}, size: {} bytes)",
                            abs_path_str,
                            thumb_path.exists(),
                            fs::metadata(&thumb_path).map(|m| m.len()).unwrap_or(0)
                        );
                        video_msg = video_msg.thumbnail(InputFile::file(abs_path_str));
                        log::info!("[THUMBNAIL] Thumbnail successfully added to video message");
                    } else {
                        log::warn!(
                            "[THUMBNAIL] Thumbnail file does not exist: {:?}, trying memory fallback",
                            thumb_path
                        );
                        // Fallback to memory if file doesn't exist
                        if let Some(thumb_bytes) = thumbnail_bytes_clone {
                            log::info!("[THUMBNAIL] Adding thumbnail from memory: {} bytes", thumb_bytes.len());
                            video_msg = video_msg.thumbnail(InputFile::memory(thumb_bytes));
                        }
                    }
                } else if let Some(thumb_bytes) = thumbnail_bytes_clone {
                    log::info!("[THUMBNAIL] Adding thumbnail from memory: {} bytes", thumb_bytes.len());
                    // Fallback to InputFile::memory if temporary file not created
                    video_msg = video_msg.thumbnail(InputFile::memory(thumb_bytes));
                    log::info!("[THUMBNAIL] Thumbnail successfully added to video message");
                } else {
                    log::info!("[THUMBNAIL] No thumbnail bytes available, sending video without thumbnail");
                }

                // Enable streaming support for better compatibility
                video_msg = video_msg.supports_streaming(true);

                video_msg.await
            }
        },
    )
    .await;

    // Delete temporary thumbnail file after successful send
    // Add small delay so teloxide has time to read the file
    if let Some(thumb_path) = temp_thumb_path {
        // Give teloxide time to read file before deleting
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        if result.is_ok() {
            let _ = fs::remove_file(&thumb_path);
            log::info!("[THUMBNAIL] Cleaned up temporary thumbnail file: {:?}", thumb_path);
        } else {
            // On error also delete, as retry will create new file
            let _ = fs::remove_file(&thumb_path);
            log::info!(
                "[THUMBNAIL] Cleaned up temporary thumbnail file after error: {:?}",
                thumb_path
            );
        }
    }

    // If sending as video failed and file > 50 MB, try as document
    if result.is_err() && use_document_fallback {
        if let Err(AppError::Download(ref msg)) = result {
            if is_timeout_or_network_error(msg) {
                log::warn!(
                    "send_video failed with timeout/network error; skipping send_document fallback to avoid duplicates"
                );
                return result;
            }
        }

        log::info!("send_video failed, trying send_document as fallback for large file");
        let title_for_fallback = title.to_string();
        return send_file_with_retry(
            bot,
            chat_id,
            download_path,
            progress_msg,
            title,
            "video",
            move |bot, chat_id, path, progress| {
                let title_for_fallback = title_for_fallback.clone();
                async move {
                    let input_file = input_file_with_progress(&path, progress).await?;
                    bot.send_document(chat_id, input_file)
                        .caption(&title_for_fallback)
                        .parse_mode(ParseMode::MarkdownV2)
                        .await
                }
            },
        )
        .await;
    }

    result
}
