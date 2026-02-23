//! HTTP server for exposing Prometheus metrics
//!
//! This module provides a simple HTTP server that exposes metrics for Prometheus scraping.
//! It runs on a separate port (configurable via METRICS_PORT env var, default 9090).

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use prometheus::{Encoder, TextEncoder};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::time::Instant;

/// Application state for the metrics server
#[derive(Clone)]
struct AppState {
    start_time: Instant,
}

/// Start the metrics HTTP server
///
/// This server exposes two endpoints:
/// - /metrics - Prometheus metrics in text format
/// - /health - Health check endpoint
///
/// # Arguments
/// * `port` - Port to listen on (typically 9090)
pub async fn start_metrics_server(port: u16) -> Result<(), Box<dyn std::error::Error>> {
    let addr = SocketAddr::from(([0, 0, 0, 0], port));

    let state = AppState {
        start_time: Instant::now(),
    };

    let app = Router::new()
        .route("/metrics", get(metrics_handler))
        .route("/health", get(health_handler))
        .route("/ready", get(ready_handler))
        .route("/", get(root_handler))
        .with_state(Arc::new(state));

    log::info!("Starting metrics server on http://{}", addr);
    log::info!("  /metrics - Prometheus metrics");
    log::info!("  /health  - Health check (liveness)");
    log::info!("  /ready   - Readiness check (K8s)");

    let listener = TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

/// Handler for /metrics endpoint
///
/// Returns Prometheus metrics in text exposition format
async fn metrics_handler() -> Response {
    let encoder = TextEncoder::new();
    let metric_families = prometheus::gather();

    let mut buffer = Vec::new();
    match encoder.encode(&metric_families, &mut buffer) {
        Ok(_) => {
            // Successfully encoded metrics
            Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", encoder.format_type())
                .body(buffer.into())
                .unwrap()
        }
        Err(e) => {
            log::error!("Failed to encode metrics: {}", e);
            Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(format!("Failed to encode metrics: {}", e).into())
                .unwrap()
        }
    }
}

/// Handler for /health endpoint
///
/// Returns a simple health check response with uptime
async fn health_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let uptime = state.start_time.elapsed();
    let uptime_secs = uptime.as_secs();

    let health_status = serde_json::json!({
        "status": "healthy",
        "uptime_seconds": uptime_secs,
        "uptime_human": format_duration(uptime),
        "service": "doradura-bot",
        "version": env!("CARGO_PKG_VERSION"),
    });

    (StatusCode::OK, axum::Json(health_status))
}

/// Handler for /ready endpoint (Kubernetes readiness probe)
///
/// Returns 200 if the service is ready to accept traffic (more thorough than /health)
/// Checks: basic availability
async fn ready_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let uptime = state.start_time.elapsed();

    // Service is ready if it's been running for at least 5 seconds
    // (allows time for initialization)
    if uptime.as_secs() < 5 {
        let status = serde_json::json!({
            "status": "starting",
            "uptime_seconds": uptime.as_secs(),
            "message": "Service is still initializing"
        });
        return (StatusCode::SERVICE_UNAVAILABLE, axum::Json(status));
    }

    let status = serde_json::json!({
        "status": "ready",
        "uptime_seconds": uptime.as_secs(),
    });

    (StatusCode::OK, axum::Json(status))
}

/// Handler for root endpoint
///
/// Provides basic information about available endpoints
async fn root_handler() -> impl IntoResponse {
    let info = r#"{
  "service": "doradura-bot-metrics",
  "version": "0.1.0",
  "endpoints": {
    "/metrics": "Prometheus metrics (text format)",
    "/health": "Health check (JSON)",
    "/ready": "Readiness check for K8s (JSON)",
    "/": "This information page"
  }
}"#;

    (StatusCode::OK, [("Content-Type", "application/json")], info)
}

/// Format duration as human-readable string
fn format_duration(duration: Duration) -> String {
    let total_secs = duration.as_secs();
    let days = total_secs / 86400;
    let hours = (total_secs % 86400) / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;

    if days > 0 {
        format!("{}d {}h {}m {}s", days, hours, minutes, seconds)
    } else if hours > 0 {
        format!("{}h {}m {}s", hours, minutes, seconds)
    } else if minutes > 0 {
        format!("{}m {}s", minutes, seconds)
    } else {
        format!("{}s", seconds)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_duration() {
        assert_eq!(format_duration(Duration::from_secs(30)), "30s");
        assert_eq!(format_duration(Duration::from_secs(90)), "1m 30s");
        assert_eq!(format_duration(Duration::from_secs(3665)), "1h 1m 5s");
        assert_eq!(format_duration(Duration::from_secs(90061)), "1d 1h 1m 1s");
    }
}
