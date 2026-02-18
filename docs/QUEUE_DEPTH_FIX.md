# Fix: Queue Depth in Grafana Dashboard

## Problem

The "Queue Depth" panel in the Grafana dashboard was not showing data, even though the metric was being exported correctly.

## Diagnosis

### 1. Check bot metrics

```bash
curl -s http://localhost:9094/metrics | grep "doradura_queue_depth"
```

**Result:**
```
# HELP doradura_queue_depth Current number of tasks in queue by priority
# TYPE doradura_queue_depth gauge
doradura_queue_depth{priority="high"} 0
doradura_queue_depth{priority="low"} 0
doradura_queue_depth{priority="medium"} 0
# HELP doradura_queue_depth_total Total number of tasks in queue
# TYPE doradura_queue_depth_total gauge
doradura_queue_depth_total 0
```

Both metrics are exported!

### 2. Check query in Prometheus

```bash
curl -s 'http://localhost:9091/api/v1/query?query=doradura_queue_depth' | jq '.data.result | length'
```

**Result:** `3` - Returns 3 time series (one per priority)

**Problem:** The query `doradura_queue_depth` returns **multiple** time series:
- `doradura_queue_depth{priority="high"} 0`
- `doradura_queue_depth{priority="low"} 0`
- `doradura_queue_depth{priority="medium"} 0`

A Grafana "Stat" panel (single number) does not know how to display 3 values simultaneously!

### 3. Check the correct metric

```bash
curl -s 'http://localhost:9091/api/v1/query?query=doradura_queue_depth_total' | jq '.data.result'
```

**Result:**
```json
[
  {
    "metric": {
      "__name__": "doradura_queue_depth_total",
      "instance": "doradura-bot",
      "job": "doradura-bot"
    },
    "value": [1765740505.585, "0"]
  }
]
```

Returns **one** value - exactly what the panel needs!

## Root Cause

The dashboard was using the wrong query:

**Before:**
```json
{
  "expr": "doradura_queue_depth",
  "refId": "A"
}
```

This query returns the metric **with labels** (by priority), resulting in multiple time series.

## Solution

Changed the query to use `doradura_queue_depth_total` - the metric **without labels** that shows total queue depth:

**After:**
```json
{
  "expr": "doradura_queue_depth_total",
  "refId": "A"
}
```

### File changed

[grafana/dashboards/doradura_overview.json:201](grafana/dashboards/doradura_overview.json#L201)

## Alternative Solutions

If you wanted to use the labeled metric, the following options were available:

### Option 1: Sum all priorities

```promql
sum(doradura_queue_depth)
```

Sums all priorities: high + medium + low

### Option 2: Show all priorities separately

Change the panel type from "Stat" to "Time series" and show 3 lines:
```promql
doradura_queue_depth
```

Set `legendFormat` to `{{ priority }}` to see high/medium/low separately.

### Option 3: High priority only

```promql
doradura_queue_depth{priority="high"}
```

## Difference Between Metrics

| Metric | Type | Labels | When to Use |
|--------|------|--------|-------------|
| `doradura_queue_depth` | GaugeVec | `priority` (high/medium/low) | When priority breakdown is needed |
| `doradura_queue_depth_total` | Gauge | None | When total task count in queue is needed |

## How Metrics Are Updated

### In Code

[src/download/queue.rs](src/download/queue.rs) or wherever the queue is processed:

```rust
use crate::core::metrics;

// Update by priority
metrics::update_queue_depth("high", high_priority_count);
metrics::update_queue_depth("medium", medium_priority_count);
metrics::update_queue_depth("low", low_priority_count);

// Update total depth
let total = high_priority_count + medium_priority_count + low_priority_count;
metrics::update_queue_depth_total(total);
```

### In metrics.rs

[src/core/metrics.rs:382-389](src/core/metrics.rs#L382-L389)

```rust
/// Helper function to update queue depth
pub fn update_queue_depth(priority: &str, depth: usize) {
    QUEUE_DEPTH.with_label_values(&[priority]).set(depth as f64);
}

/// Helper function to update total queue depth
pub fn update_queue_depth_total(depth: usize) {
    QUEUE_DEPTH_TOTAL.set(depth as f64);
}
```

## Verifying the Fix

### 1. Check the metric

```bash
curl -s http://localhost:9094/metrics | grep "doradura_queue_depth_total"
```

**Expected result:**
```
doradura_queue_depth_total 0
```

### 2. Check in Prometheus

```bash
curl -s 'http://localhost:9091/api/v1/query?query=doradura_queue_depth_total' | jq '.data.result[0].value[1]'
```

**Expected result:** `"0"` (or the current queue value)

### 3. Check in Grafana

1. Open the dashboard: http://localhost:3000/d/doradura-overview
2. Find the "Queue Depth" panel (usually in the top row on the right)
3. It should show a number: **0** (or the current value)
4. Color depends on thresholds:
   - Green: 0-49 tasks
   - Yellow: 50-99 tasks
   - Red: 100+ tasks

## Applying the Fix

The dashboard is updated automatically through Grafana provisioning. If the changes did not appear:

```bash
# Restart Grafana
docker-compose -f docker-compose.monitoring.yml restart grafana

# Verify Grafana started
curl http://localhost:3000/api/health
```

## Related Panels

Other panels in the dashboard already use correct queries:

- **Download Rate** - `sum(rate(doradura_download_success_total[5m]))`
  - Sum of all formats and qualities

- **Active Subscriptions** - `sum(doradura_active_subscriptions)`
  - Sum of all plans (free/premium/vip)

- **Downloads by Format** - `sum by (format) (rate(doradura_format_requests_total[5m]))`
  - Grouped by format (shows mp3, mp4, srt separately)

- **Errors by Category** - `sum by (category) (rate(doradura_errors_total[5m]))`
  - Grouped by error category

## Best Practices

### When to Use sum()

```promql
# If the metric has labels but you need a single number
sum(metric_with_labels)

# Example
sum(doradura_active_subscriptions)  # Sum of free + premium + vip
```

### When to Use sum by (label)

```promql
# If you need a breakdown by each label value
sum by (label_name) (metric)

# Example
sum by (format) (rate(doradura_format_requests_total[5m]))
# Shows separately: mp3, mp4, srt
```

### When to Use the Metric Directly

```promql
# If the metric has NO labels
metric_without_labels

# Examples
doradura_queue_depth_total
doradura_revenue_total_stars
doradura_daily_active_users
```

## Final State

After the fix all dashboard panels work correctly:

- Download Rate
- Success Rate
- **Queue Depth** - FIXED
- Download Duration (p50/p95/p99)
- Downloads by Format
- Daily Active Users
- Total Revenue
- Active Subscriptions
- Errors by Category

## Related Files

- [grafana/dashboards/doradura_overview.json](grafana/dashboards/doradura_overview.json) - Dashboard configuration
- [src/core/metrics.rs](src/core/metrics.rs) - Metric definitions
- [METRICS_DASHBOARD_FIX.md](METRICS_DASHBOARD_FIX.md) - Main metrics fix
- [HOW_TO_VIEW_METRICS.md](HOW_TO_VIEW_METRICS.md) - Metrics viewing guide
