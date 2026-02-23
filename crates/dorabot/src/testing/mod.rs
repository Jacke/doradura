//! Testing utilities for snapshot-based bot testing
//!
//! This module provides tools for recording real bot interactions and replaying them in tests.
//!
//! ## Usage
//!
//! ### Recording mode:
//! ```bash
//! TELEGRAM_RECORD_MODE=true cargo run
//! ```
//!
//! ### Replay mode (in tests):
//! ```rust
//! #[tokio::test]
//! async fn test_start_command() {
//!     let mock = TelegramMock::from_snapshot("start_command").await;
//!     let bot = mock.create_bot();
//!     // Your test code here
//!     mock.verify().await;
//! }
//! ```

pub mod recorder;
pub mod snapshots;

pub use recorder::{RecordingClient, RecordingMode};
pub use snapshots::{ApiCall, ApiResponse, TelegramMock, TelegramSnapshot};
