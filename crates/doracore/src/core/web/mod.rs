//! Public-facing web server for share pages and admin dashboard.
//!
//! Serves beautiful ambilight share pages with streaming links at /s/{id}.
//! Runs on WEB_PORT (default 3000) alongside the internal metrics server.

use std::sync::Arc;

use axum::{
    extract::{ConnectInfo, DefaultBodyLimit, Request},
    http::StatusCode,
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use std::net::{IpAddr, SocketAddr};
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

/// Derive the trusted client IP for an incoming request.
///
/// By default, trust the socket peer address (set by axum via `ConnectInfo`).
/// If `TRUSTED_PROXY_HOPS=N` is set, trust the N-th-from-right entry of
/// `X-Forwarded-For` (the *rightmost* entries are added by trusted proxies;
/// anything further left is attacker-controlled). Fails closed — unknown = deny.
fn trusted_client_ip(req: &Request, socket_addr: IpAddr) -> IpAddr {
    let hops: usize = std::env::var("TRUSTED_PROXY_HOPS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    if hops == 0 {
        return socket_addr;
    }

    if let Some(xff) = req.headers().get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
        let parts: Vec<&str> = xff.split(',').map(str::trim).collect();
        if parts.len() >= hops {
            if let Ok(ip) = parts[parts.len() - hops].parse::<IpAddr>() {
                return ip;
            }
        }
    }
    socket_addr
}

/// IP allowlist middleware for `/admin/*` routes.
///
/// Fails **closed**: if `ADMIN_IP_ALLOWLIST` is empty, ALL admin routes are
/// blocked with 404 (indistinguishable from "route not found" to the attacker).
/// Non-admin routes pass through unchanged.
async fn admin_ip_guard(ConnectInfo(socket_addr): ConnectInfo<SocketAddr>, req: Request, next: Next) -> Response {
    if !req.uri().path().starts_with("/admin") {
        return next.run(req).await;
    }

    let allowlist_str = std::env::var("ADMIN_IP_ALLOWLIST").unwrap_or_default();
    if allowlist_str.trim().is_empty() {
        log::error!("ADMIN_IP_ALLOWLIST not configured — blocking all admin routes");
        return (StatusCode::NOT_FOUND, "Not Found").into_response();
    }

    let allowlist: Vec<IpAddr> = allowlist_str.split(',').filter_map(|s| s.trim().parse().ok()).collect();

    if allowlist.is_empty() {
        log::error!("ADMIN_IP_ALLOWLIST is set but contains no valid IPs — blocking");
        return (StatusCode::NOT_FOUND, "Not Found").into_response();
    }

    let peer = trusted_client_ip(&req, socket_addr.ip());
    if allowlist.contains(&peer) {
        next.run(req).await
    } else {
        log::warn!("Admin access denied from {} (path {})", peer, req.uri().path());
        (StatusCode::NOT_FOUND, "Not Found").into_response()
    }
}

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
        .route("/admin/api/errors/{id}/retry", post(admin_errors::admin_api_error_retry))
        .route("/admin/api/errors/{id}/notify", post(admin_errors::admin_api_error_notify))
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
        // IP allowlist MUST run before security_headers so that a denied
        // admin request returns a plain 404 (not a response with headers).
        .layer(middleware::from_fn(admin_ip_guard))
        .layer(middleware::from_fn(security_headers));

    log::info!("Starting web server on http://{}", addr);
    log::info!("  /s/:id      - Share page (HTML)");
    log::info!("  /api/s/:id  - Share page (JSON)");
    log::info!("  /privacy    - Privacy Policy");
    log::info!("  /admin      - Admin Dashboard");
    log::info!("  /health     - Health check");
    log::info!("  /metrics    - Prometheus metrics (Bearer auth)");

    let allowlist_env = std::env::var("ADMIN_IP_ALLOWLIST").unwrap_or_default();
    if allowlist_env.trim().is_empty() {
        log::error!("⚠️  ADMIN_IP_ALLOWLIST is empty — admin panel is DISABLED (fails closed)");
    } else {
        log::info!("Admin IP allowlist: {}", allowlist_env);
    }

    let listener = TcpListener::bind(&addr).await?;
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await?;

    Ok(())
}
