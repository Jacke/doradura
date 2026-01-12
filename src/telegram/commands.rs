use crate::core::config;
use crate::core::error::AppError;
use crate::core::rate_limiter::RateLimiter;
use crate::core::utils::pluralize_seconds;
use crate::download::queue::DownloadQueue;
use crate::downsub::{DownsubError, DownsubGateway};
use crate::i18n;
use crate::storage::db::{self, DbPool};
use crate::telegram::preview::{get_preview_metadata, send_preview};
use fluent_templates::fluent_bundle::FluentArgs;
use once_cell::sync::Lazy;
use regex::Regex;
use std::sync::Arc;
use teloxide::prelude::*;
use crate::telegram::Bot;
use teloxide::types::{InputFile, ParseMode};
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
    let lang = i18n::user_lang_from_pool_with_fallback(
        &db_pool,
        msg.chat.id.0,
        msg.from.as_ref().and_then(|user| user.language_code.as_deref()),
    );

    // Handle document upload (for cookies file)
    if let Some(document) = msg.document() {
        if let Some(user) = msg.from.as_ref() {
            let user_id = user.id.0 as i64;
            // Check if user has active cookies upload session
            if let Ok(conn) = db::get_connection(&db_pool) {
                if let Ok(Some(_session)) = db::get_active_cookies_upload_session(&conn, user_id) {
                    // Handle cookies file upload
                    if let Err(e) = crate::telegram::handle_cookies_file_upload(
                        db_pool.clone(),
                        &bot,
                        msg.chat.id,
                        user_id,
                        document,
                    )
                    .await
                    {
                        log::error!("Failed to handle cookies file upload: {}", e);
                    }
                    return Ok(None);
                }
            }
        }
    }

    if let Some(text) = msg.text() {
        log::debug!("handle_message: {:?}", text);
        if text.starts_with("/start") || text.starts_with("/help") {
            return Ok(None);
        }

        // Audio cut sessions (from "Cut Audio" button)
        if !text.trim().starts_with('/') {
            if let Ok(conn) = db::get_connection(&db_pool) {
                if let Ok(Some(session)) = db::get_active_audio_cut_session(&conn, msg.chat.id.0) {
                    let trimmed = text.trim();
                    if is_cancel_text(trimmed) {
                        let _ = db::delete_audio_cut_session_by_user(&conn, msg.chat.id.0);
                        bot.send_message(msg.chat.id, i18n::t(&lang, "commands.audio_cut_cancelled"))
                            .await
                            .ok();
                        return Ok(None);
                    }

                    let audio_session = match db::get_audio_effect_session(&conn, &session.audio_session_id) {
                        Ok(Some(audio_session)) => audio_session,
                        Ok(None) => {
                            let _ = db::delete_audio_cut_session_by_user(&conn, msg.chat.id.0);
                            bot.send_message(msg.chat.id, i18n::t(&lang, "commands.audio_session_expired"))
                                .await
                                .ok();
                            return Ok(None);
                        }
                        Err(e) => {
                            log::warn!("Failed to load audio session for cut: {}", e);
                            return Ok(None);
                        }
                    };
                    if audio_session.is_expired() {
                        let _ = db::delete_audio_cut_session_by_user(&conn, msg.chat.id.0);
                        bot.send_message(msg.chat.id, i18n::t(&lang, "commands.audio_session_expired"))
                            .await
                            .ok();
                        return Ok(None);
                    }

                    let audio_duration = Some(audio_session.duration as i64);
                    if let Some((segments, segments_text)) = parse_audio_segments_spec(trimmed, audio_duration) {
                        let _ = db::delete_audio_cut_session_by_user(&conn, msg.chat.id.0);

                        let bot_clone = bot.clone();
                        let db_pool_clone = db_pool.clone();
                        let chat_id = msg.chat.id;
                        tokio::spawn(async move {
                            if let Err(e) = process_audio_cut(
                                bot_clone,
                                db_pool_clone,
                                chat_id,
                                audio_session,
                                segments,
                                segments_text,
                            )
                            .await
                            {
                                log::warn!("Failed to process audio cut: {}", e);
                            }
                        });

                        return Ok(None);
                    } else {
                        crate::telegram::send_message_markdown_v2(
                            &bot,
                            msg.chat.id,
                            i18n::t(&lang, "commands.audio_cut_invalid_intervals"),
                            None,
                        )
                        .await
                        .ok();
                        return Ok(None);
                    }
                }
            }
        }

        // Video clip sessions (from /downloads or /cuts -> ✂️ Вырезка)
        if !text.trim().starts_with('/') {
            if let Ok(conn) = db::get_connection(&db_pool) {
                if let Ok(Some(session)) = db::get_active_video_clip_session(&conn, msg.chat.id.0) {
                    let trimmed = text.trim();
                    if is_cancel_text(trimmed) {
                        let _ = db::delete_video_clip_session_by_user(&conn, msg.chat.id.0);
                        bot.send_message(msg.chat.id, i18n::t(&lang, "commands.video_clip_cancelled"))
                            .await
                            .ok();
                        return Ok(None);
                    }

                    // Get video duration from source
                    let video_duration = match session.source_kind.as_str() {
                        "download" => db::get_download_history_entry(&conn, msg.chat.id.0, session.source_id)
                            .ok()
                            .flatten()
                            .and_then(|d| d.duration),
                        "cut" => db::get_cut_entry(&conn, msg.chat.id.0, session.source_id)
                            .ok()
                            .flatten()
                            .and_then(|c| c.duration),
                        _ => None,
                    };

                    if let Some((segments, segments_text, speed)) = parse_segments_spec(trimmed, video_duration) {
                        let _ = db::delete_video_clip_session_by_user(&conn, msg.chat.id.0);

                        let bot_clone = bot.clone();
                        let db_pool_clone = db_pool.clone();
                        let chat_id = msg.chat.id;
                        tokio::spawn(async move {
                            if let Err(e) = process_video_clip(
                                bot_clone,
                                db_pool_clone,
                                chat_id,
                                session,
                                segments,
                                segments_text,
                                speed,
                            )
                            .await
                            {
                                log::warn!("Failed to process video clip: {}", e);
                            }
                        });

                        return Ok(None);
                    } else {
                        let extra_note = if session.output_kind == "video_note" {
                            "\n\n💡 Если длительность превысит 60 секунд \\(лимит Telegram для кружков\\), видео будет автоматически обрезано\\."
                        } else {
                            ""
                        };
                        bot.send_message(
                            msg.chat.id,
                            format!(
                                "❌ Не понял интервалы\\.\n\nОтправь в формате `мм:сс-мм:сс` или `чч:мм:сс-чч:мм:сс`\\.\nМожно несколько через запятую\\.\nПример: `00:10-00:25, 01:00-01:10`\n\nИли команды: `full`, `first30`, `last30`, `middle30`\\.\n\n💡 Можно добавить скорость: `first30 2x`, `full 1\\.5x`\\.\n\nИли напиши `отмена`\\.{extra_note}",
                            ),
                        )
                        .parse_mode(ParseMode::MarkdownV2)
                        .await
                        .ok();
                        return Ok(None);
                    }
                }
            }
        }

        // Check if user is waiting to provide feedback
        if crate::telegram::feedback::is_waiting_for_feedback(msg.chat.id.0).await {
            // Get user info for admin notification
            let username = msg.from.as_ref().and_then(|u| u.username.as_deref());
            let first_name = msg.from.as_ref().map(|u| u.first_name.as_str()).unwrap_or("Unknown");

            // Send feedback to admin
            let _ = crate::telegram::feedback::notify_admin_feedback(
                &bot,
                msg.chat.id.0,
                username,
                first_name,
                text,
                db_pool.clone(),
            )
            .await;

            // Send confirmation to user and return to main menu
            let _ = crate::telegram::feedback::send_feedback_confirmation(&bot, msg.chat.id, &lang).await;
            let _ = crate::telegram::show_enhanced_main_menu(&bot, msg.chat.id, db_pool.clone()).await;

            return Ok(None);
        }

        // Use cached regex for better performance - find all URLs
        let urls: Vec<&str> = URL_REGEX.find_iter(text).map(|m| m.as_str()).collect();

        if !urls.is_empty() {
            // Mark the user's link message as "seen"
            crate::telegram::try_set_reaction(&bot, msg.chat.id, teloxide::types::MessageId(msg.id.0), "👀").await;

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

                    crate::telegram::cache::store_link_message_id(url.as_str(), msg.id.0).await;
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

                crate::telegram::cache::store_link_message_id(url.as_str(), msg.id.0).await;

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
                            // If it's already translated error (from preview), use it
                            if msg.contains("слишком длинное")
                                || msg.contains("too long")
                                || msg.contains("zu lang")
                                || msg.contains("trop long")
                            {
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

fn is_cancel_text(text: &str) -> bool {
    let lower = text.trim().to_lowercase();
    matches!(lower.as_str(), "отмена" | "cancel" | "/cancel" | "❌" | "x")
}

fn parse_command_segment(text: &str, video_duration: Option<i64>) -> Option<(i64, i64, String)> {
    let normalized = text.trim().to_lowercase();

    // Strip speed modifiers if present (e.g., "first30 2x", "full speed1.5")
    // We'll just parse the segment here, speed will be handled separately
    let segment_part = normalized.split_whitespace().next().unwrap_or(&normalized);

    // full - всё видео
    if segment_part == "full" {
        let duration = video_duration?;
        let end = duration.min(60); // Для кружков максимум 60 секунд
        return Some((0, end, format!("00:00-{}", format_timestamp(end))));
    }

    // first<N> - первые N секунд (first30, first15, etc.)
    if let Some(num_str) = segment_part.strip_prefix("first") {
        if let Ok(secs) = num_str.parse::<i64>() {
            if secs > 0 && secs <= 60 {
                return Some((0, secs, format!("00:00-{}", format_timestamp(secs))));
            }
        }
    }

    // last<N> - последние N секунд (last30, last15, etc.)
    if let Some(num_str) = segment_part.strip_prefix("last") {
        if let Ok(secs) = num_str.parse::<i64>() {
            let duration = video_duration?;
            if secs > 0 && secs <= 60 && secs <= duration {
                let start = (duration - secs).max(0);
                return Some((
                    start,
                    duration,
                    format!("{}-{}", format_timestamp(start), format_timestamp(duration)),
                ));
            }
        }
    }

    // middle<N> - N секунд из середины (middle30, middle15, etc.)
    if let Some(num_str) = segment_part.strip_prefix("middle") {
        if let Ok(secs) = num_str.parse::<i64>() {
            let duration = video_duration?;
            if secs > 0 && secs <= 60 && secs <= duration {
                let start = ((duration - secs) / 2).max(0);
                let end = start + secs;
                return Some((
                    start,
                    end,
                    format!("{}-{}", format_timestamp(start), format_timestamp(end)),
                ));
            }
        }
    }

    None
}

fn parse_time_range_secs(text: &str) -> Option<(i64, i64)> {
    let normalized = text.trim().replace(['—', '–', '−'], "-").replace(' ', "");
    let (start_str, end_str) = normalized.split_once('-')?;
    let start = parse_timestamp_secs(start_str)?;
    let end = parse_timestamp_secs(end_str)?;
    if end <= start {
        return None;
    }
    Some((start, end))
}

fn parse_timestamp_secs(text: &str) -> Option<i64> {
    let parts: Vec<&str> = text.split(':').collect();
    match parts.len() {
        2 => {
            let minutes: i64 = parts[0].parse().ok()?;
            let seconds: i64 = parts[1].parse().ok()?;
            if minutes < 0 || !(0..60).contains(&seconds) {
                return None;
            }
            Some(minutes * 60 + seconds)
        }
        3 => {
            let hours: i64 = parts[0].parse().ok()?;
            let minutes: i64 = parts[1].parse().ok()?;
            let seconds: i64 = parts[2].parse().ok()?;
            if hours < 0 || minutes < 0 || !(0..60).contains(&minutes) || !(0..60).contains(&seconds) {
                return None;
            }
            Some(hours * 3600 + minutes * 60 + seconds)
        }
        _ => None,
    }
}

fn format_timestamp(secs: i64) -> String {
    let secs = secs.max(0);
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{h:02}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

#[derive(Debug, Clone, Copy, serde::Serialize)]
struct CutSegment {
    start_secs: i64,
    end_secs: i64,
}

fn parse_segments_spec(text: &str, video_duration: Option<i64>) -> Option<(Vec<CutSegment>, String, Option<f32>)> {
    let normalized = text.trim().replace(['—', '–', '−'], "-");

    // Extract speed modifier from anywhere in the text (e.g., "first30 2x", "1.5x full", "speed2 middle30")
    let speed = parse_speed_modifier(&normalized);

    let raw_parts: Vec<&str> = normalized
        .split([',', ';', '\n'])
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    if raw_parts.is_empty() {
        return None;
    }

    let mut segments = Vec::new();
    let mut pretty_parts = Vec::new();
    for part in raw_parts {
        // Try parsing as command first (full, first30, last30, etc.)
        if let Some((start_secs, end_secs, pretty)) = parse_command_segment(part, video_duration) {
            segments.push(CutSegment { start_secs, end_secs });
            pretty_parts.push(pretty);
        } else if let Some((start_secs, end_secs)) = parse_time_range_secs(part) {
            // Fall back to time range parsing
            segments.push(CutSegment { start_secs, end_secs });
            pretty_parts.push(format!(
                "{}-{}",
                format_timestamp(start_secs),
                format_timestamp(end_secs)
            ));
        } else {
            return None; // Invalid format
        }
    }

    Some((segments, pretty_parts.join(", "), speed))
}

fn parse_audio_segments_spec(text: &str, audio_duration: Option<i64>) -> Option<(Vec<CutSegment>, String)> {
    let normalized = text.trim();
    if normalized.is_empty() {
        return None;
    }

    let raw_parts: Vec<&str> = normalized
        .split([',', ';', '\n'])
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .collect();
    if raw_parts.is_empty() {
        return None;
    }

    let mut segments = Vec::new();
    let mut pretty_parts = Vec::new();
    for part in raw_parts {
        if let Some((start_secs, end_secs, pretty)) = parse_audio_command_segment(part, audio_duration) {
            segments.push(CutSegment { start_secs, end_secs });
            pretty_parts.push(pretty);
        } else if let Some((start_secs, end_secs)) = parse_time_range_secs(part) {
            segments.push(CutSegment { start_secs, end_secs });
            pretty_parts.push(format!(
                "{}-{}",
                format_timestamp(start_secs),
                format_timestamp(end_secs)
            ));
        } else {
            return None;
        }
    }

    Some((segments, pretty_parts.join(", ")))
}

fn parse_speed_modifier(text: &str) -> Option<f32> {
    let lower = text.to_lowercase();

    // Look for patterns like: "2x", "1.5x", "speed2", "speed1.5", "x2", "x1.5"
    for word in lower.split_whitespace() {
        // "2x", "1.5x"
        if let Some(num_str) = word.strip_suffix('x') {
            if let Ok(speed) = num_str.parse::<f32>() {
                if speed > 0.0 && speed <= 2.0 {
                    return Some(speed);
                }
            }
        }
        // "x2", "x1.5"
        if let Some(num_str) = word.strip_prefix('x') {
            if let Ok(speed) = num_str.parse::<f32>() {
                if speed > 0.0 && speed <= 2.0 {
                    return Some(speed);
                }
            }
        }
        // "speed2", "speed1.5"
        if let Some(num_str) = word.strip_prefix("speed") {
            if let Ok(speed) = num_str.parse::<f32>() {
                if speed > 0.0 && speed <= 2.0 {
                    return Some(speed);
                }
            }
        }
    }

    None
}

fn parse_audio_command_segment(text: &str, audio_duration: Option<i64>) -> Option<(i64, i64, String)> {
    let normalized = text.trim().to_lowercase();
    let segment_part = normalized.split_whitespace().next().unwrap_or(&normalized);
    let duration = audio_duration?;

    if segment_part == "full" {
        return Some((0, duration, format!("00:00-{}", format_timestamp(duration))));
    }

    if let Some(num_str) = segment_part.strip_prefix("first") {
        if let Ok(secs) = num_str.parse::<i64>() {
            if secs > 0 {
                let end = secs.min(duration);
                return Some((0, end, format!("00:00-{}", format_timestamp(end))));
            }
        }
    }

    if let Some(num_str) = segment_part.strip_prefix("last") {
        if let Ok(secs) = num_str.parse::<i64>() {
            if secs > 0 && secs <= duration {
                let start = (duration - secs).max(0);
                return Some((
                    start,
                    duration,
                    format!("{}-{}", format_timestamp(start), format_timestamp(duration)),
                ));
            }
        }
    }

    if let Some(num_str) = segment_part.strip_prefix("middle") {
        if let Ok(secs) = num_str.parse::<i64>() {
            if secs > 0 && secs <= duration {
                let start = ((duration - secs) / 2).max(0);
                let end = start + secs;
                return Some((
                    start,
                    end,
                    format!("{}-{}", format_timestamp(start), format_timestamp(end)),
                ));
            }
        }
    }

    None
}

async fn process_video_clip(
    bot: Bot,
    db_pool: Arc<DbPool>,
    chat_id: ChatId,
    session: db::VideoClipSession,
    segments: Vec<CutSegment>,
    segments_text: String,
    speed: Option<f32>,
) -> Result<(), AppError> {
    use tokio::process::Command;

    let lang = i18n::user_lang_from_pool(&db_pool, chat_id.0);
    let total_len: i64 = segments.iter().map(|s| (s.end_secs - s.start_secs).max(0)).sum();
    let is_video_note = session.output_kind == "video_note";
    let is_ringtone = session.output_kind == "iphone_ringtone";
    let max_len_secs = if is_video_note {
        60
    } else if is_ringtone {
        30
    } else {
        60 * 10
    };

    // For video notes and ringtones, truncate segments to fit within limit and notify user
    let (adjusted_segments, truncated) = if (is_video_note || is_ringtone) && total_len > max_len_secs {
        let mut adjusted = Vec::new();
        let mut accumulated = 0i64;

        for seg in &segments {
            let seg_len = seg.end_secs - seg.start_secs;
            if accumulated >= max_len_secs {
                break; // Already reached limit
            }

            if accumulated + seg_len <= max_len_secs {
                // Segment fits completely
                adjusted.push(*seg);
                accumulated += seg_len;
            } else {
                // Partial segment to fill remaining time
                let remaining = max_len_secs - accumulated;
                adjusted.push(CutSegment {
                    start_secs: seg.start_secs,
                    end_secs: seg.start_secs + remaining,
                });
                break;
            }
        }

        (adjusted, true)
    } else if !is_video_note && !is_ringtone && total_len > 600 {
        // For regular cuts, reject if too long (10 min)
        bot.send_message(chat_id, i18n::t(&lang, "commands.cut_too_long"))
            .await
            .ok();
        return Ok(());
    } else {
        (segments.clone(), false)
    };

    // Calculate actual length after truncation
    let actual_total_len: i64 = adjusted_segments
        .iter()
        .map(|s| (s.end_secs - s.start_secs).max(0))
        .sum();

    // Notify user if segments were truncated
    if truncated {
        let limit_text = if is_ringtone {
            i18n::t(&lang, "commands.cut_limit_ringtone")
        } else {
            i18n::t(&lang, "commands.cut_limit_video_note")
        };
        let mut args = FluentArgs::new();
        args.set("total", total_len);
        args.set("limit", limit_text);
        args.set("actual", actual_total_len);
        bot.send_message(chat_id, i18n::t_args(&lang, "commands.cut_truncated", &args))
            .await
            .ok();
    }

    let conn = db::get_connection(&db_pool)?;
    let (file_id, original_url, base_title, video_quality) = match session.source_kind.as_str() {
        "download" => {
            let download = match db::get_download_history_entry(&conn, chat_id.0, session.source_id)? {
                Some(d) => d,
                None => {
                    bot.send_message(chat_id, i18n::t(&lang, "commands.cut_file_not_found"))
                        .await
                        .ok();
                    return Ok(());
                }
            };
            if download.format != "mp4" {
                bot.send_message(chat_id, i18n::t(&lang, "commands.cut_only_mp4"))
                    .await
                    .ok();
                return Ok(());
            }
            let fid = match download.file_id.clone() {
                Some(fid) => fid,
                None => {
                    bot.send_message(chat_id, i18n::t(&lang, "commands.cut_missing_file_id"))
                        .await
                        .ok();
                    return Ok(());
                }
            };
            (fid, download.url, download.title, download.video_quality)
        }
        "cut" => {
            let cut = match db::get_cut_entry(&conn, chat_id.0, session.source_id)? {
                Some(c) => c,
                None => {
                    bot.send_message(chat_id, i18n::t(&lang, "commands.cut_not_found"))
                        .await
                        .ok();
                    return Ok(());
                }
            };
            let fid = match cut.file_id.clone() {
                Some(fid) => fid,
                None => {
                    bot.send_message(chat_id, i18n::t(&lang, "commands.cut_missing_file_id"))
                        .await
                        .ok();
                    return Ok(());
                }
            };
            (
                fid,
                if !cut.original_url.is_empty() {
                    cut.original_url
                } else {
                    session.original_url.clone()
                },
                cut.title,
                cut.video_quality,
            )
        }
        _ => {
            bot.send_message(chat_id, i18n::t(&lang, "commands.cut_unknown_source"))
                .await
                .ok();
            return Ok(());
        }
    };

    let status_msg = if let Some(spd) = speed {
        let mut args = FluentArgs::new();
        args.set("segments", segments_text.as_str());
        args.set("speed", spd as f64);
        if is_video_note {
            i18n::t_args(&lang, "commands.cut_status_video_note_speed", &args)
        } else if is_ringtone {
            i18n::t_args(&lang, "commands.cut_status_ringtone_speed", &args)
        } else {
            i18n::t_args(&lang, "commands.cut_status_clip_speed", &args)
        }
    } else {
        let mut args = FluentArgs::new();
        args.set("segments", segments_text.as_str());
        if is_video_note {
            i18n::t_args(&lang, "commands.cut_status_video_note", &args)
        } else if is_ringtone {
            i18n::t_args(&lang, "commands.cut_status_ringtone", &args)
        } else {
            i18n::t_args(&lang, "commands.cut_status_clip", &args)
        }
    };

    let status = bot.send_message(chat_id, status_msg).await?;

    let temp_dir = std::path::PathBuf::from(crate::core::config::TEMP_FILES_DIR.as_str()).join("doradura_clip");
    tokio::fs::create_dir_all(&temp_dir).await.ok();

    let input_path = temp_dir.join(format!("input_{}_{}.mp4", chat_id.0, session.source_id));
    let output_path = temp_dir.join(format!(
        "{}_{}_{}.{}",
        if is_video_note {
            "circle"
        } else if is_ringtone {
            "ringtone"
        } else {
            "cut"
        },
        chat_id.0,
        uuid::Uuid::new_v4(),
        if is_ringtone { "m4r" } else { "mp4" }
    ));

    let _ = crate::telegram::download_file_from_telegram(&bot, &file_id, Some(input_path.clone()))
        .await
        .map_err(AppError::from)?;

    // Probe file for video stream
    let probe_output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=codec_type",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(&input_path)
        .output()
        .await
        .map_err(AppError::from)?;
    let has_video = !probe_output.stdout.is_empty();

    if is_video_note && !has_video {
        bot.delete_message(chat_id, status.id).await.ok();
        bot.send_message(chat_id, i18n::t(&lang, "commands.video_note_requires_video"))
            .await
            .ok();
        tokio::fs::remove_file(&input_path).await.ok();
        return Ok(());
    }

    let base_filter_av = build_cut_filter(&adjusted_segments, has_video, true);
    let base_filter_v = if has_video {
        build_cut_filter(&adjusted_segments, true, false)
    } else {
        String::new()
    };

    // Apply speed modification if requested
    let (filter_av, filter_v, map_v_label, map_a_label, crf) = if is_video_note {
        let video_note_post = "scale=640:640:force_original_aspect_ratio=increase,crop=640:640,format=yuv420p";

        if let Some(spd) = speed {
            // Apply speed change: setpts for video, atempo for audio
            let setpts_factor = 1.0 / spd;
            let atempo_filter = if spd > 2.0 {
                format!("atempo=2.0,atempo={}", spd / 2.0)
            } else if spd < 0.5 {
                format!("atempo=0.5,atempo={}", spd / 0.5)
            } else {
                format!("atempo={}", spd)
            };

            (
                format!(
                    "{base_filter_av};[v]setpts={}*PTS,{video_note_post}[vout];[a]{atempo_filter}[aout]",
                    setpts_factor
                ),
                format!(
                    "{base_filter_v};[v]setpts={}*PTS,{video_note_post}[vout]",
                    setpts_factor
                ),
                "[vout]",
                "[aout]",
                "28",
            )
        } else {
            (
                format!("{base_filter_av};[v]{video_note_post}[vout]"),
                format!("{base_filter_v};[v]{video_note_post}[vout]"),
                "[vout]",
                "[a]",
                "28",
            )
        }
    } else if is_ringtone {
        let atempo_filter = if let Some(spd) = speed {
            if spd > 2.0 {
                format!("atempo=2.0,atempo={}", spd / 2.0)
            } else if spd < 0.5 {
                format!("atempo=0.5,atempo={}", spd / 0.5)
            } else {
                format!("atempo={}", spd)
            }
        } else {
            "atempo=1.0".to_string()
        };
        // If !has_video, base_filter_av outputs only [a]. If has_video, [v][a].
        // Ringtone uses input [a] for atempo.
        // We need to match output of base_filter

        (
            format!("{base_filter_av};{}[a]{atempo_filter}[aout]", ""), // standard [a] is output by build_cut_filter
            String::new(),
            "[v]",
            "[aout]",
            "23",
        )
    } else if let Some(spd) = speed {
        let setpts_factor = 1.0 / spd;
        let atempo_filter = if spd > 2.0 {
            format!("atempo=2.0,atempo={}", spd / 2.0)
        } else if spd < 0.5 {
            format!("atempo=0.5,atempo={}", spd / 0.5)
        } else {
            format!("atempo={}", spd)
        };

        if has_video {
            (
                format!(
                    "{base_filter_av};[v]setpts={}*PTS[vout];[a]{atempo_filter}[aout]",
                    setpts_factor
                ),
                format!("{base_filter_v};[v]setpts={}*PTS[vout]", setpts_factor),
                "[vout]",
                "[aout]",
                "23",
            )
        } else {
            (
                format!("{base_filter_av};[a]{atempo_filter}[aout]"),
                String::new(),
                "",
                "[aout]",
                "23",
            )
        }
    } else {
        (base_filter_av, base_filter_v, "[v]", "[a]", "23")
    };

    log::info!("🎬 Starting ffmpeg with filter: {}", filter_av);
    log::info!("🎬 Input: {:?}, Output: {:?}", input_path, output_path);

    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-hide_banner")
        .arg("-loglevel")
        .arg("info")
        .arg("-i")
        .arg(&input_path);

    if is_ringtone {
        // For ringtone we only care about audio
        cmd.arg("-filter_complex")
            .arg(&filter_av)
            .arg("-map")
            .arg(map_a_label)
            .arg("-c:a")
            .arg("aac")
            .arg("-b:a")
            .arg("192k")
            .arg("-f")
            .arg("ipod");
    } else {
        cmd.arg("-filter_complex").arg(&filter_av);
        if has_video {
            cmd.arg("-map").arg(map_v_label);
        }
        cmd.arg("-map").arg(map_a_label);

        if has_video {
            cmd.arg("-c:v")
                .arg("libx264")
                .arg("-preset")
                .arg("fast")
                .arg("-crf")
                .arg(crf);
        }
        cmd.arg("-c:a")
            .arg("aac")
            .arg("-b:a")
            .arg("192k")
            .arg("-movflags")
            .arg("+faststart");
    }

    let output = cmd.arg("-y").arg(&output_path).output().await.map_err(AppError::from)?;

    log::info!("✅ ffmpeg processing completed with status: {}", output.status);

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let retry_output = Command::new("ffmpeg")
            .arg("-hide_banner")
            .arg("-loglevel")
            .arg("error")
            .arg("-i")
            .arg(&input_path)
            .arg("-filter_complex")
            .arg(&filter_v)
            .arg("-map")
            .arg(map_v_label)
            .arg("-c:v")
            .arg("libx264")
            .arg("-preset")
            .arg("fast")
            .arg("-crf")
            .arg(crf)
            .arg("-movflags")
            .arg("+faststart")
            .arg("-y")
            .arg(&output_path)
            .output()
            .await
            .map_err(AppError::from)?;

        if !retry_output.status.success() {
            let stderr2 = String::from_utf8_lossy(&retry_output.stderr);
            bot.delete_message(chat_id, status.id).await.ok();
            let mut args = FluentArgs::new();
            args.set("stderr", stderr.to_string());
            args.set("stderr2", stderr2.to_string());
            bot.send_message(chat_id, i18n::t_args(&lang, "commands.ffmpeg_error_dual", &args))
                .await
                .ok();
            tokio::fs::remove_file(&input_path).await.ok();
            tokio::fs::remove_file(&output_path).await.ok();
            return Ok(());
        }
    }

    let file_size = tokio::fs::metadata(&output_path)
        .await
        .map(|m| m.len() as i64)
        .unwrap_or(0);

    let (output_kind, clip_title) = if is_video_note {
        ("video_note", format!("{} [circle {}]", base_title, segments_text))
    } else if is_ringtone {
        ("ringtone", format!("{} [ringtone {}]", base_title, segments_text))
    } else {
        ("clip", format!("{} [cut {}]", base_title, segments_text))
    };

    // Check output file before sending
    if !output_path.exists() {
        log::error!("❌ Output file does not exist: {:?}", output_path);
        bot.delete_message(chat_id, status.id).await.ok();
        bot.send_message(chat_id, i18n::t(&lang, "commands.output_file_missing"))
            .await
            .ok();
        tokio::fs::remove_file(&input_path).await.ok();
        return Ok(());
    }

    let output_size = tokio::fs::metadata(&output_path)
        .await
        .ok()
        .map(|m| m.len())
        .unwrap_or(0);
    log::info!(
        "📤 Sending {} (size: {} bytes, duration: {}s)",
        if is_video_note { "video note" } else { "video" },
        output_size,
        actual_total_len
    );

    let sent = if is_video_note {
        match bot
            .send_video_note(chat_id, teloxide::types::InputFile::file(output_path.clone()))
            .duration(actual_total_len.max(1) as u32)
            .length(640)
            .await
        {
            Ok(m) => m,
            Err(e) => {
                log::error!("❌ Failed to send video note: {}", e);
                bot.delete_message(chat_id, status.id).await.ok();
                let msg = if e.to_string().to_lowercase().contains("file is too big") {
                    i18n::t(&lang, "commands.video_note_too_big")
                } else {
                    let mut args = FluentArgs::new();
                    args.set("error", e.to_string());
                    i18n::t_args(&lang, "commands.video_note_send_failed", &args)
                };
                bot.send_message(chat_id, msg).await.ok();
                tokio::fs::remove_file(&input_path).await.ok();
                tokio::fs::remove_file(&output_path).await.ok();
                return Ok(());
            }
        }
    } else if is_ringtone {
        let lang = i18n::user_lang_from_pool(&db_pool, chat_id.0);
        let instructions = i18n::t(&lang, "history.iphone_ringtone_instructions");
        match bot
            .send_document(chat_id, teloxide::types::InputFile::file(output_path.clone()))
            .caption(format!("{}\n\n{}", clip_title, instructions))
            .parse_mode(ParseMode::MarkdownV2)
            .await
        {
            Ok(m) => m,
            Err(e) => {
                bot.delete_message(chat_id, status.id).await.ok();
                let mut args = FluentArgs::new();
                args.set("error", e.to_string());
                bot.send_message(chat_id, i18n::t_args(&lang, "commands.ringtone_send_failed", &args))
                    .await
                    .ok();
                tokio::fs::remove_file(&input_path).await.ok();
                tokio::fs::remove_file(&output_path).await.ok();
                return Ok(());
            }
        }
    } else if has_video {
        match bot
            .send_video(chat_id, teloxide::types::InputFile::file(output_path.clone()))
            .caption(&clip_title)
            .await
        {
            Ok(m) => m,
            Err(e) => {
                bot.delete_message(chat_id, status.id).await.ok();
                let mut args = FluentArgs::new();
                args.set("error", e.to_string());
                bot.send_message(chat_id, i18n::t_args(&lang, "commands.clip_send_failed", &args))
                    .await
                    .ok();
                tokio::fs::remove_file(&input_path).await.ok();
                tokio::fs::remove_file(&output_path).await.ok();
                return Ok(());
            }
        }
    } else {
        match bot
            .send_audio(chat_id, teloxide::types::InputFile::file(output_path.clone()))
            .caption(&clip_title)
            .await
        {
            Ok(m) => m,
            Err(e) => {
                bot.delete_message(chat_id, status.id).await.ok();
                let mut args = FluentArgs::new();
                args.set("error", e.to_string());
                bot.send_message(chat_id, i18n::t_args(&lang, "commands.audio_send_failed", &args))
                    .await
                    .ok();
                tokio::fs::remove_file(&input_path).await.ok();
                tokio::fs::remove_file(&output_path).await.ok();
                return Ok(());
            }
        }
    };

    if is_video_note {
        bot.send_message(chat_id, clip_title.clone()).await.ok();
    }

    if !original_url.trim().is_empty() {
        bot.send_message(chat_id, original_url.clone()).await.ok();
    }
    bot.delete_message(chat_id, status.id).await.ok();

    let sent_file_id = if is_video_note {
        sent.video_note().map(|v| v.file.id.0.clone())
    } else if is_ringtone {
        sent.document().map(|d| d.file.id.0.clone())
    } else {
        sent.video()
            .map(|v| v.file.id.0.clone())
            .or_else(|| sent.document().map(|d| d.file.id.0.clone()))
            .or_else(|| sent.audio().map(|a| a.file.id.0.clone()))
    };

    if let Some(fid) = sent_file_id {
        let segments_json = serde_json::to_string(&segments).unwrap_or_else(|_| "[]".to_string());
        let _ = db::create_cut(
            &conn,
            chat_id.0,
            &original_url,
            &session.source_kind,
            session.source_id,
            output_kind,
            &segments_json,
            &segments_text,
            &clip_title,
            Some(&fid),
            Some(file_size),
            Some(actual_total_len.max(1)),
            video_quality.as_deref(),
        );
    }

    tokio::fs::remove_file(&input_path).await.ok();
    tokio::fs::remove_file(&output_path).await.ok();

    Ok(())
}

async fn process_audio_cut(
    bot: Bot,
    db_pool: Arc<DbPool>,
    chat_id: ChatId,
    session: crate::download::audio_effects::AudioEffectSession,
    segments: Vec<CutSegment>,
    segments_text: String,
) -> Result<(), AppError> {
    use tokio::process::Command;

    let lang = i18n::user_lang_from_pool(&db_pool, chat_id.0);
    let total_len: i64 = segments.iter().map(|s| (s.end_secs - s.start_secs).max(0)).sum();
    if total_len <= 0 {
        bot.send_message(chat_id, i18n::t(&lang, "commands.empty_cut"))
            .await
            .ok();
        return Ok(());
    }

    let input_path = std::path::PathBuf::from(&session.original_file_path);
    if !input_path.exists() {
        bot.send_message(chat_id, i18n::t(&lang, "commands.audio_source_missing"))
            .await
            .ok();
        return Ok(());
    }

    let mut args = FluentArgs::new();
    args.set("segments", segments_text.as_str());
    let status = bot
        .send_message(chat_id, i18n::t_args(&lang, "commands.audio_cut_processing", &args))
        .await?;

    let temp_dir = std::path::PathBuf::from(crate::core::config::TEMP_FILES_DIR.as_str()).join("doradura_audio_cut");
    tokio::fs::create_dir_all(&temp_dir).await.ok();

    let output_path = temp_dir.join(format!("cut_audio_{}_{}.mp3", chat_id.0, uuid::Uuid::new_v4()));
    let filter = build_cut_filter(&segments, false, true);

    let output = Command::new("ffmpeg")
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("info")
        .arg("-i")
        .arg(&input_path)
        .arg("-filter_complex")
        .arg(&filter)
        .arg("-map")
        .arg("[a]")
        .arg("-q:a")
        .arg("0")
        .arg("-y")
        .arg(&output_path)
        .output()
        .await
        .map_err(AppError::from)?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bot.delete_message(chat_id, status.id).await.ok();
        let mut args = FluentArgs::new();
        args.set("stderr", stderr.to_string());
        bot.send_message(chat_id, i18n::t_args(&lang, "commands.ffmpeg_error_single", &args))
            .await
            .ok();
        tokio::fs::remove_file(&output_path).await.ok();
        return Ok(());
    }

    if !output_path.exists() {
        bot.delete_message(chat_id, status.id).await.ok();
        bot.send_message(chat_id, i18n::t(&lang, "commands.output_file_missing"))
            .await
            .ok();
        return Ok(());
    }

    let file_size = tokio::fs::metadata(&output_path).await.map(|m| m.len()).unwrap_or(0);
    if file_size > config::validation::max_audio_size_bytes() {
        bot.delete_message(chat_id, status.id).await.ok();
        bot.send_message(chat_id, i18n::t(&lang, "commands.audio_too_large_for_telegram"))
            .await
            .ok();
        tokio::fs::remove_file(&output_path).await.ok();
        return Ok(());
    }

    let caption = format!("{} [cut {}]", session.title, segments_text);
    let conn = db::get_connection(&db_pool)?;
    let send_as_document = db::get_user_send_audio_as_document(&conn, chat_id.0).unwrap_or(0);

    let send_res = if send_as_document == 0 {
        bot.send_audio(chat_id, teloxide::types::InputFile::file(output_path.clone()))
            .caption(caption)
            .duration(total_len.max(1) as u32)
            .await
    } else {
        bot.send_document(chat_id, teloxide::types::InputFile::file(output_path.clone()))
            .caption(caption)
            .await
    };

    if let Err(e) = send_res {
        bot.delete_message(chat_id, status.id).await.ok();
        let mut args = FluentArgs::new();
        args.set("error", e.to_string());
        bot.send_message(chat_id, i18n::t_args(&lang, "commands.audio_send_failed", &args))
            .await
            .ok();
        tokio::fs::remove_file(&output_path).await.ok();
        return Ok(());
    }

    bot.delete_message(chat_id, status.id).await.ok();
    tokio::fs::remove_file(&output_path).await.ok();
    Ok(())
}

fn build_cut_filter(segments: &[CutSegment], with_video: bool, with_audio: bool) -> String {
    let mut parts = Vec::new();
    for (i, seg) in segments.iter().enumerate() {
        if with_video {
            parts.push(format!(
                "[0:v]trim=start={}:end={},setpts=PTS-STARTPTS[v{}]",
                seg.start_secs, seg.end_secs, i
            ));
        }
        if with_audio {
            parts.push(format!(
                "[0:a]atrim=start={}:end={},asetpts=PTS-STARTPTS[a{}]",
                seg.start_secs, seg.end_secs, i
            ));
        }
    }

    let n = segments.len();
    let mut concat_inputs = String::new();
    for i in 0..n {
        if with_video {
            concat_inputs.push_str(&format!("[v{}]", i));
        }
        if with_audio {
            concat_inputs.push_str(&format!("[a{}]", i));
        }
    }

    let v_count = if with_video { 1 } else { 0 };
    let a_count = if with_audio { 1 } else { 0 };
    let output_labels = format!(
        "{}{}",
        if with_video { "[v]" } else { "" },
        if with_audio { "[a]" } else { "" }
    );

    parts.push(format!(
        "{}concat=n={}:v={}:a={}{}",
        concat_inputs, n, v_count, a_count, output_labels
    ));

    parts.join(";")
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
pub async fn handle_info_command(bot: Bot, msg: Message, db_pool: Arc<DbPool>) -> ResponseResult<()> {
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
            let lang = i18n::user_lang_from_pool(&db_pool, msg.chat.id.0);
            match bot
                .send_message(msg.chat.id, i18n::t(&lang, "commands.info_usage"))
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
                let lang = i18n::user_lang_from_pool(&db_pool, msg.chat.id.0);
                match bot
                    .send_message(msg.chat.id, i18n::t(&lang, "commands.invalid_url"))
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
        let lang = i18n::user_lang_from_pool(&db_pool, msg.chat.id.0);
        let processing_msg = match bot
            .send_message(msg.chat.id, i18n::t(&lang, "commands.processing"))
            .await
        {
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

pub async fn handle_downsub_command(
    bot: Bot,
    msg: Message,
    db_pool: Arc<DbPool>,
    downsub_gateway: Arc<DownsubGateway>,
) -> ResponseResult<()> {
    let lang = i18n::user_lang_from_pool(&db_pool, msg.chat.id.0);
    let usage_text = i18n::t(&lang, "commands.downsub_usage");
    let disabled_text = i18n::t(&lang, "commands.downsub_disabled");

    let message_text = match msg.text() {
        Some(text) => text.trim(),
        None => {
            bot.send_message(msg.chat.id, usage_text.clone()).await?;
            return Ok(());
        }
    };

    let tokens: Vec<&str> = message_text.split_whitespace().collect();
    if tokens.len() < 2 {
        bot.send_message(msg.chat.id, usage_text.clone()).await?;
        return Ok(());
    }

    let action = tokens[1].to_lowercase();
    let options = parse_downsub_options(&tokens[3..]);

    match action.as_str() {
        "summary" => {
            if tokens.len() < 3 {
                bot.send_message(msg.chat.id, usage_text.clone()).await?;
                return Ok(());
            }

            let url = tokens[2].to_string();
            match downsub_gateway
                .summarize_url(msg.chat.id.0, options.phone.clone(), url, options.language.clone())
                .await
            {
                Ok(summary) => {
                    let mut response = String::new();
                    response.push_str(&i18n::t(&lang, "commands.downsub_summary_header"));
                    response.push('\n');
                    response.push_str(&summary.summary);

                    if !summary.highlights.is_empty() {
                        response.push_str("\n\nHighlights:\n");
                        for highlight in summary.highlights {
                            response.push_str("- ");
                            response.push_str(&highlight);
                            response.push('\n');
                        }
                    }

                    if !summary.sections.is_empty() {
                        for section in summary.sections {
                            if let Some(title) = section.title {
                                response.push_str("\n*");
                                response.push_str(&title);
                                response.push_str("*\n");
                            }
                            response.push_str(&section.text);
                            response.push('\n');
                        }
                    }

                    bot.send_message(msg.chat.id, response).await?;
                }
                Err(DownsubError::Unavailable) => {
                    bot.send_message(msg.chat.id, disabled_text.clone()).await?;
                }
                Err(err) => {
                    log::warn!("Downsub summary request failed: {}", err);
                    let mut args = FluentArgs::new();
                    args.set("error", err.to_string());
                    bot.send_message(msg.chat.id, i18n::t_args(&lang, "commands.downsub_error", &args))
                        .await?;
                }
            }
        }
        "subtitles" => {
            if tokens.len() < 3 {
                bot.send_message(msg.chat.id, usage_text.clone()).await?;
                return Ok(());
            }

            let url = tokens[2].to_string();
            match downsub_gateway
                .fetch_subtitles(
                    msg.chat.id.0,
                    options.phone.clone(),
                    url,
                    options.format.clone(),
                    options.language.clone(),
                )
                .await
            {
                Ok(result) => {
                    let segments_count = result.segments.len() as i64;
                    let format_value = if result.format.is_empty() {
                        "srt".to_string()
                    } else {
                        result.format.clone()
                    };
                    let extension = format_value.split('.').next().unwrap_or("srt").to_lowercase();
                    let file_name = format!("downsub_subtitles.{}", extension);
                    let bytes = result.raw_subtitles.into_bytes();

                    bot.send_document(msg.chat.id, InputFile::memory(bytes).file_name(file_name))
                        .await?;

                    let mut args = FluentArgs::new();
                    args.set("format", format_value.clone());
                    args.set("count", segments_count);
                    let text = i18n::t_args(&lang, "commands.downsub_subtitles_sent", &args);
                    bot.send_message(msg.chat.id, text).await?;
                }
                Err(DownsubError::Unavailable) => {
                    bot.send_message(msg.chat.id, disabled_text.clone()).await?;
                }
                Err(err) => {
                    log::warn!("Downsub subtitles request failed: {}", err);
                    let mut args = FluentArgs::new();
                    args.set("error", err.to_string());
                    bot.send_message(msg.chat.id, i18n::t_args(&lang, "commands.downsub_error", &args))
                        .await?;
                }
            }
        }
        _ => {
            bot.send_message(msg.chat.id, usage_text.clone()).await?;
        }
    }

    Ok(())
}

#[derive(Clone, Default)]
struct DownsubOptions {
    language: Option<String>,
    format: Option<String>,
    phone: Option<String>,
}

fn parse_downsub_options(tokens: &[&str]) -> DownsubOptions {
    let mut options = DownsubOptions::default();

    for &token in tokens {
        if let Some((key, value)) = token.split_once('=') {
            match key.to_lowercase().as_str() {
                "lang" | "language" => {
                    options.language = Some(value.to_string());
                }
                "format" => {
                    options.format = Some(value.to_string());
                }
                "phone" => {
                    options.phone = Some(value.to_string());
                }
                _ => {}
            }
        }
    }

    options
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
