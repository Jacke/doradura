//! Shared LLM client for Anthropic Claude API.
//!
//! Provides a thin, reusable wrapper around the Messages API.
//! Used by categorizer, lyrics highlights, and any future LLM features.

use serde_json::json;

/// Send a prompt to Claude and return the text response.
///
/// Returns `None` if `ANTHROPIC_API_KEY` is not set, the API call fails,
/// or the response cannot be parsed. Never panics.
pub async fn ask(model: &str, max_tokens: u32, prompt: &str) -> Option<String> {
    let api_key = std::env::var("ANTHROPIC_API_KEY").ok()?;

    let body = json!({
        "model": model,
        "max_tokens": max_tokens,
        "messages": [{"role": "user", "content": prompt}]
    });

    let client = reqwest::Client::new();
    let response = client
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", &api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .timeout(std::time::Duration::from_secs(30))
        .json(&body)
        .send()
        .await
        .ok()?;

    if !response.status().is_success() {
        log::warn!("llm::ask: API returned status {}", response.status());
        return None;
    }

    let json: serde_json::Value = response.json().await.ok()?;
    let text = json.get("content")?.get(0)?.get("text")?.as_str()?.trim().to_string();

    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

/// Claude Haiku model ID — cheapest, fastest.
pub const HAIKU: &str = "claude-haiku-4-5-20251001";
