//! LLM-based download category suggestion.
//!
//! If `ANTHROPIC_API_KEY` is set in the environment and the user has at least one
//! category defined, `suggest_category` asks Claude Haiku which of the user's
//! categories best fits a new download. Returns `None` when the API key is absent,
//! the category list is empty, or the model returns an unrecognised value.

use serde_json::json;

/// Ask Claude Haiku to pick the best category for a download from the user's own list.
///
/// Returns `Some(name)` on a confident match, `None` otherwise.
/// Always fails silently — never panics, never propagates errors to callers.
pub async fn suggest_category(user_categories: &[String], title: &str, author: &str) -> Option<String> {
    if user_categories.is_empty() {
        return None;
    }

    let api_key = std::env::var("ANTHROPIC_API_KEY").ok()?;

    let categories_list = user_categories.join(", ");
    let prompt = format!(
        "User's categories: {}. New download: '{}' by '{}'. \
         Which category fits best? Reply with EXACTLY one category name from the list, or 'none'.",
        categories_list, title, author
    );

    let body = json!({
        "model": "claude-haiku-4-5-20251001",
        "max_tokens": 20,
        "messages": [{"role": "user", "content": prompt}]
    });

    let client = reqwest::Client::new();
    let response = client
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", &api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
        .ok()?;

    if !response.status().is_success() {
        log::warn!("categorizer: API returned status {}", response.status());
        return None;
    }

    let json: serde_json::Value = response.json().await.ok()?;
    let text = json.get("content")?.get(0)?.get("text")?.as_str()?.trim().to_string();

    // Match against the user's list (case-insensitive)
    user_categories
        .iter()
        .find(|cat| cat.eq_ignore_ascii_case(&text))
        .cloned()
}
