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
pub use doracore::download::builder; // DownloadRequest builder (shared with doracore)

// ── Bot-specific modules ──────────────────────────────────────────────────────
pub mod audio; // Telegram audio download + send pipeline
pub mod context; // Shared DownloadContext for download entry points
pub mod downloader; // Full download logic with Telegram upload
pub use doracore::download::metadata; // Metadata fetching (shared with doracore)
pub mod pipeline; // Orchestration pipeline for bot flows
pub mod playlist_import; // External playlist import (YouTube, Spotify)
pub mod playlist_sync;
pub mod progress; // Telegram progress messages
pub mod queue; // Download queue management
pub mod search; // Music search engine (YouTube, SoundCloud)
pub mod send; // Telegram send utilities
pub mod source; // Source backends (bot-specific YtDlp/Instagram behaviour)
pub mod vault; // Vault cache: private channel file storage
pub mod video; // Telegram video download + send pipeline // External playlist sync (Spotify, SoundCloud, YM, YouTube)

// ── Re-exports for convenience ────────────────────────────────────────────────
pub use audio::download_and_send_audio;
pub use downloader::{cleanup_partial_download, download_and_send_subtitles};
pub use proxy::{Proxy, ProxyList, ProxyListManager, ProxyProtocol, ProxySelectionStrategy};
pub use queue::DownloadQueue;
pub use video::download_and_send_video;
