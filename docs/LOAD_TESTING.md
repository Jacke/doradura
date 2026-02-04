# Load Testing Guide

This document describes the load testing infrastructure for doradura Telegram bot.

## Overview

The load testing suite verifies the bot can handle concurrent users requesting downloads. It simulates user behavior, measures queue performance, and identifies bottlenecks.

## Quick Start

### Run Quick Sanity Check
```bash
cargo test --test load_test quick_sanity
```

### Run All Load Tests
```bash
cargo test --test load_test -- --ignored
```

### Run Specific Scenario
```bash
# Baseline test (single user)
cargo test --test load_test -- --ignored baseline

# Spike test (100 users simultaneously)
cargo test --test load_test -- --ignored spike_100

# Sustained load (50 users for extended period)
cargo test --test load_test -- --ignored sustained

# Mixed user plans
cargo test --test load_test -- --ignored mixed_plans
```

## Test Scenarios

### 1. Baseline
- **Users**: 1
- **Duration**: 30 seconds
- **Purpose**: Establish baseline performance metrics
- **Pass Criteria**: P95 queue wait < 60s, error rate < 1%

### 2. Ramp
- **Users**: 10 → 100 (gradual increase)
- **Duration**: 3 minutes
- **Purpose**: Find the breaking point as load increases
- **Pass Criteria**: P95 queue wait < 5m, error rate < 5%

### 3. Spike (spike_100)
- **Users**: 100 simultaneous
- **Duration**: 2 minutes
- **Purpose**: Test system behavior under sudden load
- **Pass Criteria**: Queue depth < 500, P95 wait < 10m, error rate < 5%

### 4. Sustained
- **Users**: 50 continuous
- **Duration**: 2 minutes (configurable to 30 minutes)
- **Purpose**: Detect memory leaks and connection exhaustion
- **Pass Criteria**: Stable memory, no connection timeouts

### 5. Mixed Plans
- **Users**: 100 (70 free, 20 premium, 10 VIP)
- **Duration**: 1 minute
- **Purpose**: Verify priority queue correctness
- **Pass Criteria**: VIP users processed faster than free

## Configuration Tuning

The load tests can help find optimal configuration values.

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `QUEUE_MAX_CONCURRENT` | 2 | Maximum concurrent downloads |
| `QUEUE_INTER_DOWNLOAD_DELAY_MS` | 3000 | Delay between starting downloads |
| `QUEUE_CHECK_INTERVAL_MS` | 100 | Queue polling interval |

### Testing Different Configurations

```bash
# Test with 4 concurrent downloads
QUEUE_MAX_CONCURRENT=4 cargo test --test load_test -- --ignored spike_100

# Test with faster processing
QUEUE_MAX_CONCURRENT=8 QUEUE_INTER_DOWNLOAD_DELAY_MS=1000 \
  cargo test --test load_test -- --ignored sustained
```

### Recommended Settings by Environment

#### Development (local)
```bash
QUEUE_MAX_CONCURRENT=4
QUEUE_INTER_DOWNLOAD_DELAY_MS=1000
QUEUE_CHECK_INTERVAL_MS=50
```

#### Production (Railway, 2GB RAM)
```bash
QUEUE_MAX_CONCURRENT=2  # Conservative to avoid YouTube rate limiting
QUEUE_INTER_DOWNLOAD_DELAY_MS=3000
QUEUE_CHECK_INTERVAL_MS=100
```

#### High-Traffic Production (dedicated server)
```bash
QUEUE_MAX_CONCURRENT=8
QUEUE_INTER_DOWNLOAD_DELAY_MS=1500
QUEUE_CHECK_INTERVAL_MS=50
```

## Architecture Bottlenecks

Based on code analysis, these are the current system limits:

| Component | Current Limit | Location |
|-----------|---------------|----------|
| Concurrent downloads | **2** | `src/core/config.rs` |
| Inter-download delay | 3000ms | `src/core/config.rs` |
| DB connection pool | 10 | `src/storage/db.rs` |
| Rate limit (free) | 30s | `src/core/rate_limiter.rs` |
| Rate limit (premium) | 10s | `src/core/rate_limiter.rs` |
| Rate limit (VIP) | 5s | `src/core/rate_limiter.rs` |

### Theoretical Throughput

With default settings:
- Max downloads/minute: ~20 (2 concurrent × ~3s per download + 3s delay)
- Max users before queue grows: ~40 (20 downloads/min ÷ 30s rate limit)
- 100 users: expect queue wait times of 5-10 minutes

## Metrics Collected

### Request Metrics
- Requests submitted, completed, failed
- Success rate, error rate
- Throughput (requests/second)

### Queue Metrics
- Current queue depth
- Average queue depth
- Maximum queue depth
- Time at peak depth

### Latency Metrics
- Queue wait time (P50, P95, P99)
- Processing time (P50, P95, P99)
- Total latency (submit → complete)

### System Metrics
- Memory usage (average, peak)
- Active downloads count

## Interpreting Results

### Sample Output
```
Load Test Results
================
Duration: 60.0s
Requests: 150 submitted, 145 completed, 5 failed (2 timeouts)
Success Rate: 96.7%
Error Rate: 3.3%
Throughput: 2.42 req/s

Queue Stats:
  Current Depth: 0
  Avg Depth: 25.3
  Max Depth: 85 (at 45.2s)

Latency (Queue Wait):
  Avg: 15234.5ms, P50: 12000ms, P95: 45000ms, P99: 58000ms, Max: 62000ms
```

### Key Indicators

| Metric | Good | Warning | Critical |
|--------|------|---------|----------|
| Success Rate | > 99% | 95-99% | < 95% |
| P95 Queue Wait | < 1m | 1-5m | > 5m |
| Max Queue Depth | < 50 | 50-200 | > 200 |
| Memory (Railway) | < 1GB | 1-1.5GB | > 1.5GB |

## Generating Reports

After running tests, generate a markdown report:

```rust
use load_test_report::ReportGenerator;

let mut generator = ReportGenerator::new();
generator.add_result(result);
generator.save_to_file(Path::new("tests/reports/load_test_report.md"))?;
```

Reports include:
- Executive summary
- Configuration used
- Detailed metrics
- Identified bottlenecks
- Recommendations

## Mock Downloader

Tests use a mock downloader to avoid actual network calls:

```rust
// Fast mode for stress testing
let mock = MockDownloader::fast();

// Realistic mode with delays
let mock = MockDownloader::realistic();

// Stress mode with high failure rate
let mock = MockDownloader::stress();
```

### Configuring Mock Behavior

```rust
let config = MockDownloaderConfig {
    base_delay_ms: 500,      // Simulated download time
    delay_variance_ms: 200,  // Random variance
    failure_rate: 0.02,      // 2% failure rate
    timeout_rate: 0.01,      // 1% timeout rate
    file_size_range: (1_000_000, 50_000_000),
    collect_detailed_metrics: true,
};
let mock = MockDownloader::new(config);
```

## CI Integration

Add to your CI workflow:

```yaml
- name: Run load tests
  run: cargo test --test load_test quick_sanity

- name: Run full load test suite (nightly)
  if: github.event_name == 'schedule'
  run: cargo test --test load_test -- --ignored
```

## Troubleshooting

### Tests Failing with Timeout
- Increase test duration
- Reduce number of simulated users
- Check if mock downloader delay is too high

### High Memory Usage
- Reduce `max_history_samples` in MetricsConfig
- Disable `collect_detailed_metrics` for long tests
- Check for memory leaks in actual download code

### Queue Never Drains
- Verify concurrent download limit is reasonable
- Check inter-download delay is not too high
- Ensure mock downloader isn't blocking

## Future Improvements

- [ ] Integration with Grafana for real-time monitoring
- [ ] Chaos testing (random failures)
- [ ] Geographic distribution simulation
- [ ] Real yt-dlp validation tests
