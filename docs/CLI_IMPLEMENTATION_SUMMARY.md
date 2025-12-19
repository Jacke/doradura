# CLI Implementation Summary

## Что Было Сделано

Реализована полноценная система CLI (Command Line Interface) для бота с поддержкой нескольких режимов работы и утилитных команд.

## Новые Файлы

### 1. `src/cli.rs` - CLI Structure

Определяет структуру CLI с использованием библиотеки `clap`:

```rust
pub enum Commands {
    Run { webhook: bool },
    RunStaging { webhook: bool },
    RunWithCookies { cookies: Option<String>, webhook: bool },
    RefreshMetadata { limit: Option<usize>, dry_run: bool, verbose: bool },
}
```

**Возможности:**
- Автоматическая генерация справки (`--help`)
- Типобезопасные аргументы
- Подкоманды с опциями
- Версия (`--version`)

### 2. `src/metadata_refresh.rs` - Metadata Refresh Utility

Утилита для обновления отсутствующих метаданных в таблице `download_history`.

**Функционал:**
- Находит записи с `file_id` но без метаданных
- Скачивает файл из Telegram
- Извлекает метаданные с помощью `ffprobe`:
  - `file_size` - размер файла
  - `duration` - длительность
  - `video_quality` - разрешение видео (для mp4)
  - `audio_bitrate` - битрейт аудио (для mp3)
- Обновляет базу данных
- Поддерживает `--dry-run` для безопасного тестирования
- Поддерживает `--limit` для ограничения количества обработанных записей
- Поддерживает `--verbose` для подробного вывода

**Основные функции:**
- `refresh_missing_metadata()` - главная функция
- `download_telegram_file()` - скачивание файла по file_id
- `extract_metadata()` - извлечение метаданных с ffprobe
- `update_metadata()` - обновление записи в БД

### 3. `CLI_USAGE.md` - Documentation

Полная документация по использованию CLI:
- Описание всех команд
- Примеры использования
- Troubleshooting
- Миграция со скриптов

## Изменённые Файлы

### 1. `Cargo.toml`

Добавлена зависимость `clap`:

```toml
clap = { version = "4.5", features = ["derive", "cargo"] }
```

### 2. `src/lib.rs`

Добавлены новые модули:

```rust
pub mod cli;
pub mod metadata_refresh;
```

### 3. `src/main.rs`

Полностью рефакторен для поддержки CLI:

**Изменения:**
- Добавлен парсинг аргументов командной строки
- Создана функция `run_bot(use_webhook: bool)` - весь код запуска бота вынесен сюда
- Создана функция `run_metadata_refresh()` - запуск утилиты обновления метаданных
- `main()` теперь диспетчер команд:
  ```rust
  match cli.command {
      Some(Commands::Run { webhook }) => run_bot(webhook).await,
      Some(Commands::RunStaging { webhook }) => { /* load .env.staging */ run_bot(webhook).await },
      Some(Commands::RunWithCookies { cookies, webhook }) => { /* set cookies */ run_bot(webhook).await },
      Some(Commands::RefreshMetadata { ... }) => run_metadata_refresh(...).await,
      None => run_bot(false).await,  // default
  }
  ```

**Поддержка webhook:**
- Добавлен параметр `use_webhook` в `run_bot()`
- Webhook включается только если параметр `true` И установлена `WEBHOOK_URL`

### 4. `README.md`

Добавлена ссылка на CLI документацию:

```markdown
> **💡 Note:** The bot now supports CLI commands. See [CLI_USAGE.md](CLI_USAGE.md)
> for all available commands including `run-staging`, `run-with-cookies`, and `refresh-metadata`.
```

## Доступные Команды

### 1. `doradura run [--webhook]`

Запускает бота в обычном режиме (использует `.env`).

**Примеры:**
```bash
./doradura run
./doradura run --webhook
```

### 2. `doradura run-staging [--webhook]`

Запускает бота в staging режиме (использует `.env.staging`).

**Примеры:**
```bash
./doradura run-staging
./doradura run-staging --webhook
```

### 3. `doradura run-with-cookies [--cookies PATH] [--webhook]`

Запускает бота с указанием пути к cookies файлу.

**Примеры:**
```bash
./doradura run-with-cookies
./doradura run-with-cookies --cookies /path/to/cookies.txt
./doradura run-with-cookies --cookies cookies.txt --webhook
```

### 4. `doradura refresh-metadata [OPTIONS]`

Обновляет отсутствующие метаданные в download_history.

**Опции:**
- `-l, --limit <N>` - Обработать только первые N записей
- `--dry-run` - Показать что будет обновлено, но не обновлять
- `-v, --verbose` - Подробный вывод

**Примеры:**
```bash
# Dry run
./doradura refresh-metadata --dry-run --verbose

# Обновить первые 10
./doradura refresh-metadata --limit 10

# Обновить все с подробным выводом
./doradura refresh-metadata --verbose

# Обновить все (тихо)
./doradura refresh-metadata
```

## Преимущества

### 1. Единая Точка Входа

**Было:**
- `run_staging.sh`
- `run_with_cookies.sh`
- Разные скрипты для разных задач

**Стало:**
```bash
./doradura <command>
```

### 2. Встроенная Документация

```bash
./doradura --help
./doradura refresh-metadata --help
```

### 3. Типобезопасность

Clap валидирует аргументы на этапе парсинга:
- `--limit` должен быть числом
- `--cookies` принимает строку
- Флаги (`--webhook`, `--dry-run`, `--verbose`) - булевы

### 4. Расширяемость

Легко добавить новые команды:

```rust
// В src/cli.rs
pub enum Commands {
    // ...
    Backup { output: Option<String> },
    Stats,
    Clean,
}

// В src/main.rs
match cli.command {
    // ...
    Some(Commands::Backup { output }) => run_backup(output).await,
    Some(Commands::Stats) => run_stats().await,
    Some(Commands::Clean) => run_clean().await,
}
```

## Use Cases

### Development

```bash
# Запуск в dev режиме
cargo run -- run

# Staging с другой базой данных
cargo run -- run-staging

# Тестирование метаданных
cargo run -- refresh-metadata --dry-run --limit 5
```

### Production

```bash
# Сборка
cargo build --release

# Запуск
./target/release/doradura run

# Systemd service
[Service]
ExecStart=/opt/doradura/doradura run
```

### Maintenance

```bash
# Обновление метаданных после миграции
./doradura refresh-metadata

# Запуск с новыми cookies
./doradura run-with-cookies --cookies fresh_cookies.txt
```

## Миграция

### До (Скрипты)

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

### После (CLI)

```bash
# Просто команды
./doradura run-staging
./doradura run-with-cookies --cookies /path/to/cookies.txt
```

## Тестирование

### Сборка

```bash
cargo build
# ✅ Successful compilation
```

### Запуск помощи

```bash
./target/debug/doradura --help
# ✅ Shows all commands

./target/debug/doradura refresh-metadata --help
# ✅ Shows refresh-metadata options
```

### Тестирование команд

```bash
# Run (по умолчанию)
./doradura
# ✅ Starts bot in default mode

# Refresh metadata (dry run)
./doradura refresh-metadata --dry-run
# ✅ Would show entries to refresh without making changes
```

## Зависимости

### Новые

- `clap = "4.5"` - CLI argument parsing

### Используемые в metadata_refresh

- `reqwest` - HTTP requests для скачивания файлов из Telegram (уже есть)
- `serde_json` - Парсинг JSON ответов от Telegram API (уже есть)
- `uuid` - Генерация уникальных имён временных файлов (уже есть)
- `ffprobe` - Системная утилита для извлечения метаданных (требует установки)

## Требования

### Runtime

- `ffprobe` должен быть установлен для `refresh-metadata`:
  ```bash
  # macOS
  brew install ffmpeg

  # Ubuntu/Debian
  sudo apt-get install ffmpeg
  ```

### Environment Variables

Все команды требуют `.env` файл с:
- `BOT_TOKEN` - для всех команд
- `WEBHOOK_URL` - только для `--webhook` режима
- Другие переменные из `config.rs`

## Roadmap

Планируемые команды:

1. `doradura backup [--output PATH]` - Создание резервной копии БД
2. `doradura stats` - Статистика использования
3. `doradura migrate` - Запуск миграций
4. `doradura clean` - Очистка временных файлов
5. `doradura export [--format csv|json]` - Экспорт данных
6. `doradura validate` - Проверка конфигурации

## Breaking Changes

### Для Railway/Docker

Нужно обновить команду запуска:

**Docker:**
```dockerfile
# Было
CMD ["./doradura"]

# Стало
CMD ["./doradura", "run"]
```

**Railway:**
```
Start Command: ./doradura run
```

### Для Systemd

```ini
[Service]
# Было
ExecStart=/opt/doradura/doradura

# Стало
ExecStart=/opt/doradura/doradura run
```

**Обратная совместимость:**
Запуск без аргументов (`./doradura`) всё ещё работает - запускает бота в режиме `run` по умолчанию.

## Файлы

### Созданы

1. `src/cli.rs` - CLI structure (59 строк)
2. `src/metadata_refresh.rs` - Metadata refresh utility (282 строки)
3. `CLI_USAGE.md` - Документация (400+ строк)
4. `CLI_IMPLEMENTATION_SUMMARY.md` - Этот файл

### Изменены

1. `Cargo.toml` - Добавлен clap
2. `src/lib.rs` - Экспорт новых модулей
3. `src/main.rs` - Рефакторинг для CLI (~100 строк изменений)
4. `README.md` - Ссылка на CLI документацию

## Итого

✅ **Реализовано:**
- CLI система с 4 командами
- Утилита обновления метаданных
- Поддержка staging окружения
- Поддержка cookies через аргументы
- Webhook toggle через флаг
- Полная документация

✅ **Качество:**
- Типобезопасные аргументы
- Встроенная справка
- Dry-run mode для безопасности
- Verbose mode для отладки
- Обратная совместимость

✅ **Готово к использованию:**
- Компилируется без ошибок
- Протестировано `--help`
- Документация готова
- Примеры использования

## Как Использовать

1. **Сборка:**
   ```bash
   cargo build --release
   ```

2. **Запуск бота:**
   ```bash
   ./target/release/doradura run
   ```

3. **Обновление метаданных:**
   ```bash
   ./target/release/doradura refresh-metadata --dry-run --verbose
   ```

4. **Справка:**
   ```bash
   ./target/release/doradura --help
   ```

Готово! 🎉
