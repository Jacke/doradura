use anyhow::Result;
use dotenvy::dotenv;
use std::sync::Arc;
use std::time::Duration;
use teloxide::prelude::*;
use tokio::signal;
use tokio::time::{interval, sleep};

use doradura::cli::{Cli, Commands};
use doradura::metadata_refresh;
// Use library modules
use doradura::core::{
    alerts, config, init_logger, log_cookies_configuration,
    rate_limiter::{self, RateLimiter},
    stats_reporter,
};
use doradura::download::queue::{self as queue};
use doradura::download::ytdlp::{self as ytdlp};
use doradura::download::ytdlp_errors::sanitize_user_error_message;
use doradura::download::{
    download_and_send_audio, download_and_send_subtitles, download_and_send_video, DownloadQueue,
};
use doradura::downsub::DownsubGateway;
use doradura::storage::db::{self as db, expire_old_subscriptions, get_failed_tasks};
use doradura::storage::{create_pool, get_connection};
use doradura::telegram::notifications::notify_admin_task_failed;
// DISABLED: Mini App not ready for production
// use doradura::telegram::webapp::run_webapp_server;
use doradura::telegram::Bot;
use doradura::telegram::{create_bot, schema, setup_all_language_commands, HandlerDeps};
use std::env;

/// Main entry point for the Telegram bot
///
/// Parses CLI arguments and dispatches to appropriate subcommand.
///
/// # Errors
/// Returns an error if initialization fails (logging, database, bot creation).
#[tokio::main]
async fn main() -> Result<()> {
    // Parse CLI arguments
    let cli = Cli::parse_args();

    // Set up global panic handler to catch panics in dispatcher
    // This allows us to log the panic and continue working instead of terminating
    std::panic::set_hook(Box::new(|panic_info| {
        log::error!("Panic caught: {:?}", panic_info);
        if let Some(location) = panic_info.location() {
            log::error!("Panic at {}:{}:{}", location.file(), location.line(), location.column());
        }
        if let Some(msg) = panic_info.payload().downcast_ref::<&str>() {
            log::error!("Panic message: {}", msg);
        }
    }));

    // Initialize logger (console + file)
    init_logger(&config::LOG_FILE_PATH)?;

    // Load environment variables from .env if present
    let _ = dotenv();

    // Dispatch to appropriate command
    match cli.command {
        Some(Commands::Run { webhook }) => {
            log::info!("Running bot in normal mode (webhook: {})", webhook);
            run_bot(webhook).await
        }
        Some(Commands::RunStaging { webhook }) => {
            log::info!("Running bot in staging mode (webhook: {})", webhook);
            // Load staging environment variables
            if let Err(e) = dotenvy::from_filename(".env.staging") {
                log::warn!("Failed to load .env.staging: {}", e);
            }
            run_bot(webhook).await
        }
        Some(Commands::RunWithCookies { cookies, webhook }) => {
            log::info!("Running bot with cookies refresh (webhook: {})", webhook);
            if let Some(cookies_path) = cookies {
                unsafe {
                    env::set_var("YOUTUBE_COOKIES_PATH", cookies_path);
                }
            }
            run_bot(webhook).await
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
            run_metadata_refresh(limit, dry_run, verbose).await
        }
        Some(Commands::UpdateYtdlp { force, check }) => {
            log::info!("Managing yt-dlp (force: {}, check: {})", force, check);
            run_ytdlp_update(force, check).await
        }
        None => {
            // No command specified - default to running the bot
            log::info!("No command specified, running bot in default mode");
            run_bot(false).await
        }
    }
}

/// Run the metadata refresh command
async fn run_metadata_refresh(limit: Option<usize>, dry_run: bool, verbose: bool) -> Result<()> {
    // Create database pool
    let db_pool = Arc::new(
        create_pool(&config::DATABASE_PATH).map_err(|e| anyhow::anyhow!("Failed to create database pool: {}", e))?,
    );

    // Get bot token
    let bot_token = config::BOT_TOKEN.to_string();
    if bot_token.is_empty() {
        return Err(anyhow::anyhow!("BOT_TOKEN environment variable not set"));
    }

    // Run metadata refresh
    metadata_refresh::refresh_missing_metadata(db_pool, bot_token, limit, dry_run, verbose).await?;

    Ok(())
}

/// Run yt-dlp update command
async fn run_ytdlp_update(force: bool, check: bool) -> Result<()> {
    if check {
        // Just check version without updating
        ytdlp::print_ytdlp_version().await?;
    } else if force {
        // Force update
        ytdlp::force_update_ytdlp().await?;
    } else {
        // Normal check and update (only if needed)
        ytdlp::check_and_update_ytdlp().await?;
    }
    Ok(())
}

/// Run the Telegram bot
async fn run_bot(use_webhook: bool) -> Result<()> {
    log::info!("Starting bot...");

    // Initialize metrics registry
    doradura::core::metrics::init_metrics();

    // Log cookies configuration at startup
    log_cookies_configuration();

    // Check and update yt-dlp on startup
    if let Err(e) = ytdlp::check_and_update_ytdlp().await {
        log::warn!("Failed to check/update yt-dlp: {}. Continuing anyway.", e);
    }

    // Start background auto-update task (every 6 hours)
    ytdlp::start_auto_update_task();

    // Create bot instance
    let bot = create_bot()?;

    let mut retry_count = 0;
    let max_retries = config::retry::MAX_DISPATCHER_RETRIES;

    // Get bot information to check mentions
    // Retry if Bot API is still initializing (returns "restart" error)
    let bot_info = {
        let startup_max_retries = 60; // Up to 5 minutes (60 * 5s)
        let mut startup_retry = 0;
        loop {
            match bot.get_me().await {
                Ok(info) => break info,
                Err(e) => {
                    let err_str = e.to_string();
                    let is_retryable = err_str.contains("restart")
                        || err_str.contains("network")
                        || err_str.contains("connection")
                        || err_str.contains("timed out")
                        || err_str.contains("Connection refused");

                    startup_retry += 1;
                    if startup_retry >= startup_max_retries || !is_retryable {
                        return Err(anyhow::anyhow!(
                            "Failed to connect to Bot API after {} retries: {}",
                            startup_retry,
                            e
                        ));
                    }

                    log::warn!(
                        "Bot API not ready (attempt {}/{}): {}. Retrying in 5 seconds...",
                        startup_retry,
                        startup_max_retries,
                        err_str
                    );
                    sleep(Duration::from_secs(5)).await;
                }
            }
        }
    };
    let bot_username = bot_info.username.as_deref();
    let bot_id = bot_info.id;
    log::info!("Bot username: {:?}, Bot ID: {}", bot_username, bot_id);

    // Set up bot commands for all languages
    setup_all_language_commands(&bot).await?;

    // Create database connection pool
    let db_pool = Arc::new(
        create_pool(&config::DATABASE_PATH).map_err(|e| anyhow::anyhow!("Failed to create database pool: {}", e))?,
    );

    // Initialize error logger
    doradura::core::error_logger::init_error_logger(Arc::clone(&db_pool));

    // Start audio effects cleanup task
    doradura::download::audio_effects::start_cleanup_task(Arc::clone(&db_pool));

    let rate_limiter = Arc::new(RateLimiter::new());
    let download_queue = Arc::new(DownloadQueue::new());
    let downsub_gateway = Arc::new(DownsubGateway::from_env().await);
    if downsub_gateway.is_available() {
        log::info!(
            "Downsub gRPC gateway enabled ({})",
            config::DOWNSUB_GRPC_ENDPOINT.as_deref().unwrap_or("<unknown>")
        );
    } else {
        log::info!("Downsub gRPC gateway disabled (DOWNSUB_GRPC_ENDPOINT unset or unreachable)");
    }

    // Do not restore failed tasks on startup; users should retry manually
    // recover_failed_tasks(&download_queue, &db_pool).await;

    // Start metrics HTTP server if enabled
    if *config::metrics::ENABLED {
        let metrics_port = *config::metrics::PORT;
        log::info!("Starting metrics server on port {}", metrics_port);

        tokio::spawn(async move {
            if let Err(e) = doradura::core::metrics_server::start_metrics_server(metrics_port).await {
                log::error!("Metrics server error: {}", e);
            }
        });

        // Start background task to update bot uptime counter every 60 seconds
        tokio::spawn(async {
            let mut interval = interval(Duration::from_secs(60));
            loop {
                interval.tick().await;
                doradura::core::metrics::BOT_UPTIME_SECONDS.inc_by(60.0);
            }
        });
    } else {
        log::info!("Metrics collection disabled (METRICS_ENABLED=false)");
    }

    // Start internal alert monitoring (sends Telegram alerts to admin based on metrics thresholds)
    let alert_manager: Option<Arc<alerts::AlertManager>> = if *config::alerts::ENABLED {
        let admin_user_id = *config::admin::ADMIN_USER_ID;
        if admin_user_id == 0 {
            log::warn!("Alerts enabled but ADMIN_USER_ID is not set; skipping alert monitor startup");
            None
        } else {
            let manager = alerts::start_alert_monitor(bot.clone(), ChatId(admin_user_id), Arc::clone(&db_pool)).await;
            log::info!("Internal alert monitor started");
            Some(manager)
        }
    } else {
        log::info!("Alerting disabled (ALERTS_ENABLED=false)");
        None
    };

    // Start periodic stats reporter (sends statistics to admin every STATS_REPORT_INTERVAL hours)
    {
        let admin_user_id = *config::admin::ADMIN_USER_ID;
        let interval_hours = env::var("STATS_REPORT_INTERVAL")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(3); // Default: every 3 hours

        if admin_user_id != 0 && interval_hours > 0 {
            let _stats_reporter = stats_reporter::start_stats_reporter(
                bot.clone(),
                ChatId(admin_user_id),
                Arc::clone(&db_pool),
                interval_hours,
            );
            log::info!("Stats reporter started (every {} hours)", interval_hours);
        } else if interval_hours == 0 {
            log::info!("Stats reporter disabled (STATS_REPORT_INTERVAL=0)");
        } else {
            log::warn!("Stats reporter disabled (ADMIN_USER_ID not set)");
        }
    }

    // DISABLED: Mini App web server — not ready for production yet
    // To re-enable, uncomment the block below and set WEBAPP_PORT env var
    //
    // if let Ok(webapp_port_str) = env::var("WEBAPP_PORT") {
    //     if let Ok(webapp_port) = webapp_port_str.parse::<u16>() {
    //         log::info!("Starting Mini App web server on port {}", webapp_port);
    //         let db_pool_webapp = Arc::clone(&db_pool);
    //         let download_queue_webapp = Arc::clone(&download_queue);
    //         let rate_limiter_webapp = Arc::clone(&rate_limiter);
    //         let bot_token_webapp = bot.token().to_string();
    //
    //         tokio::spawn(async move {
    //             if let Err(e) = run_webapp_server(
    //                 webapp_port,
    //                 db_pool_webapp,
    //                 download_queue_webapp,
    //                 rate_limiter_webapp,
    //                 bot_token_webapp,
    //             )
    //             .await
    //             {
    //                 log::error!("Mini App web server error: {}", e);
    //             }
    //         });
    //     } else {
    //         log::warn!("Invalid WEBAPP_PORT value: {}", webapp_port_str);
    //     }
    // } else {
    //     log::info!("WEBAPP_PORT not set, Mini App web server disabled");
    //     log::info!("Set WEBAPP_PORT environment variable to enable Mini App (e.g., WEBAPP_PORT=8080)");
    // }

    // Start the queue processing
    tokio::spawn(process_queue(
        bot.clone(),
        Arc::clone(&download_queue),
        Arc::clone(&rate_limiter),
        Arc::clone(&db_pool),
    ));

    // Start automatic backup scheduler (daily backups)
    //let db_path = config::DATABASE_PATH.to_string();
    // tokio::spawn(async move {
    //     let mut interval = interval(Duration::from_secs(24 * 60 * 60)); // 24 hours
    //     loop {
    //         interval.tick().await;
    //         match create_backup(&db_path) {
    //             Ok(path) => log::info!("Automatic backup created: {}", path.display()),
    //             Err(e) => log::error!("Failed to create automatic backup: {}", e),
    //         }
    //     }
    // });

    // Start automatic subscription expiry checker (every hour)
    let db_pool_expiry = Arc::clone(&db_pool);
    tokio::spawn(async move {
        let mut interval = interval(Duration::from_secs(60 * 60)); // 1 hour
        loop {
            interval.tick().await;
            match get_connection(&db_pool_expiry) {
                Ok(conn) => {
                    match expire_old_subscriptions(&conn) {
                        Ok(count) if count > 0 => {
                            log::info!("Expired {} subscription(s) automatically", count);
                        }
                        Ok(_) => {} // No expired subscriptions
                        Err(e) => log::error!("Failed to expire old subscriptions: {}", e),
                    }
                }
                Err(e) => log::error!("Failed to get DB connection for expiry check: {}", e),
            }
        }
    });

    // Start automatic cookies validation checker (every 5 minutes)
    let bot_cookies = bot.clone();
    tokio::spawn(async move {
        use doradura::download::cookies;
        use doradura::telegram::notify_admin_cookies_refresh;

        let mut interval = interval(Duration::from_secs(5 * 60)); // 5 minutes
        loop {
            interval.tick().await;
            log::debug!("Running periodic cookies validation check");

            if let Some(reason) = cookies::needs_refresh().await {
                log::warn!("🔴 Cookies need refresh: {}", reason);

                // Notify all admins
                let admin_ids = config::admin::ADMIN_IDS.clone();
                let primary_admin = *config::admin::ADMIN_USER_ID;

                let mut notified_admins = std::collections::HashSet::new();

                // Notify from ADMIN_IDS list
                for admin_id in admin_ids.iter() {
                    if notified_admins.insert(*admin_id) {
                        if let Err(e) = notify_admin_cookies_refresh(&bot_cookies, *admin_id, &reason).await {
                            log::error!("Failed to notify admin {} about cookies: {}", admin_id, e);
                        }
                    }
                }

                // Notify primary admin if not already notified
                if primary_admin != 0 && notified_admins.insert(primary_admin) {
                    if let Err(e) = notify_admin_cookies_refresh(&bot_cookies, primary_admin, &reason).await {
                        log::error!("Failed to notify primary admin {} about cookies: {}", primary_admin, e);
                    }
                }
            }
        }
    });

    // Create handler dependencies for the modular schema
    let handler_deps = HandlerDeps::new(
        Arc::clone(&db_pool),
        Arc::clone(&download_queue),
        Arc::clone(&rate_limiter),
        Arc::clone(&downsub_gateway),
        bot_username.map(|s| s.to_string()),
        bot_id,
        alert_manager,
    );

    // Create the dispatcher handler tree using the modular schema
    let handler = schema(handler_deps);

    // Check if webhook mode is enabled
    let webhook_url = if use_webhook { config::WEBHOOK_URL.clone() } else { None };

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
            let bot_clone = bot.clone();
            let handler_clone = handler.clone();

            // Create a new dispatcher in a separate task to isolate panics
            // "TX is dead" panics will be caught via the JoinHandle
            let handle = tokio::spawn(async move {
                use teloxide::prelude::*;
                use teloxide::update_listeners::Polling;

                // Create polling listener that drops pending updates on start
                let listener = Polling::builder(bot_clone.clone()).drop_pending_updates().build();

                Dispatcher::builder(bot_clone, handler_clone)
                    .dependencies(DependencyMap::new())
                    .enable_ctrlc_handler()
                    .build()
                    .dispatch_with_listener(
                        listener,
                        LoggingErrorHandler::with_custom_text("An error from the update listener"),
                    )
                    .await
            });

            match handle.await {
                Ok(()) => {
                    // Dispatcher finished normally
                    log::info!("Dispatcher shutdown gracefully");
                    break;
                }
                Err(join_err) => {
                    // Task was cancelled or panicked
                    if join_err.is_panic() {
                        let panic_msg = join_err.to_string();
                        log::error!("Dispatcher panicked: {}", panic_msg);

                        if panic_msg.contains("TX is dead") || panic_msg.contains("SendError") {
                            log::warn!("Detected TX is dead panic - will reconnect...");
                        }

                        if retry_count < max_retries {
                            retry_count += 1;
                            log::info!(
                                "Retrying dispatcher connection after panic (attempt {}/{})...",
                                retry_count,
                                max_retries
                            );
                            exponential_backoff(retry_count).await;
                        } else {
                            log::error!("Max retries reached after panic. Exiting...");
                            break;
                        }
                    } else {
                        log::warn!("Dispatcher task was cancelled: {}", join_err);
                        break;
                    }
                }
            }

            // Add a delay between retries to avoid overwhelming the API
            if retry_count > 0 {
                sleep(config::retry::dispatcher_delay()).await;
            }
        }
    }

    Ok(())
}

/// Recovers failed tasks from the database and adds them back to the queue.
///
/// Logs detailed information about each failed task before re-queuing it to
/// make debugging easier.
///
/// # Parameters
/// - `queue`: download queue that receives recovered tasks
/// - `db_pool`: database pool used to fetch failed tasks
#[allow(dead_code)]
async fn recover_failed_tasks(queue: &Arc<DownloadQueue>, db_pool: &Arc<db::DbPool>) {
    match get_connection(db_pool) {
        Ok(conn) => {
            match get_failed_tasks(&conn, config::admin::MAX_TASK_RETRIES) {
                Ok(failed_tasks) => {
                    if failed_tasks.is_empty() {
                        log::info!("✅ No failed tasks to recover - all tasks are completed or processing");
                        return;
                    }

                    let task_count = failed_tasks.len();
                    log::info!("═══════════════════════════════════════════════════════════");
                    log::info!("🔄 Found {} failed task(s) in database", task_count);
                    log::info!("═══════════════════════════════════════════════════════════");

                    // Log detailed information about each failed task
                    for (idx, task_entry) in failed_tasks.iter().enumerate() {
                        let priority_str = match task_entry.priority {
                            2 => "HIGH",
                            1 => "MEDIUM",
                            _ => "LOW",
                        };

                        let error_preview = task_entry
                            .error_message
                            .as_ref()
                            .map(|e| {
                                let preview = if e.len() > 100 {
                                    format!("{}...", &e[..100])
                                } else {
                                    e.clone()
                                };
                                preview.replace(['\n', '\r'], " ")
                            })
                            .unwrap_or_else(|| "No error message".to_string());

                        log::info!("  [{}/{}] Task ID: {}", idx + 1, task_count, task_entry.id);
                        log::info!("      └─ User ID: {}", task_entry.user_id);
                        log::info!("      └─ URL: {}", task_entry.url);
                        log::info!(
                            "      └─ Format: {} (video: {})",
                            task_entry.format,
                            task_entry.is_video
                        );
                        log::info!("      └─ Priority: {}", priority_str);
                        log::info!(
                            "      └─ Retry count: {}/{}",
                            task_entry.retry_count,
                            config::admin::MAX_TASK_RETRIES
                        );
                        log::info!("      └─ Created: {}", task_entry.created_at);
                        log::info!("      └─ Error: {}", error_preview);
                        log::info!("");
                    }

                    log::info!("═══════════════════════════════════════════════════════════");
                    log::info!("🔄 Starting recovery of {} failed task(s)...", task_count);
                    log::info!("═══════════════════════════════════════════════════════════");

                    let mut recovered_count = 0;

                    for task_entry in failed_tasks {
                        // Convert TaskQueueEntry into a DownloadTask
                        let priority = match task_entry.priority {
                            2 => queue::TaskPriority::High,
                            1 => queue::TaskPriority::Medium,
                            _ => queue::TaskPriority::Low,
                        };

                        let download_task = queue::DownloadTask {
                            id: task_entry.id.clone(),
                            url: task_entry.url.clone(),
                            chat_id: teloxide::types::ChatId(task_entry.user_id),
                            message_id: None, // Recovered tasks don't have original message
                            is_video: task_entry.is_video,
                            format: task_entry.format.clone(),
                            video_quality: task_entry.video_quality.clone(),
                            audio_bitrate: task_entry.audio_bitrate.clone(),
                            created_timestamp: chrono::DateTime::parse_from_rfc3339(&task_entry.created_at)
                                .map(|dt| dt.with_timezone(&chrono::Utc))
                                .unwrap_or_else(|_| chrono::Utc::now()),
                            priority,
                        };

                        // Add the task back to the queue
                        queue.add_task(download_task, Some(Arc::clone(db_pool))).await;
                        recovered_count += 1;
                        log::info!(
                            "  ✅ Recovered task {} (retry: {}/{}) - URL: {}",
                            task_entry.id,
                            task_entry.retry_count + 1,
                            config::admin::MAX_TASK_RETRIES,
                            task_entry.url
                        );
                    }

                    log::info!("═══════════════════════════════════════════════════════════");
                    log::info!("✅ Recovery completed:");
                    log::info!("   • Found in DB: {} task(s)", task_count);
                    log::info!("   • Successfully recovered: {} task(s)", recovered_count);
                    log::info!("═══════════════════════════════════════════════════════════");
                }
                Err(e) => {
                    log::error!("❌ Failed to get failed tasks from database: {}", e);
                }
            }
        }
        Err(e) => {
            log::error!("❌ Failed to get DB connection for task recovery: {}", e);
        }
    }
}

async fn process_queue(
    bot: Bot,
    queue: Arc<DownloadQueue>,
    rate_limiter: Arc<rate_limiter::RateLimiter>,
    db_pool: Arc<db::DbPool>,
) {
    // Semaphore to limit concurrent downloads
    let semaphore = Arc::new(tokio::sync::Semaphore::new(config::queue::MAX_CONCURRENT_DOWNLOADS));
    let mut interval = interval(config::queue::check_interval());
    // Track last download start to enforce global delay between downloads
    let last_download_start = Arc::new(tokio::sync::Mutex::new(std::time::Instant::now()));

    loop {
        interval.tick().await;
        if let Some(task) = queue.get_task().await {
            log::info!("Got task {} from queue", task.id);
            let bot = bot.clone();
            let rate_limiter = Arc::clone(&rate_limiter);
            let semaphore = Arc::clone(&semaphore);
            let db_pool = Arc::clone(&db_pool);
            let last_download_start = Arc::clone(&last_download_start);

            tokio::spawn(async move {
                // Acquire permit from semaphore (will wait if all permits are taken)
                let _permit = match semaphore.acquire().await {
                    Ok(p) => p,
                    Err(e) => {
                        log::error!("Failed to acquire semaphore permit for task {}: {}", task.id, e);
                        // Mark the task as failed
                        if let Ok(conn) = db::get_connection(&db_pool) {
                            let _ =
                                db::mark_task_failed(&conn, &task.id, &format!("Failed to acquire semaphore: {}", e));
                        }
                        return;
                    }
                };
                log::info!(
                    "Processing task {} (permits available: {})",
                    task.id,
                    semaphore.available_permits()
                );

                // Enforce global delay between download starts to avoid YouTube rate limiting
                {
                    let mut last_start = last_download_start.lock().await;
                    let elapsed = last_start.elapsed();
                    let inter_delay = config::queue::inter_download_delay();
                    if elapsed < inter_delay {
                        let wait_time = inter_delay - elapsed;
                        log::info!(
                            "Waiting {:?} before starting task {} (rate limit protection)",
                            wait_time,
                            task.id
                        );
                        tokio::time::sleep(wait_time).await;
                    }
                    *last_start = std::time::Instant::now();
                }

                // Mark the task as processing
                if let Ok(conn) = db::get_connection(&db_pool) {
                    if let Err(e) = db::mark_task_processing(&conn, &task.id) {
                        log::warn!("Failed to mark task {} as processing: {}", task.id, e);
                    }
                }

                let url = match url::Url::parse(&task.url) {
                    Ok(u) => u,
                    Err(e) => {
                        log::error!("Invalid URL for task {}: {} - {}", task.id, task.url, e);
                        let error_msg = format!("Invalid URL: {}", e);
                        // Mark the task as failed
                        if let Ok(conn) = db::get_connection(&db_pool) {
                            let _ = db::mark_task_failed(&conn, &task.id, &error_msg);
                            // Notify the administrator
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
                        return;
                    }
                };

                if let Some(msg_id) = task.message_id {
                    use teloxide::types::MessageId;
                    doradura::telegram::try_set_reaction(
                        &bot,
                        task.chat_id,
                        MessageId(msg_id),
                        doradura::telegram::emoji::EYES,
                    )
                    .await;
                }

                // Process task based on format
                let db_pool_clone = Arc::clone(&db_pool);
                let video_quality = task.video_quality.clone();
                let audio_bitrate = task.audio_bitrate.clone();
                let task_id = task.id.clone();
                let task_url = task.url.clone();
                let task_format = task.format.clone();
                let task_chat_id = task.chat_id;
                let result = match task.format.as_str() {
                    "mp4" => {
                        download_and_send_video(
                            bot.clone(),
                            task.chat_id,
                            url,
                            rate_limiter.clone(),
                            task.created_timestamp,
                            Some(db_pool_clone.clone()),
                            video_quality,
                            task.message_id,
                        )
                        .await
                    }
                    "srt" | "txt" => {
                        download_and_send_subtitles(
                            bot.clone(),
                            task.chat_id,
                            url,
                            rate_limiter.clone(),
                            task.created_timestamp,
                            task.format.clone(),
                            Some(db_pool_clone.clone()),
                            task.message_id,
                        )
                        .await
                    }
                    _ => {
                        // Default to audio (mp3)
                        download_and_send_audio(
                            bot.clone(),
                            task.chat_id,
                            url,
                            rate_limiter.clone(),
                            task.created_timestamp,
                            Some(db_pool_clone.clone()),
                            audio_bitrate,
                            task.message_id,
                        )
                        .await
                    }
                };

                match result {
                    Ok(_) => {
                        // Mark the task as completed
                        if let Ok(conn) = db::get_connection(&db_pool) {
                            if let Err(e) = db::mark_task_completed(&conn, &task_id) {
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

                        // Mark the task as failed
                        if let Ok(conn) = db::get_connection(&db_pool) {
                            if let Err(db_err) = db::mark_task_failed(&conn, &task_id, &user_error_msg) {
                                log::error!("Failed to mark task {} as failed in DB: {}", task_id, db_err);
                            } else {
                                // Notify the administrator only if the task has not exceeded retry limits
                                if let Ok(conn) = db::get_connection(&db_pool) {
                                    if let Ok(Some(task_entry)) = db::get_task_by_id(&conn, &task_id) {
                                        if task_entry.retry_count < config::admin::MAX_TASK_RETRIES {
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
                    }
                }

                log::info!("Task {} processing finished, permit released", task_id);
                // Permit is automatically released when _permit goes out of scope
            });
        }
    }
}

/// Exponential backoff delay for retries
async fn exponential_backoff(retry_count: u32) {
    let delay = Duration::from_secs(config::retry::EXPONENTIAL_BACKOFF_BASE.pow(retry_count));
    sleep(delay).await;
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

        // Test adding a task to the queue
        queue.add_task(task.clone(), None).await;
        assert_eq!(queue.queue.lock().await.len(), 1);

        // Test retrieving a task from the queue
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

        // After retrieving, the queue should be empty
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

        // Check the count after adding tasks
        assert_eq!(queue.queue.lock().await.len(), 2);

        // Retrieve tasks and check the order and properties
        let first_retrieved_task = queue.get_task().await.expect("Should retrieve first task from queue");
        assert_eq!(first_retrieved_task.url, "http://example.com/second.mp4");
        assert_eq!(first_retrieved_task.chat_id, teloxide::types::ChatId(111111111));
        assert!(!first_retrieved_task.is_video);

        let second_retrieved_task = queue.get_task().await.expect("Should retrieve second task from queue");
        assert_eq!(second_retrieved_task.url, "http://example.com/second.mp4");
        assert_eq!(second_retrieved_task.chat_id, teloxide::types::ChatId(111111111));
        assert!(second_retrieved_task.is_video);

        // After retrieving all tasks, the queue should be empty
        assert!(queue.queue.lock().await.is_empty());
    }

    #[tokio::test]
    async fn test_queue_empty_initially() {
        let queue = DownloadQueue::new();
        assert!(queue.queue.lock().await.is_empty());
    }
}
