use crate::core::config;
use crate::core::error::AppError;
use crate::download::error::DownloadError;
use crate::download::metadata::{add_cookies_args_with_proxy, get_proxy_chain, is_proxy_related_error};
use crate::download::ytdlp_errors::{YtDlpErrorType, analyze_ytdlp_error, get_error_message};
use crate::telegram::types::{AudioTrackInfo, VideoFormatInfo};
use serde_json::Value;
use std::collections::HashMap;
use tokio::process::Command as TokioCommand;
use tokio::time::timeout;
use url::Url;

/// Filter preview video formats so we only show what the Bot API can actually
/// send. Uses the same dynamic size ceiling as the send path
/// (`doracore::core::config::validation::max_video_size_bytes`), which returns
/// 5 GB on a local Bot API server and 50 MB on the standard `api.telegram.org`.
///
/// Previously this filter hardcoded a 2 GB cap, which was too tight for local
/// Bot API (max is 5 GB) and hid 720p/1080p from long videos — user report
/// 2026-04-20 on a 2h26m Noize MC concert showing only 480p max.
pub fn filter_video_formats_by_size(formats: &[VideoFormatInfo]) -> Vec<VideoFormatInfo> {
    let max_bytes = doracore::core::config::validation::max_video_size_bytes();
    formats
        .iter()
        .filter(|format| format.size_bytes.is_none_or(|size| size <= max_bytes))
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
        4320.. => Some("4320p"),
        2160..=4319 => Some("2160p"),
        1440..=2159 => Some("1440p"),
        1080..=1439 => Some("1080p"),
        720..=1079 => Some("720p"),
        480..=719 => Some("480p"),
        360..=479 => Some("360p"),
        240..=359 => Some("240p"),
        1..=239 => Some("144p"),
        0 => None,
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
    // Check from highest to lowest to avoid substring issues (e.g. "1440" before "144")
    if lowered.contains("4320") {
        Some("4320p")
    } else if lowered.contains("2160") {
        Some("2160p")
    } else if lowered.contains("1440") {
        Some("1440p")
    } else if lowered.contains("1080") {
        Some("1080p")
    } else if lowered.contains("720") {
        Some("720p")
    } else if lowered.contains("480") {
        Some("480p")
    } else if lowered.contains("360") {
        Some("360p")
    } else if lowered.contains("240") {
        Some("240p")
    } else if lowered.contains("144p") {
        // Use "144p" (not "144") to avoid matching "1440"
        Some("144p")
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
        if let Some(size) = size
            && best_audio_size.is_none_or(|current| size > current)
        {
            best_audio_size = Some(size);
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

        if (width.is_none() || height.is_none())
            && let Some(resolution) = resolution_field
            && let Some((parsed_width, parsed_height)) = parse_resolution_string(resolution)
        {
            width = width.or(Some(parsed_width));
            height = height.or(Some(parsed_height));
        }

        // Prefer format_note from yt-dlp (most accurate, e.g. "360p" for 640x352)
        // then fall back to dimensions, then resolution string.
        let mut quality = None;
        if let Some(note) = format.get("format_note").and_then(|v| v.as_str()) {
            quality = quality_from_note(note);
        }
        if quality.is_none() {
            quality = quality_from_dimensions(width, height);
        }
        if quality.is_none()
            && let Some(resolution) = resolution_field
            && let Some((parsed_width, parsed_height)) = parse_resolution_string(resolution)
        {
            quality = quality_from_dimensions(Some(parsed_width), Some(parsed_height));
        }

        let quality = match quality {
            Some(value) => value,
            None => continue,
        };

        let mut size_bytes = format
            .get("filesize")
            .or_else(|| format.get("filesize_approx"))
            .and_then(|v| v.as_u64());

        // Fallback: estimate from tbr (kbits/s) × duration when yt-dlp omits file size
        // for adaptive DASH streams (common for 720p+). Same formula as yt-dlp filesize_approx.
        if size_bytes.is_none()
            && let (Some(tbr), Some(dur)) = (
                format.get("tbr").and_then(|v| v.as_f64()),
                json.get("duration").and_then(|v| v.as_f64()),
            )
        {
            size_bytes = Some((tbr * 125.0 * dur) as u64); // tbr kbps × 1000/8 × secs
        }

        let acodec = format.get("acodec").and_then(|v| v.as_str()).unwrap_or("");
        if acodec == "none"
            && let (Some(size), Some(audio_size)) = (size_bytes, best_audio_size)
        {
            size_bytes = Some(size + audio_size);
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
    for quality in [
        "4320p", "2160p", "1440p", "1080p", "720p", "480p", "360p", "240p", "144p",
    ] {
        if let Some(info) = by_quality.remove(quality) {
            ordered.push(info);
        }
    }

    ordered
}

/// Extracts unique audio track languages from yt-dlp JSON metadata.
///
/// Iterates the `formats` array, collecting unique `language` values from
/// audio-only entries (`vcodec == "none"`). Returns empty vec if fewer than
/// 2 distinct languages (single track = no selection needed).
pub fn extract_audio_tracks_from_json(json: &Value) -> Vec<AudioTrackInfo> {
    let formats = match json.get("formats").and_then(|v| v.as_array()) {
        Some(formats) => formats,
        None => return Vec::new(),
    };

    let mut seen = std::collections::HashMap::<String, Option<String>>::new();

    for format in formats {
        let vcodec = format.get("vcodec").and_then(|v| v.as_str()).unwrap_or("");
        if vcodec != "none" {
            continue;
        }

        let language = match format.get("language").and_then(|v| v.as_str()) {
            Some(lang) if !lang.is_empty() && lang != "und" => lang.to_string(),
            _ => continue,
        };

        seen.entry(language).or_insert_with(|| {
            format
                .get("format_note")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        });
    }

    if seen.len() < 2 {
        return Vec::new();
    }

    let mut tracks: Vec<AudioTrackInfo> = seen
        .into_iter()
        .map(|(language, display_name)| AudioTrackInfo { language, display_name })
        .collect();
    tracks.sort_by(|a, b| a.language.cmp(&b.language));
    tracks
}

/// Fetches the list of available video formats with file sizes
///
/// Parses the output of yt-dlp --list-formats and extracts format information:
/// - 1080p, 720p, 480p, 360p
/// - File sizes
/// - Resolutions
///
/// On proxy-related errors, automatically tries the next proxy in the chain.
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
            add_cookies_args_with_proxy(&mut list_formats_args, proxy_option.as_ref(), None);

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

    // Universal format parser: detect quality from explicit labels (e.g. "240p") or resolution
    let mut by_quality: HashMap<String, (Option<u64>, Option<String>)> = HashMap::new();

    for line in formats_output.lines() {
        // Skip non-format lines, audio-only, and storyboard
        if line.contains("audio only") || line.contains("storyboard") || line.contains("images") {
            continue;
        }

        // Try to determine quality from the line
        let quality = match detect_quality_from_text_line(line) {
            Some(q) => q,
            None => continue,
        };

        // Extract resolution (WxH pattern)
        let resolution = extract_resolution_from_line(line);

        // Extract file size
        let size_bytes = extract_size_from_line(line);

        let entry = by_quality.entry(quality.to_string()).or_insert((None, None));
        // Keep the maximum size (best bitrate format)
        if let Some(new_size) = size_bytes
            && entry.0.is_none_or(|current| new_size > current)
        {
            entry.0 = Some(new_size);
        }
        if entry.1.is_none() {
            entry.1 = resolution;
        }
    }

    // Convert to VideoFormatInfo, ordered by quality
    let quality_order = [
        "4320p", "2160p", "1440p", "1080p", "720p", "480p", "360p", "240p", "144p",
    ];
    for quality in quality_order {
        if let Some((size_bytes, resolution)) = by_quality.remove(quality) {
            formats.push(VideoFormatInfo {
                quality: quality.to_string(),
                size_bytes,
                resolution,
            });
        }
    }

    // Find the best audio format size to add to video-only format sizes
    let mut best_audio_size: Option<u64> = None;
    for line in formats_output.lines() {
        if line.contains("audio only") {
            // Look for m4a or webm audio with the highest bitrate
            if (line.contains("m4a") || line.contains("webm"))
                && let Some(mib_pos) = line.find("MiB")
            {
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
                    if let Ok(size_mb) = size_str.trim().parse::<f64>()
                        && size_mb > 0.0
                        && size_mb < 1000.0
                    {
                        let size_bytes = (size_mb * 1024.0 * 1024.0) as u64;
                        if best_audio_size.is_none_or(|current| size_bytes > current) {
                            best_audio_size = Some(size_bytes);
                        }
                    }
                }
            }
        }
    }

    // Add the audio size to each video format's size
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

    // Sort formats by quality (best to worst)
    formats.sort_by(|a, b| {
        let order = |q: &str| match q {
            "4320p" => 9,
            "2160p" => 8,
            "1440p" => 7,
            "1080p" => 6,
            "720p" => 5,
            "480p" => 4,
            "360p" => 3,
            "240p" => 2,
            "144p" => 1,
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

/// Detect quality label from a --list-formats text line.
/// First checks for explicit quality labels (e.g. "240p"), then falls back to resolution parsing.
fn detect_quality_from_text_line(line: &str) -> Option<&'static str> {
    // First: look for explicit quality labels like "240p", "1080p" in the line
    // Check from highest to lowest to avoid substring issues (4320 before 432, 1440 before 144)
    for (label, quality) in [
        ("4320p", "4320p"),
        ("2160p", "2160p"),
        ("1440p", "1440p"),
        ("1080p", "1080p"),
        ("720p", "720p"),
        ("480p", "480p"),
        ("360p", "360p"),
        ("240p", "240p"),
        ("144p", "144p"),
    ] {
        if line.contains(label) {
            return Some(quality);
        }
    }

    // Fallback: parse WxH resolution and classify by short side
    if let Some((w, h)) = extract_dimensions_from_line(line) {
        let short_side = w.min(h);
        return quality_from_short_side(short_side);
    }

    None
}

/// Extract WxH dimensions from a text line (e.g. "1920x1080" or "202x360")
fn extract_dimensions_from_line(line: &str) -> Option<(u64, u64)> {
    let bytes = line.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    while i < len {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < len && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if i < len && bytes[i] == b'x' {
                let width_str = &line[start..i];
                i += 1; // skip 'x'
                let h_start = i;
                while i < len && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                if i > h_start {
                    let height_str = &line[h_start..i];
                    if let (Ok(w), Ok(h)) = (width_str.parse::<u64>(), height_str.parse::<u64>())
                        && w > 0
                        && h > 0
                        && w <= 10000
                        && h <= 10000
                    {
                        return Some((w, h));
                    }
                }
            }
        }
        i += 1;
    }
    None
}

/// Extract resolution string from a text line (returns the "WxH" portion)
fn extract_resolution_from_line(line: &str) -> Option<String> {
    extract_dimensions_from_line(line).map(|(w, h)| format!("{}x{}", w, h))
}

/// Extract file size in bytes from a text line (supports KiB, MiB, GiB)
fn extract_size_from_line(line: &str) -> Option<u64> {
    for (suffix, multiplier) in [
        ("GiB", 1024.0 * 1024.0 * 1024.0),
        ("MiB", 1024.0 * 1024.0),
        ("KiB", 1024.0),
    ] {
        if let Some(pos) = line.find(suffix) {
            let before = &line[..pos];
            let num_str: String = before
                .chars()
                .rev()
                .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '\u{2248}' || *c == '~')
                .collect::<String>()
                .chars()
                .rev()
                .filter(|c| c.is_ascii_digit() || *c == '.')
                .collect();
            if let Ok(size) = num_str.parse::<f64>()
                && size > 0.0
                && size < 100_000.0
            {
                return Some((size * multiplier) as u64);
            }
        }
    }
    None
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
        assert_eq!(quality_from_short_side(4320), Some("4320p"));
        assert_eq!(quality_from_short_side(2160), Some("2160p"));
        assert_eq!(quality_from_short_side(1440), Some("1440p"));
        assert_eq!(quality_from_short_side(1080), Some("1080p"));
        assert_eq!(quality_from_short_side(720), Some("720p"));
        assert_eq!(quality_from_short_side(480), Some("480p"));
        assert_eq!(quality_from_short_side(360), Some("360p"));
        assert_eq!(quality_from_short_side(240), Some("240p"));
        assert_eq!(quality_from_short_side(144), Some("144p"));
    }

    #[test]
    fn test_quality_from_short_side_ranges() {
        // Non-standard resolutions should map to the appropriate bucket
        assert_eq!(quality_from_short_side(202), Some("144p")); // 202x360 video, short_side=202
        assert_eq!(quality_from_short_side(100), Some("144p"));
        assert_eq!(quality_from_short_side(300), Some("240p"));
        assert_eq!(quality_from_short_side(500), Some("480p"));
        assert_eq!(quality_from_short_side(600), Some("480p"));
        assert_eq!(quality_from_short_side(900), Some("720p"));
        assert_eq!(quality_from_short_side(1200), Some("1080p"));
        assert_eq!(quality_from_short_side(1800), Some("1440p"));
        assert_eq!(quality_from_short_side(3000), Some("2160p"));
        assert_eq!(quality_from_short_side(5000), Some("4320p"));
    }

    #[test]
    fn test_quality_from_short_side_zero() {
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
    fn test_quality_from_note_extended() {
        assert_eq!(quality_from_note("4320p"), Some("4320p"));
        assert_eq!(quality_from_note("2160p"), Some("2160p"));
        assert_eq!(quality_from_note("1440p"), Some("1440p"));
        assert_eq!(quality_from_note("240p"), Some("240p"));
        assert_eq!(quality_from_note("144p"), Some("144p"));
    }

    #[test]
    fn test_quality_from_note_no_match() {
        assert_eq!(quality_from_note(""), None);
        assert_eq!(quality_from_note("audio only"), None);
        assert_eq!(quality_from_note("unknown"), None);
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
        // Sizes chosen to fit under BOTH the standard 50 MB and the local 5 GB
        // caps — the test must pass regardless of how BOT_API_URL is (or isn't)
        // set in the test environment.
        let formats = vec![
            VideoFormatInfo {
                quality: "1080p".to_string(),
                size_bytes: Some(30 * 1024 * 1024), // 30 MB
                resolution: Some("1920x1080".to_string()),
            },
            VideoFormatInfo {
                quality: "720p".to_string(),
                size_bytes: Some(20 * 1024 * 1024), // 20 MB
                resolution: Some("1280x720".to_string()),
            },
        ];
        let filtered = filter_video_formats_by_size(&formats);
        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn test_filter_video_formats_by_size_filters_large() {
        // 6 GB exceeds the 5 GB local-Bot-API ceiling and the 50 MB standard
        // ceiling, so this format is dropped under both configurations.
        let formats = vec![
            VideoFormatInfo {
                quality: "1080p".to_string(),
                size_bytes: Some(6 * 1024 * 1024 * 1024), // 6 GB - exceeds 5 GB local cap
                resolution: Some("1920x1080".to_string()),
            },
            VideoFormatInfo {
                quality: "720p".to_string(),
                size_bytes: Some(30 * 1024 * 1024), // 30 MB - fits both local and standard caps
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

    // ==================== detect_quality_from_text_line tests ====================

    #[test]
    fn test_detect_quality_from_text_line_explicit_label() {
        let line =
            "134 mp4   202x360     25    |  5.36MiB 200k https | avc1.4d400d 200k video only          240p, mp4_dash";
        assert_eq!(detect_quality_from_text_line(line), Some("240p"));
    }

    #[test]
    fn test_detect_quality_from_text_line_1080p() {
        let line =
            "137 mp4   1920x1080   30    | 100.00MiB 5000k https | avc1.640028 5000k video only       1080p, mp4_dash";
        assert_eq!(detect_quality_from_text_line(line), Some("1080p"));
    }

    #[test]
    fn test_detect_quality_from_text_line_no_label_uses_resolution() {
        // Line with resolution but no explicit quality label
        let line = "999 mp4   1920x1080   30    | 100.00MiB 5000k https | avc1.640028";
        assert_eq!(detect_quality_from_text_line(line), Some("1080p"));
    }

    #[test]
    fn test_detect_quality_from_text_line_audio_only_skipped_by_caller() {
        // This function itself does not skip audio only; the caller does
        let line = "140 m4a   audio only        |  3.50MiB 128k https | mp4a.40.2 128k audio only";
        // It would match dimensions if any, or return None
        assert_eq!(detect_quality_from_text_line(line), None);
    }

    // ==================== extract_dimensions_from_line tests ====================

    #[test]
    fn test_extract_dimensions_standard() {
        assert_eq!(extract_dimensions_from_line("1920x1080"), Some((1920, 1080)));
        assert_eq!(extract_dimensions_from_line("202x360"), Some((202, 360)));
        assert_eq!(extract_dimensions_from_line("256x144"), Some((256, 144)));
    }

    #[test]
    fn test_extract_dimensions_in_context() {
        let line = "134 mp4   202x360     25    |  5.36MiB";
        assert_eq!(extract_dimensions_from_line(line), Some((202, 360)));
    }

    #[test]
    fn test_extract_dimensions_none() {
        assert_eq!(extract_dimensions_from_line("audio only"), None);
        assert_eq!(extract_dimensions_from_line("no dimensions"), None);
    }

    // ==================== extract_size_from_line tests ====================

    #[test]
    fn test_extract_size_mib() {
        let line = "134 mp4   202x360     25    |  5.36MiB 200k";
        let size = extract_size_from_line(line).unwrap();
        assert!((size as f64 - 5.36 * 1024.0 * 1024.0).abs() < 1024.0);
    }

    #[test]
    fn test_extract_size_gib() {
        let line = "137 mp4   1920x1080   30    |  1.50GiB 5000k";
        let size = extract_size_from_line(line).unwrap();
        assert!((size as f64 - 1.5 * 1024.0 * 1024.0 * 1024.0).abs() < 1024.0 * 1024.0);
    }

    #[test]
    fn test_extract_size_kib() {
        let line = "999 mp4   256x144     15    | 500.00KiB 50k";
        let size = extract_size_from_line(line).unwrap();
        assert!((size as f64 - 500.0 * 1024.0).abs() < 1024.0);
    }

    #[test]
    fn test_extract_size_none() {
        assert_eq!(extract_size_from_line("no size info here"), None);
    }

    // ==================== 202x360 end-to-end test ====================

    #[test]
    fn test_202x360_video_detected_as_240p() {
        // Simulate yt-dlp --list-formats output for a video with 202x360 resolution
        let output = r#"[info] Available formats for SxnSmmhRXoI:
ID  EXT   RESOLUTION FPS CH │    FILESIZE   TBR PROTO │ VCODEC          VBR ACODEC      ABR ASR MORE INFO
──────────────────────────────────────────────────────────────────────────────────────────────────────────
sb0 mhtml 48x48          │                    mhtml │ images                                  storyboard
233 mp4   audio only        │                    m3u8  │ audio only          mp4a.40.5    31k 22k ultralow, m3u8_native, en
599 webm  audio only        │                    m3u8  │ audio only          opus          31k 48k ultralow, m3u8_native, en
139 m4a   audio only      2 │    1.28MiB   48k https │ audio only          mp4a.40.5    48k 22k low, m4a_dash
134 mp4   202x360     25    │    5.36MiB  200k https │ avc1.4d400d   200k video only          240p, mp4_dash
18  mp4   202x360     25  2 │ ≈  8.77MiB  326k https │ avc1.42001E         mp4a.40.2       44k [ru] 240p
"#;

        // Parse using the text parser logic
        let mut by_quality: HashMap<String, (Option<u64>, Option<String>)> = HashMap::new();
        for line in output.lines() {
            if line.contains("audio only") || line.contains("storyboard") || line.contains("images") {
                continue;
            }
            let quality = match detect_quality_from_text_line(line) {
                Some(q) => q,
                None => continue,
            };
            let resolution = extract_resolution_from_line(line);
            let size_bytes = extract_size_from_line(line);
            let entry = by_quality.entry(quality.to_string()).or_insert((None, None));
            if let Some(new_size) = size_bytes
                && entry.0.is_none_or(|current| new_size > current)
            {
                entry.0 = Some(new_size);
            }
            if entry.1.is_none() {
                entry.1 = resolution;
            }
        }

        assert!(
            by_quality.contains_key("240p"),
            "Should detect 240p quality from 202x360 video"
        );
        let (size, resolution) = by_quality.get("240p").unwrap();
        assert!(size.is_some(), "Should extract file size for 240p");
        assert_eq!(resolution.as_deref(), Some("202x360"));
    }
}
