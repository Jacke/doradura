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
    /// Ошибки при загрузке фрагментов видео (обычно временные)
    FragmentError,
    /// Ошибка постобработки (ffmpeg FixupM3u8, конвертация и т.д.)
    PostprocessingError,
    /// Недостаточно места на диске
    DiskSpaceError,
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

    // Проверяем ошибки при загрузке фрагментов (обычно временные блокировки)
    if stderr_lower.contains("fragment")
        && (stderr_lower.contains("http error 403")
            || stderr_lower.contains("retrying fragment")
            || stderr_lower.contains("fragment not found")
            || stderr_lower.contains("skipping fragment"))
    {
        return YtDlpErrorType::FragmentError;
    }

    // Проверяем bot detection (если это не фрагменты)
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

    // Проверяем ошибки постобработки (ffmpeg, FixupM3u8 и т.д.)
    if stderr_lower.contains("postprocessing")
        || stderr_lower.contains("conversion failed")
        || stderr_lower.contains("fixupm3u8")
        || stderr_lower.contains("ffmpeg")
        || stderr_lower.contains("merger")
        || stderr_lower.contains("error fixing")
    {
        return YtDlpErrorType::PostprocessingError;
    }

    // Проверяем ошибки нехватки места на диске
    if stderr_lower.contains("no space left")
        || stderr_lower.contains("disk quota")
        || stderr_lower.contains("not enough space")
        || stderr_lower.contains("insufficient disk space")
        || stderr_lower.contains("enospc")
        || stderr_lower.contains("no free space")
        || stderr_lower.contains("disk full")
    {
        return YtDlpErrorType::DiskSpaceError;
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
        YtDlpErrorType::FragmentError => {
            "❌ Временная проблема при загрузке видео.\n\nПопробуй повторить попытку.".to_string()
        }
        YtDlpErrorType::PostprocessingError => "❌ Ошибка обработки видео.\n\nПопробуй повторить попытку.".to_string(),
        YtDlpErrorType::DiskSpaceError => {
            "❌ Сервер перегружен.\n\nПопробуй позже — мы уже работаем над этим.".to_string()
        }
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
        YtDlpErrorType::FragmentError => false, // Временные ошибки фрагментов - не требуют внимания
        YtDlpErrorType::PostprocessingError => false, // Пробуем retry с --fixup never
        YtDlpErrorType::DiskSpaceError => true, // КРИТИЧНО: нужно срочно освободить место!
        YtDlpErrorType::Unknown => true,
    }
}

/// Sanitizes a raw error string for user-facing output.
///
/// If the message looks like a yt-dlp stderr dump, return a friendly
/// user message instead of the raw error text.
pub fn sanitize_user_error_message(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return "❌ Не удалось скачать видео.\n\nПопробуй ещё раз позже.".to_string();
    }

    let lower = trimmed.to_lowercase();
    let looks_like_ytdlp = lower.contains("yt-dlp")
        || lower.contains("youtube-dl")
        || lower.contains("http error 403")
        || lower.contains("fragment")
        || lower.contains("signature extraction")
        || lower.contains("bot detection")
        || lower.contains("stderr")
        || lower.contains("stdout")
        || lower.contains("recommendations")
        || lower.contains("[download]")
        || lower.contains("warning: [youtube]")
        || lower.contains("error: [youtube]")
        || lower.contains("downloaded file is empty")
        || lower.contains("unable to download")
        || lower.contains("sign in to confirm you're not a bot");

    if looks_like_ytdlp {
        let error_type = analyze_ytdlp_error(trimmed);
        return get_error_message(&error_type);
    }

    trimmed.to_string()
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
        YtDlpErrorType::FragmentError => "🔧 РЕКОМЕНДАЦИИ ПО ИСПРАВЛЕНИЮ:\n\
            • Это временная ошибка при загрузке видео - yt-dlp автоматически переделывает фрагменты\n\
            • Если проблема повторяется часто:\n\
              1. Проверь интернет-соединение\n\
              2. Попробуй загрузить позже (YouTube может ограничивать частые запросы)\n\
              3. Убедись что используешь актуальную версию yt-dlp"
            .to_string(),
        YtDlpErrorType::PostprocessingError => "🔧 РЕКОМЕНДАЦИИ ПО ИСПРАВЛЕНИЮ:\n\
            • Ошибка постобработки видео (ffmpeg/FixupM3u8)\n\
            • Бот автоматически попробует повторить без постобработки\n\
            • Если проблема повторяется:\n\
              1. Проверь версию ffmpeg\n\
              2. Проверь место на диске\n\
              3. Проверь права записи в /tmp"
            .to_string(),
        YtDlpErrorType::DiskSpaceError => "🚨 КРИТИЧНО - НЕХВАТКА МЕСТА НА ДИСКЕ:\n\
            • Загрузки будут падать пока не освободить место!\n\
            \n\
            📋 СРОЧНЫЕ ДЕЙСТВИЯ:\n\
              1. Проверь место: df -h\n\
              2. Очисти downloads/: rm -rf /app/downloads/*\n\
              3. Очисти /tmp: rm -rf /tmp/*\n\
              4. Проверь логи: du -sh /app/logs/*\n\
              5. Если Railway — увеличь размер диска в настройках"
            .to_string(),
        YtDlpErrorType::Unknown => "🔧 РЕКОМЕНДАЦИИ ПО ИСПРАВЛЕНИЮ:\n\
            • Проверь логи yt-dlp для деталей\n\
            • Убедись что видео доступно\n\
            • Проверь что yt-dlp обновлен до последней версии"
            .to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ==================== analyze_ytdlp_error Tests ====================

    #[test]
    fn test_analyze_invalid_cookies_error() {
        let cases = vec![
            "cookies are no longer valid",
            "Cookies have likely been rotated",
            "Sign in to confirm you're not a bot",
            "Please sign in",
            "Use --cookies-from-browser",
            "Use --cookies for the authentication",
            "The provided YouTube account cookies are no longer valid",
        ];

        for case in cases {
            assert_eq!(
                analyze_ytdlp_error(case),
                YtDlpErrorType::InvalidCookies,
                "Failed for: {}",
                case
            );
        }
    }

    #[test]
    fn test_analyze_bot_detection_error() {
        let cases = vec![
            "bot detection triggered",
            "HTTP Error 403: Forbidden",
            "Unable to extract video data",
            "Signature extraction failed",
        ];

        for case in cases {
            assert_eq!(
                analyze_ytdlp_error(case),
                YtDlpErrorType::BotDetection,
                "Failed for: {}",
                case
            );
        }
    }

    #[test]
    fn test_analyze_video_unavailable_error() {
        let cases = vec![
            "Private video",
            "Video unavailable",
            "This video is not available in your country",
            "Video is private",
            "Video has been removed",
            "This video does not exist",
            "Video is not available",
        ];

        for case in cases {
            assert_eq!(
                analyze_ytdlp_error(case),
                YtDlpErrorType::VideoUnavailable,
                "Failed for: {}",
                case
            );
        }
    }

    #[test]
    fn test_analyze_network_error() {
        let cases = vec![
            "Connection timeout",
            "Connection refused",
            "Network unreachable",
            "Socket error",
            "DNS resolution failed",
            "Failed to connect to server",
        ];

        for case in cases {
            assert_eq!(
                analyze_ytdlp_error(case),
                YtDlpErrorType::NetworkError,
                "Failed for: {}",
                case
            );
        }
    }

    #[test]
    fn test_analyze_unknown_error() {
        let cases = vec!["Some random error", "Unknown error occurred", "Unexpected behavior", ""];

        for case in cases {
            assert_eq!(
                analyze_ytdlp_error(case),
                YtDlpErrorType::Unknown,
                "Failed for: '{}'",
                case
            );
        }
    }

    #[test]
    fn test_analyze_case_insensitive() {
        // Should work regardless of case
        assert_eq!(
            analyze_ytdlp_error("COOKIES ARE NO LONGER VALID"),
            YtDlpErrorType::InvalidCookies
        );
        assert_eq!(analyze_ytdlp_error("http error 403"), YtDlpErrorType::BotDetection);
        assert_eq!(analyze_ytdlp_error("PRIVATE VIDEO"), YtDlpErrorType::VideoUnavailable);
        assert_eq!(analyze_ytdlp_error("CONNECTION TIMEOUT"), YtDlpErrorType::NetworkError);
    }

    // ==================== get_error_message Tests ====================

    #[test]
    fn test_get_error_message_invalid_cookies() {
        let msg = get_error_message(&YtDlpErrorType::InvalidCookies);
        assert!(msg.contains("❌"));
        assert!(msg.contains("YouTube"));
    }

    #[test]
    fn test_get_error_message_bot_detection() {
        let msg = get_error_message(&YtDlpErrorType::BotDetection);
        assert!(msg.contains("❌"));
        assert!(msg.contains("YouTube"));
        assert!(msg.contains("заблокировал"));
    }

    #[test]
    fn test_get_error_message_video_unavailable() {
        let msg = get_error_message(&YtDlpErrorType::VideoUnavailable);
        assert!(msg.contains("❌"));
        assert!(msg.contains("недоступно"));
    }

    #[test]
    fn test_get_error_message_network() {
        let msg = get_error_message(&YtDlpErrorType::NetworkError);
        assert!(msg.contains("❌"));
        assert!(msg.contains("сет"));
    }

    #[test]
    fn test_get_error_message_unknown() {
        let msg = get_error_message(&YtDlpErrorType::Unknown);
        assert!(msg.contains("❌"));
        assert!(msg.contains("скачать"));
    }

    // ==================== should_notify_admin Tests ====================

    #[test]
    fn test_should_notify_admin_critical_errors() {
        assert!(should_notify_admin(&YtDlpErrorType::InvalidCookies));
        assert!(should_notify_admin(&YtDlpErrorType::BotDetection));
        assert!(should_notify_admin(&YtDlpErrorType::Unknown));
    }

    #[test]
    fn test_should_not_notify_admin_normal_errors() {
        assert!(!should_notify_admin(&YtDlpErrorType::VideoUnavailable));
        assert!(!should_notify_admin(&YtDlpErrorType::NetworkError));
    }

    // ==================== get_fix_recommendations Tests ====================

    #[test]
    fn test_get_fix_recommendations_invalid_cookies() {
        let recs = get_fix_recommendations(&YtDlpErrorType::InvalidCookies);
        assert!(recs.contains("РЕКОМЕНДАЦИИ"));
        assert!(recs.contains("cookies"));
        assert!(recs.contains("браузер"));
    }

    #[test]
    fn test_get_fix_recommendations_bot_detection() {
        let recs = get_fix_recommendations(&YtDlpErrorType::BotDetection);
        assert!(recs.contains("РЕКОМЕНДАЦИИ"));
        assert!(recs.contains("yt-dlp"));
    }

    #[test]
    fn test_get_fix_recommendations_video_unavailable() {
        let recs = get_fix_recommendations(&YtDlpErrorType::VideoUnavailable);
        assert!(recs.contains("недоступно"));
        assert!(recs.contains("не требует"));
    }

    #[test]
    fn test_get_fix_recommendations_network() {
        let recs = get_fix_recommendations(&YtDlpErrorType::NetworkError);
        assert!(recs.contains("интернет"));
        assert!(recs.contains("youtube.com"));
    }

    #[test]
    fn test_get_fix_recommendations_unknown() {
        let recs = get_fix_recommendations(&YtDlpErrorType::Unknown);
        assert!(recs.contains("логи"));
        assert!(recs.contains("yt-dlp"));
    }

    // ==================== sanitize_user_error_message Tests ====================

    #[test]
    fn test_sanitize_user_error_message_ytdlp() {
        let raw = "ERROR: [youtube] abc: HTTP Error 403: Forbidden";
        let sanitized = sanitize_user_error_message(raw);
        assert!(!sanitized.to_lowercase().contains("yt-dlp"));
        assert!(sanitized.contains("YouTube"));
    }

    #[test]
    fn test_sanitize_user_error_message_passthrough() {
        let raw = "❌ Видео недоступно.\n\nПопробуй другое видео.";
        let sanitized = sanitize_user_error_message(raw);
        assert_eq!(sanitized, raw);
    }

    // ==================== YtDlpErrorType Trait Tests ====================

    #[test]
    fn test_error_type_debug() {
        assert_eq!(format!("{:?}", YtDlpErrorType::InvalidCookies), "InvalidCookies");
        assert_eq!(format!("{:?}", YtDlpErrorType::BotDetection), "BotDetection");
        assert_eq!(format!("{:?}", YtDlpErrorType::VideoUnavailable), "VideoUnavailable");
        assert_eq!(format!("{:?}", YtDlpErrorType::NetworkError), "NetworkError");
        assert_eq!(format!("{:?}", YtDlpErrorType::Unknown), "Unknown");
    }

    #[test]
    fn test_error_type_clone() {
        let original = YtDlpErrorType::InvalidCookies;
        let cloned = original.clone();
        assert_eq!(original, cloned);
    }

    #[test]
    fn test_error_type_equality() {
        assert_eq!(YtDlpErrorType::InvalidCookies, YtDlpErrorType::InvalidCookies);
        assert_ne!(YtDlpErrorType::InvalidCookies, YtDlpErrorType::BotDetection);
        assert_ne!(YtDlpErrorType::NetworkError, YtDlpErrorType::Unknown);
    }

    // ==================== Integration Tests ====================

    #[test]
    fn test_full_error_handling_flow() {
        let stderr = "ERROR: Cookies are no longer valid. Please use --cookies-from-browser";

        let error_type = analyze_ytdlp_error(stderr);
        assert_eq!(error_type, YtDlpErrorType::InvalidCookies);

        let user_msg = get_error_message(&error_type);
        assert!(!user_msg.is_empty());

        let notify = should_notify_admin(&error_type);
        assert!(notify);

        let recommendations = get_fix_recommendations(&error_type);
        assert!(recommendations.contains("cookies"));
    }

    #[test]
    fn test_real_world_error_messages() {
        // Real error messages from yt-dlp
        let cases = vec![
            (
                "ERROR: [youtube] dQw4w9WgXcQ: Sign in to confirm you're not a bot. Use --cookies-from-browser",
                YtDlpErrorType::InvalidCookies,
            ),
            (
                "ERROR: [youtube] abc123: Private video. Sign in if you've been granted access to this video",
                YtDlpErrorType::VideoUnavailable,
            ),
            (
                "ERROR: unable to download video data: HTTP Error 403: Forbidden",
                YtDlpErrorType::BotDetection,
            ),
            // Note: "timed out" matches "timeout" check since we use contains
            (
                "ERROR: Unable to download webpage: Connection timeout",
                YtDlpErrorType::NetworkError,
            ),
        ];

        for (stderr, expected) in cases {
            assert_eq!(analyze_ytdlp_error(stderr), expected, "Failed for stderr: {}", stderr);
        }
    }
}
