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

// ============================================================================
// Typed mutation-response envelopes
//
// Replaces the scattered `Json(json!({"ok": true, ...}))` stringly-typed
// responses with compile-time-checked structs. Keeps the JSON shape
// byte-identical to what the admin SPA already expects.
// ============================================================================

/// Bare `{"ok": true}` success envelope.
#[derive(Serialize)]
pub(super) struct OkResponse {
    pub ok: bool,
}

impl OkResponse {
    pub fn ok() -> Self {
        Self { ok: true }
    }
}

/// `{"error": "..."}` error envelope used by `public.rs` rejections.
#[derive(Serialize)]
pub(super) struct ErrorResponse {
    pub error: &'static str,
}

/// Successful retry from the errors view: returns the new `task_id` and the
/// `user_id` the task was re-queued for.
#[derive(Serialize)]
pub(super) struct RetryOk {
    pub ok: bool,
    pub task_id: String,
    pub user_id: i64,
}

impl RetryOk {
    pub fn new(task_id: String, user_id: i64) -> Self {
        Self {
            ok: true,
            task_id,
            user_id,
        }
    }
}

/// Successful notify-user response from the errors view.
#[derive(Serialize)]
pub(super) struct NotifyOk {
    pub ok: bool,
    pub user_id: i64,
}

impl NotifyOk {
    pub fn new(user_id: i64) -> Self {
        Self { ok: true, user_id }
    }
}

/// Bulk-action response with a count.
#[derive(Serialize)]
pub(super) struct BulkCountOk {
    pub ok: bool,
    /// Key name is configurable via the sibling helper constructors so we can
    /// keep the wire shape identical to the old `json!` sites
    /// (`resolved`, `cancelled`, `total`, ...).
    #[serde(flatten)]
    pub count: std::collections::HashMap<&'static str, i64>,
}

impl BulkCountOk {
    pub fn new(key: &'static str, value: i64) -> Self {
        let mut m = std::collections::HashMap::with_capacity(1);
        m.insert(key, value);
        Self { ok: true, count: m }
    }
}

/// Plan-change response: includes resolved plan and optional expiry.
#[derive(Serialize)]
pub(super) struct PlanChangeOk {
    pub ok: bool,
    pub plan: String,
    pub expires_at: Option<String>,
}

impl PlanChangeOk {
    pub fn new(plan: String, expires_at: Option<String>) -> Self {
        Self {
            ok: true,
            plan,
            expires_at,
        }
    }
}

/// Block/unblock response: just echoes the new state.
#[derive(Serialize)]
pub(super) struct BlockOk {
    pub ok: bool,
    pub blocked: bool,
}

impl BlockOk {
    pub fn new(blocked: bool) -> Self {
        Self { ok: true, blocked }
    }
}

/// Settings-update response: the list of fields that were actually updated
/// (so the SPA can show "Language updated, plan updated").
#[derive(Serialize)]
pub(super) struct SettingsUpdatedOk {
    pub ok: bool,
    pub updated: Vec<&'static str>,
}

impl SettingsUpdatedOk {
    pub fn new(updated: Vec<&'static str>) -> Self {
        Self { ok: true, updated }
    }
}

/// Feedback-status update response.
#[derive(Serialize)]
pub(super) struct FeedbackStatusOk {
    pub ok: bool,
    pub status: String,
}

impl FeedbackStatusOk {
    pub fn new(status: String) -> Self {
        Self { ok: true, status }
    }
}

/// Single broadcast-send response.
#[derive(Serialize)]
pub(super) struct BroadcastSingleOk {
    pub ok: bool,
    pub sent: u32,
    pub blocked: u32,
    pub failed: u32,
}

impl BroadcastSingleOk {
    pub fn sent() -> Self {
        Self {
            ok: true,
            sent: 1,
            blocked: 0,
            failed: 0,
        }
    }
    pub fn blocked() -> Self {
        Self {
            ok: true,
            sent: 0,
            blocked: 1,
            failed: 0,
        }
    }
}

/// Start-broadcast response (async bulk).
#[derive(Serialize)]
pub(super) struct BroadcastStartOk {
    pub ok: bool,
    pub total: i64,
    pub status: &'static str,
}

impl BroadcastStartOk {
    pub fn broadcasting(total: i64) -> Self {
        Self {
            ok: true,
            total,
            status: "broadcasting",
        }
    }
}

/// Subscription toggle response.
#[derive(Serialize)]
pub(super) struct ToggleOk {
    pub ok: bool,
    pub is_active: bool,
}

impl ToggleOk {
    pub fn new(is_active: bool) -> Self {
        Self { ok: true, is_active }
    }
}
