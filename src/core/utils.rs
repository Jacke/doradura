use once_cell::sync::Lazy;
use regex::Regex;

// =============================================================================
// Lazy-compiled Regex patterns for performance
// =============================================================================

/// Regex for extracting retry-after seconds from error messages (format: "retry after N s")
pub static RETRY_AFTER_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)retry\s+after\s+(\d+)\s*s").expect("Invalid RETRY_AFTER_REGEX"));

/// Alternative regex for retry_after (format: "retry_after: N" or "retry_after N")
pub static RETRY_AFTER_ALT_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)retry_after[:\s]+(\d+)").expect("Invalid RETRY_AFTER_ALT_REGEX"));

/// Regex for parsing Bot API log entries (query start)
pub static BOT_API_START_REGEX: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\[(\d+\.\d+)\].*Query (0x[0-9a-f]+): .*method:\s*([a-z_]+).*\[name:([^]]+)\]\[size:(\d+)\]")
        .expect("Invalid BOT_API_START_REGEX")
});

/// Regex for parsing Bot API log entries (query start without name/size - for admin)
pub static BOT_API_START_SIMPLE_REGEX: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\[(\d+\.\d+)\].*Query (0x[0-9a-f]+): .*method:\s*([a-z_]+).*\[size:(\d+)\]")
        .expect("Invalid BOT_API_START_SIMPLE_REGEX")
});

/// Regex for parsing Bot API log entries (query response)
pub static BOT_API_RESPONSE_REGEX: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\[(\d+\.\d+)\].*Query (0x[0-9a-f]+): \[method:([a-z_]+)\]").expect("Invalid BOT_API_RESPONSE_REGEX")
});

// =============================================================================
// Utility functions using the lazy regexes
// =============================================================================

/// Extract retry-after seconds from an error message.
///
/// Looks for patterns like:
/// - "retry after 30 s"
/// - "retry_after: 30"
/// - "retry_after 30"
///
/// Returns the number of seconds to wait, or None if not found.
pub fn extract_retry_after(error_str: &str) -> Option<u64> {
    // Try the first pattern: "retry after N s"
    if let Some(caps) = RETRY_AFTER_REGEX.captures(error_str) {
        if let Some(secs) = caps.get(1) {
            return secs.as_str().parse().ok();
        }
    }

    // Try the alternative pattern: "retry_after: N" or "retry_after N"
    if let Some(caps) = RETRY_AFTER_ALT_REGEX.captures(error_str) {
        if let Some(secs) = caps.get(1) {
            return secs.as_str().parse().ok();
        }
    }

    None
}

/// Check if an error is a timeout or network error that should be retried.
pub fn is_timeout_or_network_error(error_str: &str) -> bool {
    let lower = error_str.to_lowercase();
    lower.contains("timed out")
        || lower.contains("timeout")
        || lower.contains("connection reset")
        || lower.contains("connection refused")
        || lower.contains("network is unreachable")
        || lower.contains("network error")
        || lower.contains("error sending request")
        || lower.contains("broken pipe")
}

/// Truncate string from the beginning (keeping the tail) to fit within max_bytes.
/// Ensures valid UTF-8 boundaries and adds ellipsis prefix if truncated.
pub fn truncate_tail_utf8(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }

    // Need to leave room for "…\n" (ellipsis + newline = 4 bytes for … in UTF-8 + 1 for \n)
    let prefix = "…\n";
    let prefix_len = prefix.len(); // 4 bytes for UTF-8 ellipsis + 1 for newline
    let target_bytes = max_bytes.saturating_sub(prefix_len);
    let skip_bytes = text.len() - target_bytes;

    // Find the next valid UTF-8 boundary after skip_bytes
    let mut start_idx = skip_bytes;
    while start_idx < text.len() && !text.is_char_boundary(start_idx) {
        start_idx += 1;
    }

    format!("{}{}", prefix, &text[start_idx..])
}

/// Truncate string from the end to fit within max_len characters.
/// Adds ellipsis suffix if truncated.
pub fn truncate_string_safe(text: &str, max_len: usize) -> String {
    if text.chars().count() <= max_len {
        return text.to_string();
    }
    let truncated: String = text.chars().take(max_len.saturating_sub(3)).collect();
    format!("{}...", truncated)
}

/// Telegram message character limit (4096 minus safety margin)
pub const TELEGRAM_MESSAGE_LIMIT: usize = 4000;

/// Truncate text for Telegram messages (max 4000 chars)
pub fn truncate_for_telegram(text: &str) -> String {
    truncate_string_safe(text, TELEGRAM_MESSAGE_LIMIT)
}

// =============================================================================
// Filename utilities
// =============================================================================

/// Санитизирует имя файла для безопасного использования с ffmpeg и yt-dlp.
///
/// СТРОГАЯ ASCII-ONLY санитизация для предотвращения проблем с постобработкой.
/// Non-ASCII символы и специальные символы могут вызывать сбои FixupM3u8 и других постпроцессоров.
///
/// Поведение:
/// - ASCII буквы, цифры, `_`, `-`, `.` - сохраняются
/// - Пробелы -> `_`
/// - Латинские акцентированные символы (á, é, ñ и т.д.) -> ASCII эквивалент
/// - Кириллица -> транслитерация
/// - Все остальные символы (запятые, скобки, кавычки и т.д.) -> `_`
/// - Множественные `_` -> один `_`
/// - Ограничение длины: 200 символов
///
/// # Arguments
///
/// * `filename` - Исходное имя файла
///
/// # Returns
///
/// Безопасное ASCII-only имя файла.
///
/// # Example
///
/// ```
/// use doradura::core::utils::escape_filename;
///
/// // Специальные символы -> underscore
/// assert_eq!(escape_filename("song/name*.mp3"), "song_name.mp3");
/// // Акценты -> ASCII
/// assert_eq!(escape_filename("Nacho Barón.mp4"), "Nacho_Baron.mp4");
/// // Кириллица -> транслит
/// assert_eq!(escape_filename("Привет.mp3"), "Privet.mp3");
/// // Запятые -> underscore (collapsed)
/// assert_eq!(escape_filename("A, B, C.mp4"), "A_B_C.mp4");
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
    // STRICT ASCII-ONLY sanitization to prevent ffmpeg/postprocessing issues
    // Non-ASCII characters can cause FixupM3u8 and other postprocessors to fail
    let mut result = String::with_capacity(filename.len());

    for c in filename.chars() {
        match c {
            // Safe ASCII: letters, digits, underscore, hyphen, dot (for extension)
            'a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '-' | '.' => result.push(c),
            // Space becomes underscore
            ' ' => result.push('_'),
            // Common transliterations for accented characters
            'á' | 'à' | 'â' | 'ä' | 'ã' | 'å' => result.push('a'),
            'Á' | 'À' | 'Â' | 'Ä' | 'Ã' | 'Å' => result.push('A'),
            'é' | 'è' | 'ê' | 'ë' => result.push('e'),
            'É' | 'È' | 'Ê' | 'Ë' => result.push('E'),
            'í' | 'ì' | 'î' | 'ï' => result.push('i'),
            'Í' | 'Ì' | 'Î' | 'Ï' => result.push('I'),
            'ó' | 'ò' | 'ô' | 'ö' | 'õ' => result.push('o'),
            'Ó' | 'Ò' | 'Ô' | 'Ö' | 'Õ' => result.push('O'),
            'ú' | 'ù' | 'û' | 'ü' => result.push('u'),
            'Ú' | 'Ù' | 'Û' | 'Ü' => result.push('U'),
            'ñ' => result.push('n'),
            'Ñ' => result.push('N'),
            'ç' => result.push('c'),
            'Ç' => result.push('C'),
            'ß' => result.push_str("ss"),
            // Cyrillic transliteration (common in Russian music)
            'а' => result.push('a'),
            'б' => result.push('b'),
            'в' => result.push('v'),
            'г' => result.push('g'),
            'д' => result.push('d'),
            'е' | 'ё' => result.push('e'),
            'ж' => result.push_str("zh"),
            'з' => result.push('z'),
            'и' | 'й' => result.push('i'),
            'к' => result.push('k'),
            'л' => result.push('l'),
            'м' => result.push('m'),
            'н' => result.push('n'),
            'о' => result.push('o'),
            'п' => result.push('p'),
            'р' => result.push('r'),
            'с' => result.push('s'),
            'т' => result.push('t'),
            'у' => result.push('u'),
            'ф' => result.push('f'),
            'х' => result.push('h'),
            'ц' => result.push_str("ts"),
            'ч' => result.push_str("ch"),
            'ш' => result.push_str("sh"),
            'щ' => result.push_str("sch"),
            'ъ' | 'ь' => {} // Skip hard/soft sign
            'ы' => result.push('y'),
            'э' => result.push('e'),
            'ю' => result.push_str("yu"),
            'я' => result.push_str("ya"),
            'А' => result.push('A'),
            'Б' => result.push('B'),
            'В' => result.push('V'),
            'Г' => result.push('G'),
            'Д' => result.push('D'),
            'Е' | 'Ё' => result.push('E'),
            'Ж' => result.push_str("Zh"),
            'З' => result.push('Z'),
            'И' | 'Й' => result.push('I'),
            'К' => result.push('K'),
            'Л' => result.push('L'),
            'М' => result.push('M'),
            'Н' => result.push('N'),
            'О' => result.push('O'),
            'П' => result.push('P'),
            'Р' => result.push('R'),
            'С' => result.push('S'),
            'Т' => result.push('T'),
            'У' => result.push('U'),
            'Ф' => result.push('F'),
            'Х' => result.push('H'),
            'Ц' => result.push_str("Ts"),
            'Ч' => result.push_str("Ch"),
            'Ш' => result.push_str("Sh"),
            'Щ' => result.push_str("Sch"),
            'Ъ' | 'Ь' => {} // Skip hard/soft sign
            'Ы' => result.push('Y'),
            'Э' => result.push('E'),
            'Ю' => result.push_str("Yu"),
            'Я' => result.push_str("Ya"),
            // All other characters (commas, brackets, quotes, non-ASCII) become underscore
            _ => result.push('_'),
        }
    }

    // Collapse multiple underscores into one and remove underscore before dot
    let mut collapsed = String::with_capacity(result.len());
    let chars: Vec<char> = result.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '_' {
            // Skip consecutive underscores
            while i < chars.len() && chars[i] == '_' {
                i += 1;
            }
            // Don't add underscore if next char is a dot (extension separator)
            if i < chars.len() && chars[i] != '.' {
                collapsed.push('_');
            }
        } else {
            collapsed.push(c);
            i += 1;
        }
    }

    // Trim leading/trailing underscores and dots
    let trimmed = collapsed.trim_matches(|c: char| c == '_' || c == '.');

    // Limit filename length (leave room for extension)
    let max_len = 200;
    let final_name = if trimmed.len() > max_len {
        // Find a good break point
        let mut end = max_len;
        while end > 0 && !trimmed.is_char_boundary(end) {
            end -= 1;
        }
        &trimmed[..end]
    } else {
        trimmed
    };

    if final_name.is_empty() {
        "unnamed".to_string()
    } else {
        final_name.to_string()
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
    let base_caption = if artist.trim().is_empty() {
        // Только название (курсив)
        format!("_{}_", escape_markdown_v2(title))
    } else {
        // Автор (жирный) — Название (курсив)
        format!("*{}* — _{}_", escape_markdown_v2(artist), escape_markdown_v2(title))
    };

    // Add copyright signature
    crate::core::copyright::format_caption_with_copyright(&base_caption)
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
    use super::{escape_filename, escape_markdown_v2, format_media_caption, pluralize_seconds, sanitize_filename};

    #[test]
    fn test_escape_filename() {
        // Базовые тесты на замену разделителей путей
        assert_eq!(escape_filename("song/name.mp3"), "song_name.mp3");
        assert_eq!(escape_filename("path\\to\\file.mp4"), "path_to_file.mp4");

        // Зарезервированные символы Windows - все становятся _ и схлопываются
        assert_eq!(escape_filename("file:name*.mp3"), "file_name.mp3");
        assert_eq!(escape_filename("title?<>|.mp4"), "title.mp4"); // Multiple _ collapsed

        // Кавычки и скобки -> underscore (collapsed)
        assert_eq!(escape_filename("song \"live\".mp3"), "song_live.mp3");
        assert_eq!(escape_filename("Song (live) [2024].mp3"), "Song_live_2024.mp3");

        // Начальные и конечные пробелы и точки
        assert_eq!(escape_filename("  file.mp3  "), "file.mp3");
        assert_eq!(escape_filename("...file..."), "file");

        // Пустое имя
        assert_eq!(escape_filename(""), "unnamed");
        assert_eq!(escape_filename("..."), "unnamed");
        assert_eq!(escape_filename("   "), "unnamed");

        // Кириллица -> транслитерация (NEW BEHAVIOR!)
        assert_eq!(escape_filename("Дорадура - трек.mp3"), "Doradura_-_trek.mp3");

        // Акценты -> ASCII
        assert_eq!(escape_filename("Nacho Barón.mp4"), "Nacho_Baron.mp4");
        assert_eq!(escape_filename("Café.mp3"), "Cafe.mp3");

        // Запятые -> underscore (collapsed)
        assert_eq!(escape_filename("A, B, C.mp4"), "A_B_C.mp4");
        assert_eq!(
            escape_filename("JLLY, Flyy Armani - LUNA.mp4"),
            "JLLY_Flyy_Armani_-_LUNA.mp4"
        );
    }

    #[test]
    fn test_escape_markdown_v2() {
        // Тест на точки и восклицательные знаки
        assert_eq!(escape_markdown_v2("Hello. World!"), "Hello\\. World\\!");
        assert_eq!(escape_markdown_v2("file.mp3"), "file\\.mp3");

        // Тест на скобки и дефисы
        assert_eq!(escape_markdown_v2("Song (live).mp3"), "Song \\(live\\)\\.mp3");
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
        assert_eq!(sanitize_filename("Artist - Title.mp4"), "Artist_-_Title.mp4");
        assert_eq!(sanitize_filename("multiple   spaces.mp3"), "multiple___spaces.mp3");

        // Тесты с кириллицей
        assert_eq!(sanitize_filename("Дорадура - трек.mp3"), "Дорадура_-_трек.mp3");

        // Тесты с уже существующими подчеркиваниями
        assert_eq!(sanitize_filename("song_name.mp3"), "song_name.mp3");
        assert_eq!(sanitize_filename("song _ name.mp3"), "song___name.mp3");

        // Тесты с пустыми строками
        assert_eq!(sanitize_filename(""), "");
        assert_eq!(sanitize_filename("   "), "___");
    }

    #[test]
    fn test_format_media_caption() {
        // Note: format_media_caption now appends copyright signature
        // Tests check that caption starts with expected base part

        // С автором
        assert!(format_media_caption("Song Name", "Artist").starts_with("*Artist* — _Song Name_"));

        // Без автора (пустая строка)
        assert!(format_media_caption("Song Name", "").starts_with("_Song Name_"));

        // Без автора (только пробелы)
        assert!(format_media_caption("Song Name", "   ").starts_with("_Song Name_"));

        // С кириллицей
        assert!(format_media_caption("Дорадура", "NA").starts_with("*NA* — _Дорадура_"));

        // Со специальными символами, требующими экранирования
        assert!(format_media_caption("Song (live).mp3", "Artist-Name")
            .starts_with("*Artist\\-Name* — _Song \\(live\\)\\.mp3_"));

        // Сложный пример
        assert!(format_media_caption("Дорадура (акустическая версия).mp3", "NA - дора")
            .starts_with("*NA \\- дора* — _Дорадура \\(акустическая версия\\)\\.mp3_"));

        // Check copyright is appended
        let caption = format_media_caption("Test", "Artist");
        assert!(caption.contains("Ваш,"));
    }
}
