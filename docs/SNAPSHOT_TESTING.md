# 📸 Snapshot Testing - Полная Система Тестирования Telegram Бота

Система для записи реальных взаимодействий с Telegram API и их воспроизведения в тестах.

## 🎯 Что это даёт?

✅ **Снимаете кальку с живого бота** - все ответы взяты из реальных API вызовов
✅ **Быстрые тесты** - нет реальных сетевых запросов
✅ **Детерминированность** - тесты всегда дают одинаковый результат
✅ **Оффлайн работа** - можно тестировать без интернета
✅ **Документация** - snapshots показывают как работает API

## 📊 Текущее состояние

- **Snapshots**: 7 штук
- **API методов**: 7 различных (sendMessage, sendPhoto, sendAudio, ...)
- **Тестов**: 18 автоматических тестов
- **Сценариев**: Команды, настройки, загрузка, ошибки

## 🚀 Быстрый старт

### 1. Посмотрите существующие snapshots

```bash
ls tests/snapshots/*.json
```

Доступны:
- `start_command.json` - Команда /start
- `info_command.json` - Информация о форматах
- `settings_menu.json` - Меню настроек
- `youtube_processing.json` - Обработка YouTube URL
- `audio_download_complete.json` - Полный цикл скачивания
- `language_selection.json` - Выбор языка
- `rate_limit_error.json` - Ошибка лимита

### 2. Запустите тесты

```bash
# Все snapshot тесты
cargo test --test bot_snapshots_test --test bot_commands_test

# Конкретный тест
cargo test test_youtube_processing_flow

# С выводом
cargo test --test bot_commands_test -- --nocapture
```

### 3. Используйте в своих тестах

```rust
use common::TelegramMock;

#[tokio::test]
async fn test_my_feature() {
    let mock = TelegramMock::from_snapshot("youtube_processing").await?;
    let bot = mock.create_bot()?;
    
    // Используйте bot - все ответы будут из snapshot
    // handle_youtube_url(&bot, url).await?;
}
```

## 📁 Структура проекта

```
doradura/
├── src/
│   └── testing/              # (только для unit tests)
│       ├── mod.rs
│       ├── snapshots.rs
│       └── recorder.rs
│
├── tests/
│   ├── common/               # Shared testing utilities
│   │   ├── mod.rs
│   │   ├── snapshots.rs      # TelegramMock, TelegramSnapshot
│   │   └── recorder.rs       # RecordingClient (helper)
│   │
│   ├── snapshots/            # JSON snapshots ⭐
│   │   ├── README.md
│   │   ├── SNAPSHOT_INDEX.md
│   │   ├── start_command.json
│   │   ├── info_command.json
│   │   ├── settings_menu.json
│   │   ├── language_selection.json
│   │   ├── youtube_processing.json
│   │   ├── audio_download_complete.json
│   │   └── rate_limit_error.json
│   │
│   ├── bot_snapshots_test.rs    # Базовые тесты
│   └── bot_commands_test.rs     # Детальные тесты команд
│
├── tools/
│   └── log_to_snapshot.py    # Конвертер логов → JSON
│
└── docs/
    ├── SNAPSHOT_TESTING.md           # Полная документация
    └── SNAPSHOT_TESTING_QUICKSTART.md # Быстрый старт
```

## 🎬 Как создать новый snapshot

### Способ 1: Вручную (рекомендуется)

1. Запустите бота с логированием:
   ```bash
   RUST_LOG=debug cargo run
   ```

2. Выполните действие в Telegram (например, отправьте /info)

3. Скопируйте JSON из логов

4. Создайте файл `tests/snapshots/my_test.json`:
   ```json
   {
     "name": "my_test",
     "version": "1.0",
     "recorded_at": "2026-01-04T12:00:00Z",
     "interactions": [
       [
         {"method": "POST", "path": "/sendMessage", "body": {...}, "timestamp": 123},
         {"status": 200, "body": {...}, "headers": {...}}
       ]
     ],
     "metadata": {}
   }
   ```

### Способ 2: Python утилита

```bash
# Интерактивный режим
./tools/log_to_snapshot.py --interactive

# Из файла логов
./tools/log_to_snapshot.py --input bot.log --name my_test

# Из потока
cargo run 2>&1 | ./tools/log_to_snapshot.py --stdin --name my_test
```

## 📚 Документация

- **[SNAPSHOT_TESTING.md](docs/SNAPSHOT_TESTING.md)** - Полное руководство (200+ строк)
- **[SNAPSHOT_TESTING_QUICKSTART.md](docs/SNAPSHOT_TESTING_QUICKSTART.md)** - Быстрый старт
- **[tests/snapshots/README.md](tests/snapshots/README.md)** - Список всех snapshots
- **[tests/snapshots/SNAPSHOT_INDEX.md](tests/snapshots/SNAPSHOT_INDEX.md)** - Индекс с деталями

## 🧪 Примеры тестов

### Базовый тест команды
```rust
#[tokio::test]
async fn test_info_command() {
    let mock = TelegramMock::from_snapshot("info_command").await?;
    let snapshot = mock.snapshot();
    
    assert_eq!(snapshot.interactions.len(), 1);
    let (_call, response) = &snapshot.interactions[0];
    
    let text = response.body["result"]["text"].as_str().unwrap();
    assert!(text.contains("Видео"));
    assert!(text.contains("320 kbps"));
}
```

### Тест сложного flow
```rust
#[tokio::test]
async fn test_audio_download_flow() {
    let snapshot = TelegramSnapshot::load_by_name("audio_download_complete")?;
    
    // 5 шагов: 0% → 45% → 100% → sendAudio → cleanup
    assert_eq!(snapshot.interactions.len(), 5);
    
    // Проверка прогресса
    let (_call1, resp1) = &snapshot.interactions[0];
    assert!(resp1.body["result"]["caption"].as_str().unwrap().contains("0%"));
    
    // Проверка файла
    let (_call4, resp4) = &snapshot.interactions[3];
    let audio = &resp4.body["result"]["audio"];
    assert_eq!(audio["performer"].as_str().unwrap(), "Rick Astley");
}
```

## 🎨 Что можно тестировать?

### ✅ Команды бота
- `/start`, `/info`, `/settings`, `/help`
- Проверка текста, кнопок, форматирования

### ✅ Callback queries
- Выбор языка, качества, формата
- Проверка answerCallbackQuery, обновления сообщений

### ✅ Сложные flows
- Обработка URL → preview → скачивание → отправка
- Многошаговые взаимодействия

### ✅ Обработка ошибок
- Rate limiting, неверные URL, сетевые ошибки
- Проверка корректных сообщений об ошибках

### ✅ Прогресс операций
- Обновление прогресса скачивания
- editMessage операции

## 📈 Метрики покрытия

```
API методы покрыты:
  ✅ sendMessage        (6 snapshots)
  ✅ sendPhoto          (1 snapshot)
  ✅ sendAudio          (1 snapshot)
  ✅ deleteMessage      (2 snapshots)
  ✅ editMessageCaption (1 snapshot)
  ✅ editMessageText    (1 snapshot)
  ✅ answerCallbackQuery(1 snapshot)

Всего: 7/20+ методов Bot API
```

## 🔧 Расширение

### Добавьте новые snapshots для:

1. **Скачивание видео** - `video_download_complete.json`
2. **История загрузок** - `downloads_list.json`
3. **Вырезки** - `cuts_menu.json`, `cut_creation.json`
4. **Админ команды** - `admin_users_list.json`, `admin_backup.json`
5. **Подписки** - `subscription_purchase.json`
6. **Ошибки** - `invalid_url.json`, `network_error.json`

### Шаблон для нового snapshot:
```bash
cp tests/snapshots/start_command.json tests/snapshots/my_new_test.json
# Отредактируйте JSON
# Добавьте тест в tests/bot_commands_test.rs
```

## 🎯 Следующие шаги

1. **Изучите** существующие snapshots в [tests/snapshots/](tests/snapshots/)
2. **Запустите** тесты: `cargo test --test bot_commands_test`
3. **Создайте** свой snapshot для нового функционала
4. **Добавьте** тест в `tests/bot_commands_test.rs`
5. **Проверьте**: `cargo test`

## 💡 Best Practices

- Один snapshot = один сценарий
- Говорящие имена файлов
- Комментарии в metadata
- Минимальные данные (без лишних полей)
- Версионирование в Git
- Регулярное обновление при изменении API

## 🐛 Troubleshooting

**Snapshot не загружается:**
```bash
# Проверьте JSON
jq . tests/snapshots/my_test.json

# Посмотрите ошибку
cargo test test_my_snapshot -- --nocapture
```

**Тест падает:**
```rust
// Добавьте отладку
let snapshot = TelegramSnapshot::load_by_name("my_test")?;
println!("Loaded: {:?}", snapshot);
```

## 📞 Помощь

- Документация: [docs/SNAPSHOT_TESTING.md](docs/SNAPSHOT_TESTING.md)
- Примеры: [tests/bot_commands_test.rs](tests/bot_commands_test.rs)
- Индекс: [tests/snapshots/SNAPSHOT_INDEX.md](tests/snapshots/SNAPSHOT_INDEX.md)

---

**Статус**: ✅ Полностью рабочая система
**Тестов**: 18 passing
**Покрытие**: Команды, настройки, загрузка, ошибки
