//! Mock implementations for load testing
//!
//! This module provides mock implementations of download components
//! for load testing without actual network or filesystem operations.

pub mod mock_downloader;

pub use mock_downloader::{MockDownloader, MockDownloaderConfig};
