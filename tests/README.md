# Интеграционные тесты

## Быстрый старт

### 1. Диагностика системы (запустите первым!)

```bash
./test_ytdlp.sh diagnostics
```

Покажет статус системы, что установлено, что не хватает.

### 2. Настройка cookies (если нужно)

Если диагностика показала `❌ Cookies не настроены`:

```bash
# См. подробную инструкцию
cat ../QUICK_FIX.md
```

### 3. Тест скачивания

```bash
# Полный тест скачивания (требует интернет)
./test_ytdlp.sh download
```

## Доступные тесты

| Команда | Описание | Интернет |
|---------|----------|----------|
| `./test_ytdlp.sh diagnostics` | Полная диагностика системы | ❌ |
| `./test_ytdlp.sh install` | Проверка установки yt-dlp/ffmpeg | ❌ |
| `./test_ytdlp.sh version` | Проверка версии yt-dlp | ❌ |
| `./test_ytdlp.sh cookies` | Проверка конфигурации cookies | ❌ |
| `./test_ytdlp.sh metadata` | Получение метаданных видео | ✅ |
| `./test_ytdlp.sh download` | Тест скачивания аудио | ✅ |
| `./test_ytdlp.sh invalid` | Тест обработки ошибок | ✅ |
| `./test_ytdlp.sh quality` | Тест разных качеств | ✅ |
| `./test_ytdlp.sh all-basic` | Все тесты без интернета | ❌ |
| `./test_ytdlp.sh all-download` | Все тесты со скачиванием | ✅ |
| `./test_ytdlp.sh all` | ВСЕ тесты | ✅ |

## Альтернативный запуск через cargo

```bash
# Один тест
cargo test --test ytdlp_integration_test test_full_diagnostics -- --nocapture

# Все тесты кроме ignored
cargo test --test ytdlp_integration_test -- --nocapture --test-threads=1

# Все включая ignored (со скачиванием)
cargo test --test ytdlp_integration_test --ignored -- --nocapture --test-threads=1
```

## Структура тестов

- `ytdlp_integration_test.rs` - Основные интеграционные тесты
- `download_video.rs` - Тест скачивания видео (старый)

## Документация

- `../TESTING.md` - Полное руководство по тестированию
- `../QUICK_FIX.md` - Быстрое решение проблем со скачиванием
- `../MACOS_COOKIES_FIX.md` - Настройка cookies на macOS

## Troubleshooting

### Тест зависает

```bash
# Убедитесь что используется один поток
./test_ytdlp.sh download

# Или через cargo:
cargo test --test ytdlp_integration_test -- --test-threads=1
```

### "yt-dlp не найден"

```bash
# Установите
pip3 install yt-dlp

# Проверьте
which yt-dlp
```

### "Cookies не настроены"

```bash
# Следуйте инструкции
cat ../QUICK_FIX.md
```

### Ошибка "Please sign in"

YouTube требует аутентификацию. Настройте cookies:

```bash
export YTDL_COOKIES_FILE=./youtube_cookies.txt
./test_ytdlp.sh cookies  # Проверка
```

## Для разработчиков

### Добавление нового теста

Откройте `ytdlp_integration_test.rs` и добавьте:

```rust
#[test]
#[ignore] // Если требует интернет
fn test_my_new_feature() {
    // ... код теста
}
```

### Запуск конкретного теста

```bash
cargo test --test ytdlp_integration_test test_my_new_feature --ignored -- --nocapture
```

### Добавление теста в скрипт

Откройте `../test_ytdlp.sh` и добавьте case:

```bash
"mytest")
    run_test "test_my_new_feature" "--ignored"
    ;;
```

