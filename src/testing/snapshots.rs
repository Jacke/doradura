//! Snapshot storage and replay functionality

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

    // ==================== TelegramSnapshot Tests ====================

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

    #[test]
    fn test_snapshot_new_defaults() {
        let snapshot = TelegramSnapshot::new("my-snapshot");
        assert_eq!(snapshot.name, "my-snapshot");
        assert_eq!(snapshot.version, "1.0");
        assert!(snapshot.interactions.is_empty());
        assert!(snapshot.metadata.is_empty());
        assert!(!snapshot.recorded_at.is_empty());
    }

    #[test]
    fn test_snapshot_with_metadata() {
        let mut snapshot = TelegramSnapshot::new("test");
        snapshot
            .metadata
            .insert("scenario".to_string(), "download_mp3".to_string());
        snapshot.metadata.insert("user_id".to_string(), "12345".to_string());

        assert_eq!(snapshot.metadata.get("scenario"), Some(&"download_mp3".to_string()));
        assert_eq!(snapshot.metadata.len(), 2);
    }

    #[test]
    fn test_snapshot_save_and_load() {
        let temp_dir = std::env::temp_dir();
        let temp_file = temp_dir.join(format!("test_snapshot_{}.json", std::process::id()));

        let mut snapshot = TelegramSnapshot::new("test_save");
        snapshot.metadata.insert("test".to_string(), "value".to_string());
        snapshot.add_interaction(
            ApiCall {
                method: "GET".to_string(),
                path: "/getMe".to_string(),
                body: serde_json::json!({}),
                timestamp: 123,
            },
            ApiResponse {
                status: 200,
                body: serde_json::json!({"ok": true}),
                headers: HashMap::new(),
            },
        );

        // Save
        snapshot.save(&temp_file).unwrap();
        assert!(temp_file.exists());

        // Load
        let loaded = TelegramSnapshot::load(&temp_file).unwrap();
        assert_eq!(loaded.name, "test_save");
        assert_eq!(loaded.interactions.len(), 1);
        assert_eq!(loaded.metadata.get("test"), Some(&"value".to_string()));

        // Cleanup
        let _ = std::fs::remove_file(&temp_file);
    }

    #[test]
    fn test_snapshot_load_nonexistent() {
        let result = TelegramSnapshot::load("/nonexistent/path.json");
        assert!(result.is_err());
    }

    #[test]
    fn test_snapshots_dir() {
        let dir = TelegramSnapshot::snapshots_dir();
        assert!(dir.ends_with("tests/snapshots"));
    }

    // ==================== ApiCall Tests ====================

    #[test]
    fn test_api_call_serialization() {
        let call = ApiCall {
            method: "POST".to_string(),
            path: "/sendMessage".to_string(),
            body: serde_json::json!({"chat_id": 123}),
            timestamp: 1000,
        };

        let json = serde_json::to_string(&call).unwrap();
        let deserialized: ApiCall = serde_json::from_str(&json).unwrap();

        assert_eq!(call.method, deserialized.method);
        assert_eq!(call.path, deserialized.path);
        assert_eq!(call.timestamp, deserialized.timestamp);
    }

    // ==================== ApiResponse Tests ====================

    #[test]
    fn test_api_response_serialization() {
        let mut headers = HashMap::new();
        headers.insert("Content-Type".to_string(), "application/json".to_string());

        let response = ApiResponse {
            status: 200,
            body: serde_json::json!({"ok": true}),
            headers,
        };

        let json = serde_json::to_string(&response).unwrap();
        let deserialized: ApiResponse = serde_json::from_str(&json).unwrap();

        assert_eq!(response.status, deserialized.status);
        assert_eq!(response.headers.len(), 1);
    }

    #[test]
    fn test_api_response_error() {
        let response = ApiResponse {
            status: 400,
            body: serde_json::json!({
                "ok": false,
                "error_code": 400,
                "description": "Bad Request"
            }),
            headers: HashMap::new(),
        };

        assert_eq!(response.status, 400);
        assert_eq!(response.body.get("ok"), Some(&serde_json::json!(false)));
    }
}
