//! Background task spawners for the bot.
//!
//! Each function spawns a `tokio::spawn` task that runs periodically.
//! Extracted from `run_bot()` in main.rs for clarity and testability.

use sqlx::{Postgres, pool::PoolConnection};
use std::sync::Arc;
use std::time::Duration;
use teloxide::prelude::*;
use tokio::time::interval;

use crate::core::{alerts, config, metrics, stats_reporter};
use crate::storage::SharedStorage;
use crate::storage::db::DbPool;
use crate::telegram::Bot;

const LOCK_ALERT_MONITOR: i64 = 1101;
const LOCK_STATS_REPORTER: i64 = 1102;
const LOCK_UPDATES_CLEANUP: i64 = 1002;
const LOCK_SUBSCRIPTION_EXPIRY: i64 = 1103;
const LOCK_COOKIES_CHECKER: i64 = 1104;
const LOCK_CONTENT_WATCHER: i64 = 1105;
const LOCK_DOWNLOADS_CLEANUP: i64 = 1106;

/// Default retention period for files in the downloads folder (in days).
/// Override with the `DOWNLOADS_RETENTION_DAYS` env var. Files older than
/// this are deleted by `spawn_downloads_cleanup` every 6 hours.
/// Default retention window for `DOWNLOAD_FOLDER` files. Lowered v0.49.1
/// from 7 → 1 day — at Master quality, 1440p mp4 outputs are 500 MB-1.8 GB,
/// and a 18 GB Railway volume fills inside a day at normal usage. The
/// post-send 2-min cleanup catches the happy path; this nightly sweep
/// catches everything that fell through (bot restart mid-pipeline,
/// orphaned recodes from previous container generations).
/// Override with `DOWNLOADS_RETENTION_DAYS` env var.
const DOWNLOADS_RETENTION_DAYS_DEFAULT: u64 = 1;

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
pub async fn spawn_subscription_expiry_checker(shared_storage: Arc<SharedStorage>) {
    let lock_conn = match shared_storage.as_ref() {
        SharedStorage::Sqlite { .. } => None,
        SharedStorage::Postgres { .. } => {
            match try_acquire_pg_singleton_lock(
                &shared_storage,
                LOCK_SUBSCRIPTION_EXPIRY,
                "subscription expiry checker",
            )
            .await
            {
                Some(conn) => Some(conn),
                None => return,
            }
        }
    };

    tokio::spawn(async move {
        let _lock_conn = lock_conn;
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
pub async fn spawn_cookies_checker(bot: Bot, shared_storage: Arc<SharedStorage>) {
    let lock_conn = match shared_storage.as_ref() {
        SharedStorage::Sqlite { .. } => None,
        SharedStorage::Postgres { .. } => {
            match try_acquire_pg_singleton_lock(&shared_storage, LOCK_COOKIES_CHECKER, "cookies checker").await {
                Some(conn) => Some(conn),
                None => return,
            }
        }
    };

    tokio::spawn(async move {
        use crate::download::cookies;
        use crate::telegram::{AgeGateTransition, notify_admin_age_gate_state, notify_admin_cookies_refresh};

        /// Per-probe state tracked across ticks for edge-triggered notifications.
        #[derive(Clone, Copy, PartialEq, Eq)]
        enum ProbeState {
            Unknown,
            Ok,
            Fail,
        }

        let _lock_conn = lock_conn;
        let mut interval = interval(Duration::from_secs(5 * 60));
        let mut base_state = ProbeState::Unknown;
        let mut age_state = ProbeState::Unknown;

        loop {
            interval.tick().await;
            log::debug!("Running periodic cookies validation check");

            // --- Probe 1: regular cookies (Me at the zoo) ---
            let base_result = cookies::needs_refresh().await;
            let base_ok = base_result.is_none();
            let new_base_state = if base_ok { ProbeState::Ok } else { ProbeState::Fail };
            metrics::update_cookies_status(base_ok);

            // Edge-trigger for base cookies: notify on OK→Fail and Unknown→Fail transitions.
            // (Keeps existing behavior: admin only hears about the fail edge — the
            // notify_admin_cookies_refresh helper has its own 6h cooldown as a secondary
            // guard, but the state machine here is the primary dedup.)
            match (base_state, new_base_state) {
                (ProbeState::Unknown | ProbeState::Ok, ProbeState::Fail) => {
                    let reason = base_result
                        .as_deref()
                        .unwrap_or("Cookies validation failed")
                        .to_string();
                    log::warn!("🔴 Cookies need refresh: {}", reason);

                    let admin_ids = config::admin::ADMIN_IDS.clone();
                    let primary_admin = *config::admin::ADMIN_USER_ID;
                    let mut notified_admins = std::collections::HashSet::new();

                    for admin_id in admin_ids.iter() {
                        if notified_admins.insert(*admin_id)
                            && let Err(e) = notify_admin_cookies_refresh(&bot, *admin_id, &reason).await
                        {
                            log::error!("Failed to notify admin {} about cookies: {}", admin_id, e);
                        }
                    }

                    if primary_admin != 0
                        && notified_admins.insert(primary_admin)
                        && let Err(e) = notify_admin_cookies_refresh(&bot, primary_admin, &reason).await
                    {
                        log::error!("Failed to notify primary admin {} about cookies: {}", primary_admin, e);
                    }
                }
                (ProbeState::Fail, ProbeState::Ok) => {
                    log::info!("✅ Cookies recovered");
                }
                _ => {}
            }

            // --- Probe 2: age-verified cookies (only probed when base is OK) ---
            // No point testing age-gate when base auth itself is broken — that path will
            // fail with the same SessionExpired reason and spam noise.
            if base_ok {
                let age_ok = cookies::validate_age_gated_cookies_ok().await;
                let new_age_state = if age_ok { ProbeState::Ok } else { ProbeState::Fail };
                metrics::update_cookies_age_verified_status(age_ok);

                let transition = match (age_state, new_age_state) {
                    (ProbeState::Unknown | ProbeState::Ok, ProbeState::Fail) => Some(AgeGateTransition::Lost),
                    (ProbeState::Fail, ProbeState::Ok) => Some(AgeGateTransition::Recovered),
                    _ => None,
                };

                if let Some(t) = transition {
                    log::warn!("🔞 Age-gate state transition: {:?}", t);
                    let admin_ids = config::admin::ADMIN_IDS.clone();
                    let primary_admin = *config::admin::ADMIN_USER_ID;
                    let mut notified_admins = std::collections::HashSet::new();

                    for admin_id in admin_ids.iter() {
                        if notified_admins.insert(*admin_id)
                            && let Err(e) = notify_admin_age_gate_state(&bot, *admin_id, t).await
                        {
                            log::error!("Failed to notify admin {} about age-gate: {}", admin_id, e);
                        }
                    }

                    if primary_admin != 0
                        && notified_admins.insert(primary_admin)
                        && let Err(e) = notify_admin_age_gate_state(&bot, primary_admin, t).await
                    {
                        log::error!("Failed to notify primary admin {} about age-gate: {}", primary_admin, e);
                    }
                }

                age_state = new_age_state;
            }
            // If base is broken we deliberately DO NOT touch age_state — the next tick
            // where base recovers will re-probe age-gate from its last known state,
            // avoiding spurious Lost→Recovered flaps driven by base-auth outages.

            base_state = new_base_state;
        }
    });
}

/// Start the downloads folder cleanup task (every 6 hours).
///
/// Deletes files in the configured `DOWNLOAD_FOLDER` whose mtime is older than
/// `DOWNLOADS_RETENTION_DAYS` (default: 7 days). Prevents disk-full incidents
/// from accumulated download artifacts (.mp4 / .webp / .temp.mp4).
pub async fn spawn_downloads_cleanup(shared_storage: Arc<SharedStorage>) {
    let lock_conn = match shared_storage.as_ref() {
        SharedStorage::Sqlite { .. } => None,
        SharedStorage::Postgres { .. } => {
            match try_acquire_pg_singleton_lock(&shared_storage, LOCK_DOWNLOADS_CLEANUP, "downloads cleanup").await {
                Some(conn) => Some(conn),
                None => return,
            }
        }
    };

    tokio::spawn(async move {
        let _lock_conn = lock_conn;
        let retention_days: u64 = std::env::var("DOWNLOADS_RETENTION_DAYS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DOWNLOADS_RETENTION_DAYS_DEFAULT);

        let folder = shellexpand::tilde(&*doracore::core::config::DOWNLOAD_FOLDER).into_owned();
        log::info!(
            "🧹 Downloads cleanup task started: folder={}, retention={} days",
            folder,
            retention_days
        );

        // Run once shortly after startup, then every 6 hours.
        let mut interval = interval(Duration::from_secs(6 * 60 * 60));
        loop {
            interval.tick().await;
            let cutoff = std::time::SystemTime::now() - Duration::from_secs(retention_days * 24 * 60 * 60);

            let (removed, freed_bytes) = match cleanup_downloads_folder(&folder, cutoff).await {
                Ok(stats) => stats,
                Err(e) => {
                    log::warn!("🧹 Downloads cleanup failed: {}", e);
                    continue;
                }
            };

            if removed > 0 {
                log::info!(
                    "🧹 Downloads cleanup: removed {} files, freed {:.1} MB",
                    removed,
                    freed_bytes as f64 / (1024.0 * 1024.0)
                );
            } else {
                log::debug!("🧹 Downloads cleanup: nothing to delete (folder={})", folder);
            }
        }
    });
}

/// Walk `folder` (one level deep, files only), delete entries with mtime older
/// than `cutoff`. Returns `(deleted_count, total_freed_bytes)`.
///
/// Skips directories, symlinks, and files we can't `stat`/`remove` (logs at debug).
async fn cleanup_downloads_folder(folder: &str, cutoff: std::time::SystemTime) -> std::io::Result<(usize, u64)> {
    let mut removed = 0usize;
    let mut freed_bytes = 0u64;

    let mut entries = tokio::fs::read_dir(folder).await?;
    while let Some(entry) = entries.next_entry().await? {
        let metadata = match entry.metadata().await {
            Ok(md) => md,
            Err(e) => {
                log::debug!("🧹 stat failed on {:?}: {}", entry.path(), e);
                continue;
            }
        };
        if !metadata.is_file() {
            continue;
        }
        let mtime = match metadata.modified() {
            Ok(t) => t,
            Err(_) => continue,
        };
        if mtime >= cutoff {
            continue;
        }

        let size = metadata.len();
        let path = entry.path();
        match tokio::fs::remove_file(&path).await {
            Ok(_) => {
                removed += 1;
                freed_bytes += size;
            }
            Err(e) => {
                log::debug!("🧹 remove failed on {:?}: {}", path, e);
            }
        }
    }

    Ok((removed, freed_bytes))
}

pub async fn spawn_content_watcher(bot: Bot, db_pool: Arc<DbPool>, shared_storage: Arc<SharedStorage>) {
    use crate::watcher::{WatcherRegistry, scheduler};

    let lock_conn = match shared_storage.as_ref() {
        SharedStorage::Sqlite { .. } => None,
        SharedStorage::Postgres { .. } => {
            match try_acquire_pg_singleton_lock(&shared_storage, LOCK_CONTENT_WATCHER, "content watcher").await {
                Some(conn) => Some(conn),
                None => return,
            }
        }
    };

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
        lock_conn,
    );
    log::info!("Content watcher scheduler and notification dispatcher started");
}

/// Start the web server for share pages (if WEB_BASE_URL is configured).
pub fn spawn_web_server(
    shared_storage: Arc<SharedStorage>,
    plan_notifier: Option<crate::core::types::PlanChangeNotifier>,
) {
    if config::share::base_url().is_some() {
        let web_port = config::share::web_port();
        log::info!("Starting web server on port {} (WEB_BASE_URL configured)", web_port);
        tokio::spawn(async move {
            if let Err(e) = crate::core::web::start_web_server(web_port, shared_storage, plan_notifier).await {
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
                crate::core::metrics::update_process_memory();
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
            match try_acquire_pg_singleton_lock(&shared_storage, LOCK_ALERT_MONITOR, "alert monitor").await {
                Some(conn) => Some(conn),
                None => return None,
            }
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
    use crate::smoke_tests::{HealthCheckScheduler, start_health_check_scheduler};

    let bot_arc = Arc::new(bot);
    let _health_scheduler = start_health_check_scheduler(bot_arc);

    if HealthCheckScheduler::is_enabled() {
        log::info!(
            "Health check scheduler started (interval: {}s)",
            HealthCheckScheduler::get_interval_secs()
        );
    }
}
