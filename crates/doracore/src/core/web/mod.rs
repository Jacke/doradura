//! Public-facing web server for share pages and admin dashboard.
//!
//! Serves beautiful ambilight share pages with streaming links at /s/{id}.
//! Runs on WEB_PORT (default 3000) alongside the internal metrics server.

use std::sync::Arc;

use axum::{
    extract::{DefaultBodyLimit, Request},
    middleware::{self, Next},
    response::Response,
    routing::{get, post},
    Router,
};
use std::net::SocketAddr;
use tokio::net::TcpListener;

use crate::core::config;
use crate::core::types::PlanChangeNotifier;
use crate::storage::SharedStorage;

mod admin_errors;
mod admin_misc;
mod admin_queue;
mod admin_users;
mod auth;
mod dashboard;
mod helpers;
mod public;
mod types;

use types::WebState;

/// Middleware that injects standard security headers into every response.
async fn security_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert("X-Frame-Options", "DENY".parse().unwrap());
    headers.insert("X-Content-Type-Options", "nosniff".parse().unwrap());
    headers.insert("Referrer-Policy", "strict-origin-when-cross-origin".parse().unwrap());
    headers.insert(
        "Content-Security-Policy",
        "default-src 'self'; script-src 'self' https://telegram.org 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' https: data:; connect-src 'self'; frame-src https://oauth.telegram.org; frame-ancestors 'none'"
            .parse()
            .unwrap(),
    );
    headers.insert("Access-Control-Allow-Origin", "null".parse().unwrap());
    response
}

/// Start the public web server.
pub async fn start_web_server(
    port: u16,
    shared_storage: Arc<SharedStorage>,
    plan_notifier: Option<PlanChangeNotifier>,
) -> Result<(), Box<dyn std::error::Error>> {
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let bot_token = config::BOT_TOKEN.clone();
    let state = WebState {
        shared_storage,
        bot_token,
        plan_notifier,
    };

    let app = Router::new()
        .route("/s/{id}", get(public::share_page_handler))
        .route("/api/s/{id}", get(public::share_api_handler))
        .route("/health", get(public::health_handler))
        .route("/privacy", get(public::privacy_handler))
        // Admin routes
        .route("/admin", get(dashboard::admin_dashboard_handler))
        .route("/admin/login", get(auth::admin_login_handler))
        .route("/admin/auth", get(auth::admin_auth_handler))
        .route("/admin/logout", get(auth::admin_logout_handler))
        // Admin API
        .route("/admin/api/users", get(admin_users::admin_api_users))
        .route("/admin/api/users/{id}/plan", post(admin_users::admin_api_user_plan))
        .route("/admin/api/users/{id}/block", post(admin_users::admin_api_user_block))
        .route("/admin/api/downloads", get(admin_users::admin_api_downloads))
        // Queue API
        .route("/admin/api/queue", get(admin_queue::admin_api_queue))
        .route("/admin/api/queue/{id}/retry", post(admin_queue::admin_api_queue_retry))
        .route("/admin/api/queue/{id}/cancel", post(admin_queue::admin_api_queue_cancel))
        // Errors API (paginated)
        .route("/admin/api/errors", get(admin_errors::admin_api_errors))
        .route("/admin/api/errors/{id}/resolve", post(admin_errors::admin_api_error_resolve))
        // Feedback API
        .route("/admin/api/feedback", get(admin_misc::admin_api_feedback))
        .route("/admin/api/feedback/{id}/status", post(admin_misc::admin_api_feedback_status))
        // Alerts API
        .route("/admin/api/alerts", get(admin_misc::admin_api_alerts))
        .route("/admin/api/alerts/{id}/acknowledge", post(admin_misc::admin_api_alert_acknowledge))
        // User details + Health + Broadcast + Revenue + Analytics
        .route("/admin/api/users/{id}/details", get(admin_users::admin_api_user_details))
        .route("/admin/api/users/{id}/settings", post(admin_users::admin_api_user_settings))
        .route("/admin/api/health", get(admin_misc::admin_api_health))
        .route("/admin/api/broadcast", post(admin_misc::admin_api_broadcast))
        .route("/admin/api/revenue", get(admin_misc::admin_api_revenue))
        .route("/admin/api/analytics", get(admin_misc::admin_api_analytics))
        .route("/admin/api/audit", get(admin_misc::admin_api_audit))
        // Bulk actions
        .route("/admin/api/errors/bulk-resolve", post(admin_errors::admin_api_errors_bulk_resolve))
        .route("/admin/api/queue/bulk-cancel", post(admin_queue::admin_api_queue_bulk_cancel))
        // Content subscriptions
        .route("/admin/api/subscriptions", get(admin_misc::admin_api_subscriptions))
        .route("/admin/api/subscriptions/{id}/toggle", post(admin_misc::admin_api_sub_toggle))
        // Lightweight polling for tab badges
        .route("/admin/api/counts", get(admin_misc::admin_api_counts))
        .route("/metrics", get(public::metrics_handler))
        .with_state(state)
        .layer(DefaultBodyLimit::max(1024 * 1024)) // 1 MB
        .layer(middleware::from_fn(security_headers));

    log::info!("Starting web server on http://{}", addr);
    log::info!("  /s/:id      - Share page (HTML)");
    log::info!("  /api/s/:id  - Share page (JSON)");
    log::info!("  /privacy    - Privacy Policy");
    log::info!("  /admin      - Admin Dashboard");
    log::info!("  /health     - Health check");
    log::info!("  /metrics    - Prometheus metrics (Bearer auth)");

    let listener = TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
