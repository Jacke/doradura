pub mod circle;
pub mod info;
pub mod loop_to_audio;
pub mod subtitles;

pub use circle::*;
pub use info::*;
pub use subtitles::*;

use crate::core::alerts::AlertManager;
use crate::core::config;
use crate::core::error::AppError;
use crate::core::metrics;
use crate::core::rate_limiter::RateLimiter;
use crate::core::utils::pluralize_seconds;
use crate::download::queue::DownloadQueue;
use crate::i18n;
use crate::storage::db::{self, DbPool, OutputKind, SourceKind};
use crate::storage::SharedStorage;
use crate::telegram::preview::{get_preview_metadata, get_preview_metadata_with_time_range, send_preview};
use crate::telegram::Bot;
use crate::telegram::BotExt;
use lazy_regex::{lazy_regex, Lazy, Regex};
use std::sync::Arc;
use teloxide::prelude::*;
use url::Url;

const PREVIEW_CONTEXT_TTL_SECS: i64 = 3600;

/// Cached regex for matching URLs. Compiled once at startup and reused.
static URL_REGEX: Lazy<Regex> = lazy_regex!(r"https?://[^\s]+");

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
    shared_storage: &Arc<SharedStorage>,
) -> ResponseResult<bool> {
    let _ = db_pool;
    let lang = i18n::user_lang_from_storage(shared_storage, msg.chat.id.0).await;
    match rate_limiter.check_and_update(msg.chat.id, plan).await {
        Ok(Some(remaining_time)) => {
            metrics::record_rate_limit_hit(plan);
            let remaining_seconds = remaining_time.as_secs();
            let unit = if lang.language.as_str() == "ru" {
                pluralize_seconds(remaining_seconds).to_string()
            } else {
                i18n::t(&lang, "common.seconds")
            };
            let args = doracore::fluent_args!("time" => remaining_seconds as i64, "unit" => unit);
            let text = i18n::t_args(&lang, "commands.rate_limited_with_eta", &args);
            bot.send_message(msg.chat.id, text).await?;
            Ok(false)
        }
        Ok(None) => Ok(true),
        Err(e) => {
            log::error!("Rate limiter check failed for {}: {}", msg.chat.id.0, e);
            let text = i18n::t(&lang, "commands.rate_limited");
            bot.send_message(msg.chat.id, text).await?;
            Ok(false)
        }
    }
}

/// If the user has an active cookies/IG-cookies upload session AND a document
/// is attached, dispatch the upload and swallow the message. Returns `true`
/// when handled (caller should return early), `false` otherwise.
async fn try_intercept_document_upload(
    bot: &Bot,
    msg: &Message,
    db_pool: &Arc<DbPool>,
    shared_storage: &Arc<SharedStorage>,
) -> ResponseResult<bool> {
    let Some(document) = msg.document() else {
        return Ok(false);
    };
    metrics::record_message_type("document");
    let Some(user) = msg.from.as_ref() else {
        return Ok(false);
    };
    let user_id = user.id.0 as i64;
    if let Ok(Some(_session)) = shared_storage.get_active_cookies_upload_session(user_id).await {
        if let Err(e) = crate::telegram::handle_cookies_file_upload(
            db_pool.clone(),
            shared_storage.clone(),
            bot,
            msg.chat.id,
            user_id,
            document,
        )
        .await
        {
            log::error!("Failed to handle cookies file upload: {}", e);
        }
        return Ok(true);
    }
    if let Ok(Some(_session)) = shared_storage.get_active_ig_cookies_upload_session(user_id).await {
        if let Err(e) = crate::telegram::handle_ig_cookies_file_upload(
            db_pool.clone(),
            shared_storage.clone(),
            bot,
            msg.chat.id,
            user_id,
            document,
        )
        .await
        {
            log::error!("Failed to handle IG cookies file upload: {}", e);
        }
        return Ok(true);
    }
    Ok(false)
}

/// If an active `VideoClipSession` is in audio-intake mode (VideoNote or Loop)
/// and the incoming message carries audio, route it to the session. For
/// VideoNote the audio is stored and the user is prompted for a time range;
/// for Loop the audio triggers immediate processing in a spawned task.
/// Returns `true` when handled, `false` when the message should fall through.
async fn try_intercept_video_clip_audio(
    bot: &Bot,
    msg: &Message,
    shared_storage: &Arc<SharedStorage>,
    db_pool: &Arc<DbPool>,
    lang: &unic_langid::LanguageIdentifier,
) -> ResponseResult<bool> {
    let Ok(Some(mut session)) = shared_storage.get_active_video_clip_session(msg.chat.id.0).await else {
        return Ok(false);
    };
    if !matches!(session.output_kind, OutputKind::VideoNote | OutputKind::Loop) {
        return Ok(false);
    }
    let audio_file_id = msg
        .audio()
        .map(|a| a.file.id.0.clone())
        .or_else(|| msg.voice().map(|v| v.file.id.0.clone()))
        .or_else(|| {
            msg.document().and_then(|d| {
                d.mime_type.as_ref().and_then(|m| {
                    if m.type_() == mime::AUDIO {
                        Some(d.file.id.0.clone())
                    } else {
                        None
                    }
                })
            })
        });

    let Some(file_id) = audio_file_id else {
        return Ok(false);
    };
    match session.output_kind {
        OutputKind::VideoNote => {
            session.custom_audio_file_id = Some(file_id);
            let _ = shared_storage.upsert_video_clip_session(&session).await;
            bot.send_message(msg.chat.id, "🎵 Custom audio saved! Now send the time range.")
                .await
                .ok();
            Ok(true)
        }
        OutputKind::Loop => {
            session.custom_audio_file_id = Some(file_id.clone());
            let _ = shared_storage.delete_video_clip_session_by_user(msg.chat.id.0).await;

            let bot_c = bot.clone();
            let shared_storage_c = Arc::clone(shared_storage);
            let db_pool_c = Arc::clone(db_pool);
            let chat_id = msg.chat.id;
            tokio::spawn(async move {
                if let Err(e) = crate::telegram::commands::loop_to_audio::process_loop_to_audio(
                    bot_c,
                    chat_id,
                    session,
                    file_id,
                    db_pool_c,
                    shared_storage_c,
                )
                .await
                {
                    log::warn!("process_loop_to_audio failed: {}", e);
                }
            });

            bot.send_message(msg.chat.id, i18n::t(lang, "loop.processing"))
                .await
                .ok();
            Ok(true)
        }
        _ => Ok(false),
    }
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
    download_queue: Arc<DownloadQueue>,
    rate_limiter: Arc<RateLimiter>,
    db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
    alert_manager: Option<Arc<AlertManager>>,
) -> ResponseResult<Option<db::User>> {
    let lang = i18n::user_lang_from_pool_with_fallback(
        &db_pool,
        msg.chat.id.0,
        msg.from.as_ref().and_then(|user| user.language_code.as_deref()),
    );

    // Document upload (cookies / IG cookies): intercept if active session.
    if try_intercept_document_upload(&bot, &msg, &db_pool, &shared_storage).await? {
        return Ok(None);
    }

    // Check if user is blocked (skip for admins)
    {
        let user_id = msg.chat.id.0;
        if !crate::telegram::admin::is_admin(user_id) {
            match shared_storage.get_user(user_id).await {
                Ok(Some(user)) if user.is_blocked => return Ok(None),
                Ok(_) => {}
                Err(e) => {
                    log::error!("Failed to check blocked status for {}: {}", user_id, e);
                    return Ok(None);
                }
            }
        }
    }

    // Ignore replies to bot's own messages (don't show "no links" for them)
    if let Some(reply) = msg.reply_to_message() {
        if reply.from.as_ref().is_some_and(|u| u.is_bot) {
            return Ok(None);
        }
    }

    // VideoClipSession audio-intake (VideoNote: capture custom audio; Loop: kick off processing).
    if try_intercept_video_clip_audio(&bot, &msg, &shared_storage, &db_pool, &lang).await? {
        return Ok(None);
    }

    if let Some(text) = msg.text() {
        log::debug!("handle_message: {:?}", text);
        if text.starts_with("/start") || text.starts_with("/help") {
            return Ok(None);
        }

        // Admin search intercept
        if !text.trim().starts_with('/')
            && crate::telegram::admin::is_admin(msg.chat.id.0)
            && crate::telegram::menu::admin_users::is_admin_searching(&shared_storage, msg.chat.id.0).await
        {
            if let Err(e) =
                crate::telegram::menu::admin_users::handle_admin_search(&bot, msg.chat.id, &shared_storage, text.trim())
                    .await
            {
                log::error!("Admin search error: {}", e);
            }
            return Ok(None);
        }

        // New-category sessions (from downloads:newcat callback)
        if !text.trim().starts_with('/') {
            if let Ok(Some(download_id)) = shared_storage.get_active_new_category_session(msg.chat.id.0).await {
                let name = text.trim();
                if name.is_empty() || name.len() > 64 {
                    bot.send_message(msg.chat.id, "❌ Category name must be 1–64 characters")
                        .await
                        .ok();
                } else {
                    // Truncate to 32 chars for callback data safety
                    let name: String = name.chars().take(32).collect();
                    if let Err(e) = shared_storage.create_user_category(msg.chat.id.0, &name).await {
                        log::error!("Failed to create user category '{}': {}", name, e);
                        bot.send_message(msg.chat.id, "❌ Failed to create category. Please try again.")
                            .await
                            .ok();
                    } else if let Err(e) = shared_storage
                        .set_download_category(msg.chat.id.0, download_id, Some(&name))
                        .await
                    {
                        log::error!(
                            "Failed to assign category '{}' to download {}: {}",
                            name,
                            download_id,
                            e
                        );
                        bot.send_message(
                            msg.chat.id,
                            "❌ Category created but failed to assign. Please try again.",
                        )
                        .await
                        .ok();
                    } else {
                        let _ = shared_storage.delete_new_category_session(msg.chat.id.0).await;
                        bot.send_message(msg.chat.id, format!("✅ Category «{}» created and assigned", name))
                            .await
                            .ok();
                    }
                }
                return Ok(None);
            }
        }

        // Audio cut sessions (from "Cut Audio" button)
        if !text.trim().starts_with('/') {
            if let Ok(Some(session)) = shared_storage.get_active_audio_cut_session(msg.chat.id.0).await {
                let trimmed = text.trim();
                if is_cancel_text(trimmed) {
                    let _ = shared_storage.delete_audio_cut_session_by_user(msg.chat.id.0).await;
                    bot.send_message(msg.chat.id, i18n::t(&lang, "commands.audio_cut_cancelled"))
                        .await
                        .ok();
                    return Ok(None);
                }

                let audio_session = match shared_storage.get_audio_effect_session(&session.audio_session_id).await {
                    Ok(Some(audio_session)) => audio_session,
                    Ok(None) => {
                        let _ = shared_storage.delete_audio_cut_session_by_user(msg.chat.id.0).await;
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
                    let _ = shared_storage.delete_audio_cut_session_by_user(msg.chat.id.0).await;
                    bot.send_message(msg.chat.id, i18n::t(&lang, "commands.audio_session_expired"))
                        .await
                        .ok();
                    return Ok(None);
                }

                let audio_duration = Some(audio_session.duration as i64);
                if let Some((segments, segments_text)) = parse_audio_segments_spec(trimmed, audio_duration) {
                    let _ = shared_storage.delete_audio_cut_session_by_user(msg.chat.id.0).await;

                    let bot_clone = bot.clone();
                    let db_pool_clone = db_pool.clone();
                    let shared_storage_clone = shared_storage.clone();
                    let chat_id = msg.chat.id;
                    tokio::spawn(async move {
                        if let Err(e) = process_audio_cut(
                            bot_clone,
                            db_pool_clone,
                            shared_storage_clone,
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

        // Video clip sessions (from /downloads or /cuts -> ✂️ Clip)
        if !text.trim().starts_with('/') {
            if let Ok(Some(session)) = shared_storage.get_active_video_clip_session(msg.chat.id.0).await {
                let trimmed = text.trim();
                if is_cancel_text(trimmed) {
                    let _ = shared_storage.delete_video_clip_session_by_user(msg.chat.id.0).await;
                    let cancel_key = if session.output_kind == OutputKind::Loop {
                        "loop.cancelled"
                    } else {
                        "commands.video_clip_cancelled"
                    };
                    bot.send_message(msg.chat.id, i18n::t(&lang, cancel_key)).await.ok();
                    return Ok(None);
                }

                // Loop sessions accept ONLY audio uploads (handled earlier in the
                // intercept above) or cancel. Text is not valid input — re-prompt.
                if session.output_kind == OutputKind::Loop {
                    bot.send_message(msg.chat.id, i18n::t(&lang, "loop.send_audio_first"))
                        .await
                        .ok();
                    return Ok(None);
                }

                let video_duration = match session.source_kind {
                    SourceKind::Download => shared_storage
                        .get_download_history_entry(msg.chat.id.0, session.source_id)
                        .await
                        .ok()
                        .flatten()
                        .and_then(|d| d.duration),
                    SourceKind::Cut => shared_storage
                        .get_cut_entry(msg.chat.id.0, session.source_id)
                        .await
                        .ok()
                        .flatten()
                        .and_then(|c| c.duration),
                };

                if let Some((segments, segments_text, speed)) = parse_segments_spec(trimmed, video_duration) {
                    let _ = shared_storage.delete_video_clip_session_by_user(msg.chat.id.0).await;

                    let bot_clone = bot.clone();
                    let db_pool_clone = db_pool.clone();
                    let chat_id = msg.chat.id;
                    tokio::spawn(async move {
                        if let Err(e) = process_video_clip(
                            bot_clone,
                            db_pool_clone,
                            shared_storage.clone(),
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
                    let extra_note = if session.output_kind == OutputKind::VideoNote {
                        "\n\n💡 If duration exceeds 60 seconds \\(Telegram limit for video notes\\), video will be automatically trimmed\\."
                    } else {
                        ""
                    };
                    bot.send_md(
                        msg.chat.id,
                        format!(
                            "❌ Couldn't parse intervals\\.\n\nSend in format `mm:ss-mm:ss` or `hh:mm:ss-hh:mm:ss`\\.\nMultiple separated by commas\\.\nExample: `00:10-00:25, 01:00-01:10`\n\nOr commands: `full`, `first30`, `last30`, `middle30`\\.\n\n💡 You can add speed: `first30 2x`, `full 1\\.5x`\\.\n\nOr type `cancel`\\.{extra_note}",
                        ),
                    )
                    .await
                    .ok();
                    return Ok(None);
                }
            }
        }

        // Check if user is waiting to provide feedback
        if crate::telegram::feedback::is_waiting_for_feedback(&shared_storage, msg.chat.id.0).await {
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
                &shared_storage,
            )
            .await;

            // Send confirmation to user and return to main menu
            let _ =
                crate::telegram::feedback::send_feedback_confirmation(&bot, msg.chat.id, &lang, &shared_storage).await;
            let _ =
                crate::telegram::show_enhanced_main_menu(&bot, msg.chat.id, db_pool.clone(), shared_storage.clone())
                    .await;

            return Ok(None);
        }

        // Check if user is waiting for playlist name input
        if crate::telegram::menu::playlist::is_waiting_for_playlist_name(&shared_storage, msg.chat.id.0).await {
            crate::telegram::menu::playlist::handle_playlist_name_input(
                &bot,
                msg.chat.id,
                db_pool.clone(),
                shared_storage.clone(),
                text,
            )
            .await;
            return Ok(None);
        }

        // Check if user is waiting for import URL input
        if let Some(pl_id) =
            crate::telegram::menu::playlist::get_import_playlist_id(&shared_storage, msg.chat.id.0).await
        {
            let text_lower = text.trim().to_lowercase();
            if text_lower == "cancel" {
                crate::telegram::menu::playlist::clear_import_url_session(&shared_storage, msg.chat.id.0).await;
                let _ = bot.send_message(msg.chat.id, "Cancelled.").await;
                return Ok(None);
            }
            crate::telegram::menu::playlist::clear_import_url_session(&shared_storage, msg.chat.id.0).await;
            // Handle import in background
            let bot_clone = bot.clone();
            let shared_storage_clone = shared_storage.clone();
            let url_text = text.trim().to_string();
            tokio::spawn(async move {
                crate::download::playlist_import::handle_import_url(
                    &bot_clone,
                    msg.chat.id,
                    &url_text,
                    pl_id,
                    shared_storage_clone,
                )
                .await;
            });
            return Ok(None);
        }

        // Check if user is waiting for vault setup
        if crate::telegram::menu::vault::is_waiting_for_vault_setup(&shared_storage, msg.chat.id.0).await {
            let bot_clone = bot.clone();
            let db_pool_clone = db_pool.clone();
            let shared_storage_clone = shared_storage.clone();
            let msg_clone = msg.clone();
            tokio::spawn(async move {
                crate::telegram::menu::vault::handle_vault_setup_input(
                    &bot_clone,
                    &msg_clone,
                    &db_pool_clone,
                    &shared_storage_clone,
                )
                .await;
            });
            return Ok(None);
        }

        // Check if user is waiting for playlist integrations import URL
        if crate::telegram::menu::playlist_integrations::is_waiting_for_import_url(&shared_storage, msg.chat.id.0).await
        {
            let bot_clone = bot.clone();
            let db_pool_clone = db_pool.clone();
            let shared_storage_clone = shared_storage.clone();
            let url_text = text.trim().to_string();
            tokio::spawn(async move {
                crate::telegram::menu::playlist_integrations::handle_import_url_input(
                    &bot_clone,
                    msg.chat.id,
                    &url_text,
                    db_pool_clone,
                    shared_storage_clone,
                )
                .await;
            });
            return Ok(None);
        }

        // Check if user is waiting for Vlipsy search
        if crate::telegram::menu::vlipsy::is_waiting_for_vlipsy_search(&shared_storage, msg.chat.id.0).await {
            crate::telegram::menu::vlipsy::handle_search_text(
                &bot,
                msg.chat.id,
                text,
                &lang,
                db_pool.clone(),
                shared_storage.clone(),
            )
            .await;
            return Ok(None);
        }

        // Use cached regex for better performance - find all URLs
        let urls: Vec<&str> = URL_REGEX.find_iter(text).map(|m| m.as_str()).collect();

        if !urls.is_empty() {
            metrics::record_message_type("url");
            // Mark the user's link message as "seen"
            crate::telegram::try_set_reaction(
                &bot,
                msg.chat.id,
                teloxide::types::MessageId(msg.id.0),
                crate::telegram::emoji::EYES,
            )
            .await;

            // Get user's preferred download format from database
            // Use get_user to get full user info (will be reused for logging)
            let (format, user_info) = match shared_storage.get_user(msg.chat.id.0).await {
                Ok(Some(user)) => (user.download_format().to_string(), Some(user)),
                Ok(None) => (String::from("mp3"), None),
                Err(e) => {
                    log::error!("Failed to get user: {}, using default mp3", e);
                    (String::from("mp3"), None)
                }
            };

            // Force mp4 for video-only sources (Vlipsy clips are always video)
            let format = if urls.iter().all(|u| {
                let host = Url::parse(u).ok().and_then(|p| p.host_str().map(String::from));
                matches!(host.as_deref(), Some("vlipsy.com" | "www.vlipsy.com"))
            }) {
                "mp4".to_string()
            } else {
                format
            };

            // Check rate limit before processing URLs
            let plan = user_info.as_ref().map(|u| u.plan.as_str()).unwrap_or("free");
            let plan_string = plan.to_string();
            if !handle_rate_limit(&bot, &msg, &rate_limiter, &plan_string, &db_pool, &shared_storage).await? {
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

                    // Resolve channel/artist URLs to latest track
                    if doracore::download::playlist::is_playlist_url(&url) {
                        match doracore::download::playlist::extract_latest_from_channel(&url).await {
                            Ok((track_url, track_title)) => {
                                log::info!(
                                    "Resolved channel/artist URL to latest track: {} ({})",
                                    track_title,
                                    track_url
                                );
                                url = Url::parse(&track_url).unwrap_or(url);
                            }
                            Err(e) => {
                                log::warn!("Failed to extract track from channel URL: {}", e);
                                continue;
                            }
                        }
                    }

                    // Check URL against source allowlist
                    let registry = crate::download::source::bot_global();
                    if registry.resolve(&url).is_none() {
                        log::warn!("Rejected unsupported URL in group: {}", url);
                        continue;
                    }

                    let _ = shared_storage
                        .upsert_preview_link_message(msg.chat.id.0, url.as_str(), msg.id.0, PREVIEW_CONTEXT_TTL_SECS)
                        .await;
                    valid_urls.push(url);
                }

                if valid_urls.is_empty() {
                    bot.send_message(msg.chat.id, i18n::t(&lang, "commands.invalid_group_links"))
                        .await?;
                    return Ok(user_info);
                }

                // Send confirmation message
                let args = doracore::fluent_args!("count" => valid_urls.len() as i64);
                let confirmation_msg = i18n::t_args(&lang, "commands.group_added", &args);
                let status_message = bot.send_message(msg.chat.id, &confirmation_msg).await?;

                // Process each URL - get metadata and add to queue
                let download_queue_clone = download_queue.clone();
                let bot_clone = bot.clone();
                let db_pool_clone = db_pool.clone();
                let shared_storage_clone = shared_storage.clone();
                let chat_id = msg.chat.id;
                let lang_clone = lang.clone();

                tokio::spawn(async move {
                    let mut status_text = confirmation_msg.clone();
                    status_text.push_str("\n\n");
                    let preview_video_quality = if format == "mp4" {
                        shared_storage_clone
                            .get_user_video_quality(chat_id.0)
                            .await
                            .ok()
                            .or_else(|| Some("best".to_string()))
                    } else {
                        None
                    };
                    let task_video_quality = preview_video_quality.clone();
                    let task_audio_bitrate = if format == "mp3" {
                        shared_storage_clone
                            .get_user_audio_bitrate(chat_id.0)
                            .await
                            .ok()
                            .or_else(|| Some("320k".to_string()))
                    } else {
                        None
                    };

                    // Experimental features graduated to main workflow
                    for (idx, url) in valid_urls.iter().enumerate() {
                        // Get metadata for preview
                        match get_preview_metadata(url, Some(&format), preview_video_quality.as_deref()).await {
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

                                let is_video = format == "mp4";
                                let plan_for_task = plan_string.clone();
                                let dl_format = format
                                    .parse::<crate::download::queue::DownloadFormat>()
                                    .unwrap_or(crate::download::queue::DownloadFormat::Mp3);
                                let task = crate::download::queue::DownloadTask::builder()
                                    .url(url.as_str().to_string())
                                    .chat_id(chat_id)
                                    .maybe_message_id(Some(msg.id.0))
                                    .is_video(is_video)
                                    .format(dl_format)
                                    .maybe_video_quality(task_video_quality.clone())
                                    .maybe_audio_bitrate(task_audio_bitrate.clone())
                                    .priority(crate::download::queue::TaskPriority::from_plan(&plan_for_task))
                                    .build();
                                download_queue_clone
                                    .add_task(task, Some(Arc::clone(&db_pool_clone)))
                                    .await;
                            }
                            Err(e) => {
                                log::warn!(
                                    "Failed to get preview metadata for URL {}: {:?}. Queuing anyway.",
                                    url,
                                    e
                                );
                                status_text.push_str(&format!(
                                    "{}. {} [{}]\n",
                                    idx + 1,
                                    url.as_str().chars().take(50).collect::<String>(),
                                    i18n::t(&lang_clone, "commands.status_in_queue")
                                ));
                                // Still queue the download — yt-dlp will handle it
                                let is_video = format == "mp4";
                                let plan_for_task = plan_string.clone();
                                let dl_format2 = format
                                    .parse::<crate::download::queue::DownloadFormat>()
                                    .unwrap_or(crate::download::queue::DownloadFormat::Mp3);
                                let task = crate::download::queue::DownloadTask::builder()
                                    .url(url.as_str().to_string())
                                    .chat_id(chat_id)
                                    .maybe_message_id(Some(msg.id.0))
                                    .is_video(is_video)
                                    .format(dl_format2)
                                    .maybe_video_quality(task_video_quality.clone())
                                    .maybe_audio_bitrate(task_audio_bitrate.clone())
                                    .priority(crate::download::queue::TaskPriority::from_plan(&plan_for_task))
                                    .build();
                                download_queue_clone
                                    .add_task(task, Some(Arc::clone(&db_pool_clone)))
                                    .await;
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
                    let args = doracore::fluent_args!("max" => crate::config::validation::MAX_URL_LENGTH as i64);
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

                let _ = shared_storage
                    .upsert_preview_link_message(msg.chat.id.0, url.as_str(), msg.id.0, PREVIEW_CONTEXT_TTL_SECS)
                    .await;

                // Check if this is a channel/artist/playlist URL → extract latest track
                if doracore::download::playlist::is_playlist_url(&url) {
                    match doracore::download::playlist::extract_latest_from_channel(&url).await {
                        Ok((track_url, track_title)) => {
                            log::info!(
                                "Resolved channel/artist URL to latest track: {} ({})",
                                track_title,
                                track_url
                            );
                            url = Url::parse(&track_url).unwrap_or(url);
                            // Re-persist preview context for the resolved track URL
                            let _ = shared_storage
                                .upsert_preview_link_message(
                                    msg.chat.id.0,
                                    url.as_str(),
                                    msg.id.0,
                                    PREVIEW_CONTEXT_TTL_SECS,
                                )
                                .await;
                        }
                        Err(e) => {
                            log::error!("Failed to extract latest track from channel: {}", e);
                            bot.send_message(msg.chat.id, i18n::t(&lang, "commands.channel_extract_failed"))
                                .await?;
                            return Ok(user_info);
                        }
                    }
                }

                // Check if this is an Instagram profile URL → show profile card
                if let Some(username) =
                    crate::download::source::instagram::InstagramSource::extract_profile_username(&url)
                {
                    let bot_clone = bot.clone();
                    let chat_id = msg.chat.id;
                    let lang_clone = lang.clone();
                    tokio::spawn(async move {
                        crate::telegram::instagram::show_instagram_profile(&bot_clone, chat_id, &username, &lang_clone)
                            .await;
                    });
                    return Ok(user_info);
                }

                // Vlipsy URLs: custom preview (skip yt-dlp)
                if url
                    .host_str()
                    .is_some_and(|h| h == "vlipsy.com" || h == "www.vlipsy.com")
                {
                    let processing_msg = bot
                        .send_message(msg.chat.id, i18n::t(&lang, "commands.processing"))
                        .await?;
                    let url_id =
                        crate::storage::cache::store_url(&db_pool, Some(shared_storage.as_ref()), url.as_str()).await;
                    match crate::telegram::preview::vlipsy::send_vlipsy_preview(
                        &bot,
                        msg.chat.id,
                        &url,
                        &url_id,
                        processing_msg.id,
                    )
                    .await
                    {
                        Ok(_) => log::info!("Vlipsy preview sent for chat {}", msg.chat.id),
                        Err(e) => {
                            log::error!("Vlipsy preview failed: {:?}", e);
                            let _ = bot
                                .send_message(msg.chat.id, i18n::t(&lang, "commands.preview_failed"))
                                .await;
                        }
                    }
                    return Ok(user_info);
                }

                // Parse time range and optional speed from text following the URL
                // e.g. "00:01:00-00:02:30" or "2:48:45-2:49:59 2x"
                let parsed_range = parse_download_time_range(text, url_text);
                let time_range = parsed_range.as_ref().map(|(s, e, _)| (s.clone(), e.clone()));
                let speed = parsed_range.as_ref().and_then(|(_, _, s)| *s);
                if let Some(ref tr) = time_range {
                    log::info!(
                        "Parsed time range for {}: {} - {}{}",
                        url,
                        tr.0,
                        tr.1,
                        speed.map(|s| format!(" (speed: {}x)", s)).unwrap_or_default()
                    );
                    let _ = shared_storage
                        .upsert_preview_time_range(
                            msg.chat.id.0,
                            url.as_str(),
                            &tr.0,
                            &tr.1,
                            speed,
                            PREVIEW_CONTEXT_TTL_SECS,
                        )
                        .await;
                }

                // Check URL against source allowlist before any processing
                let registry = crate::download::source::bot_global();
                if registry.resolve(&url).is_none() {
                    log::warn!("Rejected unsupported URL: {}", url);
                    bot.send_message(msg.chat.id, i18n::t(&lang, "commands.unsupported_url"))
                        .await?;
                    return Ok(user_info);
                }

                // Send "processing" message
                let processing_msg = bot
                    .send_message(msg.chat.id, i18n::t(&lang, "commands.processing"))
                    .await?;

                // Show preview instead of immediately downloading
                // Get video quality for the preview
                let video_quality = if format == "mp4" {
                    shared_storage
                        .get_user_video_quality(msg.chat.id.0)
                        .await
                        .ok()
                        .or_else(|| Some("best".to_string()))
                } else {
                    None
                };

                let metadata_result = if time_range.is_some() {
                    get_preview_metadata_with_time_range(&url, Some(&format), video_quality.as_deref()).await
                } else {
                    get_preview_metadata(&url, Some(&format), video_quality.as_deref()).await
                };

                match metadata_result {
                    Ok(metadata) => {
                        // Check file size during preview ONLY for audio
                        if format != "mp4" {
                            if let Some(filesize) = metadata.filesize {
                                let max_size = config::validation::max_audio_size_bytes();

                                if filesize > max_size * 1000 {
                                    let size_mb = filesize as f64 / (1024.0 * 1024.0);
                                    let max_mb = max_size as f64 / (1024.0 * 2.0 * 1024.0);
                                    log::warn!(
                                        "Audio file too large at preview stage: {:.2} MB (max: {:.2} MB)",
                                        size_mb,
                                        max_mb
                                    );

                                    let args = doracore::fluent_args!("size" => format!("{:.1}", size_mb), "max" => format!("{:.1}", max_mb));
                                    let error_message = i18n::t_args(&lang, "commands.audio_too_large", &args);

                                    bot.try_delete(msg.chat.id, processing_msg.id).await;

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
                            Arc::clone(&shared_storage),
                            time_range.as_ref(),
                        )
                        .await
                        {
                            Ok(_) => {
                                log::info!("Preview sent successfully for chat {}", msg.chat.id);
                            }
                            Err(e) => {
                                log::error!("Failed to send preview: {:?}", e);
                                bot.send_message(msg.chat.id, i18n::t(&lang, "commands.preview_failed"))
                                    .await?;
                            }
                        }
                    }
                    Err(e) => {
                        log::error!("Failed to get preview metadata: {:?}", e);

                        let is_duration_error = if let AppError::Download(ref err_msg) = e {
                            let msg_str = err_msg.message();
                            msg_str.contains("too long") || msg_str.contains("zu lang") || msg_str.contains("trop long")
                        } else {
                            false
                        };

                        if !is_duration_error {
                            if let Some(ref alert_mgr) = alert_manager {
                                let user_id = msg.chat.id.0;
                                let error_str = format!("{:?}", e);
                                let context = crate::core::alerts::DownloadContext::with_live_status().await;
                                if let Err(alert_err) = alert_mgr
                                    .alert_download_failure(user_id, url.as_str(), &error_str, 3, Some(&context))
                                    .await
                                {
                                    log::error!("Failed to send alert: {}", alert_err);
                                }
                            }
                        }

                        bot.try_delete(msg.chat.id, processing_msg.id).await;

                        let error_message = if let AppError::Download(ref err_msg) = e {
                            if is_duration_error {
                                err_msg.to_string()
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
        } else if !text.starts_with('/') {
            // No URLs found — check for player/search context before showing "no links"

            // "exit" text stops player mode
            if text.eq_ignore_ascii_case("exit")
                && shared_storage
                    .get_player_session(msg.chat.id.0)
                    .await
                    .ok()
                    .flatten()
                    .is_some()
            {
                crate::telegram::menu::player::stop_player(&bot, msg.chat.id, &db_pool, &shared_storage).await;
                return Ok(None);
            }

            // Standalone search context: if user typed text while a search session with empty query exists
            if let Some(session) =
                crate::telegram::menu::search::get_search_session(&shared_storage, msg.chat.id.0).await
            {
                if session.query.is_empty() {
                    crate::telegram::menu::search::handle_standalone_search(
                        &bot,
                        msg.chat.id,
                        text,
                        db_pool.clone(),
                        shared_storage.clone(),
                        session.context.clone(),
                    )
                    .await;
                    return Ok(None);
                }
            }

            // Player mode: text → music search (only non-URL text)
            if let Ok(Some(session)) = shared_storage.get_player_session(msg.chat.id.0).await {
                crate::telegram::menu::search::handle_player_search(
                    &bot,
                    msg.chat.id,
                    text,
                    db_pool.clone(),
                    shared_storage.clone(),
                    session.playlist_id,
                )
                .await;
                return Ok(None);
            }

            // Implicit search: treat plain text (3+ chars) as music search query
            let trimmed = text.trim();
            if trimmed.chars().count() >= 3 && trimmed.len() <= 200 {
                let plan = match shared_storage.get_user(msg.chat.id.0).await {
                    Ok(Some(user)) => user.plan.as_str().to_string(),
                    _ => "free".to_string(),
                };
                if !handle_rate_limit(&bot, &msg, &rate_limiter, &plan, &db_pool, &shared_storage).await? {
                    return Ok(None);
                }
                metrics::record_message_type("search");
                crate::telegram::menu::search::handle_standalone_search(
                    &bot,
                    msg.chat.id,
                    trimmed,
                    db_pool.clone(),
                    shared_storage.clone(),
                    crate::telegram::menu::search::SearchContext::Standalone,
                )
                .await;
                return Ok(None);
            }
            metrics::record_message_type("text");
            bot.send_md(msg.chat.id, i18n::t(&lang, "commands.no_links")).await?;
        } else if text.eq_ignore_ascii_case("/exit") {
            // /exit command — stop player if active
            if shared_storage
                .get_player_session(msg.chat.id.0)
                .await
                .ok()
                .flatten()
                .is_some()
            {
                crate::telegram::menu::player::stop_player(&bot, msg.chat.id, &db_pool, &shared_storage).await;
                return Ok(None);
            }
            // No active player — treat as unknown command
            bot.send_md(msg.chat.id, i18n::t(&lang, "commands.no_links")).await?;
        } else {
            metrics::record_message_type("text");
            bot.send_md(msg.chat.id, i18n::t(&lang, "commands.no_links")).await?;
        }
    }
    Ok(None)
}

fn is_cancel_text(text: &str) -> bool {
    let lower = text.trim().to_lowercase();
    matches!(lower.as_str(), "cancel" | "/cancel" | "❌" | "x")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::escape_markdown;

    // ==================== is_cancel_text tests ====================

    #[test]
    fn test_is_cancel_text_english() {
        assert!(is_cancel_text("cancel"));
        assert!(is_cancel_text("Cancel"));
        assert!(is_cancel_text("CANCEL"));
        assert!(is_cancel_text("/cancel"));
    }

    #[test]
    fn test_is_cancel_text_symbols() {
        assert!(is_cancel_text("❌"));
        assert!(is_cancel_text("x"));
        assert!(is_cancel_text("X"));
    }

    #[test]
    fn test_is_cancel_text_invalid() {
        assert!(!is_cancel_text("hello"));
        assert!(!is_cancel_text(""));
        assert!(!is_cancel_text("cancellation"));
        assert!(!is_cancel_text("отмена"));
        assert!(!is_cancel_text("отменить"));
    }

    // ==================== escape_markdown tests ====================

    #[test]
    fn test_escape_markdown_special_chars() {
        assert_eq!(escape_markdown("hello_world"), "hello\\_world");
        assert_eq!(escape_markdown("*bold*"), "\\*bold\\*");
        assert_eq!(escape_markdown("[link](url)"), "\\[link\\]\\(url\\)");
        assert_eq!(escape_markdown("`code`"), "\\`code\\`");
    }

    #[test]
    fn test_escape_markdown_multiple_chars() {
        let text = "Test_with*multiple[special]chars!";
        let escaped = escape_markdown(text);
        assert_eq!(escaped, "Test\\_with\\*multiple\\[special\\]chars\\!");
    }

    #[test]
    fn test_escape_markdown_empty() {
        assert_eq!(escape_markdown(""), "");
    }

    #[test]
    fn test_escape_markdown_no_special() {
        assert_eq!(escape_markdown("hello world"), "hello world");
        assert_eq!(escape_markdown("normal text 123"), "normal text 123");
    }

    #[test]
    fn test_escape_markdown_all_special_chars() {
        // All special chars: _ * [ ] ( ) ~ ` > # + - = | { } . !
        let all_special = "_*[]()~`>#+-=|{}.!";
        let escaped = escape_markdown(all_special);
        assert_eq!(escaped, "\\_\\*\\[\\]\\(\\)\\~\\`\\>\\#\\+\\-\\=\\|\\{\\}\\.\\!");
    }

    // ==================== URL_REGEX tests ====================

    #[test]
    fn test_url_regex_matches() {
        let text = "Check out https://youtube.com/watch?v=abc and http://example.com";
        let urls: Vec<&str> = URL_REGEX.find_iter(text).map(|m| m.as_str()).collect();
        assert_eq!(urls.len(), 2);
        assert!(urls[0].starts_with("https://youtube.com"));
        assert!(urls[1].starts_with("http://example.com"));
    }

    #[test]
    fn test_url_regex_no_match() {
        let text = "No URLs here";
        let urls: Vec<&str> = URL_REGEX.find_iter(text).map(|m| m.as_str()).collect();
        assert!(urls.is_empty());
    }
}
