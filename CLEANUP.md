# Doradura — Cleanup Plan

## Критические проблемы (HIGH)

### 1. Дубликат `/tests/` в корне
- Корневая `/tests/` дублирует `/crates/dorabot/tests/`
- Файлы с суффиксом `_test.rs` в корне vs без суффикса в crates
- **Действие:** Удалить `/tests/`, оставить только `/crates/dorabot/tests/`

### 2. Монолитный `db/mod.rs` — 4856 строк
- `crates/doracore/src/storage/db/mod.rs` — 104+ функций в одном файле
- Плейлисты, сессии, vault, downloads, subscriptions, cookies — всё в одном месте
- **Действие:** Разбить на модули: `db/playlists.rs`, `db/vault.rs`, `db/users.rs`, `db/downloads.rs`, `db/subscriptions.rs`

### 3. Дубликат `builder.rs` — 100% идентичен
- `crates/doracore/src/download/builder.rs` (214 строк)
- `crates/dorabot/src/download/builder.rs` (214 строк)
- **Действие:** Удалить bot-версию, реэкспортить из core

### 4. Дубликат `/benches/` в корне
- `benches/queue_benchmark.rs` идентичен `crates/dorabot/benches/queue_benchmark.rs`
- **Действие:** Удалить корневой `/benches/`

---

## Средние проблемы (MEDIUM)

### 5. Near-duplicate `source/` модули между core и bot
| Файл | core (строк) | bot (строк) | Разница |
|------|-------------|------------|---------|
| `source/mod.rs` | 13 | 13 | Идентичны |
| `source/instagram.rs` | 1599 | 1601 | ~99% overlap, разный rate limiter |
| `source/ytdlp.rs` | 960 | 981 | Near-duplicate |
| `source/http.rs` | ~12k | ~12k | Идентичны |

- **Действие:** Консолидировать идентичные (`http.rs`, `mod.rs`). Задокументировать причину расхождений в `instagram.rs` и `ytdlp.rs`

### 6. Монолитные telegram модули
| Файл | Строк |
|------|-------|
| `telegram/commands.rs` | 3625 |
| `telegram/admin.rs` | 3595 |
| `telegram/downloads.rs` | 2723 |
| `telegram/menu/callback_router.rs` | 1679 |

- **Действие:** Разбить на субмодули по функциональности

### 7. Путаница с `vlipsy.rs`
- `dorabot/src/vlipsy.rs` — API клиент
- `dorabot/src/download/source/vlipsy.rs` — download source
- **Действие:** Переименовать клиент в `vlipsy_client.rs` или переместить

---

## Мелкие проблемы (LOW)

### 8. Артефакты
- `.DS_Store` файлы (добавить в `.gitignore` если нет)
- `locales/fr/main.ftl.bak` — бэкап файл, удалить
- Единственный TODO: `download/audio.rs` — `// TODO: Re-enable premium check after testing`

### 9. Extension система — misleading naming
- `crates/doracore/src/extension/` — звучит как "плагины", но это UI-метаданные для built-in фич
- **Действие:** Добавить doc-комментарий или переименовать в `capability/`

### 10. Naming conventions
- Тесты: `*_test.rs` (root) vs `.rs` (crates) — стандартизовать на Rust convention (без суффикса)

---

## Порядок выполнения

| # | Задача | Сложность | Риск |
|---|--------|-----------|------|
| 1 | Удалить корневые `/tests/` и `/benches/` | Низкая | Низкий |
| 2 | Удалить `locales/fr/main.ftl.bak` | Тривиальная | Нулевой |
| 3 | Удалить дубликат `builder.rs` из bot, реэкспортить из core | Низкая | Низкий |
| 4 | Консолидировать `source/http.rs` и `source/mod.rs` | Средняя | Средний |
| 5 | Разбить `db/mod.rs` на модули | Высокая | Средний |
| 6 | Разбить `telegram/commands.rs` на субмодули | Высокая | Средний |
| 7 | Разбить `telegram/admin.rs` на субмодули | Высокая | Средний |
| 8 | Задокументировать divergence в instagram/ytdlp sources | Низкая | Нулевой |

---

## Позитивные находки

- Отличное разделение core/bot — doracore реально Telegram-агностичен
- Чистый dependency management с workspace
- Всего 1 TODO во всём проекте
- Хорошее покрытие тестами (unit, integration, smoke, mocks)
- Trait-based архитектура download sources — грамотная
