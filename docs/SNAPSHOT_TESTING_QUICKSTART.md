# Быстрый старт: Snapshot Testing

## 🎯 Что это?

Система для записи и воспроизведения реальных взаимодействий с Telegram API в тестах.

## 🚀 За 5 минут

### 1. Запишите взаимодействие

```bash
# Включите логирование
RUST_LOG=debug cargo run

# Отправьте команду боту (например /start)
# Скопируйте JSON из логов
```

### 2. Создайте snapshot

```bash
./tools/log_to_snapshot.py --interactive
```

Или вручную создайте `tests/snapshots/my_test.json`:

```json
{
  "name": "my_test",
  "version": "1.0",
  "recorded_at": "2026-01-04T12:00:00Z",
  "interactions": [
    [
      {
        "method": "POST",
        "path": "/sendMessage",
        "body": {"chat_id": 123, "text": "Hello"},
        "timestamp": 1735992000
      },
      {
        "status": 200,
        "body": {"ok": true, "result": {...}},
        "headers": {"content-type": "application/json"}
      }
    ]
  ],
  "metadata": {}
}
```

### 3. Используйте в тесте

Добавьте в `tests/bot_test.rs`:

```rust
mod common;
use common::TelegramMock;

#[tokio::test]
async fn test_my_feature() {
    let mock = TelegramMock::from_snapshot("my_test").await.unwrap();
    let bot = mock.create_bot().unwrap();

    // Ваш код тестирования здесь
    // bot.send_message(...).await?;

    // mock.verify().await.unwrap(); // Опционально
}
```

### 4. Запустите тест

```bash
cargo test --test bot_test
```

## 📝 Примеры

### Тест команды /start

```rust
#[tokio::test]
async fn test_start_command() {
    let mock = TelegramMock::from_snapshot("start_command").await.unwrap();
    let bot = mock.create_bot().unwrap();

    // Вызовите ваш обработчик
    // handle_start_command(&bot, message).await?;

    // Проверки
    assert_eq!(mock.snapshot().interactions.len(), 1);
}
```

### Тест загрузки видео

```rust
#[tokio::test]
async fn test_youtube_download() {
    let mock = TelegramMock::from_snapshot("youtube_download").await.unwrap();
    let bot = mock.create_bot().unwrap();

    // Полный flow: preview -> выбор качества -> скачивание
    // ...
}
```

## 🛠️ Структура проекта

```
doradura/
├── src/
│   └── testing/          # (только для unit tests)
├── tests/
│   ├── common/           # Shared testing utilities
│   │   ├── snapshots.rs  # Snapshot loading/replay
│   │   └── recorder.rs   # Recording utilities
│   ├── snapshots/        # JSON snapshots
│   │   ├── start_command.json
│   │   └── README.md
│   └── bot_snapshots_test.rs  # Tests
├── tools/
│   └── log_to_snapshot.py     # Converter
└── docs/
    └── SNAPSHOT_TESTING.md    # Full docs
```

## ✨ Преимущества

✅ Быстрые тесты (нет реальных API вызовов)
✅ Детерминированные (всегда одинаковый результат)
✅ Работают оффлайн
✅ Документируют API взаимодействия
✅ Легко создавать новые тесты

## 📚 Дальше

- [Полная документация](SNAPSHOT_TESTING.md)
- [Примеры тестов](../tests/bot_snapshots_test.rs)
- [Существующие snapshots](../tests/snapshots/)
