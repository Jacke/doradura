//! WhatsApp Cloud API configuration, loaded from the environment.
//!
//! Mirrors the env-driven pattern in [`crate::core::config`]. Secrets live in
//! [`secrecy::SecretString`] so they don't leak into `Debug`/logs.

use secrecy::SecretString;

/// Graph API version the adapter targets. Bump deliberately — payload shapes
/// are stable across recent versions, but new fields land per version.
pub const DEFAULT_GRAPH_VERSION: &str = "v21.0";
/// Default Graph API host (overridable for tests pointing at a mock server).
pub const DEFAULT_GRAPH_BASE: &str = "https://graph.facebook.com";

/// Everything the adapter needs to talk to one WhatsApp Business phone number.
#[derive(Clone)]
pub struct WhatsAppConfig {
    /// Permanent / system-user access token (Bearer).
    pub access_token: SecretString,
    /// The Business phone number id sends originate from.
    pub phone_number_id: String,
    /// Shared secret echoed back during webhook subscription (`hub.verify_token`).
    pub verify_token: SecretString,
    /// Meta App secret used to verify `X-Hub-Signature-256` on inbound webhooks.
    pub app_secret: SecretString,
    /// Graph API base URL (no trailing slash), e.g. `https://graph.facebook.com`.
    pub graph_base: String,
    /// Graph API version segment, e.g. `v21.0`.
    pub graph_version: String,
}

impl std::fmt::Debug for WhatsAppConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WhatsAppConfig")
            .field("phone_number_id", &self.phone_number_id)
            .field("graph_base", &self.graph_base)
            .field("graph_version", &self.graph_version)
            .field("access_token", &"<redacted>")
            .field("verify_token", &"<redacted>")
            .field("app_secret", &"<redacted>")
            .finish()
    }
}

impl WhatsAppConfig {
    /// Build from explicit values (used by tests and callers that already hold
    /// the secrets).
    pub fn new(
        access_token: impl Into<String>,
        phone_number_id: impl Into<String>,
        verify_token: impl Into<String>,
        app_secret: impl Into<String>,
    ) -> Self {
        Self {
            access_token: SecretString::from(access_token.into()),
            phone_number_id: phone_number_id.into(),
            verify_token: SecretString::from(verify_token.into()),
            app_secret: SecretString::from(app_secret.into()),
            graph_base: DEFAULT_GRAPH_BASE.to_string(),
            graph_version: DEFAULT_GRAPH_VERSION.to_string(),
        }
    }

    /// Load from environment variables:
    /// `WHATSAPP_TOKEN`, `WHATSAPP_PHONE_ID`, `WHATSAPP_VERIFY_TOKEN`,
    /// `WHATSAPP_APP_SECRET`, and optional `WHATSAPP_GRAPH_BASE` /
    /// `WHATSAPP_GRAPH_VERSION`. Returns `None` (adapter disabled) if any
    /// required var is missing.
    pub fn from_env() -> Option<Self> {
        let access_token = std::env::var("WHATSAPP_TOKEN").ok().filter(|s| !s.is_empty())?;
        let phone_number_id = std::env::var("WHATSAPP_PHONE_ID").ok().filter(|s| !s.is_empty())?;
        let verify_token = std::env::var("WHATSAPP_VERIFY_TOKEN").ok().filter(|s| !s.is_empty())?;
        let app_secret = std::env::var("WHATSAPP_APP_SECRET").ok().filter(|s| !s.is_empty())?;
        let mut cfg = Self::new(access_token, phone_number_id, verify_token, app_secret);
        if let Ok(base) = std::env::var("WHATSAPP_GRAPH_BASE")
            && !base.is_empty()
        {
            cfg.graph_base = base.trim_end_matches('/').to_string();
        }
        if let Ok(ver) = std::env::var("WHATSAPP_GRAPH_VERSION")
            && !ver.is_empty()
        {
            cfg.graph_version = ver;
        }
        Some(cfg)
    }

    /// `{base}/{version}` prefix for Graph API calls.
    pub fn api_root(&self) -> String {
        format!("{}/{}", self.graph_base.trim_end_matches('/'), self.graph_version)
    }

    /// Endpoint to send messages from this phone number.
    pub fn messages_url(&self) -> String {
        format!("{}/{}/messages", self.api_root(), self.phone_number_id)
    }

    /// Endpoint to upload media for this phone number.
    pub fn media_upload_url(&self) -> String {
        format!("{}/{}/media", self.api_root(), self.phone_number_id)
    }

    /// Endpoint to resolve a `media_id` to a (short-lived) download URL.
    pub fn media_lookup_url(&self, media_id: &str) -> String {
        format!("{}/{}", self.api_root(), media_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::ExposeSecret;

    #[test]
    fn urls_compose_from_base_and_version() {
        let cfg = WhatsAppConfig::new("tok", "PID", "vt", "sec");
        assert_eq!(cfg.api_root(), "https://graph.facebook.com/v21.0");
        assert_eq!(cfg.messages_url(), "https://graph.facebook.com/v21.0/PID/messages");
        assert_eq!(cfg.media_upload_url(), "https://graph.facebook.com/v21.0/PID/media");
        assert_eq!(cfg.media_lookup_url("MID"), "https://graph.facebook.com/v21.0/MID");
    }

    #[test]
    fn debug_redacts_secrets() {
        let cfg = WhatsAppConfig::new("supersecret", "PID", "vt", "appsec");
        let dbg = format!("{cfg:?}");
        assert!(!dbg.contains("supersecret"));
        assert!(!dbg.contains("appsec"));
        assert!(dbg.contains("PID"));
    }

    #[test]
    fn secrets_round_trip_through_expose() {
        let cfg = WhatsAppConfig::new("tok", "PID", "vt", "sec");
        assert_eq!(cfg.access_token.expose_secret(), "tok");
        assert_eq!(cfg.verify_token.expose_secret(), "vt");
    }
}
