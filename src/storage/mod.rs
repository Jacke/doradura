//! Database, cache, and backup functionality

pub mod backup;
pub mod cache;
pub mod db;
pub mod migrations;
pub mod uploads;

// Re-exports for convenience
pub use db::{create_pool, get_connection, DbConnection, DbPool};
