//! MTProto client for direct Telegram API access via grammers
//!
//! This module provides low-level access to Telegram's MTProto API,
//! allowing file downloads by file_id without going through Bot API.

pub mod client;
pub mod downloader;
pub mod error;
pub mod file_id;

pub use client::MtProtoClient;
pub use downloader::{MediaInfo, MediaType, MessageInfo, MtProtoDownloader, PeerInfo, PeerType};
pub use error::MtProtoError;
pub use file_id::{DecodedFileId, FileType};
