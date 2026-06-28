//! Thin HTTP client over the WhatsApp Cloud (Graph) API.
//!
//! Holds a [`reqwest::Client`] + [`WhatsAppConfig`] and exposes the three calls
//! the adapter needs: send a prebuilt message payload, upload a local file to
//! get a reusable `media_id`, and download an inbound media by id. All payload
//! construction lives in [`super::wire`]; this module only does I/O.

use anyhow::{Context, anyhow, bail};
use futures_util::StreamExt;
use reqwest::multipart;
use secrecy::ExposeSecret;
use serde_json::Value;
use tokio::io::AsyncWriteExt;

use super::config::WhatsAppConfig;

/// Authenticated client for one WhatsApp Business phone number.
#[derive(Clone)]
pub struct WhatsAppClient {
    http: reqwest::Client,
    cfg: WhatsAppConfig,
}

impl WhatsAppClient {
    /// Build with a fresh internal [`reqwest::Client`].
    pub fn new(cfg: WhatsAppConfig) -> Self {
        Self {
            http: reqwest::Client::new(),
            cfg,
        }
    }

    /// Build reusing an existing [`reqwest::Client`] (share the connection pool
    /// with the rest of the service).
    pub fn with_http(http: reqwest::Client, cfg: WhatsAppConfig) -> Self {
        Self { http, cfg }
    }

    /// The config this client serves.
    pub fn config(&self) -> &WhatsAppConfig {
        &self.cfg
    }

    /// POST a prebuilt message payload to `/{phone_id}/messages`. Returns the
    /// sent message id (`wamid…`) from `messages[0].id`.
    pub async fn send_message(&self, payload: &Value) -> anyhow::Result<String> {
        let resp = self
            .http
            .post(self.cfg.messages_url())
            .bearer_auth(self.cfg.access_token.expose_secret())
            .json(payload)
            .send()
            .await
            .context("whatsapp send request failed")?;

        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            bail!("whatsapp send returned {status}: {body}");
        }
        let v: Value = serde_json::from_str(&body).context("whatsapp send: bad JSON response")?;
        v.get("messages")
            .and_then(|m| m.get(0))
            .and_then(|m| m.get("id"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| anyhow!("whatsapp send: no message id in response: {body}"))
    }

    /// Upload a local file and return its reusable `media_id`. `mime` must be a
    /// type WhatsApp accepts for the intended message kind.
    pub async fn upload_media(&self, path: &str, mime: &str) -> anyhow::Result<String> {
        let bytes = tokio::fs::read(path)
            .await
            .with_context(|| format!("whatsapp upload: read {path}"))?;
        let file_name = std::path::Path::new(path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file")
            .to_string();
        let part = multipart::Part::bytes(bytes)
            .file_name(file_name)
            .mime_str(mime)
            .context("whatsapp upload: invalid mime")?;
        let form = multipart::Form::new()
            .text("messaging_product", "whatsapp")
            .text("type", mime.to_string())
            .part("file", part);

        let resp = self
            .http
            .post(self.cfg.media_upload_url())
            .bearer_auth(self.cfg.access_token.expose_secret())
            .multipart(form)
            .send()
            .await
            .context("whatsapp upload request failed")?;
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            bail!("whatsapp upload returned {status}: {body}");
        }
        let v: Value = serde_json::from_str(&body).context("whatsapp upload: bad JSON")?;
        v.get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| anyhow!("whatsapp upload: no media id: {body}"))
    }

    /// Resolve a `media_id` to its short-lived signed download URL.
    pub async fn resolve_media_url(&self, media_id: &str) -> anyhow::Result<String> {
        let resp = self
            .http
            .get(self.cfg.media_lookup_url(media_id))
            .bearer_auth(self.cfg.access_token.expose_secret())
            .send()
            .await
            .context("whatsapp media lookup failed")?;
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            bail!("whatsapp media lookup returned {status}: {body}");
        }
        let v: Value = serde_json::from_str(&body).context("whatsapp media lookup: bad JSON")?;
        v.get("url")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| anyhow!("whatsapp media lookup: no url: {body}"))
    }

    /// Download an inbound media by id, streaming to `dest_path`. The signed URL
    /// still requires the Bearer token.
    pub async fn download_media(&self, media_id: &str, dest_path: &str) -> anyhow::Result<()> {
        let url = self.resolve_media_url(media_id).await?;
        let resp = self
            .http
            .get(&url)
            .bearer_auth(self.cfg.access_token.expose_secret())
            .send()
            .await
            .context("whatsapp media download failed")?;
        if !resp.status().is_success() {
            bail!("whatsapp media download returned {}", resp.status());
        }
        let mut file = tokio::fs::File::create(dest_path)
            .await
            .with_context(|| format!("whatsapp download: create {dest_path}"))?;
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.context("whatsapp download: stream error")?;
            file.write_all(&chunk).await.context("whatsapp download: write")?;
        }
        file.flush().await.context("whatsapp download: flush")?;
        Ok(())
    }
}
