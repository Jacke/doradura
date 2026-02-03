use once_cell::sync::Lazy;
use std::env;
use std::time::Duration;

/// Configuration constants for the bot
/// Cached yt-dlp binary path
/// Read once at startup from YTDL_BIN environment variable or defaults to "yt-dlp"
pub static YTDL_BIN: Lazy<String> = Lazy::new(|| env::var("YTDL_BIN").unwrap_or_else(|_| "yt-dlp".to_string()));

/// Browser to extract cookies from for YouTube authentication
/// Read from YTDL_COOKIES_BROWSER environment variable
/// Supported: chrome, firefox, safari, brave, chromium, edge, opera, vivaldi
/// Set to empty string to disable cookie extraction
///
/// NOTE: On macOS, browser cookie extraction requires Full Disk Access permission
/// It's recommended to use YTDL_COOKIES_FILE instead (see MACOS_COOKIES_FIX.md)
///
/// Default: empty (use YTDL_COOKIES_FILE for macOS)
pub static YTDL_COOKIES_BROWSER: Lazy<String> =
    Lazy::new(|| env::var("YTDL_COOKIES_BROWSER").unwrap_or_else(|_| String::new()));

/// Path to cookies file for YouTube authentication
/// Read from YTDL_COOKIES_FILE environment variable
/// If set, this takes priority over YTDL_COOKIES_BROWSER
/// Example: youtube_cookies.txt
pub static YTDL_COOKIES_FILE: Lazy<Option<String>> = Lazy::new(|| env::var("YTDL_COOKIES_FILE").ok());

/// Download folder path
/// Read from DOWNLOAD_FOLDER environment variable
/// Defaults to ~/downloads/dora-files on macOS, ~/downloads on other platforms
/// Supports tilde (~) expansion for home directory
pub static DOWNLOAD_FOLDER: Lazy<String> = Lazy::new(|| {
    env::var("DOWNLOAD_FOLDER").unwrap_or_else(|_| {
        #[cfg(target_os = "macos")]
        {
            "~/downloads/dora-files".to_string()
        }
        #[cfg(not(target_os = "macos"))]
        {
            "~/downloads".to_string()
        }
    })
});

/// Temporary files directory for processing (clips, cuts, exports, etc.)
/// Read from TEMP_FILES_DIR environment variable
/// Defaults to /tmp on production, supports tilde (~) expansion
/// Set to /telegram-bot-api on Railway for persistent storage
pub static TEMP_FILES_DIR: Lazy<String> =
    Lazy::new(|| env::var("TEMP_FILES_DIR").unwrap_or_else(|_| "/tmp".to_string()));

/// Database file path
/// Read from DATABASE_PATH environment variable
/// Default: database.sqlite
pub static DATABASE_PATH: Lazy<String> =
    Lazy::new(|| env::var("DATABASE_PATH").unwrap_or_else(|_| "database.sqlite".to_string()));

/// Log file path
/// Read from LOG_FILE_PATH environment variable
/// Default: app.log
pub static LOG_FILE_PATH: Lazy<String> =
    Lazy::new(|| env::var("LOG_FILE_PATH").unwrap_or_else(|_| "app.log".to_string()));

/// Bot token
/// Read from BOT_TOKEN or TELOXIDE_TOKEN environment variable
pub static BOT_TOKEN: Lazy<String> = Lazy::new(|| {
    env::var("BOT_TOKEN")
        .or_else(|_| env::var("TELOXIDE_TOKEN"))
        .unwrap_or_else(|_| String::new())
});

/// Webhook URL for Telegram updates
/// Read from WEBHOOK_URL environment variable
pub static WEBHOOK_URL: Lazy<Option<String>> = Lazy::new(|| env::var("WEBHOOK_URL").ok());

/// Rate limiting configuration
pub mod rate_limit {
    use super::Duration;

    /// Duration between downloads per user (in seconds)
    pub const COOLDOWN_SECONDS: u64 = 30;

    /// Rate limit duration
    pub fn duration() -> Duration {
        Duration::from_secs(COOLDOWN_SECONDS)
    }
}

/// Queue processing configuration
pub mod queue {
    use super::Duration;

    /// Maximum number of concurrent downloads
    /// Reduced to 2 to avoid YouTube 403 rate limiting
    pub const MAX_CONCURRENT_DOWNLOADS: usize = 2;

    /// Global delay between starting new download tasks (milliseconds)
    /// Helps avoid rate limiting when multiple users download simultaneously
    pub const INTER_DOWNLOAD_DELAY_MS: u64 = 3000;

    /// Interval between queue checks (in milliseconds)
    pub const CHECK_INTERVAL_MS: u64 = 100;

    /// Queue check interval duration
    pub fn check_interval() -> Duration {
        Duration::from_millis(CHECK_INTERVAL_MS)
    }

    /// Inter-download delay duration
    pub fn inter_download_delay() -> Duration {
        Duration::from_millis(INTER_DOWNLOAD_DELAY_MS)
    }
}

/// Download configuration
pub mod download {
    use super::Duration;

    /// Delay before cleaning up downloaded files (in seconds)
    pub const FILE_CLEANUP_DELAY_SECS: u64 = 600; // 10 minutes

    /// Timeout for yt-dlp commands (in seconds)
    pub const YTDLP_TIMEOUT_SECS: u64 = 240; // 4 minutes, to avoid timeouts on slow metadata fetches

    /// File cleanup delay duration
    pub fn cleanup_delay() -> Duration {
        Duration::from_secs(FILE_CLEANUP_DELAY_SECS)
    }

    /// yt-dlp command timeout duration
    pub fn ytdlp_timeout() -> Duration {
        Duration::from_secs(YTDLP_TIMEOUT_SECS)
    }
}

/// Retry configuration
pub mod retry {
    use super::Duration;

    /// Maximum number of retry attempts for sending files (disabled - only 1 attempt)
    pub const MAX_ATTEMPTS: u32 = 1;

    /// Delay between retry attempts (in seconds)
    pub const RETRY_DELAY_SECS: u64 = 10;

    /// Retry delay duration
    pub fn delay() -> Duration {
        Duration::from_secs(RETRY_DELAY_SECS)
    }

    /// Maximum number of retries for dispatcher reconnection
    pub const MAX_DISPATCHER_RETRIES: u32 = 5;

    /// Delay between dispatcher retry attempts (in seconds)
    pub const DISPATCHER_RETRY_DELAY_SECS: u64 = 5;

    /// Dispatcher retry delay duration
    pub fn dispatcher_delay() -> Duration {
        Duration::from_secs(DISPATCHER_RETRY_DELAY_SECS)
    }

    /// Base for exponential backoff calculation
    pub const EXPONENTIAL_BACKOFF_BASE: u64 = 2;
}

/// Animation configuration
pub mod animation {
    use super::Duration;

    /// Interval between animation frame updates (in milliseconds)
    pub const UPDATE_INTERVAL_MS: u64 = 500;

    /// Delay before stopping animation after completion (in milliseconds)
    pub const STOP_DELAY_MS: u64 = 50;

    /// Animation update interval duration
    pub fn update_interval() -> Duration {
        Duration::from_millis(UPDATE_INTERVAL_MS)
    }

    /// Animation stop delay duration
    pub fn stop_delay() -> Duration {
        Duration::from_millis(STOP_DELAY_MS)
    }
}

/// Network configuration
pub mod network {
    use super::Duration;

    /// Request timeout for HTTP requests (in seconds)
    /// Increased to 15 minutes for large file uploads (especially videos via local Bot API)
    pub const REQUEST_TIMEOUT_SECS: u64 = 900; // 15 minutes

    /// Request timeout duration
    pub fn timeout() -> Duration {
        Duration::from_secs(REQUEST_TIMEOUT_SECS)
    }
}

/// Downsub gRPC configuration
pub static DOWNSUB_GRPC_ENDPOINT: Lazy<Option<String>> = Lazy::new(|| {
    env::var("DOWNSUB_GRPC_ENDPOINT").ok().and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
});

pub mod downsub {
    use super::Duration;

    /// Default timeout for Downsub gRPC requests (seconds)
    pub const TIMEOUT_SECS: u64 = 10;

    pub fn timeout() -> Duration {
        Duration::from_secs(TIMEOUT_SECS)
    }
}

/// Progress message configuration
pub mod progress {
    /// Delay before clearing success message (in seconds)
    pub const CLEAR_DELAY_SECS: u64 = 10;
}

/// Admin configuration
pub mod admin {
    use once_cell::sync::Lazy;
    use std::env;

    fn parse_admin_ids(raw: &str) -> Vec<i64> {
        raw.split([',', ' ', '\n', '\t'])
            .filter_map(|part| part.trim().parse::<i64>().ok())
            .collect()
    }

    /// Admin user IDs (comma-separated)
    /// Read from ADMIN_IDS environment variable
    pub static ADMIN_IDS: Lazy<Vec<i64>> = Lazy::new(|| {
        env::var("ADMIN_IDS")
            .ok()
            .map(|raw| parse_admin_ids(&raw))
            .unwrap_or_default()
    });

    /// Admin username for notifications
    /// Read from ADMIN_USERNAME environment variable
    /// Defaults to empty string if not set (no admin access)
    pub static ADMIN_USERNAME: Lazy<String> =
        Lazy::new(|| env::var("ADMIN_USERNAME").unwrap_or_else(|_| String::new()));

    /// Admin user ID for direct messages (feedback, notifications)
    /// Read from ADMIN_USER_ID or fallback to first ADMIN_IDS entry
    /// Defaults to 0 if not set (no admin notifications)
    pub static ADMIN_USER_ID: Lazy<i64> = Lazy::new(|| {
        env::var("ADMIN_USER_ID")
            .ok()
            .and_then(|s| s.parse().ok())
            .or_else(|| ADMIN_IDS.first().copied())
            .unwrap_or(0)
    });

    /// Maximum retry attempts for failed tasks before giving up
    pub const MAX_TASK_RETRIES: i32 = 5;
}

/// Subscription pricing configuration
pub mod subscription {
    use once_cell::sync::Lazy;
    use std::env;

    /// Price for Premium subscription in Telegram Stars (charged every 30 days)
    /// Read from PREMIUM_PRICE_STARS environment variable
    /// Default: 350 Stars (~$6/month)
    pub static PREMIUM_PRICE_STARS: Lazy<u32> = Lazy::new(|| {
        env::var("PREMIUM_PRICE_STARS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(350)
    });

    /// Price for VIP subscription in Telegram Stars (charged every 30 days)
    /// Read from VIP_PRICE_STARS environment variable
    /// Default: 850 Stars (~$15/month)
    pub static VIP_PRICE_STARS: Lazy<u32> = Lazy::new(|| {
        env::var("VIP_PRICE_STARS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(850)
    });

    /// Subscription period in seconds (30 days)
    pub const SUBSCRIPTION_PERIOD_SECONDS: u32 = 2592000; // 30 days
}

/// Metrics and monitoring configuration
pub mod metrics {
    use once_cell::sync::Lazy;
    use std::env;

    /// Enable metrics collection and HTTP server
    /// Read from METRICS_ENABLED environment variable
    /// Default: true
    pub static ENABLED: Lazy<bool> = Lazy::new(|| {
        env::var("METRICS_ENABLED")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(true)
    });

    /// Port for metrics HTTP server
    /// Read from METRICS_PORT environment variable
    /// Default: 9090
    pub static PORT: Lazy<u16> = Lazy::new(|| {
        env::var("METRICS_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(9090)
    });

    /// Prometheus URL (for documentation/reference)
    /// Read from PROMETHEUS_URL environment variable
    pub static PROMETHEUS_URL: Lazy<String> =
        Lazy::new(|| env::var("PROMETHEUS_URL").unwrap_or_else(|_| "http://prometheus:9090".to_string()));
}

/// Alert configuration
pub mod alerts {
    use once_cell::sync::Lazy;
    use std::env;

    /// Enable alerting system
    /// Read from ALERTS_ENABLED environment variable
    /// Default: true
    pub static ENABLED: Lazy<bool> = Lazy::new(|| {
        env::var("ALERTS_ENABLED")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(true)
    });

    /// Error rate threshold percentage for triggering alerts
    /// Read from ALERT_ERROR_RATE_THRESHOLD environment variable
    /// Default: 5.0%
    pub static ERROR_RATE_THRESHOLD: Lazy<f64> = Lazy::new(|| {
        env::var("ALERT_ERROR_RATE_THRESHOLD")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(5.0)
    });

    /// Queue depth threshold for triggering alerts
    /// Read from ALERT_QUEUE_DEPTH_THRESHOLD environment variable
    /// Default: 50 tasks
    pub static QUEUE_DEPTH_THRESHOLD: Lazy<usize> = Lazy::new(|| {
        env::var("ALERT_QUEUE_DEPTH_THRESHOLD")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(50)
    });

    /// Retry rate threshold percentage for triggering alerts
    /// Read from ALERT_RETRY_RATE_THRESHOLD environment variable
    /// Default: 30.0%
    pub static RETRY_RATE_THRESHOLD: Lazy<f64> = Lazy::new(|| {
        env::var("ALERT_RETRY_RATE_THRESHOLD")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(30.0)
    });
}

/// Analytics cache configuration
pub mod analytics {
    use once_cell::sync::Lazy;
    use std::env;

    /// Update interval for analytics cache in seconds
    /// Read from ANALYTICS_CACHE_UPDATE_INTERVAL environment variable
    /// Default: 300 seconds (5 minutes)
    pub static CACHE_UPDATE_INTERVAL_SECS: Lazy<u64> = Lazy::new(|| {
        env::var("ANALYTICS_CACHE_UPDATE_INTERVAL")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(300)
    });
}

/// Proxy configuration for yt-dlp downloads
pub mod proxy {
    use once_cell::sync::Lazy;
    use std::env;

    /// Primary WARP proxy URL (Cloudflare WARP for free YouTube access)
    /// Read from WARP_PROXY environment variable
    /// Example: socks5://your-vps-ip:1080
    pub static WARP_PROXY: Lazy<Option<String>> = Lazy::new(|| {
        env::var("WARP_PROXY")
            .ok()
            .and_then(|s| if s.trim().is_empty() { None } else { Some(s) })
    });

    /// Path to file containing proxy list (one proxy per line)
    /// Read from PROXY_FILE environment variable
    /// Useful for managing large proxy lists
    pub static PROXY_FILE: Lazy<Option<String>> = Lazy::new(|| {
        env::var("PROXY_FILE")
            .ok()
            .and_then(|s| if s.trim().is_empty() { None } else { Some(s) })
    });

    /// Proxy selection strategy: "round_robin", "random", "weighted", "fixed"
    /// Read from PROXY_STRATEGY environment variable
    /// Default: "round_robin"
    pub static PROXY_STRATEGY: Lazy<String> = Lazy::new(|| {
        env::var("PROXY_STRATEGY")
            .unwrap_or_else(|_| "round_robin".to_string())
            .to_lowercase()
    });

    /// Enable proxy rotation (use different proxy for each download)
    /// Read from PROXY_ROTATION_ENABLED environment variable
    /// Default: true
    pub static ROTATION_ENABLED: Lazy<bool> = Lazy::new(|| {
        env::var("PROXY_ROTATION_ENABLED")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(true)
    });

    /// Minimum proxy health score (0.0 - 1.0) to use proxy
    /// Proxies with lower success rate are skipped
    /// Read from PROXY_MIN_HEALTH environment variable
    /// Default: 0.5 (50% success rate)
    pub static MIN_HEALTH: Lazy<f64> = Lazy::new(|| {
        let value: f64 = env::var("PROXY_MIN_HEALTH")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.5);
        value.clamp(0.0, 1.0)
    });

    /// URL to fetch proxy list from (useful for dynamic proxy updates)
    /// Read from PROXY_URL environment variable
    /// Default: empty (disabled)
    pub static PROXY_UPDATE_URL: Lazy<Option<String>> = Lazy::new(|| {
        env::var("PROXY_UPDATE_URL")
            .ok()
            .and_then(|s| if s.trim().is_empty() { None } else { Some(s) })
    });

    /// Interval to fetch proxy list from URL (in seconds)
    /// Read from PROXY_UPDATE_INTERVAL environment variable
    /// Default: 3600 (1 hour)
    pub static PROXY_UPDATE_INTERVAL_SECS: Lazy<u64> = Lazy::new(|| {
        env::var("PROXY_UPDATE_INTERVAL")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3600)
    });

    /// Gets the proxy selection strategy from environment configuration
    pub fn get_selection_strategy() -> crate::download::proxy::ProxySelectionStrategy {
        use crate::download::proxy::ProxySelectionStrategy;
        match PROXY_STRATEGY.as_str() {
            "random" => ProxySelectionStrategy::Random,
            "weighted" => ProxySelectionStrategy::Weighted,
            "fixed" => ProxySelectionStrategy::Fixed,
            _ => ProxySelectionStrategy::RoundRobin,
        }
    }
}

/// Validation configuration
pub mod validation {
    /// Maximum URL length (RFC 7230 recommends 8000, but we use 2048 for safety)
    pub const MAX_URL_LENGTH: usize = 2048;

    /// Maximum file size for Telegram (50MB in bytes)
    /// Telegram Bot API allows up to 50MB for files
    pub const MAX_FILE_SIZE_BYTES: u64 = 50 * 1024 * 1024; // 50 MB

    /// Maximum file size for audio files
    ///
    /// Standard Telegram Bot API (api.telegram.org): 50 MB
    /// Local Bot API Server: up to 5 GB (see https://core.telegram.org/bots/api#using-a-local-bot-api-server)
    ///
    /// Check if local Bot API server is used via BOT_API_URL environment variable.
    /// If BOT_API_URL is set and not pointing to api.telegram.org, assume local server is used.
    pub fn max_audio_size_bytes() -> u64 {
        // Check if local Bot API server is configured
        if let Ok(bot_api_url) = std::env::var("BOT_API_URL") {
            if !bot_api_url.contains("api.telegram.org") {
                // Local Bot API server allows larger files - using 5 GB limit
                log::info!(
                    "Local Bot API server detected (BOT_API_URL={}), using 5 GB limit for audio",
                    bot_api_url
                );
                return 5 * 1024 * 1024 * 1024; // 5 GB for local server
            }
        }

        // Default: 50 MB for standard API
        50 * 1024 * 1024 // 50 MB
    }

    /// Legacy constant for backward compatibility
    /// Use max_audio_size_bytes() instead for dynamic limit detection
    pub const MAX_AUDIO_SIZE_BYTES: u64 = 50 * 1024 * 1024; // 50 MB

    /// Maximum file size for video files
    ///
    /// Standard Telegram Bot API (api.telegram.org): 50 MB
    /// Local Bot API Server: up to 5 GB (see https://core.telegram.org/bots/api#using-a-local-bot-api-server)
    ///
    /// Check if local Bot API server is used via BOT_API_URL environment variable.
    /// If BOT_API_URL is set and not pointing to api.telegram.org, assume local server is used.
    pub fn max_video_size_bytes() -> u64 {
        // Check if local Bot API server is configured
        if let Ok(bot_api_url) = std::env::var("BOT_API_URL") {
            if !bot_api_url.contains("api.telegram.org") {
                // Local Bot API server allows larger files - using 5 GB limit
                log::info!(
                    "Local Bot API server detected (BOT_API_URL={}), using 5 GB limit",
                    bot_api_url
                );
                return 5 * 1024 * 1024 * 1024; // 5 GB for local server
            }
        }

        // Default: 50 MB for standard API
        50 * 1024 * 1024 // 50 MB
    }

    /// Legacy constant for backward compatibility
    /// Use max_video_size_bytes() instead for dynamic limit detection
    pub const MAX_VIDEO_SIZE_BYTES: u64 = 50 * 1024 * 1024; // 50 MB
}

/// Bot API server configuration utilities
///
/// Provides functions to check if local Bot API server is being used
/// and retrieve the Bot API URL.
pub mod bot_api {
    /// Returns the BOT_API_URL environment variable if set.
    pub fn get_url() -> Option<String> {
        std::env::var("BOT_API_URL").ok()
    }

    /// Returns true if using a local Bot API server (not api.telegram.org).
    ///
    /// Checks if BOT_API_URL is set and doesn't point to api.telegram.org.
    pub fn is_local() -> bool {
        get_url().map(|url| !url.contains("api.telegram.org")).unwrap_or(false)
    }

    /// Returns the local Bot API URL if using local server, None otherwise.
    ///
    /// This is useful when you need the URL only if it's a local server.
    pub fn local_url() -> Option<String> {
        get_url().filter(|url| !url.contains("api.telegram.org"))
    }

    /// Checks if the given URL string points to a local Bot API server.
    ///
    /// Returns true if the URL doesn't contain "api.telegram.org".
    pub fn is_local_url(url: &str) -> bool {
        !url.contains("api.telegram.org")
    }
}
