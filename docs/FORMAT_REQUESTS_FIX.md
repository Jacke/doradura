# Исправление Метрики "Downloads by Format"

## Проблема

Панель "Downloads by Format" в Grafana dashboard не показывала данные, хотя метрика `doradura_format_requests_total` экспортировалась с нулевыми значениями.

## Диагностика

### 1. Проверка экспорта метрики

```bash
curl -s http://localhost:9094/metrics | grep "doradura_format_requests_total"
```

**Результат:**
```
# TYPE doradura_format_requests_total counter
doradura_format_requests_total{format="mp3",plan="free"} 0
doradura_format_requests_total{format="mp3",plan="premium"} 0
doradura_format_requests_total{format="mp3",plan="vip"} 0
doradura_format_requests_total{format="mp4",plan="free"} 0
...
```

✅ Метрика экспортируется правильно!

### 2. Проверка query в Prometheus

```bash
curl 'http://localhost:9091/api/v1/query?query=sum%20by%20(format)%20(rate(doradura_format_requests_total%5B5m%5D))'
```

**Результат:**
```json
{
  "data": {
    "result": [
      {"metric": {"format": "mp3"}, "value": [1765740637.691, "0"]},
      {"metric": {"format": "mp4"}, "value": [1765740637.691, "0"]},
      {"metric": {"format": "srt"}, "value": [1765740637.691, "0"]},
      {"metric": {"format": "txt"}, "value": [1765740637.691, "0"]}
    ]
  }
}
```

✅ Query работает и возвращает данные!

### 3. Проверка использования в коде

```bash
grep -r "FORMAT_REQUESTS_TOTAL" src/
```

**Результат:**
- ✅ Объявлена в [src/core/metrics.rs:267](src/core/metrics.rs#L267)
- ✅ Инициализирована в [src/core/metrics.rs:409-416](src/core/metrics.rs#L409-L416)
- ❌ **НЕ ИСПОЛЬЗУЕТСЯ** нигде в коде загрузки!

## Причина

Метрика `doradura_format_requests_total` была **объявлена и инициализирована**, но **никогда не инкрементировалась** в коде.

Метрика должна инкрементироваться каждый раз, когда пользователь запрашивает скачивание в определенном формате (mp3/mp4/srt/txt), но вызовы `.inc()` отсутствовали.

### Почему метрика была = 0

```promql
rate(doradura_format_requests_total[5m])
```

Функция `rate()` вычисляет **скорость изменения** счетчика за последние 5 минут. Если счетчик никогда не инкрементировался (всегда 0), то rate = 0, и панель показывает пустые данные.

## Решение

Добавили инкрементацию метрики во все функции загрузки.

### 1. Создали Helper Функцию

[src/core/metrics.rs:456-459](src/core/metrics.rs#L456-L459)

```rust
/// Helper function to record format request
pub fn record_format_request(format: &str, plan: &str) {
    FORMAT_REQUESTS_TOTAL.with_label_values(&[format, plan]).inc();
}
```

### 2. Добавили Инкремент в download_and_send_audio

[src/download/downloader.rs:1548-1564](src/download/downloader.rs#L1548-L1564)

```rust
tokio::spawn(async move {
    log::info!("Inside spawn for audio download, chat_id: {}", chat_id);
    let mut progress_msg = ProgressMessage::new(chat_id);
    let start_time = std::time::Instant::now();

    // Get user plan for metrics
    let user_plan = if let Some(ref pool) = db_pool_clone {
        if let Ok(conn) = db::get_connection(pool) {
            db::get_user(&conn, chat_id.0)
                .ok()
                .flatten()
                .map(|u| u.plan)
                .unwrap_or_else(|| "free".to_string())
        } else {
            "free".to_string()
        }
    } else {
        "free".to_string()
    };

    // Record format request for metrics
    metrics::record_format_request("mp3", &user_plan);

    // ... rest of the function
});
```

**Логика:**
1. Получаем план пользователя из БД (`free`, `premium`, или `vip`)
2. Если БД недоступна или пользователя нет → используем `"free"` по умолчанию
3. Вызываем `record_format_request("mp3", &user_plan)` → инкрементирует счетчик

### 3. Добавили Инкремент в download_and_send_video

[src/download/downloader.rs:2740-2756](src/download/downloader.rs#L2740-L2756)

```rust
tokio::spawn(async move {
    let mut progress_msg = ProgressMessage::new(chat_id);
    let start_time = std::time::Instant::now();

    // Get user plan for metrics
    let user_plan = if let Some(ref pool) = db_pool_clone {
        if let Ok(conn) = db::get_connection(pool) {
            db::get_user(&conn, chat_id.0)
                .ok()
                .flatten()
                .map(|u| u.plan)
                .unwrap_or_else(|| "free".to_string())
        } else {
            "free".to_string()
        }
    } else {
        "free".to_string()
    };

    // Record format request for metrics
    metrics::record_format_request("mp4", &user_plan);

    // ... rest of the function
});
```

### 4. Добавили Инкремент в download_and_send_subtitles

[src/download/downloader.rs:3525-3542](src/download/downloader.rs#L3525-L3542)

```rust
tokio::spawn(async move {
    let mut progress_msg = ProgressMessage::new(chat_id);
    let start_time = std::time::Instant::now();

    // Get user plan for metrics
    let user_plan = if let Some(ref pool) = db_pool_clone {
        if let Ok(conn) = db::get_connection(pool) {
            db::get_user(&conn, chat_id.0)
                .ok()
                .flatten()
                .map(|u| u.plan)
                .unwrap_or_else(|| "free".to_string())
        } else {
            "free".to_string()
        }
    } else {
        "free".to_string()
    };

    // Record format request for metrics
    let format = subtitle_format.as_str(); // "srt" or "txt"
    metrics::record_format_request(format, &user_plan);

    // ... rest of the function
});
```

## Как Это Работает

### Жизненный Цикл Метрики

1. **Пользователь запрашивает загрузку**
   - Отправляет URL боту
   - Выбирает формат через меню (MP3 / MP4 / Subtitles)

2. **Бот вызывает функцию загрузки**
   - `download_and_send_audio()` для MP3
   - `download_and_send_video()` для MP4
   - `download_and_send_subtitles()` для SRT/TXT

3. **Получение плана пользователя**
   ```rust
   let user_plan = db::get_user(&conn, chat_id.0)
       .map(|u| u.plan)
       .unwrap_or("free")
   ```
   - Запрашивает данные из таблицы `users`
   - Получает поле `plan`: `"free"`, `"premium"`, или `"vip"`
   - Fallback на `"free"` если пользователя нет в БД

4. **Инкрементация метрики**
   ```rust
   metrics::record_format_request("mp3", "free")
   // Инкрементирует: doradura_format_requests_total{format="mp3",plan="free"}
   ```

5. **Экспорт в Prometheus**
   ```
   doradura_format_requests_total{format="mp3",plan="free"} 1
   doradura_format_requests_total{format="mp3",plan="free"} 2
   doradura_format_requests_total{format="mp3",plan="free"} 3
   ...
   ```

6. **Prometheus вычисляет rate**
   ```promql
   rate(doradura_format_requests_total{format="mp3",plan="free"}[5m])
   # Результат: 0.01 req/sec (если было 3 запроса за 5 минут)
   ```

7. **Grafana агрегирует по формату**
   ```promql
   sum by (format) (rate(doradura_format_requests_total[5m]))
   # Суммирует все планы (free + premium + vip) для каждого формата
   # Результат:
   # {format="mp3"} 0.02
   # {format="mp4"} 0.01
   ```

8. **Dashboard показывает график**
   - Линия "mp3" - все MP3 запросы (от всех пользователей)
   - Линия "mp4" - все MP4 запросы
   - Линия "srt" - субтитры SRT
   - Линия "txt" - субтитры TXT

## Проверка Исправления

### 1. Проверить что метрика инициализирована

```bash
curl http://localhost:9094/metrics | grep "doradura_format_requests_total"
```

**Ожидаемый результат:**
```
doradura_format_requests_total{format="mp3",plan="free"} 0
doradura_format_requests_total{format="mp3",plan="premium"} 0
doradura_format_requests_total{format="mp3",plan="vip"} 0
...
```

### 2. Сделать тестовую загрузку

Отправьте URL боту и выберите MP3:
```
https://www.youtube.com/watch?v=dQw4w9WgXcQ
```

### 3. Проверить что метрика инкрементировалась

```bash
curl http://localhost:9094/metrics | grep "format_requests_total"
```

**Ожидаемый результат:**
```
doradura_format_requests_total{format="mp3",plan="free"} 1  ← Инкрементировалась!
doradura_format_requests_total{format="mp3",plan="premium"} 0
doradura_format_requests_total{format="mp3",plan="vip"} 0
...
```

### 4. Проверить в Prometheus

```bash
curl 'http://localhost:9091/api/v1/query?query=sum%20by%20(format)%20(rate(doradura_format_requests_total%5B5m%5D))'
```

**Ожидаемый результат:** Ненулевое значение для mp3

### 5. Проверить в Grafana

Откройте dashboard: http://localhost:3000/d/doradura-overview

Панель **"Downloads by Format"** должна показывать:
- Линия для `mp3` с ненулевым значением
- Возможно линии для `mp4`, `srt`, `txt` (если были запросы)

## Связь с Другими Метриками

### Метрики загрузки работают параллельно:

| Метрика | Когда Инкрементируется | Назначение |
|---------|------------------------|------------|
| `doradura_format_requests_total` | При **старте** загрузки | Считает запросы по формату и плану |
| `doradura_download_success_total` | При **успехе** загрузки | Считает успешные загрузки |
| `doradura_download_failure_total` | При **ошибке** загрузки | Считает неудачные загрузки |
| `doradura_download_duration_seconds` | При **завершении** загрузки | Измеряет длительность |

**Пример:**
```
1. Пользователь запрашивает MP3
   → format_requests_total{format="mp3"} += 1

2. Загрузка начинается
   → download_duration_seconds starts timer

3. Загрузка завершается успешно
   → download_success_total{format="mp3"} += 1
   → download_duration_seconds observes 8.5 seconds

ИЛИ

3. Загрузка завершается с ошибкой
   → download_failure_total{format="mp3",error_type="timeout"} += 1
   → download_duration_seconds observes 120 seconds
```

## Dashboard Query

Панель использует следующий PromQL query:

```promql
sum by (format) (rate(doradura_format_requests_total[5m]))
```

**Разбор:**
- `rate(doradura_format_requests_total[5m])` - вычисляет скорость изменения за 5 минут
- `sum by (format) (...)` - суммирует по всем планам (free + premium + vip)
- Результат: запросов в секунду для каждого формата

**Альтернативные queries:**

Показать breakdown по планам:
```promql
sum by (format, plan) (rate(doradura_format_requests_total[5m]))
```

Только premium пользователи:
```promql
sum by (format) (rate(doradura_format_requests_total{plan="premium"}[5m]))
```

Всего запросов (все форматы):
```promql
sum(rate(doradura_format_requests_total[5m]))
```

## Best Practices

### 1. Инкрементируйте Метрики Рано

✅ **Правильно:**
```rust
// В начале функции - ДО любых await или длительных операций
metrics::record_format_request("mp3", &user_plan);
```

❌ **Неправильно:**
```rust
// В конце функции - метрика не запишется если будет ранний return
if some_error {
    return Err(e); // Метрика НЕ записалась!
}
metrics::record_format_request("mp3", &user_plan);
```

### 2. Используйте Fallback Значения

```rust
let user_plan = db::get_user(&conn, chat_id.0)
    .ok()
    .flatten()
    .map(|u| u.plan)
    .unwrap_or_else(|| "free".to_string()); // Fallback!
```

Это гарантирует что метрика всегда запишется, даже если БД недоступна.

### 3. Группируйте Labels Логически

Метрика `format_requests_total` имеет 2 labels:
- `format` - что запросили (mp3/mp4/srt/txt)
- `plan` - кто запросил (free/premium/vip)

Это позволяет анализировать:
- "Какие форматы популярнее?" → `sum by (format)`
- "Как premium пользователи используют бота?" → `{plan="premium"}`
- "Сколько free пользователей качают MP4?" → `{format="mp4",plan="free"}`

## Итоговое Состояние

После исправления панель "Downloads by Format" работает корректно:

✅ Метрика инкрементируется при каждом запросе на загрузку
✅ Prometheus собирает данные каждые 10 секунд
✅ Grafana показывает rate в req/sec по каждому формату
✅ График обновляется автоматически каждые 30 секунд

## Связанные Файлы

- [src/core/metrics.rs](src/core/metrics.rs) - Определение метрик и helper функции
- [src/download/downloader.rs](src/download/downloader.rs) - Использование метрик в коде загрузки
- [grafana/dashboards/doradura_overview.json](grafana/dashboards/doradura_overview.json) - Grafana dashboard
- [METRICS_DASHBOARD_FIX.md](METRICS_DASHBOARD_FIX.md) - Основное исправление метрик
- [QUEUE_DEPTH_FIX.md](QUEUE_DEPTH_FIX.md) - Исправление Queue Depth
