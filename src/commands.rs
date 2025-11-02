use regex::Regex;
use teloxide::prelude::*;
use crate::rate_limiter::RateLimiter;
use crate::db::{self, DbPool};
use crate::utils::pluralize_seconds;
use std::sync::Arc;
use url::Url;
use crate::queue::DownloadQueue;
use crate::preview::{get_preview_metadata, send_preview};
use once_cell::sync::Lazy;

/// Cached regex for matching URLs
/// Compiled once at startup and reused for all requests
static URL_REGEX: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"https?://[^\s]+").expect("Failed to compile URL regex")
});

/// Handle rate limiting for a user message
/// 
/// Checks if the user is rate-limited and sends an appropriate message if they are.
/// 
/// # Arguments
/// 
/// * `bot` - Telegram bot instance
/// * `msg` - Message to check rate limit for
/// * `rate_limiter` - Rate limiter instance
/// 
/// # Returns
/// 
/// Returns `Ok(true)` if the user is not rate-limited, `Ok(false)` if they are.
/// 
/// # Errors
/// 
/// Returns `ResponseResult` error if sending a message fails.
pub async fn handle_rate_limit(bot: &Bot, msg: &Message, rate_limiter: &RateLimiter) -> ResponseResult<bool> {
    if rate_limiter.is_rate_limited(msg.chat.id).await {
        if let Some(remaining_time) = rate_limiter.get_remaining_time(msg.chat.id).await {
            let remaining_seconds = remaining_time.as_secs();
            bot.send_message(msg.chat.id, format!("Я Дора, чай закончился и я не смогу скачать тебе трек сейчас. Попробуй попозже через {} {}.", remaining_seconds, pluralize_seconds(remaining_seconds))).await?;
        } else {
            bot.send_message(msg.chat.id, "Я Дора, чай закончился и я не смогу скачать тебе трек сейчас. Попробуй попозже.").await?;
        }
        return Ok(false);
    }
    rate_limiter.update_rate_limit(msg.chat.id).await;
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
pub async fn handle_message(bot: Bot, msg: Message, _download_queue: Arc<DownloadQueue>, rate_limiter: Arc<RateLimiter>, db_pool: Arc<DbPool>) -> ResponseResult<Option<db::User>> {
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
                Ok(conn) => {
                    match db::get_user(&conn, msg.chat.id.0) {
                        Ok(Some(user)) => {
                            (user.download_format().to_string(), Some(user))
                        }
                        Ok(None) => {
                            (String::from("mp3"), None)
                        }
                        Err(e) => {
                            log::warn!("Failed to get user: {}, using default mp3", e);
                            (String::from("mp3"), None)
                        }
                    }
                }
                Err(e) => {
                    log::error!("Failed to get database connection: {}, using default mp3", e);
                    (String::from("mp3"), None)
                }
            };
            
            // Check rate limit before processing URLs
            if !handle_rate_limit(&bot, &msg, &rate_limiter).await? {
                return Ok(user_info);
            }
            
            // Process multiple URLs (group downloads)
            if urls.len() > 1 {
                // Group download mode
                let mut valid_urls = Vec::new();
                
                for url_text in urls {
                    // Validate URL length
                    if url_text.len() > crate::config::validation::MAX_URL_LENGTH {
                        log::warn!("URL too long: {} characters (max: {})", url_text.len(), crate::config::validation::MAX_URL_LENGTH);
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
                        url.set_query(if new_query.is_empty() { None } else { Some(&new_query) });
                    }
                    
                    valid_urls.push(url);
                }
                
                if valid_urls.is_empty() {
                    bot.send_message(msg.chat.id, "Извини, я не смогла распознать ни одной корректной ссылки. Пожалуйста, пришли мне корректные ссылки на YouTube или SoundCloud.").await?;
                    return Ok(user_info);
                }
                
                // Send confirmation message
                let confirmation_msg = format!("✅ Добавлено {} треков в очередь!", valid_urls.len());
                let status_message = bot.send_message(msg.chat.id, &confirmation_msg).await?;
                
                // Process each URL - get metadata and add to queue
                let download_queue = _download_queue.clone();
                let bot_clone = bot.clone();
                let db_pool_clone = db_pool.clone();
                let chat_id = msg.chat.id;
                
                tokio::spawn(async move {
                    let mut status_text = confirmation_msg.clone();
                    status_text.push_str("\n\n");
                    
                    for (idx, url) in valid_urls.iter().enumerate() {
                        // Get metadata for preview
                        match get_preview_metadata(url).await {
                            Ok(metadata) => {
                                let display_title = metadata.display_title();
                                status_text.push_str(&format!("{}. {} [⏳ В очереди]\n", 
                                    idx + 1, 
                                    display_title.chars().take(50).collect::<String>()
                                ));
                                
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
                                let task = crate::queue::DownloadTask::new(
                                    url.as_str().to_string(),
                                    chat_id,
                                    is_video,
                                    format.clone(),
                                    video_quality,
                                    audio_bitrate,
                                );
                                download_queue.add_task(task).await;
                            }
                            Err(e) => {
                                log::error!("Failed to get preview metadata for URL {}: {:?}", url, e);
                                status_text.push_str(&format!("{}. {} [❌ Ошибка]\n", 
                                    idx + 1, 
                                    url.as_str().chars().take(50).collect::<String>()
                                ));
                            }
                        }
                        
                        // Update status message every few URLs
                        if (idx + 1) % 5 == 0 || idx == valid_urls.len() - 1 {
                            if let Err(e) = bot_clone.edit_message_text(chat_id, status_message.id, &status_text).await {
                                log::warn!("Failed to update status message: {:?}", e);
                            }
                        }
                    }
                    
                    // Final update
                    status_text.push_str("\n✅ Все треки добавлены в очередь!");
                    let _ = bot_clone.edit_message_text(chat_id, status_message.id, &status_text).await;
                });
                
                return Ok(user_info);
            } else {
                // Single URL mode (existing behavior)
                let url_text = urls[0];
                
                // Validate URL length
                if url_text.len() > crate::config::validation::MAX_URL_LENGTH {
                    log::warn!("URL too long: {} characters (max: {})", url_text.len(), crate::config::validation::MAX_URL_LENGTH);
                    bot.send_message(msg.chat.id, format!("Извини, ссылка слишком длинная (максимум {} символов). Пожалуйста, пришли более короткую ссылку.", crate::config::validation::MAX_URL_LENGTH)).await?;
                    return Ok(user_info);
                }
                
                let mut url = match Url::parse(url_text) {
                    Ok(parsed_url) => parsed_url,
                    Err(e) => {
                        log::warn!("Failed to parse URL '{}': {}", url_text, e);
                        bot.send_message(msg.chat.id, "Извини, я не смогла распознать ссылку. Пожалуйста, пришли мне корректную ссылку на YouTube или SoundCloud.").await?;
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
                    url.set_query(if new_query.is_empty() { None } else { Some(&new_query) });
                }
                
                // Show preview instead of immediately downloading
                match get_preview_metadata(&url).await {
                    Ok(metadata) => {
                        // Send preview with inline buttons
                        match send_preview(&bot, msg.chat.id, &url, &metadata, &format).await {
                            Ok(_) => {
                                log::info!("Preview sent successfully for chat {}", msg.chat.id);
                            }
                            Err(e) => {
                                log::error!("Failed to send preview: {:?}", e);
                                // Fallback: send error message
                                bot.send_message(msg.chat.id, "У меня не получилось показать превью 😢 Попробуй еще раз или напиши Стэну.").await?;
                            }
                        }
                    }
                    Err(e) => {
                        log::error!("Failed to get preview metadata: {:?}", e);
                        // Fallback: send error message
                        bot.send_message(msg.chat.id, "У меня не получилось получить информацию о треке 😢 Попробуй еще раз или напиши Стэну.").await?;
                    }
                }
                
                // Return user info for reuse in logging
                return Ok(user_info);
            }
        } else {
            bot.send_message(msg.chat.id, "Извини, я не нашла ссылки на YouTube или SoundCloud. Пожалуйста, пришли мне ссылку на трек или видео, который ты хочешь скачать.").await?;
        }
    } else {
        bot.send_message(msg.chat.id, "Извини, я не нашла ссылки на YouTube или SoundCloud. Пожалуйста, пришли мне ссылку на трек или видео, который ты хочешь скачать.").await?;
    }
    Ok(None)
}