# Исправление проблемы с именами файлов

## Проблема
При скачивании видео оно получало имя "Unknown Track.mp4", что указывало на проблемы с получением метаданных от yt-dlp.

## Решение

### 1. Улучшено получение метаданных (`src/downloader.rs`)

**Изменения в функции `get_metadata_from_ytdlp`:**

- Заменён `--get-title` на более надёжный `--print "%(title)s"`
- Добавлен флаг `--skip-download` для ускорения получения метаданных
- Вместо fallback на "Unknown Track" теперь возвращается ошибка с понятным сообщением
- Добавлена проверка на пустое название
- Улучшены сообщения об ошибках для упрощения диагностики

**До:**
```rust
let title = if title_output.status.success() {
    String::from_utf8_lossy(&title_output.stdout).trim().to_string()
} else {
    log::warn!("yt-dlp returned non-zero status, using default title");
    "Unknown Track".to_string()
};
```

**После:**
```rust
if !title_output.status.success() {
    let stderr = String::from_utf8_lossy(&title_output.stderr);
    log::error!("yt-dlp failed to get metadata, stderr: {}", stderr);
    return Err(AppError::Download(format!(
        "Failed to get video metadata. Please check if video is available and cookies are configured."
    )));
}

let title = String::from_utf8_lossy(&title_output.stdout).trim().to_string();

if title.is_empty() {
    return Err(AppError::Download(format!(
        "Failed to get video title. Video might be unavailable or private."
    )));
}
```

### 2. Улучшена функция экранирования имён файлов (`src/utils.rs`)

**Новые возможности:**

- Обрабатывает разделители путей: `/` и `\`
- Заменяет зарезервированные символы Windows: `:`, `*`, `?`, `<`, `>`, `|`
- Заменяет двойные кавычки на одинарные
- Удаляет управляющие символы (0x00-0x1F)
- Убирает начальные и конечные пробелы и точки
- Возвращает "unnamed" если результат пустой

**Обработка специальных символов:**

| Символ | Замена | Причина |
|--------|--------|---------|
| `/` | `_` | Разделитель путей Unix |
| `\` | `_` | Разделитель путей Windows |
| `:` | `_` | Зарезервирован в Windows |
| `*`, `?` | `_` | Wildcard символы |
| `<`, `>` | `_` | Символы перенаправления |
| `|` | `_` | Pipe символ |
| `"` | `'` | Кавычки |
| Управляющие символы | `_` | Могут вызывать проблемы |

### 3. Полный путь при отправке файлов

Убедились, что при отправке файлов используется правильный полный путь:

```rust
let download_path = shellexpand::tilde(&full_path).into_owned();
```

Путь преобразуется из `~/downloads/filename.mp4` в `/Users/username/downloads/filename.mp4`, что гарантирует корректную отправку файлов через Telegram API.

## Тестирование

Добавлены тесты для проверки корректности экранирования имён файлов:

```rust
#[test]
fn test_escape_filename() {
    // Разделители путей
    assert_eq!(escape_filename("song/name.mp3"), "song_name.mp3");
    assert_eq!(escape_filename("path\\to\\file.mp4"), "path_to_file.mp4");
    
    // Зарезервированные символы Windows
    assert_eq!(escape_filename("file:name*.mp3"), "file_name_.mp3");
    
    // Кириллица сохраняется
    assert_eq!(escape_filename("Дорадура - трек.mp3"), "Дорадура - трек.mp3");
}
```

## Результат

Теперь:
1. ✅ Видео скачиваются с правильными названиями, полученными от yt-dlp
2. ✅ Специальные символы в названиях корректно обрабатываются
3. ✅ Полный путь к файлу учитывается при отправке
4. ✅ При ошибках получения метаданных пользователь видит понятное сообщение вместо "Unknown Track"

## Примеры

**Было:**
- "Unknown Track.mp4" - для всех видео с ошибками получения метаданных

**Стало:**
- "How to Code in Rust - Tutorial.mp4" - корректное название
- "Дорадура - Новый трек (2024).mp4" - кириллица и спецсимволы обрабатываются
- Понятное сообщение об ошибке если видео недоступно

