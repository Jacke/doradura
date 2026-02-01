//! Copyright/branding module for downloaded content
//!
//! Adds a Dora the Explorer themed signature to media captions.
//! "We did it! ¡Lo hicimos!" - Dora

use once_cell::sync::Lazy;
use rand::Rng;
use std::env;
use std::sync::OnceLock;

use super::utils::escape_markdown_v2;

/// Global bot username, set at startup from getMe()
static BOT_USERNAME: OnceLock<String> = OnceLock::new();

/// Set the bot username (called once at startup from main.rs)
pub fn set_bot_username(username: &str) {
    let tag = if username.starts_with('@') {
        username.to_string()
    } else {
        format!("@{}", username)
    };
    let _ = BOT_USERNAME.set(tag);
}

/// Get the bot tag for branding
/// Priority: 1) Username from getMe(), 2) BOT_TAG env var, 3) Default
fn get_bot_tag() -> String {
    if let Some(username) = BOT_USERNAME.get() {
        return username.clone();
    }

    // Fallback to env var or default
    env::var("BOT_TAG").unwrap_or_else(|_| "@SaveAsBot".to_string())
}

/// Enable copyright/branding in captions
/// Read from COPYRIGHT_ENABLED environment variable
/// Default: true
pub static COPYRIGHT_ENABLED: Lazy<bool> = Lazy::new(|| {
    env::var("COPYRIGHT_ENABLED")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(true)
});

/// Dora the Explorer themed sign-off messages
/// These are inspired by Dora's famous catchphrases
const DORA_MESSAGES: &[&str] = &[
    // Classic Dora
    "We did it! ¡Lo hicimos!",
    "¡Vámonos! Let's go!",
    "We couldn't have done it without you!",
    // Map references
    "I'm the Map, I'm the Map!",
    "If there's a place you wanna go, I'm the one you need to know!",
    // Adventure vibes
    "Come on, vámonos! Everybody, let's go!",
    "Delicioso!",
    // Backpack references
    "Backpack, Backpack!",
    "Anything that you might need, I've got inside for you!",
    // Encouraging
    "If you just believe in yourself, anything is possible!",
    "I have to keep going no matter what!",
    // Swiper reference (playful)
    "Swiper, no swiping!",
    // Short and sweet
    "¡Excelente!",
    "Super!",
    "You did it!",
];

/// Russian Dora-themed messages for Russian-speaking users
const DORA_MESSAGES_RU: &[&str] = &[
    // Classic Dora translations
    "Мы сделали это! ¡Lo hicimos!",
    "Поехали! ¡Vámonos!",
    "Мы бы не справились без тебя!",
    // Map references
    "Я — Карта! I'm the Map!",
    // Adventure vibes
    "Вперёд, vámonos! Все вместе!",
    "Вкуснятина! Delicioso!",
    // Backpack references
    "Рюкзачок, Рюкзачок!",
    // Encouraging
    "Если веришь в себя — всё возможно!",
    "Надо идти дальше, что бы ни случилось!",
    // Swiper reference
    "Жулик, не воруй! Swiper, no swiping!",
    // Short and sweet
    "Отлично! ¡Excelente!",
    "Супер!",
    "Ты справился!",
    "Рада была помочь!",
    "Приключение завершено!",
];

/// Get a random Dora message
pub fn get_random_dora_message() -> &'static str {
    let messages = DORA_MESSAGES_RU; // Use Russian messages by default
    let index = rand::thread_rng().gen_range(0..messages.len());
    messages[index]
}

/// Get a random English Dora message
#[allow(dead_code)]
pub fn get_random_dora_message_en() -> &'static str {
    let index = rand::thread_rng().gen_range(0..DORA_MESSAGES.len());
    DORA_MESSAGES[index]
}

/// Formats a copyright signature for media captions
///
/// Returns a string like:
/// "Мы сделали это! ¡Lo hicimos!
/// Ваш, @SaveAsBot"
pub fn format_copyright_signature() -> String {
    if !*COPYRIGHT_ENABLED {
        return String::new();
    }

    let message = get_random_dora_message();
    let tag = get_bot_tag();

    format!(
        "\n\n_{}_\nВаш, {}",
        escape_markdown_v2(message),
        escape_markdown_v2(&tag)
    )
}

/// Formats a media caption with copyright signature
///
/// Takes the base caption (title/artist) and appends the copyright signature.
pub fn format_caption_with_copyright(base_caption: &str) -> String {
    if !*COPYRIGHT_ENABLED {
        return base_caption.to_string();
    }

    let signature = format_copyright_signature();
    format!("{}{}", base_caption, signature)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dora_messages_count() {
        // Verify we have enough messages for variety
        assert!(DORA_MESSAGES.len() >= 10);
        assert!(DORA_MESSAGES_RU.len() >= 10);
    }

    #[test]
    fn test_random_message_returns_valid() {
        let msg = get_random_dora_message();
        assert!(!msg.is_empty());
    }

    #[test]
    fn test_format_copyright_signature() {
        let sig = format_copyright_signature();
        // Should contain "Ваш," and be non-empty (when enabled) or empty (when disabled)
        assert!(sig.contains("Ваш,") || sig.is_empty());
    }
}
