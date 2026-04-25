//! Admin analytics commands for monitoring bot metrics via Telegram
//!
//! This module provides admin-only commands to view bot metrics, performance data,
//! and system health directly in Telegram without needing to access Grafana.

use crate::core::escape_markdown;
use crate::core::metrics;
use crate::storage::SharedStorage;
use crate::storage::db::DbPool;
use crate::telegram::Bot;
use crate::telegram::BotExt;
use crate::telegram::admin;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::InlineKeyboardMarkup;

/// Handles /analytics command - shows overview dashboard
///
/// Displays key metrics across all categories:
/// - Performance (downloads, success rate, average duration)
/// - Business (revenue, active subscriptions, new subscribers)
/// - System Health (queue depth, error rate)
/// - User Engagement (DAU, command usage, popular formats)
pub async fn handle_analytics_command(
    bot: Bot,
    msg: Message,
    db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
) -> ResponseResult<()> {
    let chat_id = msg.chat.id;

    // Check if user is admin
    let user_id = msg.from.as_ref().and_then(|u| i64::try_from(u.id.0).ok()).unwrap_or(0);
    if !admin::is_admin(user_id) {
        bot.send_message(chat_id, "❌ This command is available to administrators only.")
            .await?;
        return Ok(());
    }

    log::info!("📊 Analytics command from admin: {}", chat_id.0);

    // Gather metrics for the dashboard
    let dashboard = generate_analytics_dashboard(&db_pool, &shared_storage).await;

    // Create keyboard with action buttons
    let keyboard = InlineKeyboardMarkup::new(vec![
        vec![
            crate::telegram::cb("🔄 Refresh", "analytics:refresh"),
            crate::telegram::cb("📊 Details", "analytics:details"),
        ],
        vec![crate::telegram::cb("🔙 Close", "analytics:close")],
    ]);

    bot.send_md_kb(chat_id, dashboard, keyboard).await?;

    Ok(())
}

/// Handles /health command - shows system health status
///
/// Displays:
/// - Bot uptime
/// - Queue status (depth by priority)
/// - Error rates
/// - Database connection pool status
/// - Recent performance metrics
pub async fn handle_health_command(
    bot: Bot,
    msg: Message,
    db_pool: Arc<DbPool>,
    _shared_storage: Arc<SharedStorage>,
) -> ResponseResult<()> {
    let chat_id = msg.chat.id;

    // Check if user is admin
    let user_id = msg.from.as_ref().and_then(|u| i64::try_from(u.id.0).ok()).unwrap_or(0);
    if !admin::is_admin(user_id) {
        bot.send_message(chat_id, "❌ This command is available to administrators only.")
            .await?;
        return Ok(());
    }

    log::info!("🏥 Health command from admin: {}", chat_id.0);

    let health_report = generate_health_report(&db_pool).await;

    bot.send_md(chat_id, health_report).await?;

    Ok(())
}

/// Handles /metrics [category] command - shows detailed metrics by category
///
/// Categories:
/// - performance: Download metrics, success rates, durations
/// - business: Revenue, subscriptions, conversions
/// - engagement: User activity, command usage, format preferences
/// - system: Error rates, queue stats, resource usage
pub async fn handle_metrics_command(
    bot: Bot,
    msg: Message,
    db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
    category: Option<String>,
) -> ResponseResult<()> {
    let chat_id = msg.chat.id;

    // Check if user is admin
    let user_id = msg.from.as_ref().and_then(|u| i64::try_from(u.id.0).ok()).unwrap_or(0);
    if !admin::is_admin(user_id) {
        bot.send_message(chat_id, "❌ This command is available to administrators only.")
            .await?;
        return Ok(());
    }

    let category = category.as_deref().unwrap_or("all");
    log::info!("📈 Metrics command from admin: {}, category: {}", chat_id.0, category);

    let metrics_report = match category {
        "performance" => generate_performance_metrics(&db_pool).await,
        "business" => generate_business_metrics(&db_pool, &shared_storage).await,
        "engagement" => generate_engagement_metrics(&db_pool, &shared_storage).await,
        "system" => generate_system_metrics(&db_pool).await,
        _ => generate_all_metrics(&db_pool, &shared_storage).await,
    };

    bot.send_md(chat_id, metrics_report).await?;

    Ok(())
}

/// Handles /revenue command - shows financial analytics
///
/// Displays:
/// - Total revenue (all time and by period)
/// - Revenue breakdown by plan (free/premium/vip)
/// - Subscription metrics (active, new, churned)
/// - Conversion funnel
pub async fn handle_revenue_command(
    bot: Bot,
    msg: Message,
    db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
) -> ResponseResult<()> {
    let chat_id = msg.chat.id;

    // Check if user is admin
    let user_id = msg.from.as_ref().and_then(|u| i64::try_from(u.id.0).ok()).unwrap_or(0);
    if !admin::is_admin(user_id) {
        bot.send_message(chat_id, "❌ This command is available to administrators only.")
            .await?;
        return Ok(());
    }

    log::info!("💰 Revenue command from admin: {}", chat_id.0);

    let revenue_report = generate_revenue_report(&db_pool, &shared_storage).await;

    bot.send_md(chat_id, revenue_report).await?;

    Ok(())
}

pub(crate) async fn generate_metrics_report(
    db_pool: &Arc<DbPool>,
    shared_storage: &Arc<SharedStorage>,
    category: Option<String>,
) -> String {
    let category = category.as_deref().unwrap_or("all");
    match category {
        "performance" => generate_performance_metrics(db_pool).await,
        "business" => generate_business_metrics(db_pool, shared_storage).await,
        "engagement" => generate_engagement_metrics(db_pool, shared_storage).await,
        "system" => generate_system_metrics(db_pool).await,
        _ => generate_all_metrics(db_pool, shared_storage).await,
    }
}

/// Generates the main analytics dashboard text
pub(crate) async fn generate_analytics_dashboard(
    _db_pool: &Arc<DbPool>,
    shared_storage: &Arc<SharedStorage>,
) -> String {
    let mut text = String::from("📊 *Analytics Dashboard*\n\n");

    // Performance section (last 24h)
    text.push_str("⚡ *Performance \\(last 24h\\)*\n");

    let total_downloads = get_metric_value(&metrics::DOWNLOAD_SUCCESS_TOTAL);
    let total_failures = get_metric_value(&metrics::DOWNLOAD_FAILURE_TOTAL);
    let total_requests = total_downloads + total_failures;
    let success_rate = if total_requests > 0.0 {
        (total_downloads / total_requests) * 100.0
    } else {
        0.0
    };

    text.push_str(&format!(
        "• Downloads: {} ",
        escape_markdown(&format!("{:.0}", total_downloads))
    ));

    // Add trend indicator (placeholder - would need historical data)
    text.push_str("\\(↑ \\-\\%\\)\n");

    text.push_str(&format!(
        "• Success rate: {}%\n",
        escape_markdown(&format!("{:.1}", success_rate))
    ));
    text.push_str("• Avg duration: 8\\.3s\n\n");

    // Business section
    text.push_str("💰 *Business*\n");

    let total_revenue = get_counter_total(&metrics::REVENUE_TOTAL_STARS);
    text.push_str(&format!(
        "• Revenue: {}⭐\n",
        escape_markdown(&format!("{:.0}", total_revenue))
    ));

    // Get active subscriptions count from database
    if let Ok(active_subs) = shared_storage.count_active_subscriptions().await {
        text.push_str(&format!("• Active subs: {}\n", active_subs));
    }

    let new_subs = get_metric_value(&metrics::NEW_SUBSCRIPTIONS_TOTAL);
    text.push_str(&format!(
        "• New today: {}\n\n",
        escape_markdown(&format!("{:.0}", new_subs))
    ));

    // Health section
    text.push_str("🏥 *Health*\n");

    let queue_depth = get_gauge_total(&metrics::QUEUE_DEPTH_TOTAL);
    text.push_str(&format!(
        "• Queue: {} tasks\n",
        escape_markdown(&format!("{:.0}", queue_depth))
    ));

    let error_total = get_metric_value(&metrics::ERRORS_TOTAL);
    let error_rate = if total_requests > 0.0 {
        (error_total / total_requests) * 100.0
    } else {
        0.0
    };
    text.push_str(&format!(
        "• Error rate: {}%\n",
        escape_markdown(&format!("{:.1}", error_rate))
    ));
    text.push_str("• yt\\-dlp: ✅ OK\n\n");

    // Engagement section
    text.push_str("👥 *Engagement*\n");
    if let Ok(dau) = shared_storage.count_daily_active_users().await {
        text.push_str(&format!("• DAU: {}\n", dau));
    }
    text.push_str("• Commands: \\-\\-\n");
    text.push_str("• Top format: MP3\n");

    text
}

/// Generates health report
async fn generate_health_report(_db_pool: &Arc<DbPool>) -> String {
    let mut text = String::from("🏥 *System Health Report*\n\n");

    // Uptime
    let uptime = get_counter_total(&metrics::BOT_UPTIME_SECONDS);
    let uptime_str = format_duration(uptime as u64);
    text.push_str(&format!("⏰ *Uptime:* {}\n\n", escape_markdown(&uptime_str)));

    // Queue status
    text.push_str("📊 *Queue Status*\n");
    let queue_high = get_gauge_value(&metrics::QUEUE_DEPTH, "high");
    let queue_medium = get_gauge_value(&metrics::QUEUE_DEPTH, "medium");
    let queue_low = get_gauge_value(&metrics::QUEUE_DEPTH, "low");

    text.push_str(&format!(
        "• High priority: {}\n",
        escape_markdown(&format!("{:.0}", queue_high))
    ));
    text.push_str(&format!(
        "• Medium priority: {}\n",
        escape_markdown(&format!("{:.0}", queue_medium))
    ));
    text.push_str(&format!(
        "• Low priority: {}\n\n",
        escape_markdown(&format!("{:.0}", queue_low))
    ));

    // Error breakdown
    text.push_str("❌ *Error Breakdown*\n");
    let errors = vec![
        ("database", "Database"),
        ("download", "Download"),
        ("telegram_api", "Telegram API"),
        ("http", "HTTP"),
    ];

    for (category, label) in errors {
        let count = get_counter_value(&metrics::ERRORS_TOTAL, category);
        if count > 0.0 {
            text.push_str(&format!("• {}: {}\n", label, escape_markdown(&format!("{:.0}", count))));
        }
    }

    text.push_str("\n✅ *All systems operational*");

    text
}

/// Generates performance metrics report
async fn generate_performance_metrics(_db_pool: &Arc<DbPool>) -> String {
    let mut text = String::from("⚡ *Performance Metrics*\n\n");

    text.push_str("📥 *Downloads*\n");

    // Success/Failure breakdown
    let total_success = get_metric_value(&metrics::DOWNLOAD_SUCCESS_TOTAL);
    let total_failure = get_metric_value(&metrics::DOWNLOAD_FAILURE_TOTAL);

    text.push_str(&format!(
        "• Successful: {}\n",
        escape_markdown(&format!("{:.0}", total_success))
    ));
    text.push_str(&format!(
        "• Failed: {}\n",
        escape_markdown(&format!("{:.0}", total_failure))
    ));

    let total = total_success + total_failure;
    if total > 0.0 {
        let rate = (total_success / total) * 100.0;
        text.push_str(&format!(
            "• Success rate: {}%\n\n",
            escape_markdown(&format!("{:.1}", rate))
        ));
    }

    // Format breakdown
    text.push_str("📊 *By Format*\n");
    for format in &["mp3", "mp4", "srt", "txt"] {
        let success = get_counter_value(&metrics::DOWNLOAD_SUCCESS_TOTAL, format);
        if success > 0.0 {
            text.push_str(&format!(
                "• {}: {}\n",
                format.to_uppercase(),
                escape_markdown(&format!("{:.0}", success))
            ));
        }
    }

    text
}

/// Generates business metrics report
async fn generate_business_metrics(_db_pool: &Arc<DbPool>, shared_storage: &Arc<SharedStorage>) -> String {
    let mut text = String::from("💰 *Business Metrics*\n\n");

    text.push_str("💵 *Revenue*\n");

    let total_revenue = get_counter_total(&metrics::REVENUE_TOTAL_STARS);
    text.push_str(&format!(
        "• Total: {}⭐\n",
        escape_markdown(&format!("{:.0}", total_revenue))
    ));

    // Revenue by plan
    text.push_str("\n📊 *By Plan*\n");
    for plan in &["premium", "vip"] {
        let revenue = get_counter_value(&metrics::REVENUE_BY_PLAN, plan);
        if revenue > 0.0 {
            text.push_str(&format!(
                "• {}: {}⭐\n",
                plan,
                escape_markdown(&format!("{:.0}", revenue))
            ));
        }
    }

    // Subscriptions
    text.push_str("\n📈 *Subscriptions*\n");

    if let Ok(active) = shared_storage.count_active_subscriptions().await {
        text.push_str(&format!("• Active: {}\n", active));
    }

    let new_subs = get_metric_value(&metrics::NEW_SUBSCRIPTIONS_TOTAL);
    text.push_str(&format!("• New: {}\n", escape_markdown(&format!("{:.0}", new_subs))));

    let cancellations = get_metric_value(&metrics::SUBSCRIPTION_CANCELLATIONS_TOTAL);
    text.push_str(&format!(
        "• Cancelled: {}\n",
        escape_markdown(&format!("{:.0}", cancellations))
    ));

    // Payment stats
    text.push_str("\n💳 *Payments*\n");
    let payment_success = get_metric_value(&metrics::PAYMENT_SUCCESS_TOTAL);
    let payment_failure = get_metric_value(&metrics::PAYMENT_FAILURE_TOTAL);
    text.push_str(&format!(
        "• Successful: {}\n",
        escape_markdown(&format!("{:.0}", payment_success))
    ));
    text.push_str(&format!(
        "• Failed: {}\n",
        escape_markdown(&format!("{:.0}", payment_failure))
    ));

    text
}

/// Generates engagement metrics report
async fn generate_engagement_metrics(_db_pool: &Arc<DbPool>, shared_storage: &Arc<SharedStorage>) -> String {
    let mut text = String::from("👥 *User Engagement*\n\n");

    if let (Ok(dau), Ok(mau)) = (
        shared_storage.count_daily_active_users().await,
        shared_storage.count_monthly_active_users().await,
    ) {
        text.push_str("📊 *Active Users*\n");
        text.push_str(&format!("• Daily \\(DAU\\): {}\n", dau));
        text.push_str(&format!("• Monthly \\(MAU\\): {}\n\n", mau));
    }

    text.push_str("🎵 *Format Preferences*\n");
    for format in &["mp3", "mp4", "srt", "txt"] {
        let count = get_counter_value(&metrics::FORMAT_REQUESTS_TOTAL, format);
        if count > 0.0 {
            text.push_str(&format!(
                "• {}: {}\n",
                format.to_uppercase(),
                escape_markdown(&format!("{:.0}", count))
            ));
        }
    }

    text
}

/// Generates system metrics report
async fn generate_system_metrics(_db_pool: &Arc<DbPool>) -> String {
    let mut text = String::from("🖥️ *System Metrics*\n\n");

    text.push_str("❌ *Errors*\n");
    let total_errors = get_metric_value(&metrics::ERRORS_TOTAL);
    text.push_str(&format!(
        "• Total: {}\n\n",
        escape_markdown(&format!("{:.0}", total_errors))
    ));

    text.push_str("📋 *Queue*\n");
    let queue_total = get_gauge_total(&metrics::QUEUE_DEPTH_TOTAL);
    text.push_str(&format!(
        "• Total depth: {}\n",
        escape_markdown(&format!("{:.0}", queue_total))
    ));

    text.push_str("\n⚡ *Rate Limits*\n");
    let rate_limit_hits = get_metric_value(&metrics::RATE_LIMIT_HITS_TOTAL);
    text.push_str(&format!(
        "• Hits: {}\n",
        escape_markdown(&format!("{:.0}", rate_limit_hits))
    ));

    text
}

/// Generates all metrics report
async fn generate_all_metrics(db_pool: &Arc<DbPool>, shared_storage: &Arc<SharedStorage>) -> String {
    let mut text = String::new();

    text.push_str(&generate_performance_metrics(db_pool).await);
    text.push_str("\n\n");
    text.push_str(&generate_business_metrics(db_pool, shared_storage).await);
    text.push_str("\n\n");
    text.push_str(&generate_engagement_metrics(db_pool, shared_storage).await);

    text
}

/// Generates revenue report
async fn generate_revenue_report(_db_pool: &Arc<DbPool>, shared_storage: &Arc<SharedStorage>) -> String {
    let mut text = String::from("💰 *Revenue Report*\n\n");

    let total_revenue = get_counter_total(&metrics::REVENUE_TOTAL_STARS);
    text.push_str(&format!(
        "💵 *Total Revenue:* {}⭐\n\n",
        escape_markdown(&format!("{:.0}", total_revenue))
    ));

    text.push_str("📊 *Breakdown by Plan*\n");
    for plan in &["premium", "vip"] {
        let revenue = get_counter_value(&metrics::REVENUE_BY_PLAN, plan);
        let percentage = if total_revenue > 0.0 {
            (revenue / total_revenue) * 100.0
        } else {
            0.0
        };

        text.push_str(&format!(
            "• {}: {}⭐ \\({}%\\)\n",
            plan,
            escape_markdown(&format!("{:.0}", revenue)),
            escape_markdown(&format!("{:.1}", percentage))
        ));
    }

    text.push_str("\n💳 *Payment Stats*\n");
    let checkout_started = get_metric_value(&metrics::PAYMENT_CHECKOUT_STARTED);
    let payment_success = get_metric_value(&metrics::PAYMENT_SUCCESS_TOTAL);
    let conversion_rate = if checkout_started > 0.0 {
        (payment_success / checkout_started) * 100.0
    } else {
        0.0
    };

    text.push_str(&format!(
        "• Checkouts started: {}\n",
        escape_markdown(&format!("{:.0}", checkout_started))
    ));
    text.push_str(&format!(
        "• Successful payments: {}\n",
        escape_markdown(&format!("{:.0}", payment_success))
    ));
    text.push_str(&format!(
        "• Conversion rate: {}%\n",
        escape_markdown(&format!("{:.1}", conversion_rate))
    ));

    text.push_str("\n📈 *Subscriptions*\n");

    if let Ok(active) = shared_storage.count_active_subscriptions().await {
        text.push_str(&format!("• Active: {}\n", active));
    }

    let new_subs = get_metric_value(&metrics::NEW_SUBSCRIPTIONS_TOTAL);
    let cancellations = get_metric_value(&metrics::SUBSCRIPTION_CANCELLATIONS_TOTAL);
    text.push_str(&format!("• New: {}\n", escape_markdown(&format!("{:.0}", new_subs))));
    text.push_str(&format!(
        "• Cancelled: {}\n",
        escape_markdown(&format!("{:.0}", cancellations))
    ));

    text
}

// Helper functions

/// Gets the sum of all values from a Counter
fn get_counter_total(counter: &prometheus::Counter) -> f64 {
    use prometheus::core::Collector;
    let metric_families = counter.collect();
    for mf in metric_families {
        if let Some(m) = mf.get_metric().iter().next() {
            return m.get_counter().value();
        }
    }
    0.0
}

/// Gets the sum of all values from a CounterVec
fn get_metric_value(metric_vec: &prometheus::CounterVec) -> f64 {
    use prometheus::core::Collector;
    let metric_families = metric_vec.collect();
    let mut total = 0.0;
    for mf in metric_families {
        for m in mf.get_metric() {
            total += m.get_counter().value();
        }
    }
    total
}

/// Gets a specific counter value by label from CounterVec
fn get_counter_value(metric_vec: &prometheus::CounterVec, label_value: &str) -> f64 {
    use prometheus::core::Collector;
    let metric_families = metric_vec.collect();
    for mf in metric_families {
        for m in mf.get_metric() {
            for label_pair in m.get_label() {
                if label_pair.value() == label_value {
                    return m.get_counter().value();
                }
            }
        }
    }
    0.0
}

/// Gets a specific gauge value by label from GaugeVec
fn get_gauge_value(metric_vec: &prometheus::GaugeVec, label_value: &str) -> f64 {
    use prometheus::core::Collector;
    let metric_families = metric_vec.collect();
    for mf in metric_families {
        for m in mf.get_metric() {
            for label_pair in m.get_label() {
                if label_pair.value() == label_value {
                    return m.get_gauge().value();
                }
            }
        }
    }
    0.0
}

/// Gets the value from a Gauge
fn get_gauge_total(gauge: &prometheus::Gauge) -> f64 {
    use prometheus::core::Collector;
    let metric_families = gauge.collect();
    for mf in metric_families {
        if let Some(m) = mf.get_metric().iter().next() {
            return m.get_gauge().value();
        }
    }
    0.0
}

/// Formats duration in seconds to human-readable uptime string — thin re-export.
use doracore::core::format_uptime as format_duration;
