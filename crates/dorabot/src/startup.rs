//! Bot startup and initialization logic.
//!
//! Contains `run_bot()` — the main bot lifecycle:
//! 1. Initialize services (metrics, yt-dlp, bot instance)
//! 2. Connect to Bot API and verify identity
//! 3. Set up database, rate limiter, download queue
//! 4. Spawn background tasks
//! 5. Run the Telegram dispatcher (polling or webhook)

use anyhow::Result;
use std::sync::Arc;
use std::time::Duration;
use teloxide::prelude::*;
use tokio::signal;
use tokio::time::sleep;

use crate::background_tasks;
use crate::core::{config, log_cookies_configuration, rate_limiter::RateLimiter};
use crate::download::ytdlp;
use crate::download::DownloadQueue;
use crate::downsub::DownsubGateway;
use crate::queue_processor;
use crate::storage::create_pool;
use crate::telegram::handlers::HandlerError;
use crate::telegram::{create_bot, schema, setup_all_language_commands, HandlerDeps};

/// Run the Telegram bot with all services.
pub async fn run_bot(use_webhook: bool) -> Result<()> {
    let bot_init_start = std::time::Instant::now();
    log::info!("Starting bot...");

    // Validate configuration
    let config_result = config::validate();
    config_result.log();
    if !config_result.is_ok() {
        return Err(anyhow::anyhow!(
            "Configuration validation failed with {} error(s)",
            config_result.errors.len()
        ));
    }

    // Initialize metrics registry
    crate::core::metrics::init_metrics();

    // Log cookies configuration at startup
    log_cookies_configuration();

    // Check and update yt-dlp on startup
    if let Err(e) = ytdlp::check_and_update_ytdlp().await {
        log::warn!("Failed to check/update yt-dlp: {}. Continuing anyway.", e);
    }
    ytdlp::start_auto_update_task();

    // Create bot instance
    let bot = create_bot()?;

    // Connect to Bot API with retries
    let bot_info = connect_to_bot_api(&bot).await?;
    let bot_username = bot_info.username.as_deref();
    let bot_id = bot_info.id;
    log::info!("Bot username: {:?}, Bot ID: {}", bot_username, bot_id);

    if let Some(username) = bot_username {
        crate::core::copyright::set_bot_username(username);
    }

    setup_all_language_commands(&bot).await?;

    // Notify admin about startup
    {
        use crate::telegram::notifications::notify_admin_startup;
        notify_admin_startup(&bot, bot_username).await;
    }

    // Create database connection pool
    let db_pool = Arc::new(
        create_pool(&config::DATABASE_PATH).map_err(|e| anyhow::anyhow!("Failed to create database pool: {}", e))?,
    );

    // Initialize core services
    crate::core::error_logger::init_error_logger(Arc::clone(&db_pool));
    crate::download::audio_effects::start_cleanup_task(Arc::clone(&db_pool));

    let rate_limiter = Arc::new(RateLimiter::new());
    Arc::clone(&rate_limiter).spawn_cleanup_task(std::time::Duration::from_secs(300));

    let download_queue = Arc::new(DownloadQueue::new());

    let downsub_gateway = Arc::new(DownsubGateway::from_env());
    if downsub_gateway.is_available() {
        log::info!(
            "Downsub gRPC gateway enabled ({})",
            config::DOWNSUB_GRPC_ENDPOINT.as_deref().unwrap_or("<unknown>")
        );
    } else {
        log::info!("Downsub gRPC gateway disabled (DOWNSUB_GRPC_ENDPOINT unset or unreachable)");
    }

    // --- Spawn background tasks ---
    background_tasks::spawn_web_server(Arc::clone(&db_pool));
    background_tasks::spawn_metrics_server();

    let alert_manager = background_tasks::start_alert_monitor(bot.clone(), Arc::clone(&db_pool)).await;

    let _disk_monitor_handle = crate::core::disk::start_disk_monitor_task(alert_manager.clone());

    background_tasks::spawn_stats_reporter(bot.clone(), Arc::clone(&db_pool));
    background_tasks::spawn_health_checks(bot.clone());

    tokio::spawn(queue_processor::process_queue(
        bot.clone(),
        Arc::clone(&download_queue),
        Arc::clone(&rate_limiter),
        Arc::clone(&db_pool),
        alert_manager.clone(),
    ));

    background_tasks::spawn_subscription_expiry_checker(Arc::clone(&db_pool));
    background_tasks::spawn_cookies_checker(bot.clone());
    background_tasks::spawn_content_watcher(bot.clone(), Arc::clone(&db_pool));

    // Create extension registry
    let extension_registry = Arc::new(crate::extension::ExtensionRegistry::default_registry());

    // Create subtitle cache
    let subtitle_cache = Arc::new(crate::storage::SubtitleCache::new(&format!(
        "{}/subtitles",
        *config::DOWNLOAD_FOLDER
    )));

    // Create handler dependencies
    let handler_deps = HandlerDeps::new(
        Arc::clone(&db_pool),
        Arc::clone(&download_queue),
        Arc::clone(&rate_limiter),
        Arc::clone(&downsub_gateway),
        Arc::clone(&subtitle_cache),
        bot_username.map(|s| s.to_string()),
        bot_id,
        alert_manager,
        Arc::clone(&extension_registry),
    );

    let handler = schema(handler_deps);

    // --- Run dispatcher ---
    let webhook_url = if use_webhook { config::WEBHOOK_URL.clone() } else { None };

    if let Some(url) = webhook_url {
        run_webhook_mode(bot, &url).await
    } else {
        run_polling_mode(bot, handler, bot_init_start).await
    }
}

/// Connect to the Bot API with retry logic.
async fn connect_to_bot_api(bot: &crate::telegram::Bot) -> Result<teloxide::types::Me> {
    let startup_max_retries = 60; // Up to 5 minutes (60 * 5s)
    let mut startup_retry = 0;

    loop {
        match bot.get_me().await {
            Ok(info) => return Ok(info),
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
}

/// Run the bot in webhook mode.
async fn run_webhook_mode(bot: crate::telegram::Bot, url: &str) -> Result<()> {
    log::info!("Starting bot in webhook mode at {}", url);

    let _ = bot.delete_webhook().await;
    bot.set_webhook(url::Url::parse(url)?).await?;
    log::info!("Webhook set successfully");

    log::warn!("Webhook URL set to {}, but HTTP server is not implemented yet.", url);
    log::warn!("Please set up an HTTP server to receive webhook updates, or use polling mode.");

    tokio::select! {
        _ = signal::ctrl_c() => {
            log::info!("Shutting down gracefully...");
            bot.delete_webhook().await?;
        },
    }

    Ok(())
}

/// Run the bot in long polling mode with dispatcher retry logic.
async fn run_polling_mode(
    bot: crate::telegram::Bot,
    handler: teloxide::dispatching::UpdateHandler<HandlerError>,
    bot_init_start: std::time::Instant,
) -> Result<()> {
    let mut retry_count = 0;
    let max_retries = config::retry::MAX_DISPATCHER_RETRIES;

    let init_elapsed = bot_init_start.elapsed();
    log::info!("Starting bot in long polling mode");
    log::info!("================================================");
    log::info!("🎉 Bot initialization complete in {:.2}s", init_elapsed.as_secs_f64());
    log::info!("📡 Ready to receive updates!");
    log::info!("================================================");

    // Print startup timing summary if env vars are available
    if let Ok(container_start) = std::env::var("CONTAINER_START_MS") {
        if let Ok(start_ms) = container_start.parse::<u64>() {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            let total_elapsed = now_ms.saturating_sub(start_ms);
            log::info!(
                "⏱️  Total startup time from container start: {:.2}s",
                total_elapsed as f64 / 1000.0
            );
        }
    }

    loop {
        let bot_clone = bot.clone();
        let handler_clone = handler.clone();

        let handle = tokio::spawn(async move {
            use teloxide::prelude::*;
            use teloxide::update_listeners::Polling;

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
                log::info!("Dispatcher shutdown gracefully");
                break;
            }
            Err(join_err) => {
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

        if retry_count > 0 {
            sleep(config::retry::dispatcher_delay()).await;
        }
    }

    Ok(())
}

/// Exponential backoff delay for retries.
async fn exponential_backoff(retry_count: u32) {
    let delay = Duration::from_secs(config::retry::EXPONENTIAL_BACKOFF_BASE.pow(retry_count));
    sleep(delay).await;
}
