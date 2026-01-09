# Snapshot Testing для Telegram Бота

Система snapshot-тестирования позволяет записывать реальные взаимодействия с Telegram API и воспроизводить их в тестах без живого бота.

## 🎯 Зачем это нужно?

✅ **Быстрые тесты** - не нужно ждать реальных API вызовов
✅ **Детерминированность** - тесты всегда дают одинаковый результат
✅ **Оффлайн работа** - можно тестировать без интернета
✅ **Изоляция** - тесты не зависят от состояния Telegram серверов
✅ **Документация** - snapshots показывают как работает API

## 📁 Структура

```
doradura/
├── src/testing/           # Модуль для snapshot тестирования
│   ├── mod.rs            # Экспорты
│   ├── snapshots.rs      # Загрузка и воспроизведение snapshots
│   └── recorder.rs       # Запись взаимодействий
├── tests/
│   ├── snapshots/        # Сохраненные snapshots
│   │   ├── start_command.json
│   │   ├── youtube_download.json
│   │   └── settings_menu.json
│   └── bot_snapshots_test.rs  # Тесты на основе snapshots
```

## 🎬 Как записать snapshot

### Способ 1: Ручное создание snapshot (рекомендуется)

Самый простой способ - создать snapshot вручную на основе реальных ответов:

1. **Включите детальное логирование запросов** (уже есть в коде):
```bash
RUST_LOG=debug cargo run
```

2. **Отправьте команду боту** через Telegram

3. **Скопируйте JSON из логов** - вы увидите что-то вроде:
```
[DEBUG] Request to https://api.telegram.org/bot.../sendMessage
Body: {"chat_id":123,"text":"Hello",...}
[DEBUG] Response: {"ok":true,"result":{...}}
```

4. **Создайте snapshot файл**:
```json
{
  "name": "my_test_scenario",
  "version": "1.0",
  "recorded_at": "2026-01-04T12:00:00Z",
  "interactions": [
    {
      "method": "POST",
      "path": "/sendMessage",
      "body": { /* данные из Request */ },
      "timestamp": 1735992000
    },
    {
      "status": 200,
      "body": { /* данные из Response */ },
      "headers": {
        "content-type": "application/json"
      }
    }
  ],
  "metadata": {
    "description": "Описание сценария",
    "command": "/start"
  }
}
```

5. **Сохраните в** `tests/snapshots/my_test_scenario.json`

### Способ 2: Использование локального Bot API с логированием

Если у вас настроен локальный Bot API (см. `LOCAL_BOT_API_SETUP.md`), можно перехватывать запросы через nginx/mitmproxy:

```bash
# Установить mitmproxy
brew install mitmproxy  # macOS
apt install mitmproxy   # Linux

# Запустить прокси
mitmproxy --port 8080 --mode reverse:http://localhost:8081

# В .env указать прокси
BOT_API_URL=http://localhost:8080

# Запустить бота и использовать его
cargo run

# mitmproxy сохранит все запросы/ответы
# Нажмите 'w' чтобы сохранить конкретный flow
```

### Способ 3: Программная запись (требует доработки)

```rust
use doradura::testing::recorder::RecordingClient;

#[tokio::main]
async fn record_scenario() {
    let recorder = RecordingClient::new("my_scenario");

    // Использовать бота как обычно
    // (требуется интеграция с teloxide)

    // Сохранить snapshot
    recorder.save_to_default_dir().unwrap();
}
```

## 🧪 Как использовать snapshot в тестах

### Базовый пример

```rust
use doradura::testing::TelegramMock;
use teloxide::prelude::*;

#[tokio::test]
async fn test_start_command() {
    // Загрузить snapshot
    let mock = TelegramMock::from_snapshot("start_command")
        .await
        .expect("Failed to load snapshot");

    // Создать бота с mock сервером
    let bot = mock.create_bot().expect("Failed to create bot");

    // Использовать бота как обычно
    let result = bot
        .send_message(ChatId(123456789), "Welcome!")
        .await;

    // Проверить результат
    assert!(result.is_ok());

    // Проверить что все ожидаемые вызовы были сделаны
    mock.verify().await.expect("Verification failed");
}
```

### Тестирование сложного сценария

```rust
#[tokio::test]
async fn test_youtube_download_flow() {
    // Snapshot содержит всю последовательность:
    // 1. Отправка URL
    // 2. "Обрабатываю..."
    // 3. Preview с кнопками
    // 4. Выбор качества
    // 5. Отправка файла

    let mock = TelegramMock::from_snapshot("youtube_download_complete")
        .await
        .unwrap();

    let bot = mock.create_bot().unwrap();

    // Симулировать каждый шаг
    let msg1 = bot.send_message(ChatId(123), "Processing...").await.unwrap();
    let msg2 = bot.send_photo(ChatId(123), InputFile::url(...)).await.unwrap();
    let msg3 = bot.send_audio(ChatId(123), InputFile::file(...)).await.unwrap();

    // Все ответы будут из snapshot, без реальных API вызовов
    mock.verify().await.unwrap();
}
```

### Тестирование обработчиков команд

```rust
#[tokio::test]
async fn test_info_command_handler() {
    let mock = TelegramMock::from_snapshot("info_command").await.unwrap();
    let bot = mock.create_bot().unwrap();

    // Создать фейковое сообщение
    // (можно использовать builder или JSON десериализацию)
    let message = create_test_message("/info", 123456789);

    // Вызвать обработчик
    let result = handle_info_command(bot, message, db_pool).await;

    assert!(result.is_ok());
    mock.verify().await.unwrap();
}
```

## 📝 Примеры snapshot'ов

### Start Command
```json
{
  "name": "start_command",
  "interactions": [
    {
      "method": "POST",
      "path": "/sendMessage",
      "body": {
        "chat_id": 123456789,
        "text": "🎵 Привет! Я помогу тебе...",
        "reply_markup": { /* inline keyboard */ }
      }
    },
    {
      "status": 200,
      "body": {
        "ok": true,
        "result": { /* Message object */ }
      }
    }
  ]
}
```

### Download Flow
```json
{
  "name": "youtube_download",
  "interactions": [
    // 1. Отправка "Processing..."
    { "method": "POST", "path": "/sendMessage", ... },
    { "status": 200, "body": { "ok": true, ... } },

    // 2. Отправка preview
    { "method": "POST", "path": "/sendPhoto", ... },
    { "status": 200, "body": { "ok": true, ... } },

    // 3. Отправка файла
    { "method": "POST", "path": "/sendAudio", ... },
    { "status": 200, "body": { "ok": true, ... } }
  ]
}
```

## 🛠️ Создание snapshot для разных сценариев

### 1. Команды бота
```bash
# В боте отправить: /start
# Скопировать запрос/ответ из логов
# Создать: tests/snapshots/start_command.json

# Аналогично для других команд:
# /info -> info_command.json
# /settings -> settings_command.json
# /downloads -> downloads_command.json
```

### 2. Callback кнопки
```bash
# Нажать кнопку "Настройки"
# Записать callback_query и ответ
# Создать: settings_callback.json
```

### 3. Обработка URL
```bash
# Отправить YouTube URL
# Записать всю цепочку взаимодействий
# Создать: youtube_url_processing.json
```

### 4. Ошибки
```bash
# Вызвать ошибку (например, невалидный URL)
# Записать error response
# Создать: invalid_url_error.json
```

## 🔧 Продвинутые техники

### Параметризованные тесты

```rust
#[rstest]
#[case("start_command")]
#[case("info_command")]
#[case("settings_command")]
#[tokio::test]
async fn test_commands(#[case] snapshot_name: &str) {
    let mock = TelegramMock::from_snapshot(snapshot_name).await.unwrap();
    let bot = mock.create_bot().unwrap();

    // Общая логика тестирования

    mock.verify().await.unwrap();
}
```

### Модификация snapshot в тесте

```rust
#[tokio::test]
async fn test_with_different_user_id() {
    let mut snapshot = TelegramSnapshot::load_by_name("start_command").unwrap();

    // Изменить user_id во всех взаимодействиях
    for (call, _) in &mut snapshot.interactions {
        if let Some(chat_id) = call.body.get_mut("chat_id") {
            *chat_id = serde_json::json!(999999);
        }
    }

    let mock = TelegramMock::from_snapshot_data(snapshot).await.unwrap();
    // ...
}
```

### Частичное совпадение (для нестабильных полей)

```rust
// В snapshot можно использовать placeholders для динамических полей
{
  "body": {
    "message_id": "__ANY__",  // Любое значение
    "date": "__TIMESTAMP__",   // Любой timestamp
    "text": "Hello, {{username}}!"  // Template
  }
}
```

## 🚀 Запуск тестов

```bash
# Все snapshot тесты
cargo test --test bot_snapshots_test

# Конкретный тест
cargo test --test bot_snapshots_test test_start_command

# С выводом
cargo test --test bot_snapshots_test -- --nocapture

# В режиме записи (если реализовано)
TELEGRAM_RECORD_MODE=true cargo test
```

## 📊 Best Practices

1. **Одна функция = один snapshot** - не смешивайте разные сценарии
2. **Говорящие имена** - `user_sends_youtube_url_gets_preview.json`
3. **Комментарии в metadata** - объясните что происходит
4. **Версионирование** - при изменении API обновляйте version
5. **Минимальные данные** - не записывайте лишние поля
6. **Git** - коммитьте snapshots вместе с тестами

## 🐛 Отладка

### Snapshot не загружается
```bash
# Проверить путь
ls tests/snapshots/

# Валидировать JSON
jq . tests/snapshots/my_snapshot.json

# Проверить в тесте
let result = TelegramSnapshot::load_by_name("my_snapshot");
println!("{:?}", result);
```

### Mock не отвечает
```rust
// Добавить логирование
env_logger::init();

// Проверить что URL правильный
println!("Mock URL: {}", mock.uri());

// Проверить запросы через wiremock
// (см. документацию wiremock)
```

### Тест падает на verify()
```rust
// Посмотреть сколько вызовов было сделано
println!("Expected: {}", mock.snapshot().interactions.len());
println!("Got: {}", actual_calls);

// Отключить verify если не критично
// mock.verify().await.unwrap();
```

## 🔮 Будущие улучшения

- [ ] Автоматическая запись через HTTP proxy
- [ ] UI для просмотра snapshots
- [ ] Диффы между snapshots
- [ ] Fuzzing на основе snapshots
- [ ] Генерация snapshot из Postman/Insomnia коллекций
- [ ] Integration с cucumber для BDD тестов

## 📚 Дополнительные ресурсы

- [Telegram Bot API Reference](https://core.telegram.org/bots/api)
- [wiremock документация](https://docs.rs/wiremock/)
- [teloxide документация](https://docs.rs/teloxide/)
- [Примеры snapshot тестов](../tests/bot_snapshots_test.rs)
