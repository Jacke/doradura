//! Rich messages (Bot API 10.1, `sendRichMessage`) — raw-HTTP layer.
//!
//! teloxide (pinned to a Bot API 8.x master rev) has zero knowledge of the
//! 10.1 rich-message types, so — exactly like the guest-bots path
//! (`answerGuestQuery`, alpha.29) — we POST JSON straight to the Bot API server.
//!
//! ## Why this is a probe, not a finished typed builder
//!
//! `sendRichMessage` / `InputRichMessage` shipped 2026-06-11 and the exact JSON
//! schema (block `type` discriminators, field names) is not yet in any
//! machine-readable mirror, teloxide, or a fetchable form of the official docs.
//! Rather than guess a full typed builder blind, [`send_rich_message`] takes an
//! arbitrary [`serde_json::Value`] and returns the server's **full JSON
//! response**. The admin `/richtest` command uses this to send a candidate
//! payload and surface the server's exact error/ok response, so the real schema
//! is discovered empirically against the live 10.1 server — then a typed builder
//! is written against confirmed field names.

use anyhow::{Context, Result};
use reqwest::Client;
use serde_json::{Value, json};
use std::sync::LazyLock;

use crate::core::config;

/// Shared HTTP client (connection pooling under load). Mirrors guest-bots.
static HTTP: LazyLock<Client> = LazyLock::new(|| {
    Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .expect("reqwest client builder")
});

/// Base Bot API origin: the configured server (local 10.1 in prod) or the
/// public cloud as a fallback. No trailing slash.
fn api_base() -> String {
    config::bot_api::get_url()
        .unwrap_or_else(|| "https://api.telegram.org".to_string())
        .trim_end_matches('/')
        .to_string()
}

/// POST `body` to an arbitrary Bot API `method` and return the parsed JSON
/// response (the `{ok, result}` / `{ok:false, error_code, description}` envelope).
///
/// Network/transport failures are surfaced as `Err`; a Telegram-level
/// `{ok:false, ...}` is returned as `Ok(Value)` so the caller can inspect the
/// `description` — essential for empirically discovering a new method's schema.
pub async fn call_method(method: &str, body: Value) -> Result<Value> {
    use secrecy::ExposeSecret;
    let token = config::BOT_TOKEN.expose_secret();
    let url = format!("{}/bot{}/{}", api_base(), token, method);
    let resp = HTTP
        .post(&url)
        .json(&body)
        .send()
        .await
        .with_context(|| format!("POST {method}"))?;
    let status = resp.status();
    let value: Value = resp
        .json()
        .await
        .with_context(|| format!("decode {method} response (status {status})"))?;
    Ok(value)
}

/// Send a rich message to `chat_id`. `rich_message` is the `InputRichMessage`
/// object — for sending, that is `{"markdown": "<src>"}` or `{"html": "<src>"}`
/// (schema discovered empirically against the live Bot API 10.1 server: the
/// server's `get_input_rich_message` reads a `markdown` or `html` source string
/// and tdlib renders the rich blocks; the `RichBlock`/`RichText` classes are the
/// *receiving* shape, not the input). Returns the raw server response envelope.
pub async fn send_rich_message(chat_id: i64, rich_message: Value) -> Result<Value> {
    let body = json!({
        "chat_id": chat_id,
        "rich_message": rich_message,
    });
    call_method("sendRichMessage", body).await
}

/// A showcase `InputRichMessage` (Markdown source) demonstrating the rich
/// formatting: heading, emphasis, list, blockquote, table, code, link.
pub fn demo_payload() -> Value {
    json!({ "markdown": DEMO_MARKDOWN })
}

/// Markdown showcase used by `/richtest`.
const DEMO_MARKDOWN: &str = "# 🎵 Doradura — Rich text\n\
Обычный текст: **bold**, _italic_, ~~strike~~, `code`.\n\n\
> Цитата одной строкой.\n\n\
- пункт раз\n- пункт два\n\n\
| Формат | Размер |\n|---|---|\n| MP3 | 8 MB |\n| MP4 | 24 MB |\n\n\
```\nblock code\n```\n\
[ссылка](https://t.me)";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_payload_is_markdown_source() {
        let p = demo_payload();
        // For sending, InputRichMessage is `{"markdown": <src>}` (or html).
        let md = p.get("markdown").and_then(Value::as_str).expect("markdown source");
        assert!(md.contains("# 🎵 Doradura"));
        assert!(md.contains("**bold**"));
        assert!(md.contains("| Формат | Размер |"));
        assert!(p.get("blocks").is_none());
    }

    #[test]
    fn api_base_has_no_trailing_slash() {
        // Whatever the configured origin, api_base() must not end with '/'
        // (we concatenate "/bot<token>/<method>").
        assert!(!api_base().ends_with('/'));
    }
}
