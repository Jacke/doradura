//! Telegram bot handler tree configuration
//!
//! This module provides the main dispatcher schema for the Telegram bot.
//! The handlers are organized in a testable way, allowing integration tests
//! to use the same handler tree as production code.

mod commands;
mod schema;
mod types;
mod uploads;

pub use schema::{init_boot_timestamp, schema};
pub use types::{HandlerDeps, HandlerError, UserCreationResult, UserInfo, ensure_user_exists};
