//! Download engine — source-agnostic backends, builder, progress types.

pub mod audio_effects;
pub mod builder;
pub mod cookies;
pub mod downloader;
pub mod error;
pub mod fetch;
pub mod metadata;
pub mod playlist;
pub mod pot_cache;
pub mod progress;
pub mod proxy;
pub mod ringtone;
pub mod source;
pub mod thumbnail;
pub mod ytdlp;
pub mod ytdlp_errors;

// Re-export key types
pub use downloader::{cleanup_partial_download, generate_file_name, generate_file_name_with_ext, parse_progress};
pub use error::DownloadError;
pub use proxy::{Proxy, ProxyList, ProxyListManager, ProxyProtocol, ProxySelectionStrategy};
pub use source::{DownloadOutput, DownloadSource, MediaMetadata, SourceProgress, SourceRegistry};
