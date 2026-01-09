//! Common test utilities
//!
//! This module is shared across all integration tests

// Re-export testing utilities
pub mod fixtures;
pub mod helpers;
pub mod recorder;
pub mod snapshots;

#[allow(unused_imports)]
pub use fixtures::{create_message_json, TestEnvironment};
#[allow(unused_imports)]
pub use helpers::create_test_chat_id;
#[allow(unused_imports)]
pub use recorder::{RecordingClient, RecordingMode};
#[allow(unused_imports)]
pub use snapshots::{ApiCall, ApiResponse, TelegramMock, TelegramSnapshot};
