//! SoundCloud playlist resolver using yt-dlp --flat-playlist.

use async_trait::async_trait;
use std::time::Duration;
use tokio::process::Command as TokioCommand;
use tokio::time::timeout;

use crate::core::config;
use crate::download::search::{append_proxy_args, YtdlpFlatEntry};

use super::resolver::{ImportTrack, Platform, PlaylistResolver, ProgressFn, ResolvedPlaylist, TrackStatus};

#[derive(Default)]
pub struct SoundCloudResolver;

impl SoundCloudResolver {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl PlaylistResolver for SoundCloudResolver {
    fn platform(&self) -> Platform {
        Platform::SoundCloud
    }

    fn supports_url(&self, url: &str) -> bool {
        let lower = url.to_lowercase();
        lower.contains("soundcloud.com/")
            && (lower.contains("/sets/")
                || lower.contains("/likes")
                || lower.contains("/tracks")
                || lower.contains("/albums/"))
    }

    async fn resolve(&self, url: &str, progress: Option<ProgressFn>) -> Result<ResolvedPlaylist, String> {
        let ytdl_bin = &*config::YTDL_BIN;

        let mut args: Vec<String> = vec![
            "--flat-playlist".to_string(),
            "--dump-json".to_string(),
            "--no-warnings".to_string(),
            "--no-check-certificate".to_string(),
        ];
        append_proxy_args(&mut args);
        args.push(url.to_string());

        log::info!("SoundCloud resolve: {}", url);

        let output = timeout(
            Duration::from_secs(120),
            TokioCommand::new(ytdl_bin).args(&args).output(),
        )
        .await
        .map_err(|_| "SoundCloud import timed out".to_string())?
        .map_err(|e| format!("Failed to execute yt-dlp: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!(
                "yt-dlp error: {}",
                stderr.lines().last().unwrap_or("unknown error")
            ));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut tracks = Vec::new();

        for (i, line) in stdout.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let entry: YtdlpFlatEntry = match serde_json::from_str(line) {
                Ok(e) => e,
                Err(_) => continue,
            };

            let artist = entry.artist().map(|s| s.to_string());
            let title = entry.title.unwrap_or_else(|| format!("Track {}", i + 1));
            let resolved_url = entry.webpage_url.or(entry.url);
            let duration = entry.duration.map(|d| d as i32);

            if let Some(ref cb) = progress {
                cb(i + 1, 0, &title); // total unknown during streaming
            }

            tracks.push(ImportTrack {
                title,
                artist,
                duration_secs: duration,
                external_id: resolved_url.as_deref().map(|u| format!("sc:{}", u)),
                source_url: resolved_url.clone(),
                resolved_url,
                status: TrackStatus::Matched,
            });
        }

        if tracks.is_empty() {
            return Err("No tracks found in this SoundCloud playlist".to_string());
        }

        // Extract playlist name from URL
        let name = extract_playlist_name(url);

        Ok(ResolvedPlaylist {
            name,
            description: None,
            tracks,
            platform: Platform::SoundCloud,
        })
    }
}

fn extract_playlist_name(url: &str) -> String {
    // Try to get a meaningful name from URL: soundcloud.com/user/sets/name
    if let Some(idx) = url.rfind('/') {
        let slug = &url[idx + 1..];
        let slug = slug.split('?').next().unwrap_or(slug);
        if !slug.is_empty() && slug != "likes" {
            return slug.replace('-', " ");
        }
    }
    "SoundCloud Playlist".to_string()
}
