//! LLM-based download category suggestion.
//!
//! If `ANTHROPIC_API_KEY` is set in the environment and the user has at least one
//! category defined, `suggest_category` asks Claude Haiku which of the user's
//! categories best fits a new download. Returns `None` when the API key is absent,
//! the category list is empty, or the model returns an unrecognised value.

use super::llm;

/// Ask Claude Haiku to pick the best category for a download from the user's own list.
///
/// Returns `Some(name)` on a confident match, `None` otherwise.
/// Always fails silently — never panics, never propagates errors to callers.
pub async fn suggest_category(user_categories: &[String], title: &str, author: &str) -> Option<String> {
    if user_categories.is_empty() {
        return None;
    }

    let categories_list = user_categories.join(", ");
    let prompt = format!(
        "User's categories: {}. New download: '{}' by '{}'. \
         Which category fits best? Reply with EXACTLY one category name from the list, or 'none'.",
        categories_list, title, author
    );

    let text = llm::ask(llm::HAIKU, 20, &prompt).await?;

    // Match against the user's list (case-insensitive)
    user_categories
        .iter()
        .find(|cat| cat.eq_ignore_ascii_case(&text))
        .cloned()
}
