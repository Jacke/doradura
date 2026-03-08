//! Background task spawners for the bot.
//!
//! Each function spawns a `tokio::spawn` task that runs periodically.
//! Extracted from `run_bot()` in main.rs for clarity and testability.

use sqlx::{pool::PoolConnection, Postgres};
use std::sync::Arc;
use std::time::Duration;
use teloxide::prelude::*;
use tokio::time::interval;

use crate::core::{alerts, config, stats_reporter};
use crate::storage::db::DbPool;
use crate::storage::SharedStorage;
use crate::telegram::Bot;

const LOCK_ALERT_MONITOR: i64 = 1101;
const LOCK_STATS_REPORTER: i64 = 1102;
const LOCK_UPDATES_CLEANUP: i64 = 1002;

async fn try_acquire_pg_singleton_lock(
    shared_storage: &Arc<SharedStorage>,
    lock_id: i64,
    name: &str,
) -> Option<PoolConnection<Postgres>> {
    let SharedStorage::Postgres { pg_pool, .. } = shared_storage.as_ref() else {
        return None;
    };

    let mut conn = match pg_pool.acquire().await {
        Ok(conn) => conn,
        Err(e) => {
            log::warn!("Failed to acquire PostgreSQL connection for {} lock: {}", name, e);
            return None;
        }
    };

    match sqlx::query_scalar::<_, bool>("SELECT pg_try_advisory_lock($1)")
        .bind(lock_id)
        .fetch_one(&mut *conn)
        .await
    {
        Ok(true) => {
            log::info!("Acquired singleton lock {} for {}", lock_id, name);
            Some(conn)
        }
        Ok(false) => {
            log::info!(
                "Skipping {} on this instance: singleton lock {} is held elsewhere",
                name,
                lock_id
            );
            None
        }
        Err(e) => {
            log::warn!("Failed to acquire singleton lock {} for {}: {}", lock_id, name, e);
            None
        }
    }
}

/// Start the subscription expiry checker (every hour).
///
/// Automatically expires subscriptions past their expiry date.
pub fn spawn_subscription_expiry_checker(shared_storage: Arc<SharedStorage>) {
    tokio::spawn(async move {
        let mut interval = interval(Duration::from_secs(60 * 60));
        loop {
            interval.tick().await;
            match shared_storage.expire_old_subscriptions().await {
                Ok(count) if count > 0 => {
                    log::info!("Expired {} subscription(s) automatically", count);
                }
                Ok(_) => {}
                Err(e) => log::error!("Failed to expire old subscriptions: {}", e),
            }
        }
    });
}

/// Start the cookies validation checker (every 5 minutes).
///
/// Notifies admins when YouTube cookies need refresh.
pub fn spawn_cookies_checker(bot: Bot) {
    tokio::spawn(async move {
        use crate::download::cookies;
        use crate::telegram::notify_admin_cookies_refresh;

        let mut interval = interval(Duration::from_secs(5 * 60));
        loop {
            interval.tick().await;
            log::debug!("Running periodic cookies validation check");

            if let Some(reason) = cookies::needs_refresh().await {
                log::warn!("🔴 Cookies need refresh: {}", reason);

                let admin_ids = config::admin::ADMIN_IDS.clone();
                let primary_admin = *config::admin::ADMIN_USER_ID;
                let mut notified_admins = std::collections::HashSet::new();

                for admin_id in admin_ids.iter() {
                    if notified_admins.insert(*admin_id) {
                        if let Err(e) = notify_admin_cookies_refresh(&bot, *admin_id, &reason).await {
                            log::error!("Failed to notify admin {} about cookies: {}", admin_id, e);
                        }
                    }
                }

                if primary_admin != 0 && notified_admins.insert(primary_admin) {
                    if let Err(e) = notify_admin_cookies_refresh(&bot, primary_admin, &reason).await {
                        log::error!("Failed to notify primary admin {} about cookies: {}", primary_admin, e);
                    }
                }
            }
        }
    });
}

pub fn spawn_content_watcher(bot: Bot, db_pool: Arc<DbPool>, shared_storage: Arc<SharedStorage>) {
    use crate::watcher::{scheduler, WatcherRegistry};

    let watcher_registry = Arc::new(WatcherRegistry::default_registry());
    let notification_rx = scheduler::start_scheduler(
        Arc::clone(&db_pool),
        Arc::clone(&shared_storage),
        Arc::clone(&watcher_registry),
    );
    crate::telegram::subscriptions::start_notification_dispatcher(
        bot,
        Arc::clone(&db_pool),
        Arc::clone(&shared_storage),
        notification_rx,
    );
    log::info!("Content watcher scheduler and notification dispatcher started");
}

/// Start the web server for share pages (if WEB_BASE_URL is configured).
pub fn spawn_web_server(shared_storage: Arc<SharedStorage>) {
    if config::share::base_url().is_some() {
        let web_port = config::share::web_port();
        log::info!("Starting web server on port {} (WEB_BASE_URL configured)", web_port);
        tokio::spawn(async move {
            if let Err(e) = crate::core::web_server::start_web_server(web_port, shared_storage).await {
                log::error!("Web server failed: {}", e);
            }
        });
    } else {
        log::info!("Web server disabled (WEB_BASE_URL not set)");
    }
}

/// Start metrics HTTP server and uptime counter if enabled.
pub fn spawn_metrics_server() {
    if *config::metrics::ENABLED {
        let metrics_port = *config::metrics::PORT;
        log::info!("Starting metrics server on port {}", metrics_port);

        tokio::spawn(async move {
            if let Err(e) = crate::core::metrics_server::start_metrics_server(metrics_port).await {
                log::error!("Metrics server error: {}", e);
            }
        });

        // Uptime counter
        tokio::spawn(async {
            let mut interval = interval(Duration::from_secs(60));
            loop {
                interval.tick().await;
                crate::core::metrics::BOT_UPTIME_SECONDS.inc_by(60.0);
            }
        });
    } else {
        log::info!("Metrics collection disabled (METRICS_ENABLED=false)");
    }
}

/// Start the internal alert monitor.
///
/// Returns `Some(AlertManager)` if alerts are enabled and admin is configured.
pub async fn start_alert_monitor(bot: Bot, shared_storage: Arc<SharedStorage>) -> Option<Arc<alerts::AlertManager>> {
    if !*config::alerts::ENABLED {
        log::info!("Alerting disabled (ALERTS_ENABLED=false)");
        return None;
    }

    let admin_user_id = *config::admin::ADMIN_USER_ID;
    if admin_user_id == 0 {
        log::warn!("Alerts enabled but ADMIN_USER_ID is not set; skipping alert monitor startup");
        return None;
    }

    let lock_conn = match shared_storage.as_ref() {
        SharedStorage::Sqlite { .. } => None,
        SharedStorage::Postgres { .. } => {
            try_acquire_pg_singleton_lock(&shared_storage, LOCK_ALERT_MONITOR, "alert monitor").await
        }
    };

    let manager = alerts::start_alert_monitor(bot, ChatId(admin_user_id), Arc::clone(&shared_storage), lock_conn).await;
    log::info!("Internal alert monitor started");
    Some(manager)
}

/// Start the periodic stats reporter.
pub async fn spawn_stats_reporter(bot: Bot, shared_storage: Arc<SharedStorage>) {
    let admin_user_id = *config::admin::ADMIN_USER_ID;
    let interval_hours = std::env::var("STATS_REPORT_INTERVAL")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(3);

    if admin_user_id != 0 && interval_hours > 0 {
        let lock_conn = match shared_storage.as_ref() {
            SharedStorage::Sqlite { .. } => None,
            SharedStorage::Postgres { .. } => {
                match try_acquire_pg_singleton_lock(&shared_storage, LOCK_STATS_REPORTER, "stats reporter").await {
                    Some(conn) => Some(conn),
                    None => return,
                }
            }
        };
        let _stats_reporter = stats_reporter::start_stats_reporter(
            bot,
            ChatId(admin_user_id),
            Arc::clone(&shared_storage),
            interval_hours,
            lock_conn,
        );
        log::info!("Stats reporter started (every {} hours)", interval_hours);
    } else if interval_hours == 0 {
        log::info!("Stats reporter disabled (STATS_REPORT_INTERVAL=0)");
    } else {
        log::warn!("Stats reporter disabled (ADMIN_USER_ID not set)");
    }
}

/// Start periodic database cleanup (every 6 hours).
///
/// Removes stale data: completed/failed tasks (>7 days), old error logs (>30 days).
pub async fn spawn_db_cleanup(_db_pool: Arc<DbPool>, shared_storage: Arc<SharedStorage>) {
    let lock_conn = match shared_storage.as_ref() {
        SharedStorage::Sqlite { .. } => None,
        SharedStorage::Postgres { .. } => {
            match try_acquire_pg_singleton_lock(&shared_storage, LOCK_UPDATES_CLEANUP, "db cleanup").await {
                Some(conn) => Some(conn),
                None => return,
            }
        }
    };

    tokio::spawn(async move {
        let _lock_conn = lock_conn;
        let mut interval = interval(Duration::from_secs(6 * 60 * 60)); // 6 hours
        loop {
            interval.tick().await;
            {
                let mut total = 0;
                match shared_storage.cleanup_old_tasks(7).await {
                    Ok(n) if n > 0 => {
                        total += n;
                        log::info!("DB cleanup: removed {} old task_queue entries", n);
                    }
                    Err(e) => log::warn!("DB cleanup: task_queue error: {}", e),
                    _ => {}
                }
                match shared_storage.cleanup_old_errors(30).await {
                    Ok(n) if n > 0 => {
                        total += n;
                        log::info!("DB cleanup: removed {} old error_log entries", n);
                    }
                    Err(e) => log::warn!("DB cleanup: error_log error: {}", e),
                    _ => {}
                }
                match shared_storage.cleanup_old_processed_updates(48).await {
                    Ok(n) if n > 0 => {
                        total += n as usize;
                        log::info!("DB cleanup: removed {} old processed_updates entries", n);
                    }
                    Err(e) => log::warn!("DB cleanup: processed_updates error: {}", e),
                    _ => {}
                }
                match shared_storage.cleanup_expired_url_cache().await {
                    Ok(n) if n > 0 => {
                        total += n;
                        log::info!("DB cleanup: removed {} expired url_cache entries", n);
                    }
                    Err(e) => log::warn!("DB cleanup: url_cache error: {}", e),
                    _ => {}
                }
                if total > 0 {
                    log::info!("DB cleanup: {} rows removed total", total);
                }
            }
        }
    });
}

/// Start the health check scheduler.
pub fn spawn_health_checks(bot: Bot) {
    use crate::smoke_tests::{start_health_check_scheduler, HealthCheckScheduler};

    let bot_arc = Arc::new(bot);
    let _health_scheduler = start_health_check_scheduler(bot_arc);

    if HealthCheckScheduler::is_enabled() {
        log::info!(
            "Health check scheduler started (interval: {}s)",
            HealthCheckScheduler::get_interval_secs()
        );
    }
}
