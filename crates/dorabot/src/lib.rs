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
// v0.50.4: workspace-wide `clippy::unwrap_used` / `expect_used` are set to
// `warn`. Existing 800+ usages suppressed here — remove the allow at
// file/module scope as code is cleaned up. New `.unwrap()` should be `?`,
// `.ok()`, `.unwrap_or_default()`, or `.expect("INVARIANT: ...")`.
#![allow(clippy::unwrap_used)]
#![allow(clippy::expect_used)]
#![allow(clippy::panic)]
#![allow(clippy::unreachable)]
#![allow(clippy::unwrap_in_result)]
#![allow(unsafe_code)]

// ── Local modules ─────────────────────────────────────────────────────────────
pub mod background_tasks;
pub mod cli;
pub mod cli_commands;
pub mod core;
pub mod download;
pub mod metadata_refresh;
pub mod mtproto;
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
pub use core::{BotError, config};
pub use download::{DownloadQueue, download_and_send_audio, download_and_send_subtitles, download_and_send_video};
pub use storage::{DbConnection, DbPool, QueueTaskInput, SharedStorage, create_pool, get_connection};
pub use telegram::{
    Completed, DEFAULT_EMOJI, InProgress, MarkdownV2Formatter, MessageFormatter, NotStarted, Operation,
    OperationBuilder, OperationError, OperationInfo, OperationStatus, PlainTextFormatter, handle_menu_callback,
    handle_message, show_main_menu,
};
