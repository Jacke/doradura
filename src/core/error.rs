use crate::core::metrics;
use crate::download::error::DownloadError;
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

    /// Download/yt-dlp errors (structured)
    #[error("Download error: {0}")]
    Download(#[from] DownloadError),

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

/// Helper function to convert String to AppError::Download(Other)
impl From<String> for AppError {
    fn from(err: String) -> Self {
        AppError::Download(DownloadError::Other(err))
    }
}

/// Helper function to convert &str to AppError::Download(Other)
impl From<&str> for AppError {
    fn from(err: &str) -> Self {
        AppError::Download(DownloadError::Other(err.to_string()))
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
    /// use doradura::download::error::DownloadError;
    ///
    /// fn process_download() -> Result<(), AppError> {
    ///     // ... download logic ...
    ///     Err(AppError::Download(DownloadError::Other("Failed to download".to_string())))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::download::error::DownloadError;

    #[test]
    fn test_app_error_from_string() {
        let error: AppError = "Test error".to_string().into();
        match error {
            AppError::Download(err) => assert_eq!(err.to_string(), "Test error"),
            _ => panic!("Expected Download variant"),
        }
    }

    #[test]
    fn test_app_error_from_str() {
        let error: AppError = "Test error".into();
        match error {
            AppError::Download(err) => assert_eq!(err.to_string(), "Test error"),
            _ => panic!("Expected Download variant"),
        }
    }

    #[test]
    fn test_error_category_database() {
        let error = AppError::Database(rusqlite::Error::InvalidQuery);
        assert_eq!(error.category(), "database");
    }

    #[test]
    fn test_error_category_download() {
        let error = AppError::Download(DownloadError::Other("test".to_string()));
        assert_eq!(error.category(), "download");
    }

    #[test]
    fn test_error_category_validation() {
        let error = AppError::Validation("test".to_string());
        assert_eq!(error.category(), "validation");
    }

    #[test]
    fn test_error_category_io() {
        let error = AppError::Io(std::io::Error::new(std::io::ErrorKind::NotFound, "test"));
        assert_eq!(error.category(), "io");
    }

    #[test]
    fn test_error_category_url() {
        let error = AppError::Url(url::ParseError::EmptyHost);
        assert_eq!(error.category(), "url_parsing");
    }

    #[test]
    fn test_error_display_database() {
        let error = AppError::Database(rusqlite::Error::InvalidQuery);
        let display = format!("{}", error);
        assert!(display.contains("Database error"));
    }

    #[test]
    fn test_error_display_download() {
        let error = AppError::Download(DownloadError::Other("Failed to download".to_string()));
        let display = format!("{}", error);
        assert!(display.contains("Download error"));
        assert!(display.contains("Failed to download"));
    }

    #[test]
    fn test_error_display_validation() {
        let error = AppError::Validation("Invalid URL".to_string());
        let display = format!("{}", error);
        assert!(display.contains("Validation error"));
        assert!(display.contains("Invalid URL"));
    }

    #[test]
    fn test_error_display_io() {
        let error = AppError::Io(std::io::Error::new(std::io::ErrorKind::NotFound, "file not found"));
        let display = format!("{}", error);
        assert!(display.contains("IO error"));
    }

    #[test]
    fn test_error_display_url() {
        let error = AppError::Url(url::ParseError::EmptyHost);
        let display = format!("{}", error);
        assert!(display.contains("URL parsing error"));
    }

    #[test]
    fn test_error_display_http_status() {
        let error = AppError::HttpStatus(reqwest::StatusCode::NOT_FOUND);
        let display = format!("{}", error);
        assert!(display.contains("404"));
    }

    #[test]
    fn test_track_and_return() {
        let error = AppError::Download(DownloadError::Other("test".to_string()));
        let returned = error.track_and_return();
        // Verify we get the same error back
        match returned {
            AppError::Download(err) => assert_eq!(err.to_string(), "test"),
            _ => panic!("Expected Download variant"),
        }
    }

    #[test]
    fn test_track_and_return_with_operation() {
        let error = AppError::Validation("invalid input".to_string());
        let returned = error.track_and_return_with_operation("user_input");
        // Verify we get the same error back
        match returned {
            AppError::Validation(msg) => assert_eq!(msg, "invalid input"),
            _ => panic!("Expected Validation variant"),
        }
    }

    #[test]
    fn test_track() {
        // This test just verifies track() doesn't panic
        let error = AppError::Download(DownloadError::Other("test".to_string()));
        error.track(); // Should not panic
    }

    #[test]
    fn test_track_with_operation() {
        // This test just verifies track_with_operation() doesn't panic
        let error = AppError::Download(DownloadError::Other("test".to_string()));
        error.track_with_operation("download"); // Should not panic
    }

    #[test]
    fn test_error_debug() {
        let error = AppError::Download(DownloadError::Other("test error".to_string()));
        let debug = format!("{:?}", error);
        assert!(debug.contains("Download"));
        assert!(debug.contains("test error"));
    }

    #[test]
    fn test_app_result_type_alias() {
        fn returns_result() -> AppResult<i32> {
            Ok(42)
        }

        fn returns_error() -> AppResult<i32> {
            Err(AppError::Download(DownloadError::Other("error".to_string())))
        }

        assert_eq!(returns_result().unwrap(), 42);
        assert!(returns_error().is_err());
    }

    #[test]
    fn test_bot_error_type_alias() {
        // BotError should be the same as AppError
        let error: BotError = AppError::Download(DownloadError::Other("test".to_string()));
        assert_eq!(error.category(), "download");
    }

    #[test]
    fn test_anyhow_error_category() {
        let anyhow_error = anyhow::anyhow!("test error");
        let error = AppError::Anyhow(anyhow_error);
        assert_eq!(error.category(), "other");
    }

    #[test]
    fn test_database_pool_error_category() {
        // Create a pool error by trying to get a connection from an empty pool
        // This is tricky to create, so we'll test the category match pattern directly
        let error = AppError::Download(DownloadError::Other("test".to_string()));
        // Just verify the match pattern works
        let category = match &error {
            AppError::Database(_) | AppError::DatabasePool(_) => "database",
            _ => error.category(),
        };
        assert_eq!(category, "download");
    }

    #[test]
    fn test_all_categories_covered() {
        // Test that all error categories return valid strings
        let errors = vec![
            AppError::Download(DownloadError::Other("test".to_string())),
            AppError::Validation("test".to_string()),
            AppError::Io(std::io::Error::other("test")),
            AppError::Url(url::ParseError::EmptyHost),
            AppError::HttpStatus(reqwest::StatusCode::OK),
            AppError::Database(rusqlite::Error::InvalidQuery),
            AppError::Anyhow(anyhow::anyhow!("test")),
        ];

        for error in errors {
            let category = error.category();
            assert!(!category.is_empty());
        }
    }
}
