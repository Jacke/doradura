//! Integration tests for core modules (retry, alerts, metrics)
//!
//! Run with: cargo test --test core_modules_test

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

// ============================================================================
// Retry Module Tests
// ============================================================================

mod retry_tests {
    use super::*;
    use doradura::core::retry::{retry, RetryConfig, Retryable};

    #[derive(Debug, Clone)]
    struct TestError {
        retryable: bool,
        retry_after: Option<Duration>,
    }

    impl std::fmt::Display for TestError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "TestError(retryable={})", self.retryable)
        }
    }

    impl std::error::Error for TestError {}

    impl Retryable for TestError {
        fn is_retryable(&self) -> bool {
            self.retryable
        }

        fn retry_after(&self) -> Option<Duration> {
            self.retry_after
        }
    }

    #[tokio::test]
    async fn test_retry_immediate_success() {
        let config = RetryConfig::quick();

        let result = retry(&config, || async { Ok::<i32, TestError>(42) }).await;

        assert!(result.is_ok());
        assert_eq!(result.attempts, 1);
        assert_eq!(result.result.unwrap(), 42);
    }

    #[tokio::test]
    async fn test_retry_succeeds_on_second_attempt() {
        let config = RetryConfig::quick().initial_delay(Duration::from_millis(10));
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        let result = retry(&config, || {
            let counter = counter_clone.clone();
            async move {
                let attempt = counter.fetch_add(1, Ordering::SeqCst);
                if attempt == 0 {
                    Err(TestError {
                        retryable: true,
                        retry_after: None,
                    })
                } else {
                    Ok(42)
                }
            }
        })
        .await;

        assert!(result.is_ok());
        assert_eq!(result.attempts, 2);
        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_retry_exhausts_all_attempts() {
        let config = RetryConfig::quick()
            .max_retries(3)
            .initial_delay(Duration::from_millis(10));

        let result = retry(&config, || async {
            Err::<i32, _>(TestError {
                retryable: true,
                retry_after: None,
            })
        })
        .await;

        assert!(result.is_exhausted());
        assert_eq!(result.attempts, 4); // 1 initial + 3 retries
    }

    #[tokio::test]
    async fn test_non_retryable_error_stops_immediately() {
        let config = RetryConfig::quick().max_retries(5);
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        let result = retry(&config, || {
            let counter = counter_clone.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                Err::<i32, _>(TestError {
                    retryable: false,
                    retry_after: None,
                })
            }
        })
        .await;

        assert!(result.is_exhausted());
        assert_eq!(result.attempts, 1);
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_retry_respects_retry_after_hint() {
        let config = RetryConfig::quick()
            .max_retries(2)
            .initial_delay(Duration::from_secs(60)); // Long default delay
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        let start = std::time::Instant::now();

        let result = retry(&config, || {
            let counter = counter_clone.clone();
            async move {
                let attempt = counter.fetch_add(1, Ordering::SeqCst);
                if attempt == 0 {
                    // Return short retry_after to override long default
                    Err(TestError {
                        retryable: true,
                        retry_after: Some(Duration::from_millis(50)),
                    })
                } else {
                    Ok(42)
                }
            }
        })
        .await;

        let elapsed = start.elapsed();

        assert!(result.is_ok());
        // Should use the 50ms hint, not the 60s default
        assert!(elapsed < Duration::from_secs(1));
    }

    #[test]
    fn test_retry_config_delay_calculation() {
        let config = RetryConfig::new()
            .initial_delay(Duration::from_secs(1))
            .backoff_multiplier(2.0)
            .max_delay(Duration::from_secs(10))
            .no_jitter();

        // 1 * 2^0 = 1
        assert_eq!(config.delay_for_attempt(0), Duration::from_secs(1));
        // 1 * 2^1 = 2
        assert_eq!(config.delay_for_attempt(1), Duration::from_secs(2));
        // 1 * 2^2 = 4
        assert_eq!(config.delay_for_attempt(2), Duration::from_secs(4));
        // 1 * 2^3 = 8
        assert_eq!(config.delay_for_attempt(3), Duration::from_secs(8));
        // 1 * 2^4 = 16, but capped at 10
        assert_eq!(config.delay_for_attempt(4), Duration::from_secs(10));
    }

    #[test]
    fn test_retry_config_presets() {
        let network = RetryConfig::network();
        assert_eq!(network.max_retries, 5);

        let rate_limit = RetryConfig::rate_limit();
        assert_eq!(rate_limit.max_retries, 3);
        assert!(!rate_limit.add_jitter);

        let quick = RetryConfig::quick();
        assert_eq!(quick.max_retries, 2);

        let aggressive = RetryConfig::aggressive();
        assert_eq!(aggressive.max_retries, 10);
    }
}

// ============================================================================
// Metrics Module Tests
// ============================================================================

mod metrics_tests {
    use doradura::core::metrics;

    #[test]
    fn test_extract_platform_youtube() {
        assert_eq!(
            metrics::extract_platform("https://www.youtube.com/watch?v=abc"),
            "youtube"
        );
        assert_eq!(metrics::extract_platform("https://youtu.be/abc"), "youtube");
        assert_eq!(
            metrics::extract_platform("https://m.youtube.com/watch?v=abc"),
            "youtube"
        );
    }

    #[test]
    fn test_extract_platform_soundcloud() {
        assert_eq!(
            metrics::extract_platform("https://soundcloud.com/artist/track"),
            "soundcloud"
        );
    }

    #[test]
    fn test_extract_platform_twitter() {
        assert_eq!(
            metrics::extract_platform("https://twitter.com/user/status/123"),
            "twitter"
        );
        assert_eq!(metrics::extract_platform("https://x.com/user/status/123"), "twitter");
    }

    #[test]
    fn test_extract_platform_other() {
        assert_eq!(metrics::extract_platform("https://example.com/video"), "other");
        assert_eq!(metrics::extract_platform("https://random-site.org/media"), "other");
    }

    #[test]
    fn test_init_metrics_does_not_panic() {
        // Should not panic when called multiple times
        metrics::init_metrics();
        metrics::init_metrics();
    }

    #[test]
    fn test_record_functions_do_not_panic() {
        // These should not panic
        metrics::record_download_success("mp3", "320k");
        metrics::record_download_failure("mp4", "timeout");
        metrics::record_error("download", "test");
        metrics::record_command("test");
        metrics::record_format_request("mp3", "free");
        metrics::update_queue_depth("high", 5);
        metrics::update_queue_depth_total(10);
        metrics::record_operation_success("download", "mp3");
        metrics::record_operation_failure("upload", "mp4", "network");
        metrics::record_file_size("mp3", 1_000_000);
        metrics::record_platform_download("youtube");
        metrics::update_cookies_status(true);
        metrics::record_user_feedback("positive");
        metrics::record_alert("test", "warning");
    }
}

// ============================================================================
// Alerts Module Tests
// ============================================================================

mod alerts_tests {
    use doradura::core::alerts::{Alert, AlertType, Severity};

    #[test]
    fn test_severity_levels_exist() {
        // Just verify severity levels can be created
        let _ = Severity::Critical;
        let _ = Severity::Warning;
    }

    #[test]
    fn test_alert_types_exist() {
        // Just verify all alert types can be created
        let _ = AlertType::HighErrorRate;
        let _ = AlertType::QueueBackup;
        let _ = AlertType::PaymentFailure;
        let _ = AlertType::YtdlpDown;
        let _ = AlertType::DatabaseIssues;
        let _ = AlertType::LowConversion;
        let _ = AlertType::HighRetryRate;
        let _ = AlertType::CookiesExpired;
        let _ = AlertType::HighTimeoutRate;
        let _ = AlertType::LowDiskSpace;
        let _ = AlertType::UserComplaint;
        let _ = AlertType::HighResourceUsage;
    }

    #[test]
    fn test_alert_format() {
        let alert = Alert::new(
            AlertType::HighErrorRate,
            Severity::Critical,
            "Test Alert".to_string(),
            "Test message".to_string(),
            Some("Additional details".to_string()),
        );

        let formatted = alert.format_telegram_message();

        assert!(formatted.contains("CRITICAL"));
        assert!(formatted.contains("Test Alert"));
        assert!(formatted.contains("Test message"));
        assert!(formatted.contains("Details"));
    }
}

// ============================================================================
// Utils Module Tests
// ============================================================================

mod utils_tests {
    use doradura::core::utils::{escape_filename, escape_markdown_v2, format_media_caption, pluralize_seconds};

    #[test]
    fn test_escape_filename_basic() {
        // New behavior: ASCII-only, underscores collapsed, underscore before dot removed
        assert_eq!(escape_filename("song/name.mp3"), "song_name.mp3");
        assert_eq!(escape_filename("file:name.mp4"), "file_name.mp4");
        assert_eq!(escape_filename("song*title?.mp3"), "song_title.mp3"); // underscores collapsed
    }

    #[test]
    fn test_escape_filename_quotes() {
        // New behavior: quotes become underscore and collapse
        assert_eq!(escape_filename("song \"live\".mp3"), "song_live.mp3");
    }

    #[test]
    fn test_escape_filename_trim() {
        assert_eq!(escape_filename("  file.mp3  "), "file.mp3");
        assert_eq!(escape_filename("...file..."), "file");
    }

    #[test]
    fn test_escape_filename_empty() {
        assert_eq!(escape_filename(""), "unnamed");
        assert_eq!(escape_filename("..."), "unnamed");
        assert_eq!(escape_filename("   "), "unnamed");
    }

    #[test]
    fn test_escape_markdown_v2() {
        assert_eq!(escape_markdown_v2("Hello. World!"), "Hello\\. World\\!");
        assert_eq!(escape_markdown_v2("file.mp3"), "file\\.mp3");
        assert_eq!(escape_markdown_v2("track-name"), "track\\-name");
    }

    #[test]
    fn test_pluralize_seconds() {
        // Singular
        assert_eq!(pluralize_seconds(1), "секунду");
        assert_eq!(pluralize_seconds(21), "секунду");
        assert_eq!(pluralize_seconds(101), "секунду");

        // 2-4 form
        assert_eq!(pluralize_seconds(2), "секунды");
        assert_eq!(pluralize_seconds(3), "секунды");
        assert_eq!(pluralize_seconds(22), "секунды");

        // 5+ form
        assert_eq!(pluralize_seconds(5), "секунд");
        assert_eq!(pluralize_seconds(11), "секунд");
        assert_eq!(pluralize_seconds(20), "секунд");
    }

    #[test]
    fn test_format_media_caption() {
        // Note: format_media_caption now appends copyright signature
        // Tests check that caption starts with expected base part

        // With artist
        assert!(format_media_caption("Song Name", "Artist").starts_with("*Artist* — _Song Name_"));

        // Without artist
        assert!(format_media_caption("Song Name", "").starts_with("_Song Name_"));
        assert!(format_media_caption("Song Name", "   ").starts_with("_Song Name_"));

        // Check copyright is appended
        let caption = format_media_caption("Test", "Artist");
        assert!(caption.contains("Ваша,"));
    }
}

// ============================================================================
// Operation Module Tests
// ============================================================================

mod operation_tests {
    use doradura::telegram::operation::{MessageFormatter, OperationInfo, OperationStatus, PlainTextFormatter};

    #[test]
    fn test_operation_info_default() {
        let info = OperationInfo::default();
        assert_eq!(info.title, "Operation");
        assert_eq!(info.emoji, "\u{2699}\u{fe0f}"); // Gear emoji
    }

    #[test]
    fn test_operation_info_custom() {
        let info = OperationInfo::new("Downloading", Some("\u{1F4E5}")); // Inbox tray emoji
        assert_eq!(info.title, "Downloading");
        assert_eq!(info.emoji, "\u{1F4E5}");
    }

    #[test]
    fn test_operation_status_is_terminal() {
        let info = OperationInfo::default();

        let starting = OperationStatus::Starting(info.clone());
        assert!(!starting.is_terminal());

        let success = OperationStatus::Success {
            info: info.clone(),
            message: None,
        };
        assert!(success.is_terminal());

        let error = OperationStatus::Error {
            info,
            error: "Test error".to_string(),
        };
        assert!(error.is_terminal());
    }

    #[test]
    fn test_plain_text_formatter() {
        let formatter = PlainTextFormatter;
        let info = OperationInfo::new("Test", Some("\u{1F4E5}"));

        let starting = OperationStatus::Starting(info.clone());
        let formatted = formatter.format(&starting);
        assert!(formatted.contains("Test"));
        assert!(formatted.contains("Starting"));

        let progress = OperationStatus::Progress {
            info: info.clone(),
            progress: 50,
            stage: Some("Processing".to_string()),
        };
        let formatted = formatter.format(&progress);
        assert!(formatted.contains("50%"));
        assert!(formatted.contains("Processing"));

        let success = OperationStatus::Success {
            info,
            message: Some("Complete!".to_string()),
        };
        let formatted = formatter.format(&success);
        assert!(formatted.contains("Done"));
        assert!(formatted.contains("Complete"));
    }
}
