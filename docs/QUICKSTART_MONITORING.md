# Monitoring Quick Start

## In 3 Commands

```bash
# 1. Start the bot (with metrics server)
cargo run --release

# 2. In a new terminal - start monitoring
./scripts/start-monitoring.sh

# 3. Open Grafana
open http://localhost:3000
# Login: admin / Password: admin
```

## What You Get

- **Prometheus** collects metrics every 15 seconds
- **Grafana** shows beautiful dashboards
- **AlertManager** tracks problems
- **30+ metrics** for performance, business, health

## Main URLs

| Service | URL | Description |
|---------|-----|-------------|
| Bot Metrics | http://localhost:9094/metrics | Raw metrics |
| Prometheus | http://localhost:9091 | Metrics storage & queries |
| Grafana | http://localhost:3000 | Dashboards |
| AlertManager | http://localhost:9093 | Alert management |

## Useful Scripts

```bash
# Check system health
./scripts/check-metrics.sh

# Stop monitoring
./scripts/stop-monitoring.sh

# View logs
docker-compose -f docker-compose.monitoring.yml logs -f
```

## Alternative: Telegram Only

If you don't want to run Docker, use the built-in commands:

```
/analytics - general dashboard
/health - system status
/metrics performance - detailed metrics
/revenue - finances
```

Everything works out of the box!

---

**Full documentation:** [MONITORING_SETUP.md](MONITORING_SETUP.md)
