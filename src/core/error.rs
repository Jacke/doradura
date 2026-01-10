use crate::core::metrics;
use thiserror::Error;

/// Centralized error types for the application
///
/// All errors in the application are converted to this enum for consistent error handling.
/// Uses `thiserror` for automatic error conversion and display formatting.
///
/// # Example
///
/// ```no_run
/// use doradura::core::error::AppError;
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

    /// Audio effects processing errors
    #[error("Audio effect error: {0}")]
    AudioEffect(#[from] crate::download::audio_effects::AudioEffectError),
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

impl AppError {
    /// Returns the error category for metrics tracking
    ///
    /// Categorizes errors into types for monitoring and alerting.
    pub fn category(&self) -> &'static str {
        match self {
            AppError::Database(_) | AppError::DatabasePool(_) => "database",
            AppError::Telegram(_) => "telegram_api",
            AppError::Download(_) => "download",
            AppError::Http(_) | AppError::HttpStatus(_) => "http",
            AppError::Io(_) => "io",
            AppError::Url(_) => "url_parsing",
            AppError::Validation(_) => "validation",
            AppError::AudioEffect(_) => "audio_effect",
            AppError::Anyhow(_) => "other",
        }
    }

    /// Tracks this error in metrics
    ///
    /// Increments the error counter for this error category.
    /// Should be called when errors occur to maintain accurate error metrics.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use doradura::core::error::AppError;
    ///
    /// fn process_download() -> Result<(), AppError> {
    ///     // ... download logic ...
    ///     Err(AppError::Download("Failed to download".to_string()))
    /// }
    ///
    /// match process_download() {
    ///     Ok(_) => println!("Success"),
    ///     Err(e) => {
    ///         e.track(); // Track error in metrics
    ///         eprintln!("Error: {}", e);
    ///     }
    /// }
    /// ```
    pub fn track(&self) {
        self.track_with_operation("unknown");
    }

    /// Tracks this error in metrics with a specific operation label
    pub fn track_with_operation(&self, operation: &str) {
        let category = self.category();
        metrics::record_error(category, operation);
        log::debug!(
            "Error tracked in metrics: category={}, operation={}, error={}",
            category,
            operation,
            self
        );
    }

    /// Tracks this error and returns self for chaining
    ///
    /// Convenience method that tracks the error and returns it,
    /// allowing for error tracking in Result chains.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use doradura::core::error::AppError;
    ///
    /// fn process() -> Result<(), AppError> {
    ///     some_operation().map_err(|e| e.track_and_return())?;
    ///     Ok(())
    /// }
    /// # fn some_operation() -> Result<(), AppError> { Ok(()) }
    /// ```
    pub fn track_and_return(self) -> Self {
        self.track();
        self
    }

    /// Tracks this error with an operation label and returns self for chaining
    pub fn track_and_return_with_operation(self, operation: &str) -> Self {
        self.track_with_operation(operation);
        self
    }
}
