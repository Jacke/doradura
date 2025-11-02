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
use dotenvy::dotenv;

mod commands;
mod config;
mod db;
mod downloader;
mod error;
mod fetch;
mod rate_limiter;
mod utils;
mod queue;
mod progress;
mod menu;
mod preview;
mod history;
mod stats;
mod export;
mod cache;
mod backup;
mod ytdlp;
mod subscription;

use db::{create_pool, get_connection, create_user, get_user, log_request};
use crate::commands::handle_message;
use crate::rate_limiter::RateLimiter;
use crate::queue::DownloadQueue;
use crate::downloader::{download_and_send_audio, download_and_send_video, download_and_send_subtitles};
use crate::menu::{show_main_menu, handle_menu_callback};
use crate::history::show_history;
use crate::stats::{show_user_stats, show_global_stats};
use crate::export::show_export_menu;
use crate::backup::{create_backup, list_backups};
use crate::subscription::show_subscription_info;
use std::env;

#[derive(BotCommands, Clone, Debug)]
#[command(rename_rule = "lowercase", description = "Я умею:")]
enum Command {
    #[command(description = "показывает главное меню")]
    Start,
    #[command(description = "настройки режима загрузки")]
    Mode,
    #[command(description = "история загрузок")]
    History,
    #[command(description = "личная статистика")]
    Stats,
    #[command(description = "глобальная статистика")]
    Global,
    #[command(description = "экспорт истории")]
    Export,
    #[command(description = "создать бэкап БД (только для администраторов)")]
    Backup,
    #[command(description = "информация о подписке и тарифах")]
    Plan,
}

/// Main entry point for the Telegram bot
/// 
/// Initializes logging, database connection pool, rate limiter, download queue,
/// and starts the Telegram bot dispatcher.
/// 
/// # Errors
/// 
/// Returns an error if initialization fails (logging, database, bot creation).
#[tokio::main]
async fn main() -> Result<()> {
    // Initialize simplelog for both console and file logging
    let log_file = File::create("app.log")
        .map_err(|e| anyhow::anyhow!("Failed to create log file: {}", e))?;
    
    CombinedLogger::init(vec![
        TermLogger::new(
            LevelFilter::Debug,  // Временно Debug для отладки прогресса
            Config::default(),
            TerminalMode::Mixed,
            ColorChoice::Auto,
        ),
        WriteLogger::new(
            LevelFilter::Debug,  // Временно Debug для отладки прогресса
            Config::default(),
            log_file,
        ),
    ])
    .map_err(|e| anyhow::anyhow!("Failed to initialize logger: {}", e))?;

    // Load environment variables from .env if present
    let _ = dotenv();

    log::info!("Starting bot...");

    // Check and update yt-dlp on startup
    if let Err(e) = ytdlp::check_and_update_ytdlp().await {
        log::warn!("Failed to check/update yt-dlp: {}. Continuing anyway.", e);
    }

    let bot = Bot::from_env_with_client(
        ClientBuilder::new()
            .timeout(config::network::timeout())
            .build()?,
    );

    let mut retry_count = 0;
    let max_retries = config::retry::MAX_DISPATCHER_RETRIES;

    // Set the list of bot commands
    bot.set_my_commands(vec![
        BotCommand::new("start", "показывает главное меню"),
        BotCommand::new("mode", "настройки режима загрузки"),
        BotCommand::new("history", "история загрузок"),
        BotCommand::new("stats", "личная статистика"),
        BotCommand::new("global", "глобальная статистика"),
        BotCommand::new("export", "экспорт истории"),
        BotCommand::new("backup", "создать бэкап БД (только для администраторов)"),
        BotCommand::new("plan", "информация о подписке и тарифах")
    ])
    .await?;

    // Create database connection pool
    let db_pool = Arc::new(create_pool("database.sqlite")
        .map_err(|e| anyhow::anyhow!("Failed to create database pool: {}", e))?);
    
    // Read and apply the migration.sql file
    let migration_sql = read_to_string("migration.sql")?;
    let conn = get_connection(&db_pool)
        .map_err(|e| anyhow::anyhow!("Failed to get database connection: {}", e))?;
    // Execute migration, but don't fail if some steps already exist
    if let Err(e) = conn.execute_batch(&migration_sql) {
        log::warn!("Some migration steps failed (this is normal if tables/columns already exist): {}", e);
    }

    let rate_limiter = Arc::new(RateLimiter::new());
    let download_queue = Arc::new(DownloadQueue::new());

    // Start the queue processing
    tokio::spawn(process_queue(bot.clone(), Arc::clone(&download_queue), Arc::clone(&rate_limiter), Arc::clone(&db_pool)));

    // Start automatic backup scheduler (daily backups)
    let db_path = "database.sqlite".to_string();
    tokio::spawn(async move {
        let mut interval = interval(Duration::from_secs(24 * 60 * 60)); // 24 hours
        loop {
            interval.tick().await;
            match create_backup(&db_path) {
                Ok(path) => log::info!("Automatic backup created: {}", path.display()),
                Err(e) => log::error!("Failed to create automatic backup: {}", e),
            }
        }
    });

    // Create a dispatcher to handle both commands and plain messages
    let handler = dptree::entry()
        .branch(Update::filter_message().branch(
            dptree::entry()
                .filter_command::<Command>()
                .endpoint({
                    let db_pool = Arc::clone(&db_pool);
                    move |bot: Bot, msg: Message, cmd: Command| {
                        let db_pool = Arc::clone(&db_pool);
                        async move {
                            log::debug!("Received command: {:?}", cmd);
                            match cmd {
                                Command::Start => {
                                    // Список file_id стикеров из стикерпака doraduradoradura
                                    let sticker_file_ids = vec![
                                        "CAACAgIAAxUAAWj-ZokEQu5YpTnjl6IWPzCQZ0UUAAJCEwAC52QwSC6nTghQdw-KNgQ",
                                        "CAACAgIAAxUAAWj-ZomIQgQKKpbMZA0_VDzfavIiAAK1GgACt8dBSNRj5YvFS-dmNgQ",
                                        "CAACAgIAAxUAAWj-Zokct93wagdDXh1JbhxBIyJOAALzFwACoktASAOjHltqzx0ENgQ",
                                        "CAACAgIAAxUAAWj-ZomorWU-YHGN6oQ6-ikN46CJAAInFAACqlJYSGHilrVqW1AxNgQ",
                                        "CAACAgIAAxUAAWj-ZonVzqfhCC1-YjDNhqGioqvVAALdEwAC-_ZpSB5PRC_sd93QNgQ",
                                        "CAACAgIAAxkBAAIFymj-YswNosbIex7SmXJejbO_GN7-AAJMGQAC9MFQSHBzdKlbjXskNgQ",
                                        "CAACAgIAAxUAAWj-Zol_H6tZIPG-PPHnpNZS1QkIAAJFGwACIQtBSDwm6rS-ZojVNgQ",
                                        "CAACAgIAAxUAAWj-ZomOtDnC9_6jFRp84js-HQN5AALzEgACqc5ISI4uefJ9dzZPNgQ",
                                        "CAACAgIAAxUAAWj-ZolmPZFTqhyNqwssS4JVQY_AAALgFAACU7NBSCIDa2YqXjXyNgQ",
                                        "CAACAgIAAxUAAWj-ZonZTWGW2DadfQ2Mo6bHAAHy2AACjxEAAgSTSUj1H3gU_UUHdjYE",
                                        "CAACAgIAAxUAAWj-ZolQ6OCfECavW19ATgcCup5PAAIOFgACgbdJSMOkkJfpAbs_NgQ",
                                        "CAACAgIAAxUAAWj-Zol19ilXmGth6SKa-4FRrSEJAAJRFwACM9JISKFYdRXvbsb1NgQ",
                                        "CAACAgIAAxUAAWj-ZokRA50GUCiz_OXQUih3uljfAAIeGQACsyBISDP8m_5FL5CJNgQ",
                                        "CAACAgIAAxUAAWj-ZomiM5Mt2aK1G3b8O7JK-shMAALPFQACWGhoSMeITTonc71ENgQ",
                                        "CAACAgIAAxUAAWj-ZomSF9AsKZr6myR3lYgyc-HyAAIRGQACM9KRSG5IUy40KB2KNgQ",
                                    ];

                                    // Генерируем случайный индекс используя timestamp
                                    use std::time::{SystemTime, UNIX_EPOCH};
                                    let random_index = match SystemTime::now().duration_since(UNIX_EPOCH) {
                                        Ok(timestamp) => (timestamp.as_nanos() as usize) % sticker_file_ids.len(),
                                        Err(e) => {
                                            log::error!("Failed to get system time: {}", e);
                                            // Fallback to a simple random index using length
                                            0
                                        }
                                    };
                                    let random_sticker_id = sticker_file_ids[random_index];

                                    // Отправляем случайный стикер
                                    let _ = bot.send_sticker(msg.chat.id, teloxide::types::InputFile::file_id(random_sticker_id)).await;

                                    // Отправляем приветственное сообщение и показываем mode меню
                                    let _ = bot.send_message(msg.chat.id, "Хэй\\! Я Дора, дай мне ссылку и я скачаю ❤️‍🔥")
                                        .parse_mode(ParseMode::MarkdownV2)
                                        .await;
                                }
                                Command::Mode => {
                                    let _ = show_main_menu(&bot, msg.chat.id, db_pool).await;
                                }
                                Command::History => {
                                    let _ = show_history(&bot, msg.chat.id, db_pool).await;
                                }
                                Command::Stats => {
                                    let _ = show_user_stats(&bot, msg.chat.id, db_pool).await;
                                }
                                Command::Global => {
                                    let _ = show_global_stats(&bot, msg.chat.id, db_pool).await;
                                }
                                Command::Export => {
                                    let _ = show_export_menu(&bot, msg.chat.id, db_pool).await;
                                }
                                Command::Backup => {
                                    // Проверяем, является ли пользователь администратором
                                    let admin_ids_str = env::var("ADMIN_IDS").unwrap_or_else(|_| String::new());
                                    let is_admin = admin_ids_str.split(',')
                                        .any(|id_str| {
                                            id_str.trim().parse::<i64>()
                                                .map(|id| id == msg.chat.id.0)
                                                .unwrap_or(false)
                                        });
                                    
                                    if is_admin {
                                        match create_backup("database.sqlite") {
                                            Ok(backup_path) => {
                                                let backups = list_backups().unwrap_or_default();
                                                let _ = bot.send_message(
                                                    msg.chat.id,
                                                    format!(
                                                        "✅ Бэкап создан успешно!\n\n📁 Путь: {}\n📊 Всего бэкапов: {}",
                                                        backup_path.display(),
                                                        backups.len()
                                                    )
                                                ).await;
                                            }
                                            Err(e) => {
                                                let _ = bot.send_message(
                                                    msg.chat.id,
                                                    format!("❌ Ошибка при создании бэкапа: {}", e)
                                                ).await;
                                            }
                                        }
                                    } else {
                                        let _ = bot.send_message(
                                            msg.chat.id,
                                            "❌ У тебя нет прав для выполнения этой команды."
                                        ).await;
                                    }
                                }
                                Command::Plan => {
                                    let _ = show_subscription_info(&bot, msg.chat.id, db_pool).await;
                                }
                            }
                            respond(())
                        }
                    }
                })
        ))
        .branch(Update::filter_message().endpoint({
            let rate_limiter = Arc::clone(&rate_limiter);
            let download_queue = Arc::clone(&download_queue);
            let db_pool = Arc::clone(&db_pool);
            move |bot: Bot, msg: Message| {
                let rate_limiter = Arc::clone(&rate_limiter);
                let download_queue = Arc::clone(&download_queue);
                let db_pool = Arc::clone(&db_pool);
                async move {
                    // Handle message and get user info (optimized: avoids duplicate DB query)
                    let user_info_result = handle_message(bot.clone(), msg.clone(), download_queue.clone(), rate_limiter.clone(), db_pool.clone()).await;
                    
                    // Log request and manage user (reuse user info if available)
                    if let Some(text) = msg.text() {
                        match &user_info_result {
                            Ok(Some(user)) => {
                                // User info already retrieved in handle_message, reuse it
                                match get_connection(&db_pool) {
                                    Ok(conn) => {
                                        if let Err(e) = log_request(&conn, user.telegram_id(), text) {
                                            log::error!("Failed to log request: {}", e);
                                        }
                                    }
                                    Err(e) => {
                                        log::error!("Failed to get database connection: {}", e);
                                    }
                                }
                            }
                            Ok(None) | Err(_) => {
                                // User not found or error occurred, try to get/create user
                                match get_connection(&db_pool) {
                                    Ok(conn) => {
                                        let chat_id = msg.chat.id.0;
                                        match get_user(&conn, chat_id) {
                                            Ok(Some(user)) => {
                                                if let Err(e) = log_request(&conn, user.telegram_id(), text) {
                                                    log::error!("Failed to log request: {}", e);
                                                }
                                            }
                                            Ok(None) => {
                                                if let Err(e) = create_user(&conn, chat_id, msg.from().and_then(|u| u.username.clone())) {
                                                    log::error!("Failed to create user: {}", e);
                                                } else if let Err(e) = log_request(&conn, chat_id, text) {
                                                    log::error!("Failed to log request for new user: {}", e);
                                                }
                                            }
                                            Err(e) => {
                                                log::error!("Failed to get user from database: {}", e);
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        log::error!("Failed to get database connection: {}", e);
                                    }
                                }
                            }
                        }
                    }
                    
                    if let Err(err) = user_info_result {
                        log::error!("Error handling message: {:?}", err);
                    }
                    
                    respond(())
                }
            }
        }))
        .branch(Update::filter_callback_query().endpoint({
            let db_pool = Arc::clone(&db_pool);
            let download_queue = Arc::clone(&download_queue);
            let rate_limiter = Arc::clone(&rate_limiter);
            move |bot: Bot, q: CallbackQuery| {
                let db_pool = Arc::clone(&db_pool);
                let download_queue = Arc::clone(&download_queue);
                let rate_limiter = Arc::clone(&rate_limiter);
                async move {
                    handle_menu_callback(bot, q, db_pool, download_queue, rate_limiter).await
                }
            }
        }));

    // Check if webhook mode is enabled
    let webhook_url = env::var("WEBHOOK_URL").ok();

    if let Some(url) = webhook_url {
        // Webhook mode
        log::info!("Starting bot in webhook mode at {}", url);
        
        // Delete existing webhook to ensure clean state
        let _ = bot.delete_webhook().await;
        
        // Set webhook
        bot.set_webhook(url::Url::parse(&url)?).await?;
        log::info!("Webhook set successfully");

        // Note: For full webhook support, you need to set up an HTTP server
        // (e.g., using axum) to receive webhook updates from Telegram.
        // For now, webhook URL is set but you need to handle incoming updates
        // via your HTTP server endpoint.
        // This is a placeholder - full implementation requires HTTP server setup.
        log::warn!("Webhook URL set to {}, but HTTP server is not implemented yet.", url);
        log::warn!("Please set up an HTTP server to receive webhook updates, or use polling mode.");
        
        // Keep the main thread alive
        tokio::select! {
            _ = signal::ctrl_c() => {
                log::info!("Shutting down gracefully...");
                bot.delete_webhook().await?;
            },
        }
    } else {
        // Long polling mode (default)
        log::info!("Starting bot in long polling mode");
        
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
                sleep(config::retry::dispatcher_delay()).await;
            }
        }
    }

    tokio::select! {
        _ = signal::ctrl_c() => {
            log::info!("Shutting down gracefully...");
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
    let delay = Duration::from_secs(config::retry::EXPONENTIAL_BACKOFF_BASE.pow(retry_count));
    tokio::time::sleep(delay).await;
}


async fn process_queue(bot: Bot, queue: Arc<DownloadQueue>, rate_limiter: Arc<rate_limiter::RateLimiter>, db_pool: Arc<db::DbPool>) {
    // Semaphore to limit concurrent downloads
    let semaphore = Arc::new(tokio::sync::Semaphore::new(config::queue::MAX_CONCURRENT_DOWNLOADS));
    let mut interval = interval(config::queue::check_interval());

    loop {
        interval.tick().await;
        if let Some(task) = queue.get_task().await {
            log::info!("Got task {} from queue", task.id);
            let bot = bot.clone();
            let rate_limiter = Arc::clone(&rate_limiter);
            let semaphore = Arc::clone(&semaphore);
            let db_pool = Arc::clone(&db_pool);

            tokio::spawn(async move {
                // Acquire permit from semaphore (will wait if all permits are taken)
                let _permit = match semaphore.acquire().await {
                    Ok(p) => p,
                    Err(e) => {
                        log::error!("Failed to acquire semaphore permit for task {}: {}", task.id, e);
                        return;
                    }
                };
                log::info!("Processing task {} (permits available: {})", task.id, semaphore.available_permits());

                let url = match url::Url::parse(&task.url) {
                    Ok(u) => u,
                    Err(e) => {
                        log::error!("Invalid URL for task {}: {} - {}", task.id, task.url, e);
                        return;
                    }
                };
                
                // Process task based on format
                let db_pool_clone = Arc::clone(&db_pool);
                let video_quality = task.video_quality.clone();
                let audio_bitrate = task.audio_bitrate.clone();
                let result = match task.format.as_str() {
                    "mp4" => {
                        download_and_send_video(bot.clone(), task.chat_id, url, rate_limiter.clone(), task.created_timestamp, Some(db_pool_clone.clone()), video_quality).await
                    }
                    "srt" | "txt" => {
                        download_and_send_subtitles(bot.clone(), task.chat_id, url, rate_limiter.clone(), task.created_timestamp, task.format.clone(), Some(db_pool_clone.clone())).await
                    }
                    _ => {
                        // Default to audio (mp3)
                        download_and_send_audio(bot.clone(), task.chat_id, url, rate_limiter.clone(), task.created_timestamp, Some(db_pool_clone.clone()), audio_bitrate).await
                    }
                };
                
                if let Err(e) = result {
                    log::error!("Failed to process task {} (format: {}): {:?}", task.id, task.format, e);
                }

                log::info!("Task {} completed, permit released", task.id);
                // Permit is automatically released when _permit goes out of scope
            });
        }
    }
}

#[cfg(test)]
mod tests {
    pub use crate::queue::DownloadQueue;
    pub use crate::queue::DownloadTask;

    #[tokio::test]
    async fn test_adding_and_retrieving_task() {
        let queue = DownloadQueue::new();
        let task = DownloadTask::new(
            "http://example.com/video.mp4".to_string(),
            teloxide::types::ChatId(123456789),
            true,
            "mp4".to_string(),
            Some("1080p".to_string()),
            None
        );

        // Test adding a task to the queue
        queue.add_task(task.clone()).await;
        assert_eq!(queue.queue.lock().await.len(), 1);

        // Test retrieving a task from the queue
        let retrieved_task = queue.get_task().await.expect("Should retrieve task from non-empty queue");
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
            false,
            "mp3".to_string(),
            None,
            Some("320k".to_string())
        );

        queue.add_task(task).await;
        assert_eq!(queue.queue.lock().await.len(), 1);

        // After retrieving, the queue should be empty
        let _retrieved_task = queue.get_task().await.expect("Should retrieve task that was just added");
        assert!(queue.queue.lock().await.is_empty());
    }

    #[tokio::test]
    async fn test_multiple_tasks_handling() {
        let queue = DownloadQueue::new();
        let task1 = DownloadTask::new(
            "http://example.com/second.mp4".to_string(),
            teloxide::types::ChatId(111111111),
            true,
            "mp4".to_string(),
            Some("720p".to_string()),
            None
        );
        let task2 = DownloadTask::new(
            "http://example.com/second.mp4".to_string(),
            teloxide::types::ChatId(111111111),
            false,
            "mp3".to_string(),
            None,
            Some("256k".to_string())
        );
        queue.add_task(task2).await;
        queue.add_task(task1).await;

        // Check the count after adding tasks
        assert_eq!(queue.queue.lock().await.len(), 2);

        // Retrieve tasks and check the order and properties
        let first_retrieved_task = queue.get_task().await.expect("Should retrieve first task from queue");
        assert_eq!(first_retrieved_task.url, "http://example.com/second.mp4");
        assert_eq!(first_retrieved_task.chat_id, teloxide::types::ChatId(111111111));
        assert_eq!(first_retrieved_task.is_video, false);

        let second_retrieved_task = queue.get_task().await.expect("Should retrieve second task from queue");
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
