/// Модуль для анализа ошибок yt-dlp
///
/// Предоставляет функции для определения типа ошибки yt-dlp
/// и генерации информативных сообщений для пользователя и администратора.
/// Типы ошибок yt-dlp
#[derive(Debug, Clone, PartialEq)]
pub enum YtDlpErrorType {
    /// Cookies недействительны или устарели
    InvalidCookies,
    /// YouTube обнаружил бота
    BotDetection,
    /// Видео недоступно (приватное, удалено, региональные ограничения)
    VideoUnavailable,
    /// Проблемы с сетью (таймауты, соединение)
    NetworkError,
    /// Неизвестная ошибка
    Unknown,
}

/// Анализирует stderr yt-dlp и определяет тип ошибки
///
/// # Параметры
/// - `stderr`: содержимое stderr от yt-dlp
///
/// # Возвращает
/// - `YtDlpErrorType`: тип определенной ошибки
pub fn analyze_ytdlp_error(stderr: &str) -> YtDlpErrorType {
    let stderr_lower = stderr.to_lowercase();

    // Проверяем ошибки связанные с cookies
    if stderr_lower.contains("cookies are no longer valid")
        || stderr_lower.contains("cookies have likely been rotated")
        || stderr_lower.contains("sign in to confirm you're not a bot")
        || stderr_lower.contains("please sign in")
        || stderr_lower.contains("use --cookies-from-browser")
        || stderr_lower.contains("use --cookies for the authentication")
        || stderr_lower.contains("the provided youtube account cookies are no longer valid")
    {
        return YtDlpErrorType::InvalidCookies;
    }

    // Проверяем bot detection
    if stderr_lower.contains("bot detection")
        || stderr_lower.contains("http error 403")
        || stderr_lower.contains("unable to extract")
        || stderr_lower.contains("signature extraction failed")
    {
        return YtDlpErrorType::BotDetection;
    }

    // Проверяем недоступное видео
    if stderr_lower.contains("private video")
        || stderr_lower.contains("video unavailable")
        || stderr_lower.contains("this video is not available")
        || stderr_lower.contains("video is private")
        || stderr_lower.contains("video has been removed")
        || stderr_lower.contains("this video does not exist")
        || stderr_lower.contains("video is not available")
    {
        return YtDlpErrorType::VideoUnavailable;
    }

    // Проверяем сетевые ошибки
    if stderr_lower.contains("timeout")
        || stderr_lower.contains("connection")
        || stderr_lower.contains("network")
        || stderr_lower.contains("socket")
        || stderr_lower.contains("dns")
        || stderr_lower.contains("failed to connect")
    {
        return YtDlpErrorType::NetworkError;
    }

    // Неизвестная ошибка
    YtDlpErrorType::Unknown
}

/// Возвращает пользовательское сообщение об ошибке
///
/// # Параметры
/// - `error_type`: тип ошибки
///
/// # Возвращает
/// - `String`: сообщение для пользователя
pub fn get_error_message(error_type: &YtDlpErrorType) -> String {
    match error_type {
        YtDlpErrorType::InvalidCookies => {
            "❌ Временная проблема с YouTube.\n\nПопробуй другое видео или повтори попытку позже.".to_string()
        }
        YtDlpErrorType::BotDetection => {
            "❌ YouTube заблокировал запрос.\n\nПопробуй другое видео или повтори попытку позже.".to_string()
        }
        YtDlpErrorType::VideoUnavailable => {
            "❌ Видео недоступно.\n\nВозможно оно приватное, удалено или заблокировано в твоём регионе.".to_string()
        }
        YtDlpErrorType::NetworkError => "❌ Проблема с сетью.\n\nПопробуй ещё раз через минуту.".to_string(),
        YtDlpErrorType::Unknown => "❌ Не удалось скачать видео.\n\nПроверь, что ссылка корректна.".to_string(),
    }
}

/// Определяет, нужно ли уведомлять администратора об ошибке
///
/// # Параметры
/// - `error_type`: тип ошибки
///
/// # Возвращает
/// - `true` если нужно уведомить администратора
pub fn should_notify_admin(error_type: &YtDlpErrorType) -> bool {
    match error_type {
        YtDlpErrorType::InvalidCookies => true,
        YtDlpErrorType::BotDetection => true,
        YtDlpErrorType::VideoUnavailable => false,
        YtDlpErrorType::NetworkError => false,
        YtDlpErrorType::Unknown => true,
    }
}

/// Возвращает рекомендации по исправлению ошибки для логов
///
/// # Параметры
/// - `error_type`: тип ошибки
///
/// # Возвращает
/// - `String`: рекомендации для администратора
pub fn get_fix_recommendations(error_type: &YtDlpErrorType) -> String {
    match error_type {
        YtDlpErrorType::InvalidCookies => "🔧 РЕКОМЕНДАЦИИ ПО ИСПРАВЛЕНИЮ:\n\
            • Cookies устарели или были обновлены в браузере\n\
            \n\
            📋 Вариант 1: Автоматическое извлечение из браузера (рекомендуется для Linux/Windows):\n\
              1. Убедись что залогинен в браузере на youtube.com\n\
              2. Установи зависимости: pip3 install keyring pycryptodomex\n\
              3. Установи переменную: export YTDL_COOKIES_BROWSER=chrome\n\
                 (поддерживаются: chrome, firefox, safari, brave, chromium, edge, opera, vivaldi)\n\
              4. Перезапусти бота\n\
            \n\
            📋 Вариант 2: Экспорт cookies в файл (рекомендуется для macOS):\n\
              1. Открой браузер и залогинься на youtube.com\n\
              2. Экспортируй cookies в файл youtube_cookies.txt\n\
              3. Убедись что файл в формате Netscape HTTP Cookie File\n\
              4. Установи переменную: export YTDL_COOKIES_FILE=youtube_cookies.txt\n\
              5. Перезапусти бота"
            .to_string(),
        YtDlpErrorType::BotDetection => "🔧 РЕКОМЕНДАЦИИ ПО ИСПРАВЛЕНИЮ:\n\
            • YouTube обнаружил автоматизированные запросы\n\
            • Обнови cookies из браузера\n\
            • Убедись что используешь актуальную версию yt-dlp\n\
            • Попробуй использовать другой player_client (android, web)"
            .to_string(),
        YtDlpErrorType::VideoUnavailable => {
            "ℹ️  Видео недоступно - это нормальная ситуация, не требует действий".to_string()
        }
        YtDlpErrorType::NetworkError => "🔧 РЕКОМЕНДАЦИИ ПО ИСПРАВЛЕНИЮ:\n\
            • Проверь интернет-соединение\n\
            • Проверь доступность youtube.com\n\
            • Увеличь таймауты если проблема повторяется"
            .to_string(),
        YtDlpErrorType::Unknown => "🔧 РЕКОМЕНДАЦИИ ПО ИСПРАВЛЕНИЮ:\n\
            • Проверь логи yt-dlp для деталей\n\
            • Убедись что видео доступно\n\
            • Проверь что yt-dlp обновлен до последней версии"
            .to_string(),
    }
}
