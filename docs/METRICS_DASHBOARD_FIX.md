# Fix: Grafana Dashboard - Connecting Metrics to Code

## Problem

The Grafana dashboard ([http://localhost:3000/d/doradura-overview](http://localhost:3000/d/doradura-overview)) was not showing data. All panels were empty even though the bot was running and Prometheus was collecting some metrics.

## Diagnosis

### Step 1: Check bot metrics

```bash
curl -s http://localhost:9094/metrics | grep -E "^doradura_"
```

**Result:** Only basic metrics without labels were exported:
- `doradura_bot_uptime_seconds`
- `doradura_daily_active_users`
- `doradura_total_users`
- `doradura_revenue_total_stars`
- `doradura_queue_depth_total`

**Missing:**
- `doradura_download_success_total`
- `doradura_download_failure_total`
- `doradura_format_requests_total`
- `doradura_errors_total`
- `doradura_active_subscriptions`

### Step 2: Check the dashboard

The dashboard uses the following metrics:

```promql
# Download Rate
sum(rate(doradura_download_success_total[5m]))
sum(rate(doradura_download_failure_total[5m]))

# Success Rate
sum(rate(doradura_download_success_total[5m])) /
(sum(rate(doradura_download_success_total[5m])) +
 sum(rate(doradura_download_failure_total[5m]))) * 100

# Download Duration Percentiles
histogram_quantile(0.5, rate(doradura_download_duration_seconds_bucket[5m]))
histogram_quantile(0.95, rate(doradura_download_duration_seconds_bucket[5m]))
histogram_quantile(0.99, rate(doradura_download_duration_seconds_bucket[5m]))

# Format Requests
sum by (format) (rate(doradura_format_requests_total[5m]))

# Errors by Category
sum by (category) (rate(doradura_errors_total[5m]))

# Active Subscriptions
sum(doradura_active_subscriptions)

# And others...
```

### Step 3: Check the code

In [src/core/metrics.rs](src/core/metrics.rs) the metrics were **declared** (lines 50-64):

```rust
pub static ref DOWNLOAD_SUCCESS_TOTAL: CounterVec = register_counter_vec!(
    "doradura_download_success_total",
    "Total number of successful downloads",
    &["format", "quality"]
).unwrap();

pub static ref DOWNLOAD_FAILURE_TOTAL: CounterVec = register_counter_vec!(
    "doradura_download_failure_total",
    "Total number of failed downloads",
    &["format", "error_type"]
).unwrap();
```

And even **used** in [src/download/downloader.rs](src/download/downloader.rs):

```rust
// Line 1932
metrics::record_download_success("mp3", quality);

// Line 1945
metrics::record_download_failure("mp3", error_type);
```

**BUT:** Prometheus CounterVec/GaugeVec with labels **do not export metrics** until at least one time series has been created for some label combination.

## Root Cause

Prometheus metrics with labels (`CounterVec`, `GaugeVec`, `HistogramVec`) are registered via `lazy_static`, but:

1. **Lazy initialization**: The metric is registered in the Prometheus Registry on first access to `lazy_static`
2. **Time series are created on demand**: A specific label combination (e.g., `{format="mp3", quality="320k"}`) is created only on the first call to `.with_label_values()`
3. **Prometheus does not export empty series**: If a label combination has never been used, it will not appear in the `/metrics` endpoint

### The problem with our code

In the `init_metrics()` function (line 310 in [src/core/metrics.rs](src/core/metrics.rs)) metrics were **registered** but not **initialized**:

```rust
// BEFORE fix:
pub fn init_metrics() {
    log::info!("Initializing metrics registry...");

    // Just a reference - registers the metric, but does NOT create time series
    let _ = &*DOWNLOAD_SUCCESS_TOTAL;
    let _ = &*DOWNLOAD_FAILURE_TOTAL;
    // ...
}
```

This meant:
- Metric registered in Registry
- But no time series exist
- `/metrics` endpoint does not show the metric
- Grafana sees no data

## Solution

Added explicit initialization of time series for all important label combinations in the `init_metrics()` function.

### Changes in [src/core/metrics.rs](src/core/metrics.rs)

#### 1. Download Metrics (lines 321-342)

```rust
// Initialize download counters with common format combinations
// This ensures they appear in /metrics even with 0 values
DOWNLOAD_SUCCESS_TOTAL.with_label_values(&["mp3", "320k"]);
DOWNLOAD_SUCCESS_TOTAL.with_label_values(&["mp3", "default"]);
DOWNLOAD_SUCCESS_TOTAL.with_label_values(&["mp4", "1080p"]);
DOWNLOAD_SUCCESS_TOTAL.with_label_values(&["mp4", "720p"]);
DOWNLOAD_SUCCESS_TOTAL.with_label_values(&["mp4", "480p"]);
DOWNLOAD_SUCCESS_TOTAL.with_label_values(&["srt", "default"]);
DOWNLOAD_SUCCESS_TOTAL.with_label_values(&["txt", "default"]);

DOWNLOAD_FAILURE_TOTAL.with_label_values(&["mp3", "timeout"]);
DOWNLOAD_FAILURE_TOTAL.with_label_values(&["mp3", "file_too_large"]);
DOWNLOAD_FAILURE_TOTAL.with_label_values(&["mp3", "ytdlp"]);
DOWNLOAD_FAILURE_TOTAL.with_label_values(&["mp3", "network"]);
DOWNLOAD_FAILURE_TOTAL.with_label_values(&["mp3", "other"]);
DOWNLOAD_FAILURE_TOTAL.with_label_values(&["mp4", "timeout"]);
DOWNLOAD_FAILURE_TOTAL.with_label_values(&["mp4", "file_too_large"]);
DOWNLOAD_FAILURE_TOTAL.with_label_values(&["mp4", "ytdlp"]);
DOWNLOAD_FAILURE_TOTAL.with_label_values(&["mp4", "network"]);
DOWNLOAD_FAILURE_TOTAL.with_label_values(&["mp4", "other"]);
DOWNLOAD_FAILURE_TOTAL.with_label_values(&["srt", "other"]);
DOWNLOAD_FAILURE_TOTAL.with_label_values(&["txt", "other"]);
```

#### 2. Business Metrics - Subscriptions (lines 353-372)

```rust
// Initialize subscription metrics by plan
ACTIVE_SUBSCRIPTIONS.with_label_values(&["free"]);
ACTIVE_SUBSCRIPTIONS.with_label_values(&["premium"]);
ACTIVE_SUBSCRIPTIONS.with_label_values(&["vip"]);

// Initialize revenue by plan
REVENUE_BY_PLAN.with_label_values(&["premium"]);
REVENUE_BY_PLAN.with_label_values(&["vip"]);

// Initialize new subscriptions
NEW_SUBSCRIPTIONS_TOTAL.with_label_values(&["premium", "true"]);
NEW_SUBSCRIPTIONS_TOTAL.with_label_values(&["premium", "false"]);
NEW_SUBSCRIPTIONS_TOTAL.with_label_values(&["vip", "true"]);
NEW_SUBSCRIPTIONS_TOTAL.with_label_values(&["vip", "false"]);

// Initialize payment metrics
PAYMENT_SUCCESS_TOTAL.with_label_values(&["premium", "true"]);
PAYMENT_SUCCESS_TOTAL.with_label_values(&["premium", "false"]);
PAYMENT_SUCCESS_TOTAL.with_label_values(&["vip", "true"]);
PAYMENT_SUCCESS_TOTAL.with_label_values(&["vip", "false"]);
```

#### 3. Error Metrics (lines 364-371)

```rust
// Initialize error counters with common error types
ERRORS_TOTAL.with_label_values(&["ytdlp", "download"]);
ERRORS_TOTAL.with_label_values(&["network", "download"]);
ERRORS_TOTAL.with_label_values(&["telegram", "send_file"]);
ERRORS_TOTAL.with_label_values(&["rate_limit", "download"]);
ERRORS_TOTAL.with_label_values(&["database", "query"]);
ERRORS_TOTAL.with_label_values(&["timeout", "download"]);
ERRORS_TOTAL.with_label_values(&["file_too_large", "download"]);
```

#### 4. Queue Depth (lines 373-376)

```rust
// Initialize queue depth gauges
QUEUE_DEPTH.with_label_values(&["low"]);
QUEUE_DEPTH.with_label_values(&["medium"]);
QUEUE_DEPTH.with_label_values(&["high"]);
```

#### 5. Format Requests (lines 387-395)

```rust
// Initialize format request counters
FORMAT_REQUESTS_TOTAL.with_label_values(&["mp3", "free"]);
FORMAT_REQUESTS_TOTAL.with_label_values(&["mp3", "premium"]);
FORMAT_REQUESTS_TOTAL.with_label_values(&["mp3", "vip"]);
FORMAT_REQUESTS_TOTAL.with_label_values(&["mp4", "free"]);
FORMAT_REQUESTS_TOTAL.with_label_values(&["mp4", "premium"]);
FORMAT_REQUESTS_TOTAL.with_label_values(&["mp4", "vip"]);
FORMAT_REQUESTS_TOTAL.with_label_values(&["srt", "free"]);
FORMAT_REQUESTS_TOTAL.with_label_values(&["txt", "free"]);
```

#### 6. Command Usage (lines 397-402)

```rust
// Initialize command usage counters
COMMAND_USAGE_TOTAL.with_label_values(&["start"]);
COMMAND_USAGE_TOTAL.with_label_values(&["help"]);
COMMAND_USAGE_TOTAL.with_label_values(&["settings"]);
COMMAND_USAGE_TOTAL.with_label_values(&["history"]);
COMMAND_USAGE_TOTAL.with_label_values(&["info"]);
```

#### 7. Users by Plan (lines 404-407)

```rust
// Initialize users by plan gauges
USERS_BY_PLAN.with_label_values(&["free"]);
USERS_BY_PLAN.with_label_values(&["premium"]);
USERS_BY_PLAN.with_label_values(&["vip"]);
```

## Verifying the Fix

### 1. Check bot metrics

```bash
curl -s http://localhost:9094/metrics | grep "doradura_download_success_total{"
```

**Result:**
```
doradura_download_success_total{format="mp3",quality="320k"} 0
doradura_download_success_total{format="mp3",quality="default"} 0
doradura_download_success_total{format="mp4",quality="1080p"} 0
doradura_download_success_total{format="mp4",quality="480p"} 0
doradura_download_success_total{format="mp4",quality="720p"} 0
doradura_download_success_total{format="srt",quality="default"} 0
doradura_download_success_total{format="txt",quality="default"} 0
```

All label combinations are exported with zero values!

### 2. Check Prometheus

```bash
curl -s 'http://localhost:9091/api/v1/query?query=doradura_download_success_total' | jq '.data.result | length'
```

**Result:** `7` time series

Prometheus is collecting all metrics!

### 3. Check Grafana

Open [http://localhost:3000/d/doradura-overview](http://localhost:3000/d/doradura-overview)

**Expected result:**
- **Download Rate** panel shows 0 req/sec (but chart is present)
- **Success Rate** panel shows 0% or "No data" (normal for zero values)
- **Queue Depth** panel shows 0
- **Download Duration** panel shows charts (may show No data - this is normal)
- **Daily Active Users** panel shows current value
- **Total Revenue** panel shows 0
- **Active Subscriptions** panel shows 0
- **Downloads by Format** panel shows 0 for all formats
- **Errors by Category** panel shows 0 errors

**Note:** Charts may show "No data" for computed metrics (rate, histogram_quantile) when all counters are 0. This is normal! Once downloads occur, data will appear.

## How It Works Now

### Metrics Lifecycle

1. **Bot starts** -> `init_metrics()` is called in [src/main.rs:75](src/main.rs#L75)

2. **Initialization** -> Time series are created for all important label combinations:
   ```rust
   DOWNLOAD_SUCCESS_TOTAL.with_label_values(&["mp3", "320k"]);
   // Creates time series: doradura_download_success_total{format="mp3",quality="320k"} 0
   ```

3. **Export to Prometheus** -> Metrics are available at the `/metrics` endpoint with zero values

4. **Prometheus Scraping** -> Prometheus collects metrics from the bot every 10 seconds

5. **Grafana Query** -> Grafana executes PromQL queries and receives data

6. **Code usage** -> When a download occurs:
   ```rust
   // src/download/downloader.rs:1932
   metrics::record_download_success("mp3", quality);
   // Increments: doradura_download_success_total{format="mp3",quality="320k"} = 1
   ```

7. **Dashboard update** -> Grafana refreshes automatically (every 30 seconds by default)

## Dashboard Panel to Code Mapping

| Dashboard Panel | PromQL Query | Metric Source | Code Location |
|----------------|--------------|---------------|---------------|
| **Download Rate** | `sum(rate(doradura_download_success_total[5m]))` | `DOWNLOAD_SUCCESS_TOTAL` | [downloader.rs:1932](src/download/downloader.rs#L1932) |
| **Success Rate** | `sum(rate(..._success...)) / (sum(..._success...) + sum(..._failure...)) * 100` | `DOWNLOAD_SUCCESS_TOTAL`, `DOWNLOAD_FAILURE_TOTAL` | [downloader.rs:1932,1945](src/download/downloader.rs#L1932) |
| **Queue Depth** | `doradura_queue_depth` | `QUEUE_DEPTH` | [queue.rs](src/download/queue.rs) via `metrics::update_queue_depth()` |
| **Download Duration** | `histogram_quantile(0.95, rate(doradura_download_duration_seconds_bucket[5m]))` | `DOWNLOAD_DURATION_SECONDS` | [downloader.rs:1550](src/download/downloader.rs#L1550) timer |
| **Downloads by Format** | `sum by (format) (rate(doradura_format_requests_total[5m]))` | `FORMAT_REQUESTS_TOTAL` | Used in commands handler |
| **Daily Active Users** | `doradura_daily_active_users` | `DAILY_ACTIVE_USERS` | Updated periodically |
| **Total Revenue** | `doradura_revenue_total_stars` | `REVENUE_TOTAL_STARS` | Updated on payments |
| **Active Subscriptions** | `sum(doradura_active_subscriptions)` | `ACTIVE_SUBSCRIPTIONS` | Updated on subscription changes |
| **Errors by Category** | `sum by (category) (rate(doradura_errors_total[5m]))` | `ERRORS_TOTAL` | [downloader.rs](src/download/downloader.rs) via `metrics::record_error()` |

## Best Practices Learned

### 1. Always Initialize Metrics with Labels

Incorrect:
```rust
// Only registration
let _ = &*MY_METRIC;
```

Correct:
```rust
// Registration + create time series
let _ = &*MY_METRIC;
MY_METRIC.with_label_values(&["common", "value1"]);
MY_METRIC.with_label_values(&["common", "value2"]);
```

### 2. Initialize All Important Combinations

If the dashboard uses a metric with labels, initialize all possible combinations:

```rust
// If dashboard groups by plan: sum by (plan) (...)
METRIC.with_label_values(&["free"]);
METRIC.with_label_values(&["premium"]);
METRIC.with_label_values(&["vip"]);
```

### 3. Document Labels

Add comments about what labels are expected:

```rust
/// Active subscriptions count by plan
/// Labels: plan (free/premium/vip)
pub static ref ACTIVE_SUBSCRIPTIONS: GaugeVec = ...
```

### 4. Check the Metrics Endpoint

After any metric change:

```bash
curl http://localhost:9094/metrics | grep "YOUR_METRIC"
```

Make sure the metric is present **before** checking Grafana.

## Additional Metrics

All the following metrics are now exported and ready for use in dashboards:

### Performance Metrics
- `doradura_download_duration_seconds` (histogram)
- `doradura_download_success_total` (counter with labels)
- `doradura_download_failure_total` (counter with labels)
- `doradura_queue_processing_duration_seconds` (histogram)
- `doradura_queue_wait_time_seconds` (histogram)

### Business Metrics
- `doradura_active_subscriptions` (gauge with plan label)
- `doradura_revenue_total_stars` (counter)
- `doradura_revenue_by_plan` (counter with plan label)
- `doradura_new_subscriptions_total` (counter with plan, is_recurring)
- `doradura_payment_success_total` (counter with plan, is_recurring)

### System Health Metrics
- `doradura_errors_total` (counter with error_type, operation)
- `doradura_queue_depth` (gauge with priority label)
- `doradura_queue_depth_total` (gauge)
- `doradura_ytdlp_health_status` (gauge)
- `doradura_db_connections_active` (gauge)
- `doradura_db_connections_idle` (gauge)

### User Engagement Metrics
- `doradura_daily_active_users` (gauge)
- `doradura_monthly_active_users` (gauge)
- `doradura_command_usage_total` (counter with command label)
- `doradura_format_requests_total` (counter with format, plan labels)
- `doradura_total_users` (gauge)
- `doradura_users_by_plan` (gauge with plan label)

## Next Steps

1. **Make a test download** -> Metrics will start updating
2. **Check the dashboard in an hour** -> You will see real data
3. **Create additional dashboards** if needed
4. **Configure alerts** in Prometheus for critical metrics

## Useful Commands

```bash
# Check all bot metrics
curl -s http://localhost:9094/metrics | grep "^doradura_"

# Check a specific metric
curl -s http://localhost:9094/metrics | grep "doradura_download_success_total"

# Check in Prometheus
curl -s 'http://localhost:9091/api/v1/query?query=doradura_download_success_total' | jq

# Restart monitoring (if needed)
docker-compose -f docker-compose.monitoring.yml restart

# View Prometheus logs
docker-compose -f docker-compose.monitoring.yml logs -f prometheus
```

## Related Files

- [src/core/metrics.rs](src/core/metrics.rs) - Metric definitions and initialization
- [src/download/downloader.rs](src/download/downloader.rs) - Download metric usage
- [grafana/dashboards/doradura_overview.json](grafana/dashboards/doradura_overview.json) - Grafana dashboard
- [prometheus.yml](prometheus.yml) - Prometheus configuration
- [HOW_TO_VIEW_METRICS.md](HOW_TO_VIEW_METRICS.md) - Metrics viewing guide
- [MONITORING_CHEATSHEET.md](MONITORING_CHEATSHEET.md) - Monitoring cheat sheet
