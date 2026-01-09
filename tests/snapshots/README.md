# Telegram Bot Snapshots

Эта директория содержит записанные взаимодействия с Telegram API для тестирования.

## Что такое snapshot?

Snapshot - это JSON файл, содержащий:
- Запросы к Telegram API (метод, путь, body)
- Ответы от Telegram API (status, body, headers)
- Метаданные (описание сценария, дата записи)

## Структура snapshot файла

```json
{
  "name": "имя_сценария",
  "version": "1.0",
  "recorded_at": "2026-01-04T12:00:00Z",
  "interactions": [
    [
      {
        "method": "POST",
        "path": "/sendMessage",
        "body": { "chat_id": 123, "text": "Hello" },
        "timestamp": 1735992000
      },
      {
        "status": 200,
        "body": { "ok": true, "result": {...} },
        "headers": { "content-type": "application/json" }
      }
    ]
  ],
  "metadata": {
    "description": "Описание сценария",
    "command": "/start"
  }
}
```

## Существующие snapshots

### start_command.json
- **Описание**: Пользователь отправляет /start и получает приветственное сообщение с главным меню
- **Команда**: `/start`
- **Взаимодействий**: 1
- **Использование**:
  ```rust
  let mock = TelegramMock::from_snapshot("start_command").await?;
  ```

### info_command.json
- **Описание**: Пользователь запрашивает информацию о поддерживаемых форматах
- **Команда**: `/info`
- **Взаимодействий**: 1
- **Включает**: Список форматов видео/аудио, поддерживаемые сервисы

### settings_menu.json
- **Описание**: Отображение главного меню настроек с текущими предпочтениями
- **Команда**: `/settings`
- **Взаимодействий**: 1
- **Включает**: Качество видео, битрейт аудио, формат по умолчанию, язык

### language_selection.json
- **Описание**: Полный flow выбора языка интерфейса
- **Flow**: Показ меню языков → выбор → callback → обновление настроек
- **Взаимодействий**: 3
- **Включает**: answerCallbackQuery, editMessageText

### youtube_processing.json
- **Описание**: Обработка YouTube ссылки и показ preview с опциями качества
- **Flow**: Сообщение "Обрабатываю" → Preview с thumbnail → Удаление временного сообщения
- **Взаимодействий**: 3
- **Включает**: sendMessage, sendPhoto, deleteMessage
- **URL**: `https://www.youtube.com/watch?v=dQw4w9WgXcQ`

### audio_download_complete.json
- **Описание**: Полный цикл скачивания аудио с прогрессом
- **Flow**: 0% → 45% → 100% → отправка файла → очистка
- **Взаимодействий**: 5
- **Включает**: editMessageCaption (прогресс), sendAudio, deleteMessage
- **Детали**: Rick Astley - Never Gonna Give You Up, 192kbps, 5MB

### rate_limit_error.json
- **Описание**: Пользователь превышает лимит запросов
- **Взаимодействий**: 1
- **Включает**: Сообщение об ошибке с оставшимся временем (45 сек)
- **Error type**: rate_limit

## Как создать новый snapshot

### Способ 1: Вручную (рекомендуется)

1. Запустите бота с логированием:
   ```bash
   RUST_LOG=debug cargo run
   ```

2. Выполните нужное действие в Telegram

3. Скопируйте запрос/ответ из логов

4. Создайте JSON файл в `tests/snapshots/`

5. Используйте в тестах

### Способ 2: Python утилита

```bash
# Из логов
./tools/log_to_snapshot.py --input bot.log --name my_test --output tests/snapshots/my_test.json

# Из stdin
cargo run 2>&1 | ./tools/log_to_snapshot.py --stdin --name my_test

# Интерактивно
./tools/log_to_snapshot.py --interactive
```

### Способ 3: Через mitmproxy

```bash
# Настроить прокси
mitmproxy --port 8080 --mode reverse:http://localhost:8081

# В .env
BOT_API_URL=http://localhost:8080

# Использовать бота
cargo run

# Сохранить flows из mitmproxy
```

## Соглашения об именовании

- `{command}_command.json` - для команд бота (`start_command.json`)
- `{action}_callback.json` - для callback кнопок (`settings_callback.json`)
- `{feature}_flow.json` - для сложных сценариев (`youtube_download_flow.json`)
- `{error}_error.json` - для ошибок (`invalid_url_error.json`)

## Тестирование с snapshots

```rust
use common::TelegramMock;

#[tokio::test]
async fn test_my_feature() {
    let mock = TelegramMock::from_snapshot("my_snapshot").await?;
    let bot = mock.create_bot()?;

    // Ваш код тестирования
    // Бот будет использовать mock server вместо реального API

    mock.verify().await?; // Проверить что все ожидаемые вызовы были сделаны
}
```

## Подробная документация

См. [docs/SNAPSHOT_TESTING.md](../../docs/SNAPSHOT_TESTING.md)
