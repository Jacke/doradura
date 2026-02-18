# Telegram Analytics Commands Integration - Complete

## What Was Done

### 1. Commands Added to the Command Enum

**File:** [src/telegram/bot.rs](src/telegram/bot.rs:51-58)

```rust
#[command(description = "analytics and metrics (admin only)")]
Analytics,

#[command(description = "system health status (admin only)")]
Health,

#[command(description = "detailed metrics (admin only)")]
Metrics,

#[command(description = "financial analytics (admin only)")]
Revenue,
```

### 2. Functions Imported

**File:** [src/main.rs](src/main.rs:32-37)

```rust
use doradura::telegram::{
    create_bot, handle_admin_command, handle_analytics_command, handle_backup_command,
    handle_charges_command, handle_download_tg_command, handle_health_command,
    handle_info_command, handle_menu_callback, handle_message, handle_metrics_command,
    handle_revenue_command, handle_sent_files_command, handle_setplan_command,
    handle_transactions_command, handle_users_command, is_message_addressed_to_bot,
    send_random_voice_message, setup_all_language_commands, setup_chat_bot_commands,
    show_enhanced_main_menu, show_main_menu, Command, WebAppAction, WebAppData,
};
```

### 3. Handlers Added to Dispatcher

**File:** [src/main.rs](src/main.rs:483-495)

```rust
Command::Analytics => {
    let _ = handle_analytics_command(bot.clone(), msg.clone(), db_pool.clone()).await;
}
Command::Health => {
    let _ = handle_health_command(bot.clone(), msg.clone(), db_pool.clone()).await;
}
Command::Metrics => {
    let _ = handle_metrics_command(bot.clone(), msg.clone(), db_pool.clone(), None).await;
}
Command::Revenue => {
    let _ = handle_revenue_command(bot.clone(), msg.clone(), db_pool.clone()).await;
}
```

---

## Available Commands

### `/analytics` - Overview Dashboard

Shows an overview of all metrics:

```
Analytics Dashboard

Performance (last 24h)
- Downloads: 1,234
- Success rate: 98.5%
- Avg duration: 8.3s

Business
- Revenue: 150 Stars
- Active subs: 42
- New today: 5

Health
- Queue: 3 tasks
- Error rate: 1.5%
- yt-dlp: OK

Engagement
- DAU: 85
- Commands: 523
- Top format: MP3
```

**Function:** `handle_analytics_command` ([src/telegram/analytics.rs:20](src/telegram/analytics.rs:20))

### `/health` - System Health

Shows the system health report:

```
System Health Report

Uptime: 2d 5h 23m

Queue Status
- Total: 3 tasks
- High priority: 0
- Medium: 2
- Low: 1

Errors (last 24h)
- ytdlp: 5
- network: 2
- rate_limit: 0

System Status
Bot: Running
Database: OK
yt-dlp: OK
```

**Function:** `handle_health_command` ([src/telegram/analytics.rs:61](src/telegram/analytics.rs:61))

### `/metrics` - Detailed Metrics

Shows detailed metrics (all categories by default):

```
Performance Metrics

Downloads (last 24h)
- Total: 1,234
- Success: 1,215 (98.5%)
- Failed: 19 (1.5%)

Duration
- Average: 8.3s
- p95: 15.2s
- p99: 25.8s

Queue
- Current depth: 3
- Avg wait time: 2.1s
```

**Function:** `handle_metrics_command` ([src/telegram/analytics.rs:90](src/telegram/analytics.rs:90))

### `/revenue` - Financial Analytics

Shows financial metrics and conversions:

```
Revenue Analytics

All-time
- Total: 1,250 Stars
- Premium: 850 Stars
- VIP: 400 Stars

This Month
- Revenue: 150 Stars
- New subs: 25

Conversion Funnel
- Visitors: 1,000
- Checkout: 50 (5%)
- Paid: 25 (50%)
```

**Function:** `handle_revenue_command` ([src/telegram/analytics.rs:131](src/telegram/analytics.rs:131))

---

## Security

All commands are **available to admins only**.

The check is performed in each function:

```rust
let username = msg.from.as_ref().and_then(|u| u.username.as_deref());
if !admin::is_admin(username) {
    bot.send_message(chat_id, "This command is available to admins only.")
        .await?;
    return Ok(());
}
```

**Admin setup:** In [src/telegram/admin.rs](src/telegram/admin.rs) via `ADMIN_USERNAME`

---

## How to Use

### 1. Restart the Bot

```bash
# Stop the current process (Ctrl+C)
cargo run --release
```

### 2. Check Commands in Telegram

Open the bot chat and type:

```
/analytics
```

If you are an admin, you will see the dashboard with metrics.

### 3. Try Other Commands

```
/health
/metrics
/revenue
```

---

## Data Sources

Metrics are pulled from several sources:

1. **Prometheus Registry** - runtime metrics
   - `doradura_download_success_total`
   - `doradura_download_failure_total`
   - `doradura_queue_depth`
   - `doradura_revenue_total_stars`
   - And others...

2. **Database** - historical data
   - `user_activity` table (for DAU/MAU)
   - `charges` table (for revenue analytics)
   - `users` table (for subscriptions)

3. **Cache** - aggregated data
   - Updated every 5 minutes
   - Stored in memory

---

## Configuration

### Environment Variables

Already configured in `.env`:

```bash
# Metrics & Monitoring
METRICS_ENABLED=true
METRICS_PORT=9094

# Alerting
ALERTS_ENABLED=true
ALERT_ERROR_RATE_THRESHOLD=5.0
ALERT_QUEUE_DEPTH_THRESHOLD=50
ALERT_RETRY_RATE_THRESHOLD=30.0
```

### Customization

To change message formats, edit the functions in:
- [src/telegram/analytics.rs](src/telegram/analytics.rs)

---

## Troubleshooting

### Command does not work

**Problem:** Sending `/analytics` but nothing happens

**Solution:**
1. Make sure you are an admin (check `ADMIN_USERNAME` in config)
2. Check bot logs for errors
3. Make sure the bot was restarted after changes

### "This command is available to admins only"

**Problem:** Seeing the admin-only message

**Solution:**
- Set your Telegram username in the admin configuration
- See [src/telegram/admin.rs](src/telegram/admin.rs)

### Empty data in metrics

**Problem:** Commands work but show zeros

**Solution:**
- This is normal if the bot just started
- Wait for user activity
- Or make test downloads

### Metrics not updating

**Problem:** Data does not change on repeated command calls

**Solution:**
- Check that Prometheus is collecting metrics: `curl http://localhost:9094/metrics`
- Verify the bot is writing to the DB
- Restart the bot

---

## Extending Functionality

### Adding a New Metric

1. Add the metric to [src/core/metrics.rs](src/core/metrics.rs)
2. Use it in code (e.g., on file download)
3. Display it in [src/telegram/analytics.rs](src/telegram/analytics.rs)

### Adding a Category to /metrics

Modify `handle_metrics_command` to accept a parameter:

```rust
Command::Metrics { category: String }
```

And handle different categories: `performance`, `business`, `engagement`.

### Adding Callback Buttons

The analytics functions already have inline buttons (see `handle_analytics_command`).

Add callback query handlers in main.rs.

---

## Checklist

- [x] Commands added to `Command` enum
- [x] Imports added to `main.rs`
- [x] Handlers added to dispatcher
- [x] Project compiles without errors
- [ ] Bot restarted
- [ ] Commands tested in Telegram

---

## Next Steps

1. **Restart the bot** - so commands take effect
2. **Test commands** - send `/analytics` in Telegram
3. **Configure AlertManager** - for automatic notifications (optional)
4. **Add BOT_COMMAND_DEFINITIONS** - so commands appear in the menu (optional)

---

## Related Documentation

- [ANALYTICS_SYSTEM.md](ANALYTICS_SYSTEM.md) - Full analytics system description
- [HOW_TO_VIEW_METRICS.md](HOW_TO_VIEW_METRICS.md) - How to view metrics (Grafana/Prometheus/Telegram)
- [MONITORING_SETUP.md](MONITORING_SETUP.md) - Prometheus + Grafana setup
- [src/telegram/analytics.rs](src/telegram/analytics.rs) - Command source code
- [src/core/metrics.rs](src/core/metrics.rs) - Metric definitions

---

**Status:** Integration complete and ready to use.

**Tested:** Compilation succeeded.

**Next step:** Restart the bot and try `/analytics`.
