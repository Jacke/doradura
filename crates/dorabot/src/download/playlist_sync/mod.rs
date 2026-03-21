//! External playlist sync: import playlists from Spotify, SoundCloud, YouTube, Yandex Music.

pub mod resolver;
pub mod soundcloud;
pub mod spotify;
pub mod yandex_music;

use std::sync::Arc;

use resolver::{Platform, PlaylistResolver, ProgressFn, ResolvedPlaylist};
use soundcloud::SoundCloudResolver;
use spotify::SpotifyResolver;
use yandex_music::YandexMusicResolver;

use crate::storage::db::DbPool;

/// YouTube playlist resolver (same pattern as SoundCloud — yt-dlp flat-playlist).
mod youtube {
    use async_trait::async_trait;
    use std::time::Duration;
    use tokio::process::Command as TokioCommand;
    use tokio::time::timeout;

    use crate::core::config;
    use crate::download::search::{append_proxy_args, YtdlpFlatEntry};

    use super::resolver::{ImportTrack, Platform, PlaylistResolver, ProgressFn, ResolvedPlaylist, TrackStatus};

    pub struct YouTubeResolver;

    #[async_trait]
    impl PlaylistResolver for YouTubeResolver {
        fn platform(&self) -> Platform {
            Platform::YouTube
        }

        fn supports_url(&self, url: &str) -> bool {
            let lower = url.to_lowercase();
            (lower.contains("youtube.com/playlist") || lower.contains("youtu.be/")) && lower.contains("list=")
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

            log::info!("YouTube playlist resolve: {}", url);

            let output = timeout(
                Duration::from_secs(120),
                TokioCommand::new(ytdl_bin).args(&args).output(),
            )
            .await
            .map_err(|_| "YouTube playlist import timed out".to_string())?
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
                    Err(e) => {
                        log::warn!("yt-dlp playlist parse error: {}", e);
                        continue;
                    }
                };

                let artist = entry.artist().map(|s| s.to_string());
                let title = entry.title.unwrap_or_else(|| format!("Track {}", i + 1));
                let raw_url = entry.webpage_url.or(entry.url);
                // Skip non-video entries (channels, playlists within playlists)
                let resolved_url = raw_url.filter(|u| {
                    !u.contains("/channel/") && !u.contains("/playlist?") && !u.contains("/user/") && !u.contains("/@")
                });
                let duration = entry.duration.map(|d| d as i32);

                if let Some(ref cb) = progress {
                    cb(i + 1, 0, &title);
                }

                tracks.push(ImportTrack {
                    title,
                    artist,
                    duration_secs: duration,
                    external_id: resolved_url.as_deref().map(|u| format!("yt:{}", u)),
                    source_url: resolved_url.clone(),
                    resolved_url,
                    status: TrackStatus::Matched,
                });
            }

            if tracks.is_empty() {
                return Err("No tracks found in this YouTube playlist".to_string());
            }

            Ok(ResolvedPlaylist {
                name: "YouTube Playlist".to_string(),
                description: None,
                tracks,
                platform: Platform::YouTube,
            })
        }
    }
}

/// Detect platform from URL.
pub fn detect_platform(url: &str) -> Option<Platform> {
    let lower = url.to_lowercase();
    if lower.contains("open.spotify.com/") {
        Some(Platform::Spotify)
    } else if lower.contains("soundcloud.com/") {
        Some(Platform::SoundCloud)
    } else if lower.contains("music.yandex.ru/") || lower.contains("music.yandex.com/") {
        Some(Platform::YandexMusic)
    } else if (lower.contains("youtube.com/playlist") || lower.contains("youtu.be/")) && lower.contains("list=") {
        Some(Platform::YouTube)
    } else {
        None
    }
}

/// Get the appropriate resolver for a URL.
pub fn get_resolver(url: &str, db_pool: Arc<DbPool>) -> Option<Box<dyn PlaylistResolver>> {
    let platform = detect_platform(url)?;
    Some(match platform {
        Platform::Spotify => Box::new(SpotifyResolver::new(db_pool)),
        Platform::SoundCloud => Box::new(SoundCloudResolver::new()),
        Platform::YandexMusic => Box::new(YandexMusicResolver::new()),
        Platform::YouTube => Box::new(youtube::YouTubeResolver),
    })
}

/// Import a playlist from URL, returning resolved playlist data.
pub async fn import_playlist(
    url: &str,
    db_pool: Arc<DbPool>,
    progress: Option<ProgressFn>,
) -> Result<ResolvedPlaylist, String> {
    let resolver = get_resolver(url, db_pool).ok_or_else(|| {
        "Unsupported URL. Supported: Spotify, SoundCloud, YouTube, Yandex Music playlists".to_string()
    })?;

    resolver.resolve(url, progress).await
}

/// Save a resolved playlist to the database. Returns the playlist ID.
pub fn save_resolved_playlist(
    conn: &crate::storage::db::DbConnection,
    user_id: i64,
    source_url: &str,
    resolved: &ResolvedPlaylist,
) -> Result<i64, String> {
    use crate::storage::db;

    let matched = resolved
        .tracks
        .iter()
        .filter(|t| t.status == resolver::TrackStatus::Matched)
        .count() as i32;
    let not_found = resolved
        .tracks
        .iter()
        .filter(|t| t.status == resolver::TrackStatus::NotFound)
        .count() as i32;

    conn.execute_batch("BEGIN IMMEDIATE")
        .map_err(|e| format!("Failed to begin transaction: {}", e))?;

    let result = (|| -> Result<i64, String> {
        let playlist_id = db::create_synced_playlist(
            conn,
            user_id,
            &resolved.name,
            resolved.description.as_deref(),
            source_url,
            resolved.platform.db_name(),
            resolved.tracks.len() as i32,
            matched,
            not_found,
        )
        .map_err(|e| format!("Failed to save playlist: {}", e))?;

        for (i, track) in resolved.tracks.iter().enumerate() {
            db::add_synced_track(
                conn,
                playlist_id,
                i as i32,
                &track.title,
                track.artist.as_deref(),
                track.duration_secs,
                track.external_id.as_deref(),
                track.source_url.as_deref(),
                track.resolved_url.as_deref(),
                track.status.as_str(),
            )
            .map_err(|e| format!("Failed to save track: {}", e))?;
        }

        Ok(playlist_id)
    })();

    match result {
        Ok(id) => {
            conn.execute_batch("COMMIT")
                .map_err(|e| format!("Failed to commit: {}", e))?;
            Ok(id)
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}
