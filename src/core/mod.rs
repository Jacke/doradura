//! Core utilities, configuration, and common functionality

pub mod alerts;
pub mod config;
pub mod copyright;
pub mod disk;
pub mod error;
pub mod error_logger;
pub mod export;
pub mod history;
pub mod logging;
pub mod metrics;
pub mod metrics_server;
pub mod process;
pub mod rate_limiter;
pub mod retry;
pub mod stats;
pub mod stats_reporter;
pub mod subscription;
pub mod types;
pub mod utils;
pub mod validation;

// Re-exports for convenience
pub use config::*;
pub use error::BotError;
pub use logging::{init_logger, log_cookies_configuration};
pub use types::Plan;
pub use utils::{
    escape_markdown_v2, extract_retry_after, is_timeout_or_network_error, truncate_for_telegram, truncate_string_safe,
    truncate_tail_utf8, BOT_API_RESPONSE_REGEX, BOT_API_START_REGEX, BOT_API_START_SIMPLE_REGEX, RETRY_AFTER_ALT_REGEX,
    RETRY_AFTER_REGEX, TELEGRAM_MESSAGE_LIMIT,
};

/// Alias for backward compatibility - use escape_markdown_v2
pub use utils::escape_markdown_v2 as escape_markdown;
