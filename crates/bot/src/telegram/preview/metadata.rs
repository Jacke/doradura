use crate::core::config;
use crate::core::error::AppError;
use crate::download::error::DownloadError;
use crate::download::metadata::{
    add_cookies_args_with_proxy, add_instagram_cookies_args_with_proxy, add_no_cookies_args, get_proxy_chain,
    is_proxy_related_error,
};
use crate::download::ytdlp_errors::{analyze_ytdlp_error, get_error_message, YtDlpErrorType};
use crate::storage::cache;
use crate::telegram::cache::PREVIEW_CACHE;
use crate::telegram::types::{PreviewMetadata, VideoFormatInfo};
use crate::timestamps::extract_all_timestamps;
use serde_json::Value;
use tokio::process::Command as TokioCommand;
use tokio::time::timeout;
use url::Url;

use super::formats::{extract_video_formats_from_json, get_video_formats_list};

/// Fetches metadata from the yt-dlp JSON response
///
/// Uses --dump-json to retrieve all metadata in a single call.
/// On proxy-related errors, automatically tries the next proxy in the chain.
pub(super) async fn get_metadata_from_json(url: &Url, ytdl_bin: &str) -> Result<Value, AppError> {
    let proxy_chain = get_proxy_chain();
    let total_proxies = proxy_chain.len();
    let mut last_error: Option<AppError> = None;

    for (attempt, proxy_option) in proxy_chain.into_iter().enumerate() {
        let proxy_name = proxy_option
            .as_ref()
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "Direct (no proxy)".to_string());

        log::info!(
            "📡 Preview metadata attempt {}/{} using [{}]",
            attempt + 1,
            total_proxies,
            proxy_name
        );

        let mut args: Vec<&str> = vec![
            "--dump-json",
            "--no-playlist",
            "--socket-timeout",
            "30",
            "--retries",
            "2",
            "--age-limit",
            "99",
            "--extractor-args",
            "youtube:player_client=android,web_music;formats=missing_pot",
            "--js-runtimes",
            "deno",
            "--impersonate",
            "Chrome-131:Android-14",
        ];

        // v5.0 FALLBACK CHAIN: First try WITHOUT cookies (new yt-dlp 2026+ mode)
        add_no_cookies_args(&mut args, proxy_option.as_ref());
        args.push(url.as_str());

        let command_str = format!("{} {}", ytdl_bin, args.join(" "));
        log::debug!("yt-dlp command for preview metadata (JSON): {}", command_str);

        let json_output = match timeout(
            config::download::ytdlp_timeout(),
            TokioCommand::new(ytdl_bin).args(&args).output(),
        )
        .await
        {
            Ok(Ok(output)) => output,
            Ok(Err(e)) => {
                log::warn!("🔄 Failed to execute yt-dlp with [{}]: {}", proxy_name, e);
                last_error = Some(AppError::Download(DownloadError::YtDlp(format!(
                    "Failed to get metadata: {}",
                    e
                ))));
                continue;
            }
            Err(_) => {
                log::warn!("🔄 yt-dlp command timed out with [{}], trying next proxy", proxy_name);
                last_error = Some(AppError::Download(DownloadError::Timeout(
                    "yt-dlp command timed out getting metadata".to_string(),
                )));
                continue;
            }
        };

        if json_output.status.success() {
            let json_str = String::from_utf8_lossy(&json_output.stdout);
            match serde_json::from_str(&json_str) {
                Ok(value) => {
                    log::info!("✅ Preview metadata succeeded using [{}]", proxy_name);
                    return Ok(value);
                }
                Err(e) => {
                    log::warn!("🔄 Failed to parse JSON with [{}]: {}", proxy_name, e);
                    last_error = Some(AppError::Download(DownloadError::YtDlp(format!(
                        "Failed to parse JSON metadata: {}",
                        e
                    ))));
                    continue;
                }
            }
        }

        let stderr = String::from_utf8_lossy(&json_output.stderr);
        let error_type = analyze_ytdlp_error(&stderr);

        // Log detailed error information
        log::error!(
            "❌ Preview metadata (no-cookies) failed with [{}], error type: {:?}",
            proxy_name,
            error_type
        );
        log::error!("yt-dlp stderr: {}", stderr);

        // v5.0 FALLBACK: If no-cookies failed with bot detection, try WITH cookies + PO token
        if matches!(
            error_type,
            YtDlpErrorType::InvalidCookies | YtDlpErrorType::BotDetection | YtDlpErrorType::NetworkError
        ) {
            let is_instagram = url.host_str().map(|h| h.contains("instagram.com")).unwrap_or(false);

            if is_instagram {
                log::warn!("🍪 No-cookies mode failed for Instagram, trying WITH IG cookies...");
            } else {
                log::warn!("🍪 No-cookies mode failed, trying WITH cookies + PO Token...");
            }

            let mut cookies_args: Vec<&str> = vec![
                "--dump-json",
                "--no-playlist",
                "--socket-timeout",
                "30",
                "--retries",
                "2",
                "--age-limit",
                "99",
            ];

            if is_instagram {
                // Instagram: use IG cookies, no YouTube extractor-args
                cookies_args.push("--js-runtimes");
                cookies_args.push("deno");
                add_instagram_cookies_args_with_proxy(&mut cookies_args, proxy_option.as_ref());
            } else {
                // YouTube/other: use YT cookies + PO Token
                cookies_args.push("--extractor-args");
                cookies_args.push("youtube:player_client=web,web_safari");
                cookies_args.push("--js-runtimes");
                cookies_args.push("deno");
                add_cookies_args_with_proxy(&mut cookies_args, proxy_option.as_ref());
            }

            cookies_args.push(url.as_str());

            log::info!("🔑 [WITH_COOKIES] Attempting preview metadata WITH cookies...");

            if let Ok(Ok(cookies_output)) = timeout(
                config::download::ytdlp_timeout(),
                TokioCommand::new(ytdl_bin).args(&cookies_args).output(),
            )
            .await
            {
                if cookies_output.status.success() {
                    let json_str = String::from_utf8_lossy(&cookies_output.stdout);
                    if let Ok(value) = serde_json::from_str(&json_str) {
                        log::info!("✅ [WITH_COOKIES] Preview metadata succeeded WITH cookies!");
                        return Ok(value);
                    }
                } else {
                    let cookies_stderr = String::from_utf8_lossy(&cookies_output.stderr);
                    log::warn!(
                        "❌ [WITH_COOKIES] Failed: {}",
                        &cookies_stderr[..std::cmp::min(200, cookies_stderr.len())]
                    );
                }
            }

            log::warn!("Both no-cookies and with-cookies modes failed for preview metadata");
        }

        // Check if proxy-related error that should trigger trying next proxy
        let should_try_next = is_proxy_related_error(&stderr)
            || matches!(
                error_type,
                YtDlpErrorType::BotDetection | YtDlpErrorType::InvalidCookies | YtDlpErrorType::NetworkError
            );

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

    log::error!("❌ All {} proxies failed for preview metadata", total_proxies);
    Err(last_error.unwrap_or_else(|| AppError::Download(DownloadError::YtDlp("All proxies failed".to_string()))))
}

/// Extracts a value from JSON by key
pub(super) fn get_json_value(json: &Value, key: &str) -> Option<String> {
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

/// Tries to get the file size for a specific video quality from JSON
pub(super) fn get_video_filesize_from_json(json: &Value, quality: &str) -> Option<u64> {
    let target_height = match quality {
        "1080p" => 1080,
        "720p" => 720,
        "480p" => 480,
        "360p" => 360,
        _ => return None,
    };

    // Try to fetch from the formats array
    json.get("formats").and_then(|v| v.as_array()).and_then(|formats| {
        formats
            .iter()
            .filter_map(|format| {
                // Look for a format with the desired resolution
                let height = format.get("height").and_then(|v| v.as_u64()).unwrap_or(0);

                if height == target_height as u64 {
                    // Try to get filesize or filesize_approx
                    format
                        .get("filesize")
                        .or_else(|| format.get("filesize_approx"))
                        .and_then(|v| v.as_u64())
                } else {
                    None
                }
            })
            .max() // Take the maximum size among all formats with the desired resolution
    })
}

/// Fetches extended metadata for the preview
///
/// Optimised version: uses --dump-json to retrieve all metadata in a single call
///
/// # Arguments
/// * `url` - Video/audio URL
/// * `format` - Download format ("mp3", "mp4", "srt", "txt")
/// * `video_quality` - Video quality (mp4 only, e.g. "1080p", "720p", "480p", "360p")
pub async fn get_preview_metadata(
    url: &Url,
    format: Option<&str>,
    video_quality: Option<&str>,
) -> Result<PreviewMetadata, AppError> {
    get_preview_metadata_inner(url, format, video_quality, false).await
}

/// Same as `get_preview_metadata` but skips the duration limit check when `has_time_range` is true,
/// because partial downloads can handle arbitrarily long videos.
pub async fn get_preview_metadata_with_time_range(
    url: &Url,
    format: Option<&str>,
    video_quality: Option<&str>,
) -> Result<PreviewMetadata, AppError> {
    get_preview_metadata_inner(url, format, video_quality, true).await
}

async fn get_preview_metadata_inner(
    url: &Url,
    format: Option<&str>,
    video_quality: Option<&str>,
    has_time_range: bool,
) -> Result<PreviewMetadata, AppError> {
    let ytdl_bin = &*config::YTDL_BIN;
    log::debug!("Getting preview metadata for URL: {}", url);

    // Instagram: use our GraphQL API directly instead of yt-dlp
    if crate::download::source::instagram::InstagramSource::extract_shortcode_public(url).is_some() {
        match get_instagram_preview_metadata(url).await {
            Ok(metadata) => return Ok(metadata),
            Err(e) => {
                log::warn!("Instagram GraphQL preview failed ({}), falling back to yt-dlp", e);
                // Fall through to yt-dlp flow below
            }
        }
    }

    // Check the preview cache
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

    // Check the cache for basic metadata (legacy cache, if needed)
    let (cached_title, cached_artist) = if let Some((title, artist)) = cache::get_cached_metadata(url).await {
        (Some(title), Some(artist))
    } else {
        (None, None)
    };

    // Fetch all metadata in a single JSON call (speed optimisation)
    let json_metadata = get_metadata_from_json(url, ytdl_bin).await?;

    // Extract title from JSON (use cache if available)
    let title = if let Some(cached) = cached_title {
        cached
    } else {
        get_json_value(&json_metadata, "title").ok_or_else(|| {
            AppError::Download(DownloadError::YtDlp(
                "Failed to get video title from metadata".to_string(),
            ))
        })?
    };

    if title.trim().is_empty() {
        log::warn!("yt-dlp returned empty title for URL: {}", url);
        return Err(AppError::Download(DownloadError::YtDlp(
            "Failed to get video title. Video might be unavailable or private.".to_string(),
        )));
    }

    // Extract artist from JSON (use cache if available, but ignore "NA")
    let mut artist = if let Some(cached) = cached_artist {
        // If the cache holds "NA" — ignore it and fetch fresh data
        if cached.trim() == "NA" || cached.trim().is_empty() {
            String::new() // Will fetch fresh data
        } else {
            cached
        }
    } else {
        String::new() // Will fetch fresh data
    };

    // If artist is empty — get it from JSON
    if artist.is_empty() {
        artist = get_json_value(&json_metadata, "artist").unwrap_or_default();
    }

    // If artist is still empty or "NA" — get uploader (channel) from JSON
    if artist.trim().is_empty() || artist.trim() == "NA" {
        log::debug!("Artist is empty or 'NA' in preview, trying to get channel/uploader");
        if let Some(uploader) = get_json_value(&json_metadata, "uploader") {
            artist = uploader;
            log::info!("Using uploader/channel as artist in preview: '{}'", artist);
        }
    }

    // Extract thumbnail URL from JSON
    // Try several possible fields for the thumbnail
    let thumbnail_url = get_json_value(&json_metadata, "thumbnail").or_else(|| {
        // If thumbnails is an array, take the best one (usually the last or the one with max width)
        json_metadata
            .get("thumbnails")
            .and_then(|v| v.as_array())
            .and_then(|arr| {
                // Find the thumbnail with the maximum width (best quality)
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

    // Extract duration from JSON
    let duration = get_json_value(&json_metadata, "duration")
        .and_then(|d| d.parse::<f64>().ok())
        .map(|d| d as u32);

    // Check video duration: maximum 4 hours (14400 seconds)
    // Skip this check when time_range is set — partial downloads handle long videos fine.
    if !has_time_range {
        if let Some(dur) = duration {
            const MAX_DURATION_SECONDS: u32 = 14400; // 4 hours
            if dur > MAX_DURATION_SECONDS {
                let hours = dur / 3600;
                let minutes = (dur % 3600) / 60;
                return Err(AppError::Download(DownloadError::Other(format!(
                    "Video is too long ({}h {}min). Maximum duration: 4 hours.",
                    hours, minutes
                ))));
            }
        }
    }

    // Fetch the list of available formats with sizes (if the source provides them).
    // Use --list-formats because JSON doesn't always contain exact sizes for every format.
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
            // Do not return an error — just log it and create a standard button
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

    // Instagram fallback: if yt-dlp got metadata but no video formats, and it's a reel/video,
    // add a synthetic "best" format so the UI shows MP4 button
    if video_formats.as_ref().is_none_or(|formats| formats.is_empty()) {
        if let Some(host) = url.host_str() {
            let host_lower = host.to_lowercase();
            if (host_lower == "instagram.com" || host_lower == "www.instagram.com") && url.path().contains("/reel") {
                log::info!("Instagram reel detected with no video formats, adding synthetic MP4 format");
                video_formats = Some(vec![VideoFormatInfo {
                    quality: "best".to_string(),
                    size_bytes: None,
                    resolution: None,
                }]);
            }
        }
    }

    // Fetch the approximate file size
    // For video: get the size for a specific quality via --list-formats (if needed)
    // For audio: use the filesize from JSON
    let mut filesize = if format == Some("mp4") {
        if let Some(quality) = video_quality {
            // For video with a specific quality, try to get it from the JSON formats array
            get_video_filesize_from_json(&json_metadata, quality)
        } else {
            // For video without a specific quality — use filesize from JSON
            get_json_value(&json_metadata, "filesize")
                .or_else(|| get_json_value(&json_metadata, "filesize_approx"))
                .and_then(|s| s.parse::<u64>().ok())
        }
    } else {
        // For audio use filesize from JSON
        get_json_value(&json_metadata, "filesize")
            .or_else(|| get_json_value(&json_metadata, "filesize_approx"))
            .and_then(|s| s.parse::<u64>().ok())
    };

    // If filesize was not obtained from JSON for video with a specific quality, use the size from video_formats
    if filesize.is_none() && format == Some("mp4") {
        if let Some(quality) = video_quality {
            filesize = video_formats
                .as_ref()
                .and_then(|formats| formats.iter().find(|f| f.quality == quality).and_then(|f| f.size_bytes));
        }
    }

    // Extract description from JSON
    let description = get_json_value(&json_metadata, "description").map(|desc| {
        // Truncate description length (safely, at character boundaries)
        const MAX_CHARS: usize = 200;
        let char_count = desc.chars().count();
        if char_count > MAX_CHARS {
            let truncated: String = desc.chars().take(MAX_CHARS).collect();
            format!("{}...", truncated)
        } else {
            desc
        }
    });

    // Extract timestamps from URL and metadata
    let timestamps = extract_all_timestamps(url, Some(&json_metadata));
    if !timestamps.is_empty() {
        log::debug!("Extracted {} timestamps for URL: {}", timestamps.len(), url);
    }

    let metadata = PreviewMetadata {
        title: title.clone(),
        artist: artist.clone(),
        thumbnail_url: thumbnail_url.clone(),
        duration,
        filesize,
        description,
        video_formats,
        timestamps,
        is_photo: false,
        carousel_count: 0,
    };

    // Cache the extended metadata only if the title is non-empty and not "Unknown Track"
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

        // Store in the new preview cache
        PREVIEW_CACHE.set(url.as_str().to_string(), metadata.clone()).await;
    } else {
        log::warn!("Not caching metadata with invalid title: '{}'", title);
    }

    Ok(metadata)
}

/// Build preview metadata for Instagram URLs using our GraphQL API (bypasses yt-dlp).
async fn get_instagram_preview_metadata(url: &Url) -> Result<PreviewMetadata, AppError> {
    log::info!("Using Instagram GraphQL for preview metadata: {}", url);
    let source = crate::download::source::instagram::InstagramSource::new();

    let info = source.get_media_preview(url).await.map_err(|e| {
        log::warn!("Instagram GraphQL preview failed: {}, falling back to yt-dlp", e);
        e
    })?;

    // For video content, provide a synthetic "best" format so the UI shows MP4 button
    let video_formats = if info.is_video {
        Some(vec![VideoFormatInfo {
            quality: "best".to_string(),
            size_bytes: None,
            resolution: None,
        }])
    } else {
        None
    };

    let metadata = PreviewMetadata {
        title: info.title,
        artist: info.artist,
        thumbnail_url: info.thumbnail_url,
        duration: info.duration_secs,
        filesize: None,
        description: None,
        video_formats,
        timestamps: Vec::new(),
        is_photo: !info.is_video,
        carousel_count: info.carousel_count,
    };

    // Cache it
    PREVIEW_CACHE.set(url.as_str().to_string(), metadata.clone()).await;

    Ok(metadata)
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
