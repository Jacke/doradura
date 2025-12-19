# Исправление Grafana Dashboard - Связь Метрик с Кодом

## Проблема

Dashboard в Grafana ([http://localhost:3000/d/doradura-overview](http://localhost:3000/d/doradura-overview)) не показывал данные. Все панели были пустыми, хотя бот работал и Prometheus собирал некоторые метрики.

## Диагностика

### Шаг 1: Проверка метрик бота

```bash
curl -s http://localhost:9094/metrics | grep -E "^doradura_"
```

**Результат:** Экспортировались только базовые метрики без labels:
- `doradura_bot_uptime_seconds`
- `doradura_daily_active_users`
- `doradura_total_users`
- `doradura_revenue_total_stars`
- `doradura_queue_depth_total`

**Отсутствовали:**
- `doradura_download_success_total`
- `doradura_download_failure_total`
- `doradura_format_requests_total`
- `doradura_errors_total`
- `doradura_active_subscriptions`

### Шаг 2: Проверка дашборда

Dashboard использует следующие метрики:

```promql
# Download Rate
sum(rate(doradura_download_success_total[5m]))
sum(rate(doradura_download_failure_total[5m]))

# Success Rate
sum(rate(doradura_download_success_total[5m])) /
(sum(rate(doradura_download_success_total[5m])) +
 sum(rate(doradura_download_failure_total[5m]))) * 100

# Download Duration Percentiles
histogram_quantile(0.5, rate(doradura_download_duration_seconds_bucket[5m]))
histogram_quantile(0.95, rate(doradura_download_duration_seconds_bucket[5m]))
histogram_quantile(0.99, rate(doradura_download_duration_seconds_bucket[5m]))

# Format Requests
sum by (format) (rate(doradura_format_requests_total[5m]))

# Errors by Category
sum by (category) (rate(doradura_errors_total[5m]))

# Active Subscriptions
sum(doradura_active_subscriptions)

# And others...
```

### Шаг 3: Проверка кода

В [src/core/metrics.rs](src/core/metrics.rs) метрики были **объявлены** (строки 50-64):

```rust
pub static ref DOWNLOAD_SUCCESS_TOTAL: CounterVec = register_counter_vec!(
    "doradura_download_success_total",
    "Total number of successful downloads",
    &["format", "quality"]
).unwrap();

pub static ref DOWNLOAD_FAILURE_TOTAL: CounterVec = register_counter_vec!(
    "doradura_download_failure_total",
    "Total number of failed downloads",
    &["format", "error_type"]
).unwrap();
```

И даже **использовались** в [src/download/downloader.rs](src/download/downloader.rs):

```rust
// Line 1932
metrics::record_download_success("mp3", quality);

// Line 1945
metrics::record_download_failure("mp3", error_type);
```

**НО:** Prometheus CounterVec/GaugeVec с labels **не экспортируют метрики**, пока не будет создана хотя бы одна временная серия (time series) для какой-либо комбинации labels.

## Причина

Prometheus метрики с labels (`CounterVec`, `GaugeVec`, `HistogramVec`) регистрируются через `lazy_static`, но:

1. **Ленивая инициализация**: Метрика регистрируется в Prometheus Registry при первом обращении к `lazy_static`
2. **Временные ряды создаются по требованию**: Конкретная комбинация labels (например, `{format="mp3", quality="320k"}`) создается только при первом вызове `.with_label_values()`
3. **Prometheus не экспортирует пустые серии**: Если комбинация labels никогда не использовалась, она не появится в `/metrics` endpoint

### Проблема с нашим кодом

В функции `init_metrics()` (строка 310 в [src/core/metrics.rs](src/core/metrics.rs)) метрики **регистрировались**, но не **инициализировались**:

```rust
// ДО исправления:
pub fn init_metrics() {
    log::info!("Initializing metrics registry...");

    // Только ссылка - регистрирует метрику, но НЕ создает временные ряды
    let _ = &*DOWNLOAD_SUCCESS_TOTAL;
    let _ = &*DOWNLOAD_FAILURE_TOTAL;
    // ...
}
```

Это означало:
- Метрика зарегистрирована в Registry ✅
- Но нет ни одной временной серии (time series) ❌
- `/metrics` endpoint не показывает метрику ❌
- Grafana не видит данных ❌

## Решение

Добавили явную инициализацию временных рядов для всех важных комбинаций labels в функции `init_metrics()`.

### Изменения в [src/core/metrics.rs](src/core/metrics.rs)

#### 1. Download Metrics (строки 321-342)

```rust
// Initialize download counters with common format combinations
// This ensures they appear in /metrics even with 0 values
DOWNLOAD_SUCCESS_TOTAL.with_label_values(&["mp3", "320k"]);
DOWNLOAD_SUCCESS_TOTAL.with_label_values(&["mp3", "default"]);
DOWNLOAD_SUCCESS_TOTAL.with_label_values(&["mp4", "1080p"]);
DOWNLOAD_SUCCESS_TOTAL.with_label_values(&["mp4", "720p"]);
DOWNLOAD_SUCCESS_TOTAL.with_label_values(&["mp4", "480p"]);
DOWNLOAD_SUCCESS_TOTAL.with_label_values(&["srt", "default"]);
DOWNLOAD_SUCCESS_TOTAL.with_label_values(&["txt", "default"]);

DOWNLOAD_FAILURE_TOTAL.with_label_values(&["mp3", "timeout"]);
DOWNLOAD_FAILURE_TOTAL.with_label_values(&["mp3", "file_too_large"]);
DOWNLOAD_FAILURE_TOTAL.with_label_values(&["mp3", "ytdlp"]);
DOWNLOAD_FAILURE_TOTAL.with_label_values(&["mp3", "network"]);
DOWNLOAD_FAILURE_TOTAL.with_label_values(&["mp3", "other"]);
DOWNLOAD_FAILURE_TOTAL.with_label_values(&["mp4", "timeout"]);
DOWNLOAD_FAILURE_TOTAL.with_label_values(&["mp4", "file_too_large"]);
DOWNLOAD_FAILURE_TOTAL.with_label_values(&["mp4", "ytdlp"]);
DOWNLOAD_FAILURE_TOTAL.with_label_values(&["mp4", "network"]);
DOWNLOAD_FAILURE_TOTAL.with_label_values(&["mp4", "other"]);
DOWNLOAD_FAILURE_TOTAL.with_label_values(&["srt", "other"]);
DOWNLOAD_FAILURE_TOTAL.with_label_values(&["txt", "other"]);
```

#### 2. Business Metrics - Subscriptions (строки 353-372)

```rust
// Initialize subscription metrics by plan
ACTIVE_SUBSCRIPTIONS.with_label_values(&["free"]);
ACTIVE_SUBSCRIPTIONS.with_label_values(&["premium"]);
ACTIVE_SUBSCRIPTIONS.with_label_values(&["vip"]);

// Initialize revenue by plan
REVENUE_BY_PLAN.with_label_values(&["premium"]);
REVENUE_BY_PLAN.with_label_values(&["vip"]);

// Initialize new subscriptions
NEW_SUBSCRIPTIONS_TOTAL.with_label_values(&["premium", "true"]);
NEW_SUBSCRIPTIONS_TOTAL.with_label_values(&["premium", "false"]);
NEW_SUBSCRIPTIONS_TOTAL.with_label_values(&["vip", "true"]);
NEW_SUBSCRIPTIONS_TOTAL.with_label_values(&["vip", "false"]);

// Initialize payment metrics
PAYMENT_SUCCESS_TOTAL.with_label_values(&["premium", "true"]);
PAYMENT_SUCCESS_TOTAL.with_label_values(&["premium", "false"]);
PAYMENT_SUCCESS_TOTAL.with_label_values(&["vip", "true"]);
PAYMENT_SUCCESS_TOTAL.with_label_values(&["vip", "false"]);
```

#### 3. Error Metrics (строки 364-371)

```rust
// Initialize error counters with common error types
ERRORS_TOTAL.with_label_values(&["ytdlp", "download"]);
ERRORS_TOTAL.with_label_values(&["network", "download"]);
ERRORS_TOTAL.with_label_values(&["telegram", "send_file"]);
ERRORS_TOTAL.with_label_values(&["rate_limit", "download"]);
ERRORS_TOTAL.with_label_values(&["database", "query"]);
ERRORS_TOTAL.with_label_values(&["timeout", "download"]);
ERRORS_TOTAL.with_label_values(&["file_too_large", "download"]);
```

#### 4. Queue Depth (строки 373-376)

```rust
// Initialize queue depth gauges
QUEUE_DEPTH.with_label_values(&["low"]);
QUEUE_DEPTH.with_label_values(&["medium"]);
QUEUE_DEPTH.with_label_values(&["high"]);
```

#### 5. Format Requests (строки 387-395)

```rust
// Initialize format request counters
FORMAT_REQUESTS_TOTAL.with_label_values(&["mp3", "free"]);
FORMAT_REQUESTS_TOTAL.with_label_values(&["mp3", "premium"]);
FORMAT_REQUESTS_TOTAL.with_label_values(&["mp3", "vip"]);
FORMAT_REQUESTS_TOTAL.with_label_values(&["mp4", "free"]);
FORMAT_REQUESTS_TOTAL.with_label_values(&["mp4", "premium"]);
FORMAT_REQUESTS_TOTAL.with_label_values(&["mp4", "vip"]);
FORMAT_REQUESTS_TOTAL.with_label_values(&["srt", "free"]);
FORMAT_REQUESTS_TOTAL.with_label_values(&["txt", "free"]);
```

#### 6. Command Usage (строки 397-402)

```rust
// Initialize command usage counters
COMMAND_USAGE_TOTAL.with_label_values(&["start"]);
COMMAND_USAGE_TOTAL.with_label_values(&["help"]);
COMMAND_USAGE_TOTAL.with_label_values(&["settings"]);
COMMAND_USAGE_TOTAL.with_label_values(&["history"]);
COMMAND_USAGE_TOTAL.with_label_values(&["info"]);
```

#### 7. Users by Plan (строки 404-407)

```rust
// Initialize users by plan gauges
USERS_BY_PLAN.with_label_values(&["free"]);
USERS_BY_PLAN.with_label_values(&["premium"]);
USERS_BY_PLAN.with_label_values(&["vip"]);
```

## Проверка Исправления

### 1. Проверка метрик бота

```bash
curl -s http://localhost:9094/metrics | grep "doradura_download_success_total{"
```

**Результат:**
```
doradura_download_success_total{format="mp3",quality="320k"} 0
doradura_download_success_total{format="mp3",quality="default"} 0
doradura_download_success_total{format="mp4",quality="1080p"} 0
doradura_download_success_total{format="mp4",quality="480p"} 0
doradura_download_success_total{format="mp4",quality="720p"} 0
doradura_download_success_total{format="srt",quality="default"} 0
doradura_download_success_total{format="txt",quality="default"} 0
```

✅ Все комбинации labels экспортируются с нулевыми значениями!

### 2. Проверка Prometheus

```bash
curl -s 'http://localhost:9091/api/v1/query?query=doradura_download_success_total' | jq '.data.result | length'
```

**Результат:** `7` временных рядов

✅ Prometheus собирает все метрики!

### 3. Проверка Grafana

Откройте [http://localhost:3000/d/doradura-overview](http://localhost:3000/d/doradura-overview)

**Ожидаемый результат:**
- ✅ **Download Rate** панель показывает 0 req/sec (но график есть)
- ✅ **Success Rate** панель показывает 0% или "No data" (нормально для нулевых значений)
- ✅ **Queue Depth** панель показывает 0
- ✅ **Download Duration** панель показывает графики (может быть No data, это нормально)
- ✅ **Daily Active Users** панель показывает текущее значение
- ✅ **Total Revenue** панель показывает 0⭐
- ✅ **Active Subscriptions** панель показывает 0
- ✅ **Downloads by Format** панель показывает 0 для всех форматов
- ✅ **Errors by Category** панель показывает 0 ошибок

**Важно:** Графики могут показывать "No data" для вычисляемых метрик (rate, histogram_quantile), когда все счетчики = 0. Это нормально! Как только произойдут загрузки, данные появятся.

## Как Это Работает Теперь

### Жизненный Цикл Метрик

1. **Старт бота** → Вызывается `init_metrics()` в [src/main.rs:75](src/main.rs#L75)

2. **Инициализация** → Создаются временные ряды для всех важных комбинаций labels:
   ```rust
   DOWNLOAD_SUCCESS_TOTAL.with_label_values(&["mp3", "320k"]);
   // Создается time series: doradura_download_success_total{format="mp3",quality="320k"} 0
   ```

3. **Экспорт в Prometheus** → Метрики доступны в `/metrics` endpoint с нулевыми значениями

4. **Prometheus Scraping** → Prometheus каждые 10 секунд собирает метрики с бота

5. **Grafana Query** → Grafana выполняет PromQL запросы и получает данные

6. **Использование в коде** → Когда происходит загрузка:
   ```rust
   // src/download/downloader.rs:1932
   metrics::record_download_success("mp3", quality);
   // Инкрементирует: doradura_download_success_total{format="mp3",quality="320k"} = 1
   ```

7. **Обновление dashboard** → Grafana автоматически обновляется (каждые 30 секунд по умолчанию)

## Связь Dashboard Панелей с Кодом

| Dashboard Panel | PromQL Query | Metric Source | Code Location |
|----------------|--------------|---------------|---------------|
| **Download Rate** | `sum(rate(doradura_download_success_total[5m]))` | `DOWNLOAD_SUCCESS_TOTAL` | [downloader.rs:1932](src/download/downloader.rs#L1932) |
| **Success Rate** | `sum(rate(..._success...)) / (sum(..._success...) + sum(..._failure...)) * 100` | `DOWNLOAD_SUCCESS_TOTAL`, `DOWNLOAD_FAILURE_TOTAL` | [downloader.rs:1932,1945](src/download/downloader.rs#L1932) |
| **Queue Depth** | `doradura_queue_depth` | `QUEUE_DEPTH` | [queue.rs](src/download/queue.rs) via `metrics::update_queue_depth()` |
| **Download Duration** | `histogram_quantile(0.95, rate(doradura_download_duration_seconds_bucket[5m]))` | `DOWNLOAD_DURATION_SECONDS` | [downloader.rs:1550](src/download/downloader.rs#L1550) timer |
| **Downloads by Format** | `sum by (format) (rate(doradura_format_requests_total[5m]))` | `FORMAT_REQUESTS_TOTAL` | Used in commands handler |
| **Daily Active Users** | `doradura_daily_active_users` | `DAILY_ACTIVE_USERS` | Updated periodically |
| **Total Revenue** | `doradura_revenue_total_stars` | `REVENUE_TOTAL_STARS` | Updated on payments |
| **Active Subscriptions** | `sum(doradura_active_subscriptions)` | `ACTIVE_SUBSCRIPTIONS` | Updated on sub changes |
| **Errors by Category** | `sum by (category) (rate(doradura_errors_total[5m]))` | `ERRORS_TOTAL` | [downloader.rs](src/download/downloader.rs) via `metrics::record_error()` |

## Best Practices Learned

### 1. Всегда Инициализируйте Метрики с Labels

❌ **Плохо:**
```rust
// Только регистрация
let _ = &*MY_METRIC;
```

✅ **Хорошо:**
```rust
// Регистрация + создание временных рядов
let _ = &*MY_METRIC;
MY_METRIC.with_label_values(&["common", "value1"]);
MY_METRIC.with_label_values(&["common", "value2"]);
```

### 2. Инициализируйте Все Важные Комбинации

Если dashboard использует метрику с labels, инициализируйте все возможные комбинации:

```rust
// Если dashboard группирует по plan: sum by (plan) (...)
METRIC.with_label_values(&["free"]);
METRIC.with_label_values(&["premium"]);
METRIC.with_label_values(&["vip"]);
```

### 3. Документируйте Labels

Добавьте комментарии о том, какие labels ожидаются:

```rust
/// Active subscriptions count by plan
/// Labels: plan (free/premium/vip)
pub static ref ACTIVE_SUBSCRIPTIONS: GaugeVec = ...
```

### 4. Проверяйте Metrics Endpoint

После любого изменения метрик:

```bash
curl http://localhost:9094/metrics | grep "YOUR_METRIC"
```

Убедитесь, что метрика присутствует **до** проверки Grafana.

## Дополнительные Метрики

Все следующие метрики теперь экспортируются и готовы к использованию в дашбордах:

### Performance Metrics
- `doradura_download_duration_seconds` (histogram)
- `doradura_download_success_total` (counter with labels)
- `doradura_download_failure_total` (counter with labels)
- `doradura_queue_processing_duration_seconds` (histogram)
- `doradura_queue_wait_time_seconds` (histogram)

### Business Metrics
- `doradura_active_subscriptions` (gauge with plan label)
- `doradura_revenue_total_stars` (counter)
- `doradura_revenue_by_plan` (counter with plan label)
- `doradura_new_subscriptions_total` (counter with plan, is_recurring)
- `doradura_payment_success_total` (counter with plan, is_recurring)

### System Health Metrics
- `doradura_errors_total` (counter with error_type, operation)
- `doradura_queue_depth` (gauge with priority label)
- `doradura_queue_depth_total` (gauge)
- `doradura_ytdlp_health_status` (gauge)
- `doradura_db_connections_active` (gauge)
- `doradura_db_connections_idle` (gauge)

### User Engagement Metrics
- `doradura_daily_active_users` (gauge)
- `doradura_monthly_active_users` (gauge)
- `doradura_command_usage_total` (counter with command label)
- `doradura_format_requests_total` (counter with format, plan labels)
- `doradura_total_users` (gauge)
- `doradura_users_by_plan` (gauge with plan label)

## Следующие Шаги

1. **Сделайте тестовую загрузку** → Метрики начнут обновляться
2. **Проверьте dashboard через час** → Увидите реальные данные
3. **Создайте дополнительные дашборды** при необходимости
4. **Настройте alerts** в Prometheus для критичных метрик

## Полезные Команды

```bash
# Проверка всех метрик бота
curl -s http://localhost:9094/metrics | grep "^doradura_"

# Проверка конкретной метрики
curl -s http://localhost:9094/metrics | grep "doradura_download_success_total"

# Проверка в Prometheus
curl -s 'http://localhost:9091/api/v1/query?query=doradura_download_success_total' | jq

# Перезапуск мониторинга (если нужно)
docker-compose -f docker-compose.monitoring.yml restart

# Просмотр логов Prometheus
docker-compose -f docker-compose.monitoring.yml logs -f prometheus
```

## Связанные Файлы

- [src/core/metrics.rs](src/core/metrics.rs) - Определение и инициализация метрик
- [src/download/downloader.rs](src/download/downloader.rs) - Использование download метрик
- [grafana/dashboards/doradura_overview.json](grafana/dashboards/doradura_overview.json) - Grafana dashboard
- [prometheus.yml](prometheus.yml) - Конфигурация Prometheus
- [HOW_TO_VIEW_METRICS.md](HOW_TO_VIEW_METRICS.md) - Руководство по просмотру метрик
- [MONITORING_CHEATSHEET.md](MONITORING_CHEATSHEET.md) - Шпаргалка по мониторингу
