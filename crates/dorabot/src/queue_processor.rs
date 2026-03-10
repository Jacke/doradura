//! Download queue processor.
//!
//! Runs a loop that dequeues download tasks and processes them concurrently,
//! respecting semaphore limits and inter-download delays.

use std::sync::Arc;

use teloxide::prelude::*;
use tokio::time::interval;

use crate::core::retry::Retryable;
use crate::core::{alerts, config, rate_limiter};
use crate::download::queue::{self as queue};
use crate::download::ytdlp_errors::sanitize_user_error_message;
use crate::download::{download_and_send_audio, download_and_send_subtitles, download_and_send_video, DownloadQueue};
use crate::storage::db::{self as db, DbPool};
use crate::telegram::notifications::notify_admin_task_failed;
use crate::telegram::Bot;

/// Main queue processing loop.
///
/// Continuously polls the download queue and spawns tasks to process downloads.
/// Uses a semaphore to limit concurrent downloads and enforces inter-download delays.
pub async fn process_queue(
    bot: Bot,
    queue: Arc<DownloadQueue>,
    rate_limiter: Arc<rate_limiter::RateLimiter>,
    db_pool: Arc<DbPool>,
    alert_manager: Option<Arc<alerts::AlertManager>>,
) {
    const LEASE_SECONDS: i64 = 300;
    const HEARTBEAT_SECONDS: u64 = 20;
    const REAPER_SECONDS: u64 = 60;

    let max_concurrent = config::queue::max_concurrent_downloads();
    log::info!(
        "Download queue: max_concurrent={}, inter_delay={}ms",
        max_concurrent,
        config::queue::INTER_DOWNLOAD_DELAY_MS
    );
    let semaphore = Arc::new(tokio::sync::Semaphore::new(max_concurrent));
    let mut interval = interval(config::queue::check_interval());
    let last_download_start = Arc::new(tokio::sync::Mutex::new(std::time::Instant::now()));
    let worker_id = format!("{}-{}", std::process::id(), uuid::Uuid::new_v4());

    // Periodic cleanup of stale notification_msgs (every 30 min)
    let queue_for_notif_cleanup = Arc::clone(&queue);
    tokio::spawn(async move {
        let mut cleanup_interval = tokio::time::interval(std::time::Duration::from_secs(30 * 60));
        loop {
            cleanup_interval.tick().await;
            let removed = queue_for_notif_cleanup.cleanup_stale_notifications().await;
            if removed > 0 {
                log::info!("Queue: cleaned up {} stale notification entries", removed);
            }
        }
    });

    let heartbeat_pool = Arc::clone(&db_pool);
    let heartbeat_worker_id = worker_id.clone();
    tokio::spawn(async move {
        let mut heartbeat_interval = tokio::time::interval(std::time::Duration::from_secs(HEARTBEAT_SECONDS));
        loop {
            heartbeat_interval.tick().await;
            match db::get_connection(&heartbeat_pool) {
                Ok(conn) => {
                    if let Err(e) = db::heartbeat_worker_leases(&conn, &heartbeat_worker_id, LEASE_SECONDS) {
                        log::warn!("Queue heartbeat failed for {}: {}", heartbeat_worker_id, e);
                    }
                }
                Err(e) => log::warn!("Queue heartbeat connection failed: {}", e),
            }
        }
    });

    let reaper_pool = Arc::clone(&db_pool);
    tokio::spawn(async move {
        let mut reaper_interval = tokio::time::interval(std::time::Duration::from_secs(REAPER_SECONDS));
        loop {
            reaper_interval.tick().await;
            match db::get_connection(&reaper_pool) {
                Ok(conn) => match db::recover_expired_leases(&conn, config::admin::MAX_TASK_RETRIES) {
                    Ok(recovered) if recovered > 0 => {
                        log::warn!("Recovered {} expired queue lease(s)", recovered);
                    }
                    Ok(_) => {}
                    Err(e) => log::warn!("Queue reaper failed: {}", e),
                },
                Err(e) => log::warn!("Queue reaper connection failed: {}", e),
            }
        }
    });

    loop {
        interval.tick().await;
        if semaphore.available_permits() == 0 {
            continue;
        }

        let claimed_task = match db::get_connection(&db_pool) {
            Ok(conn) => match db::claim_next_task(&conn, &worker_id, LEASE_SECONDS) {
                Ok(task) => task,
                Err(e) => {
                    log::warn!("Failed to claim next queue task: {}", e);
                    continue;
                }
            },
            Err(e) => {
                log::warn!("Failed to get queue DB connection: {}", e);
                continue;
            }
        };

        if let Some(task_entry) = claimed_task {
            let task = queue::DownloadQueue::task_from_entry(task_entry);
            log::info!("Got task {} from queue", task.id);
            let bot = bot.clone();
            let rate_limiter = Arc::clone(&rate_limiter);
            let semaphore = Arc::clone(&semaphore);
            let db_pool = Arc::clone(&db_pool);
            let last_download_start = Arc::clone(&last_download_start);
            let alert_manager = alert_manager.clone();
            let queue_for_cleanup = Arc::clone(&queue);
            let worker_id = worker_id.clone();

            tokio::spawn(async move {
                process_single_task(
                    bot,
                    task,
                    semaphore,
                    db_pool,
                    rate_limiter,
                    last_download_start,
                    alert_manager,
                    queue_for_cleanup,
                    worker_id,
                )
                .await;
            });
        }
    }
}

/// Process a single download task.
#[allow(clippy::too_many_arguments)]
async fn process_single_task(
    bot: Bot,
    task: queue::DownloadTask,
    semaphore: Arc<tokio::sync::Semaphore>,
    db_pool: Arc<DbPool>,
    rate_limiter: Arc<rate_limiter::RateLimiter>,
    last_download_start: Arc<tokio::sync::Mutex<std::time::Instant>>,
    alert_manager: Option<Arc<alerts::AlertManager>>,
    queue_for_cleanup: Arc<DownloadQueue>,
    worker_id: String,
) {
    // Acquire permit from semaphore (will wait if all permits are taken)
    let _permit = match semaphore.acquire().await {
        Ok(p) => p,
        Err(e) => {
            log::error!("Failed to acquire semaphore permit for task {}: {}", task.id, e);
            if let Ok(conn) = db::get_connection(&db_pool) {
                let _ = db::mark_task_failed(
                    &conn,
                    &task.id,
                    &worker_id,
                    &format!("Failed to acquire semaphore: {}", e),
                    false,
                    config::admin::MAX_TASK_RETRIES,
                );
            }
            queue_for_cleanup
                .remove_active_task(&task.url, task.chat_id, &task.format)
                .await;
            return;
        }
    };
    log::info!(
        "Processing task {} (permits available: {})",
        task.id,
        semaphore.available_permits()
    );

    // Enforce global delay between download starts.
    // Read timestamp and drop lock BEFORE sleeping to avoid blocking other tasks.
    let wait_time = {
        let last_start = last_download_start.lock().await;
        let elapsed = last_start.elapsed();
        let inter_delay = config::queue::inter_download_delay();
        if elapsed < inter_delay {
            Some(inter_delay - elapsed)
        } else {
            None
        }
    };
    if let Some(wait) = wait_time {
        log::info!(
            "Waiting {:?} before starting task {} (rate limit protection)",
            wait,
            task.id
        );
        tokio::time::sleep(wait).await;
    }
    {
        let mut last_start = last_download_start.lock().await;
        *last_start = std::time::Instant::now();
    }

    // Mark the task as processing
    if let Ok(conn) = db::get_connection(&db_pool) {
        if let Err(e) = db::mark_task_processing(&conn, &task.id, &worker_id) {
            log::warn!("Failed to mark task {} as processing: {}", task.id, e);
        }
    }

    let url = match url::Url::parse(&task.url) {
        Ok(u) => u,
        Err(e) => {
            log::error!("Invalid URL for task {}: {} - {}", task.id, task.url, e);
            let error_msg = format!("Invalid URL: {}", e);
            if let Ok(conn) = db::get_connection(&db_pool) {
                let _ = db::mark_task_failed(
                    &conn,
                    &task.id,
                    &worker_id,
                    &error_msg,
                    false,
                    config::admin::MAX_TASK_RETRIES,
                );
                notify_admin_task_failed(
                    bot.clone(),
                    Arc::clone(&db_pool),
                    &task.id,
                    task.chat_id.0,
                    &task.url,
                    &error_msg,
                    None,
                )
                .await;
            }
            queue_for_cleanup
                .remove_active_task(&task.url, task.chat_id, &task.format)
                .await;
            return;
        }
    };

    // Delete the "Task added to queue" notification after a short delay
    // so the user has time to read the queue position message
    {
        use teloxide::types::MessageId;
        let qmsg_id = queue_for_cleanup
            .take_notification_message(task.chat_id)
            .await
            .or(task.queue_message_id);
        if let Some(id) = qmsg_id {
            let bot_del = bot.clone();
            let chat_id_del = task.chat_id;
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                let _ = bot_del.delete_message(chat_id_del, MessageId(id)).await;
            });
        }
    }

    if let Some(msg_id) = task.message_id {
        use teloxide::types::MessageId;
        crate::telegram::try_set_reaction(&bot, task.chat_id, MessageId(msg_id), crate::telegram::emoji::EYES).await;
    }

    // Destructure task to avoid unnecessary clones
    let queue::DownloadTask {
        id: task_id,
        url: task_url,
        format: task_format,
        chat_id: task_chat_id,
        video_quality,
        audio_bitrate,
        time_range,
        message_id: task_message_id,
        created_timestamp,
        carousel_mask,
        with_lyrics,
        ..
    } = task;

    // Set carousel mask for Instagram carousel downloads
    if let Some(mask) = carousel_mask {
        crate::download::source::instagram::set_carousel_mask(&task_url, mask);
    }

    // Dispatch by format
    let result = match task_format.as_str() {
        "mp4" => {
            download_and_send_video(
                bot.clone(),
                task_chat_id,
                url,
                rate_limiter.clone(),
                created_timestamp,
                Some(Arc::clone(&db_pool)),
                video_quality,
                task_message_id,
                alert_manager.clone(),
                time_range.clone(),
            )
            .await
        }
        "srt" | "txt" => {
            download_and_send_subtitles(
                bot.clone(),
                task_chat_id,
                url,
                rate_limiter.clone(),
                created_timestamp,
                task_format.clone(),
                Some(Arc::clone(&db_pool)),
                task_message_id,
                alert_manager.clone(),
            )
            .await
        }
        _ => {
            // Default to audio (mp3)
            download_and_send_audio(
                bot.clone(),
                task_chat_id,
                url,
                rate_limiter.clone(),
                created_timestamp,
                Some(Arc::clone(&db_pool)),
                audio_bitrate,
                task_message_id,
                alert_manager.clone(),
                time_range.clone(),
                with_lyrics,
            )
            .await
        }
    };

    // Handle result
    match result {
        Ok(_) => {
            if let Ok(conn) = db::get_connection(&db_pool) {
                if let Err(e) = db::mark_task_completed(&conn, &task_id, &worker_id) {
                    log::warn!("Failed to mark task {} as completed: {}", task_id, e);
                }
            }
            log::info!("Task {} completed successfully", task_id);
        }
        Err(e) => {
            let admin_error_msg = format!("{:?}", e);
            let user_error_msg = sanitize_user_error_message(&e.to_string());
            log::error!(
                "Failed to process task {} (format: {}): {}",
                task_id,
                task_format,
                admin_error_msg
            );

            if let Ok(conn) = db::get_connection(&db_pool) {
                let should_retry = e.is_retryable();
                if let Err(db_err) = db::mark_task_failed(
                    &conn,
                    &task_id,
                    &worker_id,
                    &user_error_msg,
                    should_retry,
                    config::admin::MAX_TASK_RETRIES,
                ) {
                    log::error!("Failed to mark task {} as failed in DB: {}", task_id, db_err);
                } else {
                    let should_notify = db::get_task_by_id(&conn, &task_id)
                        .ok()
                        .flatten()
                        .is_some_and(|t| !should_retry || t.status == "dead_letter");
                    drop(conn);
                    if should_notify {
                        notify_admin_task_failed(
                            bot.clone(),
                            Arc::clone(&db_pool),
                            &task_id,
                            task_chat_id.0,
                            &task_url,
                            &admin_error_msg,
                            None,
                        )
                        .await;
                    }
                }
            }
        }
    }

    // Cleanup
    queue_for_cleanup
        .remove_active_task(&task_url, task_chat_id, &task_format)
        .await;

    {
        use teloxide::types::MessageId;
        if let Some(id) = queue_for_cleanup.take_notification_message(task_chat_id).await {
            let _ = bot.delete_message(task_chat_id, MessageId(id)).await;
        }
    }

    log::info!("Task {} processing finished, permit released", task_id);
}
