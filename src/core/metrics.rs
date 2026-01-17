//! Metrics collection for the Telegram bot using Prometheus
//!
//! This module provides a centralized metrics registry for tracking:
//! - Performance metrics (download duration, queue processing time)
//! - Business metrics (revenue, subscriptions, conversions)
//! - System health metrics (errors, queue depth, yt-dlp status)
//! - User engagement metrics (DAU/MAU, command usage)

use lazy_static::lazy_static;
use prometheus::{
    register_counter, register_counter_vec, register_gauge, register_gauge_vec, register_histogram,
    register_histogram_vec, Counter, CounterVec, Gauge, GaugeVec, Histogram, HistogramVec,
};

// ======================
// PERFORMANCE METRICS
// ======================

lazy_static! {
    /// Download duration in seconds by format and quality
    /// Labels: format (mp3/mp4/srt/txt), quality (320k/1080p/etc)
    pub static ref DOWNLOAD_DURATION_SECONDS: HistogramVec = register_histogram_vec!(
        "doradura_download_duration_seconds",
        "Time spent downloading files by format and quality",
        &["format", "quality"],
        vec![1.0, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0, 600.0]
    )
    .unwrap();

    /// Queue processing time per iteration
    pub static ref QUEUE_PROCESSING_DURATION_SECONDS: Histogram = register_histogram!(
        "doradura_queue_processing_duration_seconds",
        "Time spent processing queue per iteration",
        vec![0.1, 0.5, 1.0, 5.0, 10.0, 30.0]
    )
    .unwrap();

    /// Queue wait time from task creation to processing
    /// Labels: priority (low/medium/high)
    pub static ref QUEUE_WAIT_TIME_SECONDS: HistogramVec = register_histogram_vec!(
        "doradura_queue_wait_time_seconds",
        "Time tasks spend waiting in queue before processing",
        &["priority"],
        vec![5.0, 30.0, 60.0, 300.0, 600.0, 1800.0, 3600.0]
    )
    .unwrap();

    /// Successful downloads count
    /// Labels: format, quality
    pub static ref DOWNLOAD_SUCCESS_TOTAL: CounterVec = register_counter_vec!(
        "doradura_download_success_total",
        "Total number of successful downloads",
        &["format", "quality"]
    )
    .unwrap();

    /// Failed downloads count
    /// Labels: format, error_type
    pub static ref DOWNLOAD_FAILURE_TOTAL: CounterVec = register_counter_vec!(
        "doradura_download_failure_total",
        "Total number of failed downloads",
        &["format", "error_type"]
    )
    .unwrap();

    /// yt-dlp command execution duration
    /// Labels: operation (metadata/download/etc)
    pub static ref YTDLP_EXECUTION_DURATION_SECONDS: HistogramVec = register_histogram_vec!(
        "doradura_ytdlp_execution_duration_seconds",
        "Time spent executing yt-dlp commands",
        &["operation"],
        vec![1.0, 5.0, 10.0, 30.0, 60.0, 120.0, 240.0]
    )
    .unwrap();
}

// ======================
// BUSINESS METRICS
// ======================

lazy_static! {
    /// Active subscriptions count by plan
    /// Labels: plan (free/premium/vip)
    pub static ref ACTIVE_SUBSCRIPTIONS: GaugeVec = register_gauge_vec!(
        "doradura_active_subscriptions",
        "Number of active subscriptions by plan",
        &["plan"]
    )
    .unwrap();

    /// Total revenue in Telegram Stars
    pub static ref REVENUE_TOTAL_STARS: Counter = register_counter!(
        "doradura_revenue_total_stars",
        "Total revenue in Telegram Stars"
    )
    .unwrap();

    /// Revenue by subscription plan
    /// Labels: plan
    pub static ref REVENUE_BY_PLAN: CounterVec = register_counter_vec!(
        "doradura_revenue_by_plan",
        "Revenue by subscription plan in Stars",
        &["plan"]
    )
    .unwrap();

    /// New subscriptions count
    /// Labels: plan, is_recurring (true/false)
    pub static ref NEW_SUBSCRIPTIONS_TOTAL: CounterVec = register_counter_vec!(
        "doradura_new_subscriptions_total",
        "Total number of new subscriptions",
        &["plan", "is_recurring"]
    )
    .unwrap();

    /// Subscription cancellations
    /// Labels: plan
    pub static ref SUBSCRIPTION_CANCELLATIONS_TOTAL: CounterVec = register_counter_vec!(
        "doradura_subscription_cancellations_total",
        "Total number of subscription cancellations",
        &["plan"]
    )
    .unwrap();

    /// Payment checkout started
    /// Labels: plan
    pub static ref PAYMENT_CHECKOUT_STARTED: CounterVec = register_counter_vec!(
        "doradura_payment_checkout_started",
        "Number of times payment checkout was initiated",
        &["plan"]
    )
    .unwrap();

    /// Successful payments
    /// Labels: plan, is_recurring
    pub static ref PAYMENT_SUCCESS_TOTAL: CounterVec = register_counter_vec!(
        "doradura_payment_success_total",
        "Total number of successful payments",
        &["plan", "is_recurring"]
    )
    .unwrap();

    /// Failed payments
    /// Labels: plan, reason
    pub static ref PAYMENT_FAILURE_TOTAL: CounterVec = register_counter_vec!(
        "doradura_payment_failure_total",
        "Total number of failed payments",
        &["plan", "reason"]
    )
    .unwrap();
}

// ======================
// SYSTEM HEALTH METRICS
// ======================

lazy_static! {
    /// Errors count by type and operation
    /// Labels: error_type (download/telegram/database/http), operation
    pub static ref ERRORS_TOTAL: CounterVec = register_counter_vec!(
        "doradura_errors_total",
        "Total number of errors by type and operation",
        &["error_type", "operation"]
    )
    .unwrap();

    /// Current queue depth by priority
    /// Labels: priority (low/medium/high)
    pub static ref QUEUE_DEPTH: GaugeVec = register_gauge_vec!(
        "doradura_queue_depth",
        "Current number of tasks in queue by priority",
        &["priority"]
    )
    .unwrap();

    /// Total queue depth across all priorities
    pub static ref QUEUE_DEPTH_TOTAL: Gauge = register_gauge!(
        "doradura_queue_depth_total",
        "Total number of tasks in queue"
    )
    .unwrap();

    /// Task retry count
    /// Labels: retry_count (1/2/3/4/5)
    pub static ref TASK_RETRIES_TOTAL: CounterVec = register_counter_vec!(
        "doradura_task_retries_total",
        "Total number of task retries",
        &["retry_count"]
    )
    .unwrap();

    /// yt-dlp health status (1 = healthy, 0 = unhealthy)
    pub static ref YTDLP_HEALTH_STATUS: Gauge = register_gauge!(
        "doradura_ytdlp_health_status",
        "yt-dlp health status (1 = healthy, 0 = unhealthy)"
    )
    .unwrap();

    /// Rate limit hits count
    /// Labels: plan
    pub static ref RATE_LIMIT_HITS_TOTAL: CounterVec = register_counter_vec!(
        "doradura_rate_limit_hits_total",
        "Total number of rate limit hits",
        &["plan"]
    )
    .unwrap();

    /// Active database connections
    pub static ref DB_CONNECTIONS_ACTIVE: Gauge = register_gauge!(
        "doradura_db_connections_active",
        "Number of active database connections"
    )
    .unwrap();

    /// Idle database connections
    pub static ref DB_CONNECTIONS_IDLE: Gauge = register_gauge!(
        "doradura_db_connections_idle",
        "Number of idle database connections"
    )
    .unwrap();

    /// Bot uptime in seconds
    pub static ref BOT_UPTIME_SECONDS: Counter = register_counter!(
        "doradura_bot_uptime_seconds",
        "Bot uptime in seconds"
    )
    .unwrap();

    /// Dispatcher reconnection count
    pub static ref DISPATCHER_RECONNECTIONS_TOTAL: Counter = register_counter!(
        "doradura_dispatcher_reconnections_total",
        "Total number of dispatcher reconnections"
    )
    .unwrap();

    /// Operation duration by type
    /// Labels: operation_type (download/upload/processing), format
    pub static ref OPERATION_DURATION_SECONDS: HistogramVec = register_histogram_vec!(
        "doradura_operation_duration_seconds",
        "Duration of operations by type and format",
        &["operation_type", "format"],
        vec![1.0, 5.0, 15.0, 30.0, 60.0, 120.0, 300.0]
    )
    .unwrap();

    /// Operation success count
    /// Labels: operation_type, format
    pub static ref OPERATION_SUCCESS_TOTAL: CounterVec = register_counter_vec!(
        "doradura_operation_success_total",
        "Total number of successful operations",
        &["operation_type", "format"]
    )
    .unwrap();

    /// Operation failure count
    /// Labels: operation_type, format, error_category
    pub static ref OPERATION_FAILURE_TOTAL: CounterVec = register_counter_vec!(
        "doradura_operation_failure_total",
        "Total number of failed operations",
        &["operation_type", "format", "error_category"]
    )
    .unwrap();

    /// File size distribution
    /// Labels: format
    pub static ref FILE_SIZE_BYTES: HistogramVec = register_histogram_vec!(
        "doradura_file_size_bytes",
        "Size of files processed by format",
        &["format"],
        vec![1_000_000.0, 5_000_000.0, 10_000_000.0, 25_000_000.0, 50_000_000.0, 100_000_000.0, 500_000_000.0]
    )
    .unwrap();

    /// Cookies status (1 = valid, 0 = needs refresh)
    pub static ref COOKIES_STATUS: Gauge = register_gauge!(
        "doradura_cookies_status",
        "Cookies status (1 = valid, 0 = needs refresh)"
    )
    .unwrap();

    /// Platform distribution for downloads
    /// Labels: platform (youtube/soundcloud/vimeo/etc)
    pub static ref PLATFORM_DOWNLOADS_TOTAL: CounterVec = register_counter_vec!(
        "doradura_platform_downloads_total",
        "Downloads by source platform",
        &["platform"]
    )
    .unwrap();

    /// User feedback count
    /// Labels: sentiment (positive/neutral/negative)
    pub static ref USER_FEEDBACK_TOTAL: CounterVec = register_counter_vec!(
        "doradura_user_feedback_total",
        "User feedback submissions by sentiment",
        &["sentiment"]
    )
    .unwrap();

    /// Alert count by type and severity
    /// Labels: alert_type, severity
    pub static ref ALERTS_TOTAL: CounterVec = register_counter_vec!(
        "doradura_alerts_total",
        "Alerts triggered by type and severity",
        &["alert_type", "severity"]
    )
    .unwrap();
}

// ======================
// USER ENGAGEMENT METRICS
// ======================

lazy_static! {
    /// Daily active users count
    pub static ref DAILY_ACTIVE_USERS: Gauge = register_gauge!(
        "doradura_daily_active_users",
        "Number of daily active users (last 24h)"
    )
    .unwrap();

    /// Monthly active users count
    pub static ref MONTHLY_ACTIVE_USERS: Gauge = register_gauge!(
        "doradura_monthly_active_users",
        "Number of monthly active users (last 30d)"
    )
    .unwrap();

    /// Command usage count
    /// Labels: command (start/settings/info/history/etc)
    pub static ref COMMAND_USAGE_TOTAL: CounterVec = register_counter_vec!(
        "doradura_command_usage_total",
        "Total number of command executions",
        &["command"]
    )
    .unwrap();

    /// Format request count
    /// Labels: format, plan
    pub static ref FORMAT_REQUESTS_TOTAL: CounterVec = register_counter_vec!(
        "doradura_format_requests_total",
        "Total number of format requests by plan",
        &["format", "plan"]
    )
    .unwrap();

    /// User language distribution
    /// Labels: language (en/ru/de/fr/etc)
    pub static ref USER_LANGUAGE_DISTRIBUTION: GaugeVec = register_gauge_vec!(
        "doradura_user_language_distribution",
        "Distribution of users by language",
        &["language"]
    )
    .unwrap();

    /// Message types processed
    /// Labels: message_type (text/url/command/etc)
    pub static ref MESSAGE_TYPES_TOTAL: CounterVec = register_counter_vec!(
        "doradura_message_types_total",
        "Total number of messages by type",
        &["message_type"]
    )
    .unwrap();

    /// Total registered users
    pub static ref TOTAL_USERS: Gauge = register_gauge!(
        "doradura_total_users",
        "Total number of registered users"
    )
    .unwrap();

    /// Users by plan
    /// Labels: plan
    pub static ref USERS_BY_PLAN: GaugeVec = register_gauge_vec!(
        "doradura_users_by_plan",
        "Number of users by subscription plan",
        &["plan"]
    )
    .unwrap();
}

/// Initialize metrics (call this at startup to register all metrics)
pub fn init_metrics() {
    log::info!("Initializing metrics registry...");

    // Initialize all lazy statics by accessing them
    let _ = &*DOWNLOAD_DURATION_SECONDS;
    let _ = &*QUEUE_PROCESSING_DURATION_SECONDS;
    let _ = &*QUEUE_WAIT_TIME_SECONDS;
    let _ = &*DOWNLOAD_SUCCESS_TOTAL;
    let _ = &*DOWNLOAD_FAILURE_TOTAL;
    let _ = &*YTDLP_EXECUTION_DURATION_SECONDS;

    // Initialize download counters with common format combinations
    // This ensures they appear in /metrics even with 0 values
    DOWNLOAD_SUCCESS_TOTAL.with_label_values(&["mp3", "320k"]);
    DOWNLOAD_SUCCESS_TOTAL.with_label_values(&["mp3", "default"]);
    DOWNLOAD_SUCCESS_TOTAL.with_label_values(&["mp4", "1080p"]);
    DOWNLOAD_SUCCESS_TOTAL.with_label_values(&["mp4", "720p"]);
    DOWNLOAD_SUCCESS_TOTAL.with_label_values(&["mp4", "480p"]);
    DOWNLOAD_SUCCESS_TOTAL.with_label_values(&["srt", "default"]);
    DOWNLOAD_SUCCESS_TOTAL.with_label_values(&["txt", "default"]);

    DOWNLOAD_FAILURE_TOTAL.with_label_values(&["mp3", "timeout"]);
    DOWNLOAD_FAILURE_TOTAL.with_label_values(&["mp3", "file_too_large"]);
    DOWNLOAD_FAILURE_TOTAL.with_label_values(&["mp3", "ytdlp"]);
    DOWNLOAD_FAILURE_TOTAL.with_label_values(&["mp3", "network"]);
    DOWNLOAD_FAILURE_TOTAL.with_label_values(&["mp3", "other"]);
    DOWNLOAD_FAILURE_TOTAL.with_label_values(&["mp4", "timeout"]);
    DOWNLOAD_FAILURE_TOTAL.with_label_values(&["mp4", "file_too_large"]);
    DOWNLOAD_FAILURE_TOTAL.with_label_values(&["mp4", "ytdlp"]);
    DOWNLOAD_FAILURE_TOTAL.with_label_values(&["mp4", "network"]);
    DOWNLOAD_FAILURE_TOTAL.with_label_values(&["mp4", "other"]);
    DOWNLOAD_FAILURE_TOTAL.with_label_values(&["srt", "other"]);
    DOWNLOAD_FAILURE_TOTAL.with_label_values(&["txt", "other"]);

    let _ = &*ACTIVE_SUBSCRIPTIONS;
    let _ = &*REVENUE_TOTAL_STARS;
    let _ = &*REVENUE_BY_PLAN;
    let _ = &*NEW_SUBSCRIPTIONS_TOTAL;
    let _ = &*SUBSCRIPTION_CANCELLATIONS_TOTAL;
    let _ = &*PAYMENT_CHECKOUT_STARTED;
    let _ = &*PAYMENT_SUCCESS_TOTAL;
    let _ = &*PAYMENT_FAILURE_TOTAL;

    // Initialize subscription metrics by plan
    ACTIVE_SUBSCRIPTIONS.with_label_values(&["free"]);
    ACTIVE_SUBSCRIPTIONS.with_label_values(&["premium"]);
    ACTIVE_SUBSCRIPTIONS.with_label_values(&["vip"]);

    // Initialize revenue by plan
    REVENUE_BY_PLAN.with_label_values(&["premium"]);
    REVENUE_BY_PLAN.with_label_values(&["vip"]);

    // Initialize new subscriptions
    NEW_SUBSCRIPTIONS_TOTAL.with_label_values(&["premium", "true"]);
    NEW_SUBSCRIPTIONS_TOTAL.with_label_values(&["premium", "false"]);
    NEW_SUBSCRIPTIONS_TOTAL.with_label_values(&["vip", "true"]);
    NEW_SUBSCRIPTIONS_TOTAL.with_label_values(&["vip", "false"]);

    // Initialize payment metrics
    PAYMENT_SUCCESS_TOTAL.with_label_values(&["premium", "true"]);
    PAYMENT_SUCCESS_TOTAL.with_label_values(&["premium", "false"]);
    PAYMENT_SUCCESS_TOTAL.with_label_values(&["vip", "true"]);
    PAYMENT_SUCCESS_TOTAL.with_label_values(&["vip", "false"]);

    let _ = &*ERRORS_TOTAL;
    let _ = &*QUEUE_DEPTH;
    let _ = &*QUEUE_DEPTH_TOTAL;
    let _ = &*TASK_RETRIES_TOTAL;
    let _ = &*YTDLP_HEALTH_STATUS;
    let _ = &*RATE_LIMIT_HITS_TOTAL;
    let _ = &*DB_CONNECTIONS_ACTIVE;
    let _ = &*DB_CONNECTIONS_IDLE;
    let _ = &*BOT_UPTIME_SECONDS;
    let _ = &*DISPATCHER_RECONNECTIONS_TOTAL;

    // Initialize error counters with common error categories and operations
    ERRORS_TOTAL.with_label_values(&["download", "metadata"]);
    ERRORS_TOTAL.with_label_values(&["download", "audio_download"]);
    ERRORS_TOTAL.with_label_values(&["download", "video_download"]);
    ERRORS_TOTAL.with_label_values(&["download", "subtitle_download"]);
    ERRORS_TOTAL.with_label_values(&["telegram_api", "send_file"]);
    ERRORS_TOTAL.with_label_values(&["telegram_api", "send_file_timeout"]);
    ERRORS_TOTAL.with_label_values(&["database", "query"]);
    ERRORS_TOTAL.with_label_values(&["http", "request"]);
    ERRORS_TOTAL.with_label_values(&["io", "filesystem"]);
    ERRORS_TOTAL.with_label_values(&["validation", "size_limit"]);
    ERRORS_TOTAL.with_label_values(&["audio_effect", "processing"]);
    ERRORS_TOTAL.with_label_values(&["other", "unknown"]);

    // Initialize queue depth gauges
    QUEUE_DEPTH.with_label_values(&["low"]);
    QUEUE_DEPTH.with_label_values(&["medium"]);
    QUEUE_DEPTH.with_label_values(&["high"]);

    let _ = &*DAILY_ACTIVE_USERS;
    let _ = &*MONTHLY_ACTIVE_USERS;
    let _ = &*COMMAND_USAGE_TOTAL;
    let _ = &*FORMAT_REQUESTS_TOTAL;
    let _ = &*USER_LANGUAGE_DISTRIBUTION;
    let _ = &*MESSAGE_TYPES_TOTAL;
    let _ = &*TOTAL_USERS;
    let _ = &*USERS_BY_PLAN;

    // Initialize format request counters
    FORMAT_REQUESTS_TOTAL.with_label_values(&["mp3", "free"]);
    FORMAT_REQUESTS_TOTAL.with_label_values(&["mp3", "premium"]);
    FORMAT_REQUESTS_TOTAL.with_label_values(&["mp3", "vip"]);
    FORMAT_REQUESTS_TOTAL.with_label_values(&["mp4", "free"]);
    FORMAT_REQUESTS_TOTAL.with_label_values(&["mp4", "premium"]);
    FORMAT_REQUESTS_TOTAL.with_label_values(&["mp4", "vip"]);
    FORMAT_REQUESTS_TOTAL.with_label_values(&["srt", "free"]);
    FORMAT_REQUESTS_TOTAL.with_label_values(&["txt", "free"]);

    // Initialize command usage counters
    COMMAND_USAGE_TOTAL.with_label_values(&["start"]);
    COMMAND_USAGE_TOTAL.with_label_values(&["help"]);
    COMMAND_USAGE_TOTAL.with_label_values(&["settings"]);
    COMMAND_USAGE_TOTAL.with_label_values(&["history"]);
    COMMAND_USAGE_TOTAL.with_label_values(&["info"]);

    // Initialize users by plan gauges
    USERS_BY_PLAN.with_label_values(&["free"]);
    USERS_BY_PLAN.with_label_values(&["premium"]);
    USERS_BY_PLAN.with_label_values(&["vip"]);

    // Set yt-dlp status to healthy by default
    YTDLP_HEALTH_STATUS.set(1.0);

    // Initialize new operation metrics
    let _ = &*OPERATION_DURATION_SECONDS;
    let _ = &*OPERATION_SUCCESS_TOTAL;
    let _ = &*OPERATION_FAILURE_TOTAL;
    let _ = &*FILE_SIZE_BYTES;
    let _ = &*COOKIES_STATUS;
    let _ = &*PLATFORM_DOWNLOADS_TOTAL;
    let _ = &*USER_FEEDBACK_TOTAL;
    let _ = &*ALERTS_TOTAL;

    // Initialize operation metrics with common labels
    OPERATION_SUCCESS_TOTAL.with_label_values(&["download", "mp3"]);
    OPERATION_SUCCESS_TOTAL.with_label_values(&["download", "mp4"]);
    OPERATION_SUCCESS_TOTAL.with_label_values(&["upload", "mp3"]);
    OPERATION_SUCCESS_TOTAL.with_label_values(&["upload", "mp4"]);

    // Initialize platform metrics
    PLATFORM_DOWNLOADS_TOTAL.with_label_values(&["youtube"]);
    PLATFORM_DOWNLOADS_TOTAL.with_label_values(&["soundcloud"]);
    PLATFORM_DOWNLOADS_TOTAL.with_label_values(&["vimeo"]);
    PLATFORM_DOWNLOADS_TOTAL.with_label_values(&["other"]);

    // Set cookies status to valid by default
    COOKIES_STATUS.set(1.0);

    log::info!("Metrics registry initialized successfully");
}

/// Helper function to record download success
pub fn record_download_success(format: &str, quality: &str) {
    DOWNLOAD_SUCCESS_TOTAL.with_label_values(&[format, quality]).inc();
}

/// Helper function to record download failure
pub fn record_download_failure(format: &str, error_type: &str) {
    DOWNLOAD_FAILURE_TOTAL.with_label_values(&[format, error_type]).inc();
}

/// Helper function to record error
pub fn record_error(error_type: &str, operation: &str) {
    ERRORS_TOTAL.with_label_values(&[error_type, operation]).inc();
}

/// Helper function to record command usage
pub fn record_command(command: &str) {
    COMMAND_USAGE_TOTAL.with_label_values(&[command]).inc();
}

/// Helper function to record format request
pub fn record_format_request(format: &str, plan: &str) {
    FORMAT_REQUESTS_TOTAL.with_label_values(&[format, plan]).inc();
}

/// Helper function to record rate limit hit
pub fn record_rate_limit_hit(plan: &str) {
    RATE_LIMIT_HITS_TOTAL.with_label_values(&[plan]).inc();
}

/// Helper function to update queue depth
pub fn update_queue_depth(priority: &str, depth: usize) {
    QUEUE_DEPTH.with_label_values(&[priority]).set(depth as f64);
}

/// Helper function to update total queue depth
pub fn update_queue_depth_total(depth: usize) {
    QUEUE_DEPTH_TOTAL.set(depth as f64);
}

/// Helper function to record payment success
pub fn record_payment_success(plan: &str, is_recurring: bool) {
    let is_recurring_str = if is_recurring { "true" } else { "false" };
    PAYMENT_SUCCESS_TOTAL.with_label_values(&[plan, is_recurring_str]).inc();
}

/// Helper function to record payment failure
pub fn record_payment_failure(plan: &str, reason: &str) {
    PAYMENT_FAILURE_TOTAL.with_label_values(&[plan, reason]).inc();
}

/// Helper function to record revenue
pub fn record_revenue(plan: &str, amount: f64) {
    REVENUE_TOTAL_STARS.inc_by(amount);
    REVENUE_BY_PLAN.with_label_values(&[plan]).inc_by(amount);
}

/// Helper function to record operation start (returns timer)
pub fn start_operation_timer(operation_type: &str, format: &str) -> prometheus::HistogramTimer {
    OPERATION_DURATION_SECONDS
        .with_label_values(&[operation_type, format])
        .start_timer()
}

/// Helper function to record operation success
pub fn record_operation_success(operation_type: &str, format: &str) {
    OPERATION_SUCCESS_TOTAL
        .with_label_values(&[operation_type, format])
        .inc();
}

/// Helper function to record operation failure
pub fn record_operation_failure(operation_type: &str, format: &str, error_category: &str) {
    OPERATION_FAILURE_TOTAL
        .with_label_values(&[operation_type, format, error_category])
        .inc();
}

/// Helper function to record file size
pub fn record_file_size(format: &str, size_bytes: u64) {
    FILE_SIZE_BYTES.with_label_values(&[format]).observe(size_bytes as f64);
}

/// Helper function to record platform download
pub fn record_platform_download(platform: &str) {
    PLATFORM_DOWNLOADS_TOTAL.with_label_values(&[platform]).inc();
}

/// Helper function to update cookies status
pub fn update_cookies_status(valid: bool) {
    COOKIES_STATUS.set(if valid { 1.0 } else { 0.0 });
}

/// Helper function to record user feedback
pub fn record_user_feedback(sentiment: &str) {
    USER_FEEDBACK_TOTAL.with_label_values(&[sentiment]).inc();
}

/// Helper function to record alert
pub fn record_alert(alert_type: &str, severity: &str) {
    ALERTS_TOTAL.with_label_values(&[alert_type, severity]).inc();
}

/// Extract platform from URL for metrics
pub fn extract_platform(url: &str) -> &'static str {
    let url_lower = url.to_lowercase();
    if url_lower.contains("youtube.com") || url_lower.contains("youtu.be") {
        "youtube"
    } else if url_lower.contains("soundcloud.com") {
        "soundcloud"
    } else if url_lower.contains("vimeo.com") {
        "vimeo"
    } else if url_lower.contains("tiktok.com") {
        "tiktok"
    } else if url_lower.contains("instagram.com") {
        "instagram"
    } else if url_lower.contains("twitter.com") || url_lower.contains("x.com") {
        "twitter"
    } else if url_lower.contains("spotify.com") {
        "spotify"
    } else if url_lower.contains("bandcamp.com") {
        "bandcamp"
    } else if url_lower.contains("twitch.tv") {
        "twitch"
    } else if url_lower.contains("dailymotion.com") {
        "dailymotion"
    } else {
        "other"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_init_metrics() {
        init_metrics();
        // If this doesn't panic, metrics were initialized successfully
    }

    #[test]
    fn test_record_download_success() {
        record_download_success("mp3", "320k");
        let metric = DOWNLOAD_SUCCESS_TOTAL.with_label_values(&["mp3", "320k"]).get();
        assert!(metric >= 1.0);
    }

    #[test]
    fn test_record_command() {
        record_command("start");
        let metric = COMMAND_USAGE_TOTAL.with_label_values(&["start"]).get();
        assert!(metric >= 1.0);
    }

    #[test]
    fn test_update_queue_depth() {
        update_queue_depth("high", 10);
        let metric = QUEUE_DEPTH.with_label_values(&["high"]).get();
        assert_eq!(metric, 10.0);
    }
}
