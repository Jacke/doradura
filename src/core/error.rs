use thiserror::Error;

/// Centralized error types for the application
///
/// All errors in the application are converted to this enum for consistent error handling.
/// Uses `thiserror` for automatic error conversion and display formatting.
///
/// # Example
///
/// ```no_run
/// use doradura::error::AppError;
///
/// fn handle_error(err: AppError) {
///     eprintln!("Error: {}", err);
/// }
/// ```
#[derive(Error, Debug)]
pub enum AppError {
    /// Database-related errors
    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),

    /// Database connection pool errors
    #[error("Database pool error: {0}")]
    DatabasePool(#[from] r2d2::Error),

    /// Telegram API errors
    #[error("Telegram error: {0}")]
    Telegram(#[from] teloxide::RequestError),

    /// Download/yt-dlp errors
    #[error("Download error: {0}")]
    Download(String),

    /// HTTP/Fetch errors
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    /// HTTP status code errors
    #[error("HTTP request failed with status: {0}")]
    HttpStatus(reqwest::StatusCode),

    /// IO errors
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// URL parsing errors
    #[error("URL parsing error: {0}")]
    Url(#[from] url::ParseError),

    /// Anyhow errors (for general error handling)
    #[error("Application error: {0}")]
    Anyhow(#[from] anyhow::Error),

    /// Validation errors
    #[error("Validation error: {0}")]
    Validation(String),
}

/// Type alias for Result with AppError
pub type AppResult<T> = Result<T, AppError>;

/// Type alias for backward compatibility
pub type BotError = AppError;

/// Helper function to convert String to AppError::Download
impl From<String> for AppError {
    fn from(err: String) -> Self {
        AppError::Download(err)
    }
}

/// Helper function to convert &str to AppError::Download
impl From<&str> for AppError {
    fn from(err: &str) -> Self {
        AppError::Download(err.to_string())
    }
}
