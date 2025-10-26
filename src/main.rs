use std::fs::read_to_string;
use std::sync::Arc;
use std::hash::Hash;
use teloxide::prelude::*;
use teloxide::types::{ParseMode, Message, BotCommand};
use teloxide::utils::command::BotCommands;
use teloxide::dispatching::{UpdateFilterExt, Dispatcher};
use std::time::Duration;
use anyhow::Result;
use tokio::signal;
use dptree::di::DependencyMap;
use reqwest::ClientBuilder;
use tokio::time::{sleep, interval};
use simplelog::*;
use std::fs::File;
use chrono::{DateTime, Utc};
use dotenvy::dotenv;

mod commands;
mod db;
mod downloader;
mod fetch;
mod rate_limiter;
mod utils;
mod queue;

use db::{get_connection, create_user, get_user, log_request};
use crate::commands::handle_message;
use crate::rate_limiter::RateLimiter;
use crate::queue::DownloadQueue;
use crate::downloader::{download_and_send_audio, download_and_send_video};

#[derive(BotCommands, Clone, Debug)]
#[command(rename_rule = "lowercase", description = "Я умею:")]
enum Command {
    #[command(description = "показывает главное меню")]
    Start,
    #[command(description = "показывает это сообщение")]
    Help,
    #[command(description = "показывает настройки")]
    Settings,
    #[command(description = "показывает активные скачивания")]
    Tasks,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize simplelog for both console and file logging
    CombinedLogger::init(vec![
        TermLogger::new(
            LevelFilter::Error,
            Config::default(),
            TerminalMode::Mixed,
            ColorChoice::Auto,
        ),
        WriteLogger::new(
            LevelFilter::Error,
            Config::default(),
            File::create("app.log").unwrap(),
        ),
    ])
    .unwrap();

    // Load environment variables from .env if present
    let _ = dotenv();

    log::info!("Starting bot...");

    let bot = Bot::from_env_with_client(
        ClientBuilder::new()
            .timeout(Duration::from_secs(300)) // Increase request timeout to 30 seconds
            .build()?,
    );

    let mut retry_count = 0;
    let max_retries = 5;

    // Set the list of bot commands
    bot.set_my_commands(vec![
        BotCommand::new("start", "показывает главное меню"),
        BotCommand::new("help", "расскажу что я могу, помимо вкусного чая"),
        BotCommand::new("settings", "твои настройки"),
        BotCommand::new("tasks", "активные скачивания"),
    ])
    .await?;

    // Read and apply the migration.sql file
    let migration_sql = read_to_string("migration.sql")?;
    let conn = get_connection()?;
    conn.execute_batch(&migration_sql)?;

    let rate_limiter = Arc::new(RateLimiter::new(Duration::from_secs(30)));
    let download_queue = Arc::new(DownloadQueue::new());

    // Start the queue processing
    tokio::spawn(process_queue(bot.clone(), Arc::clone(&download_queue), Arc::clone(&rate_limiter)));

    // Create a dispatcher to handle both commands and plain messages
    let handler = dptree::entry()
        .branch(Update::filter_message().branch(
            dptree::entry()
                .filter_command::<Command>()
                .endpoint(|bot: Bot, msg: Message, cmd: Command| async move {
                    println!("cmd {:?}", cmd);
                    // let tasks = download_queue.filter_tasks_by_chat_id(msg.chat.id);
                    match cmd {
                        Command::Start => {
                            let message = "Приветик\\! Я Дора ❤️‍🔥\\. Я делаю чай и скачиваю треки и видео\\. Используй /help чтобы получить полную инфу\\.".to_string();
                            bot.send_message(msg.chat.id, message)
                                .parse_mode(ParseMode::MarkdownV2)
                                .await?;
                        }
                        Command::Help => {
                            bot.send_message(msg.chat.id, Command::descriptions().to_string())
                                .parse_mode(ParseMode::MarkdownV2)
                                .await?;
                        }
                        Command::Settings => {
                            bot.send_message(msg.chat.id, "Ты можешь качать каждые 30 секунд\\! Дополнительные возможности на подходе")
                                .parse_mode(ParseMode::MarkdownV2)
                                .await?;
                        }
                        Command::Tasks => { 
                            // let tasks = download_queue.filter_tasks_by_chat_id(msg.chat.id);
                            bot.send_message(msg.chat.id, "У вас нет активных загрузок")
                                .parse_mode(ParseMode::MarkdownV2)
                                .await?;
                        }
                    }
                    respond(())
                })
        ))
        .branch(Update::filter_message().endpoint({
            let rate_limiter = Arc::clone(&rate_limiter);
            let download_queue = Arc::clone(&download_queue);
            move |bot: Bot, msg: Message| {
                let rate_limiter = Arc::clone(&rate_limiter);
                let download_queue = Arc::clone(&download_queue);
                async move {
                    if let Err(err) = handle_message(bot.clone(), msg.clone(), download_queue.clone(), rate_limiter.clone()).await {
                        log::error!("Error handling message: {:?}", err);
                    }

                    // Log request and manage user
                    let conn = get_connection().unwrap();
                    let chat_id = msg.chat.id.0; // Extract i64 from ChatId
                    if let Some(user) = get_user(&conn, chat_id).unwrap() {
                        log_request(&conn, user.telegram_id(), &msg.text().unwrap()).unwrap();
                    } else {
                        create_user(&conn, chat_id, msg.from().and_then(|u| u.username.clone())).unwrap();
                        log_request(&conn, chat_id, &msg.text().unwrap()).unwrap();
                    }
                    respond(())
                }
            }
        }));

    // Run the dispatcher with retry logic
    loop {
        let mut dispatcher = Dispatcher::builder(bot.clone(), handler.clone())
            .dependencies(DependencyMap::new())
            .build();

        if let Err(err) = run_dispatcher(&mut dispatcher).await {
            log::error!("Dispatcher error: {:?}", err);
            if retry_count < max_retries {
                retry_count += 1;
                exponential_backoff(retry_count).await;
            } else {
                log::error!("Max retries reached. Exiting...");
                break;
            }
        } else {
            retry_count = 0; // Reset retry count on success
        }

        // Add a delay between retries to avoid overwhelming the API
        if retry_count > 0 {
            sleep(Duration::from_secs(5)).await;
        }
    }

    tokio::select! {
        _ = signal::ctrl_c() => {
            println!("Shutting down gracefully...");
        },
    }

    Ok(())
}

async fn run_dispatcher<R, E, K>(dispatcher: &mut Dispatcher<R, E, K>) -> Result<(), ()>
where
    R: Requester + Clone + Send + Sync + 'static,
    R::GetUpdates: Send,
    R::SendMessage: Send,
    E: std::error::Error + Send + Sync + 'static,
    K: Eq + Hash + Clone + Send + Sync + 'static,
{
    dispatcher.dispatch().await;
    Ok(())
}

async fn exponential_backoff(retry_count: u32) {
    let delay = Duration::from_secs(2u64.pow(retry_count));
    tokio::time::sleep(delay).await;
}

/*
fn make_menu() -> KeyboardMarkup {
    let buttons = vec![
        vec![
            KeyboardButton::new("Option 1"),
            KeyboardButton::new("Option 2"),
        ],
        vec![
            KeyboardButton::new("Option 3"),
            KeyboardButton::new("Option 4"),
        ],
    ];
    KeyboardMarkup::new(buttons)
        .resize_keyboard(true)
        .one_time_keyboard(false)
}

async fn get_updates_with_retry(client: &Client, url: &str) -> Result<String, ReqwestError> {
    let retry_strategy = ExponentialBackoff::from_millis(100).take(5);

    let response = Retry::spawn(retry_strategy, || async {
        client
            .get(url)
            .timeout(Duration::from_secs(10)) // Set a timeout for the request
            .send()
            .await?
            .text()
            .await
    })
    .await?;

    Ok(response)
}
 */

async fn process_queue(bot: Bot, queue: Arc<DownloadQueue>, rate_limiter: Arc<rate_limiter::RateLimiter>) {
    // Semaphore to limit concurrent downloads (max 5 simultaneous tasks)
    let semaphore = Arc::new(tokio::sync::Semaphore::new(5));
    let mut interval = interval(Duration::from_millis(100)); // Check queue more frequently

    loop {
        interval.tick().await;
        if let Some(task) = queue.get_task().await {
            log::info!("Got task {} from queue", task.id);
            let bot = bot.clone();
            let rate_limiter = Arc::clone(&rate_limiter);
            let semaphore = Arc::clone(&semaphore);

            tokio::spawn(async move {
                // Acquire permit from semaphore (will wait if all permits are taken)
                let _permit = semaphore.acquire().await.unwrap();
                log::info!("Processing task {} (permits available: {})", task.id, semaphore.available_permits());

                let url = url::Url::parse(&task.url).expect("Invalid URL");
                if task.is_video {
                    if let Err(e) = download_and_send_video(bot.clone(), task.chat_id, url, rate_limiter, task.created_timestamp).await {
                        log::error!("Failed to process video task {}: {:?}", task.id, e);
                    }
                } else {
                    if let Err(e) = download_and_send_audio(bot.clone(), task.chat_id, url, rate_limiter, task.created_timestamp).await {
                        log::error!("Failed to process audio task {}: {:?}", task.id, e);
                    }
                }

                log::info!("Task {} completed, permit released", task.id);
                // Permit is automatically released when _permit goes out of scope
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    pub use crate::queue::DownloadQueue;
    pub use crate::queue::DownloadTask;

    #[tokio::test]
    async fn test_adding_and_retrieving_task() {
        let queue = DownloadQueue::new();
        let task = DownloadTask::new(
            "http://example.com/video.mp4".to_string(),
            teloxide::types::ChatId(123456789),
            true
        );

        // Test adding a task to the queue
        queue.add_task(task.clone()).await;
        assert_eq!(queue.queue.lock().await.len(), 1);

        // Test retrieving a task from the queue
        let retrieved_task = queue.get_task().await.unwrap();
        assert_eq!(retrieved_task.url, "http://example.com/video.mp4");
        assert_eq!(retrieved_task.chat_id, teloxide::types::ChatId(123456789));
        assert_eq!(retrieved_task.is_video, true);
    }

    #[tokio::test]
    async fn test_queue_empty_after_retrieval() {
        let queue = DownloadQueue::new();
        let task = DownloadTask::new(
            "http://example.com/audio.mp3".to_string(),
            teloxide::types::ChatId(987654321),
            false
        );

        queue.add_task(task).await;
        assert_eq!(queue.queue.lock().await.len(), 1);

        // After retrieving, the queue should be empty
        let _retrieved_task = queue.get_task().await.unwrap();
        assert!(queue.queue.lock().await.is_empty());
    }

    #[tokio::test]
    async fn test_multiple_tasks_handling() {
        let queue = DownloadQueue::new();
        let task1 = DownloadTask::new(
            "http://example.com/second.mp4".to_string(),
            teloxide::types::ChatId(111111111),
            true
        );
        let task2 = DownloadTask::new(
            "http://example.com/second.mp4".to_string(),
            teloxide::types::ChatId(111111111),
            false
        );
        queue.add_task(task2).await;
        queue.add_task(task1).await;

        // Check the count after adding tasks
        assert_eq!(queue.queue.lock().await.len(), 2);

        // Retrieve tasks and check the order and properties
        let first_retrieved_task = queue.get_task().await.unwrap();
        assert_eq!(first_retrieved_task.url, "http://example.com/second.mp4");
        assert_eq!(first_retrieved_task.chat_id, teloxide::types::ChatId(111111111));
        assert_eq!(first_retrieved_task.is_video, false);

        let second_retrieved_task = queue.get_task().await.unwrap();
        assert_eq!(second_retrieved_task.url, "http://example.com/second.mp4");
        assert_eq!(second_retrieved_task.chat_id, teloxide::types::ChatId(111111111));
        assert_eq!(second_retrieved_task.is_video, true);

        // After retrieving all tasks, the queue should be empty
        assert!(queue.queue.lock().await.is_empty());
    }

    #[tokio::test]
    async fn test_queue_empty_initially() {
        let queue = DownloadQueue::new();
        assert!(queue.queue.lock().await.is_empty());
    }
}
