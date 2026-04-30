//! doradura-core — shared library for download engine, storage, i18n, conversion
//! No Telegram/teloxide dependencies.

#![allow(clippy::too_many_arguments)]
#![allow(clippy::manual_flatten)]
// v0.50.4: workspace-wide `clippy::unwrap_used` / `expect_used` are set to
// `warn` so new code stays out of unwrap-land. Existing codebase has
// 800+ usages — clean up gradually by removing this `#![allow]` at file
// or module scope as you fix them. Don't add new `.unwrap()` without an
// `#[allow]` and a comment.
#![allow(clippy::unwrap_used)]
#![allow(clippy::expect_used)]
#![allow(clippy::panic)]
#![allow(clippy::unreachable)]
#![allow(clippy::unwrap_in_result)]

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
pub use core::{BotError, config};
pub use storage::{DbConnection, DbPool, QueueTaskInput, SharedStorage, create_pool, get_connection};
