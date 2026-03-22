//! Audio download and processing module
//!
//! Thin wrapper around the unified download pipeline, adding audio-specific
//! post-processing (audio effects button).

use crate::core::config;
use crate::core::error::AppError;
use crate::core::metrics;
use crate::core::rate_limiter::RateLimiter;
use crate::core::types::Plan;
use crate::download::error::DownloadError;
use crate::download::pipeline::{self, PipelineFormat, PipelineResult};
use crate::download::progress::{ProgressBarStyle, ProgressMessage};
use crate::download::source::bot_global;
use crate::storage::db::DbPool;
use crate::storage::SharedStorage;
use crate::telegram::Bot;
use chrono::{DateTime, Utc};
use std::sync::Arc;
use teloxide::prelude::*;
use tokio::time::timeout;
use tracing::Instrument;
use url::Url;

/// Download audio file and send it to user
///
/// Downloads audio from URL using the unified download pipeline, shows progress updates,
/// validates file size, and sends the file to the user via Telegram.
/// After successful send, adds audio effects buttons (Edit/Cut).
pub async fn download_and_send_audio(
    bot: Bot,
    chat_id: ChatId,
    url: Url,
    rate_limiter: Arc<RateLimiter>,
    _created_timestamp: DateTime<Utc>,
    db_pool: Option<Arc<DbPool>>,
    shared_storage: Option<Arc<SharedStorage>>,
    audio_bitrate: Option<String>,
    message_id: Option<i32>,
    alert_manager: Option<Arc<crate::core::alerts::AlertManager>>,
    time_range: Option<(String, String)>,
    with_lyrics: bool,
) -> ResponseResult<()> {
    log::info!(
        "Starting download_and_send_audio for chat {} with URL: {}",
        chat_id,
        url
    );
    let bot_clone = bot.clone();
    let _rate_limiter = rate_limiter;
    let db_pool_clone = db_pool.clone();
    let shared_storage_clone = shared_storage.clone();

    // Inherit the parent span (from queue_processor) so all audio logs carry op=...
    let span = tracing::Span::current();
    tokio::spawn(
        async move {
            // Get user plan for metrics
            let user_plan = if let Some(ref storage) = shared_storage_clone {
                storage
                    .get_user(chat_id.0)
                    .await
                    .ok()
                    .flatten()
                    .map(|u| u.plan)
                    .unwrap_or_default()
            } else {
                Plan::default()
            };

            metrics::record_format_request("mp3", user_plan.as_str());
            metrics::record_platform_download(metrics::extract_platform(url.as_str()));

            let quality = audio_bitrate.as_deref().unwrap_or("default");
            let timer = metrics::DOWNLOAD_DURATION_SECONDS
                .with_label_values(&["mp3", quality])
                .start_timer();

            let format = PipelineFormat::Audio {
                bitrate: audio_bitrate.clone(),
                time_range,
            };
            let registry = bot_global();

            // Create progress_msg BEFORE timeout so we can clean it up if timeout fires
            let lang = if let Some(ref storage) = shared_storage_clone {
                crate::i18n::user_lang_from_storage(storage, chat_id.0).await
            } else {
                crate::i18n::lang_from_code("ru")
            };
            let mut progress_msg = ProgressMessage::new(chat_id, lang.clone());
            if let Some(ref storage) = shared_storage_clone {
                if let Ok(style_str) = storage.get_user_progress_bar_style(chat_id.0).await {
                    progress_msg.style = ProgressBarStyle::parse(&style_str);
                }
            }

            // Global timeout for entire download operation
            let result: Result<(), AppError> = match timeout(config::download::global_timeout(), async {
                let pipeline_result = pipeline::execute(
                    &bot_clone,
                    chat_id,
                    &url,
                    &format,
                    db_pool_clone.as_ref(),
                    shared_storage_clone.as_ref(),
                    message_id,
                    alert_manager.as_ref(),
                    registry,
                    &mut progress_msg,
                )
                .await
                .map_err(|e| e.into_app_error())?;

                metrics::record_file_size("mp3", pipeline_result.file_size);

                // Audio-specific: add effects button
                add_audio_effects_button(&bot_clone, chat_id, &pipeline_result, shared_storage_clone.as_ref()).await;

                // Lyrics highlights: fetch lyrics + LLM highlight in background, send as reply
                if with_lyrics {
                    let bot_lyr = bot_clone.clone();
                    let title_lyr = pipeline_result.title.clone();
                    let artist_lyr = pipeline_result.artist.clone();
                    let sent_msg_id = pipeline_result.sent_message.id;
                    tokio::spawn(async move {
                        send_lyrics_highlights(&bot_lyr, chat_id, sent_msg_id, &artist_lyr, &title_lyr).await;
                    });
                }

                // Share page: create after successful audio send (YouTube only, fire-and-forget)
                if crate::core::share::is_youtube_url(url.as_str()) {
                    if let Some(ref storage) = shared_storage_clone {
                        let storage_share = std::sync::Arc::clone(storage);
                        let url_str = url.to_string();
                        let title_share = pipeline_result.title.clone();
                        let artist_share = pipeline_result.artist.clone();
                        let duration_share = pipeline_result.duration;
                        let bot_share = bot_clone.clone();
                        tokio::spawn(async move {
                            let thumb = crate::core::share::youtube_thumbnail_url(&url_str);
                            let artist_opt = if artist_share.trim().is_empty() {
                                None
                            } else {
                                Some(artist_share.as_str())
                            };
                            if let Some((share_url, streaming_links)) = crate::core::share::create_share_page(
                                &storage_share,
                                &url_str,
                                &title_share,
                                artist_opt,
                                thumb.as_deref(),
                                Some(duration_share as u64),
                            )
                            .await
                            {
                                send_share_message(
                                    &bot_share,
                                    chat_id,
                                    &title_share,
                                    &share_url,
                                    streaming_links.as_ref(),
                                )
                                .await;
                            }
                        });
                    }
                }

                // Schedule file cleanup (including any carousel extras)
                let extra_paths: Vec<String> = pipeline_result
                    .output
                    .additional_files
                    .as_ref()
                    .map(|files| files.iter().map(|f| f.file_path.clone()).collect())
                    .unwrap_or_default();
                pipeline::schedule_cleanup_with_extras(pipeline_result.download_path.clone(), extra_paths);

                Ok(())
            })
            .await
            {
                Ok(inner) => inner,
                Err(_) => {
                    log::error!(
                        "🚨 Audio download timed out after {} seconds",
                        config::download::GLOBAL_TIMEOUT_SECS
                    );
                    Err(AppError::Download(DownloadError::Timeout(format!(
                        "Download timed out (exceeded {} minutes)",
                        config::download::GLOBAL_TIMEOUT_SECS / 60
                    ))))
                }
            };

            match result {
                Ok(()) => {
                    log::info!("Audio download completed successfully for chat {}", chat_id);
                    timer.observe_duration();
                    metrics::record_download_success("mp3", quality);
                    let signoff = crate::i18n::random_signoff(&lang);
                    let _ = bot_clone
                        .send_message(chat_id, signoff)
                        .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                        .await;
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

                    // Delete hanging ⏳ progress message so it doesn't stay on screen forever
                    if let Some(msg_id) = progress_msg.message_id {
                        let _ = bot_clone.delete_message(chat_id, msg_id).await;
                    }

                    let pipeline_error = pipeline::PipelineError::Operational(e);
                    pipeline::handle_pipeline_error(
                        &bot_clone,
                        chat_id,
                        &url,
                        &pipeline_error,
                        &format,
                        alert_manager.as_ref(),
                        message_id,
                    )
                    .await;
                }
            }
        }
        .instrument(span),
    );

    Ok(())
}

/// Add audio effects button (Edit/Cut) for eligible users.
///
/// Creates an AudioEffectSession, copies the downloaded file for effects processing,
/// and adds inline keyboard buttons to the sent message.
async fn add_audio_effects_button(
    bot: &Bot,
    chat_id: ChatId,
    result: &PipelineResult,
    shared_storage: Option<&Arc<SharedStorage>>,
) {
    let Some(storage) = shared_storage else {
        return;
    };

    use crate::download::audio_effects::{self, AudioEffectSession};

    let session_id = uuid::Uuid::new_v4().to_string();
    let session_file_path_raw = audio_effects::get_original_file_path(&session_id, &config::DOWNLOAD_FOLDER);
    let session_file_path = shellexpand::tilde(&session_file_path_raw).into_owned();

    // Copy file synchronously before it gets deleted
    match std::fs::copy(&result.download_path, &session_file_path) {
        Ok(bytes) => {
            log::info!("Audio effects: copied {} bytes to {}", bytes, session_file_path);
            let session = AudioEffectSession::new(
                session_id.clone(),
                chat_id.0,
                session_file_path,
                result.sent_message.id.0,
                result.display_title.as_ref().to_string(),
                result.duration,
            );

            match storage.create_audio_effect_session(&session).await {
                Ok(_) => {
                    log::info!("Audio effects: session created with id {}", session_id);
                    let bot_for_button = bot.clone();
                    let sent_message_id = result.sent_message.id;
                    let session_id_clone = session_id.clone();
                    tokio::spawn(async move {
                        use teloxide::types::InlineKeyboardMarkup;

                        let keyboard = InlineKeyboardMarkup::new(vec![
                            vec![
                                crate::telegram::cb("Edit Audio", format!("ae:open:{}", session_id_clone)),
                                crate::telegram::cb("Cut Audio", format!("ac:open:{}", session_id_clone)),
                            ],
                            vec![crate::telegram::cb("🎵 Lyrics", format!("lyr:{}", session_id_clone))],
                        ]);

                        if let Err(e) = bot_for_button
                            .edit_message_reply_markup(chat_id, sent_message_id)
                            .reply_markup(keyboard)
                            .await
                        {
                            log::warn!("Failed to add audio effects button: {}", e);
                        } else {
                            log::info!(
                                "Added audio effects button to message {} for session {}",
                                sent_message_id.0,
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
                "Failed to copy file for audio effects: {} (src: {}, dst: {})",
                e,
                result.download_path,
                session_file_path
            );
        }
    }
}

/// Send a follow-up Telegram message with streaming service buttons after a successful download.
async fn send_share_message(
    bot: &Bot,
    chat_id: ChatId,
    title: &str,
    share_url: &str,
    streaming_links: Option<&crate::core::odesli::StreamingLinks>,
) {
    use teloxide::requests::Requester;
    use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};

    let mut row1: Vec<InlineKeyboardButton> = Vec::new();
    let mut has_links = false;

    if let Some(links) = streaming_links {
        if let Some(ref url) = links.spotify {
            if let Ok(u) = url.parse() {
                row1.push(InlineKeyboardButton::url("💚 Spotify", u));
                has_links = true;
            }
        }
        if let Some(ref url) = links.apple_music {
            if let Ok(u) = url.parse() {
                row1.push(InlineKeyboardButton::url("🍎 Apple", u));
                has_links = true;
            }
        }
        if let Some(ref url) = links.youtube_music {
            if let Ok(u) = url.parse() {
                row1.push(InlineKeyboardButton::url("🔴 YT Music", u));
                has_links = true;
            }
        }
    }

    let Ok(share_parsed) = share_url.parse() else {
        log::warn!("Invalid share URL: {}", share_url);
        return;
    };
    let row2 = vec![InlineKeyboardButton::url("🔗 All platforms", share_parsed)];

    let mut rows: Vec<Vec<InlineKeyboardButton>> = Vec::new();
    if !row1.is_empty() {
        rows.push(row1);
    }
    rows.push(row2);

    let keyboard = InlineKeyboardMarkup::new(rows);

    let text = if has_links {
        format!("🎧 \"{}\" — listen legally:", title)
    } else {
        format!("🔗 \"{}\":", title)
    };

    if let Err(e) = bot.send_message(chat_id, text).reply_markup(keyboard).await {
        log::warn!("Failed to send share message: {}", e);
    }
}

/// Fetch lyrics and extract key lines via LLM, then send as a reply to the audio message.
///
/// Silently does nothing if:
/// - ANTHROPIC_API_KEY is not set
/// - Lyrics are not found for this track
/// - LLM fails to extract highlights
/// - The track is not a song (no artist metadata)
async fn send_lyrics_highlights(
    bot: &Bot,
    chat_id: ChatId,
    reply_to: teloxide::types::MessageId,
    artist: &str,
    title: &str,
) {
    // Skip if no artist — probably not a song (podcast, audiobook, etc.)
    if artist.trim().is_empty() || title.trim().is_empty() {
        return;
    }

    // Skip if ANTHROPIC_API_KEY is not configured
    if std::env::var("ANTHROPIC_API_KEY").is_err() {
        log::warn!(
            "Lyrics highlights: ANTHROPIC_API_KEY not set, skipping LLM highlights for '{} - {}'",
            artist,
            title
        );
        return;
    }

    // Step 1: Fetch lyrics
    let lyrics = match crate::lyrics::fetch_lyrics(artist, title, None).await {
        Some(lyr) => lyr,
        None => {
            log::debug!("Lyrics highlights: no lyrics found for '{} - {}'", artist, title);
            return;
        }
    };

    let full_text = lyrics.all_text();

    // Step 2: Extract highlights via LLM
    let highlights = match crate::lyrics::highlights::extract_highlights(artist, title, &full_text).await {
        Some(h) => h,
        None => {
            log::debug!("Lyrics highlights: LLM returned nothing for '{} - {}'", artist, title);
            return;
        }
    };

    // Step 3: Send as italic reply to the audio message
    let escaped = crate::core::escape_markdown(&highlights);
    let text = format!("_{}_", escaped);

    if let Err(e) = bot
        .send_message(chat_id, text)
        .parse_mode(teloxide::types::ParseMode::MarkdownV2)
        .reply_parameters(teloxide::types::ReplyParameters::new(reply_to))
        .await
    {
        log::warn!("Failed to send lyrics highlights: {}", e);
    } else {
        log::info!("Lyrics highlights sent for '{} - {}'", artist, title);
    }
}
