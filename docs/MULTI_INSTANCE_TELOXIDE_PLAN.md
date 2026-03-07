# Multi-Instance Plan for doradura + teloxide

> Конкретный план перевода `doradura` на несколько одинаковых инстансов одного приложения без split на микросервисы.

---

## 1. Целевая схема

```text
Telegram Bot API
   |
   | HTTPS webhook
   v
Load Balancer / Reverse Proxy
   |
   +---- doradura instance A
   |
   +---- doradura instance B
   |
   +---- doradura instance C
          |
          +-- PostgreSQL  <- source of truth for updates/tasks/state
          +-- Redis       <- cooldowns, metadata/file cache
```

### Главные принципы

- один публичный webhook URL на бота
- несколько одинаковых инстансов приложения за LB
- никакого `getUpdates`/long polling в production multi-instance режиме
- `PostgreSQL` хранит task lifecycle и update dedup
- `Redis` хранит только cooldown/cache/короткоживущую координацию
- каждый инстанс сам обрабатывает свои temp файлы и сам отправляет результат в Telegram

---

## 2. Что сейчас мешает multi-instance режиму

### 2.1 Webhook lifecycle управляется из runtime-инстанса

Сейчас webhook mode в [startup.rs](/Users/stan/Dev/_PROJ/doradura/crates/dorabot/src/startup.rs#L215) делает:

- `delete_webhook()`
- `set_webhook(...)`
- `delete_webhook()` при shutdown

Для нескольких инстансов это неправильно:

- каждый новый инстанс будет дергать lifecycle webhook
- graceful shutdown одного инстанса может удалить webhook для всех
- webhook setup должен быть централизованным deploy-time действием

### 2.2 В production путь все еще ориентирован на polling/single-instance

Сейчас:

- webhook server фактически не реализован: [startup.rs](/Users/stan/Dev/_PROJ/doradura/crates/dorabot/src/startup.rs#L223)
- polling реализован как основной рабочий режим: [startup.rs](/Users/stan/Dev/_PROJ/doradura/crates/dorabot/src/startup.rs#L236)

Для multi-instance production это нужно инвертировать:

- webhook mode должен стать нормальным production path
- polling должен остаться только для local/dev/single-instance fallback

### 2.3 Task queue еще не готова к distributed claiming

Сейчас `task_queue` умеет:

- `pending`
- `processing`
- `failed`
- `completed`

Но у нее нет:

- `leased_by`
- `lease_expires_at`
- `attempt_count` как отдельной lease-aware семантики
- `dead_letter`
- `not_before`

Текущие функции в [mod.rs](/Users/stan/Dev/_PROJ/doradura/crates/doracore/src/storage/db/mod.rs#L2186) и [mod.rs](/Users/stan/Dev/_PROJ/doradura/crates/doracore/src/storage/db/mod.rs#L2330) завязаны на single-instance recovery, а не на concurrent multi-instance claim.

### 2.4 Update dedup нет

Сейчас в коде нет durable dedup по `update_id`.

В multi-instance режиме это обязательно, иначе:

- LB может развести повторную доставку по разным инстансам
- Telegram может повторно отправить webhook update
- один и тот же command path может enqueue-нуть задачу дважды

---

## 3. Как должен выглядеть production bootstrap

## 3.1 Webhook setup выносится из приложения

Нужно разделить:

- `runtime app`
- `webhook management`

### Правильная схема

1. Deploy script или отдельная CLI-команда делает:
   - `setWebhook`
   - `secret_token`
   - `max_connections`

2. Runtime-инстансы:
   - не вызывают `delete_webhook`
   - не вызывают `set_webhook`
   - только принимают HTTP requests на фиксированном пути, например `/telegram/webhook`

### Что нужно сделать в коде

- убрать automatic webhook lifecycle из [startup.rs](/Users/stan/Dev/_PROJ/doradura/crates/dorabot/src/startup.rs#L215)
- добавить отдельную CLI-команду вроде:
  - `doradura webhook set`
  - `doradura webhook delete`
  - `doradura webhook info`

## 3.2 В runtime нужен настоящий HTTP webhook endpoint

Production path должен быть таким:

```text
HTTP POST /telegram/webhook
-> verify Telegram secret token
-> deserialize Update
-> try register update_id in PostgreSQL
-> if duplicate: return 200 OK
-> pass update into teloxide dispatcher/handler pipeline
-> return 200 quickly
```

Для `teloxide` это означает:

- использовать webhook mode
- не использовать helper, который сам делает setup/teardown webhook
- ориентироваться на low-level интеграцию, а не на auto-managed polling lifecycle

Дополнительно:

- не передавать bot token в URL path
- использовать `X-Telegram-Bot-Api-Secret-Token`
- не считать Redis достаточной защитой от duplicate updates

---

## 4. Минимальная схема БД для multi-instance

## 4.1 Таблица `processed_updates`

```sql
CREATE TABLE processed_updates (
    update_id      BIGINT PRIMARY KEY,
    received_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    processed_by   TEXT,
    update_kind    TEXT
);

CREATE INDEX idx_processed_updates_received_at
ON processed_updates(received_at);
```

### Семантика

- перед обработкой update делаем `INSERT`
- если `ON CONFLICT DO NOTHING` и строка не вставилась, update уже был seen
- в таком случае сразу возвращаем `200 OK`

`processed_updates` должен быть **durable source of truth** для update dedup.

Redis можно использовать поверх этого только как ускоритель, но не как единственную защиту.

### Cleanup

- хранить `processed_updates` не вечно
- cleanup policy: `7-14 days`

## 4.2 Расширение `task_queue`

Нужны поля:

```sql
ALTER TABLE task_queue ADD COLUMN leased_by TEXT;
ALTER TABLE task_queue ADD COLUMN lease_expires_at TIMESTAMPTZ;
ALTER TABLE task_queue ADD COLUMN not_before TIMESTAMPTZ;
ALTER TABLE task_queue ADD COLUMN attempt_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE task_queue ADD COLUMN started_at TIMESTAMPTZ;
ALTER TABLE task_queue ADD COLUMN finished_at TIMESTAMPTZ;
ALTER TABLE task_queue ADD COLUMN idempotency_key TEXT;
```

Желательно также:

- нормализовать `status`
- ввести terminal state `dead_letter`

### Рекомендуемые статусы

- `pending`
- `leased`
- `processing`
- `uploading`
- `completed`
- `retryable_failed`
- `dead_letter`

## 4.3 Индексы под claim/reaper path

```sql
CREATE INDEX idx_task_queue_claim
ON task_queue(status, priority DESC, created_at ASC);

CREATE INDEX idx_task_queue_lease_expiry
ON task_queue(status, lease_expires_at);

CREATE INDEX idx_task_queue_not_before
ON task_queue(status, not_before);
```

---

## 5. Как должен работать distributed task claiming

## 5.1 Claim flow

В транзакции:

```sql
SELECT id
FROM task_queue
WHERE status = 'pending'
  AND (not_before IS NULL OR not_before <= NOW())
ORDER BY priority DESC, created_at ASC
FOR UPDATE SKIP LOCKED
LIMIT 1;
```

Затем:

```sql
UPDATE task_queue
SET status = 'leased',
    leased_by = $instance_id,
    lease_expires_at = NOW() + INTERVAL '60 seconds',
    updated_at = NOW(),
    started_at = COALESCE(started_at, NOW())
WHERE id = $task_id;
```

## 5.2 Lease renewal

Пока задача выполняется, инстанс обновляет:

```sql
UPDATE task_queue
SET lease_expires_at = NOW() + INTERVAL '60 seconds',
    updated_at = NOW()
WHERE id = $task_id
  AND leased_by = $instance_id;
```

Heartbeat раз в `15-20s`.

## 5.3 Reaper

Фоновая задача:

```sql
UPDATE task_queue
SET status = 'pending',
    leased_by = NULL,
    lease_expires_at = NULL,
    updated_at = NOW()
WHERE status IN ('leased', 'processing', 'uploading')
  AND lease_expires_at < NOW();
```

Важно:

- reaper не должен бесконечно гонять явно безнадежные задачи
- после `attempt_count >= max_attempts` задача идет в `dead_letter`

---

## 6. Idempotency model

## 6.1 Update idempotency

Ключ:

- `update_id`

Назначение:

- не обрабатывать один update повторно

## 6.2 Job idempotency

Ключ:

- `idempotency_key`

Можно строить из:

- `chat_id`
- `normalized_url`
- `requested_format`
- `requested_quality`
- `request window`

Назначение:

- если один и тот же update/command по ошибке enqueue-нулся дважды, не выполнять дорогую работу дважды

## 6.3 Finalization idempotency

Даже после успешной отправки файла возможен race:

- файл уже ушел в Telegram
- инстанс упал до `mark completed`

Нужна финализация в стиле:

- проверка `task.status`
- запись sent/history через idempotent semantics
- повторная финализация не должна дублировать историю и side effects

---

## 7. Redis в этой схеме

Redis нужен только для:

1. `rate:{user_id}` cooldown keys
2. `cache:meta:{normalized_url_hash}`
3. `cache:fileid:{content_hash}:{format}:{quality}:{bot_api_mode}`
4. коротких distributed locks для singleton jobs

Не использовать Redis для:

- основной очереди
- durable task lifecycle
- единственной правды о статусе задачи
- единственной правды о `update_id` dedup

---

## 8. Singleton jobs в multi-instance

Некоторые background jobs нельзя запускать на всех инстансах одновременно:

- cleanup `processed_updates`
- cleanup expired tasks / reaper
- тяжелые periodic maintenance jobs

Для этого нужен singleton lock:

- либо Redis `SET key value NX EX`
- либо PostgreSQL advisory lock

Рекомендуемо:

- `PG advisory lock` для DB-related maintenance
- `Redis lock` для коротких cross-instance jobs

---

## 9. Как это привязать к текущему репозиторию

## 9.1 Что надо поменять в первую очередь

### A. Startup / mode split

Файлы:

- [startup.rs](/Users/stan/Dev/_PROJ/doradura/crates/dorabot/src/startup.rs)
- [cli.rs](/Users/stan/Dev/_PROJ/doradura/crates/dorabot/src/cli.rs)

Что сделать:

- убрать auto `set_webhook` / `delete_webhook` из runtime path
- добавить отдельные CLI-команды управления webhook
- сделать webhook runtime path полноценным HTTP server path
- использовать fixed webhook path + secret header model

### B. Update dedup storage

Файлы:

- [mod.rs](/Users/stan/Dev/_PROJ/doradura/crates/doracore/src/storage/db/mod.rs)
- `migrations/` новая миграция

Что сделать:

- новая таблица `processed_updates`
- DB API:
  - `register_update_if_new(update_id, processed_by, update_kind) -> bool`
  - `cleanup_processed_updates(days)`

### C. Distributed queue lifecycle

Файлы:

- [mod.rs](/Users/stan/Dev/_PROJ/doradura/crates/doracore/src/storage/db/mod.rs)
- [queue_processor.rs](/Users/stan/Dev/_PROJ/doradura/crates/dorabot/src/queue_processor.rs)
- [queue.rs](/Users/stan/Dev/_PROJ/doradura/crates/dorabot/src/download/queue.rs)

Что сделать:

- расширить `task_queue` lease-полями
- перестроить queue processor на DB claim/release semantics
- свести in-memory queue к optional fast-path/cache или убрать из canonical flow

### D. Multi-instance-safe background tasks

Файлы:

- [background_tasks.rs](/Users/stan/Dev/_PROJ/doradura/crates/dorabot/src/background_tasks.rs)

Что сделать:

- выделить singleton jobs
- добавить advisory lock / redis lock

---

## 10. Порядок внедрения

### Step 1

- webhook management вынести из runtime
- polling оставить только для dev

### Step 2

- добавить `processed_updates`
- встроить update dedup в intake path

### Step 3

- ввести lease-поля в `task_queue`
- написать claim/reaper API в DB layer

### Step 4

- переписать queue processor на distributed claim

### Step 5

- добавить singleton-lock для maintenance jobs

### Step 6

- включить несколько инстансов за LB

---

## 11. Что я могу сделать дальше в коде

Я могу сделать это поэтапно.

### Вариант 1: подготовить infra-safe bootstrap

- добавить CLI для `webhook set/info/delete`
- убрать dangerous webhook lifecycle из runtime
- подготовить runtime под настоящий webhook endpoint

### Вариант 2: сделать базу для multi-instance safety

- миграция `processed_updates`
- DB API для dedup
- миграция lease-полей в `task_queue`

### Вариант 3: начать distributed queue

- новый DB claim API
- reaper API
- адаптация `queue_processor`

### Самый правильный следующий шаг

Сначала делать **Вариант 2**, потом **Вариант 1**, потом **Вариант 3**.

Причина:

- без durable dedup и leases несколько инстансов опасны
- webhook runtime без shared state не даст правильного поведения
- queue processor имеет смысл менять только после появления правильной DB-схемы
