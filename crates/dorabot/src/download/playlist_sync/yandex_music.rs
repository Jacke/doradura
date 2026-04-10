//! Yandex Music playlist resolver using yt-dlp with Russian proxy and cookies.

use anyhow::Context;
use async_trait::async_trait;
use std::time::Duration;
use tokio::process::Command as TokioCommand;
use tokio::time::timeout;

use crate::core::config;
use crate::download::search::YtdlpFlatEntry;

use super::resolver::{ImportTrack, Platform, PlaylistResolver, ProgressFn, ResolvedPlaylist, TrackStatus};

#[derive(Default)]
pub struct YandexMusicResolver;

impl YandexMusicResolver {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl PlaylistResolver for YandexMusicResolver {
    fn platform(&self) -> Platform {
        Platform::YandexMusic
    }

    fn supports_url(&self, url: &str) -> bool {
        let lower = url.to_lowercase();
        lower.contains("music.yandex.ru/") || lower.contains("music.yandex.com/")
    }

    async fn resolve(&self, url: &str, progress: Option<ProgressFn>) -> anyhow::Result<ResolvedPlaylist> {
        let cookies_file = config::yandex_music::COOKIES_FILE
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Yandex Music requires authentication. Admin: set YM_COOKIES_FILE"))?;

        let ytdl_bin = &*config::YTDL_BIN;

        let mut args: Vec<String> = vec![
            "--flat-playlist".to_string(),
            "--dump-json".to_string(),
            "--no-warnings".to_string(),
            "--no-check-certificate".to_string(),
            "--cookies".to_string(),
            cookies_file.to_string(),
        ];

        // Use YM-specific proxy if set, otherwise fall back to WARP proxy
        if let Some(ref proxy) = *config::yandex_music::PROXY {
            let proxy_url = proxy.trim();
            if !proxy_url.is_empty() {
                args.push("--proxy".to_string());
                args.push(proxy_url.to_string());
            }
        } else {
            crate::download::search::append_proxy_args(&mut args);
        }

        args.push(url.to_string());

        log::info!("Yandex Music resolve: {}", url);

        let output = timeout(
            Duration::from_secs(120),
            TokioCommand::new(ytdl_bin).args(&args).output(),
        )
        .await
        .map_err(|_| anyhow::anyhow!("Yandex Music import timed out"))?
        .with_context(|| "Failed to execute yt-dlp")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let last_line = stderr.lines().last().unwrap_or("unknown error");
            if last_line.contains("geo") || last_line.contains("country") || last_line.contains("403") {
                anyhow::bail!("Yandex Music unavailable from this region. A Russian proxy is required.");
            }
            anyhow::bail!("yt-dlp error: {}", last_line);
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
                    log::warn!("Yandex Music yt-dlp parse error: {}", e);
                    continue;
                }
            };

            let artist = entry.artist().map(|s| s.to_string());
            let title = entry.title.unwrap_or_else(|| format!("Track {}", i + 1));
            let resolved_url = entry.webpage_url.or(entry.url);
            let duration = entry.duration.map(|d| d as i32);

            if let Some(ref cb) = progress {
                cb(i + 1, 0, &title);
            }

            tracks.push(ImportTrack {
                title,
                artist,
                duration_secs: duration,
                external_id: resolved_url.as_deref().map(|u| format!("ym:{}", u)),
                source_url: resolved_url.clone(),
                resolved_url,
                status: TrackStatus::Matched,
            });
        }

        if tracks.is_empty() {
            anyhow::bail!("No tracks found in this Yandex Music playlist");
        }

        let name = extract_playlist_name(url);

        Ok(ResolvedPlaylist {
            name,
            description: None,
            tracks,
            platform: Platform::YandexMusic,
        })
    }
}

fn extract_playlist_name(url: &str) -> String {
    // music.yandex.ru/users/user/playlists/1000 or music.yandex.ru/album/12345
    if url.contains("/album/") {
        "Yandex Music Album".to_string()
    } else {
        "Yandex Music Playlist".to_string()
    }
}
