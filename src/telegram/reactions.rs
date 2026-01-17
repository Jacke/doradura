use crate::telegram::Bot;
use teloxide::prelude::*;
use teloxide::types::{MessageId, ReactionType};

/// Allowed emoji for bot reactions per Telegram Bot API.
/// List current as of January 2025.
/// Full list: https://core.telegram.org/bots/api#reactiontypeemoji
pub mod emoji {
    /// "Eyes" reaction - indicates bot has seen/is processing the message
    pub const EYES: &str = "👀";
    /// "Thumbs up" reaction - for successful completion
    pub const THUMBS_UP: &str = "👍";
    /// "Party" reaction - for successful completion (alternative)
    pub const PARTY: &str = "🎉";
    /// "Fire" reaction - for active process indication
    pub const FIRE: &str = "🔥";
    /// "Lightning" reaction - for fast operations
    pub const ZAP: &str = "⚡";
    /// "Heart" reaction - for favorites/likes
    pub const HEART: &str = "❤️";
    /// "Thinking" reaction - for processing/thinking
    pub const THINKING: &str = "🤔";
    /// "Sad" reaction - for failures
    pub const SAD: &str = "😢";
    /// "Developer" reaction - for technical operations
    pub const DEVELOPER: &str = "👨‍💻";
}

/// Try to set a reaction, skipping invalid/unavailable reactions for the chat.
pub async fn try_set_reaction(bot: &Bot, chat_id: ChatId, message_id: MessageId, emoji: &str) {
    let mut chosen = emoji.to_string();
    if let Ok(chat) = bot.get_chat(chat_id).await {
        if let Some(available) = chat.available_reactions() {
            let allowed = available
                .iter()
                .any(|reaction| matches!(reaction, ReactionType::Emoji { emoji: allowed } if allowed == emoji));
            if !allowed {
                if let Some(first) = available.iter().find_map(|reaction| match reaction {
                    ReactionType::Emoji { emoji } => Some(emoji.clone()),
                    _ => None,
                }) {
                    log::debug!(
                        "Reaction '{}' not allowed in chat {}, falling back to '{}'",
                        emoji,
                        chat_id.0,
                        first
                    );
                    chosen = first;
                } else {
                    log::debug!("No emoji reactions available in chat {}, skipping", chat_id.0);
                    return;
                }
            }
        }
    }

    let reaction = vec![ReactionType::Emoji { emoji: chosen }];
    if let Err(e) = bot.set_message_reaction(chat_id, message_id).reaction(reaction).await {
        let error_text = e.to_string();
        if error_text.contains("REACTION_INVALID") {
            log::debug!(
                "Reaction '{}' rejected by Telegram for chat {}: {}",
                emoji,
                chat_id.0,
                error_text
            );
        } else {
            log::warn!(
                "Failed to set reaction '{}' for chat {}: {}",
                emoji,
                chat_id.0,
                error_text
            );
        }
    }
}
