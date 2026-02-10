//! Handler types, dependencies, and user management helpers

use std::sync::Arc;

use teloxide::prelude::*;
use teloxide::types::Message;

use crate::core::alerts::AlertManager;
use crate::core::rate_limiter::RateLimiter;
use crate::download::queue::DownloadQueue;
use crate::downsub::DownsubGateway;
use crate::extension::ExtensionRegistry;
use crate::storage::db::{self, create_user, create_user_with_language, get_user};
use crate::storage::get_connection;
use crate::telegram::notifications::notify_admin_new_user;
use crate::telegram::Bot;

/// Error type for handlers
pub type HandlerError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// Dependencies required by handlers
#[derive(Clone)]
pub struct HandlerDeps {
    pub db_pool: Arc<db::DbPool>,
    pub download_queue: Arc<DownloadQueue>,
    pub rate_limiter: Arc<RateLimiter>,
    pub downsub_gateway: Arc<DownsubGateway>,
    pub bot_username: Option<String>,
    pub bot_id: UserId,
    pub alert_manager: Option<Arc<AlertManager>>,
    pub extension_registry: Arc<ExtensionRegistry>,
}

impl HandlerDeps {
    /// Create new handler dependencies
    pub fn new(
        db_pool: Arc<db::DbPool>,
        download_queue: Arc<DownloadQueue>,
        rate_limiter: Arc<RateLimiter>,
        downsub_gateway: Arc<DownsubGateway>,
        bot_username: Option<String>,
        bot_id: UserId,
        alert_manager: Option<Arc<AlertManager>>,
        extension_registry: Arc<ExtensionRegistry>,
    ) -> Self {
        Self {
            db_pool,
            download_queue,
            rate_limiter,
            downsub_gateway,
            bot_username,
            bot_id,
            alert_manager,
            extension_registry,
        }
    }
}

/// User info for admin notifications
#[derive(Clone)]
pub struct UserInfo {
    pub chat_id: i64,
    pub username: Option<String>,
    pub first_name: Option<String>,
    pub language_code: Option<String>,
}

impl UserInfo {
    /// Extract user info from a Telegram message
    pub fn from_message(msg: &Message) -> Self {
        Self {
            chat_id: msg.chat.id.0,
            username: msg.from.as_ref().and_then(|u| u.username.clone()),
            first_name: msg.from.as_ref().map(|u| u.first_name.clone()),
            language_code: msg.from.as_ref().and_then(|u| u.language_code.clone()),
        }
    }
}

/// Result of ensure_user_exists operation
pub enum UserCreationResult {
    /// User already existed
    Existed,
    /// User was newly created
    Created,
    /// Failed to get DB connection
    DbError,
}

/// Ensures a user exists in the database, creating them if needed.
///
/// This is a helper function to deduplicate the common pattern of:
/// 1. Getting a DB connection
/// 2. Checking if user exists
/// 3. Creating user if not
/// 4. Notifying admins about new users
///
/// # Arguments
/// * `db_pool` - Database connection pool
/// * `bot` - Bot instance for admin notifications
/// * `user` - User information
/// * `first_action` - Description of user's first action (for admin notification)
///
/// # Returns
/// `UserCreationResult` indicating whether user existed, was created, or there was an error
pub fn ensure_user_exists(
    db_pool: &Arc<db::DbPool>,
    bot: &Bot,
    user: &UserInfo,
    first_action: Option<&str>,
) -> UserCreationResult {
    let conn = match get_connection(db_pool) {
        Ok(c) => c,
        Err(_) => return UserCreationResult::DbError,
    };

    // Check if user already exists
    match get_user(&conn, user.chat_id) {
        Ok(Some(_)) => UserCreationResult::Existed,
        Ok(None) => {
            // Create user with language if available
            let create_result = if let Some(ref lang) = user.language_code {
                create_user_with_language(&conn, user.chat_id, user.username.clone(), lang)
            } else {
                create_user(&conn, user.chat_id, user.username.clone())
            };

            if create_result.is_ok() {
                // Spawn notification task
                let bot_clone = bot.clone();
                let user_id = user.chat_id;
                let username = user.username.clone();
                let first_name = user.first_name.clone();
                let lang = user.language_code.clone();
                let action = first_action.map(|s| s.to_string());

                tokio::spawn(async move {
                    notify_admin_new_user(
                        &bot_clone,
                        user_id,
                        username.as_deref(),
                        first_name.as_deref(),
                        lang.as_deref(),
                        action.as_deref(),
                    )
                    .await;
                });

                UserCreationResult::Created
            } else {
                UserCreationResult::DbError
            }
        }
        Err(_) => UserCreationResult::DbError,
    }
}
