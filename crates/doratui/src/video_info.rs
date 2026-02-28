//! Video metadata fetching from yt-dlp and thumbnail processing.

use std::collections::BTreeSet;

use serde_json::Value;

/// Top 10 subtitle languages by Telegram user base popularity.
const PRIORITY_LANGS: &[&str] = &["ru", "en", "uk", "es", "pt", "ar", "fa", "fr", "de", "hi"];

/// Maximum number of subtitle languages shown in the selector.
const MAX_SUBTITLE_LANGS: usize = 10;

// ── Thumbnail constants ───────────────────────────────────────────────────────

/// Width of the thumbnail area in characters.
pub const THUMB_W: usize = 80;
/// Height in half-block rows (each row = 2 pixel rows).
pub const THUMB_H: usize = 22;

// ── Data types ────────────────────────────────────────────────────────────────

/// Parsed video metadata from `yt-dlp -J`.
#[derive(Debug, Clone)]
pub struct VideoInfo {
    pub title: String,
    pub uploader: String,
    /// Duration in seconds.
    pub duration_secs: u64,
    pub view_count: Option<u64>,
    pub like_count: Option<u64>,
    /// Approximate file size in bytes (best estimate from yt-dlp).
    pub filesize_approx: Option<u64>,
    /// URL of the video thumbnail image.
    pub thumbnail_url: Option<String>,
    /// Available video heights sorted descending (e.g. [2160, 1080, 720, 480, 360]).
    /// Includes only streams with a real video codec.
    pub available_heights: Vec<u32>,
    /// URL of the uploader's channel page (if provided by yt-dlp).
    pub channel_url: Option<String>,
    /// Available subtitle language codes (manual + auto, sorted with common langs first).
    pub subtitle_langs: Vec<String>,
    /// Set of language codes that have manual (human-written) subtitles.
    /// Languages NOT in this set are auto-generated only.
    pub manual_sub_langs: std::collections::HashSet<String>,
}

/// Pre-rendered thumbnail as half-block `▀` pixel pairs.
/// Each entry is (top_pixel_rgb, bottom_pixel_rgb).
/// Width = THUMB_W, height = THUMB_H rows.
#[derive(Debug, Clone)]
pub struct ThumbnailArt {
    pub rows: Vec<Vec<([u8; 3], [u8; 3])>>,
    /// Actual width in characters.
    pub width: u16,
    /// Actual height in rows.
    pub height: u16,
    /// Original image bytes for high-quality TUI image protocols (Kitty/Sixel).
    pub raw_bytes: Vec<u8>,
}

/// Result type sent from the background preview task.
pub type PreviewResult = Result<(VideoInfo, Option<ThumbnailArt>), String>;

// ── Fetchers ──────────────────────────────────────────────────────────────────

/// Fetch full video metadata by running `yt-dlp -J`.
/// Returns an error string if yt-dlp fails or JSON is unparseable.
pub async fn fetch_video_info(url: &str, ytdlp_bin: &str, cookies_file: Option<String>) -> Result<VideoInfo, String> {
    log::info!("fetch_video_info: {} (bin={})", url, ytdlp_bin);
    let mut args = vec!["-J".to_string(), "--no-playlist".to_string()];

    if let Some(cf) = cookies_file {
        args.push("--cookies".to_string());
        args.push(cf);
    }

    // Modern yt-dlp args for YouTube age-restriction / bot detection bypass
    args.push("--extractor-args".to_string());
    args.push("youtube:player_client=android_vr,web_safari;formats=missing_pot".to_string());
    args.push("--js-runtimes".to_string());
    args.push("deno".to_string());
    args.push("--no-check-certificate".to_string());

    args.push(url.to_string());

    let output = tokio::process::Command::new(ytdlp_bin)
        .args(&args)
        .output()
        .await
        .map_err(|e| format!("Cannot run yt-dlp: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        log::debug!("fetch_video_info stderr:\n{}", stderr);
        let last = stderr
            .lines()
            .rfind(|l| !l.trim().is_empty() && !l.starts_with("WARNING"))
            .unwrap_or("yt-dlp failed");
        let err = last.trim_start_matches("ERROR: ").trim().to_string();
        log::warn!("fetch_video_info failed: {}", err);
        return Err(err);
    }

    let json: Value = serde_json::from_slice(&output.stdout).map_err(|e| format!("JSON parse error: {e}"))?;

    parse_video_info(&json).map_err(|e| e.to_string())
}

/// Download a thumbnail image and convert it to `THUMB_W × THUMB_H` half-block art.
/// Returns `None` silently on any failure (optional feature).
pub async fn fetch_thumbnail_art(url: &str) -> Option<ThumbnailArt> {
    let bytes = reqwest::get(url).await.ok()?.bytes().await.ok()?;
    let bytes = bytes.to_vec();

    // Image decode + resize is CPU-heavy → offload to blocking thread pool
    tokio::task::spawn_blocking(move || process_thumbnail(bytes))
        .await
        .ok()
        .flatten()
}

// ── Internal helpers ──────────────────────────────────────────────────────────

fn parse_video_info(json: &Value) -> anyhow::Result<VideoInfo> {
    let title = json["title"].as_str().unwrap_or("Unknown Title").to_string();

    let uploader = ["uploader", "channel", "creator", "artist"]
        .iter()
        .find_map(|&k| json[k].as_str())
        .unwrap_or("Unknown")
        .to_string();

    let duration_secs = json["duration"].as_f64().unwrap_or(0.0) as u64;
    let view_count = json["view_count"].as_u64();
    let like_count = json["like_count"].as_u64();
    let filesize_approx = json["filesize_approx"].as_u64().or_else(|| json["filesize"].as_u64());
    let thumbnail_url = json["thumbnail"].as_str().map(str::to_string);

    // Collect unique video heights from formats (skip audio-only streams)
    let mut heights: BTreeSet<u32> = BTreeSet::new();
    if let Some(formats) = json["formats"].as_array() {
        for fmt in formats {
            let h = fmt["height"].as_u64().unwrap_or(0) as u32;
            let vcodec = fmt["vcodec"].as_str().unwrap_or("none");
            if h >= 240 && vcodec != "none" {
                heights.insert(h);
            }
        }
    }

    // Descending order; fall back to common presets if yt-dlp gave us nothing
    let available_heights: Vec<u32> = if heights.is_empty() {
        vec![1080, 720, 480, 360]
    } else {
        heights.into_iter().rev().collect()
    };

    let channel_url = json["uploader_url"]
        .as_str()
        .or_else(|| json["channel_url"].as_str())
        .map(str::to_string);

    // Collect manual subtitle languages
    let mut manual_sub_langs = std::collections::HashSet::new();
    if let Some(obj) = json["subtitles"].as_object() {
        for lang in obj.keys() {
            if lang != "live_chat" {
                manual_sub_langs.insert(lang.clone());
            }
        }
    }

    // Collect all subtitle languages (manual + auto-generated), deduped
    let mut sub_langs: BTreeSet<String> = BTreeSet::new();
    sub_langs.extend(manual_sub_langs.iter().cloned());
    if let Some(obj) = json["automatic_captions"].as_object() {
        for lang in obj.keys() {
            if lang != "live_chat" {
                sub_langs.insert(lang.clone());
            }
        }
    }

    // Priority sort: common languages first, then alphabetical; cap at 10
    let mut subtitle_langs: Vec<String> = Vec::new();
    for p in PRIORITY_LANGS {
        if sub_langs.remove(*p) {
            subtitle_langs.push(p.to_string());
            if subtitle_langs.len() >= MAX_SUBTITLE_LANGS {
                break;
            }
        }
    }
    for lang in sub_langs {
        if subtitle_langs.len() >= MAX_SUBTITLE_LANGS {
            break;
        }
        subtitle_langs.push(lang);
    }

    Ok(VideoInfo {
        title,
        uploader,
        duration_secs,
        view_count,
        like_count,
        filesize_approx,
        thumbnail_url,
        available_heights,
        channel_url,
        subtitle_langs,
        manual_sub_langs,
    })
}

fn process_thumbnail(bytes: Vec<u8>) -> Option<ThumbnailArt> {
    let raw_bytes = bytes.clone();
    let img = image::load_from_memory(&bytes).ok()?;

    // Resize to fit within THUMB_W × (THUMB_H * 2) while preserving aspect ratio.
    let img = img.thumbnail(THUMB_W as u32, (THUMB_H * 2) as u32);
    let rgb = img.to_rgb8();
    let (w, h) = rgb.dimensions();
    let char_w = w as u16;
    let char_h = (h / 2) as u16;

    let rows: Vec<Vec<([u8; 3], [u8; 3])>> = (0..char_h)
        .map(|row| {
            let y_top = (row * 2) as u32;
            let y_bot = y_top + 1;
            (0..w)
                .map(|x| {
                    let t = rgb.get_pixel(x, y_top);
                    let b = if y_bot < h {
                        rgb.get_pixel(x, y_bot)
                    } else {
                        &image::Rgb([0, 0, 0])
                    };
                    ([t[0], t[1], t[2]], [b[0], b[1], b[2]])
                })
                .collect()
        })
        .collect();

    Some(ThumbnailArt {
        rows,
        width: char_w,
        height: char_h,
        raw_bytes,
    })
}

// ── Quality helpers ───────────────────────────────────────────────────────────

/// Returns the quality options as (display_label, height_or_none).
/// `None` height = "best" (no height filter).
#[allow(dead_code)]
pub fn quality_list_heights(info: &VideoInfo) -> Vec<Option<u32>> {
    let mut list: Vec<Option<u32>> = info.available_heights.iter().map(|&h| Some(h)).collect();
    list.push(None); // "best"
    list
}

// ── Formatting helpers (used by preview renderer) ─────────────────────────────

pub fn fmt_duration(secs: u64) -> String {
    if secs == 0 {
        return "–".to_string();
    }
    if secs >= 3600 {
        format!("{}:{:02}:{:02}", secs / 3600, (secs % 3600) / 60, secs % 60)
    } else {
        format!("{}:{:02}", secs / 60, secs % 60)
    }
}

pub fn fmt_count(n: u64) -> String {
    if n >= 1_000_000_000 {
        format!("{:.1}B", n as f64 / 1e9)
    } else if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1e6)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1e3)
    } else {
        n.to_string()
    }
}

pub fn fmt_size(bytes: u64) -> String {
    if bytes >= 1_073_741_824 {
        format!("{:.1} GB", bytes as f64 / 1_073_741_824.0)
    } else if bytes >= 1_048_576 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1_024 {
        format!("{:.1} KB", bytes as f64 / 1_024.0)
    } else {
        format!("{} B", bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_json(subtitles: serde_json::Value, auto_captions: serde_json::Value) -> serde_json::Value {
        json!({
            "title": "Test Video",
            "uploader": "Test Channel",
            "duration": 120.0,
            "view_count": 1000,
            "like_count": 50,
            "formats": [
                {"height": 720, "vcodec": "avc1", "acodec": "mp4a"},
                {"height": 1080, "vcodec": "avc1", "acodec": "mp4a"},
                {"height": 480, "vcodec": "avc1", "acodec": "mp4a"},
            ],
            "subtitles": subtitles,
            "automatic_captions": auto_captions,
        })
    }

    #[test]
    fn parse_manual_subtitles() {
        let json = make_json(json!({"en": [{"ext": "srt"}], "fr": [{"ext": "vtt"}]}), json!({}));
        let info = parse_video_info(&json).unwrap();
        assert!(info.subtitle_langs.contains(&"en".to_string()));
        assert!(info.subtitle_langs.contains(&"fr".to_string()));
    }

    #[test]
    fn parse_auto_generated_subtitles() {
        let json = make_json(
            json!({}),
            json!({"en": [{"ext": "srv3"}], "ru": [{"ext": "srv3"}], "de": [{"ext": "json3"}]}),
        );
        let info = parse_video_info(&json).unwrap();
        assert!(info.subtitle_langs.contains(&"en".to_string()));
        assert!(info.subtitle_langs.contains(&"ru".to_string()));
        assert!(info.subtitle_langs.contains(&"de".to_string()));
    }

    #[test]
    fn parse_mixed_manual_and_auto_subtitles() {
        let json = make_json(
            json!({"en": [{"ext": "srt"}]}),
            json!({"en": [{"ext": "srv3"}], "ja": [{"ext": "srv3"}]}),
        );
        let info = parse_video_info(&json).unwrap();
        // "en" should appear only once (deduped via BTreeSet)
        let en_count = info.subtitle_langs.iter().filter(|l| *l == "en").count();
        assert_eq!(en_count, 1);
        assert!(info.subtitle_langs.contains(&"ja".to_string()));
    }

    #[test]
    fn live_chat_excluded() {
        let json = make_json(
            json!({"en": [{"ext": "srt"}], "live_chat": [{"ext": "json"}]}),
            json!({}),
        );
        let info = parse_video_info(&json).unwrap();
        assert!(!info.subtitle_langs.contains(&"live_chat".to_string()));
        assert!(info.subtitle_langs.contains(&"en".to_string()));
    }

    #[test]
    fn priority_languages_sorted_first() {
        let json = make_json(json!({"zz": [], "aa": [], "en": [], "fr": [], "ru": []}), json!({}));
        let info = parse_video_info(&json).unwrap();
        // Priority order: ru, en, fr come before non-priority aa, zz
        let ru_pos = info.subtitle_langs.iter().position(|l| l == "ru").unwrap();
        let en_pos = info.subtitle_langs.iter().position(|l| l == "en").unwrap();
        let fr_pos = info.subtitle_langs.iter().position(|l| l == "fr").unwrap();
        let aa_pos = info.subtitle_langs.iter().position(|l| l == "aa").unwrap();
        assert!(ru_pos < en_pos, "ru should be before en (first priority)");
        assert!(en_pos < fr_pos, "en should be before fr");
        assert!(fr_pos < aa_pos, "fr should be before aa");
    }

    #[test]
    fn capped_at_10_languages() {
        // 15 languages — should be truncated to 10
        let json = make_json(
            json!({
                "en": [], "ru": [], "uk": [], "es": [], "pt": [],
                "ar": [], "fa": [], "fr": [], "de": [], "hi": [],
                "ja": [], "ko": [], "it": [], "nl": [], "sv": []
            }),
            json!({}),
        );
        let info = parse_video_info(&json).unwrap();
        assert_eq!(info.subtitle_langs.len(), 10);
        // All 10 priority langs should be present
        assert!(info.subtitle_langs.contains(&"ru".to_string()));
        assert!(info.subtitle_langs.contains(&"en".to_string()));
        assert!(info.subtitle_langs.contains(&"hi".to_string()));
        // Non-priority langs (ja, ko, etc.) should be cut off
        assert!(!info.subtitle_langs.contains(&"ja".to_string()));
    }

    #[test]
    fn manual_vs_auto_distinction() {
        let json = make_json(
            json!({"en": [{"ext": "srt"}]}),                           // manual
            json!({"en": [{"ext": "srv3"}], "ru": [{"ext": "srv3"}]}), // auto
        );
        let info = parse_video_info(&json).unwrap();
        // "en" has manual subs, "ru" is auto-only
        assert!(info.manual_sub_langs.contains("en"));
        assert!(!info.manual_sub_langs.contains("ru"));
    }

    #[test]
    fn no_subtitles_returns_empty_vec() {
        let json = make_json(json!({}), json!({}));
        let info = parse_video_info(&json).unwrap();
        assert!(info.subtitle_langs.is_empty());
        assert!(info.manual_sub_langs.is_empty());
    }

    #[test]
    fn no_subtitle_keys_in_json() {
        let json = json!({
            "title": "No Subs Video",
            "duration": 60.0,
            "formats": [],
        });
        let info = parse_video_info(&json).unwrap();
        assert!(info.subtitle_langs.is_empty());
    }

    #[test]
    fn heights_sorted_descending() {
        let json = make_json(json!({}), json!({}));
        let info = parse_video_info(&json).unwrap();
        assert_eq!(info.available_heights, vec![1080, 720, 480]);
    }
}
