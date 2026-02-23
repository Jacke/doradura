//! Media download management with pluggable source backends.
//!
//! Shared, Telegram-free modules are re-exported from `doracore`.
//! Bot-specific modules (pipeline, queue, audio/video send, Telegram progress) live here.

// ── Shared modules — re-exported from doracore (identical) ───────────────────
pub use doracore::download::audio_effects;
pub use doracore::download::cookies;
pub use doracore::download::error;
pub use doracore::download::fetch;
pub use doracore::download::playlist;
pub use doracore::download::proxy;
pub use doracore::download::ringtone;
pub use doracore::download::thumbnail;
pub use doracore::download::ytdlp;
pub use doracore::download::ytdlp_errors;

// ── Local modules (use dorabot-local DownloadRequest / source types) ─────────
pub mod builder; // DownloadRequest builder (uses local source::DownloadRequest)

// ── Bot-specific modules ──────────────────────────────────────────────────────
pub mod audio; // Telegram audio download + send pipeline
pub mod downloader; // Full download logic with Telegram upload
pub mod metadata; // Metadata fetching with admin notifications
pub mod pipeline; // Orchestration pipeline for bot flows
pub mod progress; // Telegram progress messages
pub mod queue; // Download queue management
pub mod send; // Telegram send utilities
pub mod source; // Source backends (bot-specific YtDlp/Instagram behaviour)
pub mod video; // Telegram video download + send pipeline

// ── Re-exports for convenience ────────────────────────────────────────────────
pub use audio::download_and_send_audio;
pub use downloader::{cleanup_partial_download, download_and_send_subtitles};
pub use proxy::{Proxy, ProxyList, ProxyListManager, ProxyProtocol, ProxySelectionStrategy};
pub use queue::DownloadQueue;
pub use video::download_and_send_video;
