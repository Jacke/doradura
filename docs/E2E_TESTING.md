# E2E Тестирование без Реального Telegram

## 🎯 Цель

Создать **полностью изолированные E2E тесты** которые:
- ✅ Проверяют ВСЮ логику бота от начала до конца
- ✅ Не делают НИКАКИХ реальных HTTP запросов
- ✅ Тестируют внутренние состояния (БД, кэш, очереди)
- ✅ Запускаются быстро и детерминированно

## 🏗️ Архитектура

```
┌─────────────────────────────────────────────────────────┐
│                    Ваш код бота                          │
│  (handlers, commands, download logic)                    │
└────────────────────┬────────────────────────────────────┘
                     │
                     ▼
┌─────────────────────────────────────────────────────────┐
│              Bot Abstraction Layer                       │
│  ┌──────────────────┐    ┌──────────────────┐          │
│  │ Production Bot   │    │   Test Bot       │          │
│  │ (real Telegram)  │    │  (mock server)   │          │
│  └──────────────────┘    └──────────────────┘          │
└─────────────────────────────────────────────────────────┘
```

## 🔧 Решение 1: Trait-based Abstraction (Рекомендуется)

### Шаг 1: Создать trait для Bot операций

```rust
// src/bot_trait.rs

#[async_trait]
pub trait BotOperations: Clone + Send + Sync + 'static {
    async fn send_message(&self, chat_id: ChatId, text: String) -> Result<Message>;
    async fn send_photo(&self, chat_id: ChatId, photo: InputFile) -> Result<Message>;
    async fn send_audio(&self, chat_id: ChatId, audio: InputFile) -> Result<Message>;
    async fn edit_message_text(&self, chat_id: ChatId, message_id: MessageId, text: String) -> Result<()>;
    // ... остальные методы
}

// Реализация для реального бота
#[async_trait]
impl BotOperations for Bot {
    async fn send_message(&self, chat_id: ChatId, text: String) -> Result<Message> {
        self.send_message(chat_id, text).await.map_err(|e| e.into())
    }
    // ...
}

// Реализация для мока
pub struct MockBot {
    mock_server: Arc<TelegramMock>,
    bot: Bot,
}

#[async_trait]
impl BotOperations for MockBot {
    async fn send_message(&self, chat_id: ChatId, text: String) -> Result<Message> {
        self.bot.send_message(chat_id, text).await.map_err(|e| e.into())
    }
    // ...
}
```

**Проблема:** teloxide::Bot не реализует Clone, методы сложные для обёртывания.

## 🔧 Решение 2: Wrapper Pattern (Проще)

### Создать обёртку вокруг Bot

```rust
// src/testing/test_bot.rs

pub struct TestableBot {
    inner: Bot,
    recorder: Option<Arc<Mutex<Vec<ApiCall>>>>,
}

impl TestableBot {
    // Production
    pub fn production(bot: Bot) -> Self {
        Self { inner: bot, recorder: None }
    }

    // Testing
    pub fn with_mock(mock: TelegramMock) -> Self {
        let bot = mock.create_bot().unwrap();
        Self { inner: bot, recorder: Some(Arc::new(Mutex::new(Vec::new()))) }
    }

    // Wrapper methods
    pub async fn send_message(&self, chat_id: ChatId, text: impl Into<String>) -> ResponseResult<Message> {
        let text = text.into();

        // Record call if in test mode
        if let Some(recorder) = &self.recorder {
            recorder.lock().unwrap().push(ApiCall {
                method: "POST".to_string(),
                path: "/sendMessage".to_string(),
                body: serde_json::json!({"chat_id": chat_id, "text": text}),
                timestamp: 0,
            });
        }

        self.inner.send_message(chat_id, text).await
    }

    // Verification for tests
    pub fn verify_calls(&self, expected: &[(&str, &str)]) {
        if let Some(recorder) = &self.recorder {
            let calls = recorder.lock().unwrap();
            assert_eq!(calls.len(), expected.len());
            // ... verify each call
        }
    }
}
```

**Проблема:** Нужно обернуть ВСЕ методы Bot (100+ методов).

## 🔧 Решение 3: Test Fixtures (Практично)

### Полная тестовая среда

Вместо оборачивания Bot, создадим **тестовые фикстуры** которые настраивают всё окружение:

```rust
// tests/common/fixtures.rs

pub struct TestEnvironment {
    pub bot: Bot,
    pub db_pool: Arc<DbPool>,
    pub download_queue: Arc<DownloadQueue>,
    pub rate_limiter: Arc<RateLimiter>,
    pub mock: TelegramMock,
}

impl TestEnvironment {
    pub async fn new(snapshot_name: &str) -> anyhow::Result<Self> {
        // 1. Setup mock Telegram API
        let mock = TelegramMock::from_snapshot(snapshot_name).await?;
        let bot = mock.create_bot()?;

        // 2. Setup in-memory database
        let db_pool = Arc::new(create_pool(":memory:")?);
        run_migrations(&db_pool)?;
        insert_test_data(&db_pool)?;

        // 3. Setup download queue
        let download_queue = Arc::new(DownloadQueue::new());

        // 4. Setup rate limiter (disabled for tests)
        let rate_limiter = Arc::new(RateLimiter::new_disabled());

        Ok(Self {
            bot,
            db_pool,
            download_queue,
            rate_limiter,
            mock,
        })
    }

    // Helper: Create test user
    pub fn create_user(&self, user_id: i64) -> anyhow::Result<()> {
        db::create_or_get_user(&self.db_pool, user_id, "ru")?;
        Ok(())
    }

    // Helper: Simulate message from user
    pub fn user_message(&self, user_id: i64, text: &str) -> Message {
        // Deserialize from JSON snapshot or create minimal Message
        self.create_message_from_json(user_id, text)
    }

    // Verify API calls match snapshot
    pub async fn verify(&self) -> anyhow::Result<()> {
        self.mock.verify().await
    }
}
```

## 📝 Пример E2E теста

### Test: Полный flow команды /start

```rust
// tests/e2e/test_start_command.rs

#[tokio::test]
async fn test_start_command_complete_flow() {
    // 1. Setup environment
    let env = TestEnvironment::new("start_command").await.unwrap();
    env.create_user(123456789).unwrap();

    // 2. Create test message
    let message = env.user_message(123456789, "/start");

    // 3. Call REAL handler
    let result = handle_start_command(
        env.bot.clone(),
        message,
        env.db_pool.clone()
    ).await;

    // 4. Verify handler succeeded
    assert!(result.is_ok());

    // 5. Verify API calls were made
    env.verify().await.unwrap();

    // 6. Verify database state
    let user = db::get_user(&env.db_pool, 123456789).unwrap();
    assert!(user.is_some());
    assert_eq!(user.unwrap().language, "ru");
}
```

### Test: YouTube URL processing E2E

```rust
#[tokio::test]
async fn test_youtube_download_e2e() {
    let env = TestEnvironment::new("youtube_processing").await.unwrap();
    env.create_user(123456789).unwrap();

    // User sends YouTube URL
    let message = env.user_message(123456789, "https://youtube.com/watch?v=test");

    // Process URL
    let result = handle_message(
        env.bot.clone(),
        message,
        env.download_queue.clone(),
        env.rate_limiter.clone(),
        env.db_pool.clone()
    ).await;

    assert!(result.is_ok());

    // Verify sequence:
    // 1. "Processing..." sent
    // 2. Preview with buttons sent
    // 3. Processing message deleted
    env.verify().await.unwrap();

    // Verify download queue state
    assert_eq!(env.download_queue.pending_count(), 1);
}
```

### Test: Settings change E2E

```rust
#[tokio::test]
async fn test_settings_change_quality_e2e() {
    let env = TestEnvironment::new("settings_quality_change").await.unwrap();
    env.create_user(123456789).unwrap();

    // 1. User opens settings
    let msg1 = env.user_message(123456789, "/settings");
    handle_settings_command(env.bot.clone(), msg1, env.db_pool.clone()).await.unwrap();

    // 2. User clicks "1080p"
    let callback = env.create_callback("settings:quality:1080");
    handle_settings_callback(env.bot.clone(), callback, env.db_pool.clone()).await.unwrap();

    // 3. Verify database updated
    let user = db::get_user(&env.db_pool, 123456789).unwrap().unwrap();
    assert_eq!(user.video_quality, "1080p");

    // 4. Verify UI updated
    env.verify().await.unwrap();
}
```

## 🎨 Создание фейковых Message объектов

### Вариант 1: JSON Deserialization

```rust
pub fn create_message_from_json(user_id: i64, text: &str) -> Message {
    let json = format!(r#"{{
        "message_id": 1,
        "date": 1234567890,
        "chat": {{"id": {}, "type": "private", "first_name": "Test"}},
        "from": {{"id": {}, "is_bot": false, "first_name": "Test"}},
        "text": "{}",
        "entities": []
    }}"#, user_id, user_id, text);

    serde_json::from_str(&json).expect("Failed to parse message JSON")
}
```

### Вариант 2: Builder Pattern

```rust
pub struct MessageBuilder {
    user_id: i64,
    chat_id: i64,
    text: String,
}

impl MessageBuilder {
    pub fn new(user_id: i64) -> Self {
        Self {
            user_id,
            chat_id: user_id,
            text: String::new(),
        }
    }

    pub fn text(mut self, text: impl Into<String>) -> Self {
        self.text = text.into();
        self
    }

    pub fn build(self) -> Message {
        create_message_from_json(self.user_id, &self.text)
    }
}

// Usage:
let msg = MessageBuilder::new(123).text("/start").build();
```

## 🗄️ Database Setup для тестов

```rust
// tests/common/test_db.rs

pub fn create_test_db_pool() -> anyhow::Result<Arc<DbPool>> {
    let pool = create_pool(":memory:")?;

    // Run migrations
    let mut conn = pool.get()?;
    refinery::embed_migrations!("migrations");
    migrations::runner().run(&mut conn)?;

    Ok(Arc::new(pool))
}

pub fn insert_test_user(pool: &DbPool, user_id: i64) -> anyhow::Result<()> {
    let conn = pool.get()?;
    conn.execute(
        "INSERT INTO users (telegram_id, language, video_quality, audio_bitrate)
         VALUES (?1, 'ru', '1080p', '192')",
        params![user_id],
    )?;
    Ok(())
}
```

## 🧪 Полный E2E Test Suite

```rust
// tests/e2e/mod.rs

mod test_commands;      // /start, /info, /settings
mod test_downloads;     // YouTube, SoundCloud
mod test_settings;      // Quality changes, language
mod test_rate_limiting; // Rate limits, upgrades
mod test_errors;        // Invalid URLs, network errors

// tests/e2e/test_commands.rs
mod common;
use common::TestEnvironment;

#[tokio::test]
async fn test_all_commands() {
    let commands = vec![
        ("start_command", "/start"),
        ("info_command", "/info"),
        ("settings_command", "/settings"),
    ];

    for (snapshot, command) in commands {
        println!("Testing: {}", command);

        let env = TestEnvironment::new(snapshot).await.unwrap();
        env.create_user(123456789).unwrap();

        let msg = env.user_message(123456789, command);
        let result = handle_command(env.bot.clone(), msg, env.db_pool.clone()).await;

        assert!(result.is_ok(), "{} should succeed", command);
        env.verify().await.unwrap();
    }
}
```

## 📊 Что можно проверять в E2E

### ✅ API Calls
```rust
env.verify_api_calls(&[
    ("POST", "/sendMessage"),
    ("POST", "/sendPhoto"),
    ("POST", "/deleteMessage"),
]);
```

### ✅ Database State
```rust
let user = db::get_user(&env.db_pool, user_id).unwrap().unwrap();
assert_eq!(user.video_quality, "1080p");
assert_eq!(user.downloads_count, 1);
```

### ✅ Queue State
```rust
assert_eq!(env.download_queue.pending_count(), 1);
let task = env.download_queue.pop().await.unwrap();
assert_eq!(task.url, "https://youtube.com/...");
```

### ✅ Cache State
```rust
let cached = cache::get(&env.cache, "preview:video_id").unwrap();
assert!(cached.is_some());
```

### ✅ Rate Limiter
```rust
assert!(!env.rate_limiter.is_limited(user_id).await);
env.rate_limiter.record(user_id).await;
assert!(env.rate_limiter.is_limited(user_id).await);
```

## 🚀 Запуск E2E тестов

```bash
# Все E2E тесты
cargo test --test e2e

# Конкретная категория
cargo test --test e2e test_commands

# С выводом
cargo test --test e2e -- --nocapture

# Параллельно (быстро)
cargo test --test e2e -- --test-threads=4
```

## 📈 Преимущества

✅ **Полная изоляция** - никаких внешних зависимостей
✅ **Быстро** - нет сетевых запросов
✅ **Детерминированно** - всегда одинаковый результат
✅ **Проверяет ВСЁ** - от команды до состояния БД
✅ **CI/CD friendly** - запускается в любом окружении

## 🎯 Следующие шаги

1. **Реализовать** TestEnvironment в `tests/common/fixtures.rs`
2. **Создать** message builders в `tests/common/builders.rs`
3. **Написать** первый E2E тест для `/start`
4. **Расширить** на остальные команды
5. **Добавить** в CI pipeline

## 📚 См. также

- [Пример реализации](../tests/e2e/) (будет создан)
- [SNAPSHOT_TESTING.md](SNAPSHOT_TESTING.md) - база для E2E
- [SNAPSHOT_TESTING_INTEGRATION.md](SNAPSHOT_TESTING_INTEGRATION.md) - интеграция
