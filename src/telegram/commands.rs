use crate::core::config;
use crate::core::error::AppError;
use crate::core::rate_limiter::RateLimiter;
use crate::core::utils::pluralize_seconds;
use crate::download::queue::DownloadQueue;
use crate::i18n;
use crate::storage::db::{self, DbPool};
use crate::telegram::preview::{get_preview_metadata, send_preview};
use fluent_templates::fluent_bundle::FluentArgs;
use once_cell::sync::Lazy;
use regex::Regex;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::ParseMode;
use url::Url;

/// Cached regex for matching URLs
/// Compiled once at startup and reused for all requests
static URL_REGEX: Lazy<Regex> = Lazy::new(|| Regex::new(r"https?://[^\s]+").expect("Failed to compile URL regex"));

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
    db_pool: &Arc<DbPool>,
) -> ResponseResult<bool> {
    let lang = i18n::user_lang_from_pool(db_pool, msg.chat.id.0);
    if rate_limiter.is_rate_limited(msg.chat.id, plan).await {
        if let Some(remaining_time) = rate_limiter.get_remaining_time(msg.chat.id).await {
            let remaining_seconds = remaining_time.as_secs();
            let unit = if lang.language.as_str() == "ru" {
                pluralize_seconds(remaining_seconds).to_string()
            } else {
                i18n::t(&lang, "common.seconds")
            };
            let mut args = FluentArgs::new();
            args.set("time", remaining_seconds as i64);
            args.set("unit", unit);
            let text = i18n::t_args(&lang, "commands.rate_limited_with_eta", &args);
            bot.send_message(msg.chat.id, text).await?;
        } else {
            let text = i18n::t(&lang, "commands.rate_limited");
            bot.send_message(msg.chat.id, text).await?;
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
    let lang = i18n::user_lang_from_pool(&db_pool, msg.chat.id.0);

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
                    log::error!("Failed to get database connection: {}, using default mp3", e);
                    (String::from("mp3"), None)
                }
            };

            // Check rate limit before processing URLs
            let plan = user_info.as_ref().map(|u| u.plan.as_str()).unwrap_or("free");
            let plan_string = plan.to_string();
            if !handle_rate_limit(&bot, &msg, &rate_limiter, &plan_string, &db_pool).await? {
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
                        url.set_query(if new_query.is_empty() { None } else { Some(&new_query) });
                    }

                    valid_urls.push(url);
                }

                if valid_urls.is_empty() {
                    bot.send_message(msg.chat.id, i18n::t(&lang, "commands.invalid_group_links"))
                        .await?;
                    return Ok(user_info);
                }

                // Send confirmation message
                let mut args = FluentArgs::new();
                args.set("count", valid_urls.len() as i64);
                let confirmation_msg = i18n::t_args(&lang, "commands.group_added", &args);
                let status_message = bot.send_message(msg.chat.id, &confirmation_msg).await?;

                // Process each URL - get metadata and add to queue
                let download_queue = _download_queue.clone();
                let bot_clone = bot.clone();
                let db_pool_clone = db_pool.clone();
                let chat_id = msg.chat.id;
                let lang_clone = lang.clone();

                tokio::spawn(async move {
                    let mut status_text = confirmation_msg.clone();
                    status_text.push_str("\n\n");

                    // Get a DB connection to read user settings
                    let conn = match db::get_connection(&db_pool_clone) {
                        Ok(c) => c,
                        Err(_) => {
                            // If we cannot get a connection, fall back to defaults
                            for (idx, url) in valid_urls.iter().enumerate() {
                                match get_preview_metadata(url, Some(&format), None).await {
                                    Ok(metadata) => {
                                        let display_title = metadata.display_title();

                                        // Check the file size
                                        let status_marker = if let Some(filesize) = metadata.filesize {
                                            let max_size = if format == "mp4" {
                                                config::validation::max_video_size_bytes()
                                            } else {
                                                config::validation::max_audio_size_bytes()
                                            };

                                            if filesize > max_size {
                                                i18n::t(&lang_clone, "commands.status_too_large")
                                            } else {
                                                i18n::t(&lang_clone, "commands.status_in_queue")
                                            }
                                        } else {
                                            i18n::t(&lang_clone, "commands.status_in_queue")
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
                                            "{}. {} [{}]\n",
                                            idx + 1,
                                            url.as_str().chars().take(50).collect::<String>(),
                                            i18n::t(&lang_clone, "commands.status_error")
                                        ));
                                    }
                                }
                            }
                            return;
                        }
                    };

                    for (idx, url) in valid_urls.iter().enumerate() {
                        // Get metadata for preview
                        // Get video quality for preview (for group downloads)
                        let video_quality_for_preview = if format == "mp4" {
                            match db::get_user_video_quality(&conn, chat_id.0) {
                                Ok(q) => Some(q),
                                Err(_) => Some("best".to_string()),
                            }
                        } else {
                            None
                        };

                        match get_preview_metadata(url, Some(&format), video_quality_for_preview.as_deref()).await {
                            Ok(metadata) => {
                                let display_title = metadata.display_title();

                                // Check file size for group downloads
                                let status_marker = if let Some(filesize) = metadata.filesize {
                                    let max_size = if format == "mp4" {
                                        config::validation::max_video_size_bytes()
                                    } else {
                                        config::validation::max_audio_size_bytes()
                                    };

                                    if filesize > max_size {
                                        i18n::t(&lang_clone, "commands.status_too_large")
                                    } else {
                                        i18n::t(&lang_clone, "commands.status_in_queue")
                                    }
                                } else {
                                    i18n::t(&lang_clone, "commands.status_in_queue")
                                };

                                status_text.push_str(&format!(
                                    "{}. {} [{}]\n",
                                    idx + 1,
                                    display_title.chars().take(50).collect::<String>(),
                                    status_marker
                                ));

                                // Skip files that are too large and do not enqueue them
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
                                    log::info!("Skipping file {} in group download - too large", url.as_str());
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
                                download_queue.add_task(task, Some(Arc::clone(&db_pool))).await;
                            }
                            Err(e) => {
                                log::error!("Failed to get preview metadata for URL {}: {:?}", url, e);
                                status_text.push_str(&format!(
                                    "{}. {} [{}]\n",
                                    idx + 1,
                                    url.as_str().chars().take(50).collect::<String>(),
                                    i18n::t(&lang_clone, "commands.status_error")
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
                    status_text.push_str(&format!("\n{}", i18n::t(&lang_clone, "commands.group_complete")));
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
                    let mut args = FluentArgs::new();
                    args.set("max", crate::config::validation::MAX_URL_LENGTH as i64);
                    bot.send_message(msg.chat.id, i18n::t_args(&lang, "commands.url_too_long", &args))
                        .await?;
                    return Ok(user_info);
                }

                let mut url = match Url::parse(url_text) {
                    Ok(parsed_url) => parsed_url,
                    Err(e) => {
                        log::warn!("Failed to parse URL '{}': {}", url_text, e);
                        bot.send_message(msg.chat.id, i18n::t(&lang, "commands.invalid_single_link"))
                            .await?;
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

                // Send "processing" message
                let processing_msg = bot
                    .send_message(msg.chat.id, i18n::t(&lang, "commands.processing"))
                    .await?;

                // Show preview instead of immediately downloading
                // Get video quality for the preview
                let conn_for_preview = db::get_connection(&db_pool);

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
                        // Check file size during preview ONLY for audio
                        // Skip the check for MP4 so the user can pick a lower quality in the preview
                        if format != "mp4" {
                            if let Some(filesize) = metadata.filesize {
                                let max_size = config::validation::max_audio_size_bytes();

                                if filesize > max_size * 1000 {
                                    let size_mb = filesize as f64 / (1024.0 * 1024.0);
                                    //let max_mb = max_size as f64 / (1024.0 * 1024.0);
                                    let max_mb = max_size as f64 / (1024.0 * 2.0 * 1024.0);
                                    log::warn!(
                                        "Audio file too large at preview stage: {:.2} MB (max: {:.2} MB)",
                                        size_mb,
                                        max_mb
                                    );

                                    let mut args = FluentArgs::new();
                                    args.set("size", format!("{:.1}", size_mb));
                                    args.set("max", format!("{:.1}", max_mb));
                                    let error_message = i18n::t_args(&lang, "commands.audio_too_large", &args);

                                    // Delete processing message
                                    let _ = bot.delete_message(msg.chat.id, processing_msg.id).await;

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
                            Some(processing_msg.id),
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
                                bot.send_message(msg.chat.id, i18n::t(&lang, "commands.preview_failed"))
                                    .await?;
                            }
                        }
                    }
                    Err(e) => {
                        log::error!("Failed to get preview metadata: {:?}", e);

                        // Delete processing message
                        let _ = bot.delete_message(msg.chat.id, processing_msg.id).await;

                        // Check whether this is a duration-related error
                        let error_message = if let AppError::Download(ref msg) = e {
                            if msg.contains("Видео слишком длинное") {
                                msg.clone()
                            } else {
                                i18n::t(&lang, "commands.preview_info_failed")
                            }
                        } else {
                            i18n::t(&lang, "commands.preview_info_failed")
                        };

                        bot.send_message(msg.chat.id, error_message).await?;
                    }
                }

                // Return user info for reuse in logging
                return Ok(user_info);
            }
        } else {
            bot.send_message(msg.chat.id, i18n::t(&lang, "commands.no_links"))
                .await?;
        }
    } else {
        bot.send_message(msg.chat.id, i18n::t(&lang, "commands.no_links"))
            .await?;
    }
    Ok(None)
}

/// Handle /info command to show available formats for a URL
///
/// Parses URL from command text and displays detailed information about available formats,
/// sizes, quality options, and types (mp4, mp3).
///
/// # Arguments
///
/// * `bot` - Telegram bot instance
/// * `msg` - Message containing the /info command and URL
///
/// # Returns
///
/// Returns `ResponseResult<()>` indicating success or failure
///
/// # Behavior
///
/// - Extracts URL from message text (format: /info <URL>)
/// - Fetches metadata using yt-dlp
/// - Displays available video formats with quality and sizes
/// - Shows audio format information
/// - Sends formatted message to user
pub async fn handle_info_command(bot: Bot, msg: Message) -> ResponseResult<()> {
    log::info!("════════════════════════════════════════════════════════");
    log::info!("📋 /info command called");
    log::info!("Chat ID: {}", msg.chat.id);
    log::info!("════════════════════════════════════════════════════════");

    if let Some(text) = msg.text() {
        log::info!("✅ Message text found: '{}'", text);

        // Extract URL from command text
        let parts: Vec<&str> = text.split_whitespace().collect();
        log::info!("📊 Parts count: {} - Parts: {:?}", parts.len(), parts);

        if parts.len() < 2 {
            log::warn!("⚠️  No URL provided, sending usage instructions");
            match bot
                .send_message(
                    msg.chat.id,
                    "Использование: /info <URL>\n\nПример:\n/info https://www.youtube.com/watch?v=dQw4w9WgXcQ",
                )
                .await
            {
                Ok(_) => log::info!("✅ Usage message sent successfully"),
                Err(e) => log::error!("❌ Failed to send usage message: {:?}", e),
            }
            return Ok(());
        }

        let url_text = parts[1];
        log::info!("🔗 Extracted URL: {}", url_text);

        // Validate URL
        let url = match Url::parse(url_text) {
            Ok(parsed_url) => {
                log::info!("✅ URL parsed successfully: {}", parsed_url);
                parsed_url
            }
            Err(e) => {
                log::error!("❌ Failed to parse URL '{}': {}", url_text, e);
                match bot
                    .send_message(
                        msg.chat.id,
                        "Некорректная ссылка. Пожалуйста, пришли корректную ссылку.",
                    )
                    .await
                {
                    Ok(_) => log::info!("✅ Error message sent successfully"),
                    Err(e) => log::error!("❌ Failed to send error message: {:?}", e),
                }
                return Ok(());
            }
        };

        // Send "processing" message
        log::info!("📤 Sending 'processing' message...");
        let processing_msg = match bot.send_message(msg.chat.id, "⏳ Получаю информацию...").await {
            Ok(msg) => {
                log::info!("✅ Processing message sent, ID: {}", msg.id);
                msg
            }
            Err(e) => {
                log::error!("❌ Failed to send processing message: {:?}", e);
                return Err(e);
            }
        };

        // Get metadata with video formats
        log::info!("🔍 Fetching metadata from yt-dlp...");
        match get_preview_metadata(&url, Some("mp4"), Some("best")).await {
            Ok(metadata) => {
                log::info!("✅ Metadata fetched successfully");
                log::info!("📝 Title: {}", metadata.display_title());
                log::info!("⏱ Duration: {:?}", metadata.duration);
                log::info!("📦 File size: {:?}", metadata.filesize);
                log::info!(
                    "🎬 Video formats count: {:?}",
                    metadata.video_formats.as_ref().map(|f| f.len())
                );

                // Log detailed format information
                if let Some(ref formats) = metadata.video_formats {
                    log::info!("📋 Available video formats:");
                    for (idx, format) in formats.iter().enumerate() {
                        log::info!(
                            "  [{}] Quality: {}, Resolution: {:?}, Size: {:?} bytes ({:.2} MB)",
                            idx,
                            format.quality,
                            format.resolution,
                            format.size_bytes,
                            format.size_bytes.unwrap_or(0) as f64 / (1024.0 * 1024.0)
                        );
                    }
                } else {
                    log::warn!("⚠️  No video formats available in metadata");
                }

                let mut response = String::new();

                // Title and artist
                response.push_str(&format!("🎵 *{}*\n\n", escape_markdown(&metadata.display_title())));

                // Duration
                if let Some(duration) = metadata.duration {
                    let minutes = duration / 60;
                    let seconds = duration % 60;
                    response.push_str(&format!("⏱ Длительность: {}:{:02}\n\n", minutes, seconds));
                }

                // Video formats section
                if let Some(ref formats) = metadata.video_formats {
                    response.push_str("📹 *Видео форматы \\(MP4\\):*\n");

                    // Filter and sort formats by quality
                    let quality_order = ["1080p", "720p", "480p", "360p"];
                    let available_formats: Vec<_> = quality_order
                        .iter()
                        .filter_map(|&quality| formats.iter().find(|f| f.quality == quality))
                        .collect();

                    log::info!(
                        "📊 Filtered formats for display: {} out of {} total",
                        available_formats.len(),
                        formats.len()
                    );
                    for format in &available_formats {
                        log::info!(
                            "  ✓ Will display: {} - {:?} - {:.2} MB",
                            format.quality,
                            format.resolution,
                            format.size_bytes.unwrap_or(0) as f64 / (1024.0 * 1024.0)
                        );
                    }

                    if available_formats.is_empty() {
                        log::warn!("⚠️  No formats matched quality_order filter");
                        response.push_str("  • Нет доступных форматов\n");
                    } else {
                        for format in available_formats {
                            let quality = escape_markdown(&format.quality);

                            if let Some(size) = format.size_bytes {
                                let size_mb = size as f64 / (1024.0 * 1024.0);
                                let size_str = escape_markdown(&format!("{:.1} MB", size_mb));
                                response.push_str(&format!("  • {} \\- {}", quality, size_str));

                                if let Some(ref resolution) = format.resolution {
                                    let res = escape_markdown(resolution);
                                    response.push_str(&format!(" \\({}\\)", res));
                                }
                                response.push('\n');
                            } else {
                                response.push_str(&format!("  • {} \\- размер неизвестен", quality));

                                if let Some(ref resolution) = format.resolution {
                                    let res = escape_markdown(resolution);
                                    response.push_str(&format!(" \\({}\\)", res));
                                }
                                response.push('\n');
                            }
                        }
                    }
                    response.push('\n');
                }

                // Audio format section
                response.push_str("🎧 *Аудио формат \\(MP3\\):*\n");
                if let Some(size) = metadata.filesize {
                    let size_mb = size as f64 / (1024.0 * 1024.0);
                    let size_str = escape_markdown(&format!("{:.1} MB", size_mb));
                    response.push_str(&format!("  • 320 kbps \\- {}\n", size_str));
                } else {
                    response.push_str("  • 320 kbps \\- размер неизвестен\n");
                }
                response.push('\n');

                // Additional info
                response.push_str("💡 *Как скачать:*\n");
                response.push_str("1\\. Отправь мне ссылку\n");
                response.push_str("2\\. Выбери формат и качество в меню\n");
                response.push_str("3\\. Получи файл\\!");

                log::info!("📝 Response formatted, length: {} chars", response.len());
                log::debug!("Response preview: {}", &response[..response.len().min(200)]);

                // Delete processing message and send result
                log::info!("🗑 Deleting processing message...");
                match bot.delete_message(msg.chat.id, processing_msg.id).await {
                    Ok(_) => log::info!("✅ Processing message deleted"),
                    Err(e) => log::warn!("⚠️  Failed to delete processing message: {:?}", e),
                }

                log::info!("📤 Sending formatted response with MarkdownV2...");
                match bot
                    .send_message(msg.chat.id, response)
                    .parse_mode(ParseMode::MarkdownV2)
                    .await
                {
                    Ok(_) => {
                        log::info!("✅ Response sent successfully!");
                        log::info!("════════════════════════════════════════════════════════");
                    }
                    Err(e) => {
                        log::error!("❌ Failed to send response: {:?}", e);
                        log::info!("════════════════════════════════════════════════════════");
                        return Err(e);
                    }
                }
            }
            Err(e) => {
                log::error!("❌ Failed to get metadata: {:?}", e);

                log::info!("🗑 Deleting processing message...");
                match bot.delete_message(msg.chat.id, processing_msg.id).await {
                    Ok(_) => log::info!("✅ Processing message deleted"),
                    Err(e) => log::warn!("⚠️  Failed to delete processing message: {:?}", e),
                }

                let error_msg = format!("❌ Не удалось получить информацию о файле:\n{}", e);
                log::info!("📤 Sending error message...");
                match bot.send_message(msg.chat.id, error_msg).await {
                    Ok(_) => {
                        log::info!("✅ Error message sent successfully");
                        log::info!("════════════════════════════════════════════════════════");
                    }
                    Err(e) => {
                        log::error!("❌ Failed to send error message: {:?}", e);
                        log::info!("════════════════════════════════════════════════════════");
                        return Err(e);
                    }
                }
            }
        }
    } else {
        log::error!("❌ No text in message!");
        log::info!("════════════════════════════════════════════════════════");
    }

    log::info!("✅ handle_info_command completed");
    Ok(())
}

/// Helper function to escape special characters for MarkdownV2
fn escape_markdown(text: &str) -> String {
    let mut result = String::with_capacity(text.len() * 2);

    for c in text.chars() {
        match c {
            '_' | '*' | '[' | ']' | '(' | ')' | '~' | '`' | '>' | '#' | '+' | '-' | '=' | '|' | '{' | '}' | '.'
            | '!' => {
                result.push('\\');
                result.push(c);
            }
            _ => result.push(c),
        }
    }

    result
}
