/// Экранирует специальные символы в имени файла для безопасного использования.
///
/// Заменяет проблемные символы для предотвращения проблем с путями файлов
/// и совместимости с различными файловыми системами.
///
/// Заменяемые символы:
/// - `/` -> `_` (разделитель путей Unix)
/// - `\` -> `_` (разделитель путей Windows)
/// - `:` -> `_` (зарезервирован в Windows)
/// - `*` -> `_` (wildcard)
/// - `?` -> `_` (wildcard)
/// - `"` -> `'` (кавычки)
/// - `<` -> `_` (перенаправление)
/// - `>` -> `_` (перенаправление)
/// - `|` -> `_` (pipe)
/// - Управляющие символы (0x00-0x1F) -> `_`
///
/// # Arguments
///
/// * `filename` - Исходное имя файла
///
/// # Returns
///
/// Безопасное имя файла с экранированными символами.
///
/// # Example
///
/// ```
/// use doradura::core::utils::escape_filename;
///
/// let safe = escape_filename("song/name*.mp3");
/// assert_eq!(safe, "song_name_.mp3");
/// ```
/// Заменяет пробелы на подчеркивания в имени файла.
///
/// Универсальный метод для нормализации имен файлов, заменяющий все пробелы на подчеркивания.
///
/// # Arguments
///
/// * `filename` - Исходное имя файла
///
/// # Returns
///
/// Имя файла с пробелами, замененными на подчеркивания.
///
/// # Example
///
/// ```
/// use doradura::core::utils::sanitize_filename;
///
/// assert_eq!(sanitize_filename("song name.mp3"), "song_name.mp3");
/// assert_eq!(sanitize_filename("Artist - Title.mp4"), "Artist_-_Title.mp4");
/// ```
pub fn sanitize_filename(filename: &str) -> String {
    filename.replace(' ', "_")
}

pub fn escape_filename(filename: &str) -> String {
    let mut result = String::with_capacity(filename.len());

    for c in filename.chars() {
        match c {
            // Разделители путей
            '/' | '\\' => result.push('_'),
            // Зарезервированные символы Windows
            ':' | '*' | '?' | '<' | '>' | '|' => result.push('_'),
            // Кавычки заменяем на одинарные
            '"' => result.push('\''),
            // Управляющие символы
            c if c.is_control() => result.push('_'),
            // Остальные символы оставляем как есть
            _ => result.push(c),
        }
    }

    // Убираем начальные и конечные пробелы и точки (проблемно в Windows)
    let result = result.trim_matches(|c: char| c.is_whitespace() || c == '.');

    // Если результат пустой, возвращаем безопасное имя по умолчанию
    if result.is_empty() {
        "unnamed".to_string()
    } else {
        result.to_string()
    }
}

/// Экранирует специальные символы для MarkdownV2 формата Telegram.
///
/// В Telegram MarkdownV2 требуется экранировать следующие символы:
/// `_`, `*`, `[`, `]`, `(`, `)`, `~`, `` ` ``, `>`, `#`, `+`, `-`, `=`, `|`, `{`, `}`, `.`, `!`
///
/// Важно: обратный слеш должен экранироваться первым, чтобы избежать повторного экранирования.
///
/// # Arguments
///
/// * `text` - Исходный текст
///
/// # Returns
///
/// Текст с экранированными специальными символами для MarkdownV2.
///
/// # Example
///
/// ```
/// use doradura::core::utils::escape_markdown_v2;
///
/// let escaped = escape_markdown_v2("Hello. World!");
/// assert_eq!(escaped, "Hello\\. World\\!");
/// ```
pub fn escape_markdown_v2(text: &str) -> String {
    let mut result = String::with_capacity(text.len() * 2);

    for c in text.chars() {
        match c {
            '\\' => result.push_str("\\\\"),
            '_' => result.push_str("\\_"),
            '*' => result.push_str("\\*"),
            '[' => result.push_str("\\["),
            ']' => result.push_str("\\]"),
            '(' => result.push_str("\\("),
            ')' => result.push_str("\\)"),
            '~' => result.push_str("\\~"),
            '`' => result.push_str("\\`"),
            '>' => result.push_str("\\>"),
            '#' => result.push_str("\\#"),
            '+' => result.push_str("\\+"),
            '-' => result.push_str("\\-"),
            '=' => result.push_str("\\="),
            '|' => result.push_str("\\|"),
            '{' => result.push_str("\\{"),
            '}' => result.push_str("\\}"),
            '.' => result.push_str("\\."),
            '!' => result.push_str("\\!"),
            _ => result.push(c),
        }
    }

    result
}

/// Форматирует caption для видео/аудио с использованием MarkdownV2.
///
/// Создаёт красиво отформатированный caption с жирным автором и курсивным названием.
/// Формат:
/// - Если есть автор: **Автор** — _Название_
/// - Если автора нет: _Название_
///
/// # Arguments
///
/// * `title` - Название композиции/видео
/// * `artist` - Автор (опционально)
///
/// # Returns
///
/// Отформатированный caption с экранированными символами для MarkdownV2.
///
/// # Example
///
/// ```
/// use doradura::core::utils::format_media_caption;
///
/// let caption = format_media_caption("Song Name", "Artist");
/// // Результат: *Artist* — _Song Name_
/// ```
pub fn format_media_caption(title: &str, artist: &str) -> String {
    if artist.trim().is_empty() {
        // Только название (курсив)
        format!("_{}_", escape_markdown_v2(title))
    } else {
        // Автор (жирный) — Название (курсив)
        format!(
            "*{}* — _{}_",
            escape_markdown_v2(artist),
            escape_markdown_v2(title)
        )
    }
}

/// Возвращает правильную форму слова "секунд" для русского языка.
///
/// Правила склонения:
/// - 1, 21, 31, ... -> "секунду" (винительный падеж единственного числа)
/// - 2-4, 22-24, 32-34, ... -> "секунды" (именительный падеж множественного числа)
/// - 5-20, 25-30, 35-40, ... -> "секунд" (родительный падеж множественного числа)
///
/// # Arguments
///
/// * `n` - Число секунд
///
/// # Returns
///
/// Правильную форму слова "секунд" в зависимости от числа.
///
/// # Example
///
/// ```
/// use doradura::core::utils::pluralize_seconds;
///
/// assert_eq!(pluralize_seconds(1), "секунду");
/// assert_eq!(pluralize_seconds(5), "секунд");
/// assert_eq!(pluralize_seconds(2), "секунды");
/// ```
pub fn pluralize_seconds(n: u64) -> &'static str {
    let n_mod_100 = n % 100;
    let n_mod_10 = n % 10;

    // Исключения: 11, 12, 13, 14 - всегда "секунд"
    if (11..=14).contains(&n_mod_100) {
        return "секунд";
    }

    // Остальные случаи зависят от последней цифры
    match n_mod_10 {
        1 => "секунду",
        2..=4 => "секунды",
        _ => "секунд",
    }
}

#[cfg(test)]
mod tests {
    use super::{
        escape_filename, escape_markdown_v2, format_media_caption, pluralize_seconds,
        sanitize_filename,
    };

    #[test]
    fn test_escape_filename() {
        // Базовые тесты на замену разделителей путей
        assert_eq!(escape_filename("song/name.mp3"), "song_name.mp3");
        assert_eq!(escape_filename("path\\to\\file.mp4"), "path_to_file.mp4");

        // Зарезервированные символы Windows
        assert_eq!(escape_filename("file:name*.mp3"), "file_name_.mp3");
        assert_eq!(escape_filename("title?<>|.mp4"), "title____.mp4");

        // Кавычки
        assert_eq!(escape_filename("song \"live\".mp3"), "song 'live'.mp3");

        // Начальные и конечные пробелы и точки
        assert_eq!(escape_filename("  file.mp3  "), "file.mp3");
        assert_eq!(escape_filename("...file..."), "file");

        // Пустое имя
        assert_eq!(escape_filename(""), "unnamed");
        assert_eq!(escape_filename("..."), "unnamed");
        assert_eq!(escape_filename("   "), "unnamed");

        // Кириллица и специальные символы
        assert_eq!(
            escape_filename("Дорадура - трек.mp3"),
            "Дорадура - трек.mp3"
        );
        assert_eq!(
            escape_filename("Song (live) [2024].mp3"),
            "Song (live) [2024].mp3"
        );
    }

    #[test]
    fn test_escape_markdown_v2() {
        // Тест на точки и восклицательные знаки
        assert_eq!(escape_markdown_v2("Hello. World!"), "Hello\\. World\\!");
        assert_eq!(escape_markdown_v2("file.mp3"), "file\\.mp3");

        // Тест на скобки и дефисы
        assert_eq!(
            escape_markdown_v2("Song (live).mp3"),
            "Song \\(live\\)\\.mp3"
        );
        assert_eq!(escape_markdown_v2("track-name"), "track\\-name");

        // Тест на обратный слеш
        assert_eq!(escape_markdown_v2("path\\file"), "path\\\\file");

        // Тест на сложные строки
        assert_eq!(
            escape_markdown_v2("NA - дора — Дорадура (акустическая версия).mp3"),
            "NA \\- дора — Дорадура \\(акустическая версия\\)\\.mp3"
        );
    }

    #[test]
    fn test_pluralize_seconds() {
        // Единственное число
        assert_eq!(pluralize_seconds(1), "секунду");
        assert_eq!(pluralize_seconds(21), "секунду");
        assert_eq!(pluralize_seconds(31), "секунду");
        assert_eq!(pluralize_seconds(101), "секунду");

        // Множественное число (2-4)
        assert_eq!(pluralize_seconds(2), "секунды");
        assert_eq!(pluralize_seconds(3), "секунды");
        assert_eq!(pluralize_seconds(4), "секунды");
        assert_eq!(pluralize_seconds(22), "секунды");
        assert_eq!(pluralize_seconds(23), "секунды");
        assert_eq!(pluralize_seconds(24), "секунды");
        assert_eq!(pluralize_seconds(32), "секунды");

        // Множественное число (5-20, 25-30, ...)
        assert_eq!(pluralize_seconds(5), "секунд");
        assert_eq!(pluralize_seconds(10), "секунд");
        assert_eq!(pluralize_seconds(15), "секунд");
        assert_eq!(pluralize_seconds(20), "секунд");
        assert_eq!(pluralize_seconds(25), "секунд");
        assert_eq!(pluralize_seconds(30), "секунд");

        // Исключения (11-14)
        assert_eq!(pluralize_seconds(11), "секунд");
        assert_eq!(pluralize_seconds(12), "секунд");
        assert_eq!(pluralize_seconds(13), "секунд");
        assert_eq!(pluralize_seconds(14), "секунд");
        assert_eq!(pluralize_seconds(111), "секунд");
        assert_eq!(pluralize_seconds(112), "секунд");

        // Пример из запроса пользователя
        assert_eq!(pluralize_seconds(71), "секунду");
    }

    #[test]
    fn test_sanitize_filename() {
        // Базовые тесты на замену пробелов
        assert_eq!(sanitize_filename("song name.mp3"), "song_name.mp3");
        assert_eq!(
            sanitize_filename("Artist - Title.mp4"),
            "Artist_-_Title.mp4"
        );
        assert_eq!(
            sanitize_filename("multiple   spaces.mp3"),
            "multiple___spaces.mp3"
        );

        // Тесты с кириллицей
        assert_eq!(
            sanitize_filename("Дорадура - трек.mp3"),
            "Дорадура_-_трек.mp3"
        );

        // Тесты с уже существующими подчеркиваниями
        assert_eq!(sanitize_filename("song_name.mp3"), "song_name.mp3");
        assert_eq!(sanitize_filename("song _ name.mp3"), "song___name.mp3");

        // Тесты с пустыми строками
        assert_eq!(sanitize_filename(""), "");
        assert_eq!(sanitize_filename("   "), "___");
    }

    #[test]
    fn test_format_media_caption() {
        // С автором
        assert_eq!(
            format_media_caption("Song Name", "Artist"),
            "*Artist* — _Song Name_"
        );

        // Без автора (пустая строка)
        assert_eq!(format_media_caption("Song Name", ""), "_Song Name_");

        // Без автора (только пробелы)
        assert_eq!(format_media_caption("Song Name", "   "), "_Song Name_");

        // С кириллицей
        assert_eq!(format_media_caption("Дорадура", "NA"), "*NA* — _Дорадура_");

        // Со специальными символами, требующими экранирования
        assert_eq!(
            format_media_caption("Song (live).mp3", "Artist-Name"),
            "*Artist\\-Name* — _Song \\(live\\)\\.mp3_"
        );

        // Сложный пример
        assert_eq!(
            format_media_caption("Дорадура (акустическая версия).mp3", "NA - дора"),
            "*NA \\- дора* — _Дорадура \\(акустическая версия\\)\\.mp3_"
        );
    }
}
