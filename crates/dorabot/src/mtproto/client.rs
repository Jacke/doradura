//! MTProto client wrapper around grammers

use super::error::MtProtoError;
use grammers_client::{Client, Config, InitParams};
use grammers_session::Session;
use std::path::Path;

/// MTProto client wrapper for bot operations
pub struct MtProtoClient {
    client: Client,
}

impl MtProtoClient {
    /// Create a new MTProto client and sign in as bot
    ///
    /// # Arguments
    /// * `api_id` - Telegram API ID from my.telegram.org
    /// * `api_hash` - Telegram API hash from my.telegram.org
    /// * `bot_token` - Bot token from @BotFather
    /// * `session_path` - Path to save/load session file
    pub async fn new_bot(
        api_id: i32,
        api_hash: &str,
        bot_token: &str,
        session_path: &Path,
    ) -> Result<Self, MtProtoError> {
        log::info!("Initializing MTProto client...");

        // Load or create session
        let session = if session_path.exists() {
            log::info!("Loading existing session from {:?}", session_path);
            Session::load_file(session_path)
                .map_err(|e| MtProtoError::Session(format!("Failed to load session: {}", e)))?
        } else {
            log::info!("Creating new session");
            Session::new()
        };

        // Create client configuration
        let config = Config {
            session,
            api_id,
            api_hash: api_hash.to_string(),
            params: InitParams {
                // Use a recognizable device model for debugging
                device_model: "Doradura MTProto Client".to_string(),
                system_version: "1.0".to_string(),
                app_version: env!("CARGO_PKG_VERSION").to_string(),
                system_lang_code: "en".to_string(),
                lang_code: "en".to_string(),
                ..Default::default()
            },
        };

        // Connect to Telegram
        log::info!("Connecting to Telegram...");
        let client = Client::connect(config)
            .await
            .map_err(|e| MtProtoError::Session(format!("Failed to connect: {}", e)))?;

        // Sign in if not authorized
        if !client.is_authorized().await.map_err(MtProtoError::Invocation)? {
            log::info!("Not authorized, signing in as bot...");
            client
                .bot_sign_in(bot_token)
                .await
                .map_err(|e| MtProtoError::SignIn(format!("{}", e)))?;

            // Save session after successful sign-in
            // Ensure parent directory exists
            if let Some(parent) = session_path.parent() {
                if !parent.as_os_str().is_empty() && !parent.exists() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| MtProtoError::Session(format!("Failed to create session directory: {}", e)))?;
                }
            }
            // grammers-session 0.5 requires file to exist before saving (uses write, not create)
            if !session_path.exists() {
                std::fs::File::create(session_path)
                    .map_err(|e| MtProtoError::Session(format!("Failed to create session file: {}", e)))?;
            }
            client
                .session()
                .save_to_file(session_path)
                .map_err(|e| MtProtoError::Session(format!("Failed to save session: {}", e)))?;
            log::info!("Session saved to {:?}", session_path);
        } else {
            log::info!("Already authorized");
        }

        Ok(Self { client })
    }

    /// Get the underlying grammers client
    pub fn inner(&self) -> &Client {
        &self.client
    }

    /// Get the datacenter ID this client is connected to
    pub fn dc_id(&self) -> i32 {
        // grammers doesn't expose dc_id directly, default to 2 (common DC)
        2
    }

    /// Check if the client is authorized
    pub async fn is_authorized(&self) -> Result<bool, MtProtoError> {
        self.client.is_authorized().await.map_err(MtProtoError::Invocation)
    }

    /// Get information about the bot
    pub async fn get_me(&self) -> Result<grammers_client::types::User, MtProtoError> {
        self.client.get_me().await.map_err(MtProtoError::Invocation)
    }
}
