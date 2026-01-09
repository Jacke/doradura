# Индекс Snapshots

Полный список всех доступных snapshots для тестирования.

## 📊 Статистика

- **Всего snapshots**: 7
- **Всего API взаимодействий**: 17
- **Покрытие**: Команды, настройки, загрузка, ошибки
- **Тестов**: 18 (11 в bot_commands_test + 7 в bot_snapshots_test)

## 📁 Список Snapshots

| Snapshot | Тип | Взаимодействий | Описание |
|----------|-----|----------------|----------|
| [start_command](#start_command) | Command | 1 | Команда /start с главным меню |
| [info_command](#info_command) | Command | 1 | Информация о форматах |
| [settings_menu](#settings_menu) | Command | 1 | Меню настроек |
| [language_selection](#language_selection) | Flow | 3 | Выбор языка интерфейса |
| [youtube_processing](#youtube_processing) | Flow | 3 | Обработка YouTube URL |
| [audio_download_complete](#audio_download_complete) | Flow | 5 | Полный цикл скачивания аудио |
| [rate_limit_error](#rate_limit_error) | Error | 1 | Превышение лимита запросов |

## 🔍 Детальное описание

### start_command
```
Type: Command
API Calls: 1
Methods: sendMessage
```
**Сценарий**: Пользователь отправляет /start
**Ответ**: Приветственное сообщение с inline клавиатурой (Информация, Настройки, Загрузки)
**Тест**: `test_start_command_from_snapshot`

---

### info_command
```
Type: Command
API Calls: 1
Methods: sendMessage
```
**Сценарий**: Пользователь запрашивает /info
**Ответ**: Подробная информация о:
- Форматах видео (2160p, 1440p, 1080p, 720p, 480p, 360p)
- Форматах аудио (320kbps, 192kbps, 128kbps)
- Поддерживаемых сервисах (YouTube, SoundCloud, Vimeo)

**Тест**: `test_info_command_snapshot`

---

### settings_menu
```
Type: Command
API Calls: 1
Methods: sendMessage
```
**Сценарий**: Пользователь открывает /settings
**Ответ**: Меню настроек с текущими параметрами:
- Качество видео: 1080p
- Битрейт аудио: 192 kbps
- Формат по умолчанию: Аудио
- Кнопки для изменения каждого параметра

**Тест**: `test_settings_menu_snapshot`

---

### language_selection
```
Type: Flow (Multi-step)
API Calls: 3
Methods: sendMessage → answerCallbackQuery → editMessageText
```
**Сценарий**:
1. Показ меню выбора языка (🇷🇺 Русский / 🇬🇧 English)
2. Пользователь выбирает русский
3. Callback query подтверждение
4. Обновление меню настроек с новым языком

**Тест**: `test_language_selection_flow`

---

### youtube_processing
```
Type: Flow (Multi-step)
API Calls: 3
Methods: sendMessage → sendPhoto → deleteMessage
```
**Сценарий**:
1. Отправка сообщения "⏳ Обрабатываю ссылку..."
2. Отправка preview с thumbnail и опциями качества:
   - 🎵 Аудио 320kbps / 192kbps
   - 📹 Видео 1080p / 720p / 480p
3. Удаление временного сообщения

**URL**: `https://www.youtube.com/watch?v=dQw4w9WgXcQ`
**Видео**: Rick Astley - Never Gonna Give You Up
**Тест**: `test_youtube_processing_flow`

---

### audio_download_complete
```
Type: Flow (Multi-step)
API Calls: 5
Methods: editMessageCaption (x3) → sendAudio → deleteMessage
```
**Сценарий**:
1. Обновление прогресса: 0%
2. Обновление прогресса: 45%
3. Обновление прогресса: 100%
4. Отправка аудио файла:
   - Performer: Rick Astley
   - Title: Never Gonna Give You Up
   - Duration: 3:33 (213 сек)
   - Bitrate: 192 kbps
   - Size: 5 MB
5. Удаление сообщения с прогрессом

**Тест**: `test_audio_download_complete_flow`

---

### rate_limit_error
```
Type: Error
API Calls: 1
Methods: sendMessage
```
**Сценарий**: Пользователь превышает лимит запросов
**Ответ**:
- Сообщение об ошибке
- Оставшееся время: 45 секунд
- Предложение оформить подписку (/plan)

**Error Type**: rate_limit
**Тест**: `test_rate_limit_error_snapshot`

---

## 🎯 Покрытие API методов

| Метод API | Snapshots | Использований |
|-----------|-----------|---------------|
| sendMessage | start_command, info_command, settings_menu, language_selection, youtube_processing, rate_limit_error | 6 |
| sendPhoto | youtube_processing | 1 |
| sendAudio | audio_download_complete | 1 |
| deleteMessage | youtube_processing, audio_download_complete | 2 |
| editMessageCaption | audio_download_complete | 3 |
| editMessageText | language_selection | 1 |
| answerCallbackQuery | language_selection | 1 |

**Итого**: 7 различных API методов

## 🧪 Как использовать

### Загрузить snapshot
```rust
let snapshot = TelegramSnapshot::load_by_name("youtube_processing")?;
```

### Создать mock сервер
```rust
let mock = TelegramMock::from_snapshot("youtube_processing").await?;
let bot = mock.create_bot()?;
```

### Проверить структуру
```rust
assert_eq!(snapshot.interactions.len(), 3);
let (call, response) = &snapshot.interactions[0];
assert_eq!(call.path, "/sendMessage");
```

## 📝 Создание новых snapshots

### Рекомендуемые сценарии для добавления:

1. **video_download_complete.json** - Полный цикл скачивания видео
2. **settings_change_quality.json** - Изменение качества видео
3. **downloads_list.json** - Просмотр истории загрузок
4. **cuts_menu.json** - Меню вырезок
5. **invalid_url_error.json** - Ошибка при неверном URL
6. **subscription_info.json** - Информация о подписке
7. **admin_commands.json** - Админские команды

### Команда для создания
```bash
./tools/log_to_snapshot.py --interactive
```

## 🔗 См. также

- [Полная документация](../../docs/SNAPSHOT_TESTING.md)
- [Быстрый старт](../../docs/SNAPSHOT_TESTING_QUICKSTART.md)
- [Примеры тестов](../bot_commands_test.rs)
