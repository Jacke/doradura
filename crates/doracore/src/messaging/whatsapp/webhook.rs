//! WhatsApp webhook security: subscription handshake + payload signature.
//!
//! Meta verifies a webhook in two ways:
//! 1. **Subscription** — a one-time `GET` with `hub.mode`, `hub.verify_token`,
//!    `hub.challenge`. We echo the challenge iff the token matches.
//! 2. **Per-delivery** — every `POST` carries `X-Hub-Signature-256:
//!    sha256=<hex>`, an HMAC-SHA256 of the **raw** request body keyed by the
//!    Meta App secret. We recompute and compare in constant time.
//!
//! Both helpers are pure (no axum types) so they're trivially testable; the
//! axum route wiring lives in `dorabot`.

use hmac::{Hmac, Mac};
use secrecy::ExposeSecret;
use sha2::Sha256;

use super::config::WhatsAppConfig;

type HmacSha256 = Hmac<Sha256>;

/// Result of the `GET` subscription handshake: echo `Some(challenge)` back with
/// `200`, or `None` → reply `403`.
pub fn verify_subscription(mode: &str, token: &str, challenge: &str, expected_token: &str) -> Option<String> {
    if mode == "subscribe" && constant_time_eq(token.as_bytes(), expected_token.as_bytes()) {
        Some(challenge.to_string())
    } else {
        None
    }
}

/// Convenience over [`verify_subscription`] using the configured verify token.
pub fn verify_subscription_with(cfg: &WhatsAppConfig, mode: &str, token: &str, challenge: &str) -> Option<String> {
    verify_subscription(mode, token, challenge, cfg.verify_token.expose_secret())
}

/// Verify the `X-Hub-Signature-256` header against the raw request body.
///
/// `header` is the full header value (`sha256=<hex>`); `app_secret` is the Meta
/// App secret; `raw_body` is the **exact** bytes received (do not re-serialize —
/// any whitespace change breaks the HMAC). Returns `true` iff valid.
pub fn verify_signature(app_secret: &str, raw_body: &[u8], header: &str) -> bool {
    let Some(hex_sig) = header.strip_prefix("sha256=") else {
        return false;
    };
    let Ok(provided) = hex::decode(hex_sig) else {
        return false;
    };
    let Ok(mut mac) = HmacSha256::new_from_slice(app_secret.as_bytes()) else {
        return false;
    };
    mac.update(raw_body);
    mac.verify_slice(&provided).is_ok()
}

/// Convenience over [`verify_signature`] using the configured app secret.
pub fn verify_signature_with(cfg: &WhatsAppConfig, raw_body: &[u8], header: &str) -> bool {
    verify_signature(cfg.app_secret.expose_secret(), raw_body, header)
}

/// Compute the `sha256=<hex>` signature for a body (used by tests and any
/// outbound webhook we might sign).
pub fn sign_body(app_secret: &str, raw_body: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(app_secret.as_bytes()).expect("HMAC accepts any key length");
    mac.update(raw_body);
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

/// Constant-time byte comparison (avoid timing oracles on the verify token).
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subscription_echoes_challenge_on_match() {
        assert_eq!(
            verify_subscription("subscribe", "tok", "CHALLENGE", "tok"),
            Some("CHALLENGE".to_string())
        );
    }

    #[test]
    fn subscription_rejects_wrong_token_or_mode() {
        assert_eq!(verify_subscription("subscribe", "bad", "C", "tok"), None);
        assert_eq!(verify_subscription("unsubscribe", "tok", "C", "tok"), None);
    }

    #[test]
    fn signature_round_trips() {
        let secret = "appsecret";
        let body = br#"{"object":"whatsapp_business_account"}"#;
        let header = sign_body(secret, body);
        assert!(header.starts_with("sha256="));
        assert!(verify_signature(secret, body, &header));
    }

    #[test]
    fn signature_rejects_tampered_body() {
        let secret = "appsecret";
        let header = sign_body(secret, b"original");
        assert!(!verify_signature(secret, b"tampered", &header));
    }

    #[test]
    fn signature_rejects_wrong_secret() {
        let header = sign_body("secretA", b"body");
        assert!(!verify_signature("secretB", b"body", &header));
    }

    #[test]
    fn signature_rejects_malformed_header() {
        assert!(!verify_signature("s", b"body", "deadbeef"));
        assert!(!verify_signature("s", b"body", "sha256=not-hex-zz"));
    }

    #[test]
    fn known_vector() {
        // HMAC-SHA256(key="key", msg="The quick brown fox jumps over the lazy dog")
        // = f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8
        let sig = sign_body("key", b"The quick brown fox jumps over the lazy dog");
        assert_eq!(
            sig,
            "sha256=f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8"
        );
    }
}
