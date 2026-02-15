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
use crate::download::source::SourceRegistry;
use crate::storage::db::{self as db, DbPool};
use crate::telegram::Bot;
use chrono::{DateTime, Utc};
use std::sync::Arc;
use teloxide::prelude::*;
use tokio::time::timeout;
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
    audio_bitrate: Option<String>,
    message_id: Option<i32>,
    alert_manager: Option<Arc<crate::core::alerts::AlertManager>>,
    time_range: Option<(String, String)>,
) -> ResponseResult<()> {
    log::info!(
        "Starting download_and_send_audio for chat {} with URL: {}",
        chat_id,
        url
    );
    let bot_clone = bot.clone();
    let _rate_limiter = rate_limiter;
    let db_pool_clone = db_pool.clone();

    tokio::spawn(async move {
        // Get user plan for metrics
        let user_plan = if let Some(ref pool) = db_pool_clone {
            if let Ok(conn) = db::get_connection(pool) {
                db::get_user(&conn, chat_id.0)
                    .ok()
                    .flatten()
                    .map(|u| u.plan)
                    .unwrap_or_default()
            } else {
                Plan::default()
            }
        } else {
            Plan::default()
        };

        metrics::record_format_request("mp3", user_plan.as_str());

        let quality = audio_bitrate.as_deref().unwrap_or("default");
        let timer = metrics::DOWNLOAD_DURATION_SECONDS
            .with_label_values(&["mp3", quality])
            .start_timer();

        let format = PipelineFormat::Audio {
            bitrate: audio_bitrate.clone(),
            time_range,
        };
        let registry = SourceRegistry::global();

        // Global timeout for entire download operation
        let result: Result<(), AppError> = match timeout(config::download::global_timeout(), async {
            let pipeline_result = pipeline::execute(
                &bot_clone,
                chat_id,
                &url,
                &format,
                db_pool_clone.as_ref(),
                message_id,
                alert_manager.as_ref(),
                registry,
            )
            .await
            .map_err(|e| e.into_app_error())?;

            // Audio-specific: add effects button
            add_audio_effects_button(&bot_clone, chat_id, &pipeline_result, db_pool_clone.as_ref()).await;

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
                    "Таймаут загрузки (превышено {} минут)",
                    config::download::GLOBAL_TIMEOUT_SECS / 60
                ))))
            }
        };

        match result {
            Ok(()) => {
                log::info!("Audio download completed successfully for chat {}", chat_id);
                timer.observe_duration();
                metrics::record_download_success("mp3", quality);
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

                let pipeline_error = pipeline::PipelineError::Operational(e);
                pipeline::handle_pipeline_error(
                    &bot_clone,
                    chat_id,
                    &url,
                    &pipeline_error,
                    &format,
                    alert_manager.as_ref(),
                )
                .await;
            }
        }
    });

    Ok(())
}

/// Add audio effects button (Edit/Cut) for eligible users.
///
/// Creates an AudioEffectSession, copies the downloaded file for effects processing,
/// and adds inline keyboard buttons to the sent message.
async fn add_audio_effects_button(bot: &Bot, chat_id: ChatId, result: &PipelineResult, db_pool: Option<&Arc<DbPool>>) {
    let Some(pool) = db_pool else {
        return;
    };
    let Ok(conn) = db::get_connection(pool) else {
        log::warn!("Audio effects: failed to get DB connection");
        return;
    };

    // TODO: Re-enable premium check after testing
    // if !db::is_premium_or_vip(&conn, chat_id.0).unwrap_or(false) { return; }

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

            match db::create_audio_effect_session(&conn, &session) {
                Ok(_) => {
                    log::info!("Audio effects: session created with id {}", session_id);
                    let bot_for_button = bot.clone();
                    let sent_message_id = result.sent_message.id;
                    let session_id_clone = session_id.clone();
                    tokio::spawn(async move {
                        use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};

                        let keyboard = InlineKeyboardMarkup::new(vec![vec![
                            InlineKeyboardButton::callback("Edit Audio", format!("ae:open:{}", session_id_clone)),
                            InlineKeyboardButton::callback("Cut Audio", format!("ac:open:{}", session_id_clone)),
                        ]]);

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
