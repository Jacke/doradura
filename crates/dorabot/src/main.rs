//! Doradura — Telegram bot for downloading and converting media.
//!
//! This is the entry point. CLI argument parsing and dispatch only.
//! Actual logic lives in:
//! - `startup.rs`       — bot initialization and dispatcher
//! - `cli_commands.rs`  — CLI download/info/refresh commands
//! - `background_tasks.rs` — periodic background tasks
//! - `queue_processor.rs`  — download queue processing

use anyhow::Result;
use dotenvy::dotenv;

use doradura::cli::{Cli, Commands, WebhookCommand};
use doradura::core::{config, init_logger};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse_args();

    // Set up global panic handler
    std::panic::set_hook(Box::new(|panic_info| {
        log::error!("Panic caught: {:?}", panic_info);
        if let Some(location) = panic_info.location() {
            log::error!("Panic at {}:{}:{}", location.file(), location.line(), location.column());
        }
        if let Some(msg) = panic_info.payload().downcast_ref::<&str>() {
            log::error!("Panic message: {}", msg);
        }
    }));

    init_logger(&config::LOG_FILE_PATH)?;
    let _ = dotenv();

    match cli.command {
        Some(Commands::Run { webhook }) => {
            log::info!("Running bot in normal mode (webhook: {})", webhook);
            doradura::startup::run_bot(webhook).await
        }
        Some(Commands::RunStaging { webhook }) => {
            log::info!("Running bot in staging mode (webhook: {})", webhook);
            if let Err(e) = dotenvy::from_filename_override(".env.staging") {
                anyhow::bail!("Failed to load .env.staging: {}", e);
            }
            // Safety: runs before any concurrent access to env vars
            unsafe { std::env::set_var("DORADURA_STAGING", "1") };
            doradura::startup::run_bot(webhook).await
        }
        Some(Commands::RunWithCookies { cookies, webhook }) => {
            log::info!("Running bot with cookies refresh (webhook: {})", webhook);
            if let Some(cookies_path) = cookies {
                // Safety: runs before any concurrent access to env vars
                std::env::set_var("YTDL_COOKIES_FILE", cookies_path);
            }
            doradura::startup::run_bot(webhook).await
        }
        Some(Commands::RefreshMetadata {
            limit,
            dry_run,
            verbose,
        }) => {
            log::info!(
                "Refreshing metadata (limit: {:?}, dry_run: {}, verbose: {})",
                limit,
                dry_run,
                verbose
            );
            doradura::cli_commands::run_metadata_refresh(limit, dry_run, verbose).await
        }
        Some(Commands::UpdateYtdlp { force, check }) => {
            log::info!("Managing yt-dlp (force: {}, check: {})", force, check);
            doradura::cli_commands::run_ytdlp_update(force, check).await
        }
        Some(Commands::Download {
            url,
            format,
            quality,
            bitrate,
            output,
            verbose,
        }) => doradura::cli_commands::run_cli_download(url, format, quality, bitrate, output, verbose).await,
        Some(Commands::Info { url, json }) => doradura::cli_commands::run_cli_info(url, json).await,
        Some(Commands::Webhook { command }) => {
            let bot = doradura::telegram::create_bot()?;
            match command {
                WebhookCommand::Set { drop_pending_updates } => {
                    doradura::webhook::set_webhook(&bot, drop_pending_updates).await
                }
                WebhookCommand::Delete { drop_pending_updates } => {
                    doradura::webhook::delete_webhook(&bot, drop_pending_updates).await
                }
                WebhookCommand::Info => doradura::webhook::print_webhook_info(&bot).await,
            }
        }
        None => {
            log::info!("No command specified, running bot in default mode");
            doradura::startup::run_bot(false).await
        }
    }
}

#[cfg(test)]
mod tests {
    pub use doradura::download::queue::DownloadQueue;
    pub use doradura::download::queue::DownloadTask;

    #[tokio::test]
    async fn test_adding_and_retrieving_task() {
        let queue = DownloadQueue::new();
        let task = DownloadTask::new(
            "http://example.com/video.mp4".to_string(),
            teloxide::types::ChatId(123456789),
            None,
            true,
            "mp4".to_string(),
            Some("1080p".to_string()),
            None,
        );

        queue.add_task(task.clone(), None).await;
        assert_eq!(queue.queue.lock().await.len(), 1);

        let retrieved_task = queue
            .get_task()
            .await
            .expect("Should retrieve task from non-empty queue");
        assert_eq!(retrieved_task.url, "http://example.com/video.mp4");
        assert_eq!(retrieved_task.chat_id, teloxide::types::ChatId(123456789));
        assert!(retrieved_task.is_video);
    }

    #[tokio::test]
    async fn test_queue_empty_after_retrieval() {
        let queue = DownloadQueue::new();
        let task = DownloadTask::new(
            "http://example.com/audio.mp3".to_string(),
            teloxide::types::ChatId(987654321),
            None,
            false,
            "mp3".to_string(),
            None,
            Some("320k".to_string()),
        );

        queue.add_task(task, None).await;
        assert_eq!(queue.queue.lock().await.len(), 1);

        let _retrieved_task = queue
            .get_task()
            .await
            .expect("Should retrieve task that was just added");
        assert!(queue.queue.lock().await.is_empty());
    }

    #[tokio::test]
    async fn test_multiple_tasks_handling() {
        let queue = DownloadQueue::new();
        let task1 = DownloadTask::new(
            "http://example.com/second.mp4".to_string(),
            teloxide::types::ChatId(111111111),
            None,
            true,
            "mp4".to_string(),
            Some("720p".to_string()),
            None,
        );
        let task2 = DownloadTask::new(
            "http://example.com/second.mp4".to_string(),
            teloxide::types::ChatId(111111111),
            None,
            false,
            "mp3".to_string(),
            None,
            Some("256k".to_string()),
        );
        queue.add_task(task2, None).await;
        queue.add_task(task1, None).await;

        assert_eq!(queue.queue.lock().await.len(), 2);

        let first_retrieved_task = queue.get_task().await.expect("Should retrieve first task from queue");
        assert_eq!(first_retrieved_task.url, "http://example.com/second.mp4");
        assert_eq!(first_retrieved_task.chat_id, teloxide::types::ChatId(111111111));
        assert!(!first_retrieved_task.is_video);

        let second_retrieved_task = queue.get_task().await.expect("Should retrieve second task from queue");
        assert_eq!(second_retrieved_task.url, "http://example.com/second.mp4");
        assert_eq!(second_retrieved_task.chat_id, teloxide::types::ChatId(111111111));
        assert!(second_retrieved_task.is_video);

        assert!(queue.queue.lock().await.is_empty());
    }

    #[tokio::test]
    async fn test_queue_empty_initially() {
        let queue = DownloadQueue::new();
        assert!(queue.queue.lock().await.is_empty());
    }
}
