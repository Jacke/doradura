# Интеграция Snapshot Testing с Реальной Логикой Бота

## ❓ Вопрос: Запускается ли реальная логика в тестах?

**Короткий ответ:** Нет, **по умолчанию НЕТ**. Но вы можете легко добавить!

## 🔍 Текущее состояние

### Что делают существующие тесты

```rust
#[tokio::test]
async fn test_start_command_from_snapshot() {
    let mock = TelegramMock::from_snapshot("start_command").await?;
    let bot = mock.create_bot()?;

    // ❌ Ваш код НЕ вызывается!
    // Проверяется только структура snapshot
    assert_eq!(mock.snapshot().name, "start_command");
}
```

**Что происходит:**
- ✅ Загружается JSON с записанными API вызовами
- ✅ Создаётся mock Telegram сервер (wiremock)
- ✅ Проверяется структура данных
- ❌ **НО:** ваш `handle_start_command` НЕ вызывается
- ❌ **НЕ проверяется** что ваш код делает правильные вызовы

### Это полезно для:

✅ **Валидации snapshots** - проверить что JSON корректный
✅ **Документации API** - увидеть какие вызовы делает бот
✅ **Регрессионных тестов структуры** - убедиться что формат не изменился

### Но НЕ проверяет:

❌ Что ваш `handle_start_command` работает правильно
❌ Что при `/info` отправляется нужное сообщение
❌ Что обработка URL делает правильные API вызовы

## ✅ Как добавить тесты С реальной логикой

### Вариант 1: Полная интеграция (рекомендуется)

```rust
use doradura::telegram::menu::show_main_menu;
use common::{TelegramMock, create_test_message};

#[tokio::test]
async fn test_start_command_calls_real_handler() {
    // 1. Загрузить snapshot с ОЖИДАЕМЫМИ вызовами
    let mock = TelegramMock::from_snapshot("start_command").await?;
    let bot = mock.create_bot()?;

    // 2. Подготовить данные
    let chat_id = ChatId(123456789);
    let db_pool = create_test_db_pool()?;

    // 3. ВЫЗВАТЬ ВАШУ РЕАЛЬНУЮ ФУНКЦИЮ! 🎯
    let result = show_main_menu(&bot, chat_id, &db_pool).await;

    // 4. Проверить что функция успешна
    assert!(result.is_ok(), "show_main_menu должна отработать успешно");

    // 5. ВАЖНО: Проверить что были сделаны ПРАВИЛЬНЫЕ API вызовы
    mock.verify().await.expect("Функция должна была вызвать sendMessage");
}
```

**Что проверяется:**
- ✅ Ваша функция работает без ошибок
- ✅ Она делает правильные вызовы к Telegram API
- ✅ Структура вызовов совпадает со snapshot
- ✅ Параметры (text, chat_id, buttons) правильные

### Вариант 2: Тест обработчика команды

```rust
use doradura::telegram::commands::handle_info_command;

#[tokio::test]
async fn test_info_command_handler() {
    let mock = TelegramMock::from_snapshot("info_command").await?;
    let bot = mock.create_bot()?;

    // Создать фейковое сообщение "/info"
    let message = create_test_message("/info", 123456789, 111222333);
    let db_pool = create_test_db_pool()?;

    // Вызвать обработчик
    let result = handle_info_command(&bot, message, &db_pool).await;
    assert!(result.is_ok());

    // Проверить что отправлено сообщение с информацией
    mock.verify().await.expect("Должно быть отправлено сообщение с info");
}
```

### Вариант 3: Тест сложного flow

```rust
use doradura::telegram::commands::handle_message;

#[tokio::test]
async fn test_youtube_url_complete_flow() {
    // Snapshot содержит 3 взаимодействия
    let mock = TelegramMock::from_snapshot("youtube_processing").await?;
    let bot = mock.create_bot()?;

    let message = create_test_message(
        "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
        123456789,
        111222333
    );

    // Вызвать обработчик URL
    let result = handle_message(
        bot.clone(),
        message,
        download_queue,
        rate_limiter,
        db_pool
    ).await;

    assert!(result.is_ok());

    // Проверить последовательность вызовов:
    // 1. sendMessage("Обрабатываю...")
    // 2. sendPhoto(preview с кнопками)
    // 3. deleteMessage(временное сообщение)
    mock.verify().await.expect("Flow должен сделать 3 вызова");
}
```

## 🏗️ Структура тестов

### Уровень 1: Валидация snapshots (есть сейчас)

```
tests/bot_snapshots_test.rs
tests/bot_commands_test.rs
```

**Цель:** Проверить что snapshots валидны и содержат ожидаемые данные

### Уровень 2: Интеграционные тесты (нужно добавить)

```
tests/bot_integration_test.rs     ⬅️ НОВЫЙ!
tests/commands_integration_test.rs ⬅️ НОВЫЙ!
```

**Цель:** Вызывать реальные обработчики и проверять API вызовы

### Уровень 3: End-to-end тесты (опционально)

```
tests/e2e/                         ⬅️ БУДУЩЕЕ
├── test_download_flow.rs
└── test_settings_flow.rs
```

**Цель:** Полный цикл от команды до результата

## 📝 Пример: Добавление интеграционного теста

### Шаг 1: Создать snapshot (уже есть)

```json
// tests/snapshots/info_command.json
{
  "name": "info_command",
  "interactions": [
    [
      {"method": "POST", "path": "/sendMessage", ...},
      {"status": 200, "body": {...}}
    ]
  ]
}
```

### Шаг 2: Написать тест

```rust
// tests/commands_integration_test.rs

mod common;
use common::{TelegramMock, create_test_message};
use doradura::telegram::commands::handle_info_command;
use doradura::storage::create_pool;

#[tokio::test]
async fn test_info_command_sends_correct_message() {
    // Setup mock server
    let mock = TelegramMock::from_snapshot("info_command").await.unwrap();
    let bot = mock.create_bot().unwrap();

    // Setup test data
    let message = create_test_message("/info", 123456789, 111222333);
    let db_pool = create_pool(":memory:").unwrap();

    // Call REAL handler
    let result = handle_info_command(&bot, message, &db_pool).await;

    // Verify
    assert!(result.is_ok(), "Handler should succeed");
    mock.verify().await.expect("Should send info message");
}
```

### Шаг 3: Запустить

```bash
cargo test test_info_command_sends_correct_message
```

## 🎯 Что нужно для интеграционных тестов

### 1. Test DB Setup

```rust
fn create_test_db_pool() -> anyhow::Result<Arc<DbPool>> {
    let pool = create_pool(":memory:")?;

    // Выполнить миграции
    run_migrations(&pool)?;

    // Добавить тестовые данные
    insert_test_user(&pool, 123456789)?;

    Ok(Arc::new(pool))
}
```

### 2. Test Data Factories

```rust
fn create_test_user(id: i64) -> User { ... }
fn create_test_message(text: &str) -> Message { ... }
fn create_test_callback_query(data: &str) -> CallbackQuery { ... }
```

### 3. Assertions Helpers

```rust
fn assert_sent_message_with_text(mock: &TelegramMock, expected: &str) {
    let snapshot = mock.snapshot();
    let (call, _) = &snapshot.interactions[0];

    assert_eq!(call.path, "/sendMessage");
    assert!(call.body["text"].as_str().unwrap().contains(expected));
}
```

## 🔧 Готовый шаблон

Создан файл [tests/bot_integration_test.rs](../tests/bot_integration_test.rs) с примерами!

```bash
# Посмотрите шаблоны
cat tests/bot_integration_test.rs

# Раскомментируйте код и запустите
cargo test --test bot_integration_test
```

## ⚙️ Настройка проекта для интеграционных тестов

### 1. Экспортировать нужные функции

В `src/telegram/mod.rs`:

```rust
// Добавить pub use для тестов
pub use commands::{handle_info_command, handle_message};
pub use menu::show_main_menu;
```

### 2. Добавить feature для тестов (опционально)

В `Cargo.toml`:

```toml
[features]
testing = []

[dev-dependencies]
# Уже есть
```

### 3. Создать test utilities

```rust
// tests/common/test_db.rs
pub fn create_test_db() -> DbPool { ... }
pub fn insert_test_user(pool: &DbPool, id: i64) { ... }
```

## 📊 Сравнение подходов

| Подход | Что проверяет | Скорость | Сложность |
|--------|---------------|----------|-----------|
| **Валидация snapshot** | Структура данных | ⚡ Очень быстро | ✅ Просто |
| **Интеграция с mock** | Реальная логика + API вызовы | ⚡ Быстро | ⚠️ Средне |
| **E2E с реальным API** | Всё вместе | 🐌 Медленно | ❌ Сложно |

## 🎓 Рекомендации

### Используйте оба подхода:

1. **Валидация snapshots** (есть) - быстрая проверка структуры
2. **Интеграционные тесты** (добавьте) - проверка логики

### Примерное соотношение:

- 📸 70% тестов - валидация snapshots (быстрые)
- 🔧 30% тестов - интеграция с реальной логикой (важные flows)

### Приоритеты для интеграции:

1. ✅ Критичные команды (`/start`, `/info`)
2. ✅ Сложные flows (download, settings)
3. ✅ Обработка ошибок (rate limit, invalid URL)
4. ⚠️ Редкие кейсы (по мере необходимости)

## 🚀 Следующие шаги

1. **Изучите** [tests/bot_integration_test.rs](../tests/bot_integration_test.rs)
2. **Раскомментируйте** один из примеров
3. **Добавьте** недостающие зависимости (DB setup)
4. **Запустите** тест
5. **Расширяйте** покрытие

## 📚 См. также

- [SNAPSHOT_TESTING.md](SNAPSHOT_TESTING.md) - общая документация
- [tests/bot_integration_test.rs](../tests/bot_integration_test.rs) - примеры кода
- [tests/common/helpers.rs](../tests/common/helpers.rs) - test utilities
