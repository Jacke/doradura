//! Download-trigger callback handling — `dl:` prefix.
//!
//! Handles the main "download" button press on the preview card:
//! parses the `dl:{format}[:{quality}]:{url_id}[:{mask}]` payload,
//! resolves the URL from cache, reads user prefs + preview context,
//! and enqueues one (or two, for `mp4+mp3`) `DownloadTask`s.
//!
//! Extracted from `callback_router::handle_menu_callback` (this single
//! branch was 195 LOC — the biggest remaining wedge after the `pv:`
//! extraction in Phase A).

use std::sync::Arc;

use teloxide::prelude::*;
use url::Url;

use crate::core::rate_limiter::RateLimiter;
use crate::download::queue::{DownloadFormat, DownloadQueue, DownloadTask};
use crate::storage::cache;
use crate::storage::db::DbPool;
use crate::storage::SharedStorage;
use crate::telegram::Bot;

use super::helpers::send_queue_position_message;

/// Entry point for `dl:*` callback queries.
///
/// Parses the download payload, resolves the short-ID → URL via the
/// cache, enqueues the resulting `DownloadTask`(s), and emits a queue
/// position message. All original inline logic is preserved verbatim
/// — this extraction is structural only.
#[allow(clippy::too_many_arguments)]
pub async fn handle_download_callback(
    bot: &Bot,
    callback_id: teloxide::types::CallbackQueryId,
    chat_id: ChatId,
    message_id: teloxide::types::MessageId,
    data: &str,
    db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
    download_queue: Arc<DownloadQueue>,
    rate_limiter: Arc<RateLimiter>,
) -> ResponseResult<()> {
    let _ = bot.answer_callback_query(callback_id.clone()).await;
    if let Err(e) = bot.delete_message(chat_id, message_id).await {
        log::warn!("Failed to delete preview message: {:?}", e);
    }

    let parts: Vec<&str> = data.split(':').collect();

    if parts.len() >= 3 {
        let raw_format = parts[1];
        let with_lyrics = raw_format == "mp3+lyr";
        let format = if with_lyrics { "mp3" } else { raw_format };

        let (url_id, carousel_mask) = if format == "photo" && parts.len() == 4 {
            let mask = parts[3].parse::<u32>().ok();
            (parts[2], mask)
        } else {
            (
                if parts.len() == 3 {
                    parts[2]
                } else if parts.len() == 4 {
                    parts[3]
                } else {
                    log::warn!("Invalid dl callback format: {}", data);
                    let _ = bot.send_message(chat_id, "Error: invalid request format").await;
                    return Ok(());
                },
                None,
            )
        };

        let selected_quality = if parts.len() == 4 && (format == "mp4" || format == "mp4+mp3") {
            Some(parts[2].to_string())
        } else {
            None
        };

        log::debug!(
            "Download button clicked: chat={}, url_id={}, format={}",
            chat_id.0,
            url_id,
            format
        );

        match cache::get_url(&db_pool, Some(shared_storage.as_ref()), url_id).await {
            Some(url_str) => match Url::parse(&url_str) {
                Ok(url) => {
                    let preview_context = shared_storage
                        .get_preview_context(chat_id.0, &url_str)
                        .await
                        .ok()
                        .flatten();
                    let original_message_id = preview_context.as_ref().and_then(|context| context.original_message_id);
                    let time_range = preview_context.as_ref().and_then(|context| context.time_range.clone());
                    let plan = shared_storage
                        .get_user(chat_id.0)
                        .await
                        .ok()
                        .flatten()
                        .map(|user| user.plan)
                        .unwrap_or_default();

                    let _ = (rate_limiter, &plan);

                    if format == "mp4+mp3" {
                        let video_quality = if let Some(quality) = selected_quality {
                            Some(quality)
                        } else {
                            Some(
                                shared_storage
                                    .get_user_video_quality(chat_id.0)
                                    .await
                                    .unwrap_or_else(|_| "best".to_string()),
                            )
                        };
                        let mut task_mp4 = DownloadTask::builder()
                            .url(url.as_str().to_string())
                            .chat_id(chat_id)
                            .maybe_message_id(original_message_id)
                            .is_video(true)
                            .format(DownloadFormat::Mp4)
                            .maybe_video_quality(video_quality)
                            .maybe_audio_bitrate(None)
                            .priority(crate::download::queue::TaskPriority::from_plan(plan.as_str()))
                            .build();
                        task_mp4.time_range = time_range.clone();
                        download_queue.add_task(task_mp4, Some(Arc::clone(&db_pool))).await;

                        let audio_bitrate = Some(
                            shared_storage
                                .get_user_audio_bitrate(chat_id.0)
                                .await
                                .unwrap_or_else(|_| "320k".to_string()),
                        );
                        let mut task_mp3 = DownloadTask::builder()
                            .url(url.as_str().to_string())
                            .chat_id(chat_id)
                            .maybe_message_id(original_message_id)
                            .is_video(false)
                            .format(DownloadFormat::Mp3)
                            .maybe_video_quality(None)
                            .maybe_audio_bitrate(audio_bitrate)
                            .priority(crate::download::queue::TaskPriority::from_plan(plan.as_str()))
                            .build();
                        task_mp3.time_range = time_range.clone();
                        task_mp3.with_lyrics = with_lyrics;
                        download_queue.add_task(task_mp3, Some(Arc::clone(&db_pool))).await;

                        log::info!("Added 2 tasks to queue for mp4+mp3: MP4 and MP3 for chat {}", chat_id.0);

                        if let Some(msg_id) = send_queue_position_message(
                            bot,
                            chat_id,
                            plan.as_str(),
                            &download_queue,
                            &db_pool,
                            &shared_storage,
                        )
                        .await
                        {
                            download_queue.set_queue_message_id(chat_id, msg_id.0).await;
                        }
                    } else {
                        let video_quality = if format == "mp4" {
                            if let Some(quality) = selected_quality {
                                Some(quality)
                            } else {
                                Some(
                                    shared_storage
                                        .get_user_video_quality(chat_id.0)
                                        .await
                                        .unwrap_or_else(|_| "best".to_string()),
                                )
                            }
                        } else {
                            None
                        };
                        let audio_bitrate = if format == "mp3" {
                            Some(
                                shared_storage
                                    .get_user_audio_bitrate(chat_id.0)
                                    .await
                                    .unwrap_or_else(|_| "320k".to_string()),
                            )
                        } else {
                            None
                        };

                        let is_video = format == "mp4";
                        let dl_format = format.parse::<DownloadFormat>().unwrap_or(DownloadFormat::Mp3);
                        let mut task = DownloadTask::builder()
                            .url(url.as_str().to_string())
                            .chat_id(chat_id)
                            .maybe_message_id(original_message_id)
                            .is_video(is_video)
                            .format(dl_format)
                            .maybe_video_quality(video_quality)
                            .maybe_audio_bitrate(audio_bitrate)
                            .priority(crate::download::queue::TaskPriority::from_plan(plan.as_str()))
                            .build();
                        task.time_range = time_range.clone();
                        task.carousel_mask = carousel_mask;
                        task.with_lyrics = with_lyrics;
                        download_queue.add_task(task, Some(Arc::clone(&db_pool))).await;

                        if let Some(msg_id) = send_queue_position_message(
                            bot,
                            chat_id,
                            plan.as_str(),
                            &download_queue,
                            &db_pool,
                            &shared_storage,
                        )
                        .await
                        {
                            download_queue.set_queue_message_id(chat_id, msg_id.0).await;
                        }
                    }
                }
                Err(e) => {
                    log::error!("Failed to parse URL from cache: {}", e);
                    let _ = bot.send_message(chat_id, "❌ Error: invalid link").await;
                }
            },
            None => {
                log::warn!("URL not found in cache for ID: {} (expired or invalid)", url_id);
                let _ = bot.send_message(chat_id, "⏰ Link expired, please send it again").await;
            }
        }
    }
    Ok(())
}
