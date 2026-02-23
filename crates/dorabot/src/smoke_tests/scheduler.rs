//! Health check scheduler for production environment.
//!
//! Runs smoke tests at regular intervals (default: every hour) and sends
//! alerts to admins when tests fail.

use super::results::SmokeTestReport;
use super::runner::{run_all_smoke_tests, SmokeTestConfig};
use crate::telegram::notifications::notify_admin_text;
use crate::telegram::Bot;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::interval;

/// Default interval between health checks (1 hour)
pub const DEFAULT_HEALTH_CHECK_INTERVAL_SECS: u64 = 3600;

/// Health check scheduler that runs smoke tests periodically
pub struct HealthCheckScheduler {
    bot: Arc<Bot>,
    config: SmokeTestConfig,
    interval_secs: u64,
    running: Arc<AtomicBool>,
}

impl HealthCheckScheduler {
    /// Creates a new health check scheduler.
    ///
    /// # Arguments
    ///
    /// * `bot` - Telegram bot for sending notifications
    /// * `interval_secs` - Interval between health checks in seconds
    pub fn new(bot: Arc<Bot>, interval_secs: u64) -> Self {
        Self {
            bot,
            config: SmokeTestConfig::for_production(),
            interval_secs,
            running: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Creates a scheduler with custom configuration.
    pub fn with_config(bot: Arc<Bot>, interval_secs: u64, config: SmokeTestConfig) -> Self {
        Self {
            bot,
            config,
            interval_secs,
            running: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Checks if health check is enabled via environment variable.
    ///
    /// Returns true if HEALTH_CHECK_ENABLED is not set or is set to "true", "1", "yes"
    pub fn is_enabled() -> bool {
        std::env::var("HEALTH_CHECK_ENABLED")
            .map(|v| {
                let v = v.to_lowercase();
                v == "true" || v == "1" || v == "yes"
            })
            .unwrap_or(true) // Enabled by default
    }

    /// Gets the interval from environment variable or default.
    pub fn get_interval_secs() -> u64 {
        std::env::var("HEALTH_CHECK_INTERVAL_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_HEALTH_CHECK_INTERVAL_SECS)
    }

    /// Runs a single health check and returns the report.
    pub async fn run_health_check(&self) -> SmokeTestReport {
        log::info!("Starting scheduled health check...");
        let report = run_all_smoke_tests(&self.config).await;

        // Update metrics
        self.update_metrics(&report);

        // Send alert if any tests failed
        if !report.all_passed() {
            self.send_failure_alert(&report).await;
        } else {
            log::info!(
                "Health check passed: {}/{} tests OK",
                report.passed_count,
                report.results.len()
            );
        }

        report
    }

    /// Updates Prometheus metrics based on the report.
    fn update_metrics(&self, report: &SmokeTestReport) {
        use crate::core::metrics;

        // Update overall health status
        let status = if report.all_passed() { 1.0 } else { 0.0 };
        metrics::HEALTH_CHECK_STATUS.set(status);

        // Update last run timestamp
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as f64;
        metrics::HEALTH_CHECK_LAST_RUN.set(timestamp);

        // Update individual test results
        for result in &report.results {
            let status_label = match result.status {
                super::results::SmokeTestStatus::Passed => "passed",
                super::results::SmokeTestStatus::Failed => "failed",
                super::results::SmokeTestStatus::Timeout => "timeout",
                super::results::SmokeTestStatus::Skipped => "skipped",
            };

            metrics::SMOKE_TEST_RESULTS
                .with_label_values(&[result.test_name.as_str(), status_label])
                .inc();

            metrics::SMOKE_TEST_DURATION
                .with_label_values(&[result.test_name.as_str()])
                .observe(result.duration.as_secs_f64());
        }
    }

    /// Sends failure alert to admins via Telegram.
    async fn send_failure_alert(&self, report: &SmokeTestReport) {
        let message = format!(
            "🔴 HEALTH CHECK FAILED\n\n\
            {}\n\n\
            Total: {}/{} passed in {:.1}s\n\n\
            Check logs for details.",
            report
                .results
                .iter()
                .map(|r| r.format_telegram())
                .collect::<Vec<_>>()
                .join("\n"),
            report.passed_count,
            report.results.len(),
            report.total_duration.as_secs_f64()
        );

        notify_admin_text(&self.bot, &message).await;
    }

    /// Starts the scheduler loop.
    ///
    /// This runs indefinitely, executing health checks at the configured interval.
    /// The first health check runs after the initial delay (not immediately).
    pub async fn start(&self) {
        if !Self::is_enabled() {
            log::info!("Health check scheduler is disabled (HEALTH_CHECK_ENABLED=false)");
            return;
        }

        self.running.store(true, Ordering::SeqCst);
        log::info!("Starting health check scheduler with {}s interval", self.interval_secs);

        let mut timer = interval(Duration::from_secs(self.interval_secs));

        // Skip first tick (don't run immediately on startup)
        timer.tick().await;

        while self.running.load(Ordering::SeqCst) {
            timer.tick().await;

            if !self.running.load(Ordering::SeqCst) {
                break;
            }

            self.run_health_check().await;
        }

        log::info!("Health check scheduler stopped");
    }

    /// Stops the scheduler.
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }
}

/// Starts the health check scheduler in a background task.
///
/// # Arguments
///
/// * `bot` - Telegram bot for sending notifications
///
/// # Returns
///
/// Handle to the scheduler for stopping it later
pub fn start_health_check_scheduler(bot: Arc<Bot>) -> Arc<HealthCheckScheduler> {
    let interval_secs = HealthCheckScheduler::get_interval_secs();
    let scheduler = Arc::new(HealthCheckScheduler::new(bot, interval_secs));

    if HealthCheckScheduler::is_enabled() {
        let scheduler_clone = scheduler.clone();
        tokio::spawn(async move {
            scheduler_clone.start().await;
        });
    }

    scheduler
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_enabled_default() {
        // Clear env var for test
        std::env::remove_var("HEALTH_CHECK_ENABLED");
        assert!(HealthCheckScheduler::is_enabled());
    }

    #[test]
    fn test_get_interval_default() {
        std::env::remove_var("HEALTH_CHECK_INTERVAL_SECS");
        assert_eq!(
            HealthCheckScheduler::get_interval_secs(),
            DEFAULT_HEALTH_CHECK_INTERVAL_SECS
        );
    }
}
