//! Type definitions for the web server module.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use std::sync::LazyLock;
use tokio::sync::RwLock;

use crate::core::types::PlanChangeNotifier;
use crate::storage::SharedStorage;

// --- Rate limiters ---

pub(super) static AUTH_RATE_LIMIT: LazyLock<RwLock<HashMap<String, (u32, std::time::Instant)>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

pub(super) static SHARE_RATE_LIMIT: LazyLock<RwLock<HashMap<String, (u32, std::time::Instant)>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

pub(super) const AUTH_MAX_ATTEMPTS: u32 = 10;
pub(super) const AUTH_WINDOW_SECS: u64 = 300;
pub(super) const SHARE_MAX_PER_MIN: u32 = 60;
pub(super) const SHARE_WINDOW_SECS: u64 = 60;

// --- Page size constants ---
// Each handler sub-module defines its own local constant for the page size it uses.
// The canonical values are 50 for all paginated endpoints.

// --- Core state ---

/// Shared state for the web server.
#[derive(Clone)]
pub(super) struct WebState {
    pub shared_storage: Arc<SharedStorage>,
    pub bot_token: String,
    pub plan_notifier: Option<PlanChangeNotifier>,
}

/// Query parameters from Telegram Login Widget
#[derive(Deserialize, Debug)]
pub(super) struct TelegramAuth {
    pub id: i64,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub username: Option<String>,
    pub photo_url: Option<String>,
    pub auth_date: i64,
    pub hash: String,
}

// --- Admin API types ---

#[derive(Deserialize)]
pub(super) struct UserQuery {
    pub page: Option<u32>,
    pub filter: Option<String>,
    pub search: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct DownloadQuery {
    pub page: Option<u32>,
    pub search: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct PlanUpdateReq {
    pub plan: String,
    /// Optional expiry in days from now. If set, creates/updates subscription with expires_at.
    pub expires_days: Option<i32>,
}

#[derive(Deserialize)]
pub(super) struct BlockUpdateReq {
    pub blocked: bool,
}

#[derive(Serialize)]
pub(super) struct ApiUser {
    pub telegram_id: i64,
    pub username: String,
    pub plan: String,
    pub is_blocked: bool,
    pub download_count: i64,
    pub language: String,
}

#[derive(Serialize)]
pub(super) struct ApiDownload {
    pub id: i64,
    pub title: String,
    pub author: String,
    pub user: String,
    pub user_id: i64,
    pub format: String,
    pub file_size: Option<i64>,
    pub duration: Option<i64>,
    pub video_quality: String,
    pub audio_bitrate: String,
    pub downloaded_at: String,
    pub url: String,
}

#[derive(Serialize)]
pub(super) struct PaginatedResponse<T: Serialize> {
    pub items: Vec<T>,
    pub total: i64,
    pub page: u32,
    pub per_page: u32,
    pub total_pages: u32,
}

// --- Queue API types ---

#[derive(Deserialize)]
pub(super) struct QueueQuery {
    pub page: Option<u32>,
    pub status: Option<String>,
    pub search: Option<String>,
}

#[derive(Serialize)]
pub(super) struct ApiQueueTask {
    pub id: String,
    pub user_id: i64,
    pub username: String,
    pub url: String,
    pub format: String,
    pub status: String,
    pub error_message: String,
    pub retry_count: i32,
    pub worker_id: String,
    pub created_at: String,
    pub started_at: String,
    pub finished_at: String,
}

// --- Error API types ---

#[derive(Deserialize)]
pub(super) struct ErrorQuery {
    pub page: Option<u32>,
    pub error_type: Option<String>,
    pub resolved: Option<String>,
    pub search: Option<String>,
}

#[derive(Serialize)]
pub(super) struct ApiError {
    pub id: i64,
    pub timestamp: String,
    pub user_id: Option<i64>,
    pub username: String,
    pub error_type: String,
    pub error_message: String,
    pub url: String,
    pub context: String,
    pub resolved: bool,
}

// --- Feedback API types ---

#[derive(Deserialize)]
pub(super) struct FeedbackQuery {
    pub page: Option<u32>,
    pub status: Option<String>,
    pub search: Option<String>,
}

#[derive(Serialize)]
pub(super) struct ApiFeedback {
    pub id: i64,
    pub user_id: i64,
    pub username: String,
    pub first_name: String,
    pub message: String,
    pub status: String,
    pub admin_reply: String,
    pub created_at: String,
}

#[derive(Deserialize)]
pub(super) struct FeedbackStatusReq {
    pub status: String,
}

// --- Alert API types ---

#[derive(Deserialize)]
pub(super) struct AlertQuery {
    pub page: Option<u32>,
    pub severity: Option<String>,
    pub search: Option<String>,
}

#[derive(Serialize)]
pub(super) struct ApiAlert {
    pub id: i64,
    pub alert_type: String,
    pub severity: String,
    pub message: String,
    pub metadata: String,
    pub triggered_at: String,
    pub resolved_at: String,
    pub acknowledged: bool,
}

// --- Revenue API types ---

#[derive(Deserialize)]
pub(super) struct RevenueQuery {
    pub page: Option<u32>,
    pub plan: Option<String>,
}

#[derive(Serialize)]
pub(super) struct ApiChargeEntry {
    pub id: i64,
    pub user_id: i64,
    pub username: String,
    pub plan: String,
    pub amount: i64,
    pub currency: String,
    pub is_recurring: bool,
    pub payment_date: String,
}

// --- Analytics API types ---

#[derive(Deserialize)]
pub(super) struct AnalyticsQuery {
    pub days: Option<u32>,
}

// --- User settings API types ---

#[derive(Deserialize)]
pub(super) struct UserSettingsReq {
    pub language: Option<String>,
    pub plan: Option<String>,
    pub plan_days: Option<i32>,
    pub is_blocked: Option<bool>,
}

// --- Broadcast API types ---

#[derive(Deserialize)]
pub(super) struct BroadcastReq {
    pub target: String,
    pub message: String,
}

// --- Audit log API types ---

#[derive(Deserialize)]
pub(super) struct AuditQuery {
    pub page: Option<u32>,
    pub action: Option<String>,
}

#[derive(Serialize)]
pub(super) struct ApiAuditEntry {
    pub id: i64,
    pub admin_id: i64,
    pub action: String,
    pub target_type: String,
    pub target_id: String,
    pub details: String,
    pub created_at: String,
}

// --- Content subscriptions API types ---

#[derive(Deserialize)]
pub(super) struct SubsQuery {
    pub page: Option<u32>,
    pub status: Option<String>,
    pub search: Option<String>,
}

#[derive(Serialize)]
pub(super) struct ApiContentSub {
    pub id: i64,
    pub user_id: i64,
    pub username: String,
    pub source_type: String,
    pub source_id: String,
    pub display_name: String,
    pub is_active: bool,
    pub last_checked_at: String,
    pub last_error: String,
    pub consecutive_errors: i32,
    pub created_at: String,
}

#[derive(Deserialize)]
pub(super) struct SubToggleReq {
    pub is_active: bool,
}

// --- Bulk action types ---

#[derive(Deserialize)]
pub(super) struct BulkResolveReq {
    pub error_type: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct BulkCancelReq {
    pub status: Option<String>,
}

/// Body for POST /admin/api/errors/:id/notify — send a custom message to the
/// affected user about the error, optionally also marking it resolved.
#[derive(Deserialize)]
pub(super) struct NotifyUserReq {
    /// Message to send. If empty, uses a default "your issue has been resolved" text.
    #[serde(default)]
    pub message: String,
    /// Whether to also mark the error as resolved in the same action.
    #[serde(default)]
    pub mark_resolved: bool,
}

// AdminStats is defined locally in dashboard.rs where it is used.
