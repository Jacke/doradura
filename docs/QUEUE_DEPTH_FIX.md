# Исправление Queue Depth в Grafana Dashboard

## Проблема

Панель "Queue Depth" в Grafana dashboard не показывала данные, хотя метрика экспортировалась корректно.

## Диагностика

### 1. Проверка метрик в боте

```bash
curl -s http://localhost:9094/metrics | grep "doradura_queue_depth"
```

**Результат:**
```
# HELP doradura_queue_depth Current number of tasks in queue by priority
# TYPE doradura_queue_depth gauge
doradura_queue_depth{priority="high"} 0
doradura_queue_depth{priority="low"} 0
doradura_queue_depth{priority="medium"} 0
# HELP doradura_queue_depth_total Total number of tasks in queue
# TYPE doradura_queue_depth_total gauge
doradura_queue_depth_total 0
```

✅ Обе метрики экспортируются!

### 2. Проверка query в Prometheus

```bash
curl -s 'http://localhost:9091/api/v1/query?query=doradura_queue_depth' | jq '.data.result | length'
```

**Результат:** `3` - Возвращает 3 временных ряда (по одному на каждый priority)

**Проблема:** Query `doradura_queue_depth` возвращает **множественные** временные ряды:
- `doradura_queue_depth{priority="high"} 0`
- `doradura_queue_depth{priority="low"} 0`
- `doradura_queue_depth{priority="medium"} 0`

Grafana панель типа "Stat" (одно число) не знает как отобразить 3 значения одновременно!

### 3. Проверка правильной метрики

```bash
curl -s 'http://localhost:9091/api/v1/query?query=doradura_queue_depth_total' | jq '.data.result'
```

**Результат:**
```json
[
  {
    "metric": {
      "__name__": "doradura_queue_depth_total",
      "instance": "doradura-bot",
      "job": "doradura-bot"
    },
    "value": [1765740505.585, "0"]
  }
]
```

✅ Возвращает **одно** значение - именно то что нужно для панели!

## Причина

В dashboard использовался неправильный query:

**Было:**
```json
{
  "expr": "doradura_queue_depth",
  "refId": "A"
}
```

Этот query возвращает метрику **с labels** (по приоритетам), что приводит к множественным временным рядам.

## Решение

Изменили query на использование `doradura_queue_depth_total` - метрику **без labels**, которая показывает общую глубину очереди:

**Стало:**
```json
{
  "expr": "doradura_queue_depth_total",
  "refId": "A"
}
```

### Файл изменен

[grafana/dashboards/doradura_overview.json:201](grafana/dashboards/doradura_overview.json#L201)

## Альтернативные Решения

Если бы мы хотели использовать метрику с labels, были бы следующие варианты:

### Вариант 1: Сумма всех приоритетов

```promql
sum(doradura_queue_depth)
```

Сложит все приоритеты: high + medium + low

### Вариант 2: Показать все приоритеты отдельно

Изменить тип панели с "Stat" на "Time series" и показать 3 линии:
```promql
doradura_queue_depth
```

Тогда legendFormat можно установить как `{{ priority }}` чтобы видеть high/medium/low отдельно.

### Вариант 3: Только высокий приоритет

```promql
doradura_queue_depth{priority="high"}
```

## Разница Между Метриками

| Метрика | Тип | Labels | Когда Использовать |
|---------|-----|--------|-------------------|
| `doradura_queue_depth` | GaugeVec | `priority` (high/medium/low) | Когда нужна детализация по приоритетам |
| `doradura_queue_depth_total` | Gauge | Нет | Когда нужно общее число задач в очереди |

## Как Обновляются Метрики

### В Коде

[src/download/queue.rs](src/download/queue.rs) или где обрабатывается очередь:

```rust
use crate::core::metrics;

// Обновить по приоритетам
metrics::update_queue_depth("high", high_priority_count);
metrics::update_queue_depth("medium", medium_priority_count);
metrics::update_queue_depth("low", low_priority_count);

// Обновить общую глубину
let total = high_priority_count + medium_priority_count + low_priority_count;
metrics::update_queue_depth_total(total);
```

### В metrics.rs

[src/core/metrics.rs:382-389](src/core/metrics.rs#L382-L389)

```rust
/// Helper function to update queue depth
pub fn update_queue_depth(priority: &str, depth: usize) {
    QUEUE_DEPTH.with_label_values(&[priority]).set(depth as f64);
}

/// Helper function to update total queue depth
pub fn update_queue_depth_total(depth: usize) {
    QUEUE_DEPTH_TOTAL.set(depth as f64);
}
```

## Проверка Исправления

### 1. Проверить метрику

```bash
curl -s http://localhost:9094/metrics | grep "doradura_queue_depth_total"
```

**Ожидаемый результат:**
```
doradura_queue_depth_total 0
```

### 2. Проверить в Prometheus

```bash
curl -s 'http://localhost:9091/api/v1/query?query=doradura_queue_depth_total' | jq '.data.result[0].value[1]'
```

**Ожидаемый результат:** `"0"` (или текущее значение очереди)

### 3. Проверить в Grafana

1. Откройте dashboard: http://localhost:3000/d/doradura-overview
2. Найдите панель "Queue Depth" (обычно в верхнем ряду справа)
3. Должно показываться число: **0** (или текущее значение)
4. Цвет зависит от thresholds:
   - 🟢 Зеленый: 0-49 задач
   - 🟡 Желтый: 50-99 задач
   - 🔴 Красный: 100+ задач

## Применение Исправления

Dashboard обновляется автоматически через Grafana provisioning. Если изменения не появились:

```bash
# Перезапустить Grafana
docker-compose -f docker-compose.monitoring.yml restart grafana

# Проверить что Grafana запустилась
curl http://localhost:3000/api/health
```

## Связанные Панели

Другие панели в dashboard уже используют правильные queries:

✅ **Download Rate** - `sum(rate(doradura_download_success_total[5m]))`
- Сумма всех форматов и качеств

✅ **Active Subscriptions** - `sum(doradura_active_subscriptions)`
- Сумма всех планов (free/premium/vip)

✅ **Downloads by Format** - `sum by (format) (rate(doradura_format_requests_total[5m]))`
- Группировка по формату (показывает mp3, mp4, srt отдельно)

✅ **Errors by Category** - `sum by (category) (rate(doradura_errors_total[5m]))`
- Группировка по категории ошибок

## Best Practices

### Когда Использовать sum()

```promql
# Если метрика с labels, но нужно одно число
sum(metric_with_labels)

# Пример
sum(doradura_active_subscriptions)  # Сумма free + premium + vip
```

### Когда Использовать sum by (label)

```promql
# Если нужно видеть разбивку по каждому значению label
sum by (label_name) (metric)

# Пример
sum by (format) (rate(doradura_format_requests_total[5m]))
# Покажет отдельно: mp3, mp4, srt
```

### Когда Использовать Метрику Напрямую

```promql
# Если метрика БЕЗ labels
metric_without_labels

# Пример
doradura_queue_depth_total
doradura_revenue_total_stars
doradura_daily_active_users
```

## Итоговое Состояние

После исправления все панели в dashboard работают корректно:

- ✅ Download Rate
- ✅ Success Rate
- ✅ **Queue Depth** ← ИСПРАВЛЕНО
- ✅ Download Duration (p50/p95/p99)
- ✅ Downloads by Format
- ✅ Daily Active Users
- ✅ Total Revenue
- ✅ Active Subscriptions
- ✅ Errors by Category

## Связанные Файлы

- [grafana/dashboards/doradura_overview.json](grafana/dashboards/doradura_overview.json) - Dashboard конфигурация
- [src/core/metrics.rs](src/core/metrics.rs) - Определение метрик
- [METRICS_DASHBOARD_FIX.md](METRICS_DASHBOARD_FIX.md) - Основное исправление метрик
- [HOW_TO_VIEW_METRICS.md](HOW_TO_VIEW_METRICS.md) - Руководство по просмотру метрик
