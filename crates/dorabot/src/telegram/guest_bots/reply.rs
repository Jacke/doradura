//! Raw HTTP wrappers for `answerGuestQuery` (Bot API 10.0, May 2026).
//!
//! teloxide master (rev 912b5ad2, Apr 2026) doesn't expose this method
//! yet. Rather than fork the SDK for a single experimental feature, we
//! POST JSON directly to `api.telegram.org`. The wrappers below construct
//! the four `InlineQueryResult` variants we actually use:
//!
//!   - **cached audio** — Path A/C for MP3 downloads
//!   - **cached video** — Path A/C for MP4 downloads
//!   - **article** — Path B fallback (deep-link to DM)
//!
//! When the SDK ships native support we'll swap this for the typed call
//! site without touching the lookup logic.

use anyhow::{Context, Result};
use reqwest::Client;
use serde_json::{Value, json};
use std::sync::LazyLock;

/// Shared HTTP client. Reused so connection pooling kicks in under load.
static HTTP: LazyLock<Client> = LazyLock::new(|| {
    Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .expect("reqwest client builder")
});

async fn post_answer(bot_token: &str, body: Value) -> Result<()> {
    let url = format!("https://api.telegram.org/bot{}/answerGuestQuery", bot_token);
    let resp = HTTP
        .post(&url)
        .json(&body)
        .send()
        .await
        .context("POST answerGuestQuery")?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("answerGuestQuery {} → {}", status, text);
    }
    Ok(())
}

/// Reply with an already-uploaded audio file (Path A/C for MP3).
pub async fn answer_cached_audio(
    bot_token: &str,
    query_id: &str,
    file_id: &str,
    title: Option<&str>,
    performer: Option<&str>,
) -> Result<()> {
    let body = json!({
        "guest_query_id": query_id,
        "result": {
            "type": "audio",
            "id": gen_result_id(),
            "audio_file_id": file_id,
            "title": title.unwrap_or("audio"),
            "performer": performer,
        }
    });
    post_answer(bot_token, body).await
}

/// Reply with an already-uploaded video file (Path A/C for MP4).
pub async fn answer_cached_video(bot_token: &str, query_id: &str, file_id: &str, title: Option<&str>) -> Result<()> {
    let body = json!({
        "guest_query_id": query_id,
        "result": {
            "type": "video",
            "id": gen_result_id(),
            "video_file_id": file_id,
            "title": title.unwrap_or("video"),
        }
    });
    post_answer(bot_token, body).await
}

/// Reply with an article (text + button) — Path B fallback driving the
/// caller into a DM with the bot to run the full download pipeline.
///
/// `deep_link_url` should be of the form
/// `https://t.me/{bot_username}?start={payload}` (Telegram caps payload at
/// 64 chars — see `deep_link::encode_payload`).
pub async fn answer_article_with_deeplink(
    bot_token: &str,
    query_id: &str,
    title: &str,
    description: &str,
    deep_link_url: &str,
    button_label: &str,
) -> Result<()> {
    let body = json!({
        "guest_query_id": query_id,
        "result": {
            "type": "article",
            "id": gen_result_id(),
            "title": title,
            "description": description,
            "input_message_content": {
                "message_text": format!("{}\n\n{}", title, description),
            },
            "reply_markup": {
                "inline_keyboard": [[
                    {"text": button_label, "url": deep_link_url}
                ]]
            }
        }
    });
    post_answer(bot_token, body).await
}

/// Reply with a plain text article (no button) — used for parse errors
/// like "no URL found".
pub async fn answer_article_text(bot_token: &str, query_id: &str, title: &str, description: &str) -> Result<()> {
    let body = json!({
        "guest_query_id": query_id,
        "result": {
            "type": "article",
            "id": gen_result_id(),
            "title": title,
            "description": description,
            "input_message_content": {
                "message_text": format!("{}\n\n{}", title, description),
            }
        }
    });
    post_answer(bot_token, body).await
}

/// Result IDs are required by the InlineQueryResult schema but Telegram
/// doesn't surface them anywhere user-visible — any short unique string works.
fn gen_result_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn result_ids_are_unique_per_call() {
        let a = gen_result_id();
        let b = gen_result_id();
        assert_ne!(a, b);
        assert_eq!(a.len(), 32); // UUID simple form
    }
}
