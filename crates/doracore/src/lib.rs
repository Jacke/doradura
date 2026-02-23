//! doradura-core — shared library for download engine, storage, i18n, conversion
//! No Telegram/teloxide dependencies.

#![allow(clippy::too_many_arguments)]
#![allow(clippy::manual_flatten)]

pub mod conversion;
pub mod core;
pub mod download;
pub mod downsub;
pub mod extension;
pub mod i18n;
pub mod lyrics;
pub mod storage;
pub mod timestamps;

// Re-export common types
pub use core::{config, BotError};
pub use storage::{create_pool, get_connection, DbConnection, DbPool};
