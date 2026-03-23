//! Core utilities, configuration, and common functionality.
//!
//! Shared modules are re-exported from `doracore`. Bot-specific modules
//! (alerts, disk with AlertManager, subscriptions, stats, etc.) live here.

// ── Shared modules — provided by doracore ────────────────────────────────────
pub use doracore::core::categorizer;
pub use doracore::core::config;
pub use doracore::core::copyright;
pub use doracore::core::error;
pub use doracore::core::error_logger;
pub use doracore::core::logging;
pub use doracore::core::metrics;
pub use doracore::core::metrics_server;
pub use doracore::core::odesli;
pub use doracore::core::process;
pub use doracore::core::share;
pub use doracore::core::types;
pub use doracore::core::utils;
pub use doracore::core::validation;
pub use doracore::core::web;

pub use doracore::core::disk;

// ── Bot-only modules ──────────────────────────────────────────────────────────
pub mod alerts;
pub mod export;
pub mod history;
pub mod rate_limiter;
pub mod retry;
pub mod stats;
pub mod stats_reporter;
pub mod subscription;

// ── Re-exports for convenience (mirrors doracore::core) ──────────────────────
pub use config::*;
pub use error::BotError;
pub use logging::{init_logger, log_cookies_configuration};
pub use types::{plan_change_channel, Plan, PlanChangeEvent, PlanChangeNotifier, PlanChangeReason, PlanChangeReceiver};
pub use utils::{
    escape_markdown_v2, extract_retry_after, is_timeout_or_network_error, truncate_for_telegram, truncate_string_safe,
    truncate_tail_utf8, BOT_API_RESPONSE_REGEX, BOT_API_START_REGEX, BOT_API_START_SIMPLE_REGEX, RETRY_AFTER_ALT_REGEX,
    RETRY_AFTER_REGEX, TELEGRAM_MESSAGE_LIMIT,
};

/// Alias for backward compatibility — use `escape_markdown_v2`.
pub use utils::escape_markdown_v2 as escape_markdown;
pub use utils::escape_markdown_v2_url as escape_markdown_url;
