//! Download management and processing

pub mod audio;
pub mod audio_effects;
pub mod cookies;
pub mod downloader;
pub mod fetch;
pub mod metadata;
pub mod playlist;
pub mod progress;
pub mod proxy;
pub mod queue;
pub mod ringtone;
pub mod send;
pub mod thumbnail;
pub mod video;
pub mod ytdlp;
pub mod ytdlp_errors;

// Re-exports for convenience
pub use audio::download_and_send_audio;
pub use downloader::{download_and_send_subtitles, download_and_send_video};
pub use proxy::{Proxy, ProxyList, ProxyListManager, ProxyProtocol, ProxySelectionStrategy};
pub use queue::DownloadQueue;
