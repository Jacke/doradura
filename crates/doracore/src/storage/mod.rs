//! Database, cache, and backup functionality

pub mod backup;
pub mod cache;
pub mod db;
pub mod migrations;
pub mod shared;
pub mod subtitle_cache;
pub mod uploads;

// Re-exports for convenience
pub use db::{create_pool, get_connection, DbConnection, DbPool};
pub use shared::{QueueTaskInput, SharedStorage};
pub use subtitle_cache::SubtitleCache;
