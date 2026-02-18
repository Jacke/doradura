//! Bot initialization and message routing utilities
//!
//! This module contains:
//! - Command enum definition
//! - Bot instance creation
//! - Message addressing logic (private chats, mentions, replies)

use reqwest::ClientBuilder;
use teloxide::prelude::*;
use teloxide::types::{BotCommand, ChatId, ChatKind, Message, MessageEntityKind, UserId};
use teloxide::utils::command::BotCommands;
use unic_langid::LanguageIdentifier;

use crate::core::config;
use crate::i18n;
use crate::telegram::bot_api_logger::Bot;

/// Bot commands enum with descriptions
#[derive(BotCommands, Clone, Debug)]
#[command(rename_rule = "lowercase", description = "Медиа-бот:")]
pub enum Command {
    #[command(description = "показывает главное меню")]
    Start,
    #[command(description = "настройки загрузки и конвертации")]
    Settings,
    #[command(description = "показать информацию о доступных форматах")]
    Info,
    #[command(description = "запросить суммари и субтитры через Downsub")]
    Downsub,
    #[command(description = "история загрузок")]
    History,
    #[command(description = "мои загрузки")]
    Downloads,
    #[command(description = "мои файлы")]
    Uploads,
    #[command(description = "мои вырезки")]
    Cuts,
    #[command(description = "личная статистика")]
    Stats,
    #[command(description = "экспорт истории")]
    Export,
    #[command(description = "информация о подписке и тарифах")]
    Plan,
    #[command(description = "создать бэкап БД (только для администраторов)")]
    Backup,
    #[command(description = "список всех пользователей (только для администратора)")]
    Users,
    #[command(description = "изменить план пользователя (только для администратора)")]
    Setplan,
    #[command(description = "посмотреть транзакции Stars (только для администратора)")]
    Transactions,
    #[command(description = "панель управления пользователями (только для администратора)")]
    Admin,
    #[command(description = "просмотр всех платежей (только для администратора)")]
    Charges,
    #[command(description = "скачать файл из Telegram по file_id (только для администратора)")]
    DownloadTg,
    #[command(description = "список отправленных файлов с file_id (только для администратора)")]
    SentFiles,
    #[command(description = "аналитика и метрики (только для администратора)")]
    Analytics,
    #[command(description = "состояние системы (только для администратора)")]
    Health,
    #[command(
        rename = "downsub_health",
        description = "проверка Downsub gRPC (только для администратора)"
    )]
    DownsubHealth,
    #[command(description = "детальные метрики (только для администратора)")]
    Metrics,
    #[command(description = "финансовая аналитика (только для администратора)")]
    Revenue,
    #[command(
        rename = "botapi_speed",
        description = "скорость загрузки через локальный Bot API (только для администратора)"
    )]
    BotApiSpeed,
    #[command(description = "версия и обновление (только для администратора)")]
    Version,
    #[command(description = "мои подписки на обновления")]
    Subscriptions,
}

const BOT_COMMAND_DEFINITIONS: &[(&str, &str)] = &[
    ("start", "bot_commands.start"),
    ("settings", "bot_commands.settings"),
    ("info", "bot_commands.info"),
    ("downsub", "bot_commands.downsub"),
    // ("downsub_health", "bot_commands.downsub_health"),
    ("downloads", "bot_commands.downloads"),
    ("uploads", "bot_commands.uploads"),
    ("cuts", "bot_commands.cuts"),
    //("history", "bot_commands.history"),
    //("stats", "bot_commands.stats"),
    //("global", "bot_commands.global"),
    //("export", "bot_commands.export"),
    //("backup", "bot_commands.backup"),
    ("plan", "bot_commands.plan"),
    ("subscriptions", "bot_commands.subscriptions"),
    //("users", "bot_commands.users"),
    //("setplan", "bot_commands.setplan"),
    //("transactions", "bot_commands.transactions"),
];

fn build_bot_commands(lang: &LanguageIdentifier) -> Vec<BotCommand> {
    let commands: Vec<BotCommand> = BOT_COMMAND_DEFINITIONS
        .iter()
        .map(|(command, key)| {
            let description = i18n::t(lang, key);
            BotCommand::new(*command, description)
        })
        .collect();
    log::info!("Built {} commands for language '{}'", commands.len(), lang);
    commands
}

/// Sets commands for all supported languages globally.
///
/// This makes the Telegram client automatically show command descriptions in the user's
/// Telegram interface language, without needing to set commands per-chat.
pub async fn setup_all_language_commands(bot: &Bot) -> Result<(), teloxide::RequestError> {
    for (lang_code, lang_name) in i18n::SUPPORTED_LANGS.iter() {
        let lang = i18n::lang_from_code(lang_code);
        let commands = build_bot_commands(&lang);

        let result = bot.set_my_commands(commands).language_code(*lang_code).await;

        match result {
            Ok(_) => log::info!("✓ Set commands for language: {} ({})", lang_name, lang_code),
            Err(e) => log::error!(
                "✗ Failed to set commands for language {} ({}): {}",
                lang_name,
                lang_code,
                e
            ),
        }
    }

    // Also set default commands (without language_code) for unsupported languages
    let default_lang = i18n::lang_from_code("en");
    let default_commands = build_bot_commands(&default_lang);

    match bot.set_my_commands(default_commands).await {
        Ok(_) => log::info!("✓ Set default commands (fallback)"),
        Err(e) => {
            log::error!("✗ Failed to set default commands: {}", e);
            return Err(e);
        }
    }

    log::info!("✓ Successfully set up commands for all languages");
    Ok(())
}

/// Sets commands for a specific chat and language (legacy, kept for compatibility).
///
/// Note: This is now a no-op since we set commands globally for all languages.
pub async fn setup_chat_bot_commands(
    _bot: &Bot,
    chat_id: ChatId,
    lang: &LanguageIdentifier,
) -> Result<(), teloxide::RequestError> {
    log::debug!(
        "setup_chat_bot_commands called for chat {}, lang {} (no-op, using global commands)",
        chat_id.0,
        lang
    );
    Ok(())
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
        teloxide::Bot::from_env_with_client(ClientBuilder::new().timeout(config::network::timeout()).build()?)
            .set_api_url(url)
    } else {
        teloxide::Bot::from_env_with_client(ClientBuilder::new().timeout(config::network::timeout()).build()?)
    };

    Ok(Bot::new(bot))
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
        assert!(command_list.contains("Медиа-бот"));

        // Check that some key commands are present
        assert!(command_list.contains("start"));
        assert!(command_list.contains("info"));
        assert!(command_list.contains("history"));
    }
}
