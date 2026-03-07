//! Doradura - High-performance Telegram bot for downloading and converting media
//!
//! This library provides all the core functionality for the Doradura bot,
//! including download management, database operations, queue management,
//! and Telegram bot integration.
//!
//! # Module Structure
//!
//! - `core`: Core utilities, configuration, errors, and common features
//! - `storage`: Database, cache, and backup functionality
//! - `download`: Download management and processing
//! - `telegram`: Telegram bot integration and handlers

#![allow(clippy::too_many_arguments)]
#![allow(clippy::manual_flatten)]

// ── Local modules ─────────────────────────────────────────────────────────────
pub mod background_tasks;
pub mod cli;
pub mod cli_commands;
pub mod core;
pub mod download;
pub mod experimental;
pub mod metadata_refresh;
pub mod queue_processor;
pub mod startup;
pub mod telegram;
pub mod vlipsy;
pub mod watcher;
pub mod webhook;

// ── Shared modules — re-exported from doracore ───────────────────────────────
pub use doracore::conversion;
pub use doracore::downsub;
pub use doracore::extension;
pub use doracore::i18n;
pub use doracore::lyrics;
pub use doracore::storage;
pub use doracore::timestamps;

// ── Testing utilities ─────────────────────────────────────────────────────────
#[cfg(test)]
pub mod testing;

pub mod smoke_tests;

// ── Re-exports for convenience ────────────────────────────────────────────────
pub use core::{config, BotError};
pub use download::{download_and_send_audio, download_and_send_subtitles, download_and_send_video, DownloadQueue};
pub use storage::{create_pool, get_connection, DbConnection, DbPool, QueueTaskInput, SharedStorage};
pub use telegram::{
    handle_menu_callback, handle_message, show_main_menu, Completed, InProgress, MarkdownV2Formatter, MessageFormatter,
    NotStarted, Operation, OperationBuilder, OperationError, OperationInfo, OperationStatus, PlainTextFormatter,
    DEFAULT_EMOJI,
};
