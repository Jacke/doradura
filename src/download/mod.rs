//! Download management and processing

pub mod downloader;
pub mod fetch;
pub mod progress;
pub mod queue;
pub mod ytdlp;
pub mod ytdlp_errors;

// Re-exports for convenience
pub use downloader::{
    download_and_send_audio, download_and_send_subtitles, download_and_send_video,
};
pub use queue::DownloadQueue;
