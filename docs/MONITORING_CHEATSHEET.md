# 📋 Monitoring Cheatsheet - Быстрая Справка

## 🚀 Запуск

```bash
# 1. Запустить бота
cargo run --release

# 2. Запустить мониторинг
./scripts/start-monitoring.sh

# 3. Открыть Grafana
open http://localhost:3000
```

## 📊 URL Сервисов

| Сервис | URL | Описание |
|--------|-----|----------|
| Bot Metrics | http://localhost:9094/metrics | Prometheus metrics |
| Bot Health | http://localhost:9094/health | JSON health status |
| Prometheus | http://localhost:9091 | Query & visualize |
| Grafana | http://localhost:3000 | Dashboards (admin/admin) |
| AlertManager | http://localhost:9093 | Alert management |

## 🔍 Проверка

```bash
# Проверить всё сразу
./scripts/check-metrics.sh

# Проверить отдельные компоненты
curl http://localhost:9094/health    # Bot
curl http://localhost:9091/-/healthy # Prometheus
curl http://localhost:3000/api/health # Grafana
```

## 🐳 Docker Networking

### Из Контейнера → Хост

```yaml
# macOS/Windows + Linux (с extra_hosts)
host.docker.internal:9094  ✅

# Linux (без extra_hosts)
172.17.0.1:9094
```

### Из Хоста → Контейнеры

```bash
localhost:9091  # Prometheus
localhost:3000  # Grafana
localhost:9093  # AlertManager
```

### Между Контейнерами

```yaml
prometheus:9090    # Имя сервиса
grafana:3000
alertmanager:9093
```

## 📈 Полезные PromQL Запросы

```promql
# Загрузок в час
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

## 🛠️ Docker Commands

```bash
# Запустить
docker-compose -f docker-compose.monitoring.yml up -d

# Остановить
docker-compose -f docker-compose.monitoring.yml down

# Остановить + удалить данные
docker-compose -f docker-compose.monitoring.yml down -v

# Логи (все сервисы)
docker-compose -f docker-compose.monitoring.yml logs -f

# Логи (конкретный сервис)
docker-compose -f docker-compose.monitoring.yml logs -f prometheus
docker-compose -f docker-compose.monitoring.yml logs -f grafana

# Перезапустить
docker-compose -f docker-compose.monitoring.yml restart

# Перезапустить конкретный сервис
docker-compose -f docker-compose.monitoring.yml restart prometheus

# Статус
docker-compose -f docker-compose.monitoring.yml ps

# Shell в контейнере
docker exec -it doradura-prometheus sh
docker exec -it doradura-grafana sh
```

## 🔧 Troubleshooting

### Bot metrics недоступны

```bash
# Проверить что бот запущен
ps aux | grep doradura

# Проверить порт
lsof -i :9094

# Проверить .env
cat .env | grep METRICS_PORT

# Должно быть: METRICS_PORT=9094
```

### Prometheus не видит бота

```bash
# Проверить targets
curl http://localhost:9091/api/v1/targets | jq

# Проверить из контейнера
docker exec -it doradura-prometheus sh
wget -O- http://host.docker.internal:9094/metrics
```

### Grafana не показывает данные

```bash
# Проверить datasource
curl -u admin:admin http://localhost:3000/api/datasources/1/health | jq

# Проверить что Prometheus доступен из Grafana
docker exec -it doradura-grafana sh
wget -O- http://prometheus:9090/api/v1/query?query=up
```

## 📝 Telegram Команды (Альтернатива)

```
/analytics              # Общий дашборд
/health                 # Состояние системы
/metrics performance    # Performance метрики
/metrics business       # Business метрики
/metrics engagement     # User engagement
/revenue                # Финансы
```

## 🔄 Обновление Конфигурации

```bash
# После изменения prometheus.yml
docker-compose -f docker-compose.monitoring.yml restart prometheus

# После изменения alert rules
curl -X POST http://localhost:9091/-/reload

# После изменения dashboard
# Просто обновите файл - Grafana перечитает автоматически
```

## 📊 API Endpoints

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

## 🎯 Production (Railway)

```yaml
# prometheus.yml для Railway
scrape_configs:
  - job_name: 'doradura-bot'
    static_configs:
      - targets: ['doradura-bot.railway.internal:9094']
```

```bash
# Проверка в Railway
railway run bash
curl http://doradura-bot.railway.internal:9094/metrics
```

## 📚 Документация

- **Быстрый старт**: [QUICKSTART_MONITORING.md](QUICKSTART_MONITORING.md)
- **Полная настройка**: [MONITORING_SETUP.md](MONITORING_SETUP.md)
- **Архитектура**: [MONITORING_ARCHITECTURE.md](MONITORING_ARCHITECTURE.md)
- **Docker Networking**: [DOCKER_NETWORKING.md](DOCKER_NETWORKING.md)
- **Решение проблемы с портом**: [TROUBLESHOOTING_PORT_CONFLICT.md](TROUBLESHOOTING_PORT_CONFLICT.md)

---

**Сохраните эту страницу в закладки!** 🔖
