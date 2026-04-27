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
use tokio::time::sleep;

use crate::background_tasks;
use crate::core::{config, log_cookies_configuration, rate_limiter::RateLimiter};
use crate::download::DownloadQueue;
use crate::download::ytdlp;
use crate::downsub::DownsubGateway;
use crate::queue_processor;
use crate::storage::SharedStorage;
use crate::storage::create_pool;
use crate::telegram::handlers::HandlerError;
use crate::telegram::{HandlerDeps, create_bot, schema, setup_all_language_commands};

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
    crate::core::metrics::BUILD_INFO
        .with_label_values(&[env!("CARGO_PKG_VERSION")])
        .set(1.0);

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

    // Set online avatar
    if let Err(e) = crate::telegram::avatar::set_online_avatar(&bot).await {
        log::warn!("Failed to set online avatar: {}", e);
    }

    // Notify admin about startup
    {
        use crate::telegram::notifications::notify_admin_startup;
        notify_admin_startup(&bot, bot_username).await;
    }

    // Create database connection pool.
    // SQLite pool is always created (used for legacy operations).
    // If DATABASE_DRIVER=postgres, SharedStorage also connects to Postgres.
    let db_pool = Arc::new(
        create_pool(&config::DATABASE_PATH).map_err(|e| anyhow::anyhow!("Failed to create SQLite pool: {}", e))?,
    );
    let driver = *config::DATABASE_DRIVER;
    log::info!("Database driver: {:?}", driver);
    let shared_storage = SharedStorage::from_sqlite_pool(Arc::clone(&db_pool))
        .await
        .map_err(|e| anyhow::anyhow!("Failed to create shared storage backend ({:?}): {}", driver, e))?;

    // Initialize core services
    crate::core::error_logger::init_error_logger(Arc::clone(&shared_storage));
    crate::download::audio_effects::start_cleanup_task(Arc::clone(&shared_storage));

    let rate_limiter =
        Arc::new(RateLimiter::from_config().map_err(|e| anyhow::anyhow!("Failed to initialize rate limiter: {}", e))?);
    Arc::clone(&rate_limiter).spawn_cleanup_task(std::time::Duration::from_secs(300));

    let download_queue = Arc::new(DownloadQueue::with_storage(Some(Arc::clone(&shared_storage))));

    // Recover tasks that were in-progress when the previous session ended.
    // Any task still in leased/processing/uploading state belongs to a dead worker —
    // reset them all to pending so the queue processor picks them up immediately.
    match shared_storage.reset_in_progress_tasks_at_startup().await {
        Ok(0) => log::info!("Task recovery: no in-progress tasks from previous session"),
        Ok(n) => log::info!("Task recovery: reset {} in-progress task(s) to pending", n),
        Err(e) => log::warn!("Task recovery: failed to reset in-progress tasks: {}", e),
    }

    let downsub_gateway = Arc::new(DownsubGateway::from_env());
    if downsub_gateway.is_available() {
        log::info!(
            "Downsub gRPC gateway enabled ({})",
            config::DOWNSUB_GRPC_ENDPOINT.as_deref().unwrap_or("<unknown>")
        );
    } else {
        log::info!("Downsub gRPC gateway disabled (DOWNSUB_GRPC_ENDPOINT unset or unreachable)");
    }

    // --- Plan change notification channel ---
    let (plan_change_tx, plan_change_rx) = crate::core::plan_change_channel();

    // --- Spawn background tasks ---
    background_tasks::spawn_web_server(Arc::clone(&shared_storage), Some(plan_change_tx));
    background_tasks::spawn_metrics_server();

    let alert_manager = background_tasks::start_alert_monitor(bot.clone(), Arc::clone(&shared_storage)).await;

    let disk_alert_fn: Option<crate::core::disk::DiskAlertFn> = alert_manager.as_ref().map(|am| {
        let am = Arc::clone(am);
        std::sync::Arc::new(move |available_gb: f64, threshold_gb: f64| {
            let am = am.clone();
            Box::pin(async move { am.alert_low_disk_space(available_gb, threshold_gb).await })
                as std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send>>
        }) as crate::core::disk::DiskAlertFn
    });
    let _disk_monitor_handle = crate::core::disk::start_disk_monitor_task(disk_alert_fn);

    background_tasks::spawn_stats_reporter(bot.clone(), Arc::clone(&shared_storage)).await;
    background_tasks::spawn_health_checks(bot.clone());

    // Plan change notification dispatcher (listens for events from admin panel)
    spawn_plan_change_dispatcher(bot.clone(), plan_change_rx);

    tokio::spawn(queue_processor::process_queue(
        bot.clone(),
        Arc::clone(&download_queue),
        Arc::clone(&rate_limiter),
        Arc::clone(&shared_storage),
        alert_manager.clone(),
    ));

    background_tasks::spawn_subscription_expiry_checker(Arc::clone(&shared_storage)).await;
    background_tasks::spawn_cookies_checker(bot.clone(), Arc::clone(&shared_storage)).await;
    background_tasks::spawn_content_watcher(bot.clone(), Arc::clone(&db_pool), Arc::clone(&shared_storage)).await;
    background_tasks::spawn_db_cleanup(Arc::clone(&db_pool), Arc::clone(&shared_storage)).await;
    background_tasks::spawn_downloads_cleanup(Arc::clone(&shared_storage)).await;

    // Kill any orphan yt-dlp/ffmpeg processes left by a previous bot
    // generation (v0.49.2). Container restarts leave child processes
    // reparented to PID 1 — they keep running, hog CPU/RAM, and trip
    // over our high-res semaphore (which is process-local LazyLock).
    // Cleanup is best-effort: if pkill is missing or there are no
    // matches, we just continue.
    background_tasks::kill_orphan_media_processes().await;

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
        Arc::clone(&shared_storage),
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
    let bot_for_shutdown = bot.clone();
    let webhook_url = if use_webhook { config::WEBHOOK_URL.clone() } else { None };

    let result = if let Some(url) = webhook_url {
        log::info!("Webhook mode requested with public URL {}", url);
        crate::webhook::run_webhook_mode(bot, handler, Arc::clone(&shared_storage), bot_id, bot_init_start).await
    } else {
        run_polling_mode(bot, handler, bot_init_start, Arc::clone(&shared_storage)).await
    };

    // Set offline avatar before shutdown
    if let Err(e) = crate::telegram::avatar::set_offline_avatar(&bot_for_shutdown).await {
        log::warn!("Failed to set offline avatar: {}", e);
    }

    result
}

/// Connect to the Bot API with retry logic, jitter, and a hard 10-minute deadline.
///
/// Uses exponential backoff (1s → 2s → 4s … capped at 15s) with random jitter.
/// If Bot API is not reachable after 10 minutes, the process exits so the
/// container orchestrator can restart it cleanly.
async fn connect_to_bot_api(bot: &crate::telegram::Bot) -> Result<teloxide::types::Me> {
    use rand::Rng;

    const DEADLINE: Duration = Duration::from_secs(600); // 10 minutes
    const BASE_DELAY: Duration = Duration::from_secs(1);
    const MAX_DELAY: Duration = Duration::from_secs(15);

    let start = std::time::Instant::now();
    let mut attempt: u32 = 0;

    loop {
        attempt += 1;
        match bot.get_me().await {
            Ok(info) => {
                if attempt > 1 {
                    log::info!(
                        "Bot API connected after {} attempts ({:.1}s)",
                        attempt,
                        start.elapsed().as_secs_f64()
                    );
                }
                return Ok(info);
            }
            Err(e) => {
                let elapsed = start.elapsed();
                if elapsed >= DEADLINE {
                    return Err(anyhow::anyhow!(
                        "Bot API not reachable after {:.0}s ({} attempts)",
                        elapsed.as_secs_f64(),
                        attempt
                    ));
                }

                let err_str = e.to_string();
                let is_retryable = err_str.contains("restart")
                    || err_str.contains("network")
                    || err_str.contains("connection")
                    || err_str.contains("timed out")
                    || err_str.contains("Connection refused")
                    || err_str.contains("error sending request")
                    || err_str.contains("hyper");

                if !is_retryable {
                    return Err(anyhow::anyhow!("Bot API returned non-retryable error: {}", e));
                }

                // Exponential backoff with jitter: base * 2^attempt + random 0..500ms
                let exp_delay = BASE_DELAY
                    .saturating_mul(2u32.saturating_pow(attempt.min(10)))
                    .min(MAX_DELAY);
                let jitter = Duration::from_millis(rand::thread_rng().gen_range(0..500));
                let delay = exp_delay + jitter;

                let remaining = DEADLINE.saturating_sub(elapsed);
                log::warn!(
                    "Bot API not ready (attempt {}, {:.0}s/{:.0}s): {}. Retry in {:.1}s",
                    attempt,
                    elapsed.as_secs_f64(),
                    DEADLINE.as_secs_f64(),
                    err_str,
                    delay.as_secs_f64()
                );
                sleep(delay.min(remaining)).await;
            }
        }
    }
}

/// Background task: listen for plan change events and notify users via Telegram.
fn spawn_plan_change_dispatcher(bot: crate::telegram::Bot, mut rx: crate::core::PlanChangeReceiver) {
    use crate::core::{PlanChangeReason, escape_markdown};
    use crate::telegram::BotExt;

    tokio::spawn(async move {
        log::info!("Plan change notification dispatcher started");
        while let Some(event) = rx.recv().await {
            let plan_emoji = event.new_plan.emoji();
            let plan_name = event.new_plan.display_name();

            let reason_text = match event.reason {
                PlanChangeReason::Admin => "by an administrator",
                PlanChangeReason::Payment => "after successful payment",
                PlanChangeReason::Renewal => "automatically renewed",
                PlanChangeReason::Cancel => "auto\\-renewal cancelled",
            };

            let expiry_line = if let Some(ref exp) = event.expires_at {
                format!("\n📅 Valid until: {}", escape_markdown(exp))
            } else if event.new_plan.is_paid() {
                "\n♾️ Unlimited".to_string()
            } else {
                String::new()
            };

            let text = format!(
                "💳 *Plan updated*\n\n\
                 {} {} → {} {}\n\
                 Reason: {}{}\n\n\
                 Changes take effect immediately\\!",
                event.old_plan.emoji(),
                escape_markdown(event.old_plan.display_name()),
                plan_emoji,
                escape_markdown(plan_name),
                reason_text,
                expiry_line,
            );

            let chat_id = teloxide::types::ChatId(event.user_id);
            match bot.send_md(chat_id, &text).await {
                Err(e) => {
                    log::warn!("Failed to notify user {} about plan change: {}", event.user_id, e);
                }
                _ => {
                    log::info!(
                        "Notified user {} about plan change: {} → {} ({})",
                        event.user_id,
                        event.old_plan,
                        event.new_plan,
                        event.reason
                    );
                }
            }
        }
        log::warn!("Plan change notification dispatcher stopped (channel closed)");
    });
}

/// Run the bot in long polling mode with dispatcher retry logic.
async fn run_polling_mode(
    bot: crate::telegram::Bot,
    handler: teloxide::dispatching::UpdateHandler<HandlerError>,
    bot_init_start: std::time::Instant,
    shared_storage: Arc<SharedStorage>,
) -> Result<()> {
    let mut retry_count = 0;
    let max_retries = config::retry::MAX_DISPATCHER_RETRIES;

    // Record boot time for stale message filtering (must be before dispatching)
    crate::telegram::handlers::init_boot_timestamp();

    let init_elapsed = bot_init_start.elapsed();
    log::info!("Starting bot in long polling mode");
    log::info!("================================================");
    log::info!("🎉 Bot initialization complete in {:.2}s", init_elapsed.as_secs_f64());
    log::info!("📡 Ready to receive updates!");
    log::info!("================================================");

    // Print startup timing summary if env vars are available
    if let Ok(container_start) = std::env::var("CONTAINER_START_MS")
        && let Ok(start_ms) = container_start.parse::<u64>()
    {
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

    // Register SIGTERM handler once, before the retry loop.
    #[cfg(unix)]
    let mut sigterm = {
        use tokio::signal::unix::{SignalKind, signal};
        signal(SignalKind::terminate()).expect("Failed to register SIGTERM handler")
    };

    let shutdown_storage = Arc::clone(&shared_storage);
    loop {
        let bot_clone = bot.clone();
        let handler_clone = handler.clone();

        let handle = tokio::spawn(async move {
            use teloxide::prelude::*;
            use teloxide::update_listeners::Polling;

            // No drop_pending_updates() — payment events must never be lost.
            // Stale messages are filtered by is_fresh_message() in the handler chain.
            let listener = Polling::builder(bot_clone.clone()).build();

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

        #[cfg(unix)]
        let outcome = tokio::select! {
            result = handle => Some(result),
            _ = sigterm.recv() => {
                log::info!("SIGTERM received, shutting down gracefully");
                // Reset in-progress tasks so they're recovered on next startup
                match shutdown_storage.reset_in_progress_tasks_at_startup().await {
                    Ok(0) => log::info!("Shutdown: no in-progress tasks to reset"),
                    Ok(n) => log::info!("Shutdown: reset {} in-progress task(s) to pending for recovery", n),
                    Err(e) => log::warn!("Shutdown: failed to reset tasks: {}", e),
                }
                None
            }
        };

        #[cfg(not(unix))]
        let outcome = Some(handle.await);

        let join_result = match outcome {
            None => break, // SIGTERM received — exit the retry loop
            Some(r) => r,
        };

        match join_result {
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
                        crate::core::metrics::DISPATCHER_RECONNECTIONS_TOTAL.inc();
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
