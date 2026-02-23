//! Smoke test result types and reporting structures.

use chrono::{DateTime, Utc};
use std::fmt;
use std::time::Duration;

/// Status of a smoke test
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SmokeTestStatus {
    Passed,
    Failed,
    Timeout,
    Skipped,
}

impl fmt::Display for SmokeTestStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SmokeTestStatus::Passed => write!(f, "PASSED"),
            SmokeTestStatus::Failed => write!(f, "FAILED"),
            SmokeTestStatus::Timeout => write!(f, "TIMEOUT"),
            SmokeTestStatus::Skipped => write!(f, "SKIPPED"),
        }
    }
}

/// Result of a single smoke test
#[derive(Debug, Clone)]
pub struct SmokeTestResult {
    /// Name of the test
    pub test_name: String,
    /// Status of the test
    pub status: SmokeTestStatus,
    /// Duration of the test
    pub duration: Duration,
    /// Error message if failed
    pub error_message: Option<String>,
    /// File size in bytes (for download tests)
    pub file_size_bytes: Option<u64>,
    /// Audio/video duration in seconds
    pub media_duration_secs: Option<u32>,
    /// Whether video has both video and audio streams
    pub video_has_both_streams: Option<bool>,
    /// Extracted metadata title
    pub metadata_title: Option<String>,
    /// Extracted metadata artist
    pub metadata_artist: Option<String>,
    /// Proxy used for the test
    pub proxy_used: Option<String>,
    /// Timestamp when test completed
    pub timestamp: DateTime<Utc>,
}

impl SmokeTestResult {
    /// Create a passed result
    pub fn passed(test_name: &str, duration: Duration) -> Self {
        Self {
            test_name: test_name.to_string(),
            status: SmokeTestStatus::Passed,
            duration,
            error_message: None,
            file_size_bytes: None,
            media_duration_secs: None,
            video_has_both_streams: None,
            metadata_title: None,
            metadata_artist: None,
            proxy_used: None,
            timestamp: Utc::now(),
        }
    }

    /// Create a failed result
    pub fn failed(test_name: &str, duration: Duration, error: &str) -> Self {
        Self {
            test_name: test_name.to_string(),
            status: SmokeTestStatus::Failed,
            duration,
            error_message: Some(error.to_string()),
            file_size_bytes: None,
            media_duration_secs: None,
            video_has_both_streams: None,
            metadata_title: None,
            metadata_artist: None,
            proxy_used: None,
            timestamp: Utc::now(),
        }
    }

    /// Create a timeout result
    pub fn timeout(test_name: &str, timeout_duration: Duration) -> Self {
        Self {
            test_name: test_name.to_string(),
            status: SmokeTestStatus::Timeout,
            duration: timeout_duration,
            error_message: Some(format!("Test timed out after {:?}", timeout_duration)),
            file_size_bytes: None,
            media_duration_secs: None,
            video_has_both_streams: None,
            metadata_title: None,
            metadata_artist: None,
            proxy_used: None,
            timestamp: Utc::now(),
        }
    }

    /// Create a skipped result
    pub fn skipped(test_name: &str, reason: &str) -> Self {
        Self {
            test_name: test_name.to_string(),
            status: SmokeTestStatus::Skipped,
            duration: Duration::ZERO,
            error_message: Some(reason.to_string()),
            file_size_bytes: None,
            media_duration_secs: None,
            video_has_both_streams: None,
            metadata_title: None,
            metadata_artist: None,
            proxy_used: None,
            timestamp: Utc::now(),
        }
    }

    /// Format result for logging
    pub fn format_log(&self) -> String {
        let emoji = match self.status {
            SmokeTestStatus::Passed => "✅",
            SmokeTestStatus::Failed => "❌",
            SmokeTestStatus::Timeout => "⏱️",
            SmokeTestStatus::Skipped => "⏭️",
        };

        let mut msg = format!("{} {} ({:?})", emoji, self.test_name, self.duration);

        if let Some(ref error) = self.error_message {
            msg.push_str(&format!(" - {}", error));
        }

        msg
    }

    /// Format result for Telegram notification
    pub fn format_telegram(&self) -> String {
        let emoji = match self.status {
            SmokeTestStatus::Passed => "✅",
            SmokeTestStatus::Failed => "❌",
            SmokeTestStatus::Timeout => "⏱️",
            SmokeTestStatus::Skipped => "⏭️",
        };

        let mut msg = format!("{} {} ({:.1}s)", emoji, self.test_name, self.duration.as_secs_f64());

        if let Some(ref error) = self.error_message {
            // Truncate long error messages
            let error_preview = if error.len() > 100 {
                format!("{}...", &error[..100])
            } else {
                error.clone()
            };
            msg.push_str(&format!("\n   {}", error_preview));
        }

        msg
    }
}

/// Aggregated report of all smoke tests
#[derive(Debug, Clone)]
pub struct SmokeTestReport {
    /// Individual test results
    pub results: Vec<SmokeTestResult>,
    /// Total duration of all tests
    pub total_duration: Duration,
    /// Count of passed tests
    pub passed_count: usize,
    /// Count of failed tests
    pub failed_count: usize,
    /// Count of timed out tests
    pub timeout_count: usize,
    /// Count of skipped tests
    pub skipped_count: usize,
    /// Timestamp when report was generated
    pub timestamp: DateTime<Utc>,
}

impl SmokeTestReport {
    /// Create a new report from test results
    pub fn new(results: Vec<SmokeTestResult>, total_duration: Duration) -> Self {
        let passed_count = results.iter().filter(|r| r.status == SmokeTestStatus::Passed).count();
        let failed_count = results.iter().filter(|r| r.status == SmokeTestStatus::Failed).count();
        let timeout_count = results.iter().filter(|r| r.status == SmokeTestStatus::Timeout).count();
        let skipped_count = results.iter().filter(|r| r.status == SmokeTestStatus::Skipped).count();

        Self {
            results,
            total_duration,
            passed_count,
            failed_count,
            timeout_count,
            skipped_count,
            timestamp: Utc::now(),
        }
    }

    /// Check if all tests passed
    pub fn all_passed(&self) -> bool {
        self.failed_count == 0 && self.timeout_count == 0
    }

    /// Format report for Telegram notification
    pub fn format_telegram(&self) -> String {
        let status_emoji = if self.all_passed() { "✅" } else { "🔴" };
        let status_text = if self.all_passed() {
            "HEALTH CHECK PASSED"
        } else {
            "HEALTH CHECK FAILED"
        };

        let mut msg = format!("{} {}\n\n", status_emoji, status_text);

        for result in &self.results {
            msg.push_str(&result.format_telegram());
            msg.push('\n');
        }

        msg.push_str(&format!(
            "\nTotal: {}/{} passed in {:.1}s",
            self.passed_count,
            self.results.len(),
            self.total_duration.as_secs_f64()
        ));

        msg
    }

    /// Format report for logging
    pub fn format_log(&self) -> String {
        let mut msg = String::new();

        for result in &self.results {
            msg.push_str(&result.format_log());
            msg.push('\n');
        }

        msg.push_str(&format!(
            "Summary: {}/{} passed, {} failed, {} timeout in {:?}",
            self.passed_count,
            self.results.len(),
            self.failed_count,
            self.timeout_count,
            self.total_duration
        ));

        msg
    }
}
