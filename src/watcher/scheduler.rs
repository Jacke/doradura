//! Background scheduler that periodically checks subscriptions for new content.
//!
//! Runs as a `tokio::spawn`ed task, emitting `WatchNotification`s through an mpsc channel.
//! The Telegram layer receives these and sends formatted messages to users.

use crate::core::config;
use crate::storage::db::DbPool;
use crate::storage::get_connection;
use crate::watcher::db;
use crate::watcher::traits::WatchNotification;
use crate::watcher::WatcherRegistry;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::time::{interval, Duration};

/// Start the watcher scheduler background task.
///
/// Returns a receiver for `WatchNotification`s that should be consumed
/// by the Telegram notification dispatcher.
pub fn start_scheduler(
    db_pool: Arc<DbPool>,
    registry: Arc<WatcherRegistry>,
) -> mpsc::UnboundedReceiver<WatchNotification> {
    let (tx, rx) = mpsc::unbounded_channel();

    tokio::spawn(async move {
        let check_interval = Duration::from_secs(*config::watcher::CHECK_INTERVAL_SECS);
        let mut ticker = interval(check_interval);

        log::info!(
            "Watcher scheduler started (interval: {}s, budget: {} req/cycle)",
            *config::watcher::CHECK_INTERVAL_SECS,
            *config::watcher::MAX_REQUESTS_PER_CYCLE,
        );

        loop {
            ticker.tick().await;

            if let Err(e) = run_check_cycle(&db_pool, &registry, &tx).await {
                log::error!("Watcher check cycle failed: {}", e);
            }
        }
    });

    rx
}

/// Run one check cycle: iterate source groups, check for updates, emit notifications.
async fn run_check_cycle(
    db_pool: &Arc<DbPool>,
    registry: &WatcherRegistry,
    tx: &mpsc::UnboundedSender<WatchNotification>,
) -> Result<(), String> {
    let conn = get_connection(db_pool).map_err(|e| format!("DB connection error: {}", e))?;
    let groups = db::get_active_source_groups(&conn)?;
    drop(conn);

    if groups.is_empty() {
        return Ok(());
    }

    let max_requests = *config::watcher::MAX_REQUESTS_PER_CYCLE;
    let max_errors = *config::watcher::MAX_CONSECUTIVE_ERRORS;
    let mut budget = max_requests;

    log::info!(
        "Watcher cycle: {} source group(s), budget: {} requests",
        groups.len(),
        budget
    );

    for group in &groups {
        let watcher = match registry.get(&group.source_type) {
            Some(w) => w,
            None => {
                log::warn!("No watcher registered for source_type '{}'", group.source_type);
                continue;
            }
        };

        let cost = watcher.requests_per_check(group.combined_mask);
        if cost > budget {
            log::info!(
                "Watcher budget exhausted ({} remaining, need {}), deferring rest",
                budget,
                cost
            );
            break;
        }
        budget -= cost;

        // Use first subscription's state/meta (all share the same source)
        let first_sub = &group.subscriptions[0];
        let last_state = first_sub.last_seen_state.as_ref();
        let source_meta = first_sub.source_meta.as_ref();

        match watcher
            .check(&group.source_id, group.combined_mask, last_state, source_meta)
            .await
        {
            Ok(result) => {
                // Update DB state for all subscriptions of this source
                let conn = get_connection(db_pool).map_err(|e| format!("DB error: {}", e))?;
                db::update_check_success(
                    &conn,
                    &group.source_type,
                    &group.source_id,
                    &result.new_state,
                    result.new_meta.as_ref(),
                )?;
                drop(conn);

                // Fan out notifications to all subscribers
                for update in &result.updates {
                    for sub in &group.subscriptions {
                        // Only notify if the subscriber watches this content type
                        let bit = match update.content_type.as_str() {
                            "post" => 1,
                            "story" => 2,
                            _ => 0,
                        };
                        if bit == 0 || sub.watch_mask & bit != 0 {
                            let notification = WatchNotification {
                                user_id: sub.user_id,
                                source_type: group.source_type.clone(),
                                source_id: group.source_id.clone(),
                                display_name: sub.display_name.clone(),
                                subscription_id: sub.id,
                                update: update.clone(),
                            };
                            if tx.send(notification).is_err() {
                                log::warn!("Notification channel closed, stopping scheduler");
                                return Ok(());
                            }
                        }
                    }
                }

                if !result.updates.is_empty() {
                    log::info!(
                        "Watcher: {} update(s) for {}:{}",
                        result.updates.len(),
                        group.source_type,
                        group.source_id
                    );
                }
            }
            Err(e) => {
                log::warn!(
                    "Watcher check failed for {}:{}: {}",
                    group.source_type,
                    group.source_id,
                    e
                );

                let conn = get_connection(db_pool).map_err(|e| format!("DB error: {}", e))?;
                let error_count = db::update_check_error(&conn, &group.source_type, &group.source_id, &e)?;

                if error_count >= max_errors {
                    let disabled = db::auto_disable_errored(&conn, &group.source_type, &group.source_id, max_errors)?;
                    if disabled > 0 {
                        log::warn!(
                            "Auto-disabled {} subscription(s) for {}:{} ({} consecutive errors)",
                            disabled,
                            group.source_type,
                            group.source_id,
                            error_count
                        );
                    }
                }
                drop(conn);
            }
        }
    }

    Ok(())
}
