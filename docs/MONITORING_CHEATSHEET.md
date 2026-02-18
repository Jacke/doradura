# Monitoring Cheatsheet - Quick Reference

## Startup

```bash
# 1. Start the bot
cargo run --release

# 2. Start monitoring
./scripts/start-monitoring.sh

# 3. Open Grafana
open http://localhost:3000
```

## Service URLs

| Service | URL | Description |
|--------|-----|----------|
| Bot Metrics | http://localhost:9094/metrics | Prometheus metrics |
| Bot Health | http://localhost:9094/health | JSON health status |
| Prometheus | http://localhost:9091 | Query & visualize |
| Grafana | http://localhost:3000 | Dashboards (admin/admin) |
| AlertManager | http://localhost:9093 | Alert management |

## Verification

```bash
# Check everything at once
./scripts/check-metrics.sh

# Check individual components
curl http://localhost:9094/health    # Bot
curl http://localhost:9091/-/healthy # Prometheus
curl http://localhost:3000/api/health # Grafana
```

## Docker Networking

### From Container → Host

```yaml
# macOS/Windows + Linux (with extra_hosts)
host.docker.internal:9094  ✅

# Linux (without extra_hosts)
172.17.0.1:9094
```

### From Host → Containers

```bash
localhost:9091  # Prometheus
localhost:3000  # Grafana
localhost:9093  # AlertManager
```

### Between Containers

```yaml
prometheus:9090    # Service name
grafana:3000
alertmanager:9093
```

## Useful PromQL Queries

```promql
# Downloads per hour
increase(doradura_download_success_total[1h])

# Success rate (%)
sum(rate(doradura_download_success_total[5m])) /
(sum(rate(doradura_download_success_total[5m])) +
 sum(rate(doradura_download_failure_total[5m]))) * 100

# p95 download duration
histogram_quantile(0.95,
  rate(doradura_download_duration_seconds_bucket[5m]))

# Error rate (%)
sum(rate(doradura_download_failure_total[5m])) /
(sum(rate(doradura_download_success_total[5m])) +
 sum(rate(doradura_download_failure_total[5m]))) * 100

# Queue depth
doradura_queue_depth

# DAU
doradura_daily_active_users

# Revenue
doradura_revenue_total_stars
```

## Docker Commands

```bash
# Start
docker-compose -f docker-compose.monitoring.yml up -d

# Stop
docker-compose -f docker-compose.monitoring.yml down

# Stop + delete data
docker-compose -f docker-compose.monitoring.yml down -v

# Logs (all services)
docker-compose -f docker-compose.monitoring.yml logs -f

# Logs (specific service)
docker-compose -f docker-compose.monitoring.yml logs -f prometheus
docker-compose -f docker-compose.monitoring.yml logs -f grafana

# Restart
docker-compose -f docker-compose.monitoring.yml restart

# Restart specific service
docker-compose -f docker-compose.monitoring.yml restart prometheus

# Status
docker-compose -f docker-compose.monitoring.yml ps

# Shell in container
docker exec -it doradura-prometheus sh
docker exec -it doradura-grafana sh
```

## Troubleshooting

### Bot metrics unavailable

```bash
# Check that bot is running
ps aux | grep doradura

# Check port
lsof -i :9094

# Check .env
cat .env | grep METRICS_PORT

# Should be: METRICS_PORT=9094
```

### Prometheus cannot see the bot

```bash
# Check targets
curl http://localhost:9091/api/v1/targets | jq

# Check from container
docker exec -it doradura-prometheus sh
wget -O- http://host.docker.internal:9094/metrics
```

### Grafana not showing data

```bash
# Check datasource
curl -u admin:admin http://localhost:3000/api/datasources/1/health | jq

# Check that Prometheus is accessible from Grafana
docker exec -it doradura-grafana sh
wget -O- http://prometheus:9090/api/v1/query?query=up
```

## Telegram Commands (Alternative)

```
/analytics              # General dashboard
/health                 # System status
/metrics performance    # Performance metrics
/metrics business       # Business metrics
/metrics engagement     # User engagement
/revenue                # Financial analytics
```

## Updating Configuration

```bash
# After changing prometheus.yml
docker-compose -f docker-compose.monitoring.yml restart prometheus

# After changing alert rules
curl -X POST http://localhost:9091/-/reload

# After changing dashboard
# Just update the file - Grafana will reload automatically
```

## API Endpoints

### Prometheus

```bash
# Query
curl 'http://localhost:9091/api/v1/query?query=up'

# Query range
curl 'http://localhost:9091/api/v1/query_range?query=up&start=2025-12-14T00:00:00Z&end=2025-12-14T23:59:59Z&step=15s'

# Targets
curl http://localhost:9091/api/v1/targets

# Rules
curl http://localhost:9091/api/v1/rules

# Alerts
curl http://localhost:9091/api/v1/alerts
```

### Grafana

```bash
# Health
curl http://localhost:3000/api/health

# Datasources
curl -u admin:admin http://localhost:3000/api/datasources

# Dashboards
curl -u admin:admin http://localhost:3000/api/search

# Export dashboard
curl -u admin:admin http://localhost:3000/api/dashboards/uid/doradura-overview
```

## Production (Railway)

```yaml
# prometheus.yml for Railway
scrape_configs:
  - job_name: 'doradura-bot'
    static_configs:
      - targets: ['doradura-bot.railway.internal:9094']
```

```bash
# Check in Railway
railway run bash
curl http://doradura-bot.railway.internal:9094/metrics
```

## Documentation

- **Quick start**: [QUICKSTART_MONITORING.md](QUICKSTART_MONITORING.md)
- **Full setup**: [MONITORING_SETUP.md](MONITORING_SETUP.md)
- **Architecture**: [MONITORING_ARCHITECTURE.md](MONITORING_ARCHITECTURE.md)
- **Docker Networking**: [DOCKER_NETWORKING.md](DOCKER_NETWORKING.md)
- **Port conflict fix**: [TROUBLESHOOTING_PORT_CONFLICT.md](TROUBLESHOOTING_PORT_CONFLICT.md)

---

**Bookmark this page!**
