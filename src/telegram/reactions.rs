use teloxide::prelude::*;
use teloxide::types::{MessageId, ReactionType};

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
