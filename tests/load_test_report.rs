//! Load test report generator
//!
//! Generates markdown reports with test results, bottleneck identification,
//! and recommendations for configuration tuning.

use std::fmt::Write as FmtWrite;
use std::fs;
use std::io::Write;
use std::path::Path;

/// Represents a single test run result
#[derive(Debug, Clone)]
pub struct TestRunResult {
    pub scenario_name: String,
    pub config: TestConfig,
    pub metrics: TestMetrics,
    pub passed: bool,
    pub bottlenecks: Vec<Bottleneck>,
    pub recommendations: Vec<String>,
}

/// Test configuration summary
#[derive(Debug, Clone)]
pub struct TestConfig {
    pub max_concurrent_downloads: usize,
    pub inter_download_delay_ms: u64,
    pub num_users: usize,
    pub user_distribution: String,
    pub test_duration_secs: u64,
}

/// Collected metrics from test run
#[derive(Debug, Clone)]
pub struct TestMetrics {
    pub requests_submitted: u64,
    pub requests_completed: u64,
    pub requests_failed: u64,
    pub success_rate: f64,
    pub error_rate: f64,
    pub throughput_per_sec: f64,
    pub max_queue_depth: usize,
    pub avg_queue_depth: f64,
    pub queue_wait_p50_ms: u64,
    pub queue_wait_p95_ms: u64,
    pub queue_wait_p99_ms: u64,
    pub processing_p50_ms: u64,
    pub processing_p95_ms: u64,
    pub total_latency_p95_ms: u64,
    pub peak_memory_mb: f64,
}

/// Identified bottleneck
#[derive(Debug, Clone)]
pub struct Bottleneck {
    pub component: String,
    pub severity: Severity,
    pub description: String,
    pub impact: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Low,
    Medium,
    High,
    Critical,
}

impl Severity {
    pub fn emoji(&self) -> &'static str {
        match self {
            Severity::Low => "🟢",
            Severity::Medium => "🟡",
            Severity::High => "🟠",
            Severity::Critical => "🔴",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Severity::Low => "Low",
            Severity::Medium => "Medium",
            Severity::High => "High",
            Severity::Critical => "Critical",
        }
    }
}

/// Report generator
pub struct ReportGenerator {
    results: Vec<TestRunResult>,
}

impl ReportGenerator {
    pub fn new() -> Self {
        Self { results: Vec::new() }
    }

    pub fn add_result(&mut self, result: TestRunResult) {
        self.results.push(result);
    }

    /// Analyze metrics and identify bottlenecks
    pub fn analyze_bottlenecks(metrics: &TestMetrics, config: &TestConfig) -> Vec<Bottleneck> {
        let mut bottlenecks = Vec::new();

        // Check queue depth
        if metrics.max_queue_depth > 100 {
            let severity = if metrics.max_queue_depth > 500 {
                Severity::Critical
            } else if metrics.max_queue_depth > 200 {
                Severity::High
            } else {
                Severity::Medium
            };

            bottlenecks.push(Bottleneck {
                component: "Download Queue".to_string(),
                severity,
                description: format!(
                    "Queue depth reached {} tasks (configured: {} concurrent)",
                    metrics.max_queue_depth, config.max_concurrent_downloads
                ),
                impact: "Long wait times for users, potential memory pressure".to_string(),
            });
        }

        // Check queue wait time
        if metrics.queue_wait_p95_ms > 300_000 {
            // 5 minutes
            let severity = if metrics.queue_wait_p95_ms > 600_000 {
                Severity::Critical
            } else {
                Severity::High
            };

            bottlenecks.push(Bottleneck {
                component: "Queue Processing".to_string(),
                severity,
                description: format!(
                    "P95 queue wait time: {:.1} minutes",
                    metrics.queue_wait_p95_ms as f64 / 60_000.0
                ),
                impact: "Poor user experience, timeout risk".to_string(),
            });
        }

        // Check throughput
        let expected_throughput = config.num_users as f64 / 30.0; // Assuming 30s rate limit
        if metrics.throughput_per_sec < expected_throughput * 0.5 {
            bottlenecks.push(Bottleneck {
                component: "Download Throughput".to_string(),
                severity: Severity::High,
                description: format!(
                    "Actual throughput ({:.2} req/s) is less than 50% of expected ({:.2} req/s)",
                    metrics.throughput_per_sec, expected_throughput
                ),
                impact: "Cannot keep up with incoming requests".to_string(),
            });
        }

        // Check error rate
        if metrics.error_rate > 0.01 {
            let severity = if metrics.error_rate > 0.05 {
                Severity::Critical
            } else if metrics.error_rate > 0.02 {
                Severity::High
            } else {
                Severity::Medium
            };

            bottlenecks.push(Bottleneck {
                component: "Error Handling".to_string(),
                severity,
                description: format!("Error rate: {:.1}%", metrics.error_rate * 100.0),
                impact: "Failed downloads, poor user experience".to_string(),
            });
        }

        // Check memory usage
        if metrics.peak_memory_mb > 1500.0 {
            let severity = if metrics.peak_memory_mb > 2000.0 {
                Severity::Critical
            } else {
                Severity::High
            };

            bottlenecks.push(Bottleneck {
                component: "Memory Usage".to_string(),
                severity,
                description: format!("Peak memory: {:.0} MB", metrics.peak_memory_mb),
                impact: "Risk of OOM on Railway (2GB limit)".to_string(),
            });
        }

        // Check concurrent download limit
        if metrics.max_queue_depth > config.max_concurrent_downloads * 50 {
            bottlenecks.push(Bottleneck {
                component: "Concurrent Downloads".to_string(),
                severity: Severity::Medium,
                description: format!(
                    "Only {} concurrent downloads for {} queued tasks",
                    config.max_concurrent_downloads, metrics.max_queue_depth
                ),
                impact: "Consider increasing concurrent downloads".to_string(),
            });
        }

        bottlenecks
    }

    /// Generate recommendations based on bottlenecks
    pub fn generate_recommendations(bottlenecks: &[Bottleneck], config: &TestConfig) -> Vec<String> {
        let mut recommendations = Vec::new();

        for bottleneck in bottlenecks {
            match bottleneck.component.as_str() {
                "Download Queue" | "Queue Processing" => {
                    if config.max_concurrent_downloads < 8 {
                        recommendations.push(format!(
                            "Increase `MAX_CONCURRENT_DOWNLOADS` from {} to {} (set QUEUE_MAX_CONCURRENT={})",
                            config.max_concurrent_downloads,
                            config.max_concurrent_downloads * 2,
                            config.max_concurrent_downloads * 2
                        ));
                    }
                    if config.inter_download_delay_ms > 1000 {
                        recommendations.push(format!(
                            "Reduce `INTER_DOWNLOAD_DELAY_MS` from {} to {} (set QUEUE_INTER_DOWNLOAD_DELAY_MS={})",
                            config.inter_download_delay_ms,
                            config.inter_download_delay_ms / 2,
                            config.inter_download_delay_ms / 2
                        ));
                    }
                }
                "Download Throughput" => {
                    recommendations.push("Consider using proxy rotation to avoid rate limiting".to_string());
                    recommendations.push("Enable parallel downloads for playlist items".to_string());
                }
                "Error Handling" => {
                    recommendations.push("Review yt-dlp error logs for common failure patterns".to_string());
                    recommendations.push("Consider implementing retry logic for transient failures".to_string());
                }
                "Memory Usage" => {
                    recommendations.push("Implement streaming download instead of buffering entire file".to_string());
                    recommendations.push("Reduce queue buffer sizes".to_string());
                    recommendations.push("Consider file cleanup more aggressively".to_string());
                }
                _ => {}
            }
        }

        // Deduplicate
        recommendations.sort();
        recommendations.dedup();
        recommendations
    }

    /// Generate markdown report
    pub fn generate_markdown(&self) -> String {
        let mut report = String::new();

        writeln!(report, "# Load Test Report").unwrap();
        writeln!(report, "\nGenerated: {}", chrono_lite_now()).unwrap();
        writeln!(report).unwrap();

        // Executive Summary
        writeln!(report, "## Executive Summary\n").unwrap();

        let passed = self.results.iter().filter(|r| r.passed).count();
        let total = self.results.len();

        if passed == total {
            writeln!(report, "✅ **All {} test scenarios passed**\n", total).unwrap();
        } else {
            writeln!(report, "⚠️ **{}/{} test scenarios passed**\n", passed, total).unwrap();
        }

        // Results Table
        writeln!(
            report,
            "| Scenario | Users | Throughput | P95 Wait | Max Queue | Status |"
        )
        .unwrap();
        writeln!(
            report,
            "|----------|-------|------------|----------|-----------|--------|"
        )
        .unwrap();

        for result in &self.results {
            let status = if result.passed { "✅ Pass" } else { "❌ Fail" };
            writeln!(
                report,
                "| {} | {} | {:.2}/s | {:.1}m | {} | {} |",
                result.scenario_name,
                result.config.num_users,
                result.metrics.throughput_per_sec,
                result.metrics.queue_wait_p95_ms as f64 / 60_000.0,
                result.metrics.max_queue_depth,
                status
            )
            .unwrap();
        }
        writeln!(report).unwrap();

        // Detailed Results per scenario
        for result in &self.results {
            writeln!(report, "## {}\n", result.scenario_name).unwrap();

            // Configuration
            writeln!(report, "### Configuration\n").unwrap();
            writeln!(report, "| Setting | Value |").unwrap();
            writeln!(report, "|---------|-------|").unwrap();
            writeln!(
                report,
                "| Max Concurrent Downloads | {} |",
                result.config.max_concurrent_downloads
            )
            .unwrap();
            writeln!(
                report,
                "| Inter-Download Delay | {}ms |",
                result.config.inter_download_delay_ms
            )
            .unwrap();
            writeln!(report, "| Users | {} |", result.config.num_users).unwrap();
            writeln!(report, "| User Distribution | {} |", result.config.user_distribution).unwrap();
            writeln!(report, "| Test Duration | {}s |", result.config.test_duration_secs).unwrap();
            writeln!(report).unwrap();

            // Metrics
            writeln!(report, "### Metrics\n").unwrap();
            writeln!(report, "#### Request Counts\n").unwrap();
            writeln!(report, "- Submitted: {}", result.metrics.requests_submitted).unwrap();
            writeln!(
                report,
                "- Completed: {} ({:.1}%)",
                result.metrics.requests_completed,
                result.metrics.success_rate * 100.0
            )
            .unwrap();
            writeln!(
                report,
                "- Failed: {} ({:.1}%)",
                result.metrics.requests_failed,
                result.metrics.error_rate * 100.0
            )
            .unwrap();
            writeln!(report, "- Throughput: {:.2} req/s\n", result.metrics.throughput_per_sec).unwrap();

            writeln!(report, "#### Queue Stats\n").unwrap();
            writeln!(report, "- Max Depth: {}", result.metrics.max_queue_depth).unwrap();
            writeln!(report, "- Avg Depth: {:.1}", result.metrics.avg_queue_depth).unwrap();
            writeln!(report).unwrap();

            writeln!(report, "#### Latency\n").unwrap();
            writeln!(report, "| Metric | P50 | P95 | P99 |").unwrap();
            writeln!(report, "|--------|-----|-----|-----|").unwrap();
            writeln!(
                report,
                "| Queue Wait | {}ms | {}ms | {}ms |",
                result.metrics.queue_wait_p50_ms, result.metrics.queue_wait_p95_ms, result.metrics.queue_wait_p99_ms
            )
            .unwrap();
            writeln!(
                report,
                "| Processing | {}ms | {}ms | - |",
                result.metrics.processing_p50_ms, result.metrics.processing_p95_ms
            )
            .unwrap();
            writeln!(report).unwrap();

            // Bottlenecks
            if !result.bottlenecks.is_empty() {
                writeln!(report, "### Bottlenecks Identified\n").unwrap();
                for bottleneck in &result.bottlenecks {
                    writeln!(
                        report,
                        "{} **{}** ({}): {}",
                        bottleneck.severity.emoji(),
                        bottleneck.component,
                        bottleneck.severity.label(),
                        bottleneck.description
                    )
                    .unwrap();
                    writeln!(report, "  - Impact: {}\n", bottleneck.impact).unwrap();
                }
            }

            // Recommendations
            if !result.recommendations.is_empty() {
                writeln!(report, "### Recommendations\n").unwrap();
                for (i, rec) in result.recommendations.iter().enumerate() {
                    writeln!(report, "{}. {}", i + 1, rec).unwrap();
                }
                writeln!(report).unwrap();
            }
        }

        // Global Recommendations
        let all_bottlenecks: Vec<_> = self.results.iter().flat_map(|r| r.bottlenecks.iter()).collect();

        if !all_bottlenecks.is_empty() {
            writeln!(report, "## Configuration Recommendations\n").unwrap();
            writeln!(
                report,
                "Based on all test scenarios, here are the recommended configuration changes:\n"
            )
            .unwrap();

            // Count critical issues
            let critical_count = all_bottlenecks
                .iter()
                .filter(|b| b.severity == Severity::Critical)
                .count();

            if critical_count > 0 {
                writeln!(
                    report,
                    "⚠️ **{} critical issues found** - address these before production deployment.\n",
                    critical_count
                )
                .unwrap();
            }

            // Suggested config values
            writeln!(report, "### Suggested Environment Variables\n").unwrap();
            writeln!(report, "```bash").unwrap();
            writeln!(report, "# Increase concurrent downloads (default: 2)").unwrap();
            writeln!(report, "QUEUE_MAX_CONCURRENT=4").unwrap();
            writeln!(report).unwrap();
            writeln!(report, "# Reduce inter-download delay (default: 3000)").unwrap();
            writeln!(report, "QUEUE_INTER_DOWNLOAD_DELAY_MS=1500").unwrap();
            writeln!(report).unwrap();
            writeln!(report, "# Faster queue checks (default: 100)").unwrap();
            writeln!(report, "QUEUE_CHECK_INTERVAL_MS=50").unwrap();
            writeln!(report, "```\n").unwrap();
        }

        report
    }

    /// Save report to file
    pub fn save_to_file(&self, path: &Path) -> std::io::Result<()> {
        let report = self.generate_markdown();
        let mut file = fs::File::create(path)?;
        file.write_all(report.as_bytes())?;
        Ok(())
    }
}

impl Default for ReportGenerator {
    fn default() -> Self {
        Self::new()
    }
}

/// Simple timestamp without chrono dependency
fn chrono_lite_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let duration = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();

    let secs = duration.as_secs();
    // Simple UTC timestamp (not accurate but good enough for reports)
    let days_since_epoch = secs / 86400;
    let years = 1970 + (days_since_epoch / 365);
    let days_in_year = days_since_epoch % 365;
    let month = days_in_year / 30 + 1;
    let day = days_in_year % 30 + 1;
    let hours = (secs % 86400) / 3600;
    let minutes = (secs % 3600) / 60;

    format!("{:04}-{:02}-{:02} {:02}:{:02} UTC", years, month, day, hours, minutes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_metrics() -> TestMetrics {
        TestMetrics {
            requests_submitted: 1000,
            requests_completed: 980,
            requests_failed: 20,
            success_rate: 0.98,
            error_rate: 0.02,
            throughput_per_sec: 1.5,
            max_queue_depth: 150,
            avg_queue_depth: 45.0,
            queue_wait_p50_ms: 30_000,
            queue_wait_p95_ms: 180_000,
            queue_wait_p99_ms: 300_000,
            processing_p50_ms: 2000,
            processing_p95_ms: 5000,
            total_latency_p95_ms: 185_000,
            peak_memory_mb: 800.0,
        }
    }

    fn sample_config() -> TestConfig {
        TestConfig {
            max_concurrent_downloads: 2,
            inter_download_delay_ms: 3000,
            num_users: 100,
            user_distribution: "70 free, 20 premium, 10 VIP".to_string(),
            test_duration_secs: 600,
        }
    }

    #[test]
    fn test_analyze_bottlenecks() {
        let metrics = sample_metrics();
        let config = sample_config();

        let bottlenecks = ReportGenerator::analyze_bottlenecks(&metrics, &config);

        assert!(!bottlenecks.is_empty());

        // Should identify queue depth issue
        let queue_issue = bottlenecks.iter().find(|b| b.component == "Download Queue");
        assert!(queue_issue.is_some());
    }

    #[test]
    fn test_generate_recommendations() {
        let bottlenecks = vec![Bottleneck {
            component: "Download Queue".to_string(),
            severity: Severity::High,
            description: "Queue depth too high".to_string(),
            impact: "Long wait times".to_string(),
        }];

        let config = sample_config();
        let recommendations = ReportGenerator::generate_recommendations(&bottlenecks, &config);

        assert!(!recommendations.is_empty());
    }

    #[test]
    fn test_generate_markdown() {
        let metrics = sample_metrics();
        let config = sample_config();
        let bottlenecks = ReportGenerator::analyze_bottlenecks(&metrics, &config);
        let recommendations = ReportGenerator::generate_recommendations(&bottlenecks, &config);

        let result = TestRunResult {
            scenario_name: "spike_100".to_string(),
            config,
            metrics,
            passed: true,
            bottlenecks,
            recommendations,
        };

        let mut generator = ReportGenerator::new();
        generator.add_result(result);

        let report = generator.generate_markdown();

        assert!(report.contains("# Load Test Report"));
        assert!(report.contains("spike_100"));
        assert!(report.contains("Executive Summary"));
    }

    #[test]
    fn test_severity_emoji() {
        assert_eq!(Severity::Low.emoji(), "🟢");
        assert_eq!(Severity::Medium.emoji(), "🟡");
        assert_eq!(Severity::High.emoji(), "🟠");
        assert_eq!(Severity::Critical.emoji(), "🔴");
    }
}
