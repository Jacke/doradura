use crate::core::config;
use crate::core::error::AppError;
use crate::core::escape_markdown;
use crate::download::metadata::add_cookies_args;
use crate::download::ytdlp_errors::{analyze_ytdlp_error, get_error_message};
use crate::storage::cache;
use crate::storage::db::DbPool;
use crate::telegram::Bot;
use serde_json::Value;
use std::collections::HashMap;
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

fn parse_resolution_string(resolution: &str) -> Option<(u64, u64)> {
    let mut parts = resolution.split('x');
    let width_part = parts.next()?;
    let height_part = parts.next()?;

    let width_str: String = width_part.chars().filter(|c| c.is_ascii_digit()).collect();
    let height_str: String = height_part.chars().filter(|c| c.is_ascii_digit()).collect();

    if width_str.is_empty() || height_str.is_empty() {
        return None;
    }

    let width = width_str.parse::<u64>().ok()?;
    let height = height_str.parse::<u64>().ok()?;

    Some((width, height))
}

fn quality_from_short_side(short_side: u64) -> Option<&'static str> {
    match short_side {
        1080 => Some("1080p"),
        720 => Some("720p"),
        480 => Some("480p"),
        360 => Some("360p"),
        _ => None,
    }
}

fn quality_from_dimensions(width: Option<u64>, height: Option<u64>) -> Option<&'static str> {
    let short_side = match (width, height) {
        (Some(w), Some(h)) => w.min(h),
        (Some(w), None) => w,
        (None, Some(h)) => h,
        _ => return None,
    };

    quality_from_short_side(short_side)
}

fn quality_from_note(note: &str) -> Option<&'static str> {
    let lowered = note.to_ascii_lowercase();
    if lowered.contains("1080") {
        Some("1080p")
    } else if lowered.contains("720") {
        Some("720p")
    } else if lowered.contains("480") {
        Some("480p")
    } else if lowered.contains("360") {
        Some("360p")
    } else {
        None
    }
}

fn keyboard_stats(keyboard: &InlineKeyboardMarkup) -> (usize, usize) {
    let rows = keyboard.inline_keyboard.len();
    let buttons = keyboard.inline_keyboard.iter().map(|row| row.len()).sum();
    (rows, buttons)
}

fn extract_video_formats_from_json(json: &Value) -> Vec<VideoFormatInfo> {
    let formats = match json.get("formats").and_then(|v| v.as_array()) {
        Some(formats) => formats,
        None => return Vec::new(),
    };

    let mut best_audio_size: Option<u64> = None;
    for format in formats {
        let vcodec = format.get("vcodec").and_then(|v| v.as_str()).unwrap_or("");
        if vcodec != "none" {
            continue;
        }

        let size = format
            .get("filesize")
            .or_else(|| format.get("filesize_approx"))
            .and_then(|v| v.as_u64());
        if let Some(size) = size {
            if best_audio_size.is_none_or(|current| size > current) {
                best_audio_size = Some(size);
            }
        }
    }

    let mut by_quality: HashMap<String, VideoFormatInfo> = HashMap::new();

    for format in formats {
        let vcodec = format.get("vcodec").and_then(|v| v.as_str()).unwrap_or("");
        if vcodec == "none" {
            continue;
        }

        let mut width = format.get("width").and_then(|v| v.as_u64());
        let mut height = format.get("height").and_then(|v| v.as_u64());
        let resolution_field = format.get("resolution").and_then(|v| v.as_str());

        if width.is_none() || height.is_none() {
            if let Some(resolution) = resolution_field {
                if let Some((parsed_width, parsed_height)) = parse_resolution_string(resolution) {
                    width = width.or(Some(parsed_width));
                    height = height.or(Some(parsed_height));
                }
            }
        }

        let mut quality = quality_from_dimensions(width, height);
        if quality.is_none() {
            if let Some(note) = format.get("format_note").and_then(|v| v.as_str()) {
                quality = quality_from_note(note);
            }
        }
        if quality.is_none() {
            if let Some(resolution) = resolution_field {
                if let Some((parsed_width, parsed_height)) = parse_resolution_string(resolution) {
                    quality = quality_from_dimensions(Some(parsed_width), Some(parsed_height));
                }
            }
        }

        let quality = match quality {
            Some(value) => value,
            None => continue,
        };

        let mut size_bytes = format
            .get("filesize")
            .or_else(|| format.get("filesize_approx"))
            .and_then(|v| v.as_u64());

        let acodec = format.get("acodec").and_then(|v| v.as_str()).unwrap_or("");
        if acodec == "none" {
            if let (Some(size), Some(audio_size)) = (size_bytes, best_audio_size) {
                size_bytes = Some(size + audio_size);
            }
        }

        let resolution = match (width, height) {
            (Some(w), Some(h)) => Some(format!("{}x{}", w, h)),
            _ => resolution_field
                .map(|value| value.to_string())
                .filter(|value| value != "unknown"),
        };

        let mut candidate = VideoFormatInfo {
            quality: quality.to_string(),
            size_bytes,
            resolution,
        };

        if let Some(existing) = by_quality.get_mut(quality) {
            let replace = match (existing.size_bytes, candidate.size_bytes) {
                (None, Some(_)) => true,
                (Some(current), Some(new)) => new > current,
                _ => false,
            };

            if replace {
                existing.size_bytes = candidate.size_bytes;
                if candidate.resolution.is_some() {
                    existing.resolution = candidate.resolution.take();
                }
            } else if existing.resolution.is_none() {
                existing.resolution = candidate.resolution.take();
            }
        } else {
            by_quality.insert(quality.to_string(), candidate);
        }
    }

    let mut ordered = Vec::new();
    for quality in ["1080p", "720p", "480p", "360p"] {
        if let Some(info) = by_quality.remove(quality) {
            ordered.push(info);
        }
    }

    ordered
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
    if let Some(mut metadata) = PREVIEW_CACHE.get(url.as_str()).await {
        log::debug!("Preview metadata found in cache for URL: {}", url);
        let needs_video_formats = metadata.video_formats.as_ref().is_none_or(|formats| formats.is_empty());
        if needs_video_formats {
            match get_video_formats_list(url, ytdl_bin).await {
                Ok(formats) if !formats.is_empty() => {
                    log::debug!("Refreshed video formats for cached preview ({} formats)", formats.len());
                    metadata.video_formats = Some(formats);
                    PREVIEW_CACHE.set(url.as_str().to_string(), metadata.clone()).await;
                }
                Ok(_) => {}
                Err(e) => {
                    log::warn!("Failed to refresh video formats for cached preview: {}", e);
                }
            }
        }
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

    // Получаем список доступных форматов с размерами (если они есть у источника).
    // Используем --list-formats, так как JSON не всегда содержит точные размеры для всех форматов.
    let mut video_formats: Option<Vec<VideoFormatInfo>> = match get_video_formats_list(url, ytdl_bin).await {
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
    };

    if video_formats.as_ref().is_none_or(|formats| formats.is_empty()) {
        let json_formats = extract_video_formats_from_json(&json_metadata);
        if !json_formats.is_empty() {
            log::info!(
                "Using video formats from JSON metadata for URL {} ({} formats)",
                url,
                json_formats.len()
            );
            video_formats = Some(json_formats);
        }
    }

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
    list_formats_args.push("--extractor-args".to_string());
    list_formats_args.push("youtube:player_client=default,web_safari,web_embedded".to_string());
    list_formats_args.push(url.as_str().to_string());

    let command_str = format!("{} {}", ytdl_bin, list_formats_args.join(" "));
    log::info!("[DEBUG] yt-dlp command for preview formats: {}", command_str);

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

        // Возвращаем пользовательское сообщение об ошибке
        return Err(AppError::Download(get_error_message(&error_type)));
    }

    let formats_output = String::from_utf8_lossy(&list_formats_output.stdout);
    let output_line_count = formats_output.lines().count();
    log::debug!(
        "yt-dlp --list-formats output received ({} bytes, {} lines)",
        formats_output.len(),
        output_line_count
    );
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
                    if found_resolution.is_none() {
                        for &res in &resolutions {
                            if line.contains(res) {
                                found_resolution = Some(res.to_string());
                                break;
                            }
                        }
                    }

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
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        if max_size.is_some() || found_resolution.is_some() {
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

    if formats.is_empty() {
        log::warn!(
            "No video formats parsed from --list-formats output ({} lines)",
            output_line_count
        );
    }

    Ok(formats)
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

    let has_video_formats = filtered_formats.as_ref().is_some_and(|formats| !formats.is_empty());
    let raw_formats_len = metadata
        .video_formats
        .as_ref()
        .map(|formats| formats.len())
        .unwrap_or(0);
    let filtered_formats_len = filtered_formats.as_ref().map(|formats| formats.len()).unwrap_or(0);
    log::info!(
        "Preview formats for {}: raw={}, filtered={}, has_video_formats={}, format={}",
        url,
        raw_formats_len,
        filtered_formats_len,
        has_video_formats,
        default_format
    );

    // Для видео показываем список форматов с размерами
    if has_video_formats {
        if let Some(formats) = &filtered_formats {
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

    let (send_as_document, audio_bitrate) = match crate::storage::db::get_connection(&db_pool) {
        Ok(conn) => {
            let send_as_document = if has_video_formats {
                crate::storage::db::get_user_send_as_document(&conn, chat_id.0).unwrap_or(0)
            } else {
                0
            };
            let audio_bitrate =
                crate::storage::db::get_user_audio_bitrate(&conn, chat_id.0).unwrap_or_else(|_| "320k".to_string());
            (send_as_document, audio_bitrate)
        }
        Err(e) => {
            log::warn!("Failed to get db connection for preview settings: {}", e);
            (0, "320k".to_string())
        }
    };

    // Получаем message_id нового preview сообщения (будет установлен после отправки)
    // Пока используем временное значение 0, потом обновим после отправки
    let keyboard = if has_video_formats {
        if let Some(formats) = &filtered_formats {
            if formats.is_empty() {
                log::warn!(
                    "video_formats is Some but empty, using fallback button for {}",
                    default_format
                );
                // Если список форматов пустой, создаем стандартную кнопку
                create_fallback_keyboard(default_format, default_quality, &url_id, Some(audio_bitrate.as_str()))
            } else {
                let format_for_keyboard = if default_format == "mp4" || default_format == "mp4+mp3" {
                    default_format
                } else {
                    "mp4"
                };
                log::debug!(
                    "Creating video format keyboard with {} formats for {} (format_for_keyboard={})",
                    formats.len(),
                    default_format,
                    format_for_keyboard
                );
                // Для видео создаем кнопки для выбора формата с toggle для Media/Document
                create_video_format_keyboard(
                    formats,
                    default_quality,
                    &url_id,
                    send_as_document,
                    format_for_keyboard,
                    Some(audio_bitrate.as_str()),
                )
            }
        } else {
            // Если video_formats is None - создаем стандартную кнопку
            create_fallback_keyboard(default_format, default_quality, &url_id, Some(audio_bitrate.as_str()))
        }
    } else {
        // Для других форматов или если video_formats is None - стандартные кнопки
        log::debug!(
            "Creating fallback keyboard for format: {} (video_formats.is_some() = {})",
            default_format,
            metadata.video_formats.is_some()
        );
        create_fallback_keyboard(default_format, default_quality, &url_id, Some(audio_bitrate.as_str()))
    };
    let (keyboard_rows, keyboard_buttons) = keyboard_stats(&keyboard);
    log::info!(
        "Preview keyboard built (rows={}, buttons={}, format={}, quality={:?}, url_id={}, send_as_document={})",
        keyboard_rows,
        keyboard_buttons,
        default_format,
        default_quality,
        url_id,
        send_as_document
    );

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
                            log::info!(
                                "Sending preview photo ({} bytes) for url_id={}",
                                bytes_vec.len(),
                                url_id
                            );
                            let send_result = bot
                                .send_photo(chat_id, InputFile::memory(bytes_vec))
                                .caption(text)
                                .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                                .reply_markup(keyboard)
                                .await;
                            if let Ok(ref message) = send_result {
                                log::info!("Preview photo sent: message_id={}", message.id);
                            }
                            return send_result;
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
    log::info!("Sending preview text message for url_id={}", url_id);
    let send_result = bot
        .send_message(chat_id, text)
        .parse_mode(teloxide::types::ParseMode::MarkdownV2)
        .reply_markup(keyboard)
        .await;
    if let Ok(ref message) = send_result {
        log::info!("Preview text sent: message_id={}", message.id);
    }
    send_result
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

    let has_video_formats = filtered_formats.as_ref().is_some_and(|formats| !formats.is_empty());
    let raw_formats_len = metadata
        .video_formats
        .as_ref()
        .map(|formats| formats.len())
        .unwrap_or(0);
    let filtered_formats_len = filtered_formats.as_ref().map(|formats| formats.len()).unwrap_or(0);
    log::info!(
        "Update preview formats for {}: raw={}, filtered={}, has_video_formats={}, format={}",
        url,
        raw_formats_len,
        filtered_formats_len,
        has_video_formats,
        default_format
    );

    // Для видео показываем список форматов с размерами
    if has_video_formats {
        if let Some(formats) = &filtered_formats {
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

    let mut resolved_quality = default_quality.map(|q| q.to_string());
    let mut audio_bitrate = "320k".to_string();
    let mut send_as_document = 0;
    match crate::storage::db::get_connection(&db_pool) {
        Ok(conn) => {
            audio_bitrate =
                crate::storage::db::get_user_audio_bitrate(&conn, chat_id.0).unwrap_or_else(|_| "320k".to_string());
            if has_video_formats {
                if resolved_quality.is_none() {
                    resolved_quality = Some(
                        crate::storage::db::get_user_video_quality(&conn, chat_id.0)
                            .unwrap_or_else(|_| "best".to_string()),
                    );
                }
                send_as_document = crate::storage::db::get_user_send_as_document(&conn, chat_id.0).unwrap_or(0);
            }
        }
        Err(e) => {
            log::warn!("Failed to get db connection for preview settings: {}", e);
        }
    }

    let keyboard = if has_video_formats {
        let formats = filtered_formats.as_deref().unwrap_or(&[]);
        if formats.is_empty() {
            create_fallback_keyboard(default_format, default_quality, &url_id, Some(audio_bitrate.as_str()))
        } else {
            create_video_format_keyboard(
                formats,
                resolved_quality.as_deref(),
                &url_id,
                send_as_document,
                "mp4",
                Some(audio_bitrate.as_str()),
            )
        }
    } else {
        create_fallback_keyboard(default_format, default_quality, &url_id, Some(audio_bitrate.as_str()))
    };
    let (keyboard_rows, keyboard_buttons) = keyboard_stats(&keyboard);
    log::info!(
        "Preview update keyboard built (rows={}, buttons={}, format={}, quality={:?}, url_id={}, send_as_document={})",
        keyboard_rows,
        keyboard_buttons,
        default_format,
        resolved_quality.as_deref(),
        url_id,
        send_as_document
    );

    // Пытаемся отредактировать подпись (если это фото/видео)
    let caption_req = bot
        .edit_message_caption(chat_id, message_id)
        .caption(text.clone())
        .parse_mode(teloxide::types::ParseMode::MarkdownV2)
        .reply_markup(keyboard.clone());

    match caption_req.await {
        Ok(_) => Ok(()),
        Err(e) => {
            log::debug!(
                "Failed to edit preview caption for message_id={}, falling back to text: {:?}",
                message_id,
                e
            );
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
fn create_fallback_keyboard(
    default_format: &str,
    default_quality: Option<&str>,
    url_id: &str,
    audio_bitrate: Option<&str>,
) -> InlineKeyboardMarkup {
    log::debug!(
        "Creating fallback preview keyboard (format={}, quality={:?}, url_id={})",
        default_format,
        default_quality,
        url_id
    );
    let mp3_label = audio_bitrate
        .map(|bitrate| format!("MP3 {}", bitrate))
        .unwrap_or_else(|| "MP3".to_string());

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
        "mp3" => (format!("📥 Скачать ({})", mp3_label), format!("dl:mp3:{}", url_id)),
        "mp4+mp3" => ("📥 Скачать (MP4 + MP3)".to_string(), format!("dl:mp4+mp3:{}", url_id)),
        "srt" => ("📥 Скачать (SRT)".to_string(), format!("dl:srt:{}", url_id)),
        "txt" => ("📥 Скачать (TXT)".to_string(), format!("dl:txt:{}", url_id)),
        _ => (format!("📥 Скачать ({})", mp3_label), format!("dl:mp3:{}", url_id)),
    };

    let mut rows = vec![vec![InlineKeyboardButton::callback(button_text, callback_data)]];

    if default_format == "mp4" || default_format == "mp4+mp3" {
        rows.push(vec![InlineKeyboardButton::callback(
            format!("🎵 {}", mp3_label),
            format!("dl:mp3:{}", url_id),
        )]);
    }

    rows.push(vec![InlineKeyboardButton::callback(
        "⚙️ Настройки".to_string(),
        format!("pv:set:{}", url_id),
    )]);
    rows.push(vec![InlineKeyboardButton::callback(
        "❌ Отмена".to_string(),
        format!("pv:cancel:{}", url_id),
    )]);

    InlineKeyboardMarkup::new(rows)
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
    audio_bitrate: Option<&str>,
) -> InlineKeyboardMarkup {
    log::debug!(
        "Creating video format keyboard (formats={}, default_quality={:?}, url_id={}, send_as_document={}, format={})",
        formats.len(),
        default_quality,
        url_id,
        send_as_document,
        default_format
    );
    let mp3_label = audio_bitrate
        .map(|bitrate| format!("MP3 {}", bitrate))
        .unwrap_or_else(|| "MP3".to_string());
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

    buttons.push(vec![InlineKeyboardButton::callback(
        format!("🎵 {}", mp3_label),
        format!("dl:mp3:{}", url_id),
    )]);

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

#[cfg(test)]
mod tests {
    use super::*;

    // ==================== parse_resolution_string tests ====================

    #[test]
    fn test_parse_resolution_string_standard() {
        assert_eq!(parse_resolution_string("1920x1080"), Some((1920, 1080)));
        assert_eq!(parse_resolution_string("1280x720"), Some((1280, 720)));
        assert_eq!(parse_resolution_string("640x480"), Some((640, 480)));
    }

    #[test]
    fn test_parse_resolution_string_with_extra_chars() {
        // Sometimes yt-dlp returns resolutions with extra characters
        assert_eq!(parse_resolution_string("1920x1080p"), Some((1920, 1080)));
    }

    #[test]
    fn test_parse_resolution_string_invalid() {
        assert_eq!(parse_resolution_string(""), None);
        assert_eq!(parse_resolution_string("1920"), None);
        assert_eq!(parse_resolution_string("invalid"), None);
        assert_eq!(parse_resolution_string("x1080"), None);
        assert_eq!(parse_resolution_string("1920x"), None);
    }

    // ==================== quality_from_short_side tests ====================

    #[test]
    fn test_quality_from_short_side_standard() {
        assert_eq!(quality_from_short_side(1080), Some("1080p"));
        assert_eq!(quality_from_short_side(720), Some("720p"));
        assert_eq!(quality_from_short_side(480), Some("480p"));
        assert_eq!(quality_from_short_side(360), Some("360p"));
    }

    #[test]
    fn test_quality_from_short_side_unknown() {
        assert_eq!(quality_from_short_side(1440), None);
        assert_eq!(quality_from_short_side(240), None);
        assert_eq!(quality_from_short_side(0), None);
    }

    // ==================== quality_from_dimensions tests ====================

    #[test]
    fn test_quality_from_dimensions_both() {
        // Standard video with width > height (landscape)
        assert_eq!(quality_from_dimensions(Some(1920), Some(1080)), Some("1080p"));
        assert_eq!(quality_from_dimensions(Some(1280), Some(720)), Some("720p"));
    }

    #[test]
    fn test_quality_from_dimensions_portrait() {
        // Portrait video (height > width)
        assert_eq!(quality_from_dimensions(Some(1080), Some(1920)), Some("1080p"));
    }

    #[test]
    fn test_quality_from_dimensions_partial() {
        assert_eq!(quality_from_dimensions(Some(1080), None), Some("1080p"));
        assert_eq!(quality_from_dimensions(None, Some(720)), Some("720p"));
    }

    #[test]
    fn test_quality_from_dimensions_none() {
        assert_eq!(quality_from_dimensions(None, None), None);
    }

    // ==================== quality_from_note tests ====================

    #[test]
    fn test_quality_from_note_matches() {
        assert_eq!(quality_from_note("1080p"), Some("1080p"));
        assert_eq!(quality_from_note("720p HD"), Some("720p"));
        assert_eq!(quality_from_note("480p SD"), Some("480p"));
        assert_eq!(quality_from_note("360p"), Some("360p"));
    }

    #[test]
    fn test_quality_from_note_case_insensitive() {
        assert_eq!(quality_from_note("1080P"), Some("1080p"));
        assert_eq!(quality_from_note("FULL HD 1080"), Some("1080p"));
    }

    #[test]
    fn test_quality_from_note_no_match() {
        assert_eq!(quality_from_note(""), None);
        assert_eq!(quality_from_note("audio only"), None);
        assert_eq!(quality_from_note("240p"), None);
    }

    // ==================== keyboard_stats tests ====================

    #[test]
    fn test_keyboard_stats_empty() {
        let keyboard = InlineKeyboardMarkup::new(Vec::<Vec<InlineKeyboardButton>>::new());
        assert_eq!(keyboard_stats(&keyboard), (0, 0));
    }

    #[test]
    fn test_keyboard_stats_single_row() {
        let keyboard = InlineKeyboardMarkup::new(vec![vec![
            InlineKeyboardButton::callback("Button 1", "data1"),
            InlineKeyboardButton::callback("Button 2", "data2"),
        ]]);
        assert_eq!(keyboard_stats(&keyboard), (1, 2));
    }

    #[test]
    fn test_keyboard_stats_multiple_rows() {
        let keyboard = InlineKeyboardMarkup::new(vec![
            vec![InlineKeyboardButton::callback("A", "a")],
            vec![
                InlineKeyboardButton::callback("B", "b"),
                InlineKeyboardButton::callback("C", "c"),
            ],
            vec![
                InlineKeyboardButton::callback("D", "d"),
                InlineKeyboardButton::callback("E", "e"),
                InlineKeyboardButton::callback("F", "f"),
            ],
        ]);
        assert_eq!(keyboard_stats(&keyboard), (3, 6));
    }

    // ==================== escape_markdown tests ====================

    #[test]
    fn test_escape_markdown_underscore() {
        assert_eq!(escape_markdown("hello_world"), "hello\\_world");
    }

    #[test]
    fn test_escape_markdown_asterisk() {
        assert_eq!(escape_markdown("*bold*"), "\\*bold\\*");
    }

    #[test]
    fn test_escape_markdown_brackets() {
        assert_eq!(escape_markdown("[link](url)"), "\\[link\\]\\(url\\)");
    }

    #[test]
    fn test_escape_markdown_backslash() {
        // This escape_markdown also handles backslash
        assert_eq!(escape_markdown("path\\to\\file"), "path\\\\to\\\\file");
    }

    #[test]
    fn test_escape_markdown_all_special() {
        let all_special = "\\_*[]()~`>#+-=|{}.!";
        let escaped = escape_markdown(all_special);
        assert_eq!(escaped, "\\\\\\_\\*\\[\\]\\(\\)\\~\\`\\>\\#\\+\\-\\=\\|\\{\\}\\.\\!");
    }

    #[test]
    fn test_escape_markdown_empty() {
        assert_eq!(escape_markdown(""), "");
    }

    #[test]
    fn test_escape_markdown_no_special() {
        assert_eq!(escape_markdown("hello world 123"), "hello world 123");
    }

    // ==================== filter_video_formats_by_size tests ====================

    #[test]
    fn test_filter_video_formats_by_size_empty() {
        let formats: Vec<VideoFormatInfo> = vec![];
        let filtered = filter_video_formats_by_size(&formats);
        assert!(filtered.is_empty());
    }

    #[test]
    fn test_filter_video_formats_by_size_all_pass() {
        let formats = vec![
            VideoFormatInfo {
                quality: "1080p".to_string(),
                size_bytes: Some(500 * 1024 * 1024), // 500MB
                resolution: Some("1920x1080".to_string()),
            },
            VideoFormatInfo {
                quality: "720p".to_string(),
                size_bytes: Some(300 * 1024 * 1024), // 300MB
                resolution: Some("1280x720".to_string()),
            },
        ];
        let filtered = filter_video_formats_by_size(&formats);
        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn test_filter_video_formats_by_size_filters_large() {
        let formats = vec![
            VideoFormatInfo {
                quality: "1080p".to_string(),
                size_bytes: Some(3 * 1024 * 1024 * 1024), // 3GB - too large
                resolution: Some("1920x1080".to_string()),
            },
            VideoFormatInfo {
                quality: "720p".to_string(),
                size_bytes: Some(300 * 1024 * 1024), // 300MB
                resolution: Some("1280x720".to_string()),
            },
        ];
        let filtered = filter_video_formats_by_size(&formats);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].quality, "720p");
    }

    #[test]
    fn test_filter_video_formats_by_size_none_passes() {
        let formats = vec![VideoFormatInfo {
            quality: "1080p".to_string(),
            size_bytes: None, // Unknown size - should pass
            resolution: None,
        }];
        let filtered = filter_video_formats_by_size(&formats);
        assert_eq!(filtered.len(), 1);
    }

    // ==================== get_json_value tests ====================

    #[test]
    fn test_get_json_value_string() {
        let json: Value = serde_json::json!({"title": "Test Video"});
        assert_eq!(get_json_value(&json, "title"), Some("Test Video".to_string()));
    }

    #[test]
    fn test_get_json_value_number() {
        let json: Value = serde_json::json!({"duration": 120});
        assert_eq!(get_json_value(&json, "duration"), Some("120".to_string()));
    }

    #[test]
    fn test_get_json_value_null() {
        let json: Value = serde_json::json!({"title": null});
        assert_eq!(get_json_value(&json, "title"), None);
    }

    #[test]
    fn test_get_json_value_missing() {
        let json: Value = serde_json::json!({"other": "value"});
        assert_eq!(get_json_value(&json, "title"), None);
    }

    #[test]
    fn test_get_json_value_empty_string() {
        let json: Value = serde_json::json!({"title": ""});
        assert_eq!(get_json_value(&json, "title"), None);
    }

    #[test]
    fn test_get_json_value_na() {
        let json: Value = serde_json::json!({"title": "NA"});
        assert_eq!(get_json_value(&json, "title"), None);
    }

    #[test]
    fn test_get_json_value_trims_whitespace() {
        let json: Value = serde_json::json!({"title": "  Test  "});
        assert_eq!(get_json_value(&json, "title"), Some("Test".to_string()));
    }

    // ==================== get_video_filesize_from_json tests ====================

    #[test]
    fn test_get_video_filesize_from_json_found() {
        let json: Value = serde_json::json!({
            "formats": [
                {"height": 720, "filesize": 100000000},
                {"height": 1080, "filesize": 200000000}
            ]
        });
        assert_eq!(get_video_filesize_from_json(&json, "1080p"), Some(200000000));
        assert_eq!(get_video_filesize_from_json(&json, "720p"), Some(100000000));
    }

    #[test]
    fn test_get_video_filesize_from_json_approx() {
        let json: Value = serde_json::json!({
            "formats": [
                {"height": 720, "filesize_approx": 100000000}
            ]
        });
        assert_eq!(get_video_filesize_from_json(&json, "720p"), Some(100000000));
    }

    #[test]
    fn test_get_video_filesize_from_json_not_found() {
        let json: Value = serde_json::json!({
            "formats": [
                {"height": 720, "filesize": 100000000}
            ]
        });
        assert_eq!(get_video_filesize_from_json(&json, "1080p"), None);
    }

    #[test]
    fn test_get_video_filesize_from_json_invalid_quality() {
        let json: Value = serde_json::json!({"formats": []});
        assert_eq!(get_video_filesize_from_json(&json, "best"), None);
        assert_eq!(get_video_filesize_from_json(&json, "invalid"), None);
    }

    #[test]
    fn test_get_video_filesize_from_json_no_formats() {
        let json: Value = serde_json::json!({});
        assert_eq!(get_video_filesize_from_json(&json, "1080p"), None);
    }

    // ==================== MAX_VIDEO_FORMAT_SIZE_BYTES constant tests ====================

    #[test]
    fn test_max_video_format_size() {
        assert_eq!(MAX_VIDEO_FORMAT_SIZE_BYTES, 2 * 1024 * 1024 * 1024); // 2GB
    }
}
