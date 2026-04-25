//! Core utilities, configuration, and common functionality

pub mod categorizer;
pub mod config;
pub mod copyright;
pub mod disk;
pub mod error;
pub mod error_logger;
pub mod llm;
pub mod logging;
pub mod metrics;
pub mod metrics_server;
pub mod odesli;
pub mod process;
pub mod share;
pub mod types;
pub mod utils;
pub mod validation;
pub mod web;

// Re-exports for convenience
pub use config::*;
pub use error::BotError;
pub use logging::{init_logger, log_cookies_configuration};
pub use types::{Plan, PlanChangeEvent, PlanChangeNotifier, PlanChangeReason, PlanChangeReceiver, plan_change_channel};
pub use utils::{
    BOT_API_RESPONSE_REGEX, BOT_API_START_REGEX, BOT_API_START_SIMPLE_REGEX, RETRY_AFTER_ALT_REGEX, RETRY_AFTER_REGEX,
    TELEGRAM_MESSAGE_LIMIT, TempDirGuard, escape_markdown_v2, extract_retry_after, format_bytes, format_bytes_i64,
    format_media_duration, format_media_duration_i64, format_uptime, is_timeout_or_network_error,
    truncate_for_telegram, truncate_string_safe, truncate_tail_utf8,
};

/// Alias for backward compatibility - use escape_markdown_v2
pub use utils::escape_markdown_v2 as escape_markdown;
pub use utils::escape_markdown_v2_url as escape_markdown_url;
