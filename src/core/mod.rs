//! Core utilities, configuration, and common functionality

pub mod alerts;
pub mod config;
pub mod error;
pub mod error_logger;
pub mod export;
pub mod history;
pub mod logging;
pub mod metrics;
pub mod metrics_server;
pub mod rate_limiter;
pub mod retry;
pub mod stats;
pub mod stats_reporter;
pub mod subscription;
pub mod utils;

// Re-exports for convenience
pub use config::*;
pub use error::BotError;
pub use logging::{init_logger, log_cookies_configuration};
