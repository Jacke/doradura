# Doradura Scalability & Multi-Instance Architecture

Этот документ описывает стратегию и техническую реализацию масштабирования проекта от **Single Instance (SQLite)** до **Multi-Instance Cluster (PostgreSQL + Redis)**.

**Философия:** "Scale-out Monolith". Мы не разбиваем приложение на микросервисы. Мы запускаем N одинаковых экземпляров приложения, которые координируются через общую Базу Данных и Redis.

---

## 1. Архитектурные уровни

### Уровень 1: Single Instance (Dev / Low Load)
*Идеально для: Разработки, Тестирования, < 200 DAU.*

*   **Вход:** Telegram Long Polling.
*   **Очередь:** SQLite (WAL mode). Source of truth — **всегда БД**, не память.
*   **Стейт:** SQLite.
*   **Файлы:** Локальный диск (`/tmp`).

> **Optional optimization:** In-memory polling cache может использоваться чтобы не дёргать SQLite каждые 100ms. Но это **чистый кэш** — при рестарте он пуст, задачи восстанавливаются из БД. Canonical state = БД, всегда.

### Уровень 2: Multi-Instance (Production / High Load)
*Идеально для: Масштабирования, отказоустойчивости, > 200 DAU.*

*   **Вход:** Telegram Webhooks -> Load Balancer (Nginx/Railway) -> N x Doradura Instances.
*   **Очередь:** PostgreSQL (Skip Locked).
*   **Стейт:** PostgreSQL (метаданные) + Redis (кэш, лимиты, локи).
*   **Файлы:** Локальный "scratch" диск на каждом инстансе + Reuse `file_id` через БД.

---

## 2. Техническая реализация (Roadmap)

Для поддержки обоих уровней без лишнего дублирования кода нужен аккуратный слой доступа к данным, но не обязательно полный dual-backend abstraction на весь проект.

### 2.1. Абстракция хранилища без over-engineering

Главная цель до `1000 DAU` не в том, чтобы идеально абстрагировать любой storage backend, а в том, чтобы:

- перевести hot path очереди и distributed coordination на `PostgreSQL`
- минимизировать риск расхождения логики между SQLite и PostgreSQL
- не раздувать объем миграции ради архитектурной чистоты

Поэтому рекомендуемый подход:

- не строить общий `Storage` trait на весь проект в первый заход
- сначала вынести только `task queue`, `lease/retry/dead-letter`, `processed_updates`
- остальную часть storage мигрировать постепенно, если это реально нужно

Иными словами: **queue/hot path first**, а не "весь storage abstraction first".

Если abstraction все же вводится, она должна покрывать только узкий operational scope:

- `TaskQueueRepository`
- `ProcessedUpdatesRepository`
- `DistributedRateLimitRepository`

```rust
#[async_trait]
pub trait TaskQueueRepository: Send + Sync {
    /// Попытка захватить следующую задачу для выполнения
    async fn claim_next_task(&self, worker_id: &str) -> Result<Option<Task>>;
    
    /// Возврат задачи в очередь (например, при graceful shutdown)
    async fn release_task(&self, task_id: &str) -> Result<()>;
    
    /// Пометить задачу как выполненную
    async fn complete_task(&self, task_id: &str, result: TaskResult) -> Result<()>;
    
    /// Очистка "зависших" задач от упавших воркеров
    async fn recover_expired_leases(&self, now: DateTime<Utc>) -> Result<usize>;
}
```

### 2.2. Реализация Очереди (Queue Implementation)

Очередь должна переехать из `VecDeque` (память) в Базу Данных.

#### Логика для PostgreSQL (Multi-Instance)
Используем `FOR UPDATE SKIP LOCKED`. Это позволяет нескольким инстансам атомарно забирать разные задачи без блокировки всей таблицы.

Важно: одной пары `worker_id + leased_at` недостаточно. Для multi-instance нужна настоящая lease-модель:

- `lease_expires_at`
- heartbeat / lease renewal
- reclaim flow для зависших задач
- terminal state для безнадежных задач

```sql
WITH next_task AS (
    SELECT id
    FROM task_queue
    WHERE status = 'pending'
      AND (execute_at IS NULL OR execute_at <= NOW())
    ORDER BY priority DESC, created_at ASC
    LIMIT 1
    FOR UPDATE SKIP LOCKED
)
UPDATE task_queue
SET 
    status = 'leased',
    worker_id = $1,            -- ID текущего инстанса (hostname-pid)
    leased_at = NOW(),
    lease_expires_at = NOW() + INTERVAL '60 seconds',
    updated_at = NOW()
FROM next_task
WHERE task_queue.id = next_task.id
RETURNING task_queue.*;
```

После claim инстанс обязан:

- перевести задачу в `processing`
- обновлять `lease_expires_at` heartbeat-ом каждые `15-20s`
- на shutdown/retry/failure либо release-ить задачу, либо переводить ее в retryable/error state

Рекомендуемые статусы:

- `pending`
- `leased`
- `processing`
- `uploading`
- `completed`
- `retryable_failed`
- `dead_letter`

#### Логика для SQLite (Single Instance)
SQLite не поддерживает `SKIP LOCKED`, но для одного инстанса достаточно транзакции `IMMEDIATE`.

```sql
-- В транзакции BEGIN IMMEDIATE
UPDATE task_queue
SET status = 'processing', worker_id = $1, leased_at = CURRENT_TIMESTAMP
WHERE id = (
    SELECT id FROM task_queue
    WHERE status = 'pending'
    ORDER BY priority DESC, created_at ASC
    LIMIT 1
)
RETURNING *;
```

### 2.3. Входящий трафик: Webhooks & Axum

Long Polling не работает адекватно при нескольких инстансах (Telegram отдает апдейт только одному соединению).

**Задача:**
1.  Внедрить HTTP-сервер **Axum** в `startup.rs`.
2.  Настроить фиксированный маршрут `POST /telegram/webhook`.
3.  Реализовать **Deduplication Middleware**:
    *   Telegram гарантирует `at-least-once` доставку. Дубли возможны.
    *   При получении Update, регистрируем в **PostgreSQL** как durable source of truth.
    *   Если `ON CONFLICT DO NOTHING` не вставил запись -> вернуть 200 OK (игнор).
    *   Если запись вставлена -> обработать Update.

**Схема `processed_updates`:**
```sql
CREATE TABLE processed_updates (
    bot_id    BIGINT NOT NULL,    -- чтобы не привязываться к одному боту навсегда
    update_id BIGINT NOT NULL,
    created_at TIMESTAMP NOT NULL DEFAULT NOW(),
    PRIMARY KEY (bot_id, update_id)
);
```
**Retention:** 48 часов. Telegram не ретраит webhook дольше суток, 48h — с запасом.
```sql
-- cleanup job раз в час
DELETE FROM processed_updates WHERE created_at < NOW() - INTERVAL '48 hours';
```

**Почему не Redis-only dedup:**

- Redis подходит как ускоритель, но недостаточно надежен как единственная защита от повторной обработки дорогих side effects
- после рестарта/eviction duplicate update может снова пройти
- для enqueue/download/send path нужен durable dedup

Webhook безопасность:

- не передавать bot token в path
- использовать `X-Telegram-Bot-Api-Secret-Token`
- логировать только request metadata, а не секреты

### 2.4. Распределенный Rate Limiter (Redis)

Лимиты "5 видео в час" должны работать глобально, а не "5 видео на каждый инстанс".

**Реализация:**
*   Использовать Redis Atomic Counters.
*   Ключ: `ratelimit:user:{user_id}:downloads`.
*   TTL: 3600 сек.
*   Логика: `INCR` -> если > Limit -> Reject.

Redis здесь уместен, потому что cooldown/rate limiting:

- не является единственным durable source of truth для task processing
- естественно моделируется через TTL keys
- хорошо работает в multi-instance среде

---

## 3. Task Lifecycle & Lease Semantics

### 3.1. State Machine

```
pending ──► leased ──► processing ──► uploading ──► completed
              │            │              │
              ▼            ▼              ▼
         retryable    retryable      retryable
              │            │              │
              ▼            ▼              ▼
     pending (attempt_count++)  ──────────┘
                    │
                    ▼ (attempt_count >= max_attempts)
               dead_letter
```

### 3.2. Поля задачи

| Поле | Тип | Описание |
|------|-----|----------|
| `id` | UUID | Уникальный идентификатор |
| `idempotency_key` | TEXT UNIQUE | Дедуп: `{user_id}:{normalized_url}:{format}:{quality}` |
| `priority` | INT | Приоритет (тариф пользователя) |
| `status` | ENUM | `pending / leased / processing / uploading / completed / dead_letter` |
| `attempt_count` | INT | Текущая попытка (начинается с 0) |
| `max_attempts` | INT | Лимит попыток (default 3) |
| `worker_id` | TEXT NULL | Кто держит задачу (`{hostname}-{pid}` или UUID при старте) |
| `leased_at` | TIMESTAMP NULL | Когда задача была захвачена |
| `lease_expires_at` | TIMESTAMP NULL | Когда lease протухнет без heartbeat |
| `last_heartbeat_at` | TIMESTAMP NULL | Последний heartbeat от воркера |
| `last_error` | TEXT NULL | Последняя ошибка |
| `execute_at` | TIMESTAMP NULL | Не выполнять до (backoff) |
| `created_at` | TIMESTAMP | Время создания |
| `updated_at` | TIMESTAMP | Последнее обновление |
| `started_at` | TIMESTAMP NULL | Начало первой обработки |
| `finished_at` | TIMESTAMP NULL | Завершение (success или dead_letter) |

### 3.3. Lease & Heartbeat

| Параметр | Значение | Обоснование |
|----------|----------|-------------|
| `lease_duration` | 5 мин | Достаточно для старта; heartbeat продлевает |
| `heartbeat_interval` | 20 сек | Достаточно часто для обнаружения мертвых воркеров |
| `heartbeat_extends_by` | 5 мин | Каждый heartbeat сдвигает `lease_expires_at` на +5 мин от NOW() |
| `zombie_check_interval` | 60 сек | Reaper проверяет протухшие leases раз в минуту |

**Реализация heartbeat** — один sweep task на инстанс (не per-job), обновляет все свои leases одним запросом:

```sql
-- PostgreSQL
UPDATE task_queue
SET lease_expires_at = NOW() + INTERVAL '5 minutes',
    last_heartbeat_at = NOW(),
    updated_at = NOW()
WHERE worker_id = $1
  AND status IN ('leased', 'processing', 'uploading');
```

```sql
-- SQLite
UPDATE task_queue
SET lease_expires_at = datetime('now', '+5 minutes'),
    last_heartbeat_at = datetime('now'),
    updated_at = datetime('now')
WHERE worker_id = $1
  AND status IN ('leased', 'processing', 'uploading');
```

**Zombie Reaper** — запускается на каждом инстансе, безопасен для параллельного выполнения:

```sql
-- PostgreSQL
UPDATE task_queue
SET status = CASE
        WHEN attempt_count >= max_attempts THEN 'dead_letter'
        ELSE 'pending'
    END,
    worker_id = NULL,
    leased_at = NULL,
    lease_expires_at = NULL,
    execute_at = CASE
        WHEN attempt_count < max_attempts
        THEN NOW() + (POWER(2, attempt_count) * INTERVAL '1 second')
        ELSE NULL
    END,
    attempt_count = attempt_count + 1,
    updated_at = NOW()
WHERE status IN ('leased', 'processing', 'uploading')
  AND lease_expires_at < NOW();
```

```sql
-- SQLite
UPDATE task_queue
SET status = CASE
        WHEN attempt_count >= max_attempts THEN 'dead_letter'
        ELSE 'pending'
    END,
    worker_id = NULL,
    leased_at = NULL,
    lease_expires_at = NULL,
    execute_at = CASE
        WHEN attempt_count < max_attempts
        THEN datetime('now', '+' || (1 << attempt_count) || ' seconds')
        ELSE NULL
    END,
    attempt_count = attempt_count + 1,
    updated_at = datetime('now')
WHERE status IN ('leased', 'processing', 'uploading')
  AND lease_expires_at < datetime('now');
```

### 3.4. Retry Matrix

| Класс ошибки | Retry? | Backoff | Примечание |
|---------------|--------|---------|------------|
| Transient network (timeout, DNS) | Да | Exponential + jitter | Стандартный retry |
| Telegram 429 | Да | `retry_after` от Telegram | Уважать серверный backoff |
| yt-dlp extractor temp failure | Да | Exponential, max 2 retry | Платформа временно нестабильна |
| yt-dlp 403 / geo-block | Нет | — | Proxy не помог = terminal |
| Invalid URL / unsupported | Нет | — | Ошибка пользователя |
| File too large (>2GB) | Нет | — | Telegram лимит, retry бесполезен |
| Disk full | Нет | — | Admission control должен был не допустить |
| ffmpeg crash / OOM | Да | 1 retry с пониженным quality | Может помочь при нехватке RAM |
| Upload partial fail | Да | Immediate retry | Файл ещё локально, переотправить |

### 3.5. Idempotency

Дубли предотвращаются на двух уровнях:

1. **Enqueue:** `idempotency_key` (UNIQUE constraint) — повторная отправка того же URL с теми же параметрами не создаёт новую задачу.
2. **Finalization:** перед записью результата проверяем `status != 'completed'` — если другой воркер уже завершил (race после lease expiry), не перезаписываем.

---

## 4. Concurrency Budgets

Один `MAX_CONCURRENT_DOWNLOADS` не масштабируется — metadata fetch и ffmpeg конкурируют за один budget.

### 4.1. Семафоры по фазам

```rust
struct ConcurrencyBudgets {
    metadata: Semaphore,  // 4  — лёгкие HTTP запросы
    download: Semaphore,  // 3  — yt-dlp, тяжёлый IO + CPU
    ffmpeg: Semaphore,    // 2  — CPU + RAM intensive
    upload: Semaphore,    // 3  — Telegram API, network bound
}
```

Значения — defaults, настраиваются через env:
```ini
BUDGET_METADATA=4
BUDGET_DOWNLOAD=3
BUDGET_FFMPEG=2
BUDGET_UPLOAD=3
```

### 4.2. Sync fast-path тоже под budget

Metadata lookup и file_id reuse — sync операции, которые обходят queue. Но они **не бесплатны** under load:

- **Metadata lookup** (info-only, inline queries) → через `metadata` semaphore. Без этого шумные inline queries могут забить сеть и замедлить metadata фазу queued задач.
- **file_id reuse send** → через `upload` semaphore. Даже без локального файла это Telegram API call, который делит тот же rate limit budget с обычными upload'ами. Telegram не различает "send by file_id" и "send by upload" в своих rate limits — оба потребляют один и тот же API quota.

Правило: **любая внешняя операция идёт через соответствующий semaphore**, даже если она не проходит через queue.

### 4.3. Практический эффект

- Тяжёлые ffmpeg jobs не блокируют metadata/info запросы
- Telegram upload spikes не мешают старту новых задач
- Можно тюнить каждый bottleneck изолированно
- Budgets per-instance — общий throughput растёт линейно с числом инстансов
- Sync fast-paths не создают неконтролируемую нагрузку under pressure

---

## 5. Admission Control

При перегрузке система должна **осознанно отказывать**, а не молча деградировать.

### 5.1. Disk Pressure

| Уровень | Порог | Действие |
|---------|-------|----------|
| Normal | > 30% free | Работаем штатно |
| Warning | 15-30% free | Логируем warning, метрика `disk_pressure=warning` |
| Soft limit | 10-15% free | Отклоняем новые video tasks (heavy disk), audio ещё принимаем |
| Hard limit | < 10% free | Отклоняем все новые задачи, сообщаем пользователю "сервис перегружен" |

### 5.2. Queue Depth

| Условие | Действие |
|---------|----------|
| `pending_count < 50` | Нормальный режим |
| `pending_count 50-100` | Warning, метрика |
| `pending_count > 100` | Отклоняем задачи от free-tier пользователей |
| `pending_count > 200` | Отклоняем все кроме premium |

### 5.3. RAM Pressure

Мониторить RSS child processes (`yt-dlp` + `ffmpeg`). При > 75% системной RAM — не запускать новые ffmpeg/download задачи до освобождения.

---

## 6. Multi-Instance: каждый инстанс делает всё

В multi-instance режиме **не разделяем роли** (Web / Worker / Reaper). Каждый инстанс:

- Принимает webhook updates
- Создаёт задачи в очереди
- Клеймит и выполняет задачи
- Запускает heartbeat sweep
- Запускает zombie reaper

**Singleton jobs** (reaper, maintenance) безопасны при параллельном запуске — SQL-запросы идемпотентны. При необходимости эксклюзивности — PG advisory lock.

**Advisory Lock Namespace:**

| Константа | Lock ID | Job |
|-----------|---------|-----|
| `LOCK_REAPER` | `1001` | Reclaim протухших leases |
| `LOCK_UPDATES_CLEANUP` | `1002` | DELETE старых `processed_updates` |
| `LOCK_DEAD_LETTER` | `1003` | Оповещения, метрики по dead_letter задачам |
| `LOCK_DISK_CLEANUP` | `1004` | Удаление orphaned temp файлов |

```rust
// src/constants.rs или аналог
pub const LOCK_REAPER: i64 = 1001;
pub const LOCK_UPDATES_CLEANUP: i64 = 1002;
pub const LOCK_DEAD_LETTER: i64 = 1003;
pub const LOCK_DISK_CLEANUP: i64 = 1004;
```

```sql
SELECT pg_try_advisory_lock(1001);  -- LOCK_REAPER
```

Для SQLite singleton jobs не нужны — один инстанс.

---

## 7. Работа с файлами в кластере

Мы отказываемся от сетевых файловых систем (NFS/S3) для обработки видео, так как `ffmpeg` требует быстрого IO.

**Стратегия "Local Scratch, Global Index":**
1.  **Download:** Инстанс А качает видео в свой локальный `/tmp`.
2.  **Process:** Инстанс А конвертирует видео локально.
3.  **Upload:** Инстанс А загружает файл в Telegram и получает `file_id`.
4.  **Index:** Инстанс А сохраняет `file_id` + хеш URL/настроек в общую БД (Postgres).
5.  **Clean:** Инстанс А удаляет локальный файл.

**Сценарий Reuse:**
1.  Пользователь 2 просит то же видео. Запрос попадает на Инстанс Б.
2.  Инстанс Б не находит локального файла.
3.  Инстанс Б проверяет БД -> находит `file_id`.
4.  Инстанс Б делает `sendDocument(file_id)` (мгновенно, без скачивания).

**Send Guard для fast-path:**

file_id reuse — это fast-path, который обходит queue. Но он тоже должен быть идемпотентным: если webhook ретрайнется после успешного `sendDocument(file_id)`, пользователь получит дубль.

Защита — единый порядок операций для **любого** входящего update (и queue path, и fast-path):

```
1. INSERT INTO processed_updates (bot_id, update_id) VALUES ($1, $2)
   ON CONFLICT DO NOTHING;

2. Если rows_affected = 0 → дубль. Вернуть 200 OK, никаких side effects.

3. Если rows_affected = 1 → первый раз. Только теперь:
   - fast-path: sendDocument(file_id) через upload semaphore
   - или enqueue в task_queue
```

Критично: **insert до side effect, не после**. Если сначала отправить файл, а потом записать `update_id` — при crash между ними дубль пройдёт повторно.

---

## 8. Аудит текущего кода: Legacy Paths

Перед миграцией необходимо понимать, какие места в коде **обходят** целевую архитектуру. Это не баги — это текущий single-instance дизайн, который нужно осознанно заменить.

### 8.1. In-Memory State как Source of Truth

| Что | Где | Риск | Замена |
|-----|-----|------|--------|
| `VecDeque<DownloadTask>` | `queue.rs:190` | Crash = потеря задач. `recover_from_db()` только при startup | DB polling queue |
| `HashSet<(String, i64, String)>` active_tasks | `queue.rs:191` | Дедуп active tasks в памяти, lost on restart | `idempotency_key` UNIQUE в БД |
| `HashMap<i64, (i32, Instant)>` notification_msgs | `queue.rs:192` | Cosmetic: "задача в очереди" message IDs | Допустимо оставить in-memory |
| `HashMap<ChatId, Instant>` rate limits | `rate_limiter.rs:13` | Рестарт = все лимиты сброшены. Multi-instance = каждый свои | DB-backed (single) или Redis (multi) |
| `CAROUSEL_MASKS: HashMap<String, u32>` | `instagram.rs:29` | Race: mask set → crash → mask lost → carousel filtering fails | Передавать mask через task payload, не через static |
| `FEEDBACK_STATES: HashMap<i64, bool>` | `feedback.rs:19` | User session state lost on restart | Допустимо: пользователь начнёт заново |
| `PreviewCache`, `LinkMessageCache`, `TimeRangeCache`, `BurnSubLangCache` | `cache.rs` | Чистые кэши, re-fetch при рестарте | Оставить as-is, это кэши |
| `TOKEN_CACHE: Option<(String, Instant)>` Spotify OAuth | `spotify.rs:16` | Re-auth при рестарте | Оставить as-is |
| `AlertManager` last_alert_time, active_alerts | `alerts.rs:301` | Burst алертов после рестарта | Оставить as-is, minor |

**Правило:** если потеря состояния = потеря пользовательской задачи или нарушение rate limit → мигрировать в БД. Если потеря = cosmetic re-fetch → оставить in-memory.

### 8.2. Update Dedup — отсутствует полностью

Zero occurrences of `update_id` в текущем коде. Telegram retry → дубли задач, дубли отправок. Единственная защита — `drop_pending_updates()` при startup (`startup.rs:275`), но это не runtime dedup.

### 8.3. Fire-and-Forget `tokio::spawn`

| Где | Что делает |
|-----|-----------|
| `audio.rs:49` | `tokio::spawn(async move { download + ffmpeg + send })` |
| `video.rs:54` | `tokio::spawn(async move { download + ffmpeg + send })` |

Queue processor отдаёт задачу в spawn и **теряет контроль**. Spawned task:
- Не обновляет lease / heartbeat
- Не контролируется при shutdown (может продолжить выполнение)
- Semaphore permit (`_permit`) привязан к spawn, а не к queue processor lifecycle

**Замена:** managed execution внутри queue processor loop. Task lifecycle (claim → heartbeat → complete/fail) управляется одним owner'ом.

### 8.4. Single Global Semaphore

`queue_processor.rs:36`: один `Semaphore::new(max_concurrent)` на все типы операций. Metadata fetch конкурирует с ffmpeg за один budget.

### 8.5. Нет Admission Control

Нет проверок disk pressure, queue depth, RAM pressure перед enqueue. Система принимает задачи до OOM/disk full.

---

## 9. План миграции (Пошаговый)

Каждый шаг — самодостаточный. Можно деплоить после каждого шага, не дожидаясь завершения всех.

### Step 1: Update Dedup (блокер всего остального)

**Без этого любое масштабирование умножает проблемы.**

- [ ] Создать таблицу `processed_updates` (миграция)
- [ ] Добавить dedup gate как **первую операцию** при обработке любого update:
  ```
  INSERT processed_updates → conflict? → 200 OK, stop
                           → inserted? → proceed to handler
  ```
- [ ] Gate покрывает **все** пути: команды, callback queries, inline queries, messages
- [ ] Cleanup job: DELETE записей старше 48h

**Что удалить:** ничего. Это новый слой поверх существующего кода.

### Step 2: DB Queue как Source of Truth

- [ ] Добавить lease-поля в `task_queue` (миграция): `lease_expires_at`, `last_heartbeat_at`, `attempt_count`, `max_attempts`, `execute_at`, `last_error`, `idempotency_key`
- [ ] `DownloadQueue::add_task()` → INSERT в БД с `idempotency_key` (UNIQUE). Убрать `active_tasks: HashSet` дедуп
- [ ] Queue processor poll loop: вместо `queue.pop()` из VecDeque → `claim_next_task()` из БД
- [ ] Убрать `recover_from_db()` из startup — больше не нужен, задачи всегда в БД
- [ ] VecDeque можно оставить как optional polling cache с явным комментарием: "not source of truth, optimization only"

**Что удалить:**
- `active_tasks: HashSet` — заменен `idempotency_key`
- `recover_from_db()` — больше не нужен

### Step 3: Managed Task Execution (убить fire-and-forget)

- [ ] Заменить `tokio::spawn(download_and_send_*)` на managed execution:
  ```rust
  // Queue processor loop
  let task = repo.claim_next_task(worker_id).await?;
  repo.update_status(task.id, "processing").await?;

  let result = execute_task(&task).await;  // НЕ spawn

  match result {
      Ok(_) => repo.complete_task(task.id).await?,
      Err(e) if e.is_retryable() => repo.fail_retryable(task.id, &e).await?,
      Err(e) => repo.fail_terminal(task.id, &e).await?,
  }
  ```
- [ ] Heartbeat sweep task: обновляет `lease_expires_at` для всех задач текущего worker'а каждые 20s
- [ ] Zombie reaper task: reclaim протухших leases каждые 60s
- [ ] Graceful shutdown: release все свои leased задачи обратно в pending

**Что удалить:**
- `tokio::spawn` в `audio.rs:49` и `video.rs:54` — заменить на inline/managed execution
- `last_download_start` global delay mutex — заменить на per-phase semaphore

### Step 4: Separate Concurrency Budgets

- [ ] Создать `ConcurrencyBudgets` struct с 4 семафорами
- [ ] Pipeline фазы acquire/release соответствующий семафор:
  - metadata fetch → `budgets.metadata`
  - yt-dlp download → `budgets.download`
  - ffmpeg processing → `budgets.ffmpeg`
  - Telegram send (включая file_id reuse) → `budgets.upload`
- [ ] Sync fast-paths (metadata lookup, file_id send) тоже через семафоры

**Что удалить:**
- Единый `Semaphore::new(max_concurrent)` в `queue_processor.rs:36`

### Step 5: Admission Control

- [ ] Disk pressure check перед enqueue (через `statvfs` или аналог)
- [ ] Queue depth check перед enqueue (`SELECT COUNT(*) WHERE status = 'pending'`)
- [ ] Пользовательское сообщение при отказе: "Сервис сейчас перегружен, попробуйте позже"

### Step 6: Rate Limiter → Persistent

- [ ] Single-instance: rate limits в SQLite (тот же DB)
- [ ] Multi-instance: rate limits в Redis (`INCR` + TTL)
- [ ] Текущий `HashMap<ChatId, Instant>` → fallback/local cache, canonical check в persistent store

### Step 7: Carousel Masks Fix

- [ ] Перенести carousel mask из `static HashMap` в поле task payload
- [ ] При enqueue Instagram carousel → mask сохраняется в `task_queue.metadata` (JSONB/TEXT)
- [ ] При execute → читать mask из task, не из static

---

## 10. Защита от Legacy Bypass (Invariants)

После миграции — набор инвариантов, которые **никогда не должны нарушаться**:

| Инвариант | Как проверяется |
|-----------|----------------|
| Ни один side effect без dedup check | Все update handlers начинаются с `INSERT processed_updates` |
| Ни одна задача вне БД | `VecDeque` (если оставлен) — кэш. Потеря = ок. Задача восстанавливается из БД |
| Ни один download/ffmpeg/send вне семафора | `ConcurrencyBudgets` inject через shared state, нет прямого вызова без acquire |
| Ни один `tokio::spawn` для download pipeline | Все download tasks — managed execution с lease lifecycle |
| Lease без heartbeat = reclaim | Reaper забирает протухшие leases безусловно |
| Complete после reclaim = noop | `UPDATE ... WHERE status = 'processing' AND worker_id = $me` — если reaper уже reclaimed, rows_affected = 0 |

**Compile-time enforcement (где возможно):**
- `ConcurrencyBudgets` не `Clone` — единственный экземпляр, нельзя обойти
- Download/send функции принимают `SemaphorePermit` как аргумент — без permit не вызвать
- `TaskQueueRepository` trait — единственный способ работы с очередью

---

## 11. Test Matrix

### 11.1. Race Condition Tests

```rust
#[tokio::test]
async fn test_concurrent_claim_no_double_assign() {
    // Insert 1 pending task
    // Spawn 10 concurrent claim_next_task()
    // Assert: exactly 1 returns Some, 9 return None
    // Assert: task.worker_id set, status = 'leased'
}

#[tokio::test]
async fn test_complete_after_reclaim_is_noop() {
    // Insert task, claim by worker_A
    // Expire lease (set lease_expires_at = past)
    // Run reaper → task back to pending, attempt_count = 1
    // Worker_A calls complete_task(task_id)
    // Assert: complete returns Ok but rows_affected = 0
    // Assert: task still pending (reaper's state wins)
}

#[tokio::test]
async fn test_reaper_concurrent_safe() {
    // Insert 3 expired tasks
    // Run reaper from 5 concurrent "instances"
    // Assert: each task reclaimed exactly once
    // Assert: attempt_count incremented by exactly 1 per task
}
```

### 11.2. Duplicate Delivery Tests

```rust
#[tokio::test]
async fn test_duplicate_update_id_ignored() {
    // INSERT processed_updates (bot_id=1, update_id=42)
    // Assert: rows_affected = 1
    // INSERT same (bot_id=1, update_id=42) ON CONFLICT DO NOTHING
    // Assert: rows_affected = 0
    // Assert: side effect handler NOT called second time
}

#[tokio::test]
async fn test_idempotency_key_prevents_double_enqueue() {
    // Enqueue task with idempotency_key = "user:1:url:abc:audio:320"
    // Enqueue again with same key
    // Assert: only 1 task in DB
    // Assert: second enqueue returns existing task (not error)
}

#[tokio::test]
async fn test_dedup_covers_fast_path() {
    // Process update_id=42 → file_id reuse fast-path → sendDocument
    // Process update_id=42 again (webhook retry)
    // Assert: sendDocument called exactly once
}
```

### 11.3. Lease Lifecycle Tests

```rust
#[tokio::test]
async fn test_heartbeat_extends_lease() {
    // Claim task (lease_expires_at = now + 5min)
    // Wait 3 minutes (simulated)
    // Heartbeat sweep
    // Assert: lease_expires_at = now + 5min (refreshed, not original)
}

#[tokio::test]
async fn test_zombie_reaper_respects_max_attempts() {
    // Insert task with attempt_count = 2, max_attempts = 3
    // Expire lease, run reaper
    // Assert: status = 'pending', attempt_count = 3
    // Expire lease again, run reaper
    // Assert: status = 'dead_letter', attempt_count = 4 (or 3 + terminal)
}

#[tokio::test]
async fn test_graceful_shutdown_releases_tasks() {
    // Claim 3 tasks by worker_A
    // Trigger graceful shutdown for worker_A
    // Assert: all 3 tasks back to pending, worker_id = NULL
}
```

### 11.4. Concurrency Budget Tests

```rust
#[tokio::test]
async fn test_semaphore_budgets_independent() {
    // Fill ffmpeg semaphore to capacity (2/2)
    // Assert: metadata claim still succeeds (independent budget)
    // Assert: upload still succeeds
    // Assert: ffmpeg blocked until permit released
}

#[tokio::test]
async fn test_fast_path_uses_upload_semaphore() {
    // Fill upload semaphore to capacity
    // Attempt file_id reuse send (fast-path)
    // Assert: blocked until upload permit available
    // Assert: same semaphore, not a bypass
}
```

### 11.5. Backend Parity Tests (SQLite / PostgreSQL)

Все тесты из 11.1-11.4 должны проходить на **обоих** бэкендах. Реализация:

```rust
// Macro или test fixture, параметризованный по backend
#[test_case(Backend::Sqlite ; "sqlite")]
#[test_case(Backend::Postgres ; "postgres")]
async fn test_claim_lifecycle(backend: Backend) {
    let repo = create_test_repo(backend).await;
    // ... тест логики, одинаковый для обоих бэкендов
}
```

Это главная причина держать `TaskQueueRepository` trait — **один набор тестов, два бэкенда**. Если тест проходит на SQLite но fails на PG (или наоборот) — баг в имплементации, не в тесте.

---

## 12. Конфигурация (Environment Variables)

Для управления режимом работы используются переменные окружения:

```ini
# Режим базы данных
# На production multi-instance path canonical backend = postgres
DATABASE_DRIVER=sqlite  # dev/local only
DATABASE_URL=postgres://user:pass@host:5432/db

# Режим бота
BOT_MODE=webhook        # или 'polling'
WEBHOOK_URL=https://doradura.fly.dev/telegram/webhook
PORT=8080

# Redis (опционально для Single, обязательно для Multi)
REDIS_URL=redis://localhost:6379

# Идентификация воркера (для логов и claims)
WORKER_ID=hostname-uuid
```

---

## 13. Метрики и мониторинг

В Multi-Instance среде важно различать метрики.
*   Добавлять лейбл `instance_id` только к infrastructure/worker-level метрикам, а не ко всем бизнесовым метрикам подряд.
*   **Критические метрики:**
    *   `queue_claim_latency`: Сколько времени воркер ждет задачу.
    *   `queue_depth`: Количество pending задач.
    *   `p95_queue_wait`: Время от создания задачи до начала обработки.
    *   `p95_total_job_latency`: End-to-end время от запроса до отправки.
    *   `lease_expiry_count`: Сколько leases протухло (индикатор мертвых воркеров).
    *   `dead_letter_count`: Задачи, исчерпавшие все попытки.
    *   `disk_free_pct`: Свободное место на диске.
    *   `child_process_rss`: RSS дочерних процессов (yt-dlp + ffmpeg).
    *   `telegram_429_count`: Rate limit от Telegram API.
    *   `db_pool_active_pct`: Утилизация пула соединений к БД.
    *   `redis_latency`: Задержки кэша.

---

## 14. SLO Triggers (когда переходить на следующий уровень)

Решение о переходе принимается по **метрикам**, а не по DAU.

| Сигнал | Триггер | Действие |
|--------|---------|----------|
| `p95_queue_wait < 60s`, ошибки низкие | — | Текущий уровень достаточен |
| `p95_queue_wait > 120s` стабильно 3 дня | Перейти на PG queue | Phase 2 |
| Много повторных URL, `yt-dlp` тратится впустую | Добавить Redis cache | Phase 2-3 |
| Один инстанс не держит нагрузку при нормальном CPU/RAM | Добавить инстансы | Phase 3 |
| `disk_free_pct < 15%` регулярно | Увеличить диск или admission control | Любая фаза |
| `child_process_rss > 75%` системной RAM длительно | Ужесточить concurrency budgets | Любая фаза |
| `telegram_429_count` растёт в 5m окне | Снизить upload concurrency | Любая фаза |

**DAU как ориентир (не как триггер):**
- `< 200 DAU` — обычно single instance + SQLite достаточно
- `200-600 DAU` — обычно нужен PG queue
- `600-1000 DAU` — обычно PG + Redis + multi-instance
- `> 1000 DAU` — обсуждать split только при доказанной необходимости

---

## 15. Чего НЕ делать до 1000 DAU

- Kafka / RabbitMQ — избыточно
- Kubernetes — не нужен для N одинаковых инстансов
- Split на отдельные gateway / worker сервисы — усложнение без выгоды
- Dual queue (PG + Redis) — Redis не должен быть второй очередью
- Shared filesystem (NFS/S3) для temp файлов — ffmpeg требует быстрый IO
- Оптимизировать по DAU, игнорируя queue wait, disk, 429, RAM
