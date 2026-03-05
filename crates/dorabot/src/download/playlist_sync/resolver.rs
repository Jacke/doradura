//! PlaylistResolver trait and shared types for external playlist import.

use async_trait::async_trait;
use std::fmt;
use std::sync::Arc;

/// Supported external platforms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Spotify,
    SoundCloud,
    YandexMusic,
    YouTube,
}

impl Platform {
    pub fn icon(&self) -> &'static str {
        match self {
            Platform::Spotify => "🟢",
            Platform::SoundCloud => "🟠",
            Platform::YandexMusic => "🟣",
            Platform::YouTube => "🔴",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Platform::Spotify => "Spotify",
            Platform::SoundCloud => "SoundCloud",
            Platform::YandexMusic => "Yandex Music",
            Platform::YouTube => "YouTube",
        }
    }

    pub fn db_name(&self) -> &'static str {
        match self {
            Platform::Spotify => "spotify",
            Platform::SoundCloud => "soundcloud",
            Platform::YandexMusic => "yandex_music",
            Platform::YouTube => "youtube",
        }
    }

    pub fn from_db_name(name: &str) -> Option<Self> {
        match name {
            "spotify" => Some(Platform::Spotify),
            "soundcloud" => Some(Platform::SoundCloud),
            "yandex_music" => Some(Platform::YandexMusic),
            "youtube" => Some(Platform::YouTube),
            _ => None,
        }
    }
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.label())
    }
}

/// Track import status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrackStatus {
    Matched,
    NotFound,
    Pending,
}

impl TrackStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            TrackStatus::Matched => "matched",
            TrackStatus::NotFound => "not_found",
            TrackStatus::Pending => "pending",
        }
    }
}

/// A track resolved from an external playlist.
#[derive(Debug, Clone)]
pub struct ImportTrack {
    pub title: String,
    pub artist: Option<String>,
    pub duration_secs: Option<i32>,
    pub external_id: Option<String>,
    pub source_url: Option<String>,
    pub resolved_url: Option<String>,
    pub status: TrackStatus,
}

/// Result of resolving an external playlist.
#[derive(Debug)]
pub struct ResolvedPlaylist {
    pub name: String,
    pub description: Option<String>,
    pub tracks: Vec<ImportTrack>,
    pub platform: Platform,
}

/// Progress callback type: (current_track, total_tracks, track_title).
pub type ProgressFn = Arc<dyn Fn(usize, usize, &str) + Send + Sync>;

/// Trait for platform-specific playlist resolvers.
#[async_trait]
pub trait PlaylistResolver: Send + Sync {
    fn platform(&self) -> Platform;
    fn supports_url(&self, url: &str) -> bool;
    async fn resolve(&self, url: &str, progress: Option<ProgressFn>) -> Result<ResolvedPlaylist, String>;
}
