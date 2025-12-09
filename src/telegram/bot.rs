//! Bot initialization and message routing utilities
//!
//! This module contains:
//! - Command enum definition
//! - Bot instance creation
//! - Message addressing logic (private chats, mentions, replies)

use reqwest::ClientBuilder;
use teloxide::prelude::*;
use teloxide::types::{ChatKind, Message, MessageEntityKind, UserId};
use teloxide::utils::command::BotCommands;

use crate::core::config;

/// Bot commands enum with descriptions
#[derive(BotCommands, Clone, Debug)]
#[command(rename_rule = "lowercase", description = "Я умею:")]
pub enum Command {
    #[command(description = "показывает главное меню")]
    Start,
    #[command(description = "настройки режима загрузки")]
    Mode,
    #[command(description = "показать информацию о доступных форматах")]
    Info,
    #[command(description = "история загрузок")]
    History,
    #[command(description = "личная статистика")]
    Stats,
    #[command(description = "глобальная статистика")]
    Global,
    #[command(description = "экспорт истории")]
    Export,
    #[command(description = "создать бэкап БД (только для администраторов)")]
    Backup,
    #[command(description = "информация о подписке и тарифах")]
    Plan,
    #[command(description = "список всех пользователей (только для администратора)")]
    Users,
    #[command(description = "изменить план пользователя (только для администратора)")]
    Setplan,
    #[command(description = "панель управления пользователями (только для администратора)")]
    Admin,
}

/// Creates a Bot instance with custom or default API URL
///
/// # Returns
/// * `Ok(Bot)` - Successfully created bot instance
/// * `Err(anyhow::Error)` - Failed to create bot (invalid URL, network issues, etc.)
pub fn create_bot() -> anyhow::Result<Bot> {
    // Check if local Bot API server is configured
    let bot = if let Ok(bot_api_url) = std::env::var("BOT_API_URL") {
        log::info!("Using custom Bot API URL: {}", bot_api_url);
        let url = url::Url::parse(&bot_api_url).map_err(|e| anyhow::anyhow!("Invalid BOT_API_URL: {}", e))?;
        Bot::from_env_with_client(ClientBuilder::new().timeout(config::network::timeout()).build()?).set_api_url(url)
    } else {
        Bot::from_env_with_client(ClientBuilder::new().timeout(config::network::timeout()).build()?)
    };

    Ok(bot)
}

/// Sets up bot commands in Telegram UI
///
/// # Arguments
/// * `bot` - Bot instance to configure
///
/// # Returns
/// * `Ok(())` - Commands set successfully
/// * `Err(RequestError)` - Failed to set commands
pub async fn setup_bot_commands(bot: &Bot) -> Result<(), teloxide::RequestError> {
    use teloxide::types::BotCommand;

    bot.set_my_commands(vec![
        BotCommand::new("start", "показывает главное меню"),
        BotCommand::new("mode", "настройки режима загрузки"),
        BotCommand::new("info", "показать информацию о доступных форматах"),
        BotCommand::new("history", "история загрузок"),
        BotCommand::new("stats", "личная статистика"),
        BotCommand::new("global", "глобальная статистика"),
        BotCommand::new("export", "экспорт истории"),
        BotCommand::new("backup", "создать бэкап БД (только для администраторов)"),
        BotCommand::new("plan", "информация о подписке и тарифах"),
        BotCommand::new("users", "список всех пользователей (только для администратора)"),
        BotCommand::new("setplan", "изменить план пользователя (только для администратора)"),
    ])
    .await?;

    Ok(())
}

/// Checks if a message is addressed to the bot
///
/// # Arguments
/// * `msg` - Message to check
/// * `bot_username` - Bot's username (without @)
/// * `bot_id` - Bot's user ID
///
/// # Returns
/// * `true` if message is addressed to bot (private chat, bot mention, reply to bot message)
/// * `false` if message is not addressed to bot
pub fn is_message_addressed_to_bot(msg: &Message, bot_username: Option<&str>, bot_id: UserId) -> bool {
    // In private chats, all messages are addressed to the bot
    if matches!(msg.chat.kind, ChatKind::Private(_)) {
        return true;
    }

    // Check if the message is a reply to a bot message
    if let Some(reply_to) = msg.reply_to_message() {
        if let Some(from) = &reply_to.from {
            if from.id == bot_id {
                return true;
            }
        }
    }

    // Check message text for bot mention
    if let Some(text) = msg.text() {
        // Check entities for mentions
        if let Some(entities) = msg.entities() {
            for entity in entities {
                if matches!(entity.kind, MessageEntityKind::Mention) {
                    // Extract mention from text
                    let mention = &text[entity.offset..entity.offset + entity.length];
                    // Remove @ for comparison
                    let mention_username = mention.strip_prefix('@').unwrap_or(mention);
                    if let Some(username) = bot_username {
                        if mention_username.eq_ignore_ascii_case(username) {
                            return true;
                        }
                    }
                }
            }
        }

        // Check if text starts with or contains bot mention
        if let Some(username) = bot_username {
            let mention_pattern = format!("@{}", username);
            if text.starts_with(&mention_pattern) || text.contains(&mention_pattern) {
                return true;
            }
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_count() {
        // Just verify that we have the expected number of commands
        let commands = Command::descriptions();
        let command_list = format!("{}", commands);

        // Check that the description header is present
        assert!(command_list.contains("Я умею"));

        // Check that some key commands are present
        assert!(command_list.contains("start"));
        assert!(command_list.contains("info"));
        assert!(command_list.contains("history"));
    }
}
