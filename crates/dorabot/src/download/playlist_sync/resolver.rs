//! PlaylistResolver trait and shared types for external playlist import.

use async_trait::async_trait;
use std::sync::Arc;

/// Supported external platforms.
///
/// Two distinct string representations:
/// - **`Display`** produces the human-readable label (`"Spotify"`,
///   `"Yandex Music"`, ...) for UI.
/// - **`db_name()`** / **`from_db_name()`** produce the stable
///   snake_case DB column value (`"spotify"`, `"yandex_music"`, ...) —
///   derived via strum with its own `serialize_all` rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::Display)]
pub enum Platform {
    #[strum(serialize = "Spotify")]
    Spotify,
    #[strum(serialize = "SoundCloud")]
    SoundCloud,
    #[strum(serialize = "Yandex Music")]
    YandexMusic,
    #[strum(serialize = "YouTube")]
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

    /// Pretty human-readable label. Alias for `Display` for ergonomic
    /// call sites that prefer a method over `format!`.
    pub fn label(&self) -> &'static str {
        match self {
            Platform::Spotify => "Spotify",
            Platform::SoundCloud => "SoundCloud",
            Platform::YandexMusic => "Yandex Music",
            Platform::YouTube => "YouTube",
        }
    }

    /// snake_case identifier used in `synced_playlists.source_platform`.
    /// Kept as a manual function — strum can't express both the spaced
    /// `Display` form and the snake_case storage form on the same enum.
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

/// Track import status.
#[derive(Debug, Clone, PartialEq, Eq, strum::AsRefStr, strum::IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
pub enum TrackStatus {
    Matched,
    NotFound,
    Pending,
}

impl TrackStatus {
    /// Alias for `Into::<&'static str>::into` to preserve existing call sites.
    pub fn as_str(&self) -> &'static str {
        self.into()
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
