use crate::core::config;
use crate::core::error::AppError;
use crate::download::error::DownloadError;
use crate::download::metadata::{add_cookies_args_with_proxy, get_proxy_chain, is_proxy_related_error};
use crate::download::ytdlp_errors::{analyze_ytdlp_error, get_error_message, YtDlpErrorType};
use crate::telegram::types::VideoFormatInfo;
use serde_json::Value;
use std::collections::HashMap;
use tokio::process::Command as TokioCommand;
use tokio::time::timeout;
use url::Url;

pub(super) const MAX_VIDEO_FORMAT_SIZE_BYTES: u64 = 2 * 1024 * 1024 * 1024;

pub fn filter_video_formats_by_size(formats: &[VideoFormatInfo]) -> Vec<VideoFormatInfo> {
    formats
        .iter()
        .filter(|format| format.size_bytes.is_none_or(|size| size <= MAX_VIDEO_FORMAT_SIZE_BYTES))
        .cloned()
        .collect()
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

pub fn extract_video_formats_from_json(json: &Value) -> Vec<VideoFormatInfo> {
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

/// Получает список доступных форматов видео с размерами
///
/// Парсит вывод yt-dlp --list-formats и извлекает информацию о форматах:
/// - 1080p, 720p, 480p, 360p
/// - Размеры файлов
/// - Разрешения
///
/// При ошибке связанной с прокси автоматически пробует следующий прокси из цепочки.
pub async fn get_video_formats_list(url: &Url, ytdl_bin: &str) -> Result<Vec<VideoFormatInfo>, AppError> {
    let proxy_chain = get_proxy_chain();
    let total_proxies = proxy_chain.len();
    let mut last_error: Option<AppError> = None;

    // Try each proxy in the chain
    let formats_output: String = 'proxy_loop: {
        for (attempt, proxy_option) in proxy_chain.into_iter().enumerate() {
            let proxy_name = proxy_option
                .as_ref()
                .map(|p| p.name.clone())
                .unwrap_or_else(|| "Direct (no proxy)".to_string());

            log::info!(
                "📡 Formats list attempt {}/{} using [{}]",
                attempt + 1,
                total_proxies,
                proxy_name
            );

            let mut list_formats_args: Vec<&str> = vec!["--list-formats", "--no-playlist", "--age-limit", "99"];

            // Add proxy and cookies
            add_cookies_args_with_proxy(&mut list_formats_args, proxy_option.as_ref());

            list_formats_args.push("--extractor-args");
            list_formats_args.push("youtube:player_client=android,web_music;formats=missing_pot");
            list_formats_args.push("--js-runtimes");
            list_formats_args.push("deno");
            list_formats_args.push("--impersonate");
            list_formats_args.push("Chrome-131:Android-14");
            list_formats_args.push(url.as_str());

            let command_str = format!("{} {}", ytdl_bin, list_formats_args.join(" "));
            log::debug!("yt-dlp command for preview formats: {}", command_str);

            let list_formats_output = match timeout(
                config::download::ytdlp_timeout(),
                TokioCommand::new(ytdl_bin).args(&list_formats_args).output(),
            )
            .await
            {
                Ok(Ok(output)) => output,
                Ok(Err(e)) => {
                    log::warn!("🔄 Failed to execute yt-dlp with [{}]: {}", proxy_name, e);
                    last_error = Some(AppError::Download(DownloadError::YtDlp(format!(
                        "Failed to get formats list: {}",
                        e
                    ))));
                    continue;
                }
                Err(_) => {
                    log::warn!("🔄 yt-dlp command timed out with [{}], trying next proxy", proxy_name);
                    last_error = Some(AppError::Download(DownloadError::Timeout(
                        "yt-dlp command timed out getting formats list".to_string(),
                    )));
                    continue;
                }
            };

            if list_formats_output.status.success() {
                log::info!("✅ Formats list succeeded using [{}]", proxy_name);
                break 'proxy_loop String::from_utf8_lossy(&list_formats_output.stdout).to_string();
            }

            let stderr = String::from_utf8_lossy(&list_formats_output.stderr);
            let error_type = analyze_ytdlp_error(&stderr);

            log::error!(
                "❌ Formats list failed with [{}], error type: {:?}",
                proxy_name,
                error_type
            );
            log::error!("yt-dlp stderr: {}", stderr);

            // Check if proxy-related error that should trigger fallback
            let should_try_next = is_proxy_related_error(&stderr)
                || matches!(error_type, YtDlpErrorType::BotDetection | YtDlpErrorType::NetworkError);

            if should_try_next && attempt + 1 < total_proxies {
                log::warn!(
                    "🔄 Proxy-related error detected, will try next proxy (attempt {}/{})",
                    attempt + 2,
                    total_proxies
                );
                last_error = Some(AppError::Download(DownloadError::YtDlp(get_error_message(&error_type))));
                continue;
            }

            // Non-recoverable error or last proxy
            return Err(AppError::Download(DownloadError::YtDlp(get_error_message(&error_type))));
        }

        // All proxies failed
        log::error!("❌ All {} proxies failed for formats list", total_proxies);
        return Err(
            last_error.unwrap_or_else(|| AppError::Download(DownloadError::YtDlp("All proxies failed".to_string())))
        );
    };

    let output_line_count = formats_output.lines().count();
    log::debug!(
        "yt-dlp --list-formats output received ({} bytes, {} lines)",
        formats_output.len(),
        output_line_count
    );
    let mut formats: Vec<VideoFormatInfo> = Vec::new();

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::escape_markdown;

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

    // ==================== MAX_VIDEO_FORMAT_SIZE_BYTES constant tests ====================

    #[test]
    fn test_max_video_format_size() {
        assert_eq!(MAX_VIDEO_FORMAT_SIZE_BYTES, 2 * 1024 * 1024 * 1024); // 2GB
    }
}
