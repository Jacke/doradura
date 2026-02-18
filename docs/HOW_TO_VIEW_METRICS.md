# How to View Metrics - Complete Guide

## 3 Ways to View Metrics

### 1. Grafana - Visual Dashboards (Recommended)
### 2. Prometheus - Queries and Charts
### 3. Telegram - Directly in the Bot

---

## 1. Grafana - Visual Dashboards

### Open Grafana

```bash
open http://localhost:3000
# Or just open it in a browser
```

**Login:**
- Username: `admin`
- Password: `admin` (you will be prompted to change it on first login)

### Find the Dashboard

After logging in:

1. In the **left sidebar**, click the **"Dashboards"** icon (four squares)
2. You will see the **"Doradura Bot - Overview"** dashboard
3. Click on it

### What You Will See

**9 metric panels:**

#### Performance
- **Download Rate** - Downloads per second (success vs failure)
- **Success Rate** - Percentage of successful downloads (gauge)
- **Queue Depth** - Current task queue depth
- **Download Duration** - p50, p95, p99 (median, 95th and 99th percentiles)

#### Business
- **Daily Active Users** - Active users today
- **Total Revenue** - Total revenue in Stars
- **Active Subscriptions** - Number of active subscriptions

#### Formats & Errors
- **Downloads by Format** - MP3 vs MP4 vs Subtitles
- **Errors by Category** - ytdlp, network, rate_limit, etc.

### Dashboard Settings

**Time range** (top right):
- Last 5 minutes
- Last 15 minutes
- Last 1 hour
- Last 6 hours (default)
- Last 24 hours
- Last 7 days
- Custom range

**Auto-refresh** (top right):
- Off
- 5s
- 10s
- 30s (default)
- 1m

### Drill Down into Metrics

**Click on a chart** -> see details
**Hover over a point** -> tooltip with exact values
**Legend** -> click to show/hide a series

---

## 2. Prometheus - Queries and Exploration

### Open Prometheus

```bash
open http://localhost:9091
```

### Graph Tab - Visualization

1. Go to the **"Graph"** tab
2. In the **"Expression"** field, enter a query (PromQL)
3. Click **"Execute"**
4. Switch between **"Graph"** and **"Table"** views

### PromQL Query Examples

#### Basic Metrics

```promql
# Current queue depth
doradura_queue_depth

# Daily Active Users
doradura_daily_active_users

# Total Revenue
doradura_revenue_total_stars

# Active subscriptions
doradura_active_subscriptions
```

#### Rate - Speed Over a Period

```promql
# Downloads per second (last 5 minutes)
rate(doradura_download_success_total[5m])

# Errors per second
rate(doradura_download_failure_total[5m])

# By format
rate(doradura_format_requests_total{format="mp3"}[5m])
```

#### Aggregate - Summation

```promql
# Total downloads per second (all formats)
sum(rate(doradura_download_success_total[5m]))

# By format
sum by (format) (rate(doradura_download_success_total[5m]))

# By quality
sum by (quality) (rate(doradura_download_success_total[5m]))
```

#### Calculations

```promql
# Success Rate (%)
sum(rate(doradura_download_success_total[5m])) /
(sum(rate(doradura_download_success_total[5m])) +
 sum(rate(doradura_download_failure_total[5m]))) * 100

# Error Rate (%)
sum(rate(doradura_download_failure_total[5m])) /
(sum(rate(doradura_download_success_total[5m])) +
 sum(rate(doradura_download_failure_total[5m]))) * 100

# Conversion Rate (%)
rate(doradura_new_subscriptions_total[1h]) /
rate(doradura_command_usage_total{command="start"}[1h]) * 100
```

#### Histograms - Percentiles

```promql
# Median download duration (p50)
histogram_quantile(0.5,
  rate(doradura_download_duration_seconds_bucket[5m]))

# 95th percentile
histogram_quantile(0.95,
  rate(doradura_download_duration_seconds_bucket[5m]))

# 99th percentile
histogram_quantile(0.99,
  rate(doradura_download_duration_seconds_bucket[5m]))

# By format
histogram_quantile(0.95,
  rate(doradura_download_duration_seconds_bucket{format="mp3"}[5m]))
```

#### Time Ranges - Over a Period

```promql
# Downloads in the last hour
increase(doradura_download_success_total[1h])

# Revenue today
increase(doradura_revenue_total_stars[1d])

# New subscriptions this week
increase(doradura_new_subscriptions_total[7d])
```

### Targets - Check Data Sources

1. Go to the **"Status" -> "Targets"** tab
2. Find **"doradura-bot"**
3. It should show:
   - **State:** `UP` (green)
   - **Endpoint:** `http://host.docker.internal:9094/metrics`
   - **Last Scrape:** recently (< 15 seconds ago)

### Alerts - Active Notifications

1. Go to the **"Alerts"** tab
2. You will see all configured alerts
3. Active alerts will be **red**
4. Inactive ones will be **green**

---

## 3. Telegram - Directly in the Bot

### Admin Commands

Send to the bot (as admin):

#### `/analytics` - Overview Dashboard

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

#### `/health` - System Health

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

#### `/metrics performance` - Performance Metrics

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

#### `/metrics business` - Business Metrics

```
Business Metrics

Revenue
- Total: 150 Stars
- Premium: 100 Stars
- VIP: 50 Stars

Subscriptions
- Active: 42
- New (24h): 5
- Cancelled (24h): 1

Conversion
- Rate: 2.3%
- Checkout starts: 218
- Completed: 5
```

#### `/metrics engagement` - User Engagement

```
User Engagement

Activity
- DAU: 85
- MAU: 523
- DAU/MAU: 16.3%

Format Preferences
- MP3: 65%
- MP4: 30%
- Subtitles: 5%

Commands (24h)
- /download: 523
- /start: 45
- /help: 12
```

#### `/revenue` - Financial Analytics

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

---

## Raw Metrics - For Development

### Bot Metrics Endpoint

```bash
# All metrics in Prometheus format
curl http://localhost:9094/metrics

# With less for scrolling
curl -s http://localhost:9094/metrics | less

# Grep a specific metric
curl -s http://localhost:9094/metrics | grep download_success

# Health endpoint
curl http://localhost:9094/health | jq
```

### Prometheus API

```bash
# Query API
curl -s 'http://localhost:9091/api/v1/query?query=doradura_queue_depth' | jq

# Query range (time range)
curl -s 'http://localhost:9091/api/v1/query_range?query=rate(doradura_download_success_total[5m])&start=2025-12-14T00:00:00Z&end=2025-12-14T23:59:59Z&step=1m' | jq

# All targets
curl -s http://localhost:9091/api/v1/targets | jq

# Active alerts
curl -s http://localhost:9091/api/v1/alerts | jq
```

### Grafana API

```bash
# All dashboards
curl -s -u admin:admin http://localhost:3000/api/search | jq

# Specific dashboard
curl -s -u admin:admin http://localhost:3000/api/dashboards/uid/doradura-overview | jq

# Datasources
curl -s -u admin:admin http://localhost:3000/api/datasources | jq
```

---

## Practical Scenarios

### Scenario 1: Check Performance

**Question:** How fast are downloads?

**Grafana:**
1. Open the dashboard
2. Look at the **"Download Duration"** panel
3. p95 shows: 95% of downloads complete faster than this time

**Prometheus:**
```promql
histogram_quantile(0.95,
  rate(doradura_download_duration_seconds_bucket[5m]))
```

**Telegram:**
```
/metrics performance
```

### Scenario 2: Find Problems

**Question:** Why are there many errors?

**Grafana:**
1. **"Errors by Category"** panel
2. See which category has the most errors
3. ytdlp errors -> problem with yt-dlp
4. network errors -> network issue

**Prometheus:**
```promql
# Top error categories
topk(5, sum by (category) (rate(doradura_errors_total[1h])))
```

**Telegram:**
```
/health
# Look at the "Errors" section
```

### Scenario 3: Revenue Analysis

**Question:** How much did we earn?

**Grafana:**
1. **"Total Revenue"** panel
2. See the total amount

**Prometheus:**
```promql
# Total
doradura_revenue_total_stars

# By plan
sum by (plan) (doradura_revenue_by_plan)

# Growth over 24 hours
increase(doradura_revenue_total_stars[1d])
```

**Telegram:**
```
/revenue
```

### Scenario 4: Queue Monitoring

**Question:** Is the queue overloaded?

**Grafana:**
1. **"Queue Depth"** panel
2. If > 50 -> possible problem

**Prometheus:**
```promql
# Current depth
doradura_queue_depth

# Maximum over an hour
max_over_time(doradura_queue_depth[1h])

# Alert if > 100
doradura_queue_depth > 100
```

**Telegram:**
```
/health
# Look at "Queue Status"
```

---

## Advanced: Creating Custom Charts

### In Grafana

1. Click **"+"** (Add panel) in the dashboard
2. Select **"Add a new panel"**
3. In **"Query"** enter PromQL
4. Configure visualization:
   - Time series (lines)
   - Gauge (circular scale)
   - Stat (number)
   - Bar chart (bars)
   - Table
5. Click **"Apply"**

**Example:** Downloads chart by hour

```promql
sum(increase(doradura_download_success_total[1h]))
```

### In Prometheus

1. **"Graph"** tab
2. Enter PromQL query
3. **"Add Graph"** to add another on the same page
4. Compare multiple metrics simultaneously

---

## Recommended Workflow

### Daily Check

```bash
# Telegram (quick)
/analytics
/health
```

### Weekly Analysis

1. **Grafana** -> view dashboard for the last 7 days
2. Pay attention to:
   - Success Rate trends
   - Revenue growth
   - Changes in Error Rate

### When Problems Occur

1. **Telegram** `/health` -> overall status
2. **Grafana** -> detailed chart analysis
3. **Prometheus** -> complex investigation queries

### For Presentations/Reports

1. **Grafana** -> Share dashboard -> Snapshot
2. Or export to PDF (requires plugin)
3. Or panel screenshots

---

## Pro Tips

### Grafana

- **Shift + Click** on time chart -> zoom in
- **Variables** -> create variables for filters (format, quality)
- **Annotations** -> mark important events (deploys, incidents)
- **Playlists** -> automatic dashboard rotation on a TV

### Prometheus

- **`{__name__=~"doradura.*"}`** -> all bot metrics
- **Recording rules** -> already created for frequently used queries
- **Console** -> tab for experimentation

### Telegram

- Set up **cron** for automatic `/analytics` delivery every day
- Use for quick checks from your phone

---

## Quick Reference

| What to View | Grafana | Prometheus | Telegram |
|-------------|---------|------------|----------|
| **Quick overview** | Dashboard | - | `/analytics` |
| **Detailed analysis** | Best | Good | - |
| **Complex queries** | Good | Best | - |
| **Mobile access** | Inconvenient | Inconvenient | Best |
| **Visualization** | Best | Basic | - |
| **Export/Share** | Best | Limited | - |

**Recommendation:**
- **Every day:** Telegram
- **Every week:** Grafana
- **When investigating:** Prometheus

---

## Additional Resources

### Learning Resources

- [PromQL Tutorial](https://prometheus.io/docs/prometheus/latest/querying/basics/)
- [Grafana Tutorials](https://grafana.com/tutorials/)
- [Query Examples](https://prometheus.io/docs/prometheus/latest/querying/examples/)

### Ready Dashboards

Your dashboard: `grafana/dashboards/doradura_overview.json`

You can create additional ones:
- Business Dashboard (revenue/subscriptions only)
- Technical Dashboard (performance/errors only)
- Executive Dashboard (high-level KPIs)

---

**Start with Grafana** -> http://localhost:3000
