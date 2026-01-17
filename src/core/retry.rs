//! Retry logic for failed operations with exponential backoff and user notifications.
//!
//! Provides configurable retry strategies for download operations with:
//! - Exponential backoff with jitter
//! - Max retry limits
//! - User notification on retry
//! - Different strategies for different error types

use crate::core::metrics;
use crate::telegram::Bot;
use std::future::Future;
use std::time::Duration;
use teloxide::prelude::*;
use thiserror::Error;

/// Retry-related errors.
#[derive(Debug, Error)]
pub enum RetryError<E> {
    /// All retries exhausted
    #[error("Max retries ({max_retries}) exhausted")]
    MaxRetriesExhausted { max_retries: u32, last_error: E },

    /// Operation was cancelled
    #[error("Operation cancelled")]
    Cancelled,
}

/// Retry strategy configuration.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retry attempts
    pub max_retries: u32,
    /// Initial delay before first retry
    pub initial_delay: Duration,
    /// Maximum delay between retries
    pub max_delay: Duration,
    /// Multiplier for exponential backoff
    pub backoff_multiplier: f64,
    /// Whether to add jitter to delays
    pub add_jitter: bool,
    /// Whether to notify user on retry
    pub notify_user: bool,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_delay: Duration::from_secs(2),
            max_delay: Duration::from_secs(60),
            backoff_multiplier: 2.0,
            add_jitter: true,
            notify_user: true,
        }
    }
}

impl RetryConfig {
    /// Creates a new retry config with default settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the maximum number of retries.
    #[must_use]
    pub fn max_retries(mut self, max: u32) -> Self {
        self.max_retries = max;
        self
    }

    /// Sets the initial delay.
    #[must_use]
    pub fn initial_delay(mut self, delay: Duration) -> Self {
        self.initial_delay = delay;
        self
    }

    /// Sets the maximum delay.
    #[must_use]
    pub fn max_delay(mut self, delay: Duration) -> Self {
        self.max_delay = delay;
        self
    }

    /// Sets the backoff multiplier.
    #[must_use]
    pub fn backoff_multiplier(mut self, multiplier: f64) -> Self {
        self.backoff_multiplier = multiplier;
        self
    }

    /// Disables jitter.
    #[must_use]
    pub fn no_jitter(mut self) -> Self {
        self.add_jitter = false;
        self
    }

    /// Disables user notifications.
    #[must_use]
    pub fn no_notify(mut self) -> Self {
        self.notify_user = false;
        self
    }

    /// Calculates delay for a given attempt number.
    pub fn delay_for_attempt(&self, attempt: u32) -> Duration {
        let base_delay = self.initial_delay.as_secs_f64() * self.backoff_multiplier.powi(attempt as i32);
        let capped_delay = base_delay.min(self.max_delay.as_secs_f64());

        let final_delay = if self.add_jitter {
            // Add up to 25% jitter
            let jitter = rand::random::<f64>() * 0.25 * capped_delay;
            capped_delay + jitter
        } else {
            capped_delay
        };

        Duration::from_secs_f64(final_delay)
    }
}

/// Predefined retry configs for different scenarios.
impl RetryConfig {
    /// Config for network errors (more retries, longer delays).
    pub fn network() -> Self {
        Self {
            max_retries: 5,
            initial_delay: Duration::from_secs(3),
            max_delay: Duration::from_secs(120),
            backoff_multiplier: 2.0,
            add_jitter: true,
            notify_user: true,
        }
    }

    /// Config for rate limiting (shorter delays, respect server hints).
    pub fn rate_limit() -> Self {
        Self {
            max_retries: 3,
            initial_delay: Duration::from_secs(5),
            max_delay: Duration::from_secs(30),
            backoff_multiplier: 1.5,
            add_jitter: false,
            notify_user: false,
        }
    }

    /// Config for quick retries (e.g., temporary failures).
    pub fn quick() -> Self {
        Self {
            max_retries: 2,
            initial_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(5),
            backoff_multiplier: 2.0,
            add_jitter: true,
            notify_user: false,
        }
    }

    /// Config for aggressive retries (many attempts, short delays).
    pub fn aggressive() -> Self {
        Self {
            max_retries: 10,
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(30),
            backoff_multiplier: 1.5,
            add_jitter: true,
            notify_user: true,
        }
    }
}

/// Result of a retry attempt.
#[derive(Debug)]
pub struct RetryResult<T, E> {
    /// The final result (success or last error)
    pub result: Result<T, RetryError<E>>,
    /// Number of attempts made
    pub attempts: u32,
    /// Total time spent retrying
    pub total_duration: Duration,
}

impl<T, E> RetryResult<T, E> {
    /// Returns true if the operation succeeded.
    pub fn is_ok(&self) -> bool {
        self.result.is_ok()
    }

    /// Returns true if all retries were exhausted.
    pub fn is_exhausted(&self) -> bool {
        matches!(self.result, Err(RetryError::MaxRetriesExhausted { .. }))
    }

    /// Unwraps the result, panicking on error.
    pub fn unwrap(self) -> T
    where
        E: std::fmt::Debug,
    {
        self.result.unwrap()
    }
}

/// Determines if an error is retryable.
pub trait Retryable {
    /// Returns true if the error should be retried.
    fn is_retryable(&self) -> bool;

    /// Returns an optional hint for retry delay (e.g., from rate limit headers).
    fn retry_after(&self) -> Option<Duration> {
        None
    }
}

// Implement Retryable for common error types
impl Retryable for teloxide::RequestError {
    fn is_retryable(&self) -> bool {
        match self {
            teloxide::RequestError::Network(_) => true,
            teloxide::RequestError::RetryAfter(_) => true,
            teloxide::RequestError::Api(api_error) => {
                // Retry on server errors (5xx equivalent)
                let error_str = format!("{:?}", api_error);
                error_str.contains("Bad Gateway")
                    || error_str.contains("Service Unavailable")
                    || error_str.contains("Gateway Timeout")
                    || error_str.contains("Too Many Requests")
            }
            _ => false,
        }
    }

    fn retry_after(&self) -> Option<Duration> {
        if let teloxide::RequestError::RetryAfter(seconds) = self {
            Some(seconds.duration())
        } else {
            None
        }
    }
}

impl Retryable for std::io::Error {
    fn is_retryable(&self) -> bool {
        use std::io::ErrorKind;
        matches!(
            self.kind(),
            ErrorKind::ConnectionReset
                | ErrorKind::ConnectionAborted
                | ErrorKind::TimedOut
                | ErrorKind::Interrupted
                | ErrorKind::WouldBlock
        )
    }
}

impl Retryable for anyhow::Error {
    fn is_retryable(&self) -> bool {
        // Check if the underlying error is retryable
        if let Some(io_err) = self.downcast_ref::<std::io::Error>() {
            return io_err.is_retryable();
        }
        if let Some(req_err) = self.downcast_ref::<teloxide::RequestError>() {
            return req_err.is_retryable();
        }

        // Check error message for common retryable patterns
        let msg = self.to_string().to_lowercase();
        msg.contains("timeout")
            || msg.contains("connection reset")
            || msg.contains("connection refused")
            || msg.contains("network")
            || msg.contains("temporarily unavailable")
    }
}

/// Executes an async operation with retry logic.
///
/// # Arguments
/// * `config` - Retry configuration
/// * `operation` - The async operation to execute
///
/// # Returns
/// A `RetryResult` containing either the successful result or the last error.
pub async fn retry<F, Fut, T, E>(config: &RetryConfig, mut operation: F) -> RetryResult<T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    E: Retryable + std::fmt::Debug,
{
    let start = std::time::Instant::now();
    let mut attempts = 0;

    loop {
        attempts += 1;

        match operation().await {
            Ok(value) => {
                return RetryResult {
                    result: Ok(value),
                    attempts,
                    total_duration: start.elapsed(),
                };
            }
            Err(e) if attempts <= config.max_retries && e.is_retryable() => {
                // Record retry metric
                metrics::TASK_RETRIES_TOTAL
                    .with_label_values(&[&attempts.to_string()])
                    .inc();

                // Calculate delay (respect retry_after hint if provided)
                let delay = e
                    .retry_after()
                    .unwrap_or_else(|| config.delay_for_attempt(attempts - 1));

                log::warn!(
                    "Attempt {}/{} failed (retrying in {:?}): {:?}",
                    attempts,
                    config.max_retries + 1,
                    delay,
                    e
                );

                tokio::time::sleep(delay).await;
            }
            Err(e) => {
                return RetryResult {
                    result: Err(RetryError::MaxRetriesExhausted {
                        max_retries: config.max_retries,
                        last_error: e,
                    }),
                    attempts,
                    total_duration: start.elapsed(),
                };
            }
        }
    }
}

/// Executes an async operation with retry logic and user notification.
///
/// Similar to `retry`, but also notifies the user about retry attempts.
pub async fn retry_with_notification<F, Fut, T, E>(
    bot: &Bot,
    chat_id: ChatId,
    config: &RetryConfig,
    operation_name: &str,
    mut operation: F,
) -> RetryResult<T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    E: Retryable + std::fmt::Debug,
{
    let start = std::time::Instant::now();
    let mut attempts = 0;

    loop {
        attempts += 1;

        match operation().await {
            Ok(value) => {
                // If we had to retry, send success message
                if attempts > 1 && config.notify_user {
                    let _ = bot
                        .send_message(
                            chat_id,
                            format!("✅ {} succeeded after {} attempt(s)", operation_name, attempts),
                        )
                        .await;
                }

                return RetryResult {
                    result: Ok(value),
                    attempts,
                    total_duration: start.elapsed(),
                };
            }
            Err(e) if attempts <= config.max_retries && e.is_retryable() => {
                // Record retry metric
                metrics::TASK_RETRIES_TOTAL
                    .with_label_values(&[&attempts.to_string()])
                    .inc();

                // Calculate delay
                let delay = e
                    .retry_after()
                    .unwrap_or_else(|| config.delay_for_attempt(attempts - 1));

                log::warn!(
                    "Attempt {}/{} for {} failed (retrying in {:?}): {:?}",
                    attempts,
                    config.max_retries + 1,
                    operation_name,
                    delay,
                    e
                );

                // Notify user about retry (only on first retry to avoid spam)
                if attempts == 1 && config.notify_user {
                    let _ = bot
                        .send_message(
                            chat_id,
                            format!(
                                "⚠️ {} failed, retrying... (attempt {}/{})",
                                operation_name,
                                attempts + 1,
                                config.max_retries + 1
                            ),
                        )
                        .await;
                }

                tokio::time::sleep(delay).await;
            }
            Err(e) => {
                // Notify user about final failure
                if config.notify_user {
                    let _ = bot
                        .send_message(
                            chat_id,
                            format!(
                                "❌ {} failed after {} attempt(s). Please try again later.",
                                operation_name, attempts
                            ),
                        )
                        .await;
                }

                return RetryResult {
                    result: Err(RetryError::MaxRetriesExhausted {
                        max_retries: config.max_retries,
                        last_error: e,
                    }),
                    attempts,
                    total_duration: start.elapsed(),
                };
            }
        }
    }
}

/// A wrapper that makes any error retryable.
#[derive(Debug)]
pub struct AlwaysRetryable<E>(pub E);

impl<E: std::fmt::Debug> Retryable for AlwaysRetryable<E> {
    fn is_retryable(&self) -> bool {
        true
    }
}

/// A wrapper that makes any error non-retryable.
#[derive(Debug)]
pub struct NeverRetryable<E>(pub E);

impl<E: std::fmt::Debug> Retryable for NeverRetryable<E> {
    fn is_retryable(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    #[derive(Debug)]
    struct TestError(bool); // bool = is_retryable

    impl std::fmt::Display for TestError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "TestError(retryable={})", self.0)
        }
    }

    impl std::error::Error for TestError {}

    impl Retryable for TestError {
        fn is_retryable(&self) -> bool {
            self.0
        }
    }

    #[tokio::test]
    async fn test_retry_succeeds_first_try() {
        let config = RetryConfig::quick();
        let result = retry(&config, || async { Ok::<_, TestError>(42) }).await;

        assert!(result.is_ok());
        assert_eq!(result.attempts, 1);
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn test_retry_succeeds_after_failures() {
        let config = RetryConfig::quick().initial_delay(Duration::from_millis(10));
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        let result = retry(&config, || {
            let counter = counter_clone.clone();
            async move {
                let attempt = counter.fetch_add(1, Ordering::SeqCst);
                if attempt < 2 {
                    Err(TestError(true))
                } else {
                    Ok(42)
                }
            }
        })
        .await;

        assert!(result.is_ok());
        assert_eq!(result.attempts, 3);
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn test_retry_exhausted() {
        let config = RetryConfig::quick()
            .max_retries(2)
            .initial_delay(Duration::from_millis(10));

        let result = retry(&config, || async { Err::<i32, _>(TestError(true)) }).await;

        assert!(result.is_exhausted());
        assert_eq!(result.attempts, 3); // 1 initial + 2 retries
    }

    #[tokio::test]
    async fn test_non_retryable_error_stops_immediately() {
        let config = RetryConfig::quick();
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        let result = retry(&config, || {
            let counter = counter_clone.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                Err::<i32, _>(TestError(false))
            }
        })
        .await;

        assert!(result.is_exhausted());
        assert_eq!(result.attempts, 1);
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_delay_calculation() {
        let config = RetryConfig::new()
            .initial_delay(Duration::from_secs(1))
            .backoff_multiplier(2.0)
            .max_delay(Duration::from_secs(10))
            .no_jitter();

        assert_eq!(config.delay_for_attempt(0), Duration::from_secs(1));
        assert_eq!(config.delay_for_attempt(1), Duration::from_secs(2));
        assert_eq!(config.delay_for_attempt(2), Duration::from_secs(4));
        assert_eq!(config.delay_for_attempt(3), Duration::from_secs(8));
        assert_eq!(config.delay_for_attempt(4), Duration::from_secs(10)); // capped
    }
}
