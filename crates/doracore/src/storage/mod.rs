//! Database, cache, and backup functionality

pub mod backup;
pub mod cache;
pub mod db;
pub mod migrations;
pub mod shared;
pub mod subtitle_cache;
pub mod uploads;

// Re-exports for convenience
pub use db::{DbConnection, DbPool, create_pool, get_connection};
pub use shared::{QueueTaskInput, SharePageRecord, SharedStorage};
pub use subtitle_cache::SubtitleCache;
