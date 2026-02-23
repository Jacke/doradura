//! Playlist extraction for batch downloads.
//!
//! Extracts individual video URLs from playlists for processing.

use crate::core::config;
use crate::core::process::run_with_timeout;
use crate::download::metadata::add_cookies_args;
use serde::Deserialize;
use std::process::Stdio;
use tokio::process::Command;
use url::Url;

/// Maximum number of videos to extract from a playlist
const MAX_PLAYLIST_ITEMS: usize = 50;

/// Playlist metadata
#[derive(Debug, Clone)]
pub struct PlaylistInfo {
    /// Playlist title
    pub title: String,
    /// Playlist uploader/channel
    pub uploader: Option<String>,
    /// Total number of entries in the playlist
    pub entry_count: usize,
    /// Extracted video entries (limited by MAX_PLAYLIST_ITEMS)
    pub entries: Vec<PlaylistEntry>,
    /// Whether the playlist was truncated
    pub truncated: bool,
}

/// Single entry in a playlist
#[derive(Debug, Clone)]
pub struct PlaylistEntry {
    /// Video URL
    pub url: String,
    /// Video title
    pub title: String,
    /// Video duration in seconds
    pub duration: Option<u64>,
    /// Video position in playlist (1-indexed)
    pub position: usize,
}

/// JSON structure from yt-dlp --flat-playlist
#[derive(Debug, Deserialize)]
struct YtdlpPlaylistJson {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    uploader: Option<String>,
    #[serde(default)]
    entries: Vec<YtdlpEntryJson>,
}

#[derive(Debug, Deserialize)]
struct YtdlpEntryJson {
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    duration: Option<f64>,
}

/// Checks if a URL is a playlist URL
pub fn is_playlist_url(url: &Url) -> bool {
    let url_str = url.as_str().to_lowercase();

    // YouTube playlists
    if url_str.contains("youtube.com") || url_str.contains("youtu.be") {
        // Has list parameter
        if url.query_pairs().any(|(key, _)| key == "list") {
            return true;
        }
        // Is a playlist page
        if url_str.contains("/playlist") {
            return true;
        }
        // Is a channel or user
        if url_str.contains("/channel/")
            || url_str.contains("/c/")
            || url_str.contains("/user/")
            || url_str.contains("/@")
        {
            return true;
        }
    }

    // SoundCloud sets/albums
    if url_str.contains("soundcloud.com") && url_str.contains("/sets/") {
        return true;
    }

    // Spotify playlists/albums
    if url_str.contains("spotify.com") && (url_str.contains("/playlist/") || url_str.contains("/album/")) {
        return true;
    }

    false
}

/// Extracts playlist entries from a URL using yt-dlp --flat-playlist
pub async fn extract_playlist(url: &Url) -> Result<PlaylistInfo, String> {
    let ytdl_bin = &*config::YTDL_BIN;

    let mut args: Vec<&str> = vec![
        "--flat-playlist",
        "--dump-json",
        "-i", // Ignore errors
        "--socket-timeout",
        "30",
    ];

    // Add cookies if configured
    add_cookies_args(&mut args);

    args.push(url.as_str());

    log::info!("Extracting playlist from: {}", url);

    let mut cmd = Command::new(ytdl_bin);
    cmd.args(&args).stdout(Stdio::piped()).stderr(Stdio::piped());
    let output = run_with_timeout(&mut cmd, config::download::ytdlp_timeout())
        .await
        .map_err(|e| format!("Failed to run yt-dlp: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("yt-dlp failed: {}", stderr));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);

    // yt-dlp outputs one JSON object per line
    let mut playlist_title = None;
    let mut playlist_uploader = None;
    let mut entries = Vec::new();

    for (idx, line) in stdout.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }

        // Try to parse as playlist metadata first
        if idx == 0 {
            if let Ok(playlist_json) = serde_json::from_str::<YtdlpPlaylistJson>(line) {
                if playlist_json.title.is_some() && !playlist_json.entries.is_empty() {
                    // This is the playlist metadata with all entries
                    playlist_title = playlist_json.title;
                    playlist_uploader = playlist_json.uploader;

                    for (pos, entry) in playlist_json.entries.into_iter().enumerate() {
                        if entries.len() >= MAX_PLAYLIST_ITEMS {
                            break;
                        }

                        let video_url = entry
                            .url
                            .or_else(|| entry.id.map(|id| format!("https://www.youtube.com/watch?v={}", id)));

                        if let Some(video_url) = video_url {
                            entries.push(PlaylistEntry {
                                url: video_url,
                                title: entry.title.unwrap_or_else(|| format!("Video {}", pos + 1)),
                                duration: entry.duration.map(|d| d as u64),
                                position: pos + 1,
                            });
                        }
                    }

                    break;
                }
            }
        }

        // Parse as individual entry
        if let Ok(entry) = serde_json::from_str::<YtdlpEntryJson>(line) {
            if entries.len() >= MAX_PLAYLIST_ITEMS {
                continue;
            }

            let video_url = entry
                .url
                .or_else(|| entry.id.map(|id| format!("https://www.youtube.com/watch?v={}", id)));

            if let Some(video_url) = video_url {
                entries.push(PlaylistEntry {
                    url: video_url,
                    title: entry.title.unwrap_or_else(|| format!("Video {}", entries.len() + 1)),
                    duration: entry.duration.map(|d| d as u64),
                    position: entries.len() + 1,
                });
            }
        }
    }

    if entries.is_empty() {
        return Err("No videos found in playlist".to_string());
    }

    let total_count = entries.len();
    let truncated = total_count > MAX_PLAYLIST_ITEMS;

    Ok(PlaylistInfo {
        title: playlist_title.unwrap_or_else(|| "Playlist".to_string()),
        uploader: playlist_uploader,
        entry_count: total_count,
        entries: entries.into_iter().take(MAX_PLAYLIST_ITEMS).collect(),
        truncated,
    })
}

/// Extracts only the video ID from a YouTube URL, removing playlist parameters
pub fn clean_youtube_url(url: &Url) -> Option<Url> {
    if !url
        .host_str()
        .map(|h| h.contains("youtube.com") || h.contains("youtu.be"))
        .unwrap_or(false)
    {
        return Some(url.clone());
    }

    // For youtu.be short URLs
    if url.host_str() == Some("youtu.be") {
        let video_id = url.path().trim_start_matches('/');
        if !video_id.is_empty() {
            return Url::parse(&format!("https://www.youtube.com/watch?v={}", video_id)).ok();
        }
    }

    // For youtube.com URLs
    if let Some(video_id) = url
        .query_pairs()
        .find_map(|(k, v)| if k == "v" { Some(v.to_string()) } else { None })
    {
        return Url::parse(&format!("https://www.youtube.com/watch?v={}", video_id)).ok();
    }

    None
}

/// Format duration as mm:ss or hh:mm:ss
pub fn format_duration(seconds: u64) -> String {
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let secs = seconds % 60;

    if hours > 0 {
        format!("{}:{:02}:{:02}", hours, minutes, secs)
    } else {
        format!("{}:{:02}", minutes, secs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_playlist_url_youtube_list() {
        let url = Url::parse("https://www.youtube.com/watch?v=abc&list=PLdef").unwrap();
        assert!(is_playlist_url(&url));
    }

    #[test]
    fn test_is_playlist_url_youtube_playlist_page() {
        let url = Url::parse("https://www.youtube.com/playlist?list=PLdef").unwrap();
        assert!(is_playlist_url(&url));
    }

    #[test]
    fn test_is_playlist_url_youtube_channel() {
        let url = Url::parse("https://www.youtube.com/@channelname").unwrap();
        assert!(is_playlist_url(&url));
    }

    #[test]
    fn test_is_playlist_url_single_video() {
        let url = Url::parse("https://www.youtube.com/watch?v=abc").unwrap();
        assert!(!is_playlist_url(&url));
    }

    #[test]
    fn test_is_playlist_url_soundcloud_set() {
        let url = Url::parse("https://soundcloud.com/artist/sets/album-name").unwrap();
        assert!(is_playlist_url(&url));
    }

    #[test]
    fn test_clean_youtube_url() {
        let url = Url::parse("https://www.youtube.com/watch?v=abc&list=PLdef").unwrap();
        let cleaned = clean_youtube_url(&url).unwrap();
        assert_eq!(cleaned.as_str(), "https://www.youtube.com/watch?v=abc");
    }

    #[test]
    fn test_clean_youtube_url_short() {
        let url = Url::parse("https://youtu.be/abc").unwrap();
        let cleaned = clean_youtube_url(&url).unwrap();
        assert_eq!(cleaned.as_str(), "https://www.youtube.com/watch?v=abc");
    }

    #[test]
    fn test_format_duration() {
        assert_eq!(format_duration(65), "1:05");
        assert_eq!(format_duration(3661), "1:01:01");
        assert_eq!(format_duration(30), "0:30");
    }
}
