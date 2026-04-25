//! SoundCloud playlist resolver using yt-dlp --flat-playlist.

use anyhow::Context;
use async_trait::async_trait;
use std::time::Duration;
use tokio::process::Command as TokioCommand;
use tokio::time::timeout;

use crate::core::config;
use crate::download::search::{YtdlpFlatEntry, append_proxy_args};

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

    async fn resolve(&self, url: &str, progress: Option<ProgressFn>) -> anyhow::Result<ResolvedPlaylist> {
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
        .map_err(|_| anyhow::anyhow!("SoundCloud import timed out"))?
        .with_context(|| "Failed to execute yt-dlp")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("yt-dlp error: {}", stderr.lines().last().unwrap_or("unknown error"));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut tracks = Vec::new();

        for (i, line) in stdout.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let entry: YtdlpFlatEntry = match serde_json::from_str(line) {
                Ok(e) => e,
                Err(e) => {
                    log::warn!("SoundCloud yt-dlp parse error: {}", e);
                    continue;
                }
            };

            let uploader = entry.artist().map(|s| s.to_string());
            let raw_title = entry.title.unwrap_or_else(|| format!("Track {}", i + 1));
            let resolved_url = entry.webpage_url.or(entry.url);
            let duration = entry.duration.map(|d| d as i32);

            // SoundCloud titles often contain artist: "Artist1, Artist2 - Track Name [tags]"
            // Parse artist from title if uploader is missing or is just the account name
            let (title, artist) = parse_soundcloud_title(&raw_title, uploader.as_deref());

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
            anyhow::bail!("No tracks found in this SoundCloud playlist");
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

/// Parse SoundCloud title into (title, artist).
///
/// SoundCloud titles often follow the pattern: "Artist1, Artist2 - Track Name [genre tags]"
/// If a " - " separator is found, split into artist and title.
/// Falls back to uploader name if no separator and uploader is available.
fn parse_soundcloud_title(raw_title: &str, uploader: Option<&str>) -> (String, Option<String>) {
    // Try to split on " - " (the most common artist/title separator)
    if let Some(pos) = raw_title.find(" - ") {
        let artist_part = raw_title[..pos].trim();
        let title_part = raw_title[pos + 3..].trim();

        // Only split if both parts are non-empty and the artist part looks reasonable
        // (not too long — more than 80 chars is likely not an artist name)
        if !artist_part.is_empty() && !title_part.is_empty() && artist_part.len() <= 80 {
            return (title_part.to_string(), Some(artist_part.to_string()));
        }
    }

    // No separator found — use uploader as artist
    (raw_title.to_string(), uploader.map(|s| s.to_string()))
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
