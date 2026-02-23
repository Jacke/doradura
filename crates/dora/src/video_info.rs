//! Video metadata fetching from yt-dlp and thumbnail processing.

use std::collections::BTreeSet;

use serde_json::Value;

// ── Thumbnail constants ───────────────────────────────────────────────────────

/// Width of the thumbnail area in characters (each character = one `▀` = 1px wide).
pub const THUMB_W: usize = 64;
/// Height in half-block rows (each row = 2 pixel rows, rendered as `▀`).
pub const THUMB_H: usize = 18;

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
}

/// Pre-rendered thumbnail as half-block `▀` pixel pairs.
/// Each entry is (top_pixel_rgb, bottom_pixel_rgb).
/// Width = THUMB_W, height = THUMB_H rows.
#[derive(Debug, Clone)]
pub struct ThumbnailArt {
    pub rows: Vec<Vec<([u8; 3], [u8; 3])>>,
}

/// Result type sent from the background preview task.
pub type PreviewResult = Result<(VideoInfo, Option<ThumbnailArt>), String>;

// ── Fetchers ──────────────────────────────────────────────────────────────────

/// Fetch full video metadata by running `yt-dlp -J`.
/// Returns an error string if yt-dlp fails or JSON is unparseable.
pub async fn fetch_video_info(url: &str, ytdlp_bin: &str) -> Result<VideoInfo, String> {
    log::info!("fetch_video_info: {} (bin={})", url, ytdlp_bin);
    let output = tokio::process::Command::new(ytdlp_bin)
        .args(["-J", "--no-playlist", url])
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
    })
}

fn process_thumbnail(bytes: Vec<u8>) -> Option<ThumbnailArt> {
    let img = image::load_from_memory(&bytes).ok()?;

    // Resize to THUMB_W × (THUMB_H * 2) — half-block uses 2 pixel rows per char row
    let img = img.resize_exact(
        THUMB_W as u32,
        (THUMB_H * 2) as u32,
        image::imageops::FilterType::Lanczos3,
    );
    let rgb = img.to_rgb8();

    let rows: Vec<Vec<([u8; 3], [u8; 3])>> = (0..THUMB_H)
        .map(|row| {
            let y_top = (row * 2) as u32;
            let y_bot = y_top + 1;
            (0..THUMB_W as u32)
                .map(|x| {
                    let t = rgb.get_pixel(x, y_top);
                    let b = rgb.get_pixel(x, y_bot);
                    ([t[0], t[1], t[2]], [b[0], b[1], b[2]])
                })
                .collect()
        })
        .collect();

    Some(ThumbnailArt { rows })
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
