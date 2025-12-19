# CLI Usage - Использование CLI

## Обзор

Бот теперь поддерживает CLI (Command Line Interface) с несколькими подкомандами для различных режимов работы.

## Установка и Сборка

```bash
cargo build --release
```

Исполняемый файл будет находиться в `target/release/doradura`.

## Доступные Команды

### 1. `run` - Запуск бота в обычном режиме

Запускает бота в стандартном режиме с использованием переменных окружения из `.env`.

```bash
# Long polling mode (по умолчанию)
./doradura run

# Webhook mode
./doradura run --webhook
```

**Без аргументов (по умолчанию):**
```bash
./doradura
# Эквивалентно: ./doradura run
```

### 2. `run-staging` - Запуск бота в staging окружении

Загружает переменные окружения из `.env.staging` вместо `.env`.

```bash
# Long polling mode
./doradura run-staging

# Webhook mode
./doradura run-staging --webhook
```

**Использование:**
- Создайте файл `.env.staging` с тестовыми настройками
- Запустите бота с этим файлом окружения
- Полезно для тестирования изменений без влияния на production

**Пример `.env.staging`:**
```env
BOT_TOKEN=your_test_bot_token
DATABASE_PATH=database_staging.sqlite
ADMIN_USER_ID=123456789
# ... остальные переменные
```

### 3. `run-with-cookies` - Запуск бота с cookies

Запускает бота с указанием пути к файлу cookies для YouTube.

```bash
# С автоопределением пути к cookies из переменных окружения
./doradura run-with-cookies

# С явным указанием пути к cookies
./doradura run-with-cookies --cookies /path/to/youtube_cookies.txt

# Webhook mode
./doradura run-with-cookies --cookies /path/to/cookies.txt --webhook
```

**Использование:**
- Указывает путь к файлу cookies для обхода ограничений YouTube
- Полезно при обновлении cookies или тестировании новых cookies
- Если `--cookies` не указан, используется значение из переменных окружения

### 4. `refresh-metadata` - Обновление метаданных в истории загрузок

Сканирует таблицу `download_history` и обновляет отсутствующие метаданные (file_size, duration, video_quality, audio_bitrate) для файлов, которые уже были успешно отправлены в Telegram.

```bash
# Обновить ВСЕ записи с отсутствующими метаданными
./doradura refresh-metadata

# Dry run - показать что будет обновлено, но не обновлять
./doradura refresh-metadata --dry-run

# Обновить только первые 10 записей
./doradura refresh-metadata --limit 10

# Подробный вывод (показывать каждую обработанную запись)
./doradura refresh-metadata --verbose

# Комбинация: dry run + verbose + limit
./doradura refresh-metadata --dry-run --verbose --limit 5
```

**Опции:**
- `--limit <N>` - Обработать только первые N записей (полезно для тестирования)
- `--dry-run` - Показать что будет обновлено, но НЕ вносить изменения в базу данных
- `--verbose` - Подробный вывод: показывать каждую обработанную запись

**Как это работает:**
1. Находит все записи в `download_history` с `file_id IS NOT NULL` и отсутствующими метаданными
2. Для каждой записи:
   - Скачивает файл из Telegram используя `file_id`
   - Извлекает метаданные с помощью `ffprobe`:
     - `file_size` - размер файла в байтах
     - `duration` - длительность в секундах
     - `video_quality` - разрешение видео (например, "1080p", "720p")
     - `audio_bitrate` - битрейт аудио (например, "320k", "192k")
   - Обновляет запись в базе данных
   - Удаляет временный файл
3. Выводит итоговую статистику

**Пример вывода:**
```
📊 Found 15 entries with missing metadata

[1/15] Processing: Rick Astley - Never Gonna Give You Up (format: mp3, file_id: AgAC...)
  Missing: file_size, duration, audio_bitrate
  ✅ Updated: Metadata { file_size: Some(3145728), duration: Some(213), audio_bitrate: Some("320k") }

[2/15] Processing: Example Video (format: mp4, file_id: BAADBAADAgI...)
  Missing: duration, video_quality
  ✅ Updated: Metadata { duration: Some(125), video_quality: Some("1080p") }

...

════════════════════════════════════════════════════════════
📊 Metadata Refresh Summary:
   • Total entries found: 15
   • Successfully updated: 13
   • Failed: 2
════════════════════════════════════════════════════════════
```

**Когда использовать:**
- После миграции с V9 на V10 (добавлены новые поля в download_history)
- Когда метаданные не были сохранены из-за ошибки
- Для заполнения истории старых загрузок

**Требования:**
- Установленный `ffprobe` (часть FFmpeg)
- Доступ к Telegram Bot API
- `BOT_TOKEN` в переменных окружения

## Переменные Окружения

Все команды используют переменные окружения из `.env` (или `.env.staging` для `run-staging`):

```env
# Required
BOT_TOKEN=your_telegram_bot_token

# Optional
BOT_API_URL=http://localhost:8081              # Локальный Bot API (опционально)
WEBHOOK_URL=https://yourdomain.com/webhook     # Для webhook mode
YOUTUBE_COOKIES_PATH=/path/to/cookies.txt      # Путь к cookies YouTube
DATABASE_PATH=database.sqlite                   # Путь к базе данных
ADMIN_USER_ID=123456789                        # ID администратора

# Metrics
METRICS_ENABLED=true
METRICS_PORT=9094

# Alerts
ALERTS_ENABLED=true

# Mini App
WEBAPP_PORT=8080

# ... и другие переменные из config.rs
```

## Миграция со Скриптов

### Было:

**run_staging.sh:**
```bash
#!/bin/bash
export $(cat .env.staging | xargs)
cargo run
```

**run_with_cookies.sh:**
```bash
#!/bin/bash
export YOUTUBE_COOKIES_PATH=/path/to/cookies.txt
cargo run
```

### Стало:

```bash
# Вместо run_staging.sh
./doradura run-staging

# Вместо run_with_cookies.sh
./doradura run-with-cookies --cookies /path/to/cookies.txt
```

**Преимущества:**
- ✅ Не нужны отдельные скрипты
- ✅ Единая точка входа
- ✅ Встроенная документация (`--help`)
- ✅ Типобезопасные аргументы
- ✅ Автодополнение команд (с shell completion)

## Примеры Использования

### Development

```bash
# Запуск в обычном режиме
cargo run -- run

# Запуск в staging
cargo run -- run-staging

# Обновление метаданных (dry run)
cargo run -- refresh-metadata --dry-run --verbose --limit 5
```

### Production

```bash
# Сборка release версии
cargo build --release

# Запуск бота
./target/release/doradura run

# Systemd service (пример)
[Service]
ExecStart=/path/to/doradura run
Restart=always
```

### Обновление Метаданных

```bash
# 1. Сначала dry run чтобы посмотреть что будет обновлено
./doradura refresh-metadata --dry-run --verbose

# 2. Обновить первые 10 для теста
./doradura refresh-metadata --limit 10 --verbose

# 3. Если всё ок, обновить все
./doradura refresh-metadata
```

## Docker

Если используется Docker, обновите `CMD` в `Dockerfile`:

```dockerfile
# Было
CMD ["./doradura"]

# Стало (явно указываем команду)
CMD ["./doradura", "run"]
```

Или используйте аргументы при запуске:

```bash
# Normal mode
docker run mybot run

# Staging mode
docker run mybot run-staging

# Refresh metadata
docker run mybot refresh-metadata --limit 100
```

## Railway Deployment

Обновите команду запуска в настройках Railway:

```bash
# Вместо: ./doradura
# Используйте: ./doradura run

# Или с webhook:
./doradura run --webhook
```

## Shell Completion (Опционально)

Clap поддерживает генерацию автодополнения для различных shell:

```bash
# Для bash
doradura --generate-completion bash > /etc/bash_completion.d/doradura

# Для zsh
doradura --generate-completion zsh > /usr/local/share/zsh/site-functions/_doradura

# Для fish
doradura --generate-completion fish > ~/.config/fish/completions/doradura.fish
```

(Требуется добавить `clap_complete` feature и код генерации)

## Troubleshooting

### "BOT_TOKEN environment variable not set"

Убедитесь что файл `.env` существует и содержит `BOT_TOKEN`:

```bash
# Проверка
cat .env | grep BOT_TOKEN

# Или запустите с явным указанием
BOT_TOKEN=your_token ./doradura run
```

### "Failed to create database pool"

Проверьте права доступа к файлу базы данных:

```bash
ls -la database.sqlite

# Если нужно
chmod 644 database.sqlite
```

### Ошибки при refresh-metadata

**"Failed to run ffprobe":**
```bash
# Установите ffmpeg
# macOS:
brew install ffmpeg

# Ubuntu/Debian:
sudo apt-get install ffmpeg

# Проверка
ffprobe -version
```

**"Failed to download file from Telegram":**
- Проверьте что `BOT_TOKEN` корректный
- Проверьте интернет соединение
- Проверьте что файл не был удалён из Telegram

## Roadmap

Планируемые дополнительные команды:

- `doradura backup` - Создать резервную копию базы данных
- `doradura stats` - Показать статистику использования
- `doradura migrate` - Запустить миграции базы данных
- `doradura clean` - Очистить временные файлы
- `doradura export` - Экспортировать данные в CSV/JSON

## См. также

- [README.md](README.md) - Основная документация
- [ERROR_METRICS_COMPREHENSIVE.md](ERROR_METRICS_COMPREHENSIVE.md) - Метрики ошибок
- [ANALYTICS_SYSTEM.md](ANALYTICS_SYSTEM.md) - Система аналитики
