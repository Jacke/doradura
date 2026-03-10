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
use crate::storage::SharedStorage;
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
    shared_storage: Arc<SharedStorage>,
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

    let heartbeat_storage = Arc::clone(&shared_storage);
    let heartbeat_worker_id = worker_id.clone();
    tokio::spawn(async move {
        let mut heartbeat_interval = tokio::time::interval(std::time::Duration::from_secs(HEARTBEAT_SECONDS));
        loop {
            heartbeat_interval.tick().await;
            if let Err(e) = heartbeat_storage
                .heartbeat_worker_leases(&heartbeat_worker_id, LEASE_SECONDS)
                .await
            {
                log::warn!("Queue heartbeat failed for {}: {}", heartbeat_worker_id, e);
            }
        }
    });

    let reaper_storage = Arc::clone(&shared_storage);
    tokio::spawn(async move {
        let mut reaper_interval = tokio::time::interval(std::time::Duration::from_secs(REAPER_SECONDS));
        loop {
            reaper_interval.tick().await;
            match reaper_storage
                .recover_expired_leases(config::admin::MAX_TASK_RETRIES)
                .await
            {
                Ok(recovered) if recovered > 0 => {
                    log::warn!("Recovered {} expired queue lease(s)", recovered);
                }
                Ok(_) => {}
                Err(e) => log::warn!("Queue reaper failed: {}", e),
            }
        }
    });

    loop {
        interval.tick().await;
        if semaphore.available_permits() == 0 {
            continue;
        }

        let claimed_task = match shared_storage.claim_next_task(&worker_id, LEASE_SECONDS).await {
            Ok(task) => task,
            Err(e) => {
                log::warn!("Failed to claim next queue task: {}", e);
                continue;
            }
        };

        if let Some(task_entry) = claimed_task {
            let task = queue::DownloadQueue::task_from_entry(task_entry);
            log::info!("Got task {} from queue", task.id);
            let bot = bot.clone();
            let rate_limiter = Arc::clone(&rate_limiter);
            let semaphore = Arc::clone(&semaphore);
            let shared_storage = Arc::clone(&shared_storage);
            let last_download_start = Arc::clone(&last_download_start);
            let alert_manager = alert_manager.clone();
            let queue_for_cleanup = Arc::clone(&queue);
            let worker_id = worker_id.clone();

            tokio::spawn(async move {
                process_single_task(
                    bot,
                    task,
                    semaphore,
                    shared_storage,
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
    shared_storage: Arc<SharedStorage>,
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
            let _ = shared_storage
                .mark_task_failed(
                    &task.id,
                    &worker_id,
                    &format!("Failed to acquire semaphore: {}", e),
                    false,
                    config::admin::MAX_TASK_RETRIES,
                )
                .await;
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
    if let Err(e) = shared_storage.mark_task_processing(&task.id, &worker_id).await {
        log::warn!("Failed to mark task {} as processing: {}", task.id, e);
    }

    let url = match url::Url::parse(&task.url) {
        Ok(u) => u,
        Err(e) => {
            log::error!("Invalid URL for task {}: {} - {}", task.id, task.url, e);
            let error_msg = format!("Invalid URL: {}", e);
            let _ = shared_storage
                .mark_task_failed(&task.id, &worker_id, &error_msg, false, config::admin::MAX_TASK_RETRIES)
                .await;
            notify_admin_task_failed(
                bot.clone(),
                shared_storage.sqlite_pool(),
                &task.id,
                task.chat_id.0,
                &task.url,
                &error_msg,
                None,
            )
            .await;
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
    let sqlite_pool = shared_storage.sqlite_pool();
    let result = match task_format.as_str() {
        "mp4" => {
            download_and_send_video(
                bot.clone(),
                task_chat_id,
                url,
                rate_limiter.clone(),
                created_timestamp,
                Some(Arc::clone(&sqlite_pool)),
                Some(Arc::clone(&shared_storage)),
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
                Some(Arc::clone(&sqlite_pool)),
                Some(Arc::clone(&shared_storage)),
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
                Some(Arc::clone(&sqlite_pool)),
                Some(Arc::clone(&shared_storage)),
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
            if let Err(e) = shared_storage.mark_task_completed(&task_id, &worker_id).await {
                log::warn!("Failed to mark task {} as completed: {}", task_id, e);
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

            let should_retry = e.is_retryable();
            match shared_storage
                .mark_task_failed(
                    &task_id,
                    &worker_id,
                    &user_error_msg,
                    should_retry,
                    config::admin::MAX_TASK_RETRIES,
                )
                .await
            {
                Ok(retry_scheduled) => {
                    if !retry_scheduled {
                        notify_admin_task_failed(
                            bot.clone(),
                            Arc::clone(&sqlite_pool),
                            &task_id,
                            task_chat_id.0,
                            &task_url,
                            &admin_error_msg,
                            None,
                        )
                        .await;
                    }
                }
                Err(db_err) => log::error!("Failed to mark task {} as failed in DB: {}", task_id, db_err),
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
