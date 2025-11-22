use std::time::Duration;
use once_cell::sync::Lazy;
use std::env;

/// Configuration constants for the bot

/// Cached yt-dlp binary path
/// Read once at startup from YTDL_BIN environment variable or defaults to "yt-dlp"
pub static YTDL_BIN: Lazy<String> = Lazy::new(|| {
    env::var("YTDL_BIN").unwrap_or_else(|_| "yt-dlp".to_string())
});

/// Browser to extract cookies from for YouTube authentication
/// Read from YTDL_COOKIES_BROWSER environment variable
/// Supported: chrome, firefox, safari, brave, chromium, edge, opera, vivaldi
/// Set to empty string to disable cookie extraction
/// 
/// NOTE: On macOS, browser cookie extraction requires Full Disk Access permission
/// It's recommended to use YTDL_COOKIES_FILE instead (see MACOS_COOKIES_FIX.md)
/// 
/// Default: empty (use YTDL_COOKIES_FILE for macOS)
pub static YTDL_COOKIES_BROWSER: Lazy<String> = Lazy::new(|| {
    env::var("YTDL_COOKIES_BROWSER").unwrap_or_else(|_| String::new())
});

/// Path to cookies file for YouTube authentication
/// Read from YTDL_COOKIES_FILE environment variable
/// If set, this takes priority over YTDL_COOKIES_BROWSER
/// Example: youtube_cookies.txt
pub static YTDL_COOKIES_FILE: Lazy<Option<String>> = Lazy::new(|| {
    env::var("YTDL_COOKIES_FILE").ok()
});

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
    pub const MAX_CONCURRENT_DOWNLOADS: usize = 5;
    
    /// Interval between queue checks (in milliseconds)
    pub const CHECK_INTERVAL_MS: u64 = 100;
    
    /// Queue check interval duration
    pub fn check_interval() -> Duration {
        Duration::from_millis(CHECK_INTERVAL_MS)
    }
}

/// Download configuration
pub mod download {
    use super::Duration;
    
    /// Delay before cleaning up downloaded files (in seconds)
    pub const FILE_CLEANUP_DELAY_SECS: u64 = 600; // 10 minutes
    
    /// Timeout for yt-dlp commands (in seconds)
    pub const YTDLP_TIMEOUT_SECS: u64 = 120; // 2 minutes
    
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
    
    /// Maximum number of retry attempts for sending files
    pub const MAX_ATTEMPTS: u32 = 3;
    
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

/// Progress message configuration
pub mod progress {
    /// Delay before clearing success message (in seconds)
    pub const CLEAR_DELAY_SECS: u64 = 10;
}

/// Admin configuration
pub mod admin {
    /// Admin username for notifications
    pub const ADMIN_USERNAME: &str = "stansob";
    
    /// Maximum retry attempts for failed tasks before giving up
    pub const MAX_TASK_RETRIES: i32 = 5;
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
                log::info!("Local Bot API server detected (BOT_API_URL={}), using 5 GB limit for audio", bot_api_url);
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
                log::info!("Local Bot API server detected (BOT_API_URL={}), using 5 GB limit", bot_api_url);
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

