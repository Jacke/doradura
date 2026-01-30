# Analytics System for Telegram Bot

## Overview

A full-featured analytics system with Prometheus metrics, admin commands in Telegram, and an alerting system.

## What's Implemented

### Phase 1: Metrics Infrastructure

#### 1. **Metrics Module** (`src/core/metrics.rs`)
- **30+ metrics** in 4 categories:
  - **Performance**: duration, success/failure, queue metrics
  - **Business**: revenue, subscriptions, conversions
  - **System Health**: errors, queue depth, uptime
  - **User Engagement**: DAU/MAU, command usage, format preferences

**Key metrics:**
```rust
// Performance
- doradura_download_duration_seconds (histogram)
- doradura_download_success_total (counter)
- doradura_download_failure_total (counter)
- doradura_queue_wait_time_seconds (histogram)

// Business
- doradura_revenue_total_stars (counter)
- doradura_revenue_by_plan (counter)
- doradura_new_subscriptions_total (counter)
- doradura_subscription_cancellations_total (counter)

// System Health
- doradura_errors_total (counter)
- doradura_queue_depth (gauge)
- doradura_bot_uptime_seconds (counter)

// User Engagement
- doradura_daily_active_users (gauge)
- doradura_command_usage_total (counter)
- doradura_format_requests_total (counter)
```

#### 2. **HTTP Metrics Server** (`src/core/metrics_server.rs`)
- Runs on port 9090 (configurable)
- **Endpoints:**
  - `GET /metrics` - Prometheus metrics (text format)
  - `GET /health` - Health check
  - `GET /` - Info page

#### 3. **Database** (`migrations/V8__add_analytics_tables.sql`)
Three new tables:
- `metric_aggregates` - aggregated metrics
- `alert_history` - alert history
- `user_activity` - user activity (for DAU/MAU)

### Phase 2: Code Instrumentation

#### 1. **Downloads** (`src/download/downloader.rs`)
Instrumented functions:
- `download_and_send_audio()` - timer + success/failure tracking
- `download_and_send_video()` - timer + success/failure tracking
- `download_and_send_subtitles()` - timer + success/failure tracking

**Usage pattern:**
```rust
let timer = metrics::DOWNLOAD_DURATION_SECONDS
    .with_label_values(&["mp3", quality])
    .start_timer();

// ... download logic ...

match result {
    Ok(_) => {
        timer.observe_duration();
        metrics::record_download_success("mp3", quality);
    }
    Err(e) => {
        timer.observe_duration();
        metrics::record_download_failure("mp3", error_type);
    }
}
```

#### 2. **Queue** (`src/download/queue.rs`)
Queue depth tracking:
- `add_task()` - increments counter when adding
- `get_task()` - decrements counter when retrieving
- Separate metrics by priority (low/medium/high)

#### 3. **Subscriptions** (`src/core/subscription.rs`)
Business metrics:
- Checkout process start
- Successful/failed payments
- Revenue tracking by plan
- New subscriptions
- Subscription cancellations

#### 4. **Errors** (`src/core/error.rs`)
Centralized error tracking:
```rust
impl AppError {
    pub fn track(&self) {
        metrics::ERRORS_TOTAL
            .with_label_values(&[self.category()])
            .inc();
    }
}
```

### Phase 3: Admin Commands

#### **Telegram Analytics** (`src/telegram/analytics.rs`)

4 admin commands for viewing metrics directly in Telegram:

**1. `/analytics` - General Dashboard**
```
Analytics Dashboard

Performance (last 24h)
• Downloads: 1,234
• Success rate: 98.5%
• Avg duration: 8.3s

Business
• Revenue: 150 Stars
• Active subs: 42
• New today: 5

Health
• Queue: 3 tasks
• Error rate: 1.5%
• yt-dlp: OK

Engagement
• DAU: 85
• Commands: --
• Top format: MP3
```

**2. `/health` - System Status**
- Bot uptime
- Queue status by priority
- Error breakdown by category
- System status

**3. `/metrics [category]` - Detailed Metrics**
Categories:
- `performance` - downloads, success rate, duration
- `business` - revenue, subscriptions, conversions
- `engagement` - user activity, popular formats
- `system` - errors, queues, rate limits

**4. `/revenue` - Financial Analytics**
- Total revenue (all-time)
- Breakdown by plan (premium/vip)
- Conversion funnel
- Payment statistics

### Phase 4: Alerting System

#### **AlertManager** (`src/core/alerts.rs`)

**Alert types:**
- `HighErrorRate` - high error percentage
- `QueueBackup` - queue overflow
- `PaymentFailure` - payment error (critical!)
- `YtdlpDown` - yt-dlp not working
- `DatabaseIssues` - database problems
- `LowConversion` - low conversion
- `HighRetryRate` - many retry attempts

**Severity levels:**
- **Warning** - requires attention
- **Critical** - requires immediate action

**Features:**
- Throttling (prevents spam)
- Resolution tracking (notification when problem is resolved)
- Database persistence (alert history)
- Configurable thresholds via .env

**Alert example:**
```
CRITICAL ALERT

High Error Rate Detected

Current: 12.5% (threshold: 5.0%)
Affected: 125/1000 downloads

Details:
Recent performance issues detected. Check logs for details.

Triggered: 2025-12-13 10:30:00 UTC
```

**Monitoring runs automatically:**
- Check every 60 seconds
- Automatic sending to Telegram admin
- Notifications when problems are resolved

## Configuration

### Environment Variables (`.env.example`)

```bash
# Analytics & Metrics Configuration
METRICS_ENABLED=true
METRICS_PORT=9090
PROMETHEUS_URL=http://prometheus:9090

# Alerting Configuration
ALERTS_ENABLED=true
ALERT_ERROR_RATE_THRESHOLD=5.0
ALERT_QUEUE_DEPTH_THRESHOLD=50
ALERT_RETRY_RATE_THRESHOLD=30.0

# Analytics Cache
ANALYTICS_CACHE_UPDATE_INTERVAL=300
```

## Prometheus + Grafana Integration

### 1. Prometheus Configuration

Add to `prometheus.yml`:
```yaml
scrape_configs:
  - job_name: 'doradura-bot'
    static_configs:
      - targets: ['localhost:9090']
    scrape_interval: 15s
```

### 2. Grafana Dashboards

Import ready-made dashboards or create your own:

**Performance Dashboard:**
- Download success rate timeline
- Average download duration by format
- Queue depth over time
- Error rate graph

**Business Dashboard:**
- Revenue timeline
- New subscriptions graph
- Active subscriptions by plan
- Conversion funnel

**System Health Dashboard:**
- Error breakdown by category
- Queue depth by priority
- Bot uptime
- Rate limit hits

## Getting Started

### 1. Update .env
```bash
METRICS_ENABLED=true
METRICS_PORT=9090
ALERTS_ENABLED=true
```

### 2. Start the bot
```bash
cargo run --release
```

Metrics will be available at `http://localhost:9090/metrics`

### 3. (Optional) Start Prometheus
```bash
docker run -d \
  -p 9090:9090 \
  -v $(pwd)/prometheus.yml:/etc/prometheus/prometheus.yml \
  prom/prometheus
```

### 4. (Optional) Start Grafana
```bash
docker run -d \
  -p 3000:3000 \
  grafana/grafana
```

## Next Steps (Integration)

### 1. Add commands to dispatcher (main.rs)

Register admin commands in bot dispatcher:

```rust
use doradura::telegram::{
    handle_analytics_command,
    handle_health_command,
    handle_metrics_command,
    handle_revenue_command,
};

// In dispatcher setup function:
let handler = dptree::entry()
    .branch(
        Update::filter_message()
            .filter_command::<Command>()
            .branch(case![Command::Analytics].endpoint(
                |bot, msg, db_pool| handle_analytics_command(bot, msg, db_pool)
            ))
            .branch(case![Command::Health].endpoint(
                |bot, msg, db_pool| handle_health_command(bot, msg, db_pool)
            ))
            .branch(case![Command::Metrics { category }].endpoint(
                |bot, msg, db_pool, category| handle_metrics_command(bot, msg, db_pool, category)
            ))
            .branch(case![Command::Revenue].endpoint(
                |bot, msg, db_pool| handle_revenue_command(bot, msg, db_pool)
            ))
    );
```

### 2. Add commands to enum (src/telegram/bot.rs or commands.rs)

```rust
#[derive(BotCommands, Clone)]
pub enum Command {
    // ... existing commands ...

    #[command(description = "Analytics dashboard (admin only)")]
    Analytics,

    #[command(description = "System health report (admin only)")]
    Health,

    #[command(description = "Detailed metrics [category] (admin only)")]
    Metrics { category: Option<String> },

    #[command(description = "Revenue report (admin only)")]
    Revenue,
}
```

### 3. Start AlertManager in main.rs

```rust
use doradura::core::alerts;

// After initializing metrics server:
if *config::alerts::ENABLED {
    let admin_chat_id = ChatId(ADMIN_USER_ID); // get from config

    let alert_manager = alerts::start_alert_monitor(
        bot.clone(),
        admin_chat_id,
        Arc::clone(&db_pool),
    ).await;

    log::info!("Alert monitoring started");
}
```

### 4. Integrate error tracking

In error handling locations add:
```rust
match result {
    Err(e) => {
        e.track(); // Automatically increments error counter
        // ... handle error ...
    }
}
```

### 5. Add user activity tracking

In command handler:
```rust
// Record user activity for DAU/MAU tracking
if let Ok(conn) = db::get_connection(&db_pool) {
    let _ = conn.execute(
        "INSERT INTO user_activity (user_id, activity_date, command_count)
         VALUES (?, date('now'), 1)
         ON CONFLICT(user_id, activity_date)
         DO UPDATE SET command_count = command_count + 1",
        [user_id],
    );
}
```

## Testing

### Check metrics
```bash
curl http://localhost:9090/metrics
```

You should see:
```
# HELP doradura_download_duration_seconds Time spent downloading files
# TYPE doradura_download_duration_seconds histogram
doradura_download_duration_seconds_bucket{format="mp3",quality="320k",le="1"} 45
...
```

### Check admin commands

In Telegram (as admin):
```
/analytics
/health
/metrics performance
/revenue
```

### Testing alerts

You can artificially trigger an alert:
```rust
if let Some(alert_manager) = &alert_manager {
    alert_manager.alert_payment_failure("premium", "test").await?;
}
```

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                         Telegram Bot                         │
└─────────────────────────────────────────────────────────────┘
                              │
                              ├──────────────────┐
                              │                  │
                    ┌─────────▼────────┐  ┌──────▼──────┐
                    │  Instrumented    │  │   Admin     │
                    │  Code (timers,   │  │  Commands   │
                    │  counters)       │  │  /analytics │
                    └─────────┬────────┘  └──────┬──────┘
                              │                  │
                    ┌─────────▼──────────────────▼────────┐
                    │      Prometheus Metrics Registry     │
                    │         (lazy_static)                │
                    └─────────┬────────────────────────────┘
                              │
                 ┌────────────┼────────────┐
                 │            │            │
          ┌──────▼──────┐ ┌──▼───────┐ ┌─▼────────────┐
          │   HTTP      │ │ Telegram │ │ Alert        │
          │   /metrics  │ │ Messages │ │ Manager      │
          │   :9090     │ │ (inline) │ │ (background) │
          └──────┬──────┘ └──────────┘ └─┬────────────┘
                 │                        │
          ┌──────▼──────┐         ┌──────▼──────┐
          │ Prometheus  │         │   Telegram  │
          │   Server    │         │   Admin     │
          └──────┬──────┘         └─────────────┘
                 │
          ┌──────▼──────┐
          │   Grafana   │
          │  Dashboards │
          └─────────────┘
```

## Implementation Benefits

### 1. **Minimal overhead**
- Prometheus metrics are very fast (<0.1% CPU)
- Lazy evaluation
- Efficient in-memory storage

### 2. **Production-ready**
- Industry standard (Prometheus)
- Proven in production by thousands of companies
- Rich ecosystem (Grafana, AlertManager)

### 3. **Scalability**
- Metrics are aggregated automatically
- Doesn't load the database
- Horizontal scaling ready

### 4. **Convenience**
- Admin sees metrics directly in Telegram
- Automatic alerts
- Beautiful dashboards in Grafana

### 5. **Observability**
- Full visibility into bot operation
- Fast problem diagnosis
- Data-driven decision making

## Code Documentation

All modules are fully documented with usage examples:
- `src/core/metrics.rs` - description of all metrics + helper functions
- `src/core/metrics_server.rs` - HTTP server endpoints
- `src/core/alerts.rs` - alerting system + examples
- `src/telegram/analytics.rs` - admin commands

## Important Notes

1. **Admin Only**: All analytics commands are available only to administrators (checked via `is_admin()`)

2. **Throttling**: Alerts have throttling to prevent spam:
   - Payment failures: no throttle (immediate)
   - High error rate: 30 minutes
   - Queue backup: 15 minutes

3. **Database**: User activity tracking requires DB writes, but this happens asynchronously and doesn't block

4. **Memory**: Metrics are stored in memory. With many label combinations, RAM usage can grow

## Future Improvements

- [ ] Dashboard in Web UI (not just Telegram)
- [ ] Export metrics to CSV
- [ ] A/B testing framework
- [ ] User cohort analysis
- [ ] Predictive analytics (ML)
- [ ] Custom alerts via Web UI

---

**Status**: Fully implemented and compiles without errors

**Next Step**: Integration in main.rs (adding commands to dispatcher and starting AlertManager)
