//! Mock downloader for load testing
//!
//! Simulates download operations with configurable delays and failure rates
//! for testing queue behavior and system performance under load.

#![allow(dead_code)] // Many methods and fields are kept for future use/extensibility

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::sleep;

/// Configuration for the mock downloader
#[derive(Debug, Clone)]
pub struct MockDownloaderConfig {
    /// Base delay for simulated downloads (milliseconds)
    pub base_delay_ms: u64,
    /// Random delay variance (milliseconds) added to base delay
    pub delay_variance_ms: u64,
    /// Probability of download failure (0.0 - 1.0)
    pub failure_rate: f64,
    /// Probability of timeout (0.0 - 1.0)
    pub timeout_rate: f64,
    /// Simulated file size range (min, max) in bytes
    pub file_size_range: (u64, u64),
    /// Whether to track detailed timing metrics
    pub collect_detailed_metrics: bool,
}

impl Default for MockDownloaderConfig {
    fn default() -> Self {
        Self {
            base_delay_ms: 500,
            delay_variance_ms: 200,
            failure_rate: 0.0,
            timeout_rate: 0.0,
            file_size_range: (1_000_000, 50_000_000), // 1MB - 50MB
            collect_detailed_metrics: true,
        }
    }
}

impl MockDownloaderConfig {
    /// Create config for fast testing (minimal delays)
    pub fn fast() -> Self {
        Self {
            base_delay_ms: 10,
            delay_variance_ms: 5,
            failure_rate: 0.0,
            timeout_rate: 0.0,
            file_size_range: (1_000_000, 10_000_000),
            collect_detailed_metrics: false,
        }
    }

    /// Create config simulating realistic download times
    pub fn realistic() -> Self {
        Self {
            base_delay_ms: 3000,
            delay_variance_ms: 2000,
            failure_rate: 0.02, // 2% failure rate
            timeout_rate: 0.01, // 1% timeout rate
            file_size_range: (5_000_000, 100_000_000),
            collect_detailed_metrics: true,
        }
    }

    /// Create config for stress testing (high failure rate)
    pub fn stress() -> Self {
        Self {
            base_delay_ms: 100,
            delay_variance_ms: 50,
            failure_rate: 0.1,  // 10% failure rate
            timeout_rate: 0.05, // 5% timeout rate
            file_size_range: (1_000_000, 500_000_000),
            collect_detailed_metrics: true,
        }
    }

    /// Set the failure rate
    pub fn with_failure_rate(mut self, rate: f64) -> Self {
        self.failure_rate = rate.clamp(0.0, 1.0);
        self
    }

    /// Set the base delay
    pub fn with_base_delay_ms(mut self, delay: u64) -> Self {
        self.base_delay_ms = delay;
        self
    }
}

/// Result of a mock download operation
#[derive(Debug, Clone)]
pub struct MockDownloadResult {
    /// Whether the download succeeded
    pub success: bool,
    /// Duration of the download in milliseconds
    pub duration_ms: u64,
    /// Simulated file size in bytes
    pub file_size: u64,
    /// Error message if failed
    pub error: Option<String>,
    /// Whether this was a timeout
    pub is_timeout: bool,
}

/// Statistics collected by the mock downloader
#[derive(Debug, Default)]
pub struct MockDownloaderStats {
    pub total_downloads: AtomicU64,
    pub successful_downloads: AtomicU64,
    pub failed_downloads: AtomicU64,
    pub timeout_downloads: AtomicU64,
    pub total_bytes: AtomicU64,
    pub total_duration_ms: AtomicU64,
    pub min_duration_ms: AtomicU64,
    pub max_duration_ms: AtomicU64,
}

impl MockDownloaderStats {
    pub fn new() -> Self {
        Self {
            min_duration_ms: AtomicU64::new(u64::MAX),
            ..Default::default()
        }
    }

    pub fn record_download(&self, result: &MockDownloadResult) {
        self.total_downloads.fetch_add(1, Ordering::Relaxed);
        self.total_duration_ms.fetch_add(result.duration_ms, Ordering::Relaxed);

        if result.success {
            self.successful_downloads.fetch_add(1, Ordering::Relaxed);
            self.total_bytes.fetch_add(result.file_size, Ordering::Relaxed);
        } else if result.is_timeout {
            self.timeout_downloads.fetch_add(1, Ordering::Relaxed);
        } else {
            self.failed_downloads.fetch_add(1, Ordering::Relaxed);
        }

        // Update min/max atomically (best effort)
        let _ = self
            .min_duration_ms
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                if result.duration_ms < current {
                    Some(result.duration_ms)
                } else {
                    None
                }
            });

        let _ = self
            .max_duration_ms
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                if result.duration_ms > current {
                    Some(result.duration_ms)
                } else {
                    None
                }
            });
    }

    pub fn summary(&self) -> MockDownloaderStatsSummary {
        let total = self.total_downloads.load(Ordering::Relaxed);
        let successful = self.successful_downloads.load(Ordering::Relaxed);
        let failed = self.failed_downloads.load(Ordering::Relaxed);
        let timeouts = self.timeout_downloads.load(Ordering::Relaxed);
        let total_bytes = self.total_bytes.load(Ordering::Relaxed);
        let total_duration = self.total_duration_ms.load(Ordering::Relaxed);
        let min_duration = self.min_duration_ms.load(Ordering::Relaxed);
        let max_duration = self.max_duration_ms.load(Ordering::Relaxed);

        MockDownloaderStatsSummary {
            total_downloads: total,
            successful_downloads: successful,
            failed_downloads: failed,
            timeout_downloads: timeouts,
            success_rate: if total > 0 {
                successful as f64 / total as f64
            } else {
                0.0
            },
            failure_rate: if total > 0 { failed as f64 / total as f64 } else { 0.0 },
            timeout_rate: if total > 0 { timeouts as f64 / total as f64 } else { 0.0 },
            total_bytes,
            avg_duration_ms: if total > 0 { total_duration / total } else { 0 },
            min_duration_ms: if min_duration == u64::MAX { 0 } else { min_duration },
            max_duration_ms: max_duration,
        }
    }
}

/// Summary of mock downloader statistics
#[derive(Debug, Clone)]
pub struct MockDownloaderStatsSummary {
    pub total_downloads: u64,
    pub successful_downloads: u64,
    pub failed_downloads: u64,
    pub timeout_downloads: u64,
    pub success_rate: f64,
    pub failure_rate: f64,
    pub timeout_rate: f64,
    pub total_bytes: u64,
    pub avg_duration_ms: u64,
    pub min_duration_ms: u64,
    pub max_duration_ms: u64,
}

/// Mock downloader that simulates download operations
pub struct MockDownloader {
    config: MockDownloaderConfig,
    stats: Arc<MockDownloaderStats>,
    rng: Mutex<SimpleRng>,
}

impl MockDownloader {
    pub fn new(config: MockDownloaderConfig) -> Self {
        Self {
            config,
            stats: Arc::new(MockDownloaderStats::new()),
            rng: Mutex::new(SimpleRng::new()),
        }
    }

    pub fn with_default_config() -> Self {
        Self::new(MockDownloaderConfig::default())
    }

    pub fn fast() -> Self {
        Self::new(MockDownloaderConfig::fast())
    }

    pub fn realistic() -> Self {
        Self::new(MockDownloaderConfig::realistic())
    }

    pub fn stress() -> Self {
        Self::new(MockDownloaderConfig::stress())
    }

    /// Get a clone of the stats Arc for sharing
    pub fn stats(&self) -> Arc<MockDownloaderStats> {
        Arc::clone(&self.stats)
    }

    /// Get current statistics summary
    pub fn get_stats_summary(&self) -> MockDownloaderStatsSummary {
        self.stats.summary()
    }

    /// Simulate a download operation
    pub async fn download(&self, _url: &str) -> MockDownloadResult {
        let start = std::time::Instant::now();

        // Calculate random delay
        let variance = {
            let mut rng = self.rng.lock().await;
            rng.next_u64() % (self.config.delay_variance_ms + 1)
        };
        let delay = Duration::from_millis(self.config.base_delay_ms + variance);

        // Simulate download time
        sleep(delay).await;

        let duration_ms = start.elapsed().as_millis() as u64;

        // Determine outcome based on random value
        let random_value = {
            let mut rng = self.rng.lock().await;
            rng.next_f64()
        };

        let result = if random_value < self.config.timeout_rate {
            MockDownloadResult {
                success: false,
                duration_ms,
                file_size: 0,
                error: Some("Download timed out".to_string()),
                is_timeout: true,
            }
        } else if random_value < self.config.timeout_rate + self.config.failure_rate {
            MockDownloadResult {
                success: false,
                duration_ms,
                file_size: 0,
                error: Some("Download failed: simulated error".to_string()),
                is_timeout: false,
            }
        } else {
            // Success - generate random file size
            let (min_size, max_size) = self.config.file_size_range;
            let size_range = max_size - min_size;
            let file_size = {
                let mut rng = self.rng.lock().await;
                min_size + (rng.next_u64() % (size_range + 1))
            };

            MockDownloadResult {
                success: true,
                duration_ms,
                file_size,
                error: None,
                is_timeout: false,
            }
        };

        // Record stats
        if self.config.collect_detailed_metrics {
            self.stats.record_download(&result);
        }

        result
    }

    /// Simulate downloading audio
    pub async fn download_audio(&self, url: &str) -> MockDownloadResult {
        self.download(url).await
    }

    /// Simulate downloading video
    pub async fn download_video(&self, url: &str) -> MockDownloadResult {
        // Videos take longer (1.5x)
        let mut result = self.download(url).await;
        result.duration_ms = result.duration_ms * 3 / 2;
        result
    }

    /// Simulate metadata extraction (faster than download)
    pub async fn get_metadata(&self, _url: &str) -> MockDownloadResult {
        let delay = Duration::from_millis(self.config.base_delay_ms / 5);
        sleep(delay).await;

        MockDownloadResult {
            success: true,
            duration_ms: delay.as_millis() as u64,
            file_size: 0,
            error: None,
            is_timeout: false,
        }
    }
}

/// Simple deterministic PRNG for reproducible tests
/// Uses xorshift64 algorithm
struct SimpleRng {
    state: u64,
}

impl SimpleRng {
    fn new() -> Self {
        // Use current time as seed, or a fixed seed for reproducibility
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x12345678_9ABCDEF0);
        Self { state: seed }
    }

    fn with_seed(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    fn next_f64(&mut self) -> f64 {
        // Convert to f64 in range [0, 1)
        (self.next_u64() as f64) / (u64::MAX as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_downloader_success() {
        let downloader = MockDownloader::new(MockDownloaderConfig {
            base_delay_ms: 10,
            delay_variance_ms: 5,
            failure_rate: 0.0,
            timeout_rate: 0.0,
            file_size_range: (1000, 2000),
            collect_detailed_metrics: true,
        });

        let result = downloader.download("https://example.com/test").await;

        assert!(result.success);
        assert!(!result.is_timeout);
        assert!(result.error.is_none());
        assert!(result.file_size >= 1000 && result.file_size <= 2000);
    }

    #[tokio::test]
    async fn test_mock_downloader_with_failures() {
        let downloader = MockDownloader::new(MockDownloaderConfig {
            base_delay_ms: 5,
            delay_variance_ms: 2,
            failure_rate: 1.0, // 100% failure
            timeout_rate: 0.0,
            file_size_range: (1000, 2000),
            collect_detailed_metrics: true,
        });

        let result = downloader.download("https://example.com/test").await;

        assert!(!result.success);
        assert!(result.error.is_some());
    }

    #[tokio::test]
    async fn test_mock_downloader_stats() {
        // Use config with metrics enabled
        let config = MockDownloaderConfig {
            base_delay_ms: 5,
            delay_variance_ms: 2,
            failure_rate: 0.0,
            timeout_rate: 0.0,
            file_size_range: (1000, 2000),
            collect_detailed_metrics: true, // Must be true to collect stats
        };
        let downloader = MockDownloader::new(config);

        // Run multiple downloads
        for i in 0..10 {
            let _ = downloader.download(&format!("https://example.com/test{}", i)).await;
        }

        let summary = downloader.get_stats_summary();
        assert_eq!(summary.total_downloads, 10);
        assert_eq!(summary.successful_downloads, 10);
        assert!(summary.avg_duration_ms > 0);
    }

    #[test]
    fn test_simple_rng() {
        let mut rng = SimpleRng::with_seed(42);
        let values: Vec<u64> = (0..10).map(|_| rng.next_u64()).collect();

        // Values should be different
        for i in 0..values.len() {
            for j in (i + 1)..values.len() {
                assert_ne!(values[i], values[j]);
            }
        }
    }

    #[test]
    fn test_config_builders() {
        let fast = MockDownloaderConfig::fast();
        assert_eq!(fast.base_delay_ms, 10);

        let realistic = MockDownloaderConfig::realistic();
        assert_eq!(realistic.base_delay_ms, 3000);
        assert!(realistic.failure_rate > 0.0);

        let stress = MockDownloaderConfig::stress();
        assert!(stress.failure_rate >= 0.1);
    }
}
