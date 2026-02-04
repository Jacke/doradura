//! Metrics collection for load testing
//!
//! Tracks queue depth, latency, wait times, error rates, and system resource usage
//! during load tests.

#![allow(dead_code)] // Many methods and fields are kept for future use/extensibility

use parking_lot::RwLock;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// Configuration for metrics collection
#[derive(Debug, Clone)]
pub struct MetricsConfig {
    /// How often to sample queue depth (ms)
    pub sample_interval_ms: u64,
    /// Maximum number of samples to keep in history
    pub max_history_samples: usize,
    /// Whether to collect detailed per-request timing
    pub collect_request_timing: bool,
    /// Whether to collect memory usage samples
    pub collect_memory_usage: bool,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self {
            sample_interval_ms: 100,
            max_history_samples: 10000,
            collect_request_timing: true,
            collect_memory_usage: true,
        }
    }
}

/// A single timing measurement
#[derive(Debug, Clone, Copy)]
pub struct TimingSample {
    /// When the request was submitted
    pub submitted_at: Instant,
    /// When the request started processing (left queue)
    pub started_at: Option<Instant>,
    /// When the request completed
    pub completed_at: Option<Instant>,
    /// User plan for priority tracking
    pub priority: u8,
    /// Whether the request succeeded
    pub success: bool,
}

impl TimingSample {
    pub fn new(priority: u8) -> Self {
        Self {
            submitted_at: Instant::now(),
            started_at: None,
            completed_at: None,
            priority,
            success: false,
        }
    }

    pub fn queue_wait_time(&self) -> Option<Duration> {
        self.started_at.map(|s| s.duration_since(self.submitted_at))
    }

    pub fn processing_time(&self) -> Option<Duration> {
        match (self.started_at, self.completed_at) {
            (Some(s), Some(c)) => Some(c.duration_since(s)),
            _ => None,
        }
    }

    pub fn total_time(&self) -> Option<Duration> {
        self.completed_at.map(|c| c.duration_since(self.submitted_at))
    }
}

/// Queue depth sample at a point in time
#[derive(Debug, Clone, Copy)]
pub struct QueueDepthSample {
    pub timestamp: Instant,
    pub depth: usize,
    pub low_priority: usize,
    pub medium_priority: usize,
    pub high_priority: usize,
}

/// Memory usage sample
#[derive(Debug, Clone, Copy)]
pub struct MemorySample {
    pub timestamp: Instant,
    pub used_bytes: u64,
    pub total_bytes: u64,
}

/// Aggregated latency statistics
#[derive(Debug, Clone, Default)]
pub struct LatencyStats {
    pub count: u64,
    pub sum_ms: u64,
    pub min_ms: u64,
    pub max_ms: u64,
    pub p50_ms: u64,
    pub p95_ms: u64,
    pub p99_ms: u64,
}

impl LatencyStats {
    pub fn avg_ms(&self) -> f64 {
        if self.count > 0 {
            self.sum_ms as f64 / self.count as f64
        } else {
            0.0
        }
    }
}

/// Load test metrics collector
pub struct LoadTestMetrics {
    config: MetricsConfig,
    start_time: Instant,

    // Request counters
    requests_submitted: AtomicU64,
    requests_completed: AtomicU64,
    requests_failed: AtomicU64,
    requests_timeout: AtomicU64,

    // Current state
    current_queue_depth: AtomicUsize,
    active_downloads: AtomicUsize,

    // Timing samples (protected by RwLock for concurrent access)
    timing_samples: RwLock<VecDeque<TimingSample>>,

    // Queue depth history
    queue_depth_history: RwLock<VecDeque<QueueDepthSample>>,

    // Memory samples
    memory_history: RwLock<VecDeque<MemorySample>>,

    // Latency histograms (buckets in ms)
    queue_wait_histogram: RwLock<Vec<u64>>,
    processing_histogram: RwLock<Vec<u64>>,
    total_latency_histogram: RwLock<Vec<u64>>,
}

impl LoadTestMetrics {
    /// Create a new metrics collector
    pub fn new(config: MetricsConfig) -> Self {
        // Histogram buckets: 0-10ms, 10-50ms, 50-100ms, 100-500ms, 500ms-1s, 1-5s, 5-30s, 30s-1m, 1-5m, 5m+
        let histogram_size = 20;

        Self {
            config,
            start_time: Instant::now(),
            requests_submitted: AtomicU64::new(0),
            requests_completed: AtomicU64::new(0),
            requests_failed: AtomicU64::new(0),
            requests_timeout: AtomicU64::new(0),
            current_queue_depth: AtomicUsize::new(0),
            active_downloads: AtomicUsize::new(0),
            timing_samples: RwLock::new(VecDeque::with_capacity(10000)),
            queue_depth_history: RwLock::new(VecDeque::with_capacity(10000)),
            memory_history: RwLock::new(VecDeque::with_capacity(1000)),
            queue_wait_histogram: RwLock::new(vec![0; histogram_size]),
            processing_histogram: RwLock::new(vec![0; histogram_size]),
            total_latency_histogram: RwLock::new(vec![0; histogram_size]),
        }
    }

    pub fn with_default_config() -> Self {
        Self::new(MetricsConfig::default())
    }

    /// Record a request submission
    pub fn record_submit(&self, priority: u8) -> usize {
        let id = self.requests_submitted.fetch_add(1, Ordering::Relaxed) as usize;
        self.current_queue_depth.fetch_add(1, Ordering::Relaxed);

        if self.config.collect_request_timing {
            let mut samples = self.timing_samples.write();
            samples.push_back(TimingSample::new(priority));

            // Trim old samples if needed
            while samples.len() > self.config.max_history_samples {
                samples.pop_front();
            }
        }

        id
    }

    /// Record that a request started processing
    pub fn record_start(&self, sample_id: usize) {
        // Use saturating subtraction to avoid underflow
        let _ = self
            .current_queue_depth
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |val| Some(val.saturating_sub(1)));
        self.active_downloads.fetch_add(1, Ordering::Relaxed);

        if self.config.collect_request_timing {
            let mut samples = self.timing_samples.write();
            let len = samples.len();
            if len > 0 {
                if let Some(sample) = samples.get_mut(sample_id % len) {
                    sample.started_at = Some(Instant::now());
                }
            }
        }
    }

    /// Record request completion
    pub fn record_complete(&self, sample_id: usize, success: bool) {
        self.active_downloads.fetch_sub(1, Ordering::Relaxed);

        if success {
            self.requests_completed.fetch_add(1, Ordering::Relaxed);
        } else {
            self.requests_failed.fetch_add(1, Ordering::Relaxed);
        }

        if self.config.collect_request_timing {
            let mut samples = self.timing_samples.write();
            let len = samples.len();
            if len > 0 {
                if let Some(sample) = samples.get_mut(sample_id % len) {
                    sample.completed_at = Some(Instant::now());
                    sample.success = success;

                    // Update histograms
                    if let Some(wait) = sample.queue_wait_time() {
                        self.record_to_histogram(&self.queue_wait_histogram, wait.as_millis() as u64);
                    }
                    if let Some(proc) = sample.processing_time() {
                        self.record_to_histogram(&self.processing_histogram, proc.as_millis() as u64);
                    }
                    if let Some(total) = sample.total_time() {
                        self.record_to_histogram(&self.total_latency_histogram, total.as_millis() as u64);
                    }
                }
            }
        }
    }

    /// Record a timeout
    pub fn record_timeout(&self) {
        self.requests_timeout.fetch_add(1, Ordering::Relaxed);
        self.requests_failed.fetch_add(1, Ordering::Relaxed);
    }

    /// Sample current queue depth
    pub fn sample_queue_depth(&self, low: usize, medium: usize, high: usize) {
        let sample = QueueDepthSample {
            timestamp: Instant::now(),
            depth: low + medium + high,
            low_priority: low,
            medium_priority: medium,
            high_priority: high,
        };

        self.current_queue_depth.store(sample.depth, Ordering::Relaxed);

        let mut history = self.queue_depth_history.write();
        history.push_back(sample);

        while history.len() > self.config.max_history_samples {
            history.pop_front();
        }
    }

    /// Sample memory usage
    pub fn sample_memory(&self, used_bytes: u64, total_bytes: u64) {
        if !self.config.collect_memory_usage {
            return;
        }

        let sample = MemorySample {
            timestamp: Instant::now(),
            used_bytes,
            total_bytes,
        };

        let mut history = self.memory_history.write();
        history.push_back(sample);

        while history.len() > self.config.max_history_samples / 10 {
            history.pop_front();
        }
    }

    fn record_to_histogram(&self, histogram: &RwLock<Vec<u64>>, value_ms: u64) {
        // Bucket boundaries in ms: 10, 50, 100, 500, 1000, 5000, 30000, 60000, 300000
        let bucket = match value_ms {
            0..=10 => 0,
            11..=50 => 1,
            51..=100 => 2,
            101..=500 => 3,
            501..=1000 => 4,
            1001..=5000 => 5,
            5001..=30000 => 6,
            30001..=60000 => 7,
            60001..=300000 => 8,
            _ => 9,
        };

        let mut hist = histogram.write();
        if bucket < hist.len() {
            hist[bucket] += 1;
        }
    }

    /// Get elapsed time since test start
    pub fn elapsed(&self) -> Duration {
        self.start_time.elapsed()
    }

    /// Get current statistics summary
    pub fn summary(&self) -> MetricsSummary {
        let submitted = self.requests_submitted.load(Ordering::Relaxed);
        let completed = self.requests_completed.load(Ordering::Relaxed);
        let failed = self.requests_failed.load(Ordering::Relaxed);
        let timeouts = self.requests_timeout.load(Ordering::Relaxed);
        let elapsed = self.elapsed();

        // Calculate latency stats from timing samples
        let (queue_wait_stats, processing_stats, total_latency_stats) = self.calculate_latency_stats();

        // Get queue depth stats
        let (avg_queue_depth, max_queue_depth, peak_queue_time) = self.calculate_queue_stats();

        // Get memory stats
        let (avg_memory_mb, peak_memory_mb) = self.calculate_memory_stats();

        MetricsSummary {
            elapsed_secs: elapsed.as_secs_f64(),
            requests_submitted: submitted,
            requests_completed: completed,
            requests_failed: failed,
            requests_timeout: timeouts,
            requests_pending: submitted - completed - failed,
            success_rate: if submitted > 0 {
                completed as f64 / submitted as f64
            } else {
                0.0
            },
            error_rate: if submitted > 0 {
                failed as f64 / submitted as f64
            } else {
                0.0
            },
            throughput_per_sec: if elapsed.as_secs() > 0 {
                completed as f64 / elapsed.as_secs_f64()
            } else {
                0.0
            },
            current_queue_depth: self.current_queue_depth.load(Ordering::Relaxed),
            active_downloads: self.active_downloads.load(Ordering::Relaxed),
            avg_queue_depth,
            max_queue_depth,
            peak_queue_time_secs: peak_queue_time,
            queue_wait_stats,
            processing_stats,
            total_latency_stats,
            avg_memory_mb,
            peak_memory_mb,
        }
    }

    fn calculate_latency_stats(&self) -> (LatencyStats, LatencyStats, LatencyStats) {
        let samples = self.timing_samples.read();

        let mut queue_waits: Vec<u64> = Vec::new();
        let mut processing_times: Vec<u64> = Vec::new();
        let mut total_times: Vec<u64> = Vec::new();

        for sample in samples.iter() {
            if let Some(wait) = sample.queue_wait_time() {
                queue_waits.push(wait.as_millis() as u64);
            }
            if let Some(proc) = sample.processing_time() {
                processing_times.push(proc.as_millis() as u64);
            }
            if let Some(total) = sample.total_time() {
                total_times.push(total.as_millis() as u64);
            }
        }

        (
            Self::compute_latency_stats(&mut queue_waits),
            Self::compute_latency_stats(&mut processing_times),
            Self::compute_latency_stats(&mut total_times),
        )
    }

    fn compute_latency_stats(values: &mut [u64]) -> LatencyStats {
        if values.is_empty() {
            return LatencyStats::default();
        }

        values.sort_unstable();

        let count = values.len() as u64;
        let sum: u64 = values.iter().sum();
        let min = values[0];
        let max = values[values.len() - 1];

        let p50 = values[values.len() * 50 / 100];
        let p95 = values[values.len() * 95 / 100];
        let p99 = values[values.len().saturating_sub(1).max(values.len() * 99 / 100)];

        LatencyStats {
            count,
            sum_ms: sum,
            min_ms: min,
            max_ms: max,
            p50_ms: p50,
            p95_ms: p95,
            p99_ms: p99,
        }
    }

    fn calculate_queue_stats(&self) -> (f64, usize, f64) {
        let history = self.queue_depth_history.read();

        if history.is_empty() {
            return (0.0, 0, 0.0);
        }

        let total: usize = history.iter().map(|s| s.depth).sum();
        let avg = total as f64 / history.len() as f64;

        let (max_depth, max_sample) = history
            .iter()
            .map(|s| (s.depth, s))
            .max_by_key(|(d, _)| *d)
            .map(|(d, s)| (d, s.timestamp.duration_since(self.start_time).as_secs_f64()))
            .unwrap_or((0, 0.0));

        (avg, max_depth, max_sample)
    }

    fn calculate_memory_stats(&self) -> (f64, f64) {
        let history = self.memory_history.read();

        if history.is_empty() {
            return (0.0, 0.0);
        }

        let total: u64 = history.iter().map(|s| s.used_bytes).sum();
        let avg = (total as f64 / history.len() as f64) / (1024.0 * 1024.0);

        let max = history.iter().map(|s| s.used_bytes).max().unwrap_or(0) as f64 / (1024.0 * 1024.0);

        (avg, max)
    }

    /// Get queue depth history for plotting
    pub fn get_queue_depth_history(&self) -> Vec<(f64, usize)> {
        let history = self.queue_depth_history.read();
        history
            .iter()
            .map(|s| (s.timestamp.duration_since(self.start_time).as_secs_f64(), s.depth))
            .collect()
    }

    /// Reset all metrics
    pub fn reset(&mut self) {
        self.start_time = Instant::now();
        self.requests_submitted.store(0, Ordering::Relaxed);
        self.requests_completed.store(0, Ordering::Relaxed);
        self.requests_failed.store(0, Ordering::Relaxed);
        self.requests_timeout.store(0, Ordering::Relaxed);
        self.current_queue_depth.store(0, Ordering::Relaxed);
        self.active_downloads.store(0, Ordering::Relaxed);
        self.timing_samples.write().clear();
        self.queue_depth_history.write().clear();
        self.memory_history.write().clear();
    }
}

/// Summary of all collected metrics
#[derive(Debug, Clone)]
pub struct MetricsSummary {
    pub elapsed_secs: f64,
    pub requests_submitted: u64,
    pub requests_completed: u64,
    pub requests_failed: u64,
    pub requests_timeout: u64,
    pub requests_pending: u64,
    pub success_rate: f64,
    pub error_rate: f64,
    pub throughput_per_sec: f64,
    pub current_queue_depth: usize,
    pub active_downloads: usize,
    pub avg_queue_depth: f64,
    pub max_queue_depth: usize,
    pub peak_queue_time_secs: f64,
    pub queue_wait_stats: LatencyStats,
    pub processing_stats: LatencyStats,
    pub total_latency_stats: LatencyStats,
    pub avg_memory_mb: f64,
    pub peak_memory_mb: f64,
}

impl MetricsSummary {
    /// Check if test passed based on criteria
    pub fn passes_criteria(&self, criteria: &PassCriteria) -> bool {
        let queue_wait_ok = self.queue_wait_stats.p95_ms <= criteria.max_queue_wait_p95_ms;
        let error_rate_ok = self.error_rate <= criteria.max_error_rate;
        let memory_ok = self.peak_memory_mb <= criteria.max_memory_mb;
        let throughput_ok = self.throughput_per_sec >= criteria.min_throughput_per_sec;

        queue_wait_ok && error_rate_ok && memory_ok && throughput_ok
    }
}

impl std::fmt::Display for MetricsSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            r#"Load Test Results
================
Duration: {:.1}s
Requests: {} submitted, {} completed, {} failed ({} timeouts)
Success Rate: {:.1}%
Error Rate: {:.1}%
Throughput: {:.2} req/s

Queue Stats:
  Current Depth: {}
  Avg Depth: {:.1}
  Max Depth: {} (at {:.1}s)

Latency (Queue Wait):
  Avg: {:.1}ms, P50: {}ms, P95: {}ms, P99: {}ms, Max: {}ms

Latency (Processing):
  Avg: {:.1}ms, P50: {}ms, P95: {}ms, P99: {}ms, Max: {}ms

Latency (Total):
  Avg: {:.1}ms, P50: {}ms, P95: {}ms, P99: {}ms, Max: {}ms

Memory:
  Avg: {:.1} MB, Peak: {:.1} MB
"#,
            self.elapsed_secs,
            self.requests_submitted,
            self.requests_completed,
            self.requests_failed,
            self.requests_timeout,
            self.success_rate * 100.0,
            self.error_rate * 100.0,
            self.throughput_per_sec,
            self.current_queue_depth,
            self.avg_queue_depth,
            self.max_queue_depth,
            self.peak_queue_time_secs,
            self.queue_wait_stats.avg_ms(),
            self.queue_wait_stats.p50_ms,
            self.queue_wait_stats.p95_ms,
            self.queue_wait_stats.p99_ms,
            self.queue_wait_stats.max_ms,
            self.processing_stats.avg_ms(),
            self.processing_stats.p50_ms,
            self.processing_stats.p95_ms,
            self.processing_stats.p99_ms,
            self.processing_stats.max_ms,
            self.total_latency_stats.avg_ms(),
            self.total_latency_stats.p50_ms,
            self.total_latency_stats.p95_ms,
            self.total_latency_stats.p99_ms,
            self.total_latency_stats.max_ms,
            self.avg_memory_mb,
            self.peak_memory_mb,
        )
    }
}

/// Criteria for determining if a test passed
#[derive(Debug, Clone)]
pub struct PassCriteria {
    /// Maximum P95 queue wait time in milliseconds
    pub max_queue_wait_p95_ms: u64,
    /// Maximum error rate (0.0 - 1.0)
    pub max_error_rate: f64,
    /// Maximum memory usage in MB
    pub max_memory_mb: f64,
    /// Minimum throughput in requests per second
    pub min_throughput_per_sec: f64,
}

impl Default for PassCriteria {
    fn default() -> Self {
        Self {
            max_queue_wait_p95_ms: 600_000, // 10 minutes
            max_error_rate: 0.01,           // 1%
            max_memory_mb: 2048.0,          // 2GB
            min_throughput_per_sec: 0.5,    // At least 0.5 req/s
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_basic() {
        let metrics = LoadTestMetrics::with_default_config();

        let id = metrics.record_submit(0);
        assert_eq!(metrics.current_queue_depth.load(Ordering::Relaxed), 1);

        metrics.record_start(id);
        assert_eq!(metrics.current_queue_depth.load(Ordering::Relaxed), 0);
        assert_eq!(metrics.active_downloads.load(Ordering::Relaxed), 1);

        metrics.record_complete(id, true);
        assert_eq!(metrics.active_downloads.load(Ordering::Relaxed), 0);
        assert_eq!(metrics.requests_completed.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_metrics_summary() {
        let metrics = LoadTestMetrics::with_default_config();

        // Submit and complete 10 requests
        for _ in 0..10 {
            let id = metrics.record_submit(0);
            metrics.record_start(id);
            metrics.record_complete(id, true);
        }

        let summary = metrics.summary();
        assert_eq!(summary.requests_submitted, 10);
        assert_eq!(summary.requests_completed, 10);
        assert_eq!(summary.requests_failed, 0);
        assert!(summary.success_rate > 0.99);
    }

    #[test]
    fn test_latency_stats() {
        let mut values = vec![10, 20, 30, 40, 50, 60, 70, 80, 90, 100];
        let stats = LoadTestMetrics::compute_latency_stats(&mut values);

        assert_eq!(stats.count, 10);
        assert_eq!(stats.min_ms, 10);
        assert_eq!(stats.max_ms, 100);
        // For 10 values, p50 is index 5 which is value 60 (median of sorted array)
        assert_eq!(stats.p50_ms, 60);
    }

    #[test]
    fn test_pass_criteria() {
        let summary = MetricsSummary {
            elapsed_secs: 60.0,
            requests_submitted: 100,
            requests_completed: 99,
            requests_failed: 1,
            requests_timeout: 0,
            requests_pending: 0,
            success_rate: 0.99,
            error_rate: 0.01,
            throughput_per_sec: 1.65,
            current_queue_depth: 0,
            active_downloads: 0,
            avg_queue_depth: 5.0,
            max_queue_depth: 20,
            peak_queue_time_secs: 30.0,
            queue_wait_stats: LatencyStats {
                count: 99,
                sum_ms: 50000,
                min_ms: 100,
                max_ms: 1000,
                p50_ms: 400,
                p95_ms: 800,
                p99_ms: 950,
            },
            processing_stats: LatencyStats::default(),
            total_latency_stats: LatencyStats::default(),
            avg_memory_mb: 500.0,
            peak_memory_mb: 800.0,
        };

        let criteria = PassCriteria::default();
        assert!(summary.passes_criteria(&criteria));
    }
}
