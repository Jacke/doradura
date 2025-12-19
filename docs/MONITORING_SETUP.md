# 📊 Руководство по Развертыванию Prometheus + Grafana

## Быстрый Старт

### 1. Запуск системы мониторинга

```bash
# Запустить бота (с metrics сервером на порту 9090)
cargo run --release

# В отдельном терминале - запустить Prometheus + Grafana
docker-compose -f docker-compose.monitoring.yml up -d
```

### 2. Доступ к сервисам

- **Bot Metrics**: http://localhost:9090/metrics
- **Prometheus**: http://localhost:9091
- **Grafana**: http://localhost:3000 (логин: admin / пароль: admin)
- **AlertManager**: http://localhost:9093

### 3. Проверка работы

```bash
# Проверить что метрики доступны
curl http://localhost:9090/metrics

# Проверить что Prometheus собирает метрики
curl http://localhost:9091/api/v1/targets

# Проверить логи
docker-compose -f docker-compose.monitoring.yml logs -f
```

---

## Подробная Настройка

### Шаг 1: Настройка переменных окружения

Обновите `.env`:

```bash
# Analytics & Metrics
METRICS_ENABLED=true
METRICS_PORT=9090

# Alerting
ALERTS_ENABLED=true
ALERT_ERROR_RATE_THRESHOLD=5.0
ALERT_QUEUE_DEPTH_THRESHOLD=50
```

### Шаг 2: Настройка для Linux

Если вы на Linux, отредактируйте `prometheus.yml`:

```yaml
scrape_configs:
  - job_name: 'doradura-bot'
    static_configs:
      # Для Linux используйте IP хост-машины вместо host.docker.internal
      - targets: ['172.17.0.1:9090']
      # Или найдите IP: ip addr show docker0
```

Для Railway/production:

```yaml
scrape_configs:
  - job_name: 'doradura-bot'
    static_configs:
      - targets: ['doradura-bot:9090']  # Имя сервиса в Railway
```

### Шаг 3: Настройка Grafana

1. Откройте http://localhost:3000
2. Войдите с admin/admin (смените пароль)
3. Дашборд "Doradura Bot - Overview" должен появиться автоматически
4. Если нет - импортируйте из `grafana/dashboards/doradura_overview.json`

#### Создание дополнительных дашбордов

**Performance Dashboard:**
- Добавьте панель с `rate(doradura_download_success_total[5m])`
- Добавьте heat map для duration: `histogram_quantile(0.95, rate(doradura_download_duration_seconds_bucket[5m]))`

**Business Dashboard:**
- Revenue timeline: `increase(doradura_revenue_total_stars[1h])`
- Conversion rate: `rate(doradura_new_subscriptions_total[1h]) / rate(doradura_command_usage_total{command="start"}[1h])`

### Шаг 4: Настройка Alerts

Alerts уже настроены в `prometheus/rules/doradura_alerts.yml`.

**Типы алертов:**
- 🔴 Critical: Требуют немедленного действия
- 🟡 Warning: Требуют внимания

**Основные алерты:**
- `HighErrorRate` - error rate > 10%
- `QueueBackup` - очередь > 100 задач
- `BotDown` - бот недоступен > 2 мин
- `SlowDownloads` - p95 duration > 60s
- `PaymentFailures` - ошибки платежей

**Просмотр активных алертов:**
```bash
# В Prometheus
curl http://localhost:9091/api/v1/alerts

# В AlertManager
curl http://localhost:9093/api/v1/alerts
```

### Шаг 5: Интеграция с Telegram (опционально)

Чтобы получать алерты в Telegram, у вас есть 2 варианта:

#### Вариант 1: Использовать встроенную систему алертов бота

Ваш бот уже имеет `AlertManager` в `src/core/alerts.rs`, который отправляет уведомления в Telegram. Просто убедитесь что он запущен в `main.rs`.

#### Вариант 2: Настроить webhook от AlertManager

1. Добавьте endpoint в бот для приема webhooks:

```rust
// В metrics_server.rs
.route("/alerts", post(alert_webhook_handler))

async fn alert_webhook_handler(
    State(bot): State<Bot>,
    Json(payload): Json<AlertWebhook>
) -> impl IntoResponse {
    // Обработать алерт от Prometheus AlertManager
    // Отправить в Telegram админу
}
```

2. Обновите `alertmanager.yml`:

```yaml
receivers:
  - name: 'telegram'
    webhook_configs:
      - url: 'http://host.docker.internal:9090/alerts'
```

---

## Развертывание в Production (Railway)

### Вариант 1: Встроенный мониторинг (рекомендуется)

Используйте только встроенный metrics server и Telegram команды:
- `/analytics` - основной дашборд
- `/health` - состояние системы
- `/metrics performance` - детальные метрики

Преимущества:
- ✅ Нет дополнительных сервисов
- ✅ Работает из коробки
- ✅ Метрики в Telegram
- ✅ Автоматические алерты

### Вариант 2: Полный стек с Prometheus + Grafana

#### На Railway

1. Добавьте Prometheus как отдельный сервис:

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

2. Создайте Dockerfiles:

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

3. Настройте networking в Railway:
   - Сервисы в одном проекте могут общаться по внутренним доменам
   - `prometheus.railway.internal:9090`
   - `doradura-bot.railway.internal:9090`

#### На обычном VPS/сервере

```bash
# Скопируйте файлы на сервер
scp -r docker-compose.monitoring.yml prometheus.yml grafana/ prometheus/ user@server:~/monitoring/

# На сервере
cd ~/monitoring
docker-compose -f docker-compose.monitoring.yml up -d

# Настройте reverse proxy (nginx) для доступа к Grafana
```

**nginx config для Grafana:**
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

## Полезные Запросы в Prometheus

### Performance

```promql
# Средняя длительность загрузки
histogram_quantile(0.5, rate(doradura_download_duration_seconds_bucket[5m]))

# Success rate
sum(rate(doradura_download_success_total[5m])) /
(sum(rate(doradura_download_success_total[5m])) + sum(rate(doradura_download_failure_total[5m])))

# Количество загрузок в час
increase(doradura_download_success_total[1h])
```

### Business

```promql
# Выручка за сегодня
increase(doradura_revenue_total_stars[1d])

# Конверсия в подписку
rate(doradura_new_subscriptions_total[1h]) / rate(doradura_command_usage_total{command="start"}[1h])

# Активные подписки
sum(doradura_active_subscriptions)
```

### Health

```promql
# Процент ошибок
sum(rate(doradura_errors_total[5m])) /
sum(rate(doradura_download_success_total[5m]) + rate(doradura_download_failure_total[5m]))

# Глубина очереди
doradura_queue_depth

# Uptime в днях
doradura_bot_uptime_seconds / 86400
```

---

## Troubleshooting

### Prometheus не видит метрики бота

```bash
# Проверьте что metrics сервер запущен
curl http://localhost:9090/metrics

# Проверьте targets в Prometheus
curl http://localhost:9091/api/v1/targets | jq

# Для Docker на Mac/Windows используйте host.docker.internal
# Для Linux найдите IP docker0: ip addr show docker0
```

### Grafana не показывает данные

1. Проверьте datasource: Configuration → Data Sources → Prometheus
2. Проверьте что URL правильный: `http://prometheus:9090`
3. Нажмите "Test" чтобы проверить соединение
4. Проверьте что в Prometheus есть данные: http://localhost:9091/graph

### Alerts не срабатывают

```bash
# Проверьте что rules загружены
curl http://localhost:9091/api/v1/rules | jq

# Проверьте активные алерты
curl http://localhost:9091/api/v1/alerts | jq

# Проверьте логи Prometheus
docker-compose -f docker-compose.monitoring.yml logs prometheus
```

### Высокое использование памяти

Prometheus хранит метрики в памяти. Если памяти мало:

1. Уменьшите retention:
```yaml
# В docker-compose.monitoring.yml
command:
  - '--storage.tsdb.retention.time=7d'  # Вместо 30d
```

2. Уменьшите scrape interval:
```yaml
# В prometheus.yml
global:
  scrape_interval: 30s  # Вместо 15s
```

---

## Backup и Restore

### Backup данных Prometheus

```bash
# Остановить Prometheus
docker-compose -f docker-compose.monitoring.yml stop prometheus

# Создать backup
docker run --rm -v doradura_prometheus_data:/data -v $(pwd):/backup \
  alpine tar czf /backup/prometheus-backup.tar.gz -C /data .

# Запустить снова
docker-compose -f docker-compose.monitoring.yml start prometheus
```

### Backup дашбордов Grafana

```bash
# Экспорт дашборда через API
curl -H "Authorization: Bearer YOUR_API_KEY" \
  http://localhost:3000/api/dashboards/uid/doradura-overview > dashboard-backup.json
```

---

## Мониторинг Расходов

Для Railway/cloud провайдеров отслеживайте:

1. **CPU Usage** - Prometheus может использовать много CPU при большом количестве метрик
2. **Memory** - Метрики хранятся в RAM
3. **Storage** - Prometheus сохраняет данные на диск
4. **Network** - Scraping метрик генерирует трафик

**Рекомендации:**
- Используйте встроенные Telegram команды для production
- Prometheus + Grafana разворачивайте на отдельном сервере
- Или используйте managed сервисы (Grafana Cloud, Datadog и т.д.)

---

## Дополнительные Возможности

### 1. Node Exporter (системные метрики)

Добавьте в `docker-compose.monitoring.yml`:

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

### 2. Blackbox Exporter (проверка доступности)

Мониторинг доступности внешних сервисов (YouTube, bot API и т.д.)

### 3. Loki для логов

Централизованное хранение логов с интеграцией в Grafana

---

## Рекомендуемая Архитектура

### Development

```
Ваш компьютер:
├── doradura bot (с metrics на :9090)
└── docker-compose:
    ├── Prometheus (:9091)
    ├── Grafana (:3000)
    └── AlertManager (:9093)
```

### Production (Simple)

```
Railway/VPS:
└── doradura bot (с metrics + Telegram команды)
    - Используйте /analytics для мониторинга
    - Автоматические алерты в Telegram
    - Нет дополнительных сервисов
```

### Production (Advanced)

```
Railway/VPS:
├── doradura bot (:9090 internal)
└── Отдельный VPS для мониторинга:
    ├── Prometheus (scrapes bot)
    ├── Grafana (+ reverse proxy nginx)
    └── AlertManager (webhooks в Telegram)
```

---

## Полезные Ссылки

- [Prometheus Documentation](https://prometheus.io/docs/)
- [Grafana Documentation](https://grafana.com/docs/)
- [PromQL Tutorial](https://prometheus.io/docs/prometheus/latest/querying/basics/)
- [Best Practices for Naming Metrics](https://prometheus.io/docs/practices/naming/)
