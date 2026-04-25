//! Error logging module
//!
//! Provides centralized error logging with user context.
//! Errors are stored in the database for monitoring and reporting.

use crate::storage::SharedStorage;
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
}

/// Error logger that stores errors in the database
#[derive(Clone)]
pub struct ErrorLogger {
    shared_storage: Arc<SharedStorage>,
}

impl ErrorLogger {
    /// Creates a new error logger
    pub fn new(shared_storage: Arc<SharedStorage>) -> Self {
        Self { shared_storage }
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
        let shared_storage = Arc::clone(&self.shared_storage);
        let username = user.username.clone();
        let error_type_str = error_type.as_str().to_string();
        let error_message = error_message.to_string();
        let url = url.map(str::to_string);
        let context = context.map(str::to_string);
        let user_id = user.user_id;

        // Use Handle::try_current to avoid panic if called outside tokio runtime
        let handle = match tokio::runtime::Handle::try_current() {
            Ok(h) => h,
            Err(_) => {
                log::debug!("ErrorLogger: no tokio runtime, skipping async error log");
                return;
            }
        };
        handle.spawn(async move {
            match shared_storage
                .log_error(
                    user_id,
                    username.as_deref(),
                    &error_type_str,
                    &error_message,
                    url.as_deref(),
                    context.as_deref(),
                )
                .await
            {
                Err(e) => {
                    log::error!("Failed to log error to database: {}", e);
                }
                _ => {
                    log::debug!(
                        "Logged error: type={}, user={:?}, message={}",
                        error_type_str,
                        user_id,
                        error_message
                    );
                }
            }
        });
    }
}

/// Global error logger instance (initialized at startup)
static ERROR_LOGGER: std::sync::OnceLock<ErrorLogger> = std::sync::OnceLock::new();

/// Initializes the global error logger
pub fn init_error_logger(shared_storage: Arc<SharedStorage>) {
    let _ = ERROR_LOGGER.set(ErrorLogger::new(shared_storage));
    log::info!("Error logger initialized");
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
