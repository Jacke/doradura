//! Snapshot storage and replay functionality

#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use teloxide::Bot;
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Represents a single API call to Telegram
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiCall {
    /// HTTP method (GET, POST, etc.)
    pub method: String,
    /// API endpoint path (e.g., "/sendMessage")
    pub path: String,
    /// Request body (JSON)
    pub body: serde_json::Value,
    /// Timestamp of the call
    pub timestamp: u64,
}

/// Represents a response from Telegram API
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiResponse {
    /// HTTP status code
    pub status: u16,
    /// Response body (JSON)
    pub body: serde_json::Value,
    /// Response headers
    pub headers: HashMap<String, String>,
}

/// A complete snapshot of bot interaction
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramSnapshot {
    /// Name/description of this snapshot
    pub name: String,
    /// Version of the snapshot format
    pub version: String,
    /// When this snapshot was recorded
    pub recorded_at: String,
    /// All API calls and their responses
    pub interactions: Vec<(ApiCall, ApiResponse)>,
    /// Metadata about the test scenario
    pub metadata: HashMap<String, String>,
}

impl TelegramSnapshot {
    /// Create a new empty snapshot
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: "1.0".to_string(),
            recorded_at: chrono::Utc::now().to_rfc3339(),
            interactions: Vec::new(),
            metadata: HashMap::new(),
        }
    }

    /// Add an interaction to the snapshot
    pub fn add_interaction(&mut self, call: ApiCall, response: ApiResponse) {
        self.interactions.push((call, response));
    }

    /// Save snapshot to file
    pub fn save(&self, path: impl AsRef<Path>) -> anyhow::Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    /// Load snapshot from file
    pub fn load(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let json = std::fs::read_to_string(path)?;
        let snapshot = serde_json::from_str(&json)?;
        Ok(snapshot)
    }

    /// Get default snapshots directory
    pub fn snapshots_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots")
    }

    /// Load snapshot by name from default directory
    pub fn load_by_name(name: &str) -> anyhow::Result<Self> {
        let path = Self::snapshots_dir().join(format!("{}.json", name));
        Self::load(path)
    }
}

/// Mock server that replays recorded Telegram API interactions
pub struct TelegramMock {
    server: MockServer,
    snapshot: TelegramSnapshot,
    calls_made: std::sync::Arc<std::sync::Mutex<Vec<ApiCall>>>,
}

impl TelegramMock {
    /// Create a mock from a snapshot file
    pub async fn from_snapshot(name: &str) -> anyhow::Result<Self> {
        let snapshot = TelegramSnapshot::load_by_name(name)?;
        Self::from_snapshot_data(snapshot).await
    }

    /// Create a mock from snapshot data
    pub async fn from_snapshot_data(snapshot: TelegramSnapshot) -> anyhow::Result<Self> {
        let server = MockServer::start().await;
        let calls_made = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

        // Mount mocks for each interaction
        for (call, response) in &snapshot.interactions {
            let response_body = response.body.clone();
            let status = response.status;

            // Create a flexible matcher that matches the method and path
            let mock = Mock::given(method(call.method.as_str()))
                .and(path_regex(format!("/bot[^/]+{}", regex::escape(&call.path))))
                .respond_with(ResponseTemplate::new(status).set_body_json(response_body));

            mock.mount(&server).await;
        }

        Ok(Self {
            server,
            snapshot,
            calls_made,
        })
    }

    /// Create a Bot instance that uses this mock server
    pub fn create_bot(&self) -> anyhow::Result<Bot> {
        let bot = Bot::new("test_token_12345:ABCDEF").set_api_url(self.server.uri().parse()?);
        Ok(bot)
    }

    /// Get the mock server URI
    pub fn uri(&self) -> String {
        self.server.uri()
    }

    /// Verify that expected calls were made (optional validation)
    pub async fn verify(&self) -> anyhow::Result<()> {
        let calls = self.calls_made.lock().unwrap();

        if calls.len() != self.snapshot.interactions.len() {
            anyhow::bail!(
                "Expected {} API calls but got {}",
                self.snapshot.interactions.len(),
                calls.len()
            );
        }

        Ok(())
    }

    /// Get the snapshot data
    pub fn snapshot(&self) -> &TelegramSnapshot {
        &self.snapshot
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_snapshot_creation() {
        let mut snapshot = TelegramSnapshot::new("test");

        snapshot.add_interaction(
            ApiCall {
                method: "POST".to_string(),
                path: "/sendMessage".to_string(),
                body: serde_json::json!({
                    "chat_id": 123,
                    "text": "Hello"
                }),
                timestamp: 1234567890,
            },
            ApiResponse {
                status: 200,
                body: serde_json::json!({
                    "ok": true,
                    "result": {
                        "message_id": 456,
                        "chat": {"id": 123, "type": "private"},
                        "text": "Hello"
                    }
                }),
                headers: HashMap::new(),
            },
        );

        assert_eq!(snapshot.interactions.len(), 1);
        assert_eq!(snapshot.name, "test");
    }

    #[test]
    fn test_snapshot_serialization() {
        let snapshot = TelegramSnapshot::new("test");
        let json = serde_json::to_string(&snapshot).unwrap();
        let deserialized: TelegramSnapshot = serde_json::from_str(&json).unwrap();

        assert_eq!(snapshot.name, deserialized.name);
        assert_eq!(snapshot.version, deserialized.version);
    }
}
