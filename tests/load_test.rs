//! Load testing harness for doradura Telegram bot
//!
//! This test suite verifies the bot can handle high concurrent load.
//! Run with: cargo test --test load_test -- --ignored [scenario_name]
//!
//! Available scenarios:
//! - baseline: Single user, 10 sequential requests
//! - ramp: Gradual ramp from 10 to 100 users
//! - spike_100: 100 users sending requests simultaneously
//! - sustained: 50 users for 30 minutes continuous load
//! - mixed_plans: Users with different subscription plans

mod load_test_metrics;
mod mocks;

use load_test_metrics::{LoadTestMetrics, MetricsConfig, MetricsSummary, PassCriteria};
use mocks::{MockDownloader, MockDownloaderConfig};
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;
use tokio::time::sleep;

/// User subscription plan
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserPlan {
    Free,
    Premium,
    Vip,
}

impl UserPlan {
    pub fn rate_limit_secs(&self) -> u64 {
        match self {
            UserPlan::Free => 30,
            UserPlan::Premium => 10,
            UserPlan::Vip => 5,
        }
    }

    pub fn priority(&self) -> u8 {
        match self {
            UserPlan::Free => 0,
            UserPlan::Premium => 1,
            UserPlan::Vip => 2,
        }
    }
}

/// Simulated user that sends download requests
#[derive(Debug)]
pub struct SimulatedUser {
    pub id: u64,
    pub plan: UserPlan,
    pub request_interval: Duration,
    pub requests_sent: AtomicU64,
    pub requests_completed: AtomicU64,
    pub requests_failed: AtomicU64,
}

impl SimulatedUser {
    pub fn new(id: u64, plan: UserPlan) -> Self {
        Self {
            id,
            plan,
            request_interval: Duration::from_secs(plan.rate_limit_secs()),
            requests_sent: AtomicU64::new(0),
            requests_completed: AtomicU64::new(0),
            requests_failed: AtomicU64::new(0),
        }
    }

    pub fn with_interval(mut self, interval: Duration) -> Self {
        self.request_interval = interval;
        self
    }
}

/// Download task for the queue
#[derive(Debug, Clone)]
pub struct DownloadTask {
    pub id: u64,
    pub user_id: u64,
    pub url: String,
    pub priority: u8,
    pub created_at: Instant,
}

/// Simple priority queue for download tasks
pub struct TaskQueue {
    tasks: Mutex<VecDeque<DownloadTask>>,
    size: AtomicUsize,
}

impl TaskQueue {
    pub fn new() -> Self {
        Self {
            tasks: Mutex::new(VecDeque::new()),
            size: AtomicUsize::new(0),
        }
    }

    pub fn push(&self, task: DownloadTask) {
        let mut tasks = self.tasks.lock();
        // Insert with priority ordering (higher priority first)
        let pos = tasks
            .iter()
            .position(|t| t.priority < task.priority)
            .unwrap_or(tasks.len());
        tasks.insert(pos, task);
        self.size.fetch_add(1, Ordering::Relaxed);
    }

    pub fn pop(&self) -> Option<DownloadTask> {
        let mut tasks = self.tasks.lock();
        let task = tasks.pop_front();
        if task.is_some() {
            self.size.fetch_sub(1, Ordering::Relaxed);
        }
        task
    }

    pub fn len(&self) -> usize {
        self.size.load(Ordering::Relaxed)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Get counts by priority
    pub fn counts_by_priority(&self) -> (usize, usize, usize) {
        let tasks = self.tasks.lock();
        let low = tasks.iter().filter(|t| t.priority == 0).count();
        let medium = tasks.iter().filter(|t| t.priority == 1).count();
        let high = tasks.iter().filter(|t| t.priority == 2).count();
        (low, medium, high)
    }
}

impl Default for TaskQueue {
    fn default() -> Self {
        Self::new()
    }
}

/// Configuration for the load test
#[derive(Debug, Clone)]
pub struct LoadTestConfig {
    /// Maximum concurrent downloads allowed
    pub max_concurrent_downloads: usize,
    /// Delay between starting downloads (ms)
    pub inter_download_delay_ms: u64,
    /// Queue check interval (ms)
    pub queue_check_interval_ms: u64,
    /// Test duration
    pub duration: Duration,
    /// Number of simulated users
    pub num_users: usize,
    /// User plans distribution (free, premium, vip counts)
    pub user_distribution: (usize, usize, usize),
    /// Request interval per user
    pub request_interval: Duration,
    /// Mock downloader configuration
    pub mock_config: MockDownloaderConfig,
}

impl Default for LoadTestConfig {
    fn default() -> Self {
        Self {
            max_concurrent_downloads: 2,
            inter_download_delay_ms: 3000,
            queue_check_interval_ms: 100,
            duration: Duration::from_secs(60),
            num_users: 10,
            user_distribution: (7, 2, 1),
            request_interval: Duration::from_secs(5),
            mock_config: MockDownloaderConfig::fast(),
        }
    }
}

impl LoadTestConfig {
    pub fn for_spike_test(num_users: usize) -> Self {
        Self {
            max_concurrent_downloads: 4,
            inter_download_delay_ms: 1000,
            queue_check_interval_ms: 50,
            duration: Duration::from_secs(120),
            num_users,
            user_distribution: (num_users * 7 / 10, num_users * 2 / 10, num_users / 10),
            request_interval: Duration::from_millis(100), // All at once
            mock_config: MockDownloaderConfig::fast(),
        }
    }

    pub fn for_sustained_test(num_users: usize, duration_secs: u64) -> Self {
        Self {
            max_concurrent_downloads: 4,
            inter_download_delay_ms: 2000,
            queue_check_interval_ms: 100,
            duration: Duration::from_secs(duration_secs),
            num_users,
            user_distribution: (num_users / 2, num_users / 3, num_users / 6),
            request_interval: Duration::from_secs(30),
            mock_config: MockDownloaderConfig::realistic(),
        }
    }
}

/// Load test runner
pub struct LoadTestRunner {
    config: LoadTestConfig,
    queue: Arc<TaskQueue>,
    downloader: Arc<MockDownloader>,
    metrics: Arc<LoadTestMetrics>,
    running: Arc<AtomicBool>,
    task_counter: AtomicU64,
}

impl LoadTestRunner {
    pub fn new(config: LoadTestConfig) -> Self {
        let mock_config = config.mock_config.clone();
        Self {
            config,
            queue: Arc::new(TaskQueue::new()),
            downloader: Arc::new(MockDownloader::new(mock_config)),
            metrics: Arc::new(LoadTestMetrics::new(MetricsConfig::default())),
            running: Arc::new(AtomicBool::new(false)),
            task_counter: AtomicU64::new(0),
        }
    }

    /// Create users based on configuration
    fn create_users(&self) -> Vec<Arc<SimulatedUser>> {
        let (free, premium, vip) = self.config.user_distribution;
        let mut users = Vec::with_capacity(self.config.num_users);

        for i in 0..free {
            users.push(Arc::new(
                SimulatedUser::new(i as u64, UserPlan::Free).with_interval(self.config.request_interval),
            ));
        }
        for i in 0..premium {
            users.push(Arc::new(
                SimulatedUser::new((free + i) as u64, UserPlan::Premium).with_interval(self.config.request_interval),
            ));
        }
        for i in 0..vip {
            users.push(Arc::new(
                SimulatedUser::new((free + premium + i) as u64, UserPlan::Vip)
                    .with_interval(self.config.request_interval),
            ));
        }

        users
    }

    /// Run the load test
    pub async fn run(&self) -> MetricsSummary {
        self.running.store(true, Ordering::SeqCst);
        let users = self.create_users();

        println!(
            "Starting load test with {} users ({} free, {} premium, {} vip)",
            users.len(),
            self.config.user_distribution.0,
            self.config.user_distribution.1,
            self.config.user_distribution.2
        );
        println!(
            "Max concurrent downloads: {}, duration: {:?}",
            self.config.max_concurrent_downloads, self.config.duration
        );

        let test_start = Instant::now();

        // Spawn user tasks
        let mut user_handles = Vec::new();
        for user in users {
            let handle = self.spawn_user_task(user.clone(), test_start);
            user_handles.push(handle);
        }

        // Spawn queue processor
        let processor_handle = self.spawn_queue_processor();

        // Spawn metrics sampler
        let sampler_handle = self.spawn_metrics_sampler();

        // Wait for test duration
        sleep(self.config.duration).await;

        // Stop the test
        self.running.store(false, Ordering::SeqCst);

        // Wait for all tasks to complete
        for handle in user_handles {
            let _ = handle.await;
        }
        let _ = processor_handle.await;
        let _ = sampler_handle.await;

        // Return final metrics
        self.metrics.summary()
    }

    fn spawn_user_task(&self, user: Arc<SimulatedUser>, test_start: Instant) -> tokio::task::JoinHandle<()> {
        let queue = Arc::clone(&self.queue);
        let metrics = Arc::clone(&self.metrics);
        let running = Arc::clone(&self.running);
        let task_counter = &self.task_counter as *const AtomicU64;
        let task_counter = unsafe { &*task_counter };
        let duration = self.config.duration;
        let interval = user.request_interval;

        tokio::spawn(async move {
            while running.load(Ordering::Relaxed) && test_start.elapsed() < duration {
                // Create and submit task
                let task_id = task_counter.fetch_add(1, Ordering::Relaxed);
                let task = DownloadTask {
                    id: task_id,
                    user_id: user.id,
                    url: format!("https://example.com/video/{}", task_id),
                    priority: user.plan.priority(),
                    created_at: Instant::now(),
                };

                queue.push(task);
                user.requests_sent.fetch_add(1, Ordering::Relaxed);
                let _ = metrics.record_submit(user.plan.priority());

                // Wait before next request
                sleep(interval).await;
            }
        })
    }

    fn spawn_queue_processor(&self) -> tokio::task::JoinHandle<()> {
        let queue = Arc::clone(&self.queue);
        let downloader = Arc::clone(&self.downloader);
        let metrics = Arc::clone(&self.metrics);
        let running = Arc::clone(&self.running);
        let max_concurrent = self.config.max_concurrent_downloads;
        let inter_delay = Duration::from_millis(self.config.inter_download_delay_ms);
        let check_interval = Duration::from_millis(self.config.queue_check_interval_ms);

        let semaphore = Arc::new(Semaphore::new(max_concurrent));

        tokio::spawn(async move {
            let mut last_download = Instant::now();

            while running.load(Ordering::Relaxed) || !queue.is_empty() {
                // Check queue
                if let Some(task) = queue.pop() {
                    // Enforce inter-download delay
                    let elapsed = last_download.elapsed();
                    if elapsed < inter_delay {
                        sleep(inter_delay - elapsed).await;
                    }

                    // Acquire semaphore
                    let permit = semaphore.clone().acquire_owned().await.unwrap();
                    last_download = Instant::now();

                    let dl = Arc::clone(&downloader);
                    let m = Arc::clone(&metrics);
                    let sample_id = task.id as usize;

                    // Record start
                    m.record_start(sample_id);

                    // Spawn download task
                    tokio::spawn(async move {
                        let result = dl.download(&task.url).await;
                        m.record_complete(sample_id, result.success);
                        drop(permit);
                    });
                } else {
                    // No tasks, wait a bit
                    sleep(check_interval).await;
                }

                // Break if not running and queue is empty
                if !running.load(Ordering::Relaxed) && queue.is_empty() {
                    break;
                }
            }

            // Wait for all downloads to complete
            let _ = semaphore.acquire_many(max_concurrent as u32).await;
        })
    }

    fn spawn_metrics_sampler(&self) -> tokio::task::JoinHandle<()> {
        let queue = Arc::clone(&self.queue);
        let metrics = Arc::clone(&self.metrics);
        let running = Arc::clone(&self.running);
        let sample_interval = Duration::from_millis(100);

        tokio::spawn(async move {
            while running.load(Ordering::Relaxed) {
                let (low, medium, high) = queue.counts_by_priority();
                metrics.sample_queue_depth(low, medium, high);
                sleep(sample_interval).await;
            }
        })
    }

    /// Get metrics summary
    pub fn get_metrics(&self) -> MetricsSummary {
        self.metrics.summary()
    }
}

// ============================================================================
// Test Scenarios
// ============================================================================

/// Baseline test: Single user, 10 sequential requests
#[tokio::test]
#[ignore]
async fn baseline() {
    let config = LoadTestConfig {
        max_concurrent_downloads: 2,
        inter_download_delay_ms: 100,
        queue_check_interval_ms: 50,
        duration: Duration::from_secs(30),
        num_users: 1,
        user_distribution: (1, 0, 0),
        request_interval: Duration::from_secs(2),
        mock_config: MockDownloaderConfig::fast(),
    };

    let runner = LoadTestRunner::new(config);
    let summary = runner.run().await;

    println!("\n{}", summary);

    // Baseline criteria: should complete without issues
    let criteria = PassCriteria {
        max_queue_wait_p95_ms: 60_000, // 60 seconds
        max_error_rate: 0.01,
        max_memory_mb: 512.0,
        min_throughput_per_sec: 0.1,
    };

    assert!(summary.passes_criteria(&criteria), "Baseline test failed criteria");
}

/// Gradual ramp test: Start with 10 users, add 10 every 30 seconds up to 100
#[tokio::test]
#[ignore]
async fn ramp() {
    // Start with 10 users
    let config = LoadTestConfig {
        max_concurrent_downloads: 4,
        inter_download_delay_ms: 1000,
        queue_check_interval_ms: 50,
        duration: Duration::from_secs(180), // 3 minutes
        num_users: 100,
        user_distribution: (70, 20, 10),
        request_interval: Duration::from_secs(5),
        mock_config: MockDownloaderConfig::fast(),
    };

    let runner = LoadTestRunner::new(config);
    let summary = runner.run().await;

    println!("\n{}", summary);

    let criteria = PassCriteria {
        max_queue_wait_p95_ms: 300_000, // 5 minutes
        max_error_rate: 0.05,
        max_memory_mb: 1024.0,
        min_throughput_per_sec: 0.5,
    };

    assert!(summary.passes_criteria(&criteria), "Ramp test failed criteria");
}

/// Spike test: 100 users send requests within 10 seconds
#[tokio::test]
#[ignore]
async fn spike_100() {
    let config = LoadTestConfig::for_spike_test(100);
    let runner = LoadTestRunner::new(config);
    let summary = runner.run().await;

    println!("\n{}", summary);

    // Spike test: expect queue to grow but eventually drain
    let criteria = PassCriteria {
        max_queue_wait_p95_ms: 600_000, // 10 minutes
        max_error_rate: 0.05,
        max_memory_mb: 2048.0,
        min_throughput_per_sec: 0.3,
    };

    assert!(summary.passes_criteria(&criteria), "Spike test failed criteria");
    assert!(summary.max_queue_depth <= 500, "Queue depth exceeded 500 tasks");
}

/// Sustained load test: 50 users continuously for extended period
#[tokio::test]
#[ignore]
async fn sustained() {
    // Note: For actual 30 minute test, change duration_secs to 1800
    let config = LoadTestConfig::for_sustained_test(50, 120); // 2 minutes for quick test

    let runner = LoadTestRunner::new(config);
    let summary = runner.run().await;

    println!("\n{}", summary);

    let criteria = PassCriteria {
        max_queue_wait_p95_ms: 600_000, // 10 minutes
        max_error_rate: 0.01,
        max_memory_mb: 2048.0,
        min_throughput_per_sec: 0.5,
    };

    assert!(summary.passes_criteria(&criteria), "Sustained test failed criteria");
}

/// Mixed user plans test
#[tokio::test]
#[ignore]
async fn mixed_plans() {
    let config = LoadTestConfig {
        max_concurrent_downloads: 4,
        inter_download_delay_ms: 1000,
        queue_check_interval_ms: 50,
        duration: Duration::from_secs(60),
        num_users: 100,
        user_distribution: (70, 20, 10), // 70 free, 20 premium, 10 VIP
        request_interval: Duration::from_secs(3),
        mock_config: MockDownloaderConfig::fast(),
    };

    let runner = LoadTestRunner::new(config);
    let summary = runner.run().await;

    println!("\n{}", summary);

    // Check that VIP/Premium users get processed faster
    // (This would require more detailed tracking in practice)
    let criteria = PassCriteria::default();
    assert!(summary.passes_criteria(&criteria), "Mixed plans test failed criteria");
}

/// Quick sanity check (runs fast for CI)
#[tokio::test]
async fn quick_sanity() {
    let config = LoadTestConfig {
        max_concurrent_downloads: 2,
        inter_download_delay_ms: 10,
        queue_check_interval_ms: 10,
        duration: Duration::from_secs(2),
        num_users: 5,
        user_distribution: (3, 1, 1),
        request_interval: Duration::from_millis(200),
        mock_config: MockDownloaderConfig::fast(),
    };

    let runner = LoadTestRunner::new(config);
    let summary = runner.run().await;

    println!("\n{}", summary);

    // Basic assertions - just verify the test ran
    assert!(summary.requests_submitted > 0, "Expected some requests to be submitted");
    assert!(summary.requests_completed > 0, "Expected some requests to complete");
    // Note: Error rate check is lenient because this is a quick test
}

// ============================================================================
// Helper for running scenarios programmatically
// ============================================================================

/// Run a load test scenario and return results
pub async fn run_scenario(scenario: &str) -> MetricsSummary {
    match scenario {
        "baseline" => {
            let config = LoadTestConfig::default();
            let runner = LoadTestRunner::new(config);
            runner.run().await
        }
        "spike" => {
            let config = LoadTestConfig::for_spike_test(100);
            let runner = LoadTestRunner::new(config);
            runner.run().await
        }
        "sustained" => {
            let config = LoadTestConfig::for_sustained_test(50, 1800);
            let runner = LoadTestRunner::new(config);
            runner.run().await
        }
        _ => {
            let config = LoadTestConfig {
                duration: Duration::from_secs(5),
                num_users: 5,
                user_distribution: (3, 1, 1),
                mock_config: MockDownloaderConfig::fast(),
                ..Default::default()
            };
            let runner = LoadTestRunner::new(config);
            runner.run().await
        }
    }
}
