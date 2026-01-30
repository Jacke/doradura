# Prometheus + Grafana Deployment Guide

## Quick Start

### 1. Start the monitoring stack

```bash
# Start the bot (with metrics server on port 9090)
cargo run --release

# In a separate terminal - start Prometheus + Grafana
docker-compose -f docker-compose.monitoring.yml up -d
```

### 2. Access services

- **Bot Metrics**: http://localhost:9090/metrics
- **Prometheus**: http://localhost:9091
- **Grafana**: http://localhost:3000 (login: admin / password: admin)
- **AlertManager**: http://localhost:9093

### 3. Verify everything works

```bash
# Check that metrics are available
curl http://localhost:9090/metrics

# Check that Prometheus is scraping metrics
curl http://localhost:9091/api/v1/targets

# Check logs
docker-compose -f docker-compose.monitoring.yml logs -f
```

---

## Detailed Setup

### Step 1: Configure environment variables

Update `.env`:

```bash
# Analytics & Metrics
METRICS_ENABLED=true
METRICS_PORT=9090

# Alerting
ALERTS_ENABLED=true
ALERT_ERROR_RATE_THRESHOLD=5.0
ALERT_QUEUE_DEPTH_THRESHOLD=50
```

### Step 2: Linux configuration

If you're on Linux, edit `prometheus.yml`:

```yaml
scrape_configs:
  - job_name: 'doradura-bot'
    static_configs:
      # For Linux use the host machine IP instead of host.docker.internal
      - targets: ['172.17.0.1:9090']
      # Or find IP: ip addr show docker0
```

For Railway/production:

```yaml
scrape_configs:
  - job_name: 'doradura-bot'
    static_configs:
      - targets: ['doradura-bot:9090']  # Service name in Railway
```

### Step 3: Grafana setup

1. Open http://localhost:3000
2. Login with admin/admin (change the password)
3. The "Doradura Bot - Overview" dashboard should appear automatically
4. If not - import from `grafana/dashboards/doradura_overview.json`

#### Creating additional dashboards

**Performance Dashboard:**
- Add a panel with `rate(doradura_download_success_total[5m])`
- Add a heat map for duration: `histogram_quantile(0.95, rate(doradura_download_duration_seconds_bucket[5m]))`

**Business Dashboard:**
- Revenue timeline: `increase(doradura_revenue_total_stars[1h])`
- Conversion rate: `rate(doradura_new_subscriptions_total[1h]) / rate(doradura_command_usage_total{command="start"}[1h])`

### Step 4: Configure Alerts

Alerts are already configured in `prometheus/rules/doradura_alerts.yml`.

**Alert types:**
- Critical: Require immediate action
- Warning: Require attention

**Main alerts:**
- `HighErrorRate` - error rate > 10%
- `QueueBackup` - queue > 100 tasks
- `BotDown` - bot unavailable > 2 min
- `SlowDownloads` - p95 duration > 60s
- `PaymentFailures` - payment errors

**View active alerts:**
```bash
# In Prometheus
curl http://localhost:9091/api/v1/alerts

# In AlertManager
curl http://localhost:9093/api/v1/alerts
```

### Step 5: Telegram integration (optional)

To receive alerts in Telegram, you have 2 options:

#### Option 1: Use the bot's built-in alert system

Your bot already has an `AlertManager` in `src/core/alerts.rs` that sends notifications to Telegram. Just make sure it's running in `main.rs`.

#### Option 2: Set up webhook from AlertManager

1. Add an endpoint in the bot to receive webhooks:

```rust
// In metrics_server.rs
.route("/alerts", post(alert_webhook_handler))

async fn alert_webhook_handler(
    State(bot): State<Bot>,
    Json(payload): Json<AlertWebhook>
) -> impl IntoResponse {
    // Process alert from Prometheus AlertManager
    // Send to Telegram admin
}
```

2. Update `alertmanager.yml`:

```yaml
receivers:
  - name: 'telegram'
    webhook_configs:
      - url: 'http://host.docker.internal:9090/alerts'
```

---

## Production Deployment (Railway)

### Option 1: Built-in monitoring (recommended)

Use only the built-in metrics server and Telegram commands:
- `/analytics` - main dashboard
- `/health` - system status
- `/metrics performance` - detailed metrics

Benefits:
- No additional services
- Works out of the box
- Metrics in Telegram
- Automatic alerts

### Option 2: Full stack with Prometheus + Grafana

#### On Railway

1. Add Prometheus as a separate service:

```yaml
# railway.toml
[[services]]
name = "prometheus"
source = "docker"
dockerfile = "Dockerfile.prometheus"

[[services]]
name = "grafana"
source = "docker"
dockerfile = "Dockerfile.grafana"
```

2. Create Dockerfiles:

**Dockerfile.prometheus:**
```dockerfile
FROM prom/prometheus:latest
COPY prometheus.yml /etc/prometheus/prometheus.yml
COPY prometheus/rules /etc/prometheus/rules
```

**Dockerfile.grafana:**
```dockerfile
FROM grafana/grafana:latest
COPY grafana/provisioning /etc/grafana/provisioning
COPY grafana/dashboards /var/lib/grafana/dashboards
```

3. Configure networking in Railway:
   - Services in the same project can communicate via internal domains
   - `prometheus.railway.internal:9090`
   - `doradura-bot.railway.internal:9090`

#### On a regular VPS/server

```bash
# Copy files to the server
scp -r docker-compose.monitoring.yml prometheus.yml grafana/ prometheus/ user@server:~/monitoring/

# On the server
cd ~/monitoring
docker-compose -f docker-compose.monitoring.yml up -d

# Set up reverse proxy (nginx) for Grafana access
```

**nginx config for Grafana:**
```nginx
server {
    listen 80;
    server_name grafana.yourdomain.com;

    location / {
        proxy_pass http://localhost:3000;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
    }
}
```

---

## Useful Prometheus Queries

### Performance

```promql
# Average download duration
histogram_quantile(0.5, rate(doradura_download_duration_seconds_bucket[5m]))

# Success rate
sum(rate(doradura_download_success_total[5m])) /
(sum(rate(doradura_download_success_total[5m])) + sum(rate(doradura_download_failure_total[5m])))

# Downloads per hour
increase(doradura_download_success_total[1h])
```

### Business

```promql
# Revenue today
increase(doradura_revenue_total_stars[1d])

# Subscription conversion
rate(doradura_new_subscriptions_total[1h]) / rate(doradura_command_usage_total{command="start"}[1h])

# Active subscriptions
sum(doradura_active_subscriptions)
```

### Health

```promql
# Error percentage
sum(rate(doradura_errors_total[5m])) /
sum(rate(doradura_download_success_total[5m]) + rate(doradura_download_failure_total[5m]))

# Queue depth
doradura_queue_depth

# Uptime in days
doradura_bot_uptime_seconds / 86400
```

---

## Troubleshooting

### Prometheus can't see bot metrics

```bash
# Check that the metrics server is running
curl http://localhost:9090/metrics

# Check targets in Prometheus
curl http://localhost:9091/api/v1/targets | jq

# For Docker on Mac/Windows use host.docker.internal
# For Linux find the docker0 IP: ip addr show docker0
```

### Grafana shows no data

1. Check datasource: Configuration -> Data Sources -> Prometheus
2. Check that the URL is correct: `http://prometheus:9090`
3. Click "Test" to verify the connection
4. Check that Prometheus has data: http://localhost:9091/graph

### Alerts not firing

```bash
# Check that rules are loaded
curl http://localhost:9091/api/v1/rules | jq

# Check active alerts
curl http://localhost:9091/api/v1/alerts | jq

# Check Prometheus logs
docker-compose -f docker-compose.monitoring.yml logs prometheus
```

### High memory usage

Prometheus stores metrics in memory. If memory is low:

1. Reduce retention:
```yaml
# In docker-compose.monitoring.yml
command:
  - '--storage.tsdb.retention.time=7d'  # Instead of 30d
```

2. Reduce scrape interval:
```yaml
# In prometheus.yml
global:
  scrape_interval: 30s  # Instead of 15s
```

---

## Backup and Restore

### Backup Prometheus data

```bash
# Stop Prometheus
docker-compose -f docker-compose.monitoring.yml stop prometheus

# Create backup
docker run --rm -v doradura_prometheus_data:/data -v $(pwd):/backup \
  alpine tar czf /backup/prometheus-backup.tar.gz -C /data .

# Start again
docker-compose -f docker-compose.monitoring.yml start prometheus
```

### Backup Grafana dashboards

```bash
# Export dashboard via API
curl -H "Authorization: Bearer YOUR_API_KEY" \
  http://localhost:3000/api/dashboards/uid/doradura-overview > dashboard-backup.json
```

---

## Monitoring Costs

For Railway/cloud providers, track:

1. **CPU Usage** - Prometheus can use a lot of CPU with many metrics
2. **Memory** - Metrics are stored in RAM
3. **Storage** - Prometheus saves data to disk
4. **Network** - Scraping metrics generates traffic

**Recommendations:**
- Use built-in Telegram commands for production
- Deploy Prometheus + Grafana on a separate server
- Or use managed services (Grafana Cloud, Datadog, etc.)

---

## Additional Features

### 1. Node Exporter (system metrics)

Add to `docker-compose.monitoring.yml`:

```yaml
  node-exporter:
    image: prom/node-exporter:latest
    container_name: node-exporter
    ports:
      - "9100:9100"
    command:
      - '--path.rootfs=/host'
    volumes:
      - '/:/host:ro,rslave'
    restart: unless-stopped
    networks:
      - monitoring
```

### 2. Blackbox Exporter (availability checks)

Monitor availability of external services (YouTube, bot API, etc.)

### 3. Loki for logs

Centralized log storage with Grafana integration

---

## Recommended Architecture

### Development

```
Your computer:
├── doradura bot (with metrics on :9090)
└── docker-compose:
    ├── Prometheus (:9091)
    ├── Grafana (:3000)
    └── AlertManager (:9093)
```

### Production (Simple)

```
Railway/VPS:
└── doradura bot (with metrics + Telegram commands)
    - Use /analytics for monitoring
    - Automatic alerts in Telegram
    - No additional services
```

### Production (Advanced)

```
Railway/VPS:
├── doradura bot (:9090 internal)
└── Separate VPS for monitoring:
    ├── Prometheus (scrapes bot)
    ├── Grafana (+ reverse proxy nginx)
    └── AlertManager (webhooks to Telegram)
```

---

## Useful Links

- [Prometheus Documentation](https://prometheus.io/docs/)
- [Grafana Documentation](https://grafana.com/docs/)
- [PromQL Tutorial](https://prometheus.io/docs/prometheus/latest/querying/basics/)
- [Best Practices for Naming Metrics](https://prometheus.io/docs/practices/naming/)
