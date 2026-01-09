//! HTTP client that records Telegram API interactions
//!
//! NOTE: This is a simplified version for demonstration.
//! For actual recording, use the Python tool or manual snapshot creation.

#![allow(dead_code)]

use super::snapshots::{ApiCall, ApiResponse, TelegramSnapshot};
use std::sync::{Arc, Mutex};

/// Mode for the recording client
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingMode {
    /// Don't record anything
    Disabled,
    /// Record all interactions
    Enabled,
    /// Record and also print to console
    Verbose,
}

impl RecordingMode {
    /// Get recording mode from environment variable
    pub fn from_env() -> Self {
        match std::env::var("TELEGRAM_RECORD_MODE").as_deref() {
            Ok("true") | Ok("1") | Ok("enabled") => Self::Enabled,
            Ok("verbose") => Self::Verbose,
            _ => Self::Disabled,
        }
    }

    pub fn is_enabled(&self) -> bool {
        matches!(self, Self::Enabled | Self::Verbose)
    }

    pub fn is_verbose(&self) -> bool {
        matches!(self, Self::Verbose)
    }
}

/// HTTP client wrapper that records all requests/responses
pub struct RecordingClient {
    mode: RecordingMode,
    snapshot: Arc<Mutex<TelegramSnapshot>>,
}

impl RecordingClient {
    /// Create a new recording client
    pub fn new(snapshot_name: impl Into<String>) -> Self {
        Self::with_mode(snapshot_name, RecordingMode::from_env())
    }

    /// Create a recording client with specific mode
    pub fn with_mode(snapshot_name: impl Into<String>, mode: RecordingMode) -> Self {
        Self {
            mode,
            snapshot: Arc::new(Mutex::new(TelegramSnapshot::new(snapshot_name))),
        }
    }

    /// Save recorded snapshot to file
    pub fn save(&self, path: impl AsRef<std::path::Path>) -> anyhow::Result<()> {
        let snapshot = self.snapshot.lock().unwrap();

        if snapshot.interactions.is_empty() {
            log::warn!("⚠️  No interactions recorded, skipping save");
            return Ok(());
        }

        snapshot.save(path.as_ref())?;

        log::info!(
            "💾 Saved {} interactions to {}",
            snapshot.interactions.len(),
            path.as_ref().display()
        );

        Ok(())
    }

    /// Save to default snapshots directory
    pub fn save_to_default_dir(&self) -> anyhow::Result<()> {
        let snapshot = self.snapshot.lock().unwrap();
        let dir = TelegramSnapshot::snapshots_dir();
        std::fs::create_dir_all(&dir)?;

        let path = dir.join(format!("{}.json", snapshot.name));
        drop(snapshot); // Release lock before calling save
        self.save(path)
    }

    /// Get the current snapshot (for inspection)
    pub fn get_snapshot(&self) -> TelegramSnapshot {
        self.snapshot.lock().unwrap().clone()
    }

    /// Get number of recorded interactions
    pub fn interaction_count(&self) -> usize {
        self.snapshot.lock().unwrap().interactions.len()
    }

    /// Manually add an interaction
    pub fn add_interaction(&self, call: ApiCall, response: ApiResponse) {
        let mut snapshot = self.snapshot.lock().unwrap();
        snapshot.add_interaction(call, response);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_recording_mode_from_env() {
        std::env::set_var("TELEGRAM_RECORD_MODE", "true");
        assert_eq!(RecordingMode::from_env(), RecordingMode::Enabled);

        std::env::set_var("TELEGRAM_RECORD_MODE", "verbose");
        assert_eq!(RecordingMode::from_env(), RecordingMode::Verbose);

        std::env::remove_var("TELEGRAM_RECORD_MODE");
        assert_eq!(RecordingMode::from_env(), RecordingMode::Disabled);
    }

    #[test]
    fn test_recording_client_creation() {
        let client = RecordingClient::new("test");
        assert_eq!(client.interaction_count(), 0);
    }
}
