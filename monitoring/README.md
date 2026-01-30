# Monitoring System

Complete monitoring system for Doradura Bot with Prometheus + Grafana.

## Structure

```
.
├── docker-compose.monitoring.yml  # Docker Compose for the full stack
├── prometheus.yml                 # Prometheus configuration
├── alertmanager.yml              # AlertManager configuration
├── prometheus/
│   └── rules/
│       └── doradura_alerts.yml   # Alert rules
├── grafana/
│   ├── provisioning/
│   │   ├── datasources/          # Automatic Prometheus setup
│   │   └── dashboards/           # Automatic dashboard setup
│   └── dashboards/
│       └── doradura_overview.json # Main dashboard
└── scripts/
    ├── start-monitoring.sh       # Start the stack
    ├── stop-monitoring.sh        # Stop the stack
    └── check-metrics.sh          # Health check
```

## Quick Start

```bash
# 1. Start monitoring
./scripts/start-monitoring.sh

# 2. Open Grafana
open http://localhost:3000
```

## Documentation

- **[QUICKSTART_MONITORING.md](../docs/QUICKSTART_MONITORING.md)** - Quick start (3 commands)
- **[MONITORING_SETUP.md](../docs/MONITORING_SETUP.md)** - Full guide
- **[ANALYTICS_SYSTEM.md](../docs/ANALYTICS_SYSTEM.md)** - Metrics and analytics description

## What's Monitored

### Performance
- Download success rate
- Download duration (p50, p95, p99)
- Queue depth
- Retry rate

### Business
- Revenue (Telegram Stars)
- Active subscriptions
- New subscriptions
- Cancellations
- Conversion rate

### System Health
- Error rate by category
- yt-dlp status
- Database status
- Bot uptime

### User Engagement
- Daily Active Users (DAU)
- Monthly Active Users (MAU)
- Format preferences (MP3 vs MP4)
- Command usage

## Alerts

Automatic alerts configured for:

- **Critical**: High error rate, bot down, payment failures
- **Warning**: Slow downloads, low conversion, high retry rate

## Technologies

- **Prometheus** - Metrics collection and storage
- **Grafana** - Visualization
- **AlertManager** - Alert management
- **Docker Compose** - Orchestration

## Support

Problems? See the **Troubleshooting** section in [MONITORING_SETUP.md](../docs/MONITORING_SETUP.md)
