//! Multi-backend download source abstraction layer.
//!
//! Core types (`DownloadSource`, `SourceRegistry`, `MediaMetadata`, etc.) are
//! re-exported from `doracore`. Bot-specific sources (`VlipsySource`) and the
//! bot-specific default registry live here.

// ── Bot-only source backends ─────────────────────────────────────────────────
pub use doracore::download::source::http; // SSRF-protected version from doracore
pub use doracore::download::source::instagram; // GraphQL API + rate limiter from doracore
pub mod vlipsy;
pub use doracore::download::source::ytdlp; // Single URL allowlist from doracore

// ── Re-export core types from doracore ───────────────────────────────────────
pub use doracore::download::source::{
    AdditionalFile, DownloadOutput, DownloadRequest, DownloadSource, MediaMetadata, SourceProgress, SourceRegistry,
};

// ── Bot-specific registry (adds VlipsySource) ───────────────────────────────

use std::sync::{Arc, LazyLock};

/// Create the bot's default registry with all 4 sources.
///
/// Priority order: Vlipsy → Instagram → yt-dlp → HTTP.
/// Vlipsy is bot-only; the other three come from doracore.
pub fn bot_default_registry() -> SourceRegistry {
    let mut registry = SourceRegistry::new();
    registry.register(Arc::new(vlipsy::VlipsySource::new()));
    registry.register(Arc::new(instagram::InstagramSource::new()));
    registry.register(Arc::new(ytdlp::YtDlpSource::new()));
    registry.register(Arc::new(http::HttpSource::new()));
    registry
}

static BOT_REGISTRY: LazyLock<SourceRegistry> = LazyLock::new(bot_default_registry);

/// Get the bot's shared default registry singleton.
///
/// Includes VlipsySource (bot-only) in addition to the doracore sources.
pub fn bot_global() -> &'static SourceRegistry {
    &BOT_REGISTRY
}
