use crate::core::config;
use crate::core::error::AppError;
use crate::core::rate_limiter::RateLimiter;
use crate::core::utils::{escape_filename, sanitize_filename};
use crate::download::progress::{DownloadStatus, ProgressMessage};
use crate::download::ytdlp_errors::{
    analyze_ytdlp_error, get_error_message, get_fix_recommendations, should_notify_admin,
};
use crate::storage::cache;
use crate::storage::db::{self as db, save_download_history, DbPool};
use chrono::{DateTime, Utc};
use rand::Rng;
use regex::Regex;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;
use teloxide::prelude::*;
use teloxide::types::InputFile;
use tokio::process::Command as TokioCommand;
use tokio::time::timeout;
use url::Url;

/// Legacy alias for backward compatibility
/// Use AppError instead
#[deprecated(note = "Use AppError instead")]
pub type CommandError = AppError;

/// Извлекает время ожидания из ошибки Telegram API (rate limiting)
///
/// Парсит строку ошибки вида "Retry after Xs" и возвращает количество секунд
fn extract_retry_after(error_str: &str) -> Option<u64> {
    // Пробуем найти паттерн "Retry after Xs" или "retry_after: X"
    let re = Regex::new(r"(?i)retry\s+after\s+(\d+)\s*s").ok()?;
    if let Some(caps) = re.captures(error_str) {
        if let Some(seconds_str) = caps.get(1) {
            return seconds_str.as_str().parse::<u64>().ok();
        }
    }

    // Альтернативный паттерн: "retry_after: X"
    let re2 = Regex::new(r"(?i)retry_after[:\s]+(\d+)").ok()?;
    if let Some(caps) = re2.captures(error_str) {
        if let Some(seconds_str) = caps.get(1) {
            return seconds_str.as_str().parse::<u64>().ok();
        }
    }

    None
}

/// Определяет формат изображения по магическим байтам
#[derive(Debug, Clone, Copy, PartialEq)]
enum ImageFormat {
    Jpeg,
    Png,
    WebP,
    Unknown,
}

/// Определяет формат изображения по первым байтам файла
fn detect_image_format(bytes: &[u8]) -> ImageFormat {
    if bytes.len() < 4 {
        return ImageFormat::Unknown;
    }

    // JPEG: FF D8 FF
    if bytes.len() >= 3 && bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes[2] == 0xFF {
        return ImageFormat::Jpeg;
    }

    // PNG: 89 50 4E 47
    if bytes.len() >= 4
        && bytes[0] == 0x89
        && bytes[1] == 0x50
        && bytes[2] == 0x4E
        && bytes[3] == 0x47
    {
        return ImageFormat::Png;
    }

    // WebP: RIFF...WEBP
    if bytes.len() >= 12
        && bytes[0] == 0x52
        && bytes[1] == 0x49
        && bytes[2] == 0x46
        && bytes[3] == 0x46
        && bytes[8] == 0x57
        && bytes[9] == 0x45
        && bytes[10] == 0x42
        && bytes[11] == 0x50
    {
        return ImageFormat::WebP;
    }

    ImageFormat::Unknown
}

/// Проверяет формат файла cookies (должен быть Netscape HTTP Cookie File)
///
/// Формат Netscape начинается с "# Netscape HTTP Cookie File" или "# HTTP Cookie File"
/// и содержит строки вида: domain\tflag\tpath\tsecure\texpiration\tname\tvalue
fn validate_cookies_file_format(cookies_file: &str) -> bool {
    if let Ok(contents) = std::fs::read_to_string(cookies_file) {
        // Проверяем наличие заголовка Netscape
        let has_header = contents.lines().any(|line| {
            line.trim().starts_with("# Netscape HTTP Cookie File")
                || line.trim().starts_with("# HTTP Cookie File")
        });

        // Проверяем наличие хотя бы одной строки с cookie (формат: domain\tflag\tpath...)
        let has_cookies = contents.lines().any(|line| {
            let trimmed = line.trim();
            !trimmed.is_empty() && !trimmed.starts_with('#') && trimmed.split('\t').count() >= 7
        });

        has_header && has_cookies
    } else {
        false
    }
}

/// Добавляет аргументы cookies к списку аргументов yt-dlp
///
/// Использует либо файл cookies (YTDL_COOKIES_FILE) либо браузер (YTDL_COOKIES_BROWSER).
/// Приоритет: файл > браузер
///
/// # Arguments
///
/// * `args` - Вектор аргументов для yt-dlp
pub fn add_cookies_args(args: &mut Vec<&str>) {
    // Приоритет 1: Файл cookies
    if let Some(ref cookies_file) = *config::YTDL_COOKIES_FILE {
        if !cookies_file.is_empty() {
            // Преобразуем относительный путь в абсолютный (если нужно)
            let cookies_path = if std::path::Path::new(cookies_file).is_absolute() {
                cookies_file.clone()
            } else {
                // Пытаемся найти файл в текущей директории или через расширение тильды
                let expanded = shellexpand::tilde(cookies_file);
                expanded.to_string()
            };

            // Проверяем существование файла
            let cookies_path_buf = std::path::Path::new(&cookies_path);
            if !cookies_path_buf.exists() {
                log::error!(
                    "❌ Cookies file not found: {} (checked: {})",
                    cookies_file,
                    cookies_path
                );
                log::error!(
                    "   Current working directory: {:?}",
                    std::env::current_dir()
                );
                log::error!("   YouTube downloads will FAIL without valid cookies!");
                log::error!("   Please check the path and ensure the file exists.");
                // НЕ добавляем аргументы cookies, если файл не найден
                return;
            } else {
                // Получаем абсолютный путь для логирования
                let abs_path = cookies_path_buf
                    .canonicalize()
                    .unwrap_or_else(|_| cookies_path_buf.to_path_buf());

                // Проверяем формат файла
                if !validate_cookies_file_format(&cookies_path) {
                    log::warn!(
                        "⚠️  Cookies file format may be invalid: {}",
                        abs_path.display()
                    );
                    log::warn!("Expected Netscape HTTP Cookie File format:");
                    log::warn!("  - Header: # Netscape HTTP Cookie File");
                    log::warn!(
                        "  - Format: domain\\tflag\\tpath\\tsecure\\texpiration\\tname\\tvalue"
                    );
                    log::warn!("See: https://github.com/yt-dlp/yt-dlp/wiki/FAQ#how-do-i-pass-cookies-to-yt-dlp");
                    log::warn!("You may need to re-export cookies from your browser.");
                } else {
                    log::info!("✅ Cookies file format validated: {}", abs_path.display());
                }

                args.push("--cookies");
                // Используем абсолютный путь для надежности
                let abs_path_str = abs_path.to_string_lossy().to_string();
                // SAFETY: Эта ссылка живет достаточно долго, так как она из Box::leak
                let leaked_path = Box::leak(abs_path_str.into_boxed_str());
                args.push(unsafe { std::mem::transmute::<&str, &'static str>(leaked_path) });
                log::info!("Using cookies from file: {}", abs_path.display());
                return;
            }
        }
    }

    // Приоритет 2: Браузер
    let browser = config::YTDL_COOKIES_BROWSER.as_str();
    if !browser.is_empty() {
        args.push("--cookies-from-browser");
        args.push(browser);
        log::info!("Using cookies from browser: {}", browser);
    } else {
        log::warn!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        log::warn!("⚠️  NO COOKIES CONFIGURED!");
        log::warn!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        log::warn!("YouTube downloads will fail with 'bot detection' or 'only images' errors!");
        log::warn!("");

        #[cfg(target_os = "macos")]
        {
            log::warn!("🍎 macOS USERS:");
            log::warn!("   Browser cookie extraction requires Full Disk Access.");
            log::warn!("   It's MUCH EASIER to export cookies to a file!");
            log::warn!("");
            log::warn!("   📖 See: MACOS_COOKIES_FIX.md for step-by-step guide");
            log::warn!("");
            log::warn!("   Quick fix:");
            log::warn!("   1. Install Chrome extension: Get cookies.txt LOCALLY");
            log::warn!("   2. Go to youtube.com → login");
            log::warn!("   3. Click extension → Export → save as youtube_cookies.txt");
            log::warn!("   4. Run: ./run_with_cookies.sh");
        }

        #[cfg(not(target_os = "macos"))]
        {
            log::warn!("💡 AUTOMATIC COOKIE EXTRACTION (Recommended):");
            log::warn!("   1. Login to YouTube in your browser (chrome/firefox/etc)");
            log::warn!("   2. Install dependencies: pip3 install keyring pycryptodomex");
            log::warn!("   3. Set browser: export YTDL_COOKIES_BROWSER=chrome");
            log::warn!(
                "      Supported: chrome, firefox, safari, brave, chromium, edge, opera, vivaldi"
            );
            log::warn!("   4. Restart the bot");
            log::warn!("");
            log::warn!("💡 OR EXPORT TO FILE (Alternative):");
            log::warn!("   1. Export cookies from browser to youtube_cookies.txt");
            log::warn!("   2. Set: export YTDL_COOKIES_FILE=youtube_cookies.txt");
            log::warn!("   3. Restart the bot");
        }

        log::warn!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    }
}

fn probe_duration_seconds(path: &str) -> Option<u32> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
            path,
        ])
        .output()
        .ok()?;

    let duration_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if duration_str.is_empty() {
        return None;
    }
    let secs = duration_str.parse::<f32>().ok()?;
    Some(secs.round() as u32)
}

/// Проверяет, содержит ли файл и видео, и аудио дорожки
fn has_both_video_and_audio(path: &str) -> Result<bool, AppError> {
    // Проверяем наличие видео дорожки
    let video_output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=codec_type",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
            path,
        ])
        .output()
        .map_err(|e| AppError::Download(format!("Failed to check video stream: {}", e)))?;

    let has_video = !String::from_utf8_lossy(&video_output.stdout)
        .trim()
        .is_empty();

    // Проверяем наличие аудио дорожки
    let audio_output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "a:0",
            "-show_entries",
            "stream=codec_type",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
            path,
        ])
        .output()
        .map_err(|e| AppError::Download(format!("Failed to check audio stream: {}", e)))?;

    let has_audio = !String::from_utf8_lossy(&audio_output.stdout)
        .trim()
        .is_empty();

    Ok(has_video && has_audio)
}

/// Получает метаданные видео: длительность, ширину и высоту
/// Используется для корректной отправки видео в Telegram
fn probe_video_metadata(path: &str) -> Option<(u32, Option<u32>, Option<u32>)> {
    // Получаем duration
    let duration = probe_duration_seconds(path)?;

    // Получаем width
    let width_output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
            path,
        ])
        .output()
        .ok()?;

    let width = String::from_utf8_lossy(&width_output.stdout)
        .trim()
        .parse::<u32>()
        .ok();

    // Получаем height
    let height_output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=height",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
            path,
        ])
        .output()
        .ok()?;

    let height = String::from_utf8_lossy(&height_output.stdout)
        .trim()
        .parse::<u32>()
        .ok();

    Some((duration, width, height))
}

/// Конвертирует WebP изображение в JPEG используя ffmpeg
///
/// Args: webp_bytes - байты WebP изображения
/// Returns: Result<Vec<u8>> - байты JPEG изображения
fn convert_webp_to_jpeg(webp_bytes: &[u8]) -> Result<Vec<u8>, AppError> {
    // Создаем временный файл для WebP
    let temp_dir = std::env::temp_dir();
    let temp_webp = temp_dir.join(format!(
        "temp_webp_{}.webp",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    ));
    let temp_jpeg = temp_dir.join(format!(
        "temp_jpeg_{}.jpg",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    ));

    // Сохраняем WebP во временный файл
    fs::write(&temp_webp, webp_bytes)
        .map_err(|e| AppError::Download(format!("Failed to write WebP temp file: {}", e)))?;

    // Конвертируем WebP в JPEG используя ffmpeg
    let output = Command::new("ffmpeg")
        .args([
            "-i",
            temp_webp.to_str().unwrap_or(""),
            "-q:v",
            "2",  // Высокое качество
            "-y", // Перезаписать выходной файл
            temp_jpeg.to_str().unwrap_or(""),
        ])
        .output();

    let _ = fs::remove_file(&temp_webp);

    match output {
        Ok(result) => {
            if result.status.success() {
                match fs::read(&temp_jpeg) {
                    Ok(jpeg_bytes) => {
                        let _ = fs::remove_file(&temp_jpeg);
                        Ok(jpeg_bytes)
                    }
                    Err(e) => {
                        let _ = fs::remove_file(&temp_jpeg);
                        Err(AppError::Download(format!(
                            "Failed to read converted JPEG: {}",
                            e
                        )))
                    }
                }
            } else {
                let stderr = String::from_utf8_lossy(&result.stderr);
                let _ = fs::remove_file(&temp_jpeg);
                Err(AppError::Download(format!(
                    "ffmpeg conversion failed: {}",
                    stderr
                )))
            }
        }
        Err(e) => {
            let _ = fs::remove_file(&temp_jpeg);
            Err(AppError::Download(format!("Failed to run ffmpeg: {}", e)))
        }
    }
}

/// Сжимает JPEG thumbnail до размера <= 200KB
///
/// Args: jpeg_bytes - байты JPEG изображения
/// Returns: Option<Vec<u8>> - сжатые байты JPEG или None при ошибке
fn compress_thumbnail_jpeg(jpeg_bytes: &[u8]) -> Option<Vec<u8>> {
    // Создаем временные файлы
    let temp_dir = std::env::temp_dir();
    let temp_input = temp_dir.join(format!(
        "thumb_in_{}.jpg",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    ));
    let temp_output = temp_dir.join(format!(
        "thumb_out_{}.jpg",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    ));

    if fs::write(&temp_input, jpeg_bytes).is_err() {
        return None;
    }

    // Сжимаем используя ffmpeg с уменьшением качества и размера
    let output = Command::new("ffmpeg")
        .args([
            "-i",
            temp_input.to_str().unwrap_or(""),
            "-vf",
            "scale=320:320:force_original_aspect_ratio=decrease",
            "-q:v",
            "5", // Среднее качество для уменьшения размера
            "-y",
            temp_output.to_str().unwrap_or(""),
        ])
        .output();

    let _ = fs::remove_file(&temp_input);

    match output {
        Ok(result) => {
            if result.status.success() {
                if let Ok(compressed) = fs::read(&temp_output) {
                    let _ = fs::remove_file(&temp_output);
                    if compressed.len() <= 200 * 1024 {
                        Some(compressed)
                    } else {
                        // Если все еще слишком большой, попробуем еще более низкое качество
                        None
                    }
                } else {
                    let _ = fs::remove_file(&temp_output);
                    None
                }
            } else {
                let _ = fs::remove_file(&temp_output);
                None
            }
        }
        Err(_) => {
            let _ = fs::remove_file(&temp_output);
            None
        }
    }
}

/// Генерирует thumbnail из видео файла используя ffmpeg
/// Извлекает первый кадр видео и сохраняет его как JPEG
///
/// Args: video_path - путь к видео файлу
/// Returns: Option<Vec<u8>> - байты JPEG изображения или None при ошибке
fn generate_thumbnail_from_video(video_path: &str) -> Option<Vec<u8>> {
    log::info!(
        "[THUMBNAIL] Generating thumbnail from video file: {}",
        video_path
    );

    // Создаем временный файл для thumbnail
    let temp_dir = std::env::temp_dir();
    let temp_thumbnail_path = temp_dir.join(format!(
        "thumb_{}.jpg",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    ));

    // Извлекаем первый кадр с помощью ffmpeg
    // Используем vframes=1 для получения одного кадра
    // Используем scale для уменьшения размера (максимум 320x320 для Telegram)
    let output = Command::new("ffmpeg")
        .args([
            "-i",
            video_path,
            "-vframes",
            "1",
            "-vf",
            "scale=320:320:force_original_aspect_ratio=decrease",
            "-q:v",
            "2", // Высокое качество JPEG (2 = высокое, 31 = низкое)
            "-f",
            "image2",
            temp_thumbnail_path.to_str().unwrap_or(""),
        ])
        .output();

    match output {
        Ok(result) => {
            if result.status.success() {
                // Читаем сгенерированный thumbnail
                match fs::read(&temp_thumbnail_path) {
                    Ok(bytes) => {
                        log::info!("[THUMBNAIL] Successfully generated thumbnail from video: {} bytes ({} KB)",
                            bytes.len(), bytes.len() as f64 / 1024.0);

                        // Удаляем временный файл
                        let _ = fs::remove_file(&temp_thumbnail_path);

                        // Проверяем размер (Telegram требует <= 200 KB)
                        if bytes.len() > 200 * 1024 {
                            log::warn!("[THUMBNAIL] Generated thumbnail size ({} KB) exceeds Telegram limit (200 KB). Will try to compress.",
                                bytes.len() as f64 / 1024.0);
                            // Можно попробовать сжать, но для простоты просто вернем
                            // Telegram может принять файл больше 200KB, но может не отобразить preview
                        }

                        Some(bytes)
                    }
                    Err(e) => {
                        log::warn!("[THUMBNAIL] Failed to read generated thumbnail: {}", e);
                        let _ = fs::remove_file(&temp_thumbnail_path);
                        None
                    }
                }
            } else {
                let stderr = String::from_utf8_lossy(&result.stderr);
                log::warn!(
                    "[THUMBNAIL] ffmpeg failed to generate thumbnail: {}",
                    stderr
                );
                let _ = fs::remove_file(&temp_thumbnail_path);
                None
            }
        }
        Err(e) => {
            log::warn!(
                "[THUMBNAIL] Failed to run ffmpeg to generate thumbnail: {}",
                e
            );
            None
        }
    }
}

/// Находит фактическое имя файла после скачивания yt-dlp
/// yt-dlp может добавлять суффиксы (1).mp4, (2).mp4 если файл уже существует
///
/// # Arguments
///
/// * `expected_path` - Ожидаемый путь к файлу
///
/// # Returns
///
/// Возвращает фактический путь к файлу или исходный путь, если файл найден
fn find_actual_downloaded_file(expected_path: &str) -> Result<String, AppError> {
    let path = Path::new(expected_path);

    // Если файл существует по ожидаемому пути - возвращаем его
    if path.exists() {
        log::debug!("File found at expected path: {}", expected_path);
        return Ok(expected_path.to_string());
    }

    log::warn!("File not found at expected path: {}", expected_path);

    // Получаем директорию и базовое имя файла
    let parent_dir = path.parent().ok_or_else(|| {
        AppError::Download(format!(
            "Cannot get parent directory for: {}",
            expected_path
        ))
    })?;

    let file_stem = path.file_stem().and_then(|s| s.to_str()).ok_or_else(|| {
        AppError::Download(format!("Cannot get file stem for: {}", expected_path))
    })?;

    let file_extension = path.extension().and_then(|s| s.to_str()).unwrap_or("mp4");

    // Ищем файлы, начинающиеся с базового имени
    let dir_entries = fs::read_dir(parent_dir)
        .map_err(|e| AppError::Download(format!("Failed to read downloads dir: {}", e)))?;

    let mut found_files = Vec::new();
    for entry in dir_entries {
        if let Ok(entry) = entry {
            let file_name = entry.file_name();
            let file_name_str = file_name.to_string_lossy();

            // Проверяем, начинается ли имя файла с нашего базового имени и имеет нужное расширение
            // yt-dlp может добавлять суффиксы как (1).mp4, (2).mp4 к имени файла
            // file_stem уже содержит timestamp, поэтому проверяем точное совпадение или начало
            let matches_pattern = file_name_str.starts_with(file_stem)
                && file_name_str.ends_with(&format!(".{}", file_extension));

            if matches_pattern {
                let full_path = entry.path().to_string_lossy().to_string();
                found_files.push(full_path);
            }
        }
    }

    if found_files.is_empty() {
        log::error!(
            "No matching files found in directory: {}",
            parent_dir.display()
        );
        return Err(AppError::Download(format!(
            "Downloaded file not found at {} or in directory",
            expected_path
        )));
    }

    // Если найдено несколько файлов, берем последний (наиболее вероятно новый)
    let actual_path = found_files.last().unwrap().clone();
    log::info!(
        "Found actual downloaded file: {} (searched for: {})",
        actual_path,
        expected_path
    );

    Ok(actual_path)
}

/// Получить метаданные от yt-dlp (быстрее чем HTTP парсинг)
/// Использует async команду чтобы не блокировать runtime
/// Проверяет кэш перед запросом к yt-dlp
async fn get_metadata_from_ytdlp(url: &Url) -> Result<(String, String), AppError> {
    // Проверяем кэш, но игнорируем "Unknown Track" и "NA" в artist
    if let Some((title, artist)) = cache::get_cached_metadata(url).await {
        if title.trim() != "Unknown Track" && !title.trim().is_empty() {
            // Если artist пустой или "NA" - игнорируем кэш и получаем свежие данные
            if artist.trim().is_empty() || artist.trim() == "NA" {
                log::debug!(
                    "Ignoring cached metadata with empty/NA artist for URL: {}",
                    url
                );
            } else {
                log::debug!("Metadata cache hit for URL: {}", url);
                return Ok((title, artist));
            }
        } else {
            log::warn!(
                "Ignoring invalid cached metadata '{}' for URL: {}",
                title,
                url
            );
        }
    }

    log::debug!("Metadata cache miss for URL: {}", url);
    let ytdl_bin = &*config::YTDL_BIN;
    log::debug!("Using downloader binary: {}", ytdl_bin);
    log::debug!("Fetching metadata for URL: {}", url);

    // Строим аргументы с поддержкой cookies
    // Используем --print для более надёжного получения метаданных
    let mut args_vec: Vec<String> = vec![
        "--print".to_string(),
        "%(title)s".to_string(),
        "--no-playlist".to_string(),
        "--skip-download".to_string(),
    ];

    // Добавляем cookies аргументы
    let mut temp_args: Vec<&str> = vec![];
    add_cookies_args(&mut temp_args);
    for arg in temp_args {
        args_vec.push(arg.to_string());
    }

    // НЕ используем android клиент!
    // YouTube изменил политику: теперь Android требует PO Token
    // Используем дефолтный web клиент который работает с cookies

    args_vec.push("--no-check-certificate".to_string());
    args_vec.push(url.as_str().to_string());

    let args: Vec<&str> = args_vec.iter().map(|s| s.as_str()).collect();

    // Логируем полную команду для отладки
    let command_str = format!("{} {}", ytdl_bin, args.join(" "));
    log::info!("[DEBUG] yt-dlp command for metadata: {}", command_str);

    // Получаем title используя async команду с таймаутом
    let title_output = timeout(
        config::download::ytdlp_timeout(),
        TokioCommand::new(&ytdl_bin).args(&args).output(),
    )
    .await
    .map_err(|_| {
        log::error!(
            "yt-dlp command timed out after {} seconds",
            config::download::YTDLP_TIMEOUT_SECS
        );
        AppError::Download(format!("yt-dlp command timed out"))
    })?
    .map_err(|e| {
        log::error!("Failed to execute {}: {}", ytdl_bin, e);
        AppError::Download(format!("Failed to get title: {}", e))
    })?;

    log::debug!(
        "yt-dlp exit status: {:?}, stdout length: {}",
        title_output.status,
        title_output.stdout.len()
    );

    if !title_output.status.success() {
        let stderr = String::from_utf8_lossy(&title_output.stderr);
        let error_type = analyze_ytdlp_error(&stderr);

        // Логируем детальную информацию об ошибке
        log::error!(
            "yt-dlp failed to get metadata, error type: {:?}",
            error_type
        );
        log::error!("yt-dlp stderr: {}", stderr);

        // Логируем рекомендации по исправлению
        let recommendations = get_fix_recommendations(&error_type);
        log::error!("{}", recommendations);

        // Если нужно уведомить администратора, логируем это
        if should_notify_admin(&error_type) {
            log::warn!("⚠️  This error requires administrator attention!");
        }

        // Возвращаем пользовательское сообщение об ошибке
        return Err(AppError::Download(get_error_message(&error_type)));
    }

    let title = String::from_utf8_lossy(&title_output.stdout)
        .trim()
        .to_string();

    // Проверяем что название не пустое
    if title.is_empty() {
        log::error!("yt-dlp returned empty title for URL: {}", url);
        return Err(AppError::Download(format!(
            "Failed to get video title. Video might be unavailable or private."
        )));
    }

    log::info!("Successfully got metadata from yt-dlp: title='{}'", title);

    // Получаем artist через --print "%(artist)s"
    let mut artist_args_vec: Vec<String> = vec![
        "--print".to_string(),
        "%(artist)s".to_string(),
        "--no-playlist".to_string(),
        "--skip-download".to_string(),
    ];

    // Добавляем cookies аргументы
    let mut temp_args: Vec<&str> = vec![];
    add_cookies_args(&mut temp_args);
    for arg in temp_args {
        artist_args_vec.push(arg.to_string());
    }

    artist_args_vec.push("--no-check-certificate".to_string());
    artist_args_vec.push(url.as_str().to_string());

    let artist_args: Vec<&str> = artist_args_vec.iter().map(|s| s.as_str()).collect();

    let artist_output = timeout(
        config::download::ytdlp_timeout(),
        TokioCommand::new(&ytdl_bin).args(&artist_args).output(),
    )
    .await
    .ok(); // Не критично, игнорируем ошибки таймаута

    let mut artist = artist_output
        .and_then(|result| result.ok())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .unwrap_or_default();

    // Если artist пустой, "NA" или содержит только пробелы - получаем channel/uploader
    if artist.trim().is_empty() || artist.trim() == "NA" {
        log::debug!("Artist is empty or 'NA', trying to get channel/uploader");

        // Пробуем получить uploader (название канала)
        let mut uploader_args_vec: Vec<String> = vec![
            "--print".to_string(),
            "%(uploader)s".to_string(),
            "--no-playlist".to_string(),
            "--skip-download".to_string(),
        ];

        // Добавляем cookies аргументы
        let mut temp_args: Vec<&str> = vec![];
        add_cookies_args(&mut temp_args);
        for arg in temp_args {
            uploader_args_vec.push(arg.to_string());
        }

        uploader_args_vec.push("--no-check-certificate".to_string());
        uploader_args_vec.push(url.as_str().to_string());

        let uploader_args: Vec<&str> = uploader_args_vec.iter().map(|s| s.as_str()).collect();

        let uploader_output = timeout(
            config::download::ytdlp_timeout(),
            TokioCommand::new(&ytdl_bin).args(&uploader_args).output(),
        )
        .await
        .ok();

        let uploader = uploader_output
            .and_then(|result| result.ok())
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
            .unwrap_or_default();

        if !uploader.trim().is_empty() && uploader.trim() != "NA" {
            artist = uploader;
            log::info!("Using uploader/channel as artist: '{}'", artist);
        } else {
            log::warn!("Could not get artist or uploader, leaving empty");
        }
    }

    // Сохраняем в кэш только если title не пустой и не "Unknown Track"
    if !title.trim().is_empty() && title.trim() != "Unknown Track" {
        cache::cache_metadata(url, title.clone(), artist.clone()).await;
    } else {
        log::warn!("Not caching metadata with invalid title: '{}'", title);
    }

    log::info!(
        "Got metadata from yt-dlp: title='{}', artist='{}'",
        title,
        artist
    );
    Ok((title, artist))
}

/// Отправляет сообщение об ошибке с случайным стикером и детальным объяснением
async fn send_error_with_sticker(bot: &Bot, chat_id: ChatId) {
    send_error_with_sticker_and_message(bot, chat_id, None).await;
}

/// Отправляет сообщение об ошибке с случайным стикером и опциональным кастомным сообщением
async fn send_error_with_sticker_and_message(
    bot: &Bot,
    chat_id: ChatId,
    custom_message: Option<&str>,
) {
    // Список file_id стикеров из стикерпака doraduradoradura
    let sticker_file_ids = vec![
        "CAACAgIAAxUAAWj-ZokEQu5YpTnjl6IWPzCQZ0UUAAJCEwAC52QwSC6nTghQdw-KNgQ",
        "CAACAgIAAxUAAWj-ZomIQgQKKpbMZA0_VDzfavIiAAK1GgACt8dBSNRj5YvFS-dmNgQ",
        "CAACAgIAAxUAAWj-Zokct93wagdDXh1JbhxBIyJOAALzFwACoktASAOjHltqzx0ENgQ",
        "CAACAgIAAxUAAWj-ZomorWU-YHGN6oQ6-ikN46CJAAInFAACqlJYSGHilrVqW1AxNgQ",
        "CAACAgIAAxUAAWj-ZonVzqfhCC1-YjDNhqGioqvVAALdEwAC-_ZpSB5PRC_sd93QNgQ",
        "CAACAgIAAxkBAAIFymj-YswNosbIex7SmXJejbO_GN7-AAJMGQAC9MFQSHBzdKlbjXskNgQ",
        "CAACAgIAAxUAAWj-Zol_H6tZIPG-PPHnpNZS1QkIAAJFGwACIQtBSDwm6rS-ZojVNgQ",
        "CAACAgIAAxUAAWj-ZomOtDnC9_6jFRp84js-HQN5AALzEgACqc5ISI4uefJ9dzZPNgQ",
        "CAACAgIAAxUAAWj-ZolmPZFTqhyNqwssS4JVQY_AAALgFAACU7NBSCIDa2YqXjXyNgQ",
        "CAACAgIAAxUAAWj-ZonZTWGW2DadfQ2Mo6bHAAHy2AACjxEAAgSTSUj1H3gU_UUHdjYE",
        "CAACAgIAAxUAAWj-ZolQ6OCfECavW19ATgcCup5PAAIOFgACgbdJSMOkkJfpAbs_NgQ",
        "CAACAgIAAxUAAWj-Zol19ilXmGth6SKa-4FRrSEJAAJRFwACM9JISKFYdRXvbsb1NgQ",
        "CAACAgIAAxUAAWj-ZokRA50GUCiz_OXQUih3uljfAAIeGQACsyBISDP8m_5FL5CJNgQ",
        "CAACAgIAAxUAAWj-ZomiM5Mt2aK1G3b8O7JK-shMAALPFQACWGhoSMeITTonc71ENgQ",
        "CAACAgIAAxUAAWj-ZomSF9AsKZr6myR3lYgyc-HyAAIRGQACM9KRSG5IUy40KB2KNgQ",
    ];

    // Генерируем случайный индекс используя настоящий генератор случайных чисел
    // Используем rand для лучшего разнообразия (timestamp может быть одинаковым для быстрых отправок)
    let random_index = rand::thread_rng().gen_range(0..sticker_file_ids.len());
    let random_sticker_id = sticker_file_ids[random_index];

    // Отправляем случайный стикер
    if let Err(e) = bot
        .send_sticker(
            chat_id,
            InputFile::file_id(teloxide::types::FileId(random_sticker_id.to_string())),
        )
        .await
    {
        log::error!("Failed to send error sticker: {}", e);
    }

    // Отправляем сообщение об ошибке
    let error_text =
        custom_message.unwrap_or("У меня не получилось, все сломалось 😢 Я написала Стэну");
    if let Err(e) = bot.send_message(chat_id, error_text).await {
        log::error!("Failed to send error message: {}", e);
    }
}

fn spawn_downloader_with_fallback(
    ytdl_bin: &str,
    args: &[&str],
) -> Result<std::process::Child, AppError> {
    Command::new(ytdl_bin)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .or_else(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                let fallback = "youtube-dl";
                Command::new(fallback)
                    .args(args)
                    .stdin(Stdio::null())
                    .stdout(Stdio::inherit())
                    .stderr(Stdio::inherit())
                    .spawn()
                    .map_err(|inner| {
                        AppError::Download(format!(
                            "Failed to start downloader. Tried '{}', then '{}': {} / {}",
                            ytdl_bin, fallback, e, inner
                        ))
                    })
            } else {
                Err(AppError::Download(format!(
                    "Failed to start downloader '{}': {}",
                    ytdl_bin, e
                )))
            }
        })
}

/// Структура для хранения данных прогресса загрузки
#[derive(Debug, Clone)]
pub struct ProgressInfo {
    pub percent: u8,
    pub speed_mbs: Option<f64>,
    pub eta_seconds: Option<u64>,
    pub current_size: Option<u64>,
    pub total_size: Option<u64>,
}

/// Парсит прогресс из строки вывода yt-dlp
/// Пример: "[download]  45.2% of 10.00MiB at 500.00KiB/s ETA 00:10"
fn parse_progress(line: &str) -> Option<ProgressInfo> {
    // Проверяем базовые требования
    if !line.contains("[download]") {
        return None;
    }

    // Для отладки: логируем все строки с [download]
    if !line.contains("%") {
        // Это может быть другое сообщение, например "[download] Destination: ..."
        log::trace!("Download line without percent: {}", line);
        return None;
    }

    let mut percent = None;
    let mut speed_mbs = None;
    let mut eta_seconds = None;
    let mut current_size = None;
    let mut total_size = None;

    // Парсим процент
    let parts: Vec<&str> = line.split_whitespace().collect();
    for (i, part) in parts.iter().enumerate() {
        if part.ends_with('%') {
            if let Ok(p) = part.trim_end_matches('%').parse::<f32>() {
                percent = Some(p.min(100.0) as u8);
            }
        }

        // Парсим размер: "of 10.00MiB"
        if *part == "of" && i + 1 < parts.len() {
            if let Some(size_bytes) = parse_size(parts[i + 1]) {
                total_size = Some(size_bytes);
            }
        }

        // Парсим скорость: "at 500.00KiB/s" или "at 2.3MiB/s"
        if *part == "at" && i + 1 < parts.len() {
            if let Some(speed) = parse_size(parts[i + 1]) {
                // Конвертируем в MB/s
                speed_mbs = Some(speed as f64 / (1024.0 * 1024.0));
            }
        }

        // Парсим ETA: "ETA 00:10" или "ETA 1:23"
        if *part == "ETA" && i + 1 < parts.len() {
            if let Some(eta) = parse_eta(parts[i + 1]) {
                eta_seconds = Some(eta);
            }
        }
    }

    // Если есть процент, возвращаем ProgressInfo
    if let Some(p) = percent {
        // Вычисляем текущий размер на основе процента
        if let Some(total) = total_size {
            current_size = Some((total as f64 * (p as f64 / 100.0)) as u64);
        }

        log::debug!(
            "Progress parsed successfully: {}% (speed: {:?} MB/s, eta: {:?}s)",
            p,
            speed_mbs,
            eta_seconds
        );

        Some(ProgressInfo {
            percent: p,
            speed_mbs,
            eta_seconds,
            current_size,
            total_size,
        })
    } else {
        log::debug!("Could not parse percent from line: {}", line);
        None
    }
}

/// Парсит размер из строки типа "10.00MiB" или "500.00KiB"
fn parse_size(size_str: &str) -> Option<u64> {
    let size_str = size_str.trim_end_matches("/s"); // Убираем "/s" если есть
    if size_str.ends_with("MiB") {
        if let Ok(mb) = size_str.trim_end_matches("MiB").parse::<f64>() {
            return Some((mb * 1024.0 * 1024.0) as u64);
        }
    } else if size_str.ends_with("KiB") {
        if let Ok(kb) = size_str.trim_end_matches("KiB").parse::<f64>() {
            return Some((kb * 1024.0) as u64);
        }
    } else if size_str.ends_with("GiB") {
        if let Ok(gb) = size_str.trim_end_matches("GiB").parse::<f64>() {
            return Some((gb * 1024.0 * 1024.0 * 1024.0) as u64);
        }
    }
    None
}

/// Парсит ETA из строки типа "00:10" или "1:23"
fn parse_eta(eta_str: &str) -> Option<u64> {
    let parts: Vec<&str> = eta_str.split(':').collect();
    if parts.len() == 2 {
        if let (Ok(minutes), Ok(seconds)) = (parts[0].parse::<u64>(), parts[1].parse::<u64>()) {
            return Some(minutes * 60 + seconds);
        }
    }
    None
}

#[allow(dead_code)]
fn download_audio_file(url: &Url, download_path: &str) -> Result<Option<u32>, AppError> {
    let ytdl_bin = &*config::YTDL_BIN;
    let args = [
        "-o",
        download_path,
        "--newline", // Выводить прогресс построчно (критично!)
        "--extract-audio",
        "--audio-format",
        "mp3",
        "--audio-quality",
        "0",
        "--add-metadata",
        "--embed-thumbnail",
        "--no-playlist",
        "--concurrent-fragments",
        "5",
        "--postprocessor-args",
        "-acodec libmp3lame -b:a 320k",
        url.as_str(),
    ];
    let mut child = spawn_downloader_with_fallback(&ytdl_bin, &args)?;
    let status = child
        .wait()
        .map_err(|e| AppError::Download(format!("downloader process failed: {}", e)))?;
    if !status.success() {
        return Err(AppError::Download(format!(
            "downloader exited with status: {}",
            status
        )));
    }
    Ok(probe_duration_seconds(download_path))
}

/// Скачивает аудио с отслеживанием прогресса через channel
async fn download_audio_file_with_progress(
    url: &Url,
    download_path: &str,
    bitrate: Option<String>,
) -> Result<
    (
        tokio::sync::mpsc::UnboundedReceiver<ProgressInfo>,
        tokio::task::JoinHandle<Result<Option<u32>, AppError>>,
    ),
    AppError,
> {
    let ytdl_bin = config::YTDL_BIN.clone();
    let url_str = url.to_string();
    let download_path_clone = download_path.to_string();
    let bitrate_str = bitrate.unwrap_or_else(|| "320k".to_string());

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

    // Запускаем в blocking task, так как читаем stdout построчно
    let handle = tokio::task::spawn_blocking(move || {
        let postprocessor_args = format!("-acodec libmp3lame -b:a {}", bitrate_str);

        // Строим аргументы с поддержкой cookies
        let mut args: Vec<&str> = vec![
            "-o",
            &download_path_clone,
            "--newline", // Выводить прогресс построчно
            "--extract-audio",
            "--audio-format",
            "mp3",
            "--audio-quality",
            "0",
            "--add-metadata",
            "--embed-thumbnail",
            "--no-playlist",
            "--concurrent-fragments",
            "5",
        ];
        add_cookies_args(&mut args);

        // НЕ используем android клиент!
        // YouTube изменил политику: теперь Android требует PO Token
        // Используем дефолтный web клиент который работает с cookies

        args.extend_from_slice(&[
            "--no-check-certificate", // Отключаем проверку сертификатов
            "--postprocessor-args",
            &postprocessor_args,
            &url_str,
        ]);

        // Логируем полную команду для отладки
        let command_str = format!("{} {}", ytdl_bin, args.join(" "));
        log::info!("[DEBUG] yt-dlp command for audio download: {}", command_str);

        let mut child = Command::new(&ytdl_bin)
            .args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| AppError::Download(format!("Failed to spawn yt-dlp: {}", e)))?;

        // Читаем stdout и stderr построчно для отслеживания прогресса
        // Прогресс может быть как в stdout, так и в stderr
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        // Собираем stderr для анализа ошибок
        let stderr_lines = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));

        // Объединяем оба потока
        use std::thread;
        let tx_clone = tx.clone();
        let stderr_lines_clone = Arc::clone(&stderr_lines);

        if let Some(stderr_stream) = stderr {
            thread::spawn(move || {
                let reader = BufReader::new(stderr_stream);
                for line in reader.lines() {
                    if let Ok(line_str) = line {
                        log::debug!("yt-dlp stderr: {}", line_str);
                        // Сохраняем строку для анализа ошибок
                        if let Ok(mut lines) = stderr_lines_clone.lock() {
                            lines.push(line_str.clone());
                        }
                        if let Some(progress_info) = parse_progress(&line_str) {
                            log::info!("Parsed progress from stderr: {}%", progress_info.percent);
                            let _ = tx_clone.send(progress_info);
                        }
                    }
                }
            });
        }

        if let Some(stdout_stream) = stdout {
            let reader = BufReader::new(stdout_stream);
            for line in reader.lines() {
                if let Ok(line_str) = line {
                    log::debug!("yt-dlp stdout: {}", line_str);
                    if let Some(progress_info) = parse_progress(&line_str) {
                        log::info!("Parsed progress from stdout: {}%", progress_info.percent);
                        let _ = tx.send(progress_info);
                    }
                }
            }
        }

        let status = child
            .wait()
            .map_err(|e| AppError::Download(format!("downloader process failed: {}", e)))?;

        if !status.success() {
            // Анализируем собранный stderr для определения типа ошибки
            let stderr_text = if let Ok(lines) = stderr_lines.lock() {
                lines.join("\n")
            } else {
                String::new()
            };

            if !stderr_text.is_empty() {
                let error_type = analyze_ytdlp_error(&stderr_text);

                // Логируем детальную информацию об ошибке
                log::error!("yt-dlp download failed, error type: {:?}", error_type);
                log::error!("yt-dlp stderr: {}", stderr_text);

                // Логируем рекомендации по исправлению
                let recommendations = get_fix_recommendations(&error_type);
                log::error!("{}", recommendations);

                // Если нужно уведомить администратора, логируем это
                if should_notify_admin(&error_type) {
                    log::warn!("⚠️  This error requires administrator attention!");
                }

                // Возвращаем пользовательское сообщение об ошибке
                return Err(AppError::Download(get_error_message(&error_type)));
            } else {
                return Err(AppError::Download(format!(
                    "downloader exited with status: {}",
                    status
                )));
            }
        }

        Ok(probe_duration_seconds(&download_path_clone))
    });

    Ok((rx, handle))
}

/// Скачивает видео с отслеживанием прогресса через channel
async fn download_video_file_with_progress(
    url: &Url,
    download_path: &str,
    format_arg: &str,
) -> Result<
    (
        tokio::sync::mpsc::UnboundedReceiver<ProgressInfo>,
        tokio::task::JoinHandle<Result<(), AppError>>,
    ),
    AppError,
> {
    let ytdl_bin = config::YTDL_BIN.clone();
    let url_str = url.to_string();
    let download_path_clone = download_path.to_string();
    let format_arg_clone = format_arg.to_string();

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

    // Запускаем в blocking task, так как читаем stdout построчно
    let handle = tokio::task::spawn_blocking(move || {
        // Строим аргументы с поддержкой cookies
        let mut args: Vec<&str> = vec![
            "-o",
            &download_path_clone,
            "--newline", // Выводить прогресс построчно
            "--format",
            &format_arg_clone,
            "--merge-output-format",
            "mp4",
            "--concurrent-fragments",
            "5",
            // Убеждаемся, что видео в совместимом формате для Telegram
            // Если видео уже в H.264/AAC - перекодирование не требуется (быстрее)
            // movflags +faststart делает видео готовым для streaming
            "--postprocessor-args",
            "ffmpeg:-movflags +faststart",
        ];
        add_cookies_args(&mut args);

        // НЕ используем android клиент для видео!
        // YouTube изменил политику: теперь Android требует PO Token для видео форматов
        // Используем дефолтный web клиент который работает с cookies
        // Если нужен android - требуется настройка PO Token: https://github.com/yt-dlp/yt-dlp/wiki/PO-Token-Guide

        args.extend_from_slice(&[
            "--no-check-certificate", // Отключаем проверку сертификатов
            &url_str,
        ]);

        // Логируем полную команду для отладки
        let command_str = format!("{} {}", ytdl_bin, args.join(" "));
        log::info!("[DEBUG] yt-dlp command for video download: {}", command_str);

        let mut child = Command::new(&ytdl_bin)
            .args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| AppError::Download(format!("Failed to spawn yt-dlp: {}", e)))?;

        // Читаем stdout и stderr построчно для отслеживания прогресса
        // Прогресс может быть как в stdout, так и в stderr
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        // Собираем stderr для анализа ошибок
        let stderr_lines = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));

        // Объединяем оба потока
        use std::thread;
        let tx_clone = tx.clone();
        let stderr_lines_clone = Arc::clone(&stderr_lines);

        if let Some(stderr_stream) = stderr {
            thread::spawn(move || {
                let reader = BufReader::new(stderr_stream);
                for line in reader.lines() {
                    if let Ok(line_str) = line {
                        log::debug!("yt-dlp stderr: {}", line_str);
                        // Сохраняем строку для анализа ошибок
                        if let Ok(mut lines) = stderr_lines_clone.lock() {
                            lines.push(line_str.clone());
                        }
                        if let Some(progress_info) = parse_progress(&line_str) {
                            log::info!("Parsed progress from stderr: {}%", progress_info.percent);
                            let _ = tx_clone.send(progress_info);
                        }
                    }
                }
            });
        }

        if let Some(stdout_stream) = stdout {
            let reader = BufReader::new(stdout_stream);
            for line in reader.lines() {
                if let Ok(line_str) = line {
                    log::debug!("yt-dlp stdout: {}", line_str);
                    if let Some(progress_info) = parse_progress(&line_str) {
                        log::info!("Parsed progress from stdout: {}%", progress_info.percent);
                        let _ = tx.send(progress_info);
                    }
                }
            }
        }

        let status = child
            .wait()
            .map_err(|e| AppError::Download(format!("downloader process failed: {}", e)))?;

        if !status.success() {
            // Анализируем собранный stderr для определения типа ошибки
            let stderr_text = if let Ok(lines) = stderr_lines.lock() {
                lines.join("\n")
            } else {
                String::new()
            };

            if !stderr_text.is_empty() {
                let error_type = analyze_ytdlp_error(&stderr_text);

                // Логируем детальную информацию об ошибке
                log::error!("yt-dlp download failed, error type: {:?}", error_type);
                log::error!("yt-dlp stderr: {}", stderr_text);

                // Логируем рекомендации по исправлению
                let recommendations = get_fix_recommendations(&error_type);
                log::error!("{}", recommendations);

                // Если нужно уведомить администратора, логируем это
                if should_notify_admin(&error_type) {
                    log::warn!("⚠️  This error requires administrator attention!");
                }

                // Возвращаем пользовательское сообщение об ошибке
                return Err(AppError::Download(get_error_message(&error_type)));
            } else {
                return Err(AppError::Download(format!(
                    "downloader exited with status: {}",
                    status
                )));
            }
        }

        Ok(())
    });

    Ok((rx, handle))
}

/// Download audio file and send it to user
///
/// Downloads audio from URL using yt-dlp, shows progress updates, validates file size,
/// and sends the file to the user via Telegram.
///
/// # Arguments
///
/// * `bot` - Telegram bot instance
/// * `chat_id` - User's chat ID
/// * `url` - URL to download from
/// * `rate_limiter` - Rate limiter instance (unused but kept for API consistency)
/// * `_created_timestamp` - Timestamp when task was created (unused)
///
/// # Returns
///
/// Returns `Ok(())` on success or a `ResponseResult` error.
///
/// # Behavior
///
/// 1. Fetches metadata (title, artist) from yt-dlp
/// 2. Shows starting status message
/// 3. Downloads audio with real-time progress updates
/// 4. Validates file size (max 49 MB)
/// 5. Sends audio file with retry logic
/// 6. Shows success message
/// 7. Cleans up temporary file after delay
pub async fn download_and_send_audio(
    bot: Bot,
    chat_id: ChatId,
    url: Url,
    rate_limiter: Arc<RateLimiter>,
    _created_timestamp: DateTime<Utc>,
    db_pool: Option<Arc<DbPool>>,
    audio_bitrate: Option<String>,
    message_id: Option<i32>,
) -> ResponseResult<()> {
    log::info!(
        "Starting download_and_send_audio for chat {} with URL: {}",
        chat_id,
        url
    );
    let bot_clone = bot.clone();
    let _rate_limiter = Arc::clone(&rate_limiter);
    let db_pool_clone = db_pool.clone();

    tokio::spawn(async move {
        log::info!("Inside spawn for audio download, chat_id: {}", chat_id);
        let mut progress_msg = ProgressMessage::new(chat_id);
        let start_time = std::time::Instant::now();

        let result: Result<(), AppError> = async {
            // Step 1: Get metadata and show starting status
            let (title, artist) = match get_metadata_from_ytdlp(&url).await {
                Ok(meta) => meta,
                Err(e) => {
                    log::error!("Failed to get metadata: {:?}", e);
                    // Проверяем, является ли это ошибкой таймаута
                    if e.to_string().contains("timed out") {
                        log::warn!("yt-dlp timed out, sending error message to user");
                        send_error_with_sticker(&bot_clone, chat_id).await;
                    }
                    return Err(e);
                }
            };

            let display_title: Arc<str> = if artist.trim().is_empty() {
                Arc::from(title.as_str())
            } else {
                Arc::from(format!("{} - {}", artist, title))
            };

            // Show starting status
            let _ = progress_msg.update(&bot_clone, DownloadStatus::Starting {
                title: display_title.as_ref().to_string(),
                file_format: Some("mp3".to_string()),
            }).await;

            let file_name = generate_file_name(&title, &artist);
            let safe_filename = escape_filename(&file_name);
            let full_path = format!("{}/{}", &*config::DOWNLOAD_FOLDER, safe_filename);
            let download_path = shellexpand::tilde(&full_path).into_owned();

            // Step 2: Download with real-time progress updates
            let (mut progress_rx, mut download_handle) = download_audio_file_with_progress(&url, &download_path, audio_bitrate.clone()).await?;

            // Показываем начальный прогресс 0%
            let _ = progress_msg.update(&bot_clone, DownloadStatus::Downloading {
                title: display_title.as_ref().to_string(),
                progress: 0,
                speed_mbs: None,
                eta_seconds: None,
                current_size: None,
                total_size: None,
                file_format: Some("mp3".to_string()),
            }).await;

            // Читаем обновления прогресса из channel
            let bot_for_progress = bot_clone.clone();
            let title_for_progress = Arc::clone(&display_title);
            let mut last_progress = 0u8;

            let duration_result = loop {
                tokio::select! {
                    // Получаем обновления прогресса
                    Some(progress_info) = progress_rx.recv() => {
                        // Обновляем при значимых изменениях (разница >= 5%)
                        let progress_diff = if progress_info.percent >= last_progress {
                            progress_info.percent - last_progress
                        } else {
                            progress_info.percent
                        };

                        if progress_diff >= 5 {
                            last_progress = progress_info.percent;
                            log::info!("Updating progress UI: {}%", progress_info.percent);
                            let _ = progress_msg.update(&bot_for_progress, DownloadStatus::Downloading {
                                title: title_for_progress.as_ref().to_string(),
                                progress: progress_info.percent,
                                speed_mbs: progress_info.speed_mbs,
                                eta_seconds: progress_info.eta_seconds,
                                current_size: progress_info.current_size,
                                total_size: progress_info.total_size,
                                file_format: Some("mp3".to_string()),
                            }).await;
                        }
                    }
                    // Ждем завершения загрузки
                    result = &mut download_handle => {
                        break result.map_err(|e| AppError::Download(format!("Task join error: {}", e)))??;
                    }
                }
            };

            log::debug!("Download path: {:?}", download_path);

            let duration: u32 = duration_result.unwrap_or(0);

            // Calculate elapsed time
            let elapsed_secs = start_time.elapsed().as_secs();
            log::info!("Audio downloaded in {} seconds", elapsed_secs);

            // Step 3: Validate file size before sending
            let file_size = fs::metadata(&download_path)
                .map_err(|e| AppError::Download(format!("Failed to get file metadata: {}", e)))?
                .len();

            let max_audio_size = config::validation::max_audio_size_bytes();
            if file_size > max_audio_size {
                let size_mb = file_size as f64 / (1024.0 * 1024.0);
                let max_mb = max_audio_size as f64 / (1024.0 * 1024.0);
                log::warn!("Audio file too large: {:.2} MB (max: {:.2} MB)", size_mb, max_mb);
                send_error_with_sticker(&bot_clone, chat_id).await;
                let _ = progress_msg.update(&bot_clone, DownloadStatus::Error {
                    title: display_title.as_ref().to_string(),
                    error: format!("Файл слишком большой ({:.2} MB). Максимальный размер: {:.2} MB", size_mb, max_mb),
                    file_format: Some("mp3".to_string()),
                }).await;
                return Err(AppError::Validation(format!("Файл слишком большой: {:.2} MB", size_mb)));
            }

            // Step 4: Get user preference for send_audio_as_document
            let send_audio_as_document = if let Some(ref pool) = db_pool_clone {
                match db::get_connection(pool) {
                    Ok(conn) => {
                        db::get_user_send_audio_as_document(&conn, chat_id.0).unwrap_or(0) == 1
                    }
                    Err(e) => {
                        log::warn!("Failed to get db connection for send_audio_as_document preference: {}", e);
                        false
                    }
                }
            } else {
                false
            };

            // Step 5: Send audio with retry logic and animation
            send_audio_with_retry(&bot_clone, chat_id, &download_path, duration, &mut progress_msg, display_title.as_ref(), send_audio_as_document).await?;

            // Save to download history after successful send
            if let Some(ref pool) = db_pool_clone {
                if let Ok(conn) = crate::storage::db::get_connection(pool) {
                    if let Err(e) = save_download_history(&conn, chat_id.0, url.as_str(), display_title.as_ref(), "mp3") {
                        log::warn!("Failed to save download history: {}", e);
                    }
                }
            }

            // Step 6: Show success status with time
            let _ = progress_msg.update(&bot_clone, DownloadStatus::Success {
                title: display_title.as_ref().to_string(),
                elapsed_secs,
                file_format: Some("mp3".to_string()),
            }).await;

            // Add eyes emoji reaction to the original message if message_id is available
            if let Some(msg_id) = message_id {
                use teloxide::types::{ReactionType, MessageId};
                let reaction = vec![ReactionType::Emoji {
                    emoji: "👀".to_string(),
                }];
                if let Err(e) = bot_clone.set_message_reaction(chat_id, MessageId(msg_id)).reaction(reaction).await {
                    log::warn!("Failed to set message reaction: {}", e);
                    // Not critical, continue
                }
            }

            log::info!("Audio sent successfully to chat {}", chat_id);

            // Step 5: Auto-clear success message after delay (оставляем только название)
            let bot_for_clear = bot_clone.clone();
            let title_for_clear = Arc::clone(&display_title);
            let mut msg_for_clear = ProgressMessage {
                chat_id: progress_msg.chat_id,
                message_id: progress_msg.message_id,
            };
            tokio::spawn(async move {
                let _ = msg_for_clear.clear_after(&bot_for_clear, config::progress::CLEAR_DELAY_SECS, title_for_clear.as_ref().to_string(), Some("mp3".to_string())).await;
            });

            // Wait before cleaning up file
            tokio::time::sleep(config::download::cleanup_delay()).await;
            if let Err(e) = fs::remove_file(&download_path) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    return Err(AppError::Download(format!("Failed to delete file: {}", e)))?;
                }
                // File doesn't exist - that's fine, it was probably deleted manually
            }

            Ok(())
        }.await;

        match result {
            Ok(_) => {
                log::info!("Audio download completed successfully for chat {}", chat_id);
            }
            Err(e) => {
                log::error!(
                    "An error occurred during audio download for chat {}: {:?}",
                    chat_id,
                    e
                );

                // Определяем тип ошибки и формируем полезное сообщение
                let error_str = e.to_string();
                let custom_message = if error_str.contains("Only images are available") {
                    Some(
                        "Это видео недоступно для скачивания 😢\n\n\
                    Возможные причины:\n\
                    • Видео удалено или приватное\n\
                    • Возрастные ограничения\n\
                    • Региональные ограничения\n\
                    • Стрим или премьера (еще не доступны)\n\n\
                    Попробуй другое видео!",
                    )
                } else if error_str.contains("Signature extraction failed") {
                    Some(
                        "У меня устарела версия загрузчика 😢\n\n\
                    Стэн уже знает и скоро обновит!\n\
                    Попробуй позже или другое видео.",
                    )
                } else if error_str.contains("Sign in to confirm you're not a bot")
                    || error_str.contains("bot detection")
                {
                    Some(
                        "YouTube заблокировал бота 🤖\n\n\
                    Нужно настроить cookies.\n\
                    Стэн уже знает и разбирается!\n\n\
                    Попробуй позже.",
                    )
                } else {
                    None
                };

                // Send error sticker and message
                send_error_with_sticker_and_message(&bot_clone, chat_id, custom_message).await;
                // Show error status
                let _ = progress_msg
                    .update(
                        &bot_clone,
                        DownloadStatus::Error {
                            title: "Скачивание".to_string(),
                            file_format: Some("mp3".to_string()),
                            error: e.to_string(),
                        },
                    )
                    .await;
            }
        }
    });
    log::info!("download_and_send_audio function returned, spawn task started");
    Ok(())
}

/// Generic function to send files with retry logic and animation
/// Args: bot - telegram bot instance, chat_id - user's chat ID, download_path - path to file, progress_msg - progress message handler, title - file title, file_type - type of file ("audio" or "video"), send_fn - closure that sends the file
/// Functionality: Sends file with retry logic, shows uploading animation, handles errors
async fn send_file_with_retry<F, Fut>(
    bot: &Bot,
    chat_id: ChatId,
    download_path: &str,
    progress_msg: &mut ProgressMessage,
    title: &str,
    file_type: &str,
    send_fn: F,
) -> Result<(), AppError>
where
    F: Fn(Bot, ChatId, String) -> Fut,
    Fut: std::future::Future<Output = ResponseResult<Message>>,
{
    let max_attempts = config::retry::MAX_ATTEMPTS;
    let download_path = download_path.to_string();

    // Validate file size before sending
    let file_size = fs::metadata(&download_path)
        .map_err(|e| AppError::Download(format!("Failed to get file metadata: {}", e)))?
        .len();

    let max_size = match file_type {
        "audio" => config::validation::max_audio_size_bytes(),
        "video" => config::validation::max_video_size_bytes(),
        _ => config::validation::MAX_FILE_SIZE_BYTES,
    };

    if file_size > max_size {
        let size_mb = file_size as f64 / (1024.0 * 1024.0);
        let max_mb = max_size as f64 / (1024.0 * 1024.0);
        log::warn!(
            "File {} too large: {:.2} MB (max: {:.2} MB)",
            download_path,
            size_mb,
            max_mb
        );
        return Err(AppError::Validation(format!(
            "Файл слишком большой ({:.2} MB). Максимальный размер: {:.2} MB",
            size_mb, max_mb
        )));
    }

    // Send chat action "Uploading document..." before sending file
    use teloxide::types::ChatAction;
    if let Err(e) = bot
        .send_chat_action(chat_id, ChatAction::UploadDocument)
        .await
    {
        log::warn!("Failed to send chat action: {}", e);
        // Not critical, continue with file upload
    }

    for attempt in 1..=max_attempts {
        log::info!(
            "Attempting to send {} to chat {} (attempt {}/{})",
            file_type,
            chat_id,
            attempt,
            max_attempts
        );

        // Запускаем анимацию точек в отдельной задаче
        let bot_clone = bot.clone();
        let title_clone = title.to_string();
        let mut msg_clone = ProgressMessage {
            chat_id: progress_msg.chat_id,
            message_id: progress_msg.message_id,
        };

        let file_size_clone = file_size;
        let file_type_clone = file_type.to_string();
        let upload_start = std::time::Instant::now();
        let bot_for_action = bot.clone();
        let progress_handle = tokio::spawn(async move {
            let mut update_count = 0u32;
            let mut last_progress = 0u8;
            let mut last_eta = Option::<u64>::None;
            let mut consecutive_99_updates = 0u32;
            let mut last_action_time = std::time::Instant::now();

            loop {
                let elapsed = upload_start.elapsed();
                let elapsed_secs = elapsed.as_secs();

                // Отправляем ChatAction каждые 4 секунды для поддержания статуса "uploading"
                // Telegram показывает ChatAction только 5 секунд, поэтому нужно повторять
                if last_action_time.elapsed().as_secs() >= 4 {
                    if let Err(e) = bot_for_action
                        .send_chat_action(chat_id, ChatAction::UploadDocument)
                        .await
                    {
                        log::debug!("Failed to send chat action during upload: {}", e);
                        // Не критично, продолжаем
                    }
                    last_action_time = std::time::Instant::now();
                }

                // Рассчитываем примерный прогресс на основе времени и размера файла
                // Предполагаем среднюю скорость отправки: 5-10 MB/s для больших файлов, 10-20 MB/s для маленьких
                let estimated_speed_mbps = if file_size_clone > 50 * 1024 * 1024 {
                    // Для больших файлов (>50MB) - медленнее
                    5.0 + (update_count as f64 * 0.1).min(5.0) // от 5 до 10 MB/s
                } else {
                    // Для маленьких файлов - быстрее
                    10.0 + (update_count as f64 * 0.2).min(10.0) // от 10 до 20 MB/s
                };

                let estimated_uploaded =
                    (estimated_speed_mbps * 1024.0 * 1024.0 * elapsed_secs as f64) as u64;
                let progress = if estimated_uploaded >= file_size_clone {
                    99 // Максимум 99% пока не завершится реальная отправка
                } else {
                    ((estimated_uploaded as f64 / file_size_clone as f64) * 100.0) as u8
                };

                // Рассчитываем ETA
                let remaining_bytes = file_size_clone.saturating_sub(estimated_uploaded);
                let eta_seconds = if estimated_speed_mbps > 0.0 && remaining_bytes > 0 {
                    Some((remaining_bytes as f64 / (estimated_speed_mbps * 1024.0 * 1024.0)) as u64)
                } else {
                    None
                };

                // Проверяем, изменился ли прогресс или ETA
                let progress_changed = progress != last_progress;
                let eta_changed = eta_seconds != last_eta;

                // Если прогресс достиг 99% и не меняется - не обновляем так часто
                if progress >= 99 {
                    consecutive_99_updates += 1;
                    // После 3 обновлений на 99% - обновляем только раз в 5 секунд
                    if consecutive_99_updates > 3 && !progress_changed && !eta_changed {
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        continue;
                    }
                } else {
                    consecutive_99_updates = 0;
                }

                // Обновляем UI только если прогресс или ETA изменились, или это первое обновление
                if progress_changed || eta_changed || update_count == 0 {
                    // Определяем формат файла на основе file_type
                    let file_format = match file_type_clone.as_str() {
                        "video" => Some("mp4".to_string()),
                        "audio" => Some("mp3".to_string()),
                        _ => None,
                    };

                    let _ = msg_clone
                        .update(
                            &bot_clone,
                            DownloadStatus::Uploading {
                                title: title_clone.clone(),
                                dots: 0, // Не используем точки, используем прогресс
                                progress: Some(progress.min(99)), // Не показываем 100% пока не завершится
                                eta_seconds,
                                current_size: Some(estimated_uploaded.min(file_size_clone)),
                                total_size: Some(file_size_clone),
                                file_format,
                            },
                        )
                        .await;

                    last_progress = progress;
                    last_eta = eta_seconds;
                }

                update_count += 1;

                // Если прошло слишком много времени, замедляем обновления
                if elapsed_secs > 600 {
                    // Ждем дольше перед следующим обновлением
                    tokio::time::sleep(Duration::from_secs(5)).await;
                } else if progress >= 99 && consecutive_99_updates > 3 {
                    // Если прогресс 99% и уже было несколько обновлений - обновляем реже
                    tokio::time::sleep(Duration::from_secs(2)).await;
                } else {
                    tokio::time::sleep(config::animation::update_interval()).await;
                }
            }
        });

        let response = send_fn(bot.clone(), chat_id, download_path.clone()).await;

        // Останавливаем отслеживание прогресса
        progress_handle.abort();

        // Небольшая задержка, чтобы убедиться, что анимация точно остановилась
        tokio::time::sleep(config::animation::stop_delay()).await;

        match response {
            Ok(_) => {
                log::info!(
                    "Successfully sent {} to chat {} on attempt {}",
                    file_type,
                    chat_id,
                    attempt
                );
                return Ok(());
            }
            Err(e) if attempt < max_attempts => {
                let error_str = e.to_string();

                // Проверяем rate limiting
                if let Some(retry_after_secs) = extract_retry_after(&error_str) {
                    log::warn!(
                        "Rate limit hit when sending {} to chat {}: Retry after {}s. Waiting...",
                        file_type,
                        chat_id,
                        retry_after_secs
                    );
                    // Ждем указанное время + небольшая задержка для надежности
                    tokio::time::sleep(Duration::from_secs(retry_after_secs + 1)).await;
                    // Продолжаем цикл для повторной попытки
                    continue;
                }

                // Проверяем, не является ли это ошибкой таймаута
                // Если это timeout или network error, возможно файл уже отправлен
                let is_timeout_or_network = error_str.contains("timeout")
                    || error_str.contains("network error")
                    || error_str.contains("error sending request");

                if is_timeout_or_network {
                    log::warn!("Attempt {}/{} failed for chat {} with timeout/network error: {}. This may indicate the file was actually sent but response timed out. Will retry once more to confirm.",
                        attempt, max_attempts, chat_id, e);
                    // Для timeout/network ошибок делаем более длинную задержку
                    tokio::time::sleep(Duration::from_secs(5)).await;
                } else {
                    log::warn!(
                        "Attempt {}/{} failed for chat {}: {}. Retrying...",
                        attempt,
                        max_attempts,
                        chat_id,
                        e
                    );
                    tokio::time::sleep(config::retry::delay()).await;
                }
            }
            Err(e) => {
                log::error!(
                    "All {} attempts failed to send {} to chat {}: {}",
                    max_attempts,
                    file_type,
                    chat_id,
                    e
                );
                let error_msg = match file_type {
                    "video" => format!("У меня не получилось отправить тебе видео 🥲 попробуй как-нибудь позже. Все {} попытки не удались: {}", max_attempts, e),
                    _ => format!("Failed to send {} file after {} attempts: {}", file_type, max_attempts, e.to_string()),
                };
                return Err(AppError::Download(error_msg));
            }
        }
    }

    unreachable!()
}

/// Send audio file with retry logic
/// Args: bot - telegram bot instance, chat_id - user's chat ID, download_path - path to audio file, duration - audio duration in seconds, progress_msg - progress message handler, title - audio title
/// Functionality: Wrapper around send_file_with_retry for audio files
async fn send_audio_with_retry(
    bot: &Bot,
    chat_id: ChatId,
    download_path: &str,
    duration: u32,
    progress_msg: &mut ProgressMessage,
    title: &str,
    send_as_document: bool,
) -> Result<(), AppError> {
    let duration = duration; // Capture duration for closure

    if send_as_document {
        log::info!("User preference: sending audio as document");
        send_file_with_retry(
            bot,
            chat_id,
            download_path,
            progress_msg,
            title,
            "audio",
            move |bot, chat_id, path| async move {
                bot.send_document(chat_id, InputFile::file(path)).await
            },
        )
        .await
    } else {
        send_file_with_retry(
            bot,
            chat_id,
            download_path,
            progress_msg,
            title,
            "audio",
            move |bot, chat_id, path| {
                let duration = duration;
                async move {
                    bot.send_audio(chat_id, InputFile::file(path))
                        .duration(duration)
                        .await
                }
            },
        )
        .await
    }
}

/// Send video file with retry logic and fallback to send_document for large files
///
/// Args:
/// - bot: Telegram bot instance
/// - chat_id: User's chat ID
/// - download_path: Path to video file
/// - progress_msg: Progress message handler
/// - title: Video title
///
/// Functionality:
/// - Tries to send as video (send_video) with metadata
/// - If file > 50 MB and send_video fails, falls back to send_document
/// - Uses send_file_with_retry for retry logic
/// - Optionally includes thumbnail preview image
async fn send_video_with_retry(
    bot: &Bot,
    chat_id: ChatId,
    download_path: &str,
    progress_msg: &mut ProgressMessage,
    title: &str,
    thumbnail_url: Option<&str>,
    send_as_document: bool,
) -> Result<(), AppError> {
    // Получаем метаданные видео для корректной отправки в Telegram
    let video_metadata = probe_video_metadata(download_path);

    log::info!("Video metadata for {}: {:?}", download_path, video_metadata);

    let duration = video_metadata.map(|(d, _, _)| d);
    let width = video_metadata.and_then(|(_, w, _)| w);
    let height = video_metadata.and_then(|(_, _, h)| h);

    // Проверяем размер файла
    let file_size = fs::metadata(download_path)
        .map_err(|e| AppError::Download(format!("Failed to get file metadata: {}", e)))?
        .len();

    let standard_limit = 50 * 1024 * 1024; // 50 MB - стандартный лимит для send_video
    let use_document_fallback = file_size > standard_limit || send_as_document;

    if send_as_document {
        log::info!("User preference: sending video as document");
    } else if use_document_fallback {
        log::info!("File size ({:.2} MB) exceeds standard send_video limit (50 MB), will use send_document fallback",
            file_size as f64 / (1024.0 * 1024.0));
    }

    // Скачиваем thumbnail если доступен, иначе генерируем из видео
    let thumbnail_bytes = if let Some(thumb_url) = thumbnail_url {
        log::info!(
            "[THUMBNAIL] Starting thumbnail download from URL: {}",
            thumb_url
        );
        match reqwest::get(thumb_url).await {
            Ok(response) => {
                log::info!(
                    "[THUMBNAIL] Thumbnail HTTP response status: {}",
                    response.status()
                );

                // Проверяем Content-Type
                if let Some(content_type) = response.headers().get("content-type") {
                    let content_type_str = content_type.to_str().unwrap_or("unknown");
                    log::info!("[THUMBNAIL] Thumbnail Content-Type: {}", content_type_str);
                }

                if response.status().is_success() {
                    match response.bytes().await {
                        Ok(bytes) => {
                            let bytes_vec = bytes.to_vec();
                            log::info!(
                                "[THUMBNAIL] Successfully downloaded thumbnail: {} bytes ({} KB)",
                                bytes_vec.len(),
                                bytes_vec.len() as f64 / 1024.0
                            );

                            // Проверяем формат файла по магическим байтам (magic bytes)
                            let format = detect_image_format(&bytes_vec);
                            log::info!("[THUMBNAIL] Detected image format: {:?}", format);

                            // Проверяем размер (Telegram требует <= 200 KB)
                            if bytes_vec.len() > 200 * 1024 {
                                log::warn!("[THUMBNAIL] Thumbnail size ({} KB) exceeds Telegram limit (200 KB). May cause issues.",
                                    bytes_vec.len() as f64 / 1024.0);
                            }

                            // Проверяем формат (Telegram требует JPEG или PNG)
                            match format {
                                ImageFormat::Jpeg | ImageFormat::Png => {
                                    log::info!("[THUMBNAIL] Thumbnail format is valid (JPEG/PNG), will use it");
                                    Some(bytes_vec)
                                }
                                ImageFormat::WebP => {
                                    log::warn!("[THUMBNAIL] Thumbnail is WebP format, Telegram may not support it properly. Trying anyway...");
                                    Some(bytes_vec)
                                }
                                ImageFormat::Unknown => {
                                    log::warn!("[THUMBNAIL] Unknown thumbnail format, may cause black screen. First bytes: {:?}",
                                        bytes_vec.iter().take(10).collect::<Vec<_>>());
                                    Some(bytes_vec)
                                }
                            }
                        }
                        Err(e) => {
                            log::warn!("[THUMBNAIL] Failed to get thumbnail bytes: {}", e);
                            None
                        }
                    }
                } else {
                    log::warn!(
                        "[THUMBNAIL] Thumbnail request failed with status: {}",
                        response.status()
                    );
                    None
                }
            }
            Err(e) => {
                log::warn!("[THUMBNAIL] Failed to download thumbnail: {}", e);
                None
            }
        }
    } else {
        log::info!("[THUMBNAIL] No thumbnail URL provided");
        None
    };

    // Если thumbnail из URL недоступен, генерируем из видео
    let thumbnail_bytes = thumbnail_bytes.or_else(|| {
        log::info!("[THUMBNAIL] Thumbnail URL not available, trying to generate from video file");
        generate_thumbnail_from_video(download_path)
    });

    // Создаем временный файл для thumbnail если он доступен
    // Это нужно для правильной передачи thumbnail в Telegram с именем файла
    // Конвертируем WebP в JPEG если нужно, так как Telegram лучше работает с JPEG
    let temp_thumb_path: Option<std::path::PathBuf> = if let Some(ref thumb_bytes) = thumbnail_bytes
    {
        let format = detect_image_format(thumb_bytes);

        // Конвертируем WebP в JPEG если нужно (Telegram лучше работает с JPEG)
        let (final_bytes, file_ext) = if format == ImageFormat::WebP {
            log::info!(
                "[THUMBNAIL] Converting WebP thumbnail to JPEG for better Telegram compatibility"
            );
            // Попробуем использовать ffmpeg для конвертации WebP в JPEG
            match convert_webp_to_jpeg(thumb_bytes) {
                Ok(jpeg_bytes) => {
                    log::info!(
                        "[THUMBNAIL] Successfully converted WebP to JPEG: {} bytes",
                        jpeg_bytes.len()
                    );
                    (jpeg_bytes, "jpg")
                }
                Err(e) => {
                    log::warn!(
                        "[THUMBNAIL] Failed to convert WebP to JPEG: {}. Using original.",
                        e
                    );
                    (thumb_bytes.clone(), "webp")
                }
            }
        } else {
            let ext = match format {
                ImageFormat::Jpeg => "jpg",
                ImageFormat::Png => "png",
                ImageFormat::Unknown => "jpg",
                _ => "jpg",
            };
            (thumb_bytes.clone(), ext)
        };

        // Проверяем размер - если больше 200KB, сжимаем
        let final_bytes = if final_bytes.len() > 200 * 1024 {
            log::warn!(
                "[THUMBNAIL] Thumbnail too large ({} KB), trying to compress",
                final_bytes.len() as f64 / 1024.0
            );
            compress_thumbnail_jpeg(&final_bytes).unwrap_or(final_bytes)
        } else {
            final_bytes
        };

        let temp_dir = std::env::temp_dir();
        let temp_path = temp_dir.join(format!(
            "thumb_{}.{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis(),
            file_ext
        ));

        // Получаем абсолютный путь (canonicalize работает только для существующих файлов)
        let abs_path = if temp_path.exists() {
            temp_path
                .canonicalize()
                .unwrap_or_else(|_| temp_path.clone())
        } else {
            // Если файл еще не создан, получаем абсолютный путь через parent
            temp_dir
                .canonicalize()
                .map(|canon_dir| canon_dir.join(temp_path.file_name().unwrap_or_default()))
                .unwrap_or_else(|_| temp_path.clone())
        };

        if fs::write(&abs_path, &final_bytes).is_ok() {
            log::info!(
                "[THUMBNAIL] Saved thumbnail to temporary file: {:?} ({} bytes)",
                abs_path,
                final_bytes.len()
            );
            Some(abs_path)
        } else {
            log::warn!("[THUMBNAIL] Failed to save thumbnail to temporary file");
            None
        }
    } else {
        None
    };

    // Клонируем значения для использования в замыкании
    let duration_clone = duration;
    // Если пользователь выбрал отправку как document, сразу отправляем как document
    if send_as_document {
        log::info!("User preference: sending video as document (skip send_video)");
        return send_file_with_retry(
            bot,
            chat_id,
            download_path,
            progress_msg,
            title,
            "video",
            |bot, chat_id, path| async move {
                bot.send_document(chat_id, InputFile::file(path)).await
            },
        ).await;
    }

    let width_clone = width;
    let height_clone = height;
    let thumbnail_bytes_clone = thumbnail_bytes.clone();
    let temp_thumb_path_clone = temp_thumb_path.clone();

    // Пробуем отправить как видео
    let result = send_file_with_retry(
        bot,
        chat_id,
        download_path,
        progress_msg,
        title,
        "video",
        move |bot, chat_id, path| {
            let duration_clone = duration_clone;
            let width_clone = width_clone;
            let height_clone = height_clone;
            let thumbnail_bytes_clone = thumbnail_bytes_clone.clone();
            let temp_thumb_path_clone = temp_thumb_path_clone.clone();

            async move {
                let mut video_msg = bot.send_video(chat_id, InputFile::file(path));

                // Добавляем метаданные для корректного воспроизведения в Telegram
                if let Some(dur) = duration_clone {
                    video_msg = video_msg.duration(dur);
                }
                if let Some(w) = width_clone {
                    video_msg = video_msg.width(w);
                }
                if let Some(h) = height_clone {
                    video_msg = video_msg.height(h);
                }

                // Добавляем thumbnail если доступен
                // ВАЖНО: Используем абсолютный путь и убеждаемся, что файл существует
                if let Some(thumb_path) = temp_thumb_path_clone {
                    // Проверяем, что файл существует перед отправкой
                    if thumb_path.exists() {
                        let abs_path_str = thumb_path.to_str().unwrap_or("thumb.jpg");
                        log::info!("[THUMBNAIL] Adding thumbnail from file: {} (exists: {}, size: {} bytes)",
                            abs_path_str,
                            thumb_path.exists(),
                            fs::metadata(&thumb_path).map(|m| m.len()).unwrap_or(0));
                        video_msg = video_msg.thumbnail(InputFile::file(abs_path_str));
                        log::info!("[THUMBNAIL] Thumbnail successfully added to video message");
                    } else {
                        log::warn!("[THUMBNAIL] Thumbnail file does not exist: {:?}, trying memory fallback", thumb_path);
                        // Fallback на memory если файл не существует
                if let Some(thumb_bytes) = thumbnail_bytes_clone {
                            log::info!("[THUMBNAIL] Adding thumbnail from memory: {} bytes", thumb_bytes.len());
                            video_msg = video_msg.thumbnail(InputFile::memory(thumb_bytes));
                        }
                    }
                } else if let Some(thumb_bytes) = thumbnail_bytes_clone {
                    log::info!("[THUMBNAIL] Adding thumbnail from memory: {} bytes", thumb_bytes.len());
                    // Fallback на InputFile::memory если временный файл не создан
                    video_msg = video_msg.thumbnail(InputFile::memory(thumb_bytes));
                    log::info!("[THUMBNAIL] Thumbnail successfully added to video message");
                } else {
                    log::info!("[THUMBNAIL] No thumbnail bytes available, sending video without thumbnail");
                }

                // Включаем поддержку streaming для лучшей совместимости
                video_msg = video_msg.supports_streaming(true);

                video_msg.await
            }
        },
    ).await;

    // Удаляем временный файл thumbnail после успешной отправки
    // Добавляем небольшую задержку, чтобы teloxide успел прочитать файл
    if let Some(thumb_path) = temp_thumb_path {
        // Даем время teloxide прочитать файл перед удалением
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        if result.is_ok() {
            let _ = fs::remove_file(&thumb_path);
            log::info!(
                "[THUMBNAIL] Cleaned up temporary thumbnail file: {:?}",
                thumb_path
            );
        } else {
            // При ошибке тоже удаляем, так как retry создаст новый файл
            let _ = fs::remove_file(&thumb_path);
            log::info!(
                "[THUMBNAIL] Cleaned up temporary thumbnail file after error: {:?}",
                thumb_path
            );
        }
    }

    // Если отправка как видео не удалась и файл > 50 MB, пробуем как document
    if result.is_err() && use_document_fallback {
        log::info!("send_video failed, trying send_document as fallback for large file");
        return send_file_with_retry(
            bot,
            chat_id,
            download_path,
            progress_msg,
            title,
            "video",
            |bot, chat_id, path| async move {
                bot.send_document(chat_id, InputFile::file(path)).await
            },
        ).await;
    }

    result
}

/// Download video file and send it to user
///
/// Downloads video from URL using yt-dlp, shows progress updates, validates file size,
/// and sends the file to the user via Telegram.
///
/// # Arguments
///
/// * `bot` - Telegram bot instance
/// * `chat_id` - User's chat ID
/// * `url` - URL to download from
/// * `rate_limiter` - Rate limiter instance (unused but kept for API consistency)
/// * `_created_timestamp` - Timestamp when task was created (unused)
///
/// # Returns
///
/// Returns `Ok(())` on success or a `ResponseResult` error.
///
/// # Behavior
///
/// Similar to [`download_and_send_audio`], but for video files.
pub async fn download_and_send_video(
    bot: Bot,
    chat_id: ChatId,
    url: Url,
    rate_limiter: Arc<RateLimiter>,
    _created_timestamp: DateTime<Utc>,
    db_pool: Option<Arc<DbPool>>,
    video_quality: Option<String>,
    message_id: Option<i32>,
) -> ResponseResult<()> {
    let bot_clone = bot.clone();
    let _rate_limiter = Arc::clone(&rate_limiter);
    let db_pool_clone = db_pool.clone();

    tokio::spawn(async move {
        let mut progress_msg = ProgressMessage::new(chat_id);
        let start_time = std::time::Instant::now();

        let result: Result<(), AppError> = async {
            // Step 1: Get metadata and show starting status
            let (title, artist) = match get_metadata_from_ytdlp(&url).await {
                Ok(meta) => {
                    log::info!("Successfully got metadata for video - title: '{}', artist: '{}'", meta.0, meta.1);
                    meta
                },
                Err(e) => {
                    log::error!("Failed to get metadata for video from URL {}: {:?}", url, e);
                    // Проверяем, является ли это ошибкой таймаута
                    if e.to_string().contains("timed out") {
                        log::warn!("yt-dlp timed out, sending error message to user");
                        send_error_with_sticker(&bot_clone, chat_id).await;
                    }
                    return Err(e);
                }
            };

            // Получаем thumbnail URL для preview изображения
            log::info!("[THUMBNAIL] Starting to get thumbnail URL for video");
            let thumbnail_url = {
                let ytdl_bin = &*config::YTDL_BIN;
                let mut thumbnail_args: Vec<&str> = vec![
                    "--get-thumbnail",
                    "--no-playlist",
                    "--socket-timeout", "30",
                    "--retries", "2",
                ];
                add_cookies_args(&mut thumbnail_args);
                thumbnail_args.push(url.as_str());

                let command_str = format!("{} {}", ytdl_bin, thumbnail_args.join(" "));
                log::info!("[THUMBNAIL] yt-dlp command for thumbnail URL: {}", command_str);

                let thumbnail_output = timeout(
                    config::download::ytdlp_timeout(),
                    TokioCommand::new(ytdl_bin)
                        .args(&thumbnail_args)
                        .output()
                )
                .await
                .ok(); // Не критично, игнорируем ошибки

                let result = thumbnail_output
                    .and_then(|result| {
                        log::info!("[THUMBNAIL] yt-dlp thumbnail command completed");
                        result.ok()
                    })
                    .and_then(|out| {
                        log::info!("[THUMBNAIL] yt-dlp exit status: {:?}, stdout length: {}, stderr length: {}",
                            out.status, out.stdout.len(), out.stderr.len());

                        if !out.stderr.is_empty() {
                            let stderr_str = String::from_utf8_lossy(&out.stderr);
                            log::debug!("[THUMBNAIL] yt-dlp stderr: {}", stderr_str);
                        }

                        if out.status.success() {
                            let url_str = String::from_utf8_lossy(&out.stdout).trim().to_string();
                            log::info!("[THUMBNAIL] yt-dlp stdout (thumbnail URL): '{}'", url_str);
                            if url_str.is_empty() {
                                log::warn!("[THUMBNAIL] Thumbnail URL is empty");
                                None
                            } else {
                                Some(url_str)
                            }
                        } else {
                            log::warn!("[THUMBNAIL] yt-dlp failed to get thumbnail URL, exit status: {:?}", out.status);
                            None
                        }
                    });

                if result.is_none() {
                    log::warn!("[THUMBNAIL] Failed to get thumbnail URL from yt-dlp (timeout or error)");
                }

                result
            };

            if let Some(ref thumb_url) = thumbnail_url {
                log::info!("[THUMBNAIL] Successfully got thumbnail URL for video: {}", thumb_url);
            } else {
                log::warn!("[THUMBNAIL] Thumbnail URL not available for video - will send without thumbnail preview");
            }

            log::info!("Video metadata received - title length: {}, artist length: {}", title.len(), artist.len());

            let display_title: Arc<str> = if artist.trim().is_empty() {
                Arc::from(title.as_str())
            } else {
                Arc::from(format!("{} - {}", artist, title))
            };

            log::info!("Display title for video: '{}'", display_title);

            // Show starting status
            let _ = progress_msg.update(&bot_clone, DownloadStatus::Starting {
                title: display_title.as_ref().to_string(),
                file_format: Some("mp4".to_string()),
            }).await;

            // Добавляем уникальный идентификатор к имени файла для избежания конфликтов
            use std::time::{SystemTime, UNIX_EPOCH};
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0);

            let base_file_name = generate_file_name_with_ext(&title, &artist, "mp4");
            // Добавляем timestamp к имени файла (перед расширением)
            let file_name = if base_file_name.ends_with(".mp4") {
                format!("{}_{}.mp4",
                    base_file_name.trim_end_matches(".mp4"),
                    timestamp
                )
            } else {
                format!("{}_{}", base_file_name, timestamp)
            };

            log::info!("Generated filename for video: '{}' (base: '{}')", file_name, base_file_name);
            let safe_filename = escape_filename(&file_name);
            let full_path = format!("{}/{}", &*config::DOWNLOAD_FOLDER, safe_filename);
            let download_path = shellexpand::tilde(&full_path).into_owned();

            // Step 2: Determine video quality format with fallback chain
            // Используем best вместо bestvideo+bestaudio для избежания проблем с объединением
            // best выбирает готовое видео с аудио, если доступно
            // Добавляем fallback цепочку форматов для обработки случаев, когда запрашиваемый формат недоступен
            // (например, из-за проблем с nsig extraction или SABR streaming)
            // Синтаксис "format1/format2/format3" позволяет yt-dlp выбрать первый доступный формат
            let format_arg = match video_quality.as_deref() {
                Some("1080p") => "best[height<=1080]/best[height<=720]/best[height<=480]/best[height<=360]/best/worst",
                Some("720p") => "best[height<=720]/best[height<=480]/best[height<=360]/best/worst",
                Some("480p") => "best[height<=480]/best[height<=360]/best/worst",
                Some("360p") => "best[height<=360]/best/worst",
                _ => "best/worst", // best - готовое видео с аудио, worst - последний fallback
            };

            log::info!("Using video format with fallback chain: {}", format_arg);

            // Step 2.5: Check estimated file size before downloading
            // Пытаемся получить размер файла для выбранного формата
            // Проблема: YouTube часто возвращает "NA" для размера, и fallback цепочка может выбрать другой формат
            // Поэтому проверяем размер для первого формата в цепочке (без fallback)
            // Если размер недоступен или слишком большой - предупреждаем пользователя
            let ytdl_bin = &*config::YTDL_BIN;

            // Получаем первый формат из цепочки для проверки (без fallback)
            let first_format = match video_quality.as_deref() {
                Some("1080p") => "best[height<=1080]",
                Some("720p") => "best[height<=720]",
                Some("480p") => "best[height<=480]",
                Some("360p") => "best[height<=360]",
                _ => "best",
            };

            let mut size_check_args: Vec<String> = vec![
                "--print".to_string(),
                "%(filesize)s".to_string(),
                "--format".to_string(),
                first_format.to_string(),
                "--no-playlist".to_string(),
                "--skip-download".to_string(),
            ];

            let mut temp_args: Vec<&str> = vec![];
            add_cookies_args(&mut temp_args);
            for arg in temp_args {
                size_check_args.push(arg.to_string());
            }
            size_check_args.push(url.as_str().to_string());

            let size_check_cmd = format!("{} {}", ytdl_bin, size_check_args.join(" "));
            log::info!("[DEBUG] Checking file size before download (format: {}): {}", first_format, size_check_cmd);

            let size_check_output = timeout(
                config::download::ytdlp_timeout(),
                TokioCommand::new(ytdl_bin)
                    .args(&size_check_args)
                    .output()
            )
            .await;

            let mut size_available = false;
            if let Ok(Ok(output)) = size_check_output {
                if output.status.success() {
                    let size_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    if !size_str.is_empty() && size_str != "NA" {
                        if let Ok(file_size) = size_str.parse::<u64>() {
                            size_available = true;
                            let size_mb = file_size as f64 / (1024.0 * 1024.0);
                            let max_size = config::validation::max_video_size_bytes();
                            let max_mb = max_size as f64 / (1024.0 * 1024.0);

                            log::info!("Estimated video file size for {}: {:.2} MB (max: {:.2} MB)", first_format, size_mb, max_mb);

                            if file_size > max_size {
                                log::warn!("Video file too large (estimated): {:.2} MB (max: {:.2} MB)", size_mb, max_mb);
                                send_error_with_sticker(&bot_clone, chat_id).await;
                                let _ = progress_msg.update(&bot_clone, DownloadStatus::Error {
                                    title: display_title.as_ref().to_string(),
                                    error: format!("Видео слишком большое (примерно {:.2} MB). Максимальный размер: {:.2} MB.\n\nПопробуй выбрать меньшее качество (720p, 480p или 360p) в настройках.", size_mb, max_mb),
                                    file_format: Some("mp4".to_string()),
                                }).await;
                                return Err(AppError::Validation(format!("Видео слишком большое (примерно {:.2} MB). Максимальный размер: {:.2} MB. Попробуй выбрать меньшее качество.", size_mb, max_mb)));
                            }
                        }
                    }
                }
            }

            // Если размер недоступен (NA) - проверяем через --list-formats для получения точных размеров
            // YouTube часто не предоставляет размер через --print для объединенных форматов
            // Но через --list-formats мы можем увидеть размеры отдельных форматов
            if !size_available {
                log::info!("File size NA via --print, trying to get sizes via --list-formats");

                // Получаем список форматов с размерами
                let mut list_formats_args: Vec<String> = vec![
                    "--list-formats".to_string(),
                    "--no-playlist".to_string(),
                ];

                let mut temp_args: Vec<&str> = vec![];
                add_cookies_args(&mut temp_args);
                for arg in temp_args {
                    list_formats_args.push(arg.to_string());
                }
                list_formats_args.push(url.as_str().to_string());

                let list_formats_output = timeout(
                    Duration::from_secs(30), // Более короткий таймаут для списка форматов
                    TokioCommand::new(ytdl_bin)
                        .args(&list_formats_args)
                        .output()
                )
                .await;

                // Парсим вывод и ищем форматы с размерами для запрошенного качества
                if let Ok(Ok(output)) = list_formats_output {
                    if output.status.success() {
                        let formats_output = String::from_utf8_lossy(&output.stdout);

                        // Ищем размеры для форматов в зависимости от запрошенного качества
                        let target_height = match video_quality.as_deref() {
                            Some("1080p") => 1080,
                            Some("720p") => 720,
                            Some("480p") => 480,
                            Some("360p") => 360,
                            _ => 0,
                        };

                        if target_height > 0 {
                            // Парсим строки вида: "137     mp4   1920x1080   24    |  154.58MiB  1786k https"
                            for line in formats_output.lines() {
                                // Ищем строки с нужным разрешением
                                if line.contains(&format!("{}x{}", target_height, target_height)) ||
                                   (target_height == 1080 && line.contains("1920x1080")) ||
                                   (target_height == 720 && line.contains("1280x720")) ||
                                   (target_height == 480 && line.contains("854x480")) ||
                                   (target_height == 360 && line.contains("640x360")) {

                                    // Извлекаем размер (формат: ~XX.XXMiB или XX.XXMiB)
                                    if let Some(size_mb_pos) = line.find("MiB") {
                                        let before_size = &line[..size_mb_pos];
                                        if let Some(start) = before_size.rfind(|c: char| c.is_ascii_digit() || c == '.' || c == '~') {
                                            let size_str = &line[start..size_mb_pos].trim().trim_start_matches('~');
                                            if let Ok(size_mb) = size_str.parse::<f64>() {
                                                let size_bytes = (size_mb * 1024.0 * 1024.0) as u64;

                                                log::info!("Found format size via --list-formats: {:.2} MB for {}p", size_mb, target_height);

                                                let max_size = config::validation::max_video_size_bytes();
                                                if size_bytes > max_size {
                                                    let max_mb = max_size as f64 / (1024.0 * 1024.0);
                                                    log::warn!("Video format too large: {:.2} MB (max: {:.2} MB) for {}p", size_mb, max_mb, target_height);

                                                    send_error_with_sticker(&bot_clone, chat_id).await;
                                                    let _ = progress_msg.update(&bot_clone, DownloadStatus::Error {
                                                        title: display_title.as_ref().to_string(),
                                                        error: format!("Видео в качестве {}p слишком большое (примерно {:.2} MB). Максимальный размер: {:.2} MB.\n\nПопробуй выбрать меньшее качество (720p, 480p или 360p) в настройках.", target_height, size_mb, max_mb),
                                                        file_format: Some("mp4".to_string()),
                                                    }).await;
                                                    return Err(AppError::Validation(format!("Видео в качестве {}p слишком большое: {:.2} MB (максимум: {:.2} MB). Попробуй выбрать меньшее качество.", target_height, size_mb, max_mb)));
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // Если размер все еще недоступен - проверяем нужно ли блокировать
                // НО: не блокируем если используется локальный Bot API сервер (лимит 2 GB)
                let is_local_bot_api = std::env::var("BOT_API_URL")
                    .map(|url| !url.contains("api.telegram.org"))
                    .unwrap_or(false);

                if !is_local_bot_api {
                    // Для стандартного API блокируем 1080p если размер NA
                    match video_quality.as_deref() {
                        Some("1080p") => {
                            log::warn!("File size not available (NA) for 1080p quality. 1080p videos almost always exceed 50 MB limit.");
                            log::warn!("Blocking download for 1080p when size is unavailable.");

                            send_error_with_sticker(&bot_clone, chat_id).await;
                            let _ = progress_msg.update(&bot_clone, DownloadStatus::Error {
                                title: display_title.as_ref().to_string(),
                                error: "Видео в качестве 1080p обычно превышают лимит 50 MB.\n\nРазмер файла недоступен заранее, но опыт показывает, что 1080p видео почти всегда > 50 MB.\n\nПожалуйста, выбери меньшее качество (720p, 480p или 360p) в настройках.".to_string(),
                                file_format: Some("mp4".to_string()),
                            }).await;
                            return Err(AppError::Validation("1080p videos typically exceed 50 MB limit. Please choose lower quality.".to_string()));
                        },
                        Some("720p") => {
                            log::warn!("File size not available (NA) for 720p quality. Will proceed but may exceed 50 MB.");
                            log::info!("Will proceed with download, will check size after download.");
                        },
                        _ => {
                            log::info!("File size not available before download (NA), will check after download");
                        }
                    }
                } else {
                    // Для локального Bot API сервера - разрешаем все форматы, даже если размер NA
                    let quality_str = video_quality.as_deref().unwrap_or("unknown");
                    log::info!("File size not available (NA) for {} quality, but local Bot API server is used (2 GB limit). Proceeding with download.", quality_str);
                }
            }

            // Step 3: Download with real-time progress updates
            let (mut progress_rx, mut download_handle) = download_video_file_with_progress(&url, &download_path, format_arg).await?;

            // Показываем начальный прогресс 0%
            let _ = progress_msg.update(&bot_clone, DownloadStatus::Downloading {
                title: display_title.as_ref().to_string(),
                progress: 0,
                speed_mbs: None,
                eta_seconds: None,
                current_size: None,
                total_size: None,
                file_format: Some("mp4".to_string()),
            }).await;

            // Читаем обновления прогресса из channel
            let bot_for_progress = bot_clone.clone();
            let title_for_progress = Arc::clone(&display_title);
            let mut last_progress = 0u8;

            loop {
                tokio::select! {
                    // Получаем обновления прогресса
                    Some(progress_info) = progress_rx.recv() => {
                        log::debug!("Received progress update: {}% (speed: {:?} MB/s, eta: {:?}s, total_size: {:?})",
                            progress_info.percent, progress_info.speed_mbs, progress_info.eta_seconds, progress_info.total_size);

                        // Сначала обновляем UI, чтобы пользователь видел прогресс
                        // Обновляем при значимых изменениях (разница >= 5%)
                        let progress_diff = if progress_info.percent >= last_progress {
                            progress_info.percent - last_progress
                        } else {
                            progress_info.percent
                        };

                        if progress_diff >= 5 {
                            last_progress = progress_info.percent;
                            log::info!("Updating progress UI: {}%", progress_info.percent);
                            let _ = progress_msg.update(&bot_for_progress, DownloadStatus::Downloading {
                                title: title_for_progress.as_ref().to_string(),
                                progress: progress_info.percent,
                                speed_mbs: progress_info.speed_mbs,
                                eta_seconds: progress_info.eta_seconds,
                                current_size: progress_info.current_size,
                                total_size: progress_info.total_size,
                                file_format: Some("mp4".to_string()),
                            }).await;
                        }

                        // Проверяем размер файла во время скачивания ПОСЛЕ обновления UI
                        // Если total_size известен и превышает лимит - прерываем скачивание
                        if let Some(total_size) = progress_info.total_size {
                            let max_size = config::validation::max_video_size_bytes();
                            if total_size > max_size {
                                let size_mb = total_size as f64 / (1024.0 * 1024.0);
                                let max_mb = max_size as f64 / (1024.0 * 1024.0);

                                log::warn!("Video file too large during download (detected from progress): {:.2} MB (max: {:.2} MB)", size_mb, max_mb);
                                log::warn!("Stopping download to save bandwidth and time");

                                // Прерываем процесс скачивания (download_handle будет отменен при выходе)
                                send_error_with_sticker(&bot_clone, chat_id).await;
                                let _ = progress_msg.update(&bot_clone, DownloadStatus::Error {
                                    title: display_title.as_ref().to_string(),
                                    error: format!("Видео слишком большое (примерно {:.2} MB). Максимальный размер: {:.2} MB.\n\nПопробуй выбрать меньшее качество (720p, 480p или 360p) в настройках.", size_mb, max_mb),
                                    file_format: Some("mp4".to_string()),
                                }).await;

                                // Удаляем частично скачанный файл
                                if let Err(e) = fs::remove_file(&download_path) {
                                    log::warn!("Failed to remove partially downloaded file: {}", e);
                                }

                                return Err(AppError::Validation(format!("Видео слишком большое (обнаружено во время скачивания): {:.2} MB (максимум: {:.2} MB). Попробуй выбрать меньшее качество.", size_mb, max_mb)));
                            }
                        }
                    }
                    // Ждем завершения загрузки
                    result = &mut download_handle => {
                        result.map_err(|e| AppError::Download(format!("Task join error: {}", e)))??;
                        break;
                    }
                }
            }

            log::debug!("Download path: {:?}", download_path);

            // Calculate elapsed time
            let elapsed_secs = start_time.elapsed().as_secs();
            log::info!("Video downloaded in {} seconds", elapsed_secs);

            // Step 2.5: Find actual downloaded file (yt-dlp may add suffixes like (1).mp4)
            let actual_file_path = match find_actual_downloaded_file(&download_path) {
                Ok(path) => {
                    log::info!("Using actual downloaded file: {}", path);
                    path
                },
                Err(e) => {
                    log::error!("Failed to find actual downloaded file: {:?}", e);
                    return Err(e);
                }
            };

            // Step 3: Validate file size before sending
            let file_size = fs::metadata(&actual_file_path)
                .map_err(|e| AppError::Download(format!("Failed to get file metadata: {}", e)))?
                .len();

            log::info!("Downloaded video file size: {:.2} MB", file_size as f64 / (1024.0 * 1024.0));

            let max_size = config::validation::max_video_size_bytes();
            if file_size > max_size {
                let size_mb = file_size as f64 / (1024.0 * 1024.0);
                let max_mb = max_size as f64 / (1024.0 * 1024.0);
                log::warn!("Video file too large: {:.2} MB (max: {:.2} MB)", size_mb, max_mb);
                log::warn!("Telegram Bot API limit exceeded by {:.2} MB", size_mb - max_mb);
                log::warn!("Consider downloading with lower quality (720p, 480p, or 360p) to reduce file size");
                send_error_with_sticker(&bot_clone, chat_id).await;
                let _ = progress_msg.update(&bot_clone, DownloadStatus::Error {
                    title: display_title.as_ref().to_string(),
                    error: format!("Файл слишком большой ({:.2} MB). Максимальный размер: {:.2} MB.\nПопробуй скачать с меньшим качеством (720p, 480p или 360p).", size_mb, max_mb),
                    file_format: Some("mp4".to_string()),
                }).await;
                return Err(AppError::Validation(format!("Файл слишком большой: {:.2} MB (максимум: {:.2} MB). Попробуй выбрать меньшее качество.", size_mb, max_mb)));
            }

            // Step 3.5: Проверяем, что файл содержит и видео, и аудио дорожки
            match has_both_video_and_audio(&actual_file_path) {
                Ok(true) => {
                    log::info!("Video file verified: contains both video and audio streams");
                },
                Ok(false) => {
                    log::error!("Video file is missing video or audio stream!");
                    log::error!("This can cause black screen or playback issues in Telegram");

                    // Попробуем получить детальную информацию о файле
                    let _ = Command::new("ffprobe")
                        .args(["-v", "error", "-show_streams", &actual_file_path])
                        .output()
                        .map(|output| {
                            log::error!("File streams info: {}", String::from_utf8_lossy(&output.stdout));
                        });

                    send_error_with_sticker(&bot_clone, chat_id).await;
                    let _ = progress_msg.update(&bot_clone, DownloadStatus::Error {
                        title: display_title.as_ref().to_string(),
                        error: "Видео файл повреждён или не содержит все необходимые дорожки".to_string(),
                        file_format: Some("mp4".to_string()),
                    }).await;
                    return Err(AppError::Download("Video file missing video or audio stream".to_string()));
                },
                Err(e) => {
                    log::warn!("Failed to verify video streams: {}. Continuing anyway...", e);
                }
            }

            // Step 4: Get user preference for send_as_document
            let send_as_document = if let Some(ref pool) = db_pool_clone {
                match db::get_connection(pool) {
                    Ok(conn) => {
                        let value = db::get_user_send_as_document(&conn, chat_id.0).unwrap_or(0);
                        log::info!("📊 User {} send_as_document value from DB: {} ({})",
                            chat_id.0,
                            value,
                            if value == 0 { "Media/send_video" } else { "Document/send_document" }
                        );
                        value == 1
                    }
                    Err(_) => false
                }
            } else {
                false
            };

            log::info!("📤 Calling send_video_with_retry with send_as_document={} for user {}", send_as_document, chat_id.0);

            // Step 5: Send video with retry logic and animation
            send_video_with_retry(&bot_clone, chat_id, &actual_file_path, &mut progress_msg, display_title.as_ref(), thumbnail_url.as_deref(), send_as_document).await?;

            // Save to download history after successful send
            if let Some(ref pool) = db_pool_clone {
                if let Ok(conn) = crate::storage::db::get_connection(pool) {
                    if let Err(e) = save_download_history(&conn, chat_id.0, url.as_str(), display_title.as_ref(), "mp4") {
                        log::warn!("Failed to save download history: {}", e);
                    }
                }
            }

            // Step 5: Show success status with time
            let _ = progress_msg.update(&bot_clone, DownloadStatus::Success {
                title: display_title.as_ref().to_string(),
                elapsed_secs,
                file_format: Some("mp4".to_string()),
            }).await;

            // Add eyes emoji reaction to the original message if message_id is available
            if let Some(msg_id) = message_id {
                use teloxide::types::{ReactionType, MessageId};
                let reaction = vec![ReactionType::Emoji {
                    emoji: "👀".to_string(),
                }];
                if let Err(e) = bot_clone.set_message_reaction(chat_id, MessageId(msg_id)).reaction(reaction).await {
                    log::warn!("Failed to set message reaction: {}", e);
                    // Not critical, continue
                }
            }

            // Step 5: Auto-clear success message after delay (оставляем только название)
            let bot_for_clear = bot_clone.clone();
            let title_for_clear = Arc::clone(&display_title);
            let mut msg_for_clear = ProgressMessage {
                chat_id: progress_msg.chat_id,
                message_id: progress_msg.message_id,
            };
            tokio::spawn(async move {
                let _ = msg_for_clear.clear_after(&bot_for_clear, config::progress::CLEAR_DELAY_SECS, title_for_clear.as_ref().to_string(), Some("mp3".to_string())).await;
            });

            tokio::time::sleep(config::download::cleanup_delay()).await;
            // Удаляем фактический файл, который был скачан и отправлен
            if let Err(e) = fs::remove_file(&actual_file_path) {
                log::warn!("Failed to delete actual file {}: {}", actual_file_path, e);
            }
            // Также пытаемся удалить исходный путь на случай если он отличается
            if actual_file_path != download_path {
                if let Err(e) = fs::remove_file(&download_path) {
                    log::debug!("Failed to delete expected file {} (this is OK if it doesn't exist): {}", download_path, e);
                }
            }

            Ok(())
        }.await;

        if let Err(e) = result {
            log::error!(
                "An error occurred during video download for chat {}: {:?}",
                chat_id,
                e
            );

            // Определяем тип ошибки и формируем полезное сообщение
            let error_str = e.to_string();
            let custom_message = if error_str.contains("Only images are available") {
                Some(
                    "Это видео недоступно для скачивания 😢\n\n\
                Возможные причины:\n\
                • Видео удалено или приватное\n\
                • Возрастные ограничения\n\
                • Региональные ограничения\n\
                • Стрим или премьера (еще не доступны)\n\n\
                Попробуй другое видео!",
                )
            } else if error_str.contains("Signature extraction failed") {
                Some(
                    "У меня устарела версия загрузчика 😢\n\n\
                Стэн уже знает и скоро обновит!\n\
                Попробуй позже или другое видео.",
                )
            } else if error_str.contains("Sign in to confirm you're not a bot")
                || error_str.contains("bot detection")
            {
                Some(
                    "YouTube заблокировал бота 🤖\n\n\
                Нужно настроить cookies.\n\
                Стэн уже знает и разбирается!\n\n\
                Попробуй позже.",
                )
            } else {
                None
            };

            // Send error sticker and message
            send_error_with_sticker_and_message(&bot_clone, chat_id, custom_message).await;
            // Show error status
            let _ = progress_msg
                .update(
                    &bot_clone,
                    DownloadStatus::Error {
                        title: "Скачивание".to_string(),
                        error: e.to_string(),
                        file_format: Some("mp4".to_string()),
                    },
                )
                .await;
        }
    });
    Ok(())
}

fn generate_file_name(title: &str, artist: &str) -> String {
    generate_file_name_with_ext(title, artist, "mp3")
}

fn generate_file_name_with_ext(title: &str, artist: &str, extension: &str) -> String {
    let title_trimmed = title.trim();
    let artist_trimmed = artist.trim();

    log::debug!(
        "Generating filename: title='{}' (len={}), artist='{}' (len={}), ext='{}'",
        title,
        title.len(),
        artist,
        artist.len(),
        extension
    );

    let filename = if artist_trimmed.is_empty() && title_trimmed.is_empty() {
        log::warn!(
            "Both title and artist are empty, using 'Unknown.{}'",
            extension
        );
        format!("Unknown.{}", extension)
    } else if artist_trimmed.is_empty() {
        log::debug!("Using title only: '{}.{}'", title_trimmed, extension);
        format!("{}.{}", title_trimmed, extension)
    } else if title_trimmed.is_empty() {
        log::debug!("Using artist only: '{}.{}'", artist_trimmed, extension);
        format!("{}.{}", artist_trimmed, extension)
    } else {
        log::debug!(
            "Using both: '{} - {}.{}'",
            artist_trimmed,
            title_trimmed,
            extension
        );
        format!("{} - {}.{}", artist_trimmed, title_trimmed, extension)
    };

    // Заменяем пробелы на подчеркивания перед возвратом
    sanitize_filename(&filename)
}

/// Download subtitles file (SRT or TXT format) and send it to user
///
/// Downloads subtitles from URL using yt-dlp and sends them as a document.
///
/// # Arguments
///
/// * `bot` - Telegram bot instance
/// * `chat_id` - User's chat ID
/// * `url` - URL to download subtitles from
/// * `rate_limiter` - Rate limiter instance (unused but kept for API consistency)
/// * `_created_timestamp` - Timestamp when task was created (unused)
/// * `subtitle_format` - Subtitle format ("srt" or "txt")
///
/// # Returns
///
/// Returns `Ok(())` on success or a `ResponseResult` error.
pub async fn download_and_send_subtitles(
    bot: Bot,
    chat_id: ChatId,
    url: Url,
    rate_limiter: Arc<RateLimiter>,
    _created_timestamp: DateTime<Utc>,
    subtitle_format: String,
    db_pool: Option<Arc<DbPool>>,
    message_id: Option<i32>,
) -> ResponseResult<()> {
    let bot_clone = bot.clone();
    let _rate_limiter = Arc::clone(&rate_limiter);
    let db_pool_clone = db_pool.clone();

    tokio::spawn(async move {
        let mut progress_msg = ProgressMessage::new(chat_id);
        let start_time = std::time::Instant::now();

        let result: Result<(), AppError> = async {
            // Step 1: Get metadata
            let (title, _) = match get_metadata_from_ytdlp(&url).await {
                Ok(meta) => meta,
                Err(e) => {
                    log::error!("Failed to get metadata: {:?}", e);
                    // Проверяем, является ли это ошибкой таймаута
                    if e.to_string().contains("timed out") {
                        log::warn!("yt-dlp timed out, sending error message to user");
                        send_error_with_sticker(&bot_clone, chat_id).await;
                    }
                    return Err(e);
                }
            };
            let display_title: Arc<str> = Arc::from(title.as_str());

            // Show starting status
            let _ = progress_msg
                .update(
                    &bot_clone,
                    DownloadStatus::Starting {
                        title: display_title.as_ref().to_string(),
                        file_format: Some(subtitle_format.clone()),
                    },
                )
                .await;

            let file_name = format!("{}.{}", title, subtitle_format);
            let safe_filename = escape_filename(&file_name);
            let full_path = format!("{}/{}", &*config::DOWNLOAD_FOLDER, safe_filename);
            let download_path = shellexpand::tilde(&full_path).into_owned();

            // Step 2: Download subtitles
            let ytdl_bin = &*config::YTDL_BIN;
            let sub_format_flag = match subtitle_format.as_str() {
                "srt" => "--convert-subs=srt",
                "txt" => "--convert-subs=txt",
                _ => "--convert-subs=srt",
            };

            let mut args: Vec<&str> = vec![
                "-o",
                &download_path,
                "--skip-download",
                "--write-auto-subs",
                sub_format_flag,
            ];
            add_cookies_args(&mut args);
            args.push(url.as_str());

            // Логируем полную команду для отладки
            let command_str = format!("{} {}", ytdl_bin, args.join(" "));
            log::info!(
                "[DEBUG] yt-dlp command for subtitles download: {}",
                command_str
            );

            let mut child = spawn_downloader_with_fallback(&ytdl_bin, &args)?;
            let status = child
                .wait()
                .map_err(|e| AppError::Download(format!("downloader process failed: {}", e)))?;

            if !status.success() {
                return Err(AppError::Download(format!(
                    "downloader exited with status: {}",
                    status
                )));
            }

            // Check if file exists
            if !fs::metadata(&download_path).is_ok() {
                // Try to find the actual filename that was downloaded
                let parent_dir = shellexpand::tilde("~/downloads/").into_owned();
                let dir_entries = fs::read_dir(&parent_dir).map_err(|e| {
                    AppError::Download(format!("Failed to read downloads dir: {}", e))
                })?;
                let mut found_file: Option<String> = None;

                for entry in dir_entries {
                    if let Ok(entry) = entry {
                        let file_name = entry.file_name().to_string_lossy().to_string();
                        if file_name.ends_with(&format!(".{}", subtitle_format)) {
                            found_file = Some(entry.path().to_string_lossy().to_string());
                            break;
                        }
                    }
                }

                if let Some(found) = found_file {
                    // Send the found file
                    let _ = bot_clone
                        .send_document(chat_id, InputFile::file(&found))
                        .await
                        .map_err(|e| {
                            AppError::Download(format!("Failed to send document: {}", e))
                        })?;
                } else {
                    return Err(AppError::Download(format!("Subtitle file not found")));
                }
            } else {
                // Send the file
                let _ = bot_clone
                    .send_document(chat_id, InputFile::file(&download_path))
                    .await
                    .map_err(|e| AppError::Download(format!("Failed to send document: {}", e)))?;
            }

            // Calculate elapsed time
            let elapsed_secs = start_time.elapsed().as_secs();
            log::info!("Subtitle downloaded in {} seconds", elapsed_secs);

            // Save to download history after successful send
            if let Some(ref pool) = db_pool_clone {
                if let Ok(conn) = crate::storage::db::get_connection(pool) {
                    if let Err(e) = save_download_history(
                        &conn,
                        chat_id.0,
                        url.as_str(),
                        display_title.as_ref(),
                        &subtitle_format,
                    ) {
                        log::warn!("Failed to save download history: {}", e);
                    }
                }
            }

            // Step 3: Show success status
            let _ = progress_msg
                .update(
                    &bot_clone,
                    DownloadStatus::Success {
                        title: display_title.as_ref().to_string(),
                        elapsed_secs,
                        file_format: Some(subtitle_format.clone()),
                    },
                )
                .await;

            // Add eyes emoji reaction to the original message if message_id is available
            if let Some(msg_id) = message_id {
                use teloxide::types::{MessageId, ReactionType};
                let reaction = vec![ReactionType::Emoji {
                    emoji: "👀".to_string(),
                }];
                if let Err(e) = bot_clone
                    .set_message_reaction(chat_id, MessageId(msg_id))
                    .reaction(reaction)
                    .await
                {
                    log::warn!("Failed to set message reaction: {}", e);
                    // Not critical, continue
                }
            }

            log::info!("Subtitle sent successfully to chat {}", chat_id);

            // Step 4: Auto-clear success message
            let bot_for_clear = bot_clone.clone();
            let title_for_clear = Arc::clone(&display_title);
            let mut msg_for_clear = ProgressMessage {
                chat_id: progress_msg.chat_id,
                message_id: progress_msg.message_id,
            };
            let subtitle_format_clone = subtitle_format.clone();
            tokio::spawn(async move {
                let _ = msg_for_clear
                    .clear_after(
                        &bot_for_clear,
                        10,
                        title_for_clear.as_ref().to_string(),
                        Some(subtitle_format_clone),
                    )
                    .await;
            });

            // Clean up file after 10 minutes
            tokio::time::sleep(config::download::cleanup_delay()).await;
            if let Err(e) = fs::remove_file(&download_path) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    return Err(AppError::Download(format!("Failed to delete file: {}", e)))?;
                }
                // File doesn't exist - that's fine, it was probably deleted manually
            }

            Ok(())
        }
        .await;

        if let Err(e) = result {
            log::error!(
                "An error occurred during subtitle download for chat {}: {:?}",
                chat_id,
                e
            );
            // Send error sticker and message
            send_error_with_sticker(&bot_clone, chat_id).await;
            // Show error status
            let _ = progress_msg
                .update(
                    &bot_clone,
                    DownloadStatus::Error {
                        title: "Скачивание".to_string(),
                        error: e.to_string(),
                        file_format: Some(subtitle_format.clone()),
                    },
                )
                .await;
        }
    });
    Ok(())
}

#[cfg(test)]
mod download_tests {
    use super::*;

    fn tool_exists(bin: &str) -> bool {
        Command::new("which")
            .arg(bin)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    #[test]
    fn test_probe_duration_seconds_handles_missing_file() {
        assert_eq!(probe_duration_seconds("/no/such/file.mp3"), None);
    }

    #[test]
    fn test_spawn_downloader_fails_without_tools() {
        if tool_exists("yt-dlp") || tool_exists("youtube-dl") {
            // Tools present; skip this specific negative test.
            return;
        }
        let res = spawn_downloader_with_fallback("youtube-dl", &["--version"]);
        assert!(res.is_err());
    }

    // Integration-ish test: requires network and yt-dlp (or youtube-dl) + ffmpeg installed.
    // It downloads to a temp path and ensures file appears, then cleans up.
    #[test]
    #[ignore]
    fn test_download_audio_file_from_youtube() {
        if !(tool_exists("yt-dlp") || tool_exists("youtube-dl")) {
            eprintln!("skipping: no yt-dlp/youtube-dl in PATH");
            return;
        }
        if !tool_exists("ffprobe") {
            // ffmpeg suite
            eprintln!("skipping: no ffprobe in PATH");
            return;
        }
        let url = Url::parse("https://www.youtube.com/watch?v=0CAltmPaNZY")
            .expect("Test URL should be valid");
        let tmp_dir = std::env::temp_dir();
        let dest = tmp_dir.join(format!("test_dl_{}.mp3", uuid::Uuid::new_v4()));
        let dest_str = dest.to_string_lossy().to_string();
        let res = download_audio_file(&url, &dest_str);
        match res {
            Ok(_dur_opt) => {
                assert!(std::path::Path::new(&dest_str).exists());
                let _ = fs::remove_file(&dest_str);
            }
            Err(e) => {
                let _ = fs::remove_file(&dest_str); // Cleanup on error
                panic!("Download test failed: {:?}", e);
            }
        }
    }
}
