//! Doradura - High-performance Telegram bot for downloading music and videos
//!
//! This library provides all the core functionality for the Doradura bot,
//! including download management, database operations, queue management,
//! and Telegram bot integration.
//!
//! # Module Structure
//!
//! - `core`: Core utilities, configuration, errors, and common features
//! - `storage`: Database, cache, and backup functionality

#![allow(clippy::too_many_arguments)]
#![allow(clippy::manual_flatten)]
//! - `download`: Download management and processing
//! - `telegram`: Telegram bot integration and handlers

pub mod cli;
pub mod core;
pub mod download;
pub mod downsub;
pub mod experimental;
pub mod i18n;
pub mod metadata_refresh;
pub mod storage;
pub mod telegram;

// Testing utilities (only available in tests and test binaries)
#[cfg(test)]
pub mod testing;

// Re-export commonly used types for convenience
pub use core::{config, BotError};
pub use download::{download_and_send_audio, download_and_send_subtitles, download_and_send_video, DownloadQueue};
pub use storage::{create_pool, get_connection, DbConnection, DbPool};
pub use telegram::{
    handle_menu_callback, handle_message, show_main_menu, Completed, InProgress, MarkdownV2Formatter, MessageFormatter,
    NotStarted, Operation, OperationBuilder, OperationError, OperationInfo, OperationStatus, PlainTextFormatter,
    DEFAULT_EMOJI,
};
