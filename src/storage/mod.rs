//! Database, cache, and backup functionality

pub mod backup;
pub mod cache;
pub mod db;

// Re-exports for convenience
pub use db::{create_pool, get_connection, DbConnection, DbPool};
