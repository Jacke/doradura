//! Error logging module
//!
//! Provides centralized error logging with user context.
//! Errors are stored in the database for monitoring and reporting.

use crate::storage::db::{self, DbPool};
use std::sync::Arc;

/// Error types for categorization
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorType {
    /// Download failed (yt-dlp, network, etc.)
    DownloadFailed,
    /// File too large for Telegram
    FileTooLarge,
    /// MTProto error (connection, auth, etc.)
    MtProtoError,
    /// File reference expired
    FileReferenceExpired,
    /// Telegram API error
    TelegramApiError,
    /// FFmpeg processing error
    FfmpegError,
    /// Rate limit exceeded
    RateLimitExceeded,
    /// Invalid URL or unsupported platform
    InvalidUrl,
    /// Timeout
    Timeout,
    /// Permission denied
    PermissionDenied,
    /// Other/unknown error
    Other,
}

impl ErrorType {
    /// Returns the string identifier for database storage
    pub fn as_str(&self) -> &'static str {
        match self {
            ErrorType::DownloadFailed => "download_failed",
            ErrorType::FileTooLarge => "file_too_large",
            ErrorType::MtProtoError => "mtproto_error",
            ErrorType::FileReferenceExpired => "file_reference_expired",
            ErrorType::TelegramApiError => "telegram_api_error",
            ErrorType::FfmpegError => "ffmpeg_error",
            ErrorType::RateLimitExceeded => "rate_limit_exceeded",
            ErrorType::InvalidUrl => "invalid_url",
            ErrorType::Timeout => "timeout",
            ErrorType::PermissionDenied => "permission_denied",
            ErrorType::Other => "other",
        }
    }

    /// Returns emoji for the error type
    pub fn emoji(&self) -> &'static str {
        match self {
            ErrorType::DownloadFailed => "📥",
            ErrorType::FileTooLarge => "📦",
            ErrorType::MtProtoError => "🔌",
            ErrorType::FileReferenceExpired => "⏰",
            ErrorType::TelegramApiError => "📱",
            ErrorType::FfmpegError => "🎬",
            ErrorType::RateLimitExceeded => "🚦",
            ErrorType::InvalidUrl => "🔗",
            ErrorType::Timeout => "⏱️",
            ErrorType::PermissionDenied => "🔒",
            ErrorType::Other => "❓",
        }
    }
}

/// User context for error logging
#[derive(Debug, Clone, Default)]
pub struct UserContext {
    pub user_id: Option<i64>,
    pub username: Option<String>,
}

impl UserContext {
    pub fn new(user_id: i64, username: Option<String>) -> Self {
        Self {
            user_id: Some(user_id),
            username,
        }
    }

    pub fn anonymous() -> Self {
        Self::default()
    }
}

/// Error logger that stores errors in the database
#[derive(Clone)]
pub struct ErrorLogger {
    db_pool: Arc<DbPool>,
}

impl ErrorLogger {
    /// Creates a new error logger
    pub fn new(db_pool: Arc<DbPool>) -> Self {
        Self { db_pool }
    }

    /// Logs an error to the database
    pub fn log(
        &self,
        error_type: ErrorType,
        error_message: &str,
        user: &UserContext,
        url: Option<&str>,
        context: Option<&str>,
    ) {
        let conn = match db::get_connection(&self.db_pool) {
            Ok(c) => c,
            Err(e) => {
                log::error!("Failed to get DB connection for error logging: {}", e);
                return;
            }
        };

        if let Err(e) = db::log_error(
            &conn,
            user.user_id,
            user.username.as_deref(),
            error_type.as_str(),
            error_message,
            url,
            context,
        ) {
            log::error!("Failed to log error to database: {}", e);
        } else {
            log::debug!(
                "Logged error: type={}, user={:?}, message={}",
                error_type.as_str(),
                user.user_id,
                error_message
            );
        }
    }

    /// Logs a download failure
    pub fn log_download_error(&self, user: &UserContext, url: &str, error_message: &str, format: Option<&str>) {
        let context = format.map(|f| format!(r#"{{"format":"{}"}}"#, f));
        self.log(
            ErrorType::DownloadFailed,
            error_message,
            user,
            Some(url),
            context.as_deref(),
        );
    }

    /// Logs a file too large error
    pub fn log_file_too_large(&self, user: &UserContext, url: Option<&str>, file_size: u64, max_size: u64) {
        let message = format!("File too large: {} bytes (max: {} bytes)", file_size, max_size);
        let context = format!(r#"{{"file_size":{},"max_size":{}}}"#, file_size, max_size);
        self.log(ErrorType::FileTooLarge, &message, user, url, Some(&context));
    }

    /// Logs an MTProto error
    pub fn log_mtproto_error(&self, user: &UserContext, error_message: &str, message_id: Option<i32>) {
        let context = message_id.map(|id| format!(r#"{{"message_id":{}}}"#, id));
        self.log(ErrorType::MtProtoError, error_message, user, None, context.as_deref());
    }

    /// Logs a Telegram API error
    pub fn log_telegram_error(&self, user: &UserContext, error_message: &str, file_id: Option<&str>) {
        let context = file_id.map(|id| format!(r#"{{"file_id":"{}"}}"#, id));
        self.log(
            ErrorType::TelegramApiError,
            error_message,
            user,
            None,
            context.as_deref(),
        );
    }

    /// Logs an FFmpeg error
    pub fn log_ffmpeg_error(&self, user: &UserContext, error_message: &str, operation: &str) {
        let context = format!(r#"{{"operation":"{}"}}"#, operation);
        self.log(ErrorType::FfmpegError, error_message, user, None, Some(&context));
    }

    /// Logs an invalid URL error
    pub fn log_invalid_url(&self, user: &UserContext, url: &str, reason: &str) {
        self.log(ErrorType::InvalidUrl, reason, user, Some(url), None);
    }

    /// Logs a timeout error
    pub fn log_timeout(&self, user: &UserContext, url: Option<&str>, operation: &str) {
        let message = format!("Operation timed out: {}", operation);
        self.log(ErrorType::Timeout, &message, user, url, None);
    }

    /// Logs a generic error
    pub fn log_other(&self, user: &UserContext, error_message: &str, url: Option<&str>) {
        self.log(ErrorType::Other, error_message, user, url, None);
    }
}

/// Global error logger instance (initialized at startup)
static ERROR_LOGGER: std::sync::OnceLock<ErrorLogger> = std::sync::OnceLock::new();

/// Initializes the global error logger
pub fn init_error_logger(db_pool: Arc<DbPool>) {
    let _ = ERROR_LOGGER.set(ErrorLogger::new(db_pool));
    log::info!("Error logger initialized");
}

/// Gets the global error logger (panics if not initialized)
pub fn get_error_logger() -> &'static ErrorLogger {
    ERROR_LOGGER.get().expect("Error logger not initialized")
}

/// Tries to get the global error logger (returns None if not initialized)
pub fn try_get_error_logger() -> Option<&'static ErrorLogger> {
    ERROR_LOGGER.get()
}

/// Convenience function to log an error (uses global logger)
pub fn log_error(
    error_type: ErrorType,
    error_message: &str,
    user: &UserContext,
    url: Option<&str>,
    context: Option<&str>,
) {
    if let Some(logger) = try_get_error_logger() {
        logger.log(error_type, error_message, user, url, context);
    } else {
        // Fallback to regular logging if error logger not initialized
        log::error!(
            "[ERROR_LOG] type={} user={:?} url={:?} message={}",
            error_type.as_str(),
            user.user_id,
            url,
            error_message
        );
    }
}
