# Fix: "Downloads by Format" Metric

## Problem

The "Downloads by Format" panel in the Grafana dashboard was not showing data, even though the `doradura_format_requests_total` metric was being exported with zero values.

## Diagnosis

### 1. Check metric export

```bash
curl -s http://localhost:9094/metrics | grep "doradura_format_requests_total"
```

**Result:**
```
# TYPE doradura_format_requests_total counter
doradura_format_requests_total{format="mp3",plan="free"} 0
doradura_format_requests_total{format="mp3",plan="premium"} 0
doradura_format_requests_total{format="mp3",plan="vip"} 0
doradura_format_requests_total{format="mp4",plan="free"} 0
...
```

Metric is exported correctly!

### 2. Check query in Prometheus

```bash
curl 'http://localhost:9091/api/v1/query?query=sum%20by%20(format)%20(rate(doradura_format_requests_total%5B5m%5D))'
```

**Result:**
```json
{
  "data": {
    "result": [
      {"metric": {"format": "mp3"}, "value": [1765740637.691, "0"]},
      {"metric": {"format": "mp4"}, "value": [1765740637.691, "0"]},
      {"metric": {"format": "srt"}, "value": [1765740637.691, "0"]},
      {"metric": {"format": "txt"}, "value": [1765740637.691, "0"]}
    ]
  }
}
```

Query works and returns data!

### 3. Check code usage

```bash
grep -r "FORMAT_REQUESTS_TOTAL" src/
```

**Result:**
- Declared in [src/core/metrics.rs:267](src/core/metrics.rs#L267)
- Initialized in [src/core/metrics.rs:409-416](src/core/metrics.rs#L409-L416)
- **NEVER USED** anywhere in the download code!

## Root Cause

The `doradura_format_requests_total` metric was **declared and initialized**, but **never incremented** in code.

The metric should be incremented every time a user requests a download in a specific format (mp3/mp4/srt/txt), but the `.inc()` calls were missing.

### Why the metric was always 0

```promql
rate(doradura_format_requests_total[5m])
```

The `rate()` function computes the **rate of change** of a counter over the last 5 minutes. If the counter was never incremented (always 0), the rate is 0, and the panel shows no data.

## Solution

Added metric incrementation to all download functions.

### 1. Created a Helper Function

[src/core/metrics.rs:456-459](src/core/metrics.rs#L456-L459)

```rust
/// Helper function to record format request
pub fn record_format_request(format: &str, plan: &str) {
    FORMAT_REQUESTS_TOTAL.with_label_values(&[format, plan]).inc();
}
```

### 2. Added Increment to download_and_send_audio

[src/download/downloader.rs:1548-1564](src/download/downloader.rs#L1548-L1564)

```rust
tokio::spawn(async move {
    log::info!("Inside spawn for audio download, chat_id: {}", chat_id);
    let mut progress_msg = ProgressMessage::new(chat_id);
    let start_time = std::time::Instant::now();

    // Get user plan for metrics
    let user_plan = if let Some(ref pool) = db_pool_clone {
        if let Ok(conn) = db::get_connection(pool) {
            db::get_user(&conn, chat_id.0)
                .ok()
                .flatten()
                .map(|u| u.plan)
                .unwrap_or_else(|| "free".to_string())
        } else {
            "free".to_string()
        }
    } else {
        "free".to_string()
    };

    // Record format request for metrics
    metrics::record_format_request("mp3", &user_plan);

    // ... rest of the function
});
```

**Logic:**
1. Fetch the user plan from the DB (`free`, `premium`, or `vip`)
2. If the DB is unavailable or the user is not found, default to `"free"`
3. Call `record_format_request("mp3", &user_plan)` to increment the counter

### 3. Added Increment to download_and_send_video

[src/download/downloader.rs:2740-2756](src/download/downloader.rs#L2740-L2756)

```rust
tokio::spawn(async move {
    let mut progress_msg = ProgressMessage::new(chat_id);
    let start_time = std::time::Instant::now();

    // Get user plan for metrics
    let user_plan = if let Some(ref pool) = db_pool_clone {
        if let Ok(conn) = db::get_connection(pool) {
            db::get_user(&conn, chat_id.0)
                .ok()
                .flatten()
                .map(|u| u.plan)
                .unwrap_or_else(|| "free".to_string())
        } else {
            "free".to_string()
        }
    } else {
        "free".to_string()
    };

    // Record format request for metrics
    metrics::record_format_request("mp4", &user_plan);

    // ... rest of the function
});
```

### 4. Added Increment to download_and_send_subtitles

[src/download/downloader.rs:3525-3542](src/download/downloader.rs#L3525-L3542)

```rust
tokio::spawn(async move {
    let mut progress_msg = ProgressMessage::new(chat_id);
    let start_time = std::time::Instant::now();

    // Get user plan for metrics
    let user_plan = if let Some(ref pool) = db_pool_clone {
        if let Ok(conn) = db::get_connection(pool) {
            db::get_user(&conn, chat_id.0)
                .ok()
                .flatten()
                .map(|u| u.plan)
                .unwrap_or_else(|| "free".to_string())
        } else {
            "free".to_string()
        }
    } else {
        "free".to_string()
    };

    // Record format request for metrics
    let format = subtitle_format.as_str(); // "srt" or "txt"
    metrics::record_format_request(format, &user_plan);

    // ... rest of the function
});
```

## How It Works

### Metric Lifecycle

1. **User requests a download**
   - Sends a URL to the bot
   - Selects a format via menu (MP3 / MP4 / Subtitles)

2. **Bot calls the download function**
   - `download_and_send_audio()` for MP3
   - `download_and_send_video()` for MP4
   - `download_and_send_subtitles()` for SRT/TXT

3. **Fetching the user plan**
   ```rust
   let user_plan = db::get_user(&conn, chat_id.0)
       .map(|u| u.plan)
       .unwrap_or("free")
   ```
   - Queries data from the `users` table
   - Gets the `plan` field: `"free"`, `"premium"`, or `"vip"`
   - Falls back to `"free"` if the user is not in the DB

4. **Metric incrementation**
   ```rust
   metrics::record_format_request("mp3", "free")
   // Increments: doradura_format_requests_total{format="mp3",plan="free"}
   ```

5. **Export to Prometheus**
   ```
   doradura_format_requests_total{format="mp3",plan="free"} 1
   doradura_format_requests_total{format="mp3",plan="free"} 2
   doradura_format_requests_total{format="mp3",plan="free"} 3
   ...
   ```

6. **Prometheus computes rate**
   ```promql
   rate(doradura_format_requests_total{format="mp3",plan="free"}[5m])
   # Result: 0.01 req/sec (if there were 3 requests in 5 minutes)
   ```

7. **Grafana aggregates by format**
   ```promql
   sum by (format) (rate(doradura_format_requests_total[5m]))
   # Sums all plans (free + premium + vip) for each format
   # Result:
   # {format="mp3"} 0.02
   # {format="mp4"} 0.01
   ```

8. **Dashboard displays the chart**
   - "mp3" line - all MP3 requests (from all users)
   - "mp4" line - all MP4 requests
   - "srt" line - SRT subtitles
   - "txt" line - TXT subtitles

## Verifying the Fix

### 1. Check metric initialization

```bash
curl http://localhost:9094/metrics | grep "doradura_format_requests_total"
```

**Expected result:**
```
doradura_format_requests_total{format="mp3",plan="free"} 0
doradura_format_requests_total{format="mp3",plan="premium"} 0
doradura_format_requests_total{format="mp3",plan="vip"} 0
...
```

### 2. Make a test download

Send a URL to the bot and select MP3:
```
https://www.youtube.com/watch?v=dQw4w9WgXcQ
```

### 3. Check that the metric incremented

```bash
curl http://localhost:9094/metrics | grep "format_requests_total"
```

**Expected result:**
```
doradura_format_requests_total{format="mp3",plan="free"} 1  <- Incremented!
doradura_format_requests_total{format="mp3",plan="premium"} 0
doradura_format_requests_total{format="mp3",plan="vip"} 0
...
```

### 4. Check in Prometheus

```bash
curl 'http://localhost:9091/api/v1/query?query=sum%20by%20(format)%20(rate(doradura_format_requests_total%5B5m%5D))'
```

**Expected result:** Non-zero value for mp3

### 5. Check in Grafana

Open the dashboard: http://localhost:3000/d/doradura-overview

The **"Downloads by Format"** panel should show:
- A line for `mp3` with a non-zero value
- Possibly lines for `mp4`, `srt`, `txt` (if there were requests)

## Relationship to Other Metrics

### Download metrics work in parallel:

| Metric | When Incremented | Purpose |
|--------|-----------------|---------|
| `doradura_format_requests_total` | At download **start** | Counts requests by format and plan |
| `doradura_download_success_total` | On download **success** | Counts successful downloads |
| `doradura_download_failure_total` | On download **error** | Counts failed downloads |
| `doradura_download_duration_seconds` | At download **completion** | Measures duration |

**Example:**
```
1. User requests MP3
   -> format_requests_total{format="mp3"} += 1

2. Download starts
   -> download_duration_seconds starts timer

3. Download completes successfully
   -> download_success_total{format="mp3"} += 1
   -> download_duration_seconds observes 8.5 seconds

OR

3. Download fails
   -> download_failure_total{format="mp3",error_type="timeout"} += 1
   -> download_duration_seconds observes 120 seconds
```

## Dashboard Query

The panel uses the following PromQL query:

```promql
sum by (format) (rate(doradura_format_requests_total[5m]))
```

**Breakdown:**
- `rate(doradura_format_requests_total[5m])` - computes rate of change over 5 minutes
- `sum by (format) (...)` - sums across all plans (free + premium + vip)
- Result: requests per second for each format

**Alternative queries:**

Show breakdown by plan:
```promql
sum by (format, plan) (rate(doradura_format_requests_total[5m]))
```

Premium users only:
```promql
sum by (format) (rate(doradura_format_requests_total{plan="premium"}[5m]))
```

All requests (all formats combined):
```promql
sum(rate(doradura_format_requests_total[5m]))
```

## Best Practices

### 1. Increment Metrics Early

Correct:
```rust
// At the start of the function - BEFORE any await or long operations
metrics::record_format_request("mp3", &user_plan);
```

Incorrect:
```rust
// At the end of the function - metric won't be recorded on early return
if some_error {
    return Err(e); // Metric was NOT recorded!
}
metrics::record_format_request("mp3", &user_plan);
```

### 2. Use Fallback Values

```rust
let user_plan = db::get_user(&conn, chat_id.0)
    .ok()
    .flatten()
    .map(|u| u.plan)
    .unwrap_or_else(|| "free".to_string()); // Fallback!
```

This guarantees that the metric is always recorded, even if the DB is unavailable.

### 3. Group Labels Logically

The `format_requests_total` metric has 2 labels:
- `format` - what was requested (mp3/mp4/srt/txt)
- `plan` - who requested it (free/premium/vip)

This allows analysis of:
- "Which formats are most popular?" -> `sum by (format)`
- "How do premium users use the bot?" -> `{plan="premium"}`
- "How many free users download MP4?" -> `{format="mp4",plan="free"}`

## Final State

After the fix, the "Downloads by Format" panel works correctly:

- Metric is incremented on every download request
- Prometheus collects data every 10 seconds
- Grafana shows req/sec rate by format
- Chart updates automatically every 30 seconds

## Related Files

- [src/core/metrics.rs](src/core/metrics.rs) - Metric definitions and helper functions
- [src/download/downloader.rs](src/download/downloader.rs) - Metric usage in download code
- [grafana/dashboards/doradura_overview.json](grafana/dashboards/doradura_overview.json) - Grafana dashboard
- [METRICS_DASHBOARD_FIX.md](METRICS_DASHBOARD_FIX.md) - Main metrics fix
- [QUEUE_DEPTH_FIX.md](QUEUE_DEPTH_FIX.md) - Queue Depth fix
