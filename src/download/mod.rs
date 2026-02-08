//! Media download management with pluggable source backends

pub mod audio;
pub mod audio_effects;
pub mod builder;
pub mod cookies;
pub mod downloader;
pub mod fetch;
pub mod metadata;
pub mod pipeline;
pub mod playlist;
pub mod progress;
pub mod proxy;
pub mod queue;
pub mod ringtone;
pub mod send;
pub mod source;
pub mod thumbnail;
pub mod video;
pub mod ytdlp;
pub mod ytdlp_errors;

// Re-exports for convenience
pub use audio::download_and_send_audio;
pub use downloader::{cleanup_partial_download, download_and_send_subtitles};
pub use proxy::{Proxy, ProxyList, ProxyListManager, ProxyProtocol, ProxySelectionStrategy};
pub use queue::DownloadQueue;
pub use video::download_and_send_video;
