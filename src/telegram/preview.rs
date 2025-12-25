use crate::core::config;
use crate::core::error::AppError;
use crate::download::downloader::add_cookies_args;
use crate::download::ytdlp_errors::{
    analyze_ytdlp_error, get_error_message, get_fix_recommendations, should_notify_admin,
};
use crate::storage::cache;
use crate::storage::db::DbPool;
use serde_json::Value;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::MessageId;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup, InputFile};
use tokio::process::Command as TokioCommand;
use tokio::time::timeout;
use url::Url;

use crate::telegram::cache::PREVIEW_CACHE;
use crate::telegram::types::{PreviewMetadata, VideoFormatInfo};

const MAX_VIDEO_FORMAT_SIZE_BYTES: u64 = 2 * 1024 * 1024 * 1024;

fn filter_video_formats_by_size(formats: &[VideoFormatInfo]) -> Vec<VideoFormatInfo> {
    formats
        .iter()
        .filter(|format| format.size_bytes.is_none_or(|size| size <= MAX_VIDEO_FORMAT_SIZE_BYTES))
        .cloned()
        .collect()
}

/// Получает метаданные из JSON ответа yt-dlp
///
/// Использует --dump-json для получения всех метаданных за один вызов
async fn get_metadata_from_json(url: &Url, ytdl_bin: &str) -> Result<Value, AppError> {
    let mut args: Vec<&str> = vec![
        "--dump-json",
        "--no-playlist",
        "--socket-timeout",
        "30",
        "--retries",
        "2",
        "--extractor-args",
        "youtube:player_client=default,web_safari,web_embedded",
    ];
    add_cookies_args(&mut args);
    args.push(url.as_str());

    let command_str = format!("{} {}", ytdl_bin, args.join(" "));
    log::info!("[DEBUG] yt-dlp command for preview metadata (JSON): {}", command_str);

    let json_output = timeout(
        config::download::ytdlp_timeout(),
        TokioCommand::new(ytdl_bin).args(&args).output(),
    )
    .await
    .map_err(|_| AppError::Download("yt-dlp command timed out getting metadata".to_string()))?
    .map_err(|e| AppError::Download(format!("Failed to get metadata: {}", e)))?;

    if !json_output.status.success() {
        let stderr = String::from_utf8_lossy(&json_output.stderr);
        let error_type = analyze_ytdlp_error(&stderr);

        // Логируем детальную информацию об ошибке
        log::error!("Failed to get metadata, error type: {:?}", error_type);
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

    let json_str = String::from_utf8_lossy(&json_output.stdout);
    serde_json::from_str(&json_str).map_err(|e| AppError::Download(format!("Failed to parse JSON metadata: {}", e)))
}

/// Извлекает значение из JSON по ключу
fn get_json_value(json: &Value, key: &str) -> Option<String> {
    json.get(key)
        .and_then(|v| {
            if v.is_null() {
                None
            } else if v.is_string() {
                v.as_str().map(|s| s.to_string())
            } else if v.is_number() {
                Some(v.to_string())
            } else {
                None
            }
        })
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s != "NA")
}

/// Пытается получить размер файла для конкретного качества видео из JSON
fn get_video_filesize_from_json(json: &Value, quality: &str) -> Option<u64> {
    let target_height = match quality {
        "1080p" => 1080,
        "720p" => 720,
        "480p" => 480,
        "360p" => 360,
        _ => return None,
    };

    // Пробуем получить из formats массива
    json.get("formats").and_then(|v| v.as_array()).and_then(|formats| {
        formats
            .iter()
            .filter_map(|format| {
                // Ищем формат с нужным разрешением
                let height = format.get("height").and_then(|v| v.as_u64()).unwrap_or(0);

                if height == target_height as u64 {
                    // Пробуем получить filesize или filesize_approx
                    format
                        .get("filesize")
                        .or_else(|| format.get("filesize_approx"))
                        .and_then(|v| v.as_u64())
                } else {
                    None
                }
            })
            .max() // Берем максимальный размер среди всех форматов с нужным разрешением
    })
}

/// Получает расширенные метаданные для превью
///
/// Оптимизированная версия: использует --dump-json для получения всех метаданных за один вызов
///
/// # Arguments
/// * `url` - URL видео/аудио
/// * `format` - Формат загрузки ("mp3", "mp4", "srt", "txt")
/// * `video_quality` - Качество видео (только для mp4, например "1080p", "720p", "480p", "360p")
pub async fn get_preview_metadata(
    url: &Url,
    format: Option<&str>,
    video_quality: Option<&str>,
) -> Result<PreviewMetadata, AppError> {
    let ytdl_bin = &*config::YTDL_BIN;
    log::debug!("Getting preview metadata for URL: {}", url);

    // Проверяем кэш превью
    if let Some(metadata) = PREVIEW_CACHE.get(url.as_str()).await {
        log::debug!("Preview metadata found in cache for URL: {}", url);
        return Ok(metadata);
    }

    // Проверяем кэш для базовых метаданных (старый кэш, если нужно)
    let (cached_title, cached_artist) = if let Some((title, artist)) = cache::get_cached_metadata(url).await {
        (Some(title), Some(artist))
    } else {
        (None, None)
    };

    // Получаем все метаданные за один вызов через JSON (оптимизация скорости)
    let json_metadata = get_metadata_from_json(url, ytdl_bin).await?;

    // Извлекаем title из JSON (используем кэш если доступен)
    let title = if let Some(cached) = cached_title {
        cached
    } else {
        get_json_value(&json_metadata, "title")
            .ok_or_else(|| AppError::Download("Failed to get video title from metadata".to_string()))?
    };

    if title.trim().is_empty() {
        log::warn!("yt-dlp returned empty title for URL: {}", url);
        return Err(AppError::Download(
            "Failed to get video title. Video might be unavailable or private.".to_string(),
        ));
    }

    // Извлекаем artist из JSON (используем кэш если доступен, но игнорируем "NA")
    let mut artist = if let Some(cached) = cached_artist {
        // Если в кэше "NA" - игнорируем и получаем свежие данные
        if cached.trim() == "NA" || cached.trim().is_empty() {
            String::new() // Будем получать свежие данные
        } else {
            cached
        }
    } else {
        String::new() // Будем получать свежие данные
    };

    // Если artist пустой - получаем из JSON
    if artist.is_empty() {
        artist = get_json_value(&json_metadata, "artist").unwrap_or_default();
    }

    // Если artist все еще пустой или "NA" - получаем uploader (channel) из JSON
    if artist.trim().is_empty() || artist.trim() == "NA" {
        log::debug!("Artist is empty or 'NA' in preview, trying to get channel/uploader");
        if let Some(uploader) = get_json_value(&json_metadata, "uploader") {
            artist = uploader;
            log::info!("Using uploader/channel as artist in preview: '{}'", artist);
        }
    }

    // Извлекаем thumbnail URL из JSON
    // Пробуем несколько возможных полей для thumbnail
    let thumbnail_url = get_json_value(&json_metadata, "thumbnail").or_else(|| {
        // Если thumbnails это массив, берем лучший (обычно последний или с максимальным width)
        json_metadata
            .get("thumbnails")
            .and_then(|v| v.as_array())
            .and_then(|arr| {
                // Ищем thumbnail с максимальным width (лучшее качество)
                arr.iter()
                    .filter_map(|thumb| {
                        thumb.get("url").and_then(|v| v.as_str()).map(|url| {
                            let width = thumb.get("width").and_then(|v| v.as_u64()).unwrap_or(0);
                            (url.to_string(), width)
                        })
                    })
                    .max_by_key(|(_, width)| *width)
                    .map(|(url, _)| url)
            })
    });

    // Извлекаем duration из JSON
    let duration = get_json_value(&json_metadata, "duration")
        .and_then(|d| d.parse::<f64>().ok())
        .map(|d| d as u32);

    // Проверяем длительность видео: максимум 4 часа (14400 секунд)
    if let Some(dur) = duration {
        const MAX_DURATION_SECONDS: u32 = 14400; // 4 часа
        if dur > MAX_DURATION_SECONDS {
            let hours = dur / 3600;
            let minutes = (dur % 3600) / 60;
            return Err(AppError::Download(format!(
                "Видео слишком длинное ({}ч {}мин). Максимальная длительность: 4 часа.",
                hours, minutes
            )));
        }
    }

    // Для видео получаем список доступных форматов с размерами
    // Для видео форматов все еще используем --list-formats, так как JSON не всегда содержит точные размеры для всех форматов
    let video_formats: Option<Vec<VideoFormatInfo>> = if format == Some("mp4") || format == Some("mp4+mp3") {
        match get_video_formats_list(url, ytdl_bin).await {
            Ok(formats) => {
                if formats.is_empty() {
                    log::warn!("get_video_formats_list returned empty list for URL: {}", url);
                    None
                } else {
                    log::debug!("Successfully got {} video formats for URL: {}", formats.len(), url);
                    Some(formats)
                }
            }
            Err(e) => {
                log::warn!(
                    "Failed to get video formats list for URL {}: {}. Will use fallback button.",
                    url,
                    e
                );
                // Не возвращаем ошибку, а просто логируем - создадим стандартную кнопку
                None
            }
        }
    } else {
        None
    };

    // Получаем примерный размер файла
    // Для видео получаем размер для конкретного качества через --list-formats (если нужно)
    // Для аудио используем filesize из JSON
    let mut filesize = if format == Some("mp4") {
        if let Some(quality) = video_quality {
            // Для видео с конкретным качеством пытаемся получить из JSON formats массива
            get_video_filesize_from_json(&json_metadata, quality)
        } else {
            // Для видео без конкретного качества - используем filesize из JSON
            get_json_value(&json_metadata, "filesize")
                .or_else(|| get_json_value(&json_metadata, "filesize_approx"))
                .and_then(|s| s.parse::<u64>().ok())
        }
    } else {
        // Для аудио используем filesize из JSON
        get_json_value(&json_metadata, "filesize")
            .or_else(|| get_json_value(&json_metadata, "filesize_approx"))
            .and_then(|s| s.parse::<u64>().ok())
    };

    // Если filesize не получен из JSON для видео с конкретным качеством, используем размер из video_formats
    if filesize.is_none() && format == Some("mp4") {
        if let Some(quality) = video_quality {
            filesize = video_formats
                .as_ref()
                .and_then(|formats| formats.iter().find(|f| f.quality == quality).and_then(|f| f.size_bytes));
        }
    }

    // Извлекаем description из JSON
    let description = get_json_value(&json_metadata, "description").map(|desc| {
        // Ограничиваем длину описания (безопасно, по границам символов)
        const MAX_CHARS: usize = 200;
        let char_count = desc.chars().count();
        if char_count > MAX_CHARS {
            let truncated: String = desc.chars().take(MAX_CHARS).collect();
            format!("{}...", truncated)
        } else {
            desc
        }
    });

    let metadata = PreviewMetadata {
        title: title.clone(),
        artist: artist.clone(),
        thumbnail_url: thumbnail_url.clone(),
        duration,
        filesize,
        description,
        video_formats,
    };

    // Сохраняем расширенные метаданные в кэш только если title не пустой и не "Unknown Track"
    if !title.trim().is_empty() && title.trim() != "Unknown Track" {
        cache::cache_extended_metadata(
            url,
            title.clone(),
            artist.clone(),
            thumbnail_url.clone(),
            duration,
            filesize,
        )
        .await;

        // Сохраняем в новый кэш превью
        PREVIEW_CACHE.set(url.as_str().to_string(), metadata.clone()).await;
    } else {
        log::warn!("Not caching metadata with invalid title: '{}'", title);
    }

    Ok(metadata)
}

/// Получает список доступных форматов видео с размерами
///
/// Парсит вывод yt-dlp --list-formats и извлекает информацию о форматах:
/// - 1080p, 720p, 480p, 360p
/// - Размеры файлов
/// - Разрешения
async fn get_video_formats_list(url: &Url, ytdl_bin: &str) -> Result<Vec<VideoFormatInfo>, AppError> {
    let mut list_formats_args: Vec<String> = vec!["--list-formats".to_string(), "--no-playlist".to_string()];

    let mut temp_args: Vec<&str> = vec![];
    add_cookies_args(&mut temp_args);
    for arg in temp_args {
        list_formats_args.push(arg.to_string());
    }
    list_formats_args.push(url.as_str().to_string());

    let list_formats_output = timeout(
        // Используем тот же таймаут, что и для остальных вызовов yt-dlp,
        // чтобы не обрывать долгие запросы к YouTube раньше времени
        config::download::ytdlp_timeout(),
        TokioCommand::new(ytdl_bin).args(&list_formats_args).output(),
    )
    .await
    .map_err(|_| AppError::Download("yt-dlp command timed out getting formats list".to_string()))?
    .map_err(|e| AppError::Download(format!("Failed to get formats list: {}", e)))?;

    if !list_formats_output.status.success() {
        let stderr = String::from_utf8_lossy(&list_formats_output.stderr);
        let error_type = analyze_ytdlp_error(&stderr);

        // Логируем детальную информацию об ошибке
        log::error!("Failed to get formats list, error type: {:?}", error_type);
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

    let formats_output = String::from_utf8_lossy(&list_formats_output.stdout);
    let mut formats: Vec<VideoFormatInfo> = Vec::new();
    // log::info!("formats: {:?}", formats_output);

    // Ищем форматы для разных разрешений
    // Включаем как горизонтальные (обычные видео), так и вертикальные (YouTube Shorts)
    let quality_resolutions = vec![
        ("1080p", vec!["1920x1080", "1080x1920"]), // Горизонтальное и вертикальное (Shorts)
        ("720p", vec!["1280x720", "720x1280"]),    // Горизонтальное и вертикальное (Shorts)
        ("480p", vec!["854x480", "640x480", "480x854", "480x640"]), // Горизонтальное и вертикальное
        ("360p", vec!["640x360", "360x640"]),      // Горизонтальное и вертикальное
    ];

    for (quality, resolutions) in quality_resolutions {
        let mut max_size: Option<u64> = None;
        let mut found_resolution: Option<String> = None;

        for line in formats_output.lines() {
            // Проверяем, содержит ли строка нужное разрешение
            let matches_resolution = resolutions.iter().any(|&res| line.contains(res));

            if matches_resolution {
                // Пропускаем только "audio only" - нам нужны видео форматы (как комбинированные, так и video-only)
                let is_audio_only = line.contains("audio only");

                if !is_audio_only {
                    // Извлекаем размер
                    if let Some(mib_pos) = line.find("MiB") {
                        let before_mib = &line[..mib_pos];
                        let mut num_chars = Vec::new();
                        let mut found_digit = false;

                        for ch in before_mib.chars().rev() {
                            if ch.is_ascii_digit() || ch == '.' {
                                num_chars.push(ch);
                                found_digit = true;
                            } else if found_digit {
                                break;
                            }
                        }

                        if !num_chars.is_empty() {
                            num_chars.reverse();
                            let size_str: String = num_chars.into_iter().collect();
                            let size_str = size_str.trim();

                            if let Ok(size_mb) = size_str.parse::<f64>() {
                                if size_mb > 0.0 && size_mb < 10000.0 {
                                    let size_bytes = (size_mb * 1024.0 * 1024.0) as u64;

                                    // Берем максимальный размер (лучший формат)
                                    if max_size.is_none_or(|current| size_bytes > current) {
                                        max_size = Some(size_bytes);

                                        // Извлекаем разрешение из строки
                                        for &res in &resolutions {
                                            if line.contains(res) {
                                                found_resolution = Some(res.to_string());
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    } else if let Some(gib_pos) = line.find("GiB") {
                        // Поддерживаем размеры в гигабайтах (yt-dlp помечает как GiB)
                        let before_gib = &line[..gib_pos];
                        let mut num_chars = Vec::new();
                        let mut found_digit = false;

                        for ch in before_gib.chars().rev() {
                            if ch.is_ascii_digit() || ch == '.' {
                                num_chars.push(ch);
                                found_digit = true;
                            } else if found_digit {
                                break;
                            }
                        }

                        if !num_chars.is_empty() {
                            num_chars.reverse();
                            let size_str: String = num_chars.into_iter().collect();
                            let size_str = size_str.trim();

                            if let Ok(size_gb) = size_str.parse::<f64>() {
                                // Ставим разумный предел, чтобы отфильтровать мусорные значения
                                if size_gb > 0.0 && size_gb < 10000.0 {
                                    let size_bytes = (size_gb * 1024.0 * 1024.0 * 1024.0) as u64;

                                    if max_size.is_none_or(|current| size_bytes > current) {
                                        max_size = Some(size_bytes);

                                        for &res in &resolutions {
                                            if line.contains(res) {
                                                found_resolution = Some(res.to_string());
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    } else if let Some(kib_pos) = line.find("KiB") {
                        // Также проверяем KiB
                        let before_kib = &line[..kib_pos];
                        let mut num_chars = Vec::new();
                        let mut found_digit = false;

                        for ch in before_kib.chars().rev() {
                            if ch.is_ascii_digit() || ch == '.' {
                                num_chars.push(ch);
                                found_digit = true;
                            } else if found_digit {
                                break;
                            }
                        }

                        if !num_chars.is_empty() {
                            num_chars.reverse();
                            let size_str: String = num_chars.into_iter().collect();
                            let size_str = size_str.trim();

                            if let Ok(size_kb) = size_str.parse::<f64>() {
                                if size_kb > 0.0 && size_kb < 100000.0 {
                                    let size_bytes = (size_kb * 1024.0) as u64;

                                    if max_size.is_none_or(|current| size_bytes > current) {
                                        max_size = Some(size_bytes);

                                        for &res in &resolutions {
                                            if line.contains(res) {
                                                found_resolution = Some(res.to_string());
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        if max_size.is_some() {
            formats.push(VideoFormatInfo {
                quality: quality.to_string(),
                size_bytes: max_size,
                resolution: found_resolution,
            });
        }
    }

    // Находим размер лучшего аудио формата чтобы добавить к размеру video-only форматов
    let mut best_audio_size: Option<u64> = None;
    for line in formats_output.lines() {
        if line.contains("audio only") {
            // Ищем m4a или webm аудио с наибольшим битрейтом
            if line.contains("m4a") || line.contains("webm") {
                if let Some(mib_pos) = line.find("MiB") {
                    let before_mib = &line[..mib_pos];
                    let mut num_chars = Vec::new();
                    let mut found_digit = false;

                    for ch in before_mib.chars().rev() {
                        if ch.is_ascii_digit() || ch == '.' {
                            num_chars.push(ch);
                            found_digit = true;
                        } else if found_digit {
                            break;
                        }
                    }

                    if !num_chars.is_empty() {
                        num_chars.reverse();
                        let size_str: String = num_chars.into_iter().collect();
                        if let Ok(size_mb) = size_str.trim().parse::<f64>() {
                            if size_mb > 0.0 && size_mb < 1000.0 {
                                let size_bytes = (size_mb * 1024.0 * 1024.0) as u64;
                                if best_audio_size.is_none_or(|current| size_bytes > current) {
                                    best_audio_size = Some(size_bytes);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Добавляем размер аудио к размеру каждого видео формата
    if let Some(audio_size) = best_audio_size {
        log::info!(
            "Found best audio size: {:.2} MB, adding to video formats",
            audio_size as f64 / (1024.0 * 1024.0)
        );
        for format in &mut formats {
            if let Some(ref mut video_size) = format.size_bytes {
                *video_size += audio_size;
            }
        }
    } else {
        log::warn!("No audio format size found, video format sizes might be underestimated");
    }

    // Сортируем форматы по качеству (от лучшего к худшему)
    formats.sort_by(|a, b| {
        let order = |q: &str| match q {
            "1080p" => 4,
            "720p" => 3,
            "480p" => 2,
            "360p" => 1,
            _ => 0,
        };
        order(&b.quality).cmp(&order(&a.quality))
    });

    Ok(formats)
}

/// Экранирует специальные символы для MarkdownV2
///
/// В Telegram MarkdownV2 требуется экранировать следующие символы:
/// _ * [ ] ( ) ~ ` > # + - = | { } . !
///
/// Важно: обратный слеш должен экранироваться первым, чтобы избежать повторного экранирования
fn escape_markdown(text: &str) -> String {
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

/// Отправляет превью с метаданными и кнопками подтверждения
///
/// Для видео показывает список форматов с кнопками выбора
/// Для других форматов - стандартные кнопки
///
/// # Arguments
/// * `bot` - Telegram bot instance
/// * `chat_id` - User's chat ID
/// * `url` - Video/audio URL
/// * `metadata` - Preview metadata with formats
/// * `default_format` - Default format (mp3, mp4, etc.)
/// * `default_quality` - Default video quality (1080p, 720p, etc.) - only for mp4
/// * `old_preview_msg_id` - Опциональный ID старого preview сообщения для удаления
#[allow(clippy::too_many_arguments)]
pub async fn send_preview(
    bot: &Bot,
    chat_id: ChatId,
    url: &Url,
    metadata: &PreviewMetadata,
    default_format: &str,
    default_quality: Option<&str>,
    old_preview_msg_id: Option<MessageId>,
    db_pool: Arc<DbPool>,
) -> ResponseResult<Message> {
    let lang = crate::i18n::user_lang_from_pool(&db_pool, chat_id.0);

    // Формируем текст превью с экранированием
    let escaped_title = escape_markdown(&metadata.display_title());
    let mut text = format!("🎵 *{}*\n\n", escaped_title);

    if metadata.duration.is_some() {
        let duration_str = metadata.format_duration();
        text.push_str(&format!("⏱️ Длительность: {}\n", escape_markdown(&duration_str)));
    }

    let filtered_formats = metadata
        .video_formats
        .as_ref()
        .map(|formats| filter_video_formats_by_size(formats));

    // Для видео показываем список форматов с размерами
    if default_format == "mp4" || default_format == "mp4+mp3" {
        if let Some(formats) = &filtered_formats {
            if !formats.is_empty() {
                text.push_str("\n📹 *Доступные форматы:*\n");
                for format_info in formats {
                    let size_str = if let Some(size) = format_info.size_bytes {
                        if size > 1024 * 1024 {
                            format!("{:.1} MB", size as f64 / (1024.0 * 1024.0))
                        } else if size > 1024 {
                            format!("{:.1} KB", size as f64 / 1024.0)
                        } else {
                            format!("{} B", size)
                        }
                    } else {
                        crate::i18n::t(&lang, "common.unknown")
                    };
                    let resolution_str = format_info
                        .resolution
                        .as_ref()
                        .map(|r| format!(" ({})", r))
                        .unwrap_or_default();
                    text.push_str(&format!(
                        "• {}: {}{}\n",
                        escape_markdown(&format_info.quality),
                        escape_markdown(&size_str),
                        escape_markdown(&resolution_str)
                    ));
                }
            }
        }
    } else if metadata.filesize.is_some() {
        let size_str = metadata.format_filesize();
        text.push_str(&format!("📦 Примерный размер: {}\n", escape_markdown(&size_str)));
    }

    if let Some(desc) = &metadata.description {
        text.push_str(&format!("\n📝 {}\n", escape_markdown(desc)));
    }

    text.push_str("\nВыбери формат\\:");

    // Удаляем старое preview сообщение если указано
    if let Some(old_msg_id) = old_preview_msg_id {
        if let Err(e) = bot.delete_message(chat_id, old_msg_id).await {
            log::warn!("Failed to delete old preview message: {:?}", e);
        }
    }

    // Создаем inline клавиатуру
    // Сохраняем URL в кэше и получаем короткий ID (вместо base64)
    let url_id = cache::store_url(&db_pool, url.as_str()).await;
    log::debug!("Stored URL {} with ID: {}", url.as_str(), url_id);

    // Получаем настройку send_as_document из БД для видео
    let send_as_document = if default_format == "mp4" {
        match crate::storage::db::get_connection(&db_pool) {
            Ok(conn) => crate::storage::db::get_user_send_as_document(&conn, chat_id.0).unwrap_or(0),
            Err(e) => {
                log::warn!("Failed to get db connection for send_as_document: {}", e);
                0
            }
        }
    } else {
        0
    };

    // Получаем message_id нового preview сообщения (будет установлен после отправки)
    // Пока используем временное значение 0, потом обновим после отправки
    let keyboard = if default_format == "mp4" || default_format == "mp4+mp3" {
        if let Some(formats) = &filtered_formats {
            if formats.is_empty() {
                log::warn!(
                    "video_formats is Some but empty, using fallback button for {}",
                    default_format
                );
                // Если список форматов пустой, создаем стандартную кнопку
                create_fallback_keyboard(default_format, default_quality, &url_id)
            } else {
                log::debug!(
                    "Creating video format keyboard with {} formats for {}",
                    formats.len(),
                    default_format
                );
                // Для видео создаем кнопки для выбора формата с toggle для Media/Document
                create_video_format_keyboard(formats, default_quality, &url_id, send_as_document, default_format)
            }
        } else {
            // Если video_formats is None для mp4 форматов
            create_fallback_keyboard(default_format, default_quality, &url_id)
        }
    } else {
        // Для других форматов или если video_formats is None - стандартные кнопки
        log::debug!(
            "Creating fallback keyboard for format: {} (video_formats.is_some() = {})",
            default_format,
            metadata.video_formats.is_some()
        );
        create_fallback_keyboard(default_format, default_quality, &url_id)
    };

    // Отправляем превью с thumbnail если доступен
    if let Some(thumb_url) = &metadata.thumbnail_url {
        // Пытаемся отправить фото с thumbnail
        match reqwest::get(thumb_url).await {
            Ok(response) => {
                if response.status().is_success() {
                    match response.bytes().await {
                        Ok(bytes) => {
                            // Отправляем фото с описанием
                            let bytes_vec = bytes.to_vec();
                            return bot
                                .send_photo(chat_id, InputFile::memory(bytes_vec))
                                .caption(text)
                                .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                                .reply_markup(keyboard)
                                .await;
                        }
                        Err(e) => {
                            log::warn!("Failed to get thumbnail bytes: {}", e);
                            // Не продолжаем выполнение - отправим текстовое сообщение ниже
                        }
                    }
                } else {
                    log::warn!("Thumbnail request failed with status: {}", response.status());
                }
            }
            Err(e) => {
                log::warn!("Failed to download thumbnail: {}", e);
            }
        }
    }

    // Если thumbnail не доступен или произошла ошибка, отправляем текстовое сообщение
    bot.send_message(chat_id, text)
        .parse_mode(teloxide::types::ParseMode::MarkdownV2)
        .reply_markup(keyboard)
        .await
}

/// Обновляет существующее сообщение превью (редактирует текст/подпись и клавиатуру)
///
/// Используется для возврата из меню настроек без пересоздания сообщения
pub async fn update_preview_message(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    url: &Url,
    metadata: &PreviewMetadata,
    default_format: &str,
    default_quality: Option<&str>,
    db_pool: Arc<DbPool>,
) -> ResponseResult<()> {
    let lang = crate::i18n::user_lang_from_pool(&db_pool, chat_id.0);

    // Формируем текст превью с экранированием (копия логики из send_preview)
    let escaped_title = escape_markdown(&metadata.display_title());
    let mut text = format!("🎵 *{}*\n\n", escaped_title);

    if metadata.duration.is_some() {
        let duration_str = metadata.format_duration();
        text.push_str(&format!("⏱️ Длительность: {}\n", escape_markdown(&duration_str)));
    }

    let filtered_formats = metadata
        .video_formats
        .as_ref()
        .map(|formats| filter_video_formats_by_size(formats));

    // Для видео показываем список форматов с размерами
    if default_format == "mp4" || default_format == "mp4+mp3" {
        if let Some(formats) = &filtered_formats {
            if !formats.is_empty() {
                text.push_str("\n📹 *Доступные форматы:*\n");
                for format_info in formats {
                    let size_str = if let Some(size) = format_info.size_bytes {
                        if size > 1024 * 1024 {
                            format!("{:.1} MB", size as f64 / (1024.0 * 1024.0))
                        } else if size > 1024 {
                            format!("{:.1} KB", size as f64 / 1024.0)
                        } else {
                            format!("{} B", size)
                        }
                    } else {
                        crate::i18n::t(&lang, "common.unknown")
                    };
                    let resolution_str = format_info
                        .resolution
                        .as_ref()
                        .map(|r| format!(" ({})", r))
                        .unwrap_or_default();
                    text.push_str(&format!(
                        "• {}: {}{}\n",
                        escape_markdown(&format_info.quality),
                        escape_markdown(&size_str),
                        escape_markdown(&resolution_str)
                    ));
                }
            }
        }
    } else if metadata.filesize.is_some() {
        let size_str = metadata.format_filesize();
        text.push_str(&format!("📦 Примерный размер: {}\n", escape_markdown(&size_str)));
    }

    if let Some(desc) = &metadata.description {
        text.push_str(&format!("\n📝 {}\n", escape_markdown(desc)));
    }

    text.push_str("\nВыбери формат\\:");

    // Создаем inline клавиатуру
    // Сохраняем URL в кэше и получаем короткий ID
    let url_id = cache::store_url(&db_pool, url.as_str()).await;

    // Получаем настройку send_as_document из БД для видео
    let send_as_document = if default_format == "mp4" {
        match crate::storage::db::get_connection(&db_pool) {
            Ok(conn) => crate::storage::db::get_user_send_as_document(&conn, chat_id.0).unwrap_or(0),
            Err(e) => {
                log::warn!("Failed to get db connection for send_as_document: {}", e);
                0
            }
        }
    } else {
        0
    };

    let keyboard = if default_format == "mp4" || default_format == "mp4+mp3" {
        if let Some(formats) = &filtered_formats {
            if formats.is_empty() {
                create_fallback_keyboard(default_format, default_quality, &url_id)
            } else {
                create_video_format_keyboard(formats, default_quality, &url_id, send_as_document, default_format)
            }
        } else {
            create_fallback_keyboard(default_format, default_quality, &url_id)
        }
    } else {
        create_fallback_keyboard(default_format, default_quality, &url_id)
    };

    // Пытаемся отредактировать подпись (если это фото/видео)
    let caption_req = bot
        .edit_message_caption(chat_id, message_id)
        .caption(text.clone())
        .parse_mode(teloxide::types::ParseMode::MarkdownV2)
        .reply_markup(keyboard.clone());

    match caption_req.await {
        Ok(_) => Ok(()),
        Err(_) => {
            // Если не получилось (например, это текстовое сообщение), редактируем текст
            bot.edit_message_text(chat_id, message_id, text)
                .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                .reply_markup(keyboard)
                .await?;
            Ok(())
        }
    }
}

/// Создает стандартную клавиатуру с кнопкой скачивания
///
/// Используется как fallback когда список форматов недоступен
///
/// # Параметры
/// - `default_format` - формат файла (mp3, mp4, srt, txt)
/// - `default_quality` - качество видео (только для mp4: "1080p", "720p", "480p", "360p", "best")
/// - `url_id` - ID URL в кэше
fn create_fallback_keyboard(default_format: &str, default_quality: Option<&str>, url_id: &str) -> InlineKeyboardMarkup {
    // Формируем текст кнопки с учетом формата и качества
    let (button_text, callback_data) = match default_format {
        "mp4" => {
            // Для видео показываем качество
            let (quality_display, quality_for_callback) = match default_quality {
                Some("1080p") => ("1080p", "1080p"),
                Some("720p") => ("720p", "720p"),
                Some("480p") => ("480p", "480p"),
                Some("360p") => ("360p", "360p"),
                Some("best") => ("Best", "best"),
                _ => ("Best", "best"), // По умолчанию используем "best" вместо "MP4"
            };

            // Формируем callback data: для mp4 всегда используем формат dl:mp4:quality:url_id
            let callback = format!("dl:mp4:{}:{}", quality_for_callback, url_id);

            (format!("📥 Скачать ({})", quality_display), callback)
        }
        "mp3" => ("📥 Скачать (MP3)".to_string(), format!("dl:mp3:{}", url_id)),
        "mp4+mp3" => ("📥 Скачать (MP4 + MP3)".to_string(), format!("dl:mp4+mp3:{}", url_id)),
        "srt" => ("📥 Скачать (SRT)".to_string(), format!("dl:srt:{}", url_id)),
        "txt" => ("📥 Скачать (TXT)".to_string(), format!("dl:txt:{}", url_id)),
        _ => ("📥 Скачать (MP3)".to_string(), format!("dl:mp3:{}", url_id)),
    };

    InlineKeyboardMarkup::new(vec![
        vec![InlineKeyboardButton::callback(button_text, callback_data)],
        vec![InlineKeyboardButton::callback(
            "⚙️ Настройки".to_string(),
            format!("pv:set:{}", url_id),
        )],
        vec![InlineKeyboardButton::callback(
            "❌ Отмена".to_string(),
            format!("pv:cancel:{}", url_id),
        )],
    ])
}

/// Создает клавиатуру для выбора формата видео
///
/// - Большая кнопка для default формата (из настроек пользователя)
/// - Маленькие кнопки для остальных форматов (по 2 в ряд)
/// - Toggle кнопка для выбора Media/Document
/// - Большая кнопка "Отмена" внизу
fn create_video_format_keyboard(
    formats: &[VideoFormatInfo],
    default_quality: Option<&str>,
    url_id: &str,
    send_as_document: i32,
    default_format: &str,
) -> InlineKeyboardMarkup {
    let mut buttons: Vec<Vec<InlineKeyboardButton>> = Vec::new();

    // Находим default формат (из настроек пользователя)
    // Маппим "best" на первый (лучший) формат из списка
    let default_format_info = if let Some(quality) = default_quality {
        if quality == "best" {
            formats.first()
        } else {
            formats
                .iter()
                .find(|f| f.quality == quality)
                .or_else(|| formats.first())
        }
    } else {
        formats.first()
    };

    // Большая кнопка для default формата (только для MP4, для MP4+MP3 показываем все как маленькие)
    if default_format != "mp4+mp3" {
        if let Some(format_info) = default_format_info {
            let size_str = format_info
                .size_bytes
                .map(|s| {
                    if s > 1024 * 1024 {
                        format!("{:.1} MB", s as f64 / (1024.0 * 1024.0))
                    } else if s > 1024 {
                        format!("{:.1} KB", s as f64 / 1024.0)
                    } else {
                        format!("{} B", s)
                    }
                })
                .unwrap_or_else(|| "?".to_string());

            buttons.push(vec![InlineKeyboardButton::callback(
                format!("📥 {} ({})", format_info.quality, size_str),
                format!("dl:{}:{}:{}", default_format, format_info.quality, url_id),
            )]);
        }
    }

    // Маленькие кнопки для форматов (по 2 в ряд)
    // Для MP4+MP3 показываем ВСЕ форматы, для MP4 - исключаем default и показываем максимум 4
    let mut row = Vec::new();
    let default_index = if default_format == "mp4+mp3" {
        usize::MAX // Для MP4+MP3 не исключаем default, показываем все
    } else {
        default_format_info
            .and_then(|df| formats.iter().position(|f| f.quality == df.quality))
            .unwrap_or(usize::MAX) // Если default не найден, пропускаем все
    };

    let mut added_count = 0;
    // Для MP4+MP3 показываем все форматы, для MP4 - максимум 4 дополнительных
    let max_formats = if default_format == "mp4+mp3" {
        formats.len() // Показываем все форматы для MP4+MP3
    } else {
        4 // Для MP4 показываем максимум 4 дополнительных формата
    };

    for (idx, format_info) in formats.iter().enumerate() {
        // Для MP4 пропускаем default, для MP4+MP3 показываем все
        if default_format != "mp4+mp3" && idx == default_index {
            continue; // Пропускаем default формат только для MP4
        }

        if added_count >= max_formats {
            break;
        }

        let size_str = format_info
            .size_bytes
            .map(|s| {
                if s > 1024 * 1024 {
                    format!("{:.1}MB", s as f64 / (1024.0 * 1024.0))
                } else if s > 1024 {
                    format!("{:.1}KB", s as f64 / 1024.0)
                } else {
                    format!("{}B", s)
                }
            })
            .unwrap_or_else(|| "?".to_string());

        row.push(InlineKeyboardButton::callback(
            format!("{} {}", format_info.quality, size_str),
            format!("dl:{}:{}:{}", default_format, format_info.quality, url_id),
        ));
        added_count += 1;

        if row.len() == 2 {
            buttons.push(row);
            row = Vec::new();
        }
    }

    // Добавляем оставшиеся кнопки если есть
    if !row.is_empty() {
        buttons.push(row);
    }

    // Toggle кнопка для выбора типа отправки (Media/Document)
    buttons.push(vec![InlineKeyboardButton::callback(
        if send_as_document == 0 {
            "📹 Отправка: Media ✓"
        } else {
            "📄 Отправка: Document ✓"
        }
        .to_string(),
        format!("video_send_type:toggle:{}", url_id),
    )]);

    // Кнопка "Настройки"
    buttons.push(vec![InlineKeyboardButton::callback(
        "⚙️ Настройки".to_string(),
        format!("pv:set:{}", url_id),
    )]);

    // Большая кнопка "Отмена" внизу
    buttons.push(vec![InlineKeyboardButton::callback(
        "❌ Отмена".to_string(),
        format!("pv:cancel:{}", url_id),
    )]);

    InlineKeyboardMarkup::new(buttons)
}
