//! Metrics collection for the Telegram bot using Prometheus
//!
//! This module provides a centralized metrics registry for tracking:
//! - Performance metrics (download duration, queue processing time)
//! - Business metrics (revenue, subscriptions, conversions)
//! - System health metrics (errors, queue depth, yt-dlp status)
//! - User engagement metrics (DAU/MAU, command usage)

use std::sync::LazyLock;

use prometheus::{
    register_counter, register_counter_vec, register_gauge, register_gauge_vec, register_histogram,
    register_histogram_vec, register_int_counter_vec, register_int_gauge, Counter, CounterVec, Gauge, GaugeVec,
    Histogram, HistogramVec, IntCounterVec, IntGauge,
};

/// Declare a Prometheus metric static with uniform panic-on-registration-failure handling.
/// Registration failure here means a duplicate metric name or invalid label — both programmer
/// errors caught at `init_metrics()` boot, not in a user-facing task.
macro_rules! metric {
    ($(#[$attr:meta])* pub $name:ident: Counter = $id:literal, $help:literal) => {
        $(#[$attr])*
        pub static $name: LazyLock<Counter> = LazyLock::new(|| {
            register_counter!($id, $help)
                .unwrap_or_else(|e| panic!("register {}: {}", stringify!($name), e))
        });
    };
    ($(#[$attr:meta])* pub $name:ident: Gauge = $id:literal, $help:literal) => {
        $(#[$attr])*
        pub static $name: LazyLock<Gauge> = LazyLock::new(|| {
            register_gauge!($id, $help)
                .unwrap_or_else(|e| panic!("register {}: {}", stringify!($name), e))
        });
    };
    ($(#[$attr:meta])* pub $name:ident: IntGauge = $id:literal, $help:literal) => {
        $(#[$attr])*
        pub static $name: LazyLock<IntGauge> = LazyLock::new(|| {
            register_int_gauge!($id, $help)
                .unwrap_or_else(|e| panic!("register {}: {}", stringify!($name), e))
        });
    };
    ($(#[$attr:meta])* pub $name:ident: CounterVec = $id:literal, $help:literal, labels = [$($lbl:literal),+ $(,)?]) => {
        $(#[$attr])*
        pub static $name: LazyLock<CounterVec> = LazyLock::new(|| {
            register_counter_vec!($id, $help, &[$($lbl),+])
                .unwrap_or_else(|e| panic!("register {}: {}", stringify!($name), e))
        });
    };
    ($(#[$attr:meta])* pub $name:ident: GaugeVec = $id:literal, $help:literal, labels = [$($lbl:literal),+ $(,)?]) => {
        $(#[$attr])*
        pub static $name: LazyLock<GaugeVec> = LazyLock::new(|| {
            register_gauge_vec!($id, $help, &[$($lbl),+])
                .unwrap_or_else(|e| panic!("register {}: {}", stringify!($name), e))
        });
    };
    ($(#[$attr:meta])* pub $name:ident: IntCounterVec = $id:literal, $help:literal, labels = [$($lbl:literal),+ $(,)?]) => {
        $(#[$attr])*
        pub static $name: LazyLock<IntCounterVec> = LazyLock::new(|| {
            register_int_counter_vec!($id, $help, &[$($lbl),+])
                .unwrap_or_else(|e| panic!("register {}: {}", stringify!($name), e))
        });
    };
    ($(#[$attr:meta])* pub $name:ident: Histogram = $id:literal, $help:literal, buckets = $buckets:expr) => {
        $(#[$attr])*
        pub static $name: LazyLock<Histogram> = LazyLock::new(|| {
            register_histogram!($id, $help, $buckets)
                .unwrap_or_else(|e| panic!("register {}: {}", stringify!($name), e))
        });
    };
    ($(#[$attr:meta])* pub $name:ident: HistogramVec = $id:literal, $help:literal, labels = [$($lbl:literal),+ $(,)?], buckets = $buckets:expr) => {
        $(#[$attr])*
        pub static $name: LazyLock<HistogramVec> = LazyLock::new(|| {
            register_histogram_vec!($id, $help, &[$($lbl),+], $buckets)
                .unwrap_or_else(|e| panic!("register {}: {}", stringify!($name), e))
        });
    };
}

// ======================
// PERFORMANCE METRICS
// ======================

metric!(
    /// Download duration in seconds by format and quality
    /// Labels: format (mp3/mp4/srt/txt), quality (320k/1080p/etc)
    pub DOWNLOAD_DURATION_SECONDS: HistogramVec =
        "doradura_download_duration_seconds",
        "Time spent downloading files by format and quality",
        labels = ["format", "quality"],
        buckets = vec![1.0, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0, 600.0]
);

metric!(
    /// Queue processing time per iteration
    pub QUEUE_PROCESSING_DURATION_SECONDS: Histogram =
        "doradura_queue_processing_duration_seconds",
        "Time spent processing queue per iteration",
        buckets = vec![0.1, 0.5, 1.0, 5.0, 10.0, 30.0]
);

metric!(
    /// Queue wait time from task creation to processing
    /// Labels: priority (low/medium/high)
    pub QUEUE_WAIT_TIME_SECONDS: HistogramVec =
        "doradura_queue_wait_time_seconds",
        "Time tasks spend waiting in queue before processing",
        labels = ["priority"],
        buckets = vec![5.0, 30.0, 60.0, 300.0, 600.0, 1800.0, 3600.0]
);

metric!(
    /// Successful downloads count
    /// Labels: format, quality
    pub DOWNLOAD_SUCCESS_TOTAL: CounterVec =
        "doradura_download_success_total",
        "Total number of successful downloads",
        labels = ["format", "quality"]
);

metric!(
    /// Failed downloads count
    /// Labels: format, error_type
    pub DOWNLOAD_FAILURE_TOTAL: CounterVec =
        "doradura_download_failure_total",
        "Total number of failed downloads",
        labels = ["format", "error_type"]
);

metric!(
    /// yt-dlp command execution duration
    /// Labels: operation (metadata/download/etc)
    pub YTDLP_EXECUTION_DURATION_SECONDS: HistogramVec =
        "doradura_ytdlp_execution_duration_seconds",
        "Time spent executing yt-dlp commands",
        labels = ["operation"],
        buckets = vec![1.0, 5.0, 10.0, 30.0, 60.0, 120.0, 240.0]
);

// ======================
// BUSINESS METRICS
// ======================

metric!(
    /// Active subscriptions count by plan
    /// Labels: plan (free/premium/vip)
    pub ACTIVE_SUBSCRIPTIONS: GaugeVec =
        "doradura_active_subscriptions",
        "Number of active subscriptions by plan",
        labels = ["plan"]
);

metric!(
    /// Total revenue in Telegram Stars
    pub REVENUE_TOTAL_STARS: Counter =
        "doradura_revenue_total_stars",
        "Total revenue in Telegram Stars"
);

metric!(
    /// Revenue by subscription plan
    /// Labels: plan
    pub REVENUE_BY_PLAN: CounterVec =
        "doradura_revenue_by_plan",
        "Revenue by subscription plan in Stars",
        labels = ["plan"]
);

metric!(
    /// New subscriptions count
    /// Labels: plan, is_recurring (true/false)
    pub NEW_SUBSCRIPTIONS_TOTAL: CounterVec =
        "doradura_new_subscriptions_total",
        "Total number of new subscriptions",
        labels = ["plan", "is_recurring"]
);

metric!(
    /// Subscription cancellations
    /// Labels: plan
    pub SUBSCRIPTION_CANCELLATIONS_TOTAL: CounterVec =
        "doradura_subscription_cancellations_total",
        "Total number of subscription cancellations",
        labels = ["plan"]
);

metric!(
    /// Payment checkout started
    /// Labels: plan
    pub PAYMENT_CHECKOUT_STARTED: CounterVec =
        "doradura_payment_checkout_started",
        "Number of times payment checkout was initiated",
        labels = ["plan"]
);

metric!(
    /// Successful payments
    /// Labels: plan, is_recurring
    pub PAYMENT_SUCCESS_TOTAL: CounterVec =
        "doradura_payment_success_total",
        "Total number of successful payments",
        labels = ["plan", "is_recurring"]
);

metric!(
    /// Failed payments
    /// Labels: plan, reason
    pub PAYMENT_FAILURE_TOTAL: CounterVec =
        "doradura_payment_failure_total",
        "Total number of failed payments",
        labels = ["plan", "reason"]
);

// ======================
// SYSTEM HEALTH METRICS
// ======================

metric!(
    /// Errors count by type and operation
    /// Labels: error_type (download/telegram/database/http), operation
    pub ERRORS_TOTAL: CounterVec =
        "doradura_errors_total",
        "Total number of errors by type and operation",
        labels = ["error_type", "operation"]
);

metric!(
    /// Current queue depth by priority
    /// Labels: priority (low/medium/high)
    pub QUEUE_DEPTH: GaugeVec =
        "doradura_queue_depth",
        "Current number of tasks in queue by priority",
        labels = ["priority"]
);

metric!(
    /// Total queue depth across all priorities
    pub QUEUE_DEPTH_TOTAL: Gauge =
        "doradura_queue_depth_total",
        "Total number of tasks in queue"
);

metric!(
    /// Task retry count
    /// Labels: retry_count (1/2/3/4/5)
    pub TASK_RETRIES_TOTAL: CounterVec =
        "doradura_task_retries_total",
        "Total number of task retries",
        labels = ["retry_count"]
);

metric!(
    /// yt-dlp health status (1 = healthy, 0 = unhealthy)
    pub YTDLP_HEALTH_STATUS: Gauge =
        "doradura_ytdlp_health_status",
        "yt-dlp health status (1 = healthy, 0 = unhealthy)"
);

metric!(
    /// Rate limit hits count
    /// Labels: plan
    pub RATE_LIMIT_HITS_TOTAL: CounterVec =
        "doradura_rate_limit_hits_total",
        "Total number of rate limit hits",
        labels = ["plan"]
);

metric!(
    /// Active database connections
    pub DB_CONNECTIONS_ACTIVE: Gauge =
        "doradura_db_connections_active",
        "Number of active database connections"
);

metric!(
    /// Idle database connections
    pub DB_CONNECTIONS_IDLE: Gauge =
        "doradura_db_connections_idle",
        "Number of idle database connections"
);

metric!(
    /// Bot uptime in seconds
    pub BOT_UPTIME_SECONDS: Counter =
        "doradura_bot_uptime_seconds",
        "Bot uptime in seconds"
);

metric!(
    /// Dispatcher reconnection count
    pub DISPATCHER_RECONNECTIONS_TOTAL: Counter =
        "doradura_dispatcher_reconnections_total",
        "Total number of dispatcher reconnections"
);

metric!(
    /// File size distribution
    /// Labels: format
    pub FILE_SIZE_BYTES: HistogramVec =
        "doradura_file_size_bytes",
        "Size of files processed by format",
        labels = ["format"],
        buckets = vec![
            1_000_000.0,
            5_000_000.0,
            10_000_000.0,
            25_000_000.0,
            50_000_000.0,
            100_000_000.0,
            500_000_000.0
        ]
);

metric!(
    /// Cookies status (1 = valid, 0 = needs refresh)
    pub COOKIES_STATUS: Gauge =
        "doradura_cookies_status",
        "Cookies status (1 = valid, 0 = needs refresh)"
);

metric!(
    /// Platform distribution for downloads
    /// Labels: platform (youtube/soundcloud/vimeo/etc)
    pub PLATFORM_DOWNLOADS_TOTAL: CounterVec =
        "doradura_platform_downloads_total",
        "Downloads by source platform",
        labels = ["platform"]
);

metric!(
    /// User feedback count
    /// Labels: sentiment (positive/neutral/negative)
    pub USER_FEEDBACK_TOTAL: CounterVec =
        "doradura_user_feedback_total",
        "User feedback submissions by sentiment",
        labels = ["sentiment"]
);

metric!(
    /// Alert count by type and severity
    /// Labels: alert_type, severity
    pub ALERTS_TOTAL: CounterVec =
        "doradura_alerts_total",
        "Alerts triggered by type and severity",
        labels = ["alert_type", "severity"]
);

// ======================
// USER ENGAGEMENT METRICS
// ======================

metric!(
    /// Daily active users count
    pub DAILY_ACTIVE_USERS: Gauge =
        "doradura_daily_active_users",
        "Number of daily active users (last 24h)"
);

metric!(
    /// Monthly active users count
    pub MONTHLY_ACTIVE_USERS: Gauge =
        "doradura_monthly_active_users",
        "Number of monthly active users (last 30d)"
);

metric!(
    /// Command usage count
    /// Labels: command (start/settings/info/history/etc)
    pub COMMAND_USAGE_TOTAL: CounterVec =
        "doradura_command_usage_total",
        "Total number of command executions",
        labels = ["command"]
);

metric!(
    /// Format request count
    /// Labels: format, plan
    pub FORMAT_REQUESTS_TOTAL: CounterVec =
        "doradura_format_requests_total",
        "Total number of format requests by plan",
        labels = ["format", "plan"]
);

metric!(
    /// User language distribution
    /// Labels: language (en/ru/de/fr/etc)
    pub USER_LANGUAGE_DISTRIBUTION: GaugeVec =
        "doradura_user_language_distribution",
        "Distribution of users by language",
        labels = ["language"]
);

metric!(
    /// Message types processed
    /// Labels: message_type (text/url/command/etc)
    pub MESSAGE_TYPES_TOTAL: CounterVec =
        "doradura_message_types_total",
        "Total number of messages by type",
        labels = ["message_type"]
);

metric!(
    /// Total registered users
    pub TOTAL_USERS: Gauge =
        "doradura_total_users",
        "Total number of registered users"
);

metric!(
    /// Users by plan
    /// Labels: plan
    pub USERS_BY_PLAN: GaugeVec =
        "doradura_users_by_plan",
        "Number of users by subscription plan",
        labels = ["plan"]
);

// ======================
// PIPELINE & EXTERNAL API METRICS
// ======================

metric!(
    /// Build information gauge (always 1, labels carry version info)
    /// Labels: version
    pub BUILD_INFO: GaugeVec =
        "doradura_build_info",
        "Build information (always 1)",
        labels = ["version"]
);

metric!(
    /// Proxy request outcomes
    /// Labels: proxy_type (warp/tailscale/direct), result (success/failure/timeout)
    pub PROXY_REQUESTS_TOTAL: CounterVec =
        "doradura_proxy_requests_total",
        "Proxy request outcomes",
        labels = ["proxy_type", "result"]
);

metric!(
    /// Metadata fetch duration via yt-dlp
    pub METADATA_FETCH_DURATION_SECONDS: Histogram =
        "doradura_metadata_fetch_duration_seconds",
        "Duration of yt-dlp metadata extraction",
        buckets = vec![0.5, 1.0, 2.0, 5.0, 10.0, 20.0, 30.0]
);

metric!(
    /// Audio effects processing duration
    pub AUDIO_EFFECTS_DURATION_SECONDS: Histogram =
        "doradura_audio_effects_duration_seconds",
        "Duration of audio effects processing (ffmpeg)",
        buckets = vec![1.0, 5.0, 10.0, 30.0, 60.0, 120.0]
);

metric!(
    /// Video encoding duration (subtitle burning, splitting)
    /// Labels: operation (burn_subtitles/split)
    pub VIDEO_ENCODING_DURATION_SECONDS: HistogramVec =
        "doradura_video_encoding_duration_seconds",
        "Duration of video encoding operations",
        labels = ["operation"],
        buckets = vec![1.0, 5.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0]
);

metric!(
    /// Metadata cache hit ratio
    /// Labels: cache_type (metadata/preview)
    pub CACHE_HIT_RATIO: GaugeVec =
        "doradura_cache_hit_ratio",
        "Cache hit ratio (0.0-1.0)",
        labels = ["cache_type"]
);

metric!(
    /// Cross-user file_id cache outcomes.
    ///
    /// Counts every pipeline execution's lookup result. Hit rate over a time
    /// window is `sum(hit) / sum(hit+miss)` — the PRD target is 80%+.
    ///
    /// Labels:
    ///   - `source`: `download_history` (cross-user cache) | `vault` (audio dedup layer)
    ///   - `outcome`: `hit` | `miss` | `send_failed` (hit but file_id expired on Bot API server)
    pub FILE_ID_CACHE_TOTAL: IntCounterVec =
        "doradura_file_id_cache_total",
        "Cross-user file_id cache lookup outcomes",
        labels = ["source", "outcome"]
);

metric!(
    /// Loop-to-audio feature outcomes.
    ///
    /// Counts every invocation of the "🔁 Loop to audio" flow, broken down by
    /// outcome. Use to monitor success rate and flag gating regressions.
    ///
    /// Labels:
    ///   - `outcome`: `success` | `audio_too_long` | `audio_too_short`
    ///     | `video_too_short` | `ffmpeg_failed` | `download_failed` | `send_failed`
    pub LOOP_TO_AUDIO_TOTAL: IntCounterVec =
        "doradura_loop_to_audio_total",
        "Loop-to-audio feature outcomes",
        labels = ["outcome"]
);

metric!(
    /// Process resident memory in bytes (RSS)
    pub PROCESS_RESIDENT_MEMORY_BYTES: Gauge =
        "doradura_process_resident_memory_bytes",
        "Process resident set size (RSS) in bytes"
);

metric!(
    /// Semaphore full events (concurrent download limit reached)
    pub SEMAPHORE_FULL_TOTAL: Counter =
        "doradura_semaphore_full_total",
        "Number of times download semaphore was at capacity"
);

// ======================
// DISK METRICS
// ======================

metric!(
    /// Available disk space in bytes
    pub DISK_AVAILABLE_BYTES: Gauge =
        "doradura_disk_available_bytes",
        "Available disk space in bytes"
);

metric!(
    /// Disk used percentage (0-100)
    pub DISK_USED_PERCENT: Gauge =
        "doradura_disk_used_percent",
        "Disk used percentage (0-100)"
);

// ======================
// NEW HIGH-VALUE METRICS
// ======================

metric!(
    /// yt-dlp fallback tier attempts
    /// Labels: tier (tier1_no_cookies/tier2_cookies/tier3_fixup_never), result (success/failure)
    pub YTDLP_TIER_ATTEMPTS: IntCounterVec =
        "doradura_ytdlp_tier_attempts_total",
        "yt-dlp fallback tier attempts",
        labels = ["tier", "result"]
);

metric!(
    /// Number of downloads currently in progress
    pub CONCURRENT_DOWNLOADS: IntGauge =
        "doradura_concurrent_downloads",
        "Number of downloads currently in progress"
);

metric!(
    /// Instagram rate limiter window size
    pub INSTAGRAM_RATE_LIMITER_QUEUE: IntGauge =
        "doradura_instagram_rate_limiter_queue_size",
        "Current Instagram rate limiter window size"
);

metric!(
    /// Telegram file send duration
    /// Labels: file_type (audio/video)
    pub TELEGRAM_SEND_DURATION_SECONDS: HistogramVec =
        "doradura_telegram_send_duration_seconds",
        "Telegram file send duration",
        labels = ["file_type"],
        buckets = vec![1.0, 5.0, 10.0, 30.0, 60.0, 120.0]
);

// ======================
// HEALTH CHECK / SMOKE TEST METRICS
// ======================

metric!(
    /// Health check status (1 = healthy, 0 = unhealthy)
    pub HEALTH_CHECK_STATUS: Gauge =
        "doradura_health_check_status",
        "Health check status (1 = healthy, 0 = unhealthy)"
);

metric!(
    /// Last health check run timestamp (Unix seconds)
    pub HEALTH_CHECK_LAST_RUN: Gauge =
        "doradura_health_check_last_run_timestamp",
        "Timestamp of last health check run (Unix seconds)"
);

metric!(
    /// Smoke test results count by test name and status
    /// Labels: test_name (ffmpeg_toolchain/cookies_validation/metadata_extraction/audio_download/video_download)
    ///         status (passed/failed/timeout/skipped)
    pub SMOKE_TEST_RESULTS: CounterVec =
        "doradura_smoke_test_results_total",
        "Total number of smoke test results by test and status",
        labels = ["test_name", "status"]
);

metric!(
    /// Smoke test duration in seconds by test name
    /// Labels: test_name
    pub SMOKE_TEST_DURATION: HistogramVec =
        "doradura_smoke_test_duration_seconds",
        "Duration of smoke tests in seconds",
        labels = ["test_name"],
        buckets = vec![1.0, 5.0, 10.0, 30.0, 60.0, 120.0, 180.0]
);

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

    // Initialize remaining system health metrics
    let _ = &*FILE_SIZE_BYTES;
    let _ = &*COOKIES_STATUS;
    let _ = &*PLATFORM_DOWNLOADS_TOTAL;
    let _ = &*USER_FEEDBACK_TOTAL;
    let _ = &*ALERTS_TOTAL;

    // Initialize platform metrics
    PLATFORM_DOWNLOADS_TOTAL.with_label_values(&["youtube"]);
    PLATFORM_DOWNLOADS_TOTAL.with_label_values(&["soundcloud"]);
    PLATFORM_DOWNLOADS_TOTAL.with_label_values(&["vimeo"]);
    PLATFORM_DOWNLOADS_TOTAL.with_label_values(&["other"]);

    // Set cookies status to valid by default
    COOKIES_STATUS.set(1.0);

    // Initialize disk metrics
    let _ = &*DISK_AVAILABLE_BYTES;
    let _ = &*DISK_USED_PERCENT;

    // Initialize new high-value metrics
    let _ = &*YTDLP_TIER_ATTEMPTS;
    let _ = &*CONCURRENT_DOWNLOADS;
    let _ = &*INSTAGRAM_RATE_LIMITER_QUEUE;
    let _ = &*TELEGRAM_SEND_DURATION_SECONDS;

    YTDLP_TIER_ATTEMPTS.with_label_values(&["tier1_no_cookies", "success"]);
    YTDLP_TIER_ATTEMPTS.with_label_values(&["tier1_no_cookies", "failure"]);
    YTDLP_TIER_ATTEMPTS.with_label_values(&["tier2_cookies", "success"]);
    YTDLP_TIER_ATTEMPTS.with_label_values(&["tier2_cookies", "failure"]);
    YTDLP_TIER_ATTEMPTS.with_label_values(&["tier3_fixup_never", "success"]);
    YTDLP_TIER_ATTEMPTS.with_label_values(&["tier3_fixup_never", "failure"]);

    TELEGRAM_SEND_DURATION_SECONDS.with_label_values(&["audio"]);
    TELEGRAM_SEND_DURATION_SECONDS.with_label_values(&["video"]);

    // Initialize pipeline & external API metrics
    let _ = &*BUILD_INFO;
    let _ = &*PROXY_REQUESTS_TOTAL;
    let _ = &*METADATA_FETCH_DURATION_SECONDS;
    let _ = &*AUDIO_EFFECTS_DURATION_SECONDS;
    let _ = &*VIDEO_ENCODING_DURATION_SECONDS;
    let _ = &*CACHE_HIT_RATIO;
    let _ = &*SEMAPHORE_FULL_TOTAL;

    let _ = &*PROCESS_RESIDENT_MEMORY_BYTES;

    PROXY_REQUESTS_TOTAL.with_label_values(&["warp", "success"]);
    PROXY_REQUESTS_TOTAL.with_label_values(&["warp", "failure"]);
    PROXY_REQUESTS_TOTAL.with_label_values(&["direct", "success"]);
    PROXY_REQUESTS_TOTAL.with_label_values(&["direct", "failure"]);

    VIDEO_ENCODING_DURATION_SECONDS.with_label_values(&["burn_subtitles"]);
    VIDEO_ENCODING_DURATION_SECONDS.with_label_values(&["split"]);

    CACHE_HIT_RATIO.with_label_values(&["metadata"]);
    CACHE_HIT_RATIO.with_label_values(&["preview"]);

    // Initialize health check / smoke test metrics
    let _ = &*HEALTH_CHECK_STATUS;
    let _ = &*HEALTH_CHECK_LAST_RUN;
    let _ = &*SMOKE_TEST_RESULTS;
    let _ = &*SMOKE_TEST_DURATION;

    // Set health check status to unknown (0) initially
    HEALTH_CHECK_STATUS.set(0.0);

    // Initialize smoke test result counters
    for test_name in [
        "ffmpeg_toolchain",
        "cookies_validation",
        "metadata_extraction",
        "audio_download",
        "video_download",
    ] {
        SMOKE_TEST_RESULTS.with_label_values(&[test_name, "passed"]);
        SMOKE_TEST_RESULTS.with_label_values(&[test_name, "failed"]);
        SMOKE_TEST_RESULTS.with_label_values(&[test_name, "timeout"]);
        SMOKE_TEST_RESULTS.with_label_values(&[test_name, "skipped"]);
    }

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

/// Helper function to record yt-dlp tier attempt
pub fn record_tier_attempt(tier: &str, success: bool) {
    YTDLP_TIER_ATTEMPTS
        .with_label_values(&[tier, if success { "success" } else { "failure" }])
        .inc();
}

/// Helper function to record proxy request outcome
pub fn record_proxy_request(proxy_type: &str, result: &str) {
    PROXY_REQUESTS_TOTAL.with_label_values(&[proxy_type, result]).inc();
}

/// Helper function to record message type
pub fn record_message_type(message_type: &str) {
    MESSAGE_TYPES_TOTAL.with_label_values(&[message_type]).inc();
}

/// Update process resident memory from /proc/self/statm (Linux only — Railway runs Linux)
pub fn update_process_memory() {
    if let Ok(statm) = fs_err::read_to_string("/proc/self/statm") {
        // Fields: size resident shared text lib data dt (in pages)
        if let Some(rss_pages) = statm.split_whitespace().nth(1) {
            if let Ok(pages) = rss_pages.parse::<u64>() {
                let page_size = 4096u64; // standard Linux page size
                PROCESS_RESIDENT_MEMORY_BYTES.set((pages * page_size) as f64);
            }
        }
    }
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
