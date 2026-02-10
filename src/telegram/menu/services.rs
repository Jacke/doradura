use crate::core::escape_markdown;
use crate::extension::{ExtensionCategory, ExtensionRegistry};
use crate::i18n;
use crate::telegram::Bot;
use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup, MessageId};
use unic_langid::LanguageIdentifier;

use super::helpers::edit_caption_or_text;

/// Shows the extensions menu with dynamic cards from the ExtensionRegistry.
pub async fn show_services_menu(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    lang: &LanguageIdentifier,
    registry: &ExtensionRegistry,
) -> ResponseResult<()> {
    let mut text = i18n::t(lang, "extensions.header");
    text.push_str("\n\n");

    let categories = [
        (ExtensionCategory::Downloader, "extensions.category_download"),
        (ExtensionCategory::Converter, "extensions.category_convert"),
        (ExtensionCategory::AudioProcessor, "extensions.category_process"),
    ];

    let status_active = i18n::t(lang, "extensions.status_active");
    let status_unavailable = i18n::t(lang, "extensions.status_unavailable");

    let mut buttons: Vec<Vec<InlineKeyboardButton>> = Vec::new();

    for (category, category_key) in &categories {
        let exts = registry.by_category(*category);
        if exts.is_empty() {
            continue;
        }

        text.push_str(&i18n::t(lang, category_key));
        text.push_str("\n\n");

        for ext in &exts {
            let name = ext.localized_name(lang);
            let desc = ext.localized_description(lang);
            let status = if ext.is_available() {
                &status_active
            } else {
                &status_unavailable
            };

            text.push_str(&format!("{} *{}*\n└ {} \\| {}\n\n", ext.icon(), name, desc, status));

            buttons.push(vec![InlineKeyboardButton::callback(
                format!("{} {}", ext.icon(), name),
                format!("ext:detail:{}", ext.id()),
            )]);
        }
    }

    text.push_str(&i18n::t(lang, "extensions.footer"));

    buttons.push(vec![InlineKeyboardButton::callback(
        i18n::t(lang, "common.back"),
        "back:enhanced_main",
    )]);

    let keyboard = InlineKeyboardMarkup::new(buttons);

    edit_caption_or_text(bot, chat_id, message_id, text, Some(keyboard)).await?;
    Ok(())
}

/// Shows detailed info about a specific extension.
pub(crate) async fn show_extension_detail(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    lang: &LanguageIdentifier,
    registry: &ExtensionRegistry,
    ext_id: &str,
) -> ResponseResult<()> {
    let Some(ext) = registry.get(ext_id) else {
        return Ok(());
    };

    let name = ext.localized_name(lang);
    let desc = ext.localized_description(lang);
    let status = if ext.is_available() {
        i18n::t(lang, "extensions.status_active")
    } else {
        i18n::t(lang, "extensions.status_unavailable")
    };

    let mut text = format!("{} *{}*\n\n{}\n\n{}\n\n", ext.icon(), name, desc, status);

    let caps = ext.capabilities();
    if !caps.is_empty() {
        for cap in &caps {
            text.push_str(&format!(
                "• *{}* — {}\n",
                escape_markdown(&cap.name),
                escape_markdown(&cap.description)
            ));
        }
    }

    let keyboard = InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::callback(
        i18n::t(lang, "extensions.detail_back"),
        "ext:back",
    )]]);

    edit_caption_or_text(bot, chat_id, message_id, text, Some(keyboard)).await?;
    Ok(())
}
