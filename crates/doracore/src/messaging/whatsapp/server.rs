//! Axum webhook router for inbound WhatsApp traffic.
//!
//! Wiring: `GET` performs the Meta subscription handshake; `POST` verifies the
//! `X-Hub-Signature-256` HMAC over the **raw** body, normalizes the payload to
//! neutral [`InboundMessage`]s, and forwards each over an `mpsc` channel. The
//! service (`dorabot`) owns the receiver and routes events into the existing
//! dispatcher — keeping this module free of any bot/teloxide knowledge.
//!
//! Mount it under the platform path, e.g.
//! `app.nest("/whatsapp/webhook", whatsapp::server::router(cfg, tx))`.

use std::sync::Arc;

use axum::{
    Router,
    body::Bytes,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::get,
};
use serde::Deserialize;
use tokio::sync::mpsc::UnboundedSender;

use crate::messaging::InboundSource;
use crate::messaging::types::InboundMessage;

use super::config::WhatsAppConfig;
use super::inbound::WhatsAppInbound;
use super::webhook;

/// Shared state for the webhook routes.
#[derive(Clone)]
pub struct WebhookState {
    cfg: Arc<WhatsAppConfig>,
    inbound: WhatsAppInbound,
    tx: UnboundedSender<InboundMessage>,
}

impl WebhookState {
    pub fn new(cfg: Arc<WhatsAppConfig>, tx: UnboundedSender<InboundMessage>) -> Self {
        Self {
            cfg,
            inbound: WhatsAppInbound::new(),
            tx,
        }
    }
}

/// Build the router. Mount at the platform webhook path; normalized inbound
/// messages are delivered on `tx`.
pub fn router(cfg: Arc<WhatsAppConfig>, tx: UnboundedSender<InboundMessage>) -> Router {
    Router::new()
        .route("/", get(verify).post(receive))
        .with_state(WebhookState::new(cfg, tx))
}

/// `hub.*` subscription handshake params.
#[derive(Debug, Deserialize)]
struct VerifyQuery {
    #[serde(rename = "hub.mode")]
    mode: Option<String>,
    #[serde(rename = "hub.verify_token")]
    verify_token: Option<String>,
    #[serde(rename = "hub.challenge")]
    challenge: Option<String>,
}

/// `GET` — echo `hub.challenge` iff `hub.verify_token` matches.
async fn verify(State(state): State<WebhookState>, Query(q): Query<VerifyQuery>) -> impl IntoResponse {
    let (mode, token, challenge) = (
        q.mode.unwrap_or_default(),
        q.verify_token.unwrap_or_default(),
        q.challenge.unwrap_or_default(),
    );
    match webhook::verify_subscription_with(&state.cfg, &mode, &token, &challenge) {
        Some(c) => (StatusCode::OK, c),
        None => (StatusCode::FORBIDDEN, "verification failed".to_string()),
    }
}

/// `POST` — verify signature, normalize, forward events. Always returns `200`
/// on a valid signature (even with zero routable messages) so Meta doesn't
/// retry; `401` when the signature is missing/invalid.
async fn receive(State(state): State<WebhookState>, headers: HeaderMap, body: Bytes) -> impl IntoResponse {
    let sig = headers
        .get("x-hub-signature-256")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();

    if !webhook::verify_signature_with(&state.cfg, &body, sig) {
        tracing::warn!("whatsapp webhook: invalid X-Hub-Signature-256");
        return StatusCode::UNAUTHORIZED;
    }

    let Ok(json) = serde_json::from_slice::<serde_json::Value>(&body) else {
        tracing::warn!("whatsapp webhook: body is not JSON");
        // Signature was valid; ack so Meta won't retry a malformed delivery.
        return StatusCode::OK;
    };

    for msg in state.inbound.normalize(&json) {
        if state.tx.send(msg).is_err() {
            tracing::error!("whatsapp webhook: inbound receiver dropped");
            break;
        }
    }
    StatusCode::OK
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messaging::types::{InboundEvent, Platform};
    use serde_json::json;

    async fn spawn(cfg: WhatsAppConfig) -> (String, tokio::sync::mpsc::UnboundedReceiver<InboundMessage>) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let app = router(Arc::new(cfg), tx);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        (format!("http://{addr}"), rx)
    }

    #[tokio::test]
    async fn get_handshake_echoes_challenge() {
        let cfg = WhatsAppConfig::new("tok", "PID", "myverify", "sec");
        let (base, _rx) = spawn(cfg).await;
        let url = format!("{base}/?hub.mode=subscribe&hub.verify_token=myverify&hub.challenge=42ABC");
        let resp = reqwest::get(&url).await.unwrap();
        assert_eq!(resp.status(), 200);
        assert_eq!(resp.text().await.unwrap(), "42ABC");
    }

    #[tokio::test]
    async fn get_handshake_rejects_bad_token() {
        let cfg = WhatsAppConfig::new("tok", "PID", "myverify", "sec");
        let (base, _rx) = spawn(cfg).await;
        let url = format!("{base}/?hub.mode=subscribe&hub.verify_token=WRONG&hub.challenge=x");
        let resp = reqwest::get(&url).await.unwrap();
        assert_eq!(resp.status(), 403);
    }

    #[tokio::test]
    async fn post_valid_signature_delivers_event() {
        let cfg = WhatsAppConfig::new("tok", "PID", "vt", "appsecret");
        let (base, mut rx) = spawn(cfg).await;
        let payload = json!({
            "object": "whatsapp_business_account",
            "entry": [{ "changes": [{ "value": {
                "messages": [{ "from": "49151", "type": "text", "text": { "body": "hi" } }]
            }}]}]
        });
        let raw = serde_json::to_vec(&payload).unwrap();
        let sig = webhook::sign_body("appsecret", &raw);

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("{base}/"))
            .header("x-hub-signature-256", sig)
            .header("content-type", "application/json")
            .body(raw)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        let msg = rx.recv().await.expect("event delivered");
        assert_eq!(msg.chat.platform, Platform::WhatsApp);
        assert_eq!(msg.event, InboundEvent::Text { body: "hi".into() });
    }

    #[tokio::test]
    async fn post_bad_signature_is_unauthorized() {
        let cfg = WhatsAppConfig::new("tok", "PID", "vt", "appsecret");
        let (base, mut rx) = spawn(cfg).await;
        let raw = br#"{"object":"x"}"#.to_vec();
        let client = reqwest::Client::new();
        let resp = client
            .post(format!("{base}/"))
            .header("x-hub-signature-256", "sha256=deadbeef")
            .body(raw)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 401);
        // nothing delivered
        assert!(rx.try_recv().is_err());
    }
}
