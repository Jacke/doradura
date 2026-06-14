//! Unified playlist-sync layer (Explore "Subscriptions" / sub-project B).
//!
//! Platform-agnostic core: a [`PlaylistProvider`] fetches a playlist from one
//! platform and returns the neutral [`PlaylistSnapshot`]. The sync engine and
//! download bridge (later steps) operate purely on these neutral types and never
//! touch a specific platform — adding a platform means implementing the trait and
//! registering it, with zero changes to the engine.
//!
//! Mirrors the existing `download::source::{DownloadSource, SourceRegistry}`
//! pattern, but for *reading playlists* rather than downloading media.

pub mod spotify;

use async_trait::async_trait;
use std::sync::Arc;
use url::Url;

/// Which platform a playlist lives on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Platform {
    Spotify,
    YouTube,
    AppleMusic,
    SoundCloud,
}

impl Platform {
    /// Stable lowercase id (DB/storage key, logging).
    pub fn id(self) -> &'static str {
        match self {
            Platform::Spotify => "spotify",
            Platform::YouTube => "youtube",
            Platform::AppleMusic => "apple_music",
            Platform::SoundCloud => "soundcloud",
        }
    }
}

/// Stable handle to a playlist, independent of platform.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PlaylistRef {
    pub platform: Platform,
    /// Platform-native playlist id (e.g. a Spotify playlist id).
    pub id: String,
}

/// One track in platform-neutral form. `isrc` (when present) is the gold key for
/// cross-platform matching; otherwise the bridge falls back to artist+title search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaylistTrack {
    /// The platform's own track id — the stable de-dup key for sync diffing.
    pub external_id: String,
    pub title: String,
    pub artist: String,
    /// ISRC, when the platform exposes it → near-perfect YouTube match.
    pub isrc: Option<String>,
    pub duration_secs: Option<u32>,
}

/// Immutable point-in-time view of a playlist's contents.
#[derive(Debug, Clone)]
pub struct PlaylistSnapshot {
    pub playlist: PlaylistRef,
    pub name: String,
    pub tracks: Vec<PlaylistTrack>,
}

/// Read-only connector to one playlist platform. Implementors translate a
/// platform-specific playlist into the neutral [`PlaylistSnapshot`]; they know
/// nothing about scheduling, downloading, or the DB.
#[async_trait]
pub trait PlaylistProvider: Send + Sync {
    /// Which platform this provider serves.
    fn platform(&self) -> Platform;

    /// Parse a URL into a [`PlaylistRef`], or `None` if this provider doesn't
    /// own the URL.
    fn parse_ref(&self, url: &Url) -> Option<PlaylistRef>;

    /// Fetch the current contents of the playlist.
    async fn fetch(&self, playlist: &PlaylistRef) -> anyhow::Result<PlaylistSnapshot>;
}

/// URL-routed set of providers. Mirrors `download::source::SourceRegistry`.
#[derive(Default, Clone)]
pub struct PlaylistRegistry {
    providers: Vec<Arc<dyn PlaylistProvider>>,
}

impl PlaylistRegistry {
    pub fn new() -> Self {
        Self { providers: Vec::new() }
    }

    pub fn register(&mut self, provider: Arc<dyn PlaylistProvider>) {
        self.providers.push(provider);
    }

    /// First provider that recognizes `url`, with the parsed ref.
    pub fn route(&self, url: &Url) -> Option<(Arc<dyn PlaylistProvider>, PlaylistRef)> {
        self.providers
            .iter()
            .find_map(|p| p.parse_ref(url).map(|r| (Arc::clone(p), r)))
    }
}
