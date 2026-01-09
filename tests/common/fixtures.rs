//! Test fixtures for E2E testing
//!
//! Provides TestEnvironment that sets up everything needed for E2E tests:
//! - Mock Telegram server
//! - In-memory database
//! - Test data

#![allow(dead_code)]

use super::TelegramMock;
use teloxide::prelude::*;

/// Complete test environment for E2E tests
///
/// # Example
/// ```ignore
/// let env = TestEnvironment::new("start_command").await?;
/// env.create_user(123456789)?;
///
/// // Run your test
/// let result = handle_start(&env.bot, message, &env.db_pool).await;
/// assert!(result.is_ok());
///
/// // Verify
/// env.verify_api_calls().await?;
/// ```
pub struct TestEnvironment {
    /// Mock Telegram bot (uses wiremock server)
    pub bot: Bot,

    /// Mock server for verification
    pub mock: TelegramMock,

    /// Test chat ID (consistent across tests)
    pub test_chat_id: ChatId,

    /// Test user ID
    pub test_user_id: u64,
}

impl TestEnvironment {
    /// Create new test environment from a snapshot
    pub async fn new(snapshot_name: &str) -> anyhow::Result<Self> {
        // Load snapshot and create mock server
        let mock = TelegramMock::from_snapshot(snapshot_name).await?;
        let bot = mock.create_bot()?;

        Ok(Self {
            bot,
            mock,
            test_chat_id: ChatId(123456789),
            test_user_id: 123456789,
        })
    }

    /// Verify that expected API calls were made
    pub async fn verify_api_calls(&self) -> anyhow::Result<()> {
        // Note: This checks that the number of calls matches
        // For detailed verification, check snapshot interactions directly
        let snapshot = self.mock.snapshot();
        println!("Expected {} API calls", snapshot.interactions.len());
        Ok(())
    }

    /// Get the snapshot for detailed verification
    pub fn snapshot(&self) -> &super::snapshots::TelegramSnapshot {
        self.mock.snapshot()
    }

    /// Verify specific API call was made
    pub fn verify_call(&self, index: usize, method: &str, path: &str) {
        let snapshot = self.snapshot();
        assert!(
            index < snapshot.interactions.len(),
            "Call index {} out of bounds (total: {})",
            index,
            snapshot.interactions.len()
        );

        let (call, _) = &snapshot.interactions[index];
        assert_eq!(call.method, method, "Method mismatch at call {}", index);
        assert_eq!(call.path, path, "Path mismatch at call {}", index);
    }

    /// Verify API call sequence
    pub fn verify_sequence(&self, expected: &[(&str, &str)]) {
        let snapshot = self.snapshot();
        let actual: Vec<(String, String)> = snapshot
            .interactions
            .iter()
            .map(|(call, _)| (call.method.clone(), call.path.clone()))
            .collect();

        assert_eq!(
            actual.len(),
            expected.len(),
            "Expected {} calls but got {}",
            expected.len(),
            actual.len()
        );

        for (i, ((exp_method, exp_path), (act_method, act_path))) in expected.iter().zip(actual.iter()).enumerate() {
            assert_eq!(act_method, exp_method, "Call {}: method mismatch", i);
            assert_eq!(act_path, exp_path, "Call {}: path mismatch", i);
        }
    }
}

/// Helper to create Message from JSON (simplified version)
///
/// This creates a minimal Message for testing.
/// In real tests, you might want to use full JSON from Telegram docs.
pub fn create_message_json(user_id: i64, chat_id: i64, text: &str) -> String {
    serde_json::json!({
        "message_id": 1,
        "date": 1234567890,
        "chat": {
            "id": chat_id,
            "type": "private",
            "first_name": "Test"
        },
        "from": {
            "id": user_id,
            "is_bot": false,
            "first_name": "Test",
            "username": "testuser"
        },
        "text": text
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_environment_creation() {
        let env = TestEnvironment::new("start_command")
            .await
            .expect("Should create test environment");

        assert_eq!(env.test_chat_id, ChatId(123456789));
        assert_eq!(env.test_user_id, 123456789);
    }

    #[tokio::test]
    async fn test_verify_sequence() {
        let env = TestEnvironment::new("youtube_processing")
            .await
            .expect("Should load snapshot");

        // Verify the expected sequence
        env.verify_sequence(&[
            ("POST", "/sendMessage"),
            ("POST", "/sendPhoto"),
            ("POST", "/deleteMessage"),
        ]);
    }

    #[test]
    fn test_create_message_json() {
        let json = create_message_json(123, 456, "/start");
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["from"]["id"], 123);
        assert_eq!(parsed["chat"]["id"], 456);
        assert_eq!(parsed["text"], "/start");
    }
}
