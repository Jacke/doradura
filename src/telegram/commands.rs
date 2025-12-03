use crate::core::config;
use crate::core::error::AppError;
use crate::core::rate_limiter::RateLimiter;
use crate::core::utils::pluralize_seconds;
use crate::download::queue::DownloadQueue;
use crate::storage::db::{self, DbPool};
use crate::telegram::preview::{get_preview_metadata, send_preview};
use once_cell::sync::Lazy;
use regex::Regex;
use std::sync::Arc;
use teloxide::prelude::*;
use url::Url;

/// Cached regex for matching URLs
/// Compiled once at startup and reused for all requests
static URL_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"https?://[^\s]+").expect("Failed to compile URL regex"));

/// Handle rate limiting for a user message
///
/// Checks if the user is rate-limited and sends an appropriate message if they are.
///
/// # Arguments
///
/// * `bot` - Telegram bot instance
/// * `msg` - Message to check rate limit for
/// * `rate_limiter` - Rate limiter instance
/// * `plan` - User's subscription plan ("free", "premium", "vip")
///
/// # Returns
///
/// Returns `Ok(true)` if the user is not rate-limited, `Ok(false)` if they are.
///
/// # Errors
///
/// Returns `ResponseResult` error if sending a message fails.
pub async fn handle_rate_limit(
    bot: &Bot,
    msg: &Message,
    rate_limiter: &RateLimiter,
    plan: &str,
) -> ResponseResult<bool> {
    if rate_limiter.is_rate_limited(msg.chat.id, plan).await {
        if let Some(remaining_time) = rate_limiter.get_remaining_time(msg.chat.id).await {
            let remaining_seconds = remaining_time.as_secs();
            bot.send_message(msg.chat.id, format!("Я Дора, чай закончился и я не смогу скачать тебе трек сейчас. Попробуй попозже через {} {}.", remaining_seconds, pluralize_seconds(remaining_seconds))).await?;
        } else {
            bot.send_message(
                msg.chat.id,
                "Я Дора, чай закончился и я не смогу скачать тебе трек сейчас. Попробуй попозже.",
            )
            .await?;
        }
        return Ok(false);
    }
    rate_limiter.update_rate_limit(msg.chat.id, plan).await;
    Ok(true)
}

/// Handle incoming message and process download requests
///
/// Parses URLs from messages, validates them, checks rate limits, and adds tasks to the download queue.
///
/// # Arguments
///
/// * `bot` - Telegram bot instance
/// * `msg` - Incoming message
/// * `download_queue` - Download queue for adding tasks
/// * `rate_limiter` - Rate limiter instance
/// * `db_pool` - Database connection pool
///
/// # Returns
///
/// Returns `Ok(Option<User>)` on success (Some(user) if found, None otherwise) or a `ResponseResult` error.
/// The User can be reused for logging to avoid duplicate DB queries.
///
/// # Behavior
///
/// - Extracts URLs from message text using regex
/// - Validates URL length (max 2048 characters)
/// - Checks user's download format preference from database (optimized: gets full user info)
/// - Adds download task to queue if rate limit allows
/// - Sends confirmation message to user
pub async fn handle_message(
    bot: Bot,
    msg: Message,
    _download_queue: Arc<DownloadQueue>,
    rate_limiter: Arc<RateLimiter>,
    db_pool: Arc<DbPool>,
) -> ResponseResult<Option<db::User>> {
    if let Some(text) = msg.text() {
        log::debug!("handle_message: {:?}", text);
        if text.starts_with("/start") || text.starts_with("/help") {
            return Ok(None);
        }

        // Use cached regex for better performance - find all URLs
        let urls: Vec<&str> = URL_REGEX.find_iter(text).map(|m| m.as_str()).collect();

        if !urls.is_empty() {
            // Get user's preferred download format from database
            // Use get_user to get full user info (will be reused for logging)
            let (format, user_info) = match db::get_connection(&db_pool) {
                Ok(conn) => match db::get_user(&conn, msg.chat.id.0) {
                    Ok(Some(user)) => (user.download_format().to_string(), Some(user)),
                    Ok(None) => (String::from("mp3"), None),
                    Err(e) => {
                        log::warn!("Failed to get user: {}, using default mp3", e);
                        (String::from("mp3"), None)
                    }
                },
                Err(e) => {
                    log::error!(
                        "Failed to get database connection: {}, using default mp3",
                        e
                    );
                    (String::from("mp3"), None)
                }
            };

            // Check rate limit before processing URLs
            let plan = user_info
                .as_ref()
                .map(|u| u.plan.as_str())
                .unwrap_or("free");
            let plan_string = plan.to_string();
            if !handle_rate_limit(&bot, &msg, &rate_limiter, &plan_string).await? {
                return Ok(user_info);
            }

            // Process multiple URLs (group downloads)
            if urls.len() > 1 {
                // Group download mode
                let mut valid_urls = Vec::new();

                for url_text in urls {
                    // Validate URL length
                    if url_text.len() > crate::config::validation::MAX_URL_LENGTH {
                        log::warn!(
                            "URL too long: {} characters (max: {})",
                            url_text.len(),
                            crate::config::validation::MAX_URL_LENGTH
                        );
                        continue;
                    }

                    let mut url = match Url::parse(url_text) {
                        Ok(parsed_url) => parsed_url,
                        Err(e) => {
                            log::warn!("Failed to parse URL '{}': {}", url_text, e);
                            continue;
                        }
                    };

                    // Remove the &list parameter if it exists
                    if url.query_pairs().any(|(key, _)| key == "list") {
                        let mut new_query = String::new();
                        for (key, value) in url.query_pairs() {
                            if key != "list" {
                                if !new_query.is_empty() {
                                    new_query.push('&');
                                }
                                new_query.push_str(&key);
                                new_query.push('=');
                                new_query.push_str(&value);
                            }
                        }
                        url.set_query(if new_query.is_empty() {
                            None
                        } else {
                            Some(&new_query)
                        });
                    }

                    valid_urls.push(url);
                }

                if valid_urls.is_empty() {
                    bot.send_message(msg.chat.id, "Извини, я не смогла распознать ни одной корректной ссылки. Пожалуйста, пришли мне корректные ссылки на поддерживаемые сервисы (YouTube, SoundCloud, VK, TikTok, Instagram, Twitch, Spotify и другие).").await?;
                    return Ok(user_info);
                }

                // Send confirmation message
                let confirmation_msg =
                    format!("✅ Добавлено {} треков в очередь!", valid_urls.len());
                let status_message = bot.send_message(msg.chat.id, &confirmation_msg).await?;

                // Process each URL - get metadata and add to queue
                let download_queue = _download_queue.clone();
                let bot_clone = bot.clone();
                let db_pool_clone = db_pool.clone();
                let chat_id = msg.chat.id;

                tokio::spawn(async move {
                    let mut status_text = confirmation_msg.clone();
                    status_text.push_str("\n\n");

                    // Получаем connection для получения настроек пользователя
                    let conn = match db::get_connection(&db_pool_clone) {
                        Ok(c) => c,
                        Err(_) => {
                            // Если не удалось получить connection, используем дефолтные значения
                            for (idx, url) in valid_urls.iter().enumerate() {
                                match get_preview_metadata(url, Some(&format), None).await {
                                    Ok(metadata) => {
                                        let display_title = metadata.display_title();

                                        // Проверяем размер файла
                                        let status_marker =
                                            if let Some(filesize) = metadata.filesize {
                                                let max_size = if format == "mp4" {
                                                    config::validation::max_video_size_bytes()
                                                } else {
                                                    config::validation::max_audio_size_bytes()
                                                };

                                                if filesize > max_size {
                                                    "❌ Слишком большой"
                                                } else {
                                                    "⏳ В очереди"
                                                }
                                            } else {
                                                "⏳ В очереди"
                                            };

                                        status_text.push_str(&format!(
                                            "{}. {} [{}]\n",
                                            idx + 1,
                                            display_title.chars().take(50).collect::<String>(),
                                            status_marker
                                        ));
                                    }
                                    Err(_) => {
                                        status_text.push_str(&format!(
                                            "{}. {} [❌ Ошибка]\n",
                                            idx + 1,
                                            url.as_str().chars().take(50).collect::<String>()
                                        ));
                                    }
                                }
                            }
                            return;
                        }
                    };

                    for (idx, url) in valid_urls.iter().enumerate() {
                        // Get metadata for preview
                        // Получаем качество видео для превью (для групповых загрузок)
                        let video_quality_for_preview = if format == "mp4" {
                            match db::get_user_video_quality(&conn, chat_id.0) {
                                Ok(q) => Some(q),
                                Err(_) => Some("best".to_string()),
                            }
                        } else {
                            None
                        };

                        match get_preview_metadata(
                            url,
                            Some(&format),
                            video_quality_for_preview.as_deref(),
                        )
                        .await
                        {
                            Ok(metadata) => {
                                let display_title = metadata.display_title();

                                // Проверяем размер файла для групповых загрузок
                                let status_marker = if let Some(filesize) = metadata.filesize {
                                    let max_size = if format == "mp4" {
                                        config::validation::max_video_size_bytes()
                                    } else {
                                        config::validation::max_audio_size_bytes()
                                    };

                                    if filesize > max_size {
                                        "❌ Слишком большой"
                                    } else {
                                        "⏳ В очереди"
                                    }
                                } else {
                                    "⏳ В очереди"
                                };

                                status_text.push_str(&format!(
                                    "{}. {} [{}]\n",
                                    idx + 1,
                                    display_title.chars().take(50).collect::<String>(),
                                    status_marker
                                ));

                                // Пропускаем файлы, которые слишком большие - не добавляем в очередь
                                let should_skip = if let Some(filesize) = metadata.filesize {
                                    let max_size = if format == "mp4" {
                                        config::validation::max_video_size_bytes()
                                    } else {
                                        config::validation::max_audio_size_bytes()
                                    };
                                    filesize > max_size
                                } else {
                                    false
                                };

                                if should_skip {
                                    log::info!(
                                        "Skipping file {} in group download - too large",
                                        url.as_str()
                                    );
                                    continue;
                                }

                                // Add to queue using preview callback logic
                                // Get user preferences for quality/bitrate
                                let conn = match db::get_connection(&db_pool_clone) {
                                    Ok(c) => c,
                                    Err(_) => continue,
                                };

                                let video_quality = if format == "mp4" {
                                    match db::get_user_video_quality(&conn, chat_id.0) {
                                        Ok(q) => Some(q),
                                        Err(_) => Some("best".to_string()),
                                    }
                                } else {
                                    None
                                };
                                let audio_bitrate = if format == "mp3" {
                                    match db::get_user_audio_bitrate(&conn, chat_id.0) {
                                        Ok(b) => Some(b),
                                        Err(_) => Some("320k".to_string()),
                                    }
                                } else {
                                    None
                                };

                                let is_video = format == "mp4";
                                let plan_for_task = plan_string.clone();
                                let task = crate::download::queue::DownloadTask::from_plan(
                                    url.as_str().to_string(),
                                    chat_id,
                                    Some(msg.id.0),
                                    is_video,
                                    format.clone(),
                                    video_quality,
                                    audio_bitrate,
                                    &plan_for_task,
                                );
                                download_queue
                                    .add_task(task, Some(Arc::clone(&db_pool)))
                                    .await;
                            }
                            Err(e) => {
                                log::error!(
                                    "Failed to get preview metadata for URL {}: {:?}",
                                    url,
                                    e
                                );
                                status_text.push_str(&format!(
                                    "{}. {} [❌ Ошибка]\n",
                                    idx + 1,
                                    url.as_str().chars().take(50).collect::<String>()
                                ));
                            }
                        }

                        // Update status message every few URLs
                        if (idx + 1) % 5 == 0 || idx == valid_urls.len() - 1 {
                            if let Err(e) = bot_clone
                                .edit_message_text(chat_id, status_message.id, &status_text)
                                .await
                            {
                                log::warn!("Failed to update status message: {:?}", e);
                            }
                        }
                    }

                    // Final update
                    status_text.push_str("\n✅ Все треки добавлены в очередь!");
                    let _ = bot_clone
                        .edit_message_text(chat_id, status_message.id, &status_text)
                        .await;
                });

                return Ok(user_info);
            } else {
                // Single URL mode (existing behavior)
                let url_text = urls[0];

                // Validate URL length
                if url_text.len() > crate::config::validation::MAX_URL_LENGTH {
                    log::warn!(
                        "URL too long: {} characters (max: {})",
                        url_text.len(),
                        crate::config::validation::MAX_URL_LENGTH
                    );
                    bot.send_message(msg.chat.id, format!("Извини, ссылка слишком длинная (максимум {} символов). Пожалуйста, пришли более короткую ссылку.", crate::config::validation::MAX_URL_LENGTH)).await?;
                    return Ok(user_info);
                }

                let mut url = match Url::parse(url_text) {
                    Ok(parsed_url) => parsed_url,
                    Err(e) => {
                        log::warn!("Failed to parse URL '{}': {}", url_text, e);
                        bot.send_message(msg.chat.id, "Извини, я не смогла распознать ссылку. Пожалуйста, пришли мне корректную ссылку на поддерживаемые сервисы (YouTube, SoundCloud, VK, TikTok, Instagram, Twitch, Spotify и другие).").await?;
                        return Ok(user_info);
                    }
                };

                // Remove the &list parameter if it exists
                if url.query_pairs().any(|(key, _)| key == "list") {
                    let mut new_query = String::new();
                    for (key, value) in url.query_pairs() {
                        if key != "list" {
                            if !new_query.is_empty() {
                                new_query.push('&');
                            }
                            new_query.push_str(&key);
                            new_query.push('=');
                            new_query.push_str(&value);
                        }
                    }
                    url.set_query(if new_query.is_empty() {
                        None
                    } else {
                        Some(&new_query)
                    });
                }

                // Show preview instead of immediately downloading
                // Получаем качество видео для превью
                let conn_for_preview = match db::get_connection(&db_pool) {
                    Ok(c) => Ok(c),
                    Err(e) => Err(e),
                };

                let video_quality = if format == "mp4" {
                    if let Ok(ref conn) = conn_for_preview {
                        match db::get_user_video_quality(conn, msg.chat.id.0) {
                            Ok(q) => Some(q),
                            Err(_) => Some("best".to_string()),
                        }
                    } else {
                        Some("best".to_string())
                    }
                } else {
                    None
                };

                match get_preview_metadata(&url, Some(&format), video_quality.as_deref()).await {
                    Ok(metadata) => {
                        // Проверяем размер файла на этапе preview ТОЛЬКО для аудио
                        // Для видео (mp4) пропускаем проверку, чтобы пользователь мог выбрать меньшее качество в preview
                        if format != "mp4" {
                            if let Some(filesize) = metadata.filesize {
                                let max_size = config::validation::max_audio_size_bytes();

                                if filesize > max_size {
                                    let size_mb = filesize as f64 / (1024.0 * 1024.0);
                                    let max_mb = max_size as f64 / (1024.0 * 1024.0);

                                    log::warn!("Audio file too large at preview stage: {:.2} MB (max: {:.2} MB)", size_mb, max_mb);

                                    let error_message = format!(
                                        "❌ Аудио файл слишком большой (примерно {:.1} MB). Максимальный размер: {:.1} MB.",
                                        size_mb, max_mb
                                    );

                                    bot.send_message(msg.chat.id, error_message).await?;
                                    return Ok(user_info);
                                }
                            }
                        }

                        // Send preview with inline buttons
                        let default_quality = if format == "mp4" {
                            video_quality.as_deref()
                        } else {
                            None
                        };
                        match send_preview(
                            &bot,
                            msg.chat.id,
                            &url,
                            &metadata,
                            &format,
                            default_quality,
                            None,
                            Arc::clone(&db_pool),
                        )
                        .await
                        {
                            Ok(_) => {
                                log::info!("Preview sent successfully for chat {}", msg.chat.id);
                            }
                            Err(e) => {
                                log::error!("Failed to send preview: {:?}", e);
                                // Fallback: send error message
                                bot.send_message(msg.chat.id, "У меня не получилось показать превью 😢 Попробуй еще раз или напиши Стэну (@stansob).").await?;
                            }
                        }
                    }
                    Err(e) => {
                        log::error!("Failed to get preview metadata: {:?}", e);

                        // Проверяем, является ли это ошибкой длительности
                        let error_message = if let AppError::Download(ref msg) = e {
                            if msg.contains("Видео слишком длинное") {
                                msg.clone()
                            } else {
                                "У меня не получилось получить информацию о треке 😢 Попробуй еще раз или напиши Стэну (@stansob).".to_string()
                            }
                        } else {
                            "У меня не получилось получить информацию о треке 😢 Попробуй еще раз или напиши Стэну (@stansob).".to_string()
                        };

                        bot.send_message(msg.chat.id, error_message).await?;
                    }
                }

                // Return user info for reuse in logging
                return Ok(user_info);
            }
        } else {
            bot.send_message(msg.chat.id, "Извини, я не нашла ссылки. Пожалуйста, пришли мне ссылку на трек или видео с поддерживаемых сервисов (YouTube, SoundCloud, VK, TikTok, Instagram, Twitch, Spotify и другие).").await?;
        }
    } else {
        bot.send_message(msg.chat.id, "Извини, я не нашла ссылки. Пожалуйста, пришли мне ссылку на трек или видео с поддерживаемых сервисов (YouTube, SoundCloud, VK, TikTok, Instagram, Twitch, Spotify и другие).").await?;
    }
    Ok(None)
}
