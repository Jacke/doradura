# 📊 Monitoring System

Полная система мониторинга для Doradura Bot с Prometheus + Grafana.

## 📁 Структура

```
.
├── docker-compose.monitoring.yml  # Docker Compose для всего стека
├── prometheus.yml                 # Конфигурация Prometheus
├── alertmanager.yml              # Конфигурация AlertManager
├── prometheus/
│   └── rules/
│       └── doradura_alerts.yml   # Alert rules
├── grafana/
│   ├── provisioning/
│   │   ├── datasources/          # Автоматическая настройка Prometheus
│   │   └── dashboards/           # Автоматическая настройка дашбордов
│   └── dashboards/
│       └── doradura_overview.json # Основной дашборд
└── scripts/
    ├── start-monitoring.sh       # 🚀 Запуск стека
    ├── stop-monitoring.sh        # 🛑 Остановка стека
    └── check-metrics.sh          # 🔍 Проверка здоровья
```

## 🚀 Быстрый Старт

```bash
# 1. Запустить мониторинг
./scripts/start-monitoring.sh

# 2. Открыть Grafana
open http://localhost:3000
```

## 📚 Документация

- **[QUICKSTART_MONITORING.md](../QUICKSTART_MONITORING.md)** - Быстрый старт (3 команды)
- **[MONITORING_SETUP.md](../MONITORING_SETUP.md)** - Полное руководство
- **[ANALYTICS_SYSTEM.md](../ANALYTICS_SYSTEM.md)** - Описание метрик и аналитики

## 🎯 Что Мониторится

### Performance
- ⚡ Download success rate
- ⏱️ Download duration (p50, p95, p99)
- 📊 Queue depth
- 🔄 Retry rate

### Business
- 💰 Revenue (Telegram Stars)
- 👥 Active subscriptions
- 📈 New subscriptions
- 📉 Cancellations
- 🎯 Conversion rate

### System Health
- ❌ Error rate by category
- 🔧 yt-dlp status
- 💾 Database status
- ⏰ Bot uptime

### User Engagement
- 👤 Daily Active Users (DAU)
- 📅 Monthly Active Users (MAU)
- 🎵 Format preferences (MP3 vs MP4)
- 📱 Command usage

## 🔔 Alerts

Автоматические оповещения настроены для:

- 🔴 **Critical**: Высокий error rate, бот down, ошибки платежей
- 🟡 **Warning**: Медленные загрузки, низкая конверсия, высокий retry rate

## 🛠️ Технологии

- **Prometheus** - Сбор и хранение метрик
- **Grafana** - Визуализация
- **AlertManager** - Управление оповещениями
- **Docker Compose** - Оркестрация

## 📞 Поддержка

Проблемы? См. раздел **Troubleshooting** в [MONITORING_SETUP.md](../MONITORING_SETUP.md)
