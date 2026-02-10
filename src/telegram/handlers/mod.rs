//! Telegram bot handler tree configuration
//!
//! This module provides the main dispatcher schema for the Telegram bot.
//! The handlers are organized in a testable way, allowing integration tests
//! to use the same handler tree as production code.

mod commands;
mod schema;
mod types;
mod uploads;

pub use schema::schema;
pub use types::{ensure_user_exists, HandlerDeps, HandlerError, UserCreationResult, UserInfo};
