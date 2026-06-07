//! Copyright/branding module for downloaded content
//!
//! Adds a Dora the Explorer themed signature to media captions.
//! "We did it! ¡Lo hicimos!" - Dora

use rand::Rng;
use std::env;
use std::sync::LazyLock;
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

/// Get the raw bot username (without @). Returns None if not yet set.
pub fn get_bot_username() -> Option<&'static str> {
    BOT_USERNAME.get().map(|s| s.strip_prefix('@').unwrap_or(s.as_str()))
}

/// Get the bot tag for branding
/// Priority: 1) Username from getMe(), 2) BOT_TAG env var, 3) Default
fn get_bot_tag() -> String {
    if let Some(username) = BOT_USERNAME.get() {
        return username.clone();
    }

    // Fallback to env var or default
    env::var("BOT_TAG").unwrap_or_else(|_| "@DoraDuraDoraDuraBot".to_string())
}

/// Enable copyright/branding in captions
/// Read from COPYRIGHT_ENABLED environment variable
/// Default: true
pub static COPYRIGHT_ENABLED: LazyLock<bool> = LazyLock::new(|| {
    env::var("COPYRIGHT_ENABLED")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(true)
});

/// Sign-off messages in the voice of singer Дора — cute-rock, self-ironic
/// vulnerability, late-night-headphones mood. Some lines reference her
/// lyrical themes (loud music as escape, small intimate moments, replay culture).
const DORA_MESSAGES_RU: &[&str] = &[
    // Headphones / volume / escape via music
    "Close the door, turn off your phone — and turn the music up. ❤️‍🔥",
    "Headphones on. The world can wait.",
    "Volume up. Thoughts down.",
    "Lights off, sound on.",
    "Press play. Forget everything else.",
    // Replay / one more time
    "One more song. Just one more.",
    "Replay until it makes sense.",
    "This one's been on repeat all week.",
    "Play it again — and again.",
    // Late night / intimate
    "It's 3 AM and we're still here.",
    "Some songs are better at 2 AM.",
    "For the long drives and longer nights.",
    // Self-ironic vulnerability
    "I'll cry to this song. And then dance to it.",
    "Soft girl. Loud songs.",
    "Heart in headphones.",
    "Dance alone. Cry alone. Same thing.",
    "Hide in the chorus.",
    // Soft handoff
    "Some songs find you when you need them.",
    "Trust me, this one hits different.",
    "Save it. You'll need it tonight.",
    "I downloaded this for you.",
    "Vibe first, words later.",
];

/// Get a random Dora message
pub fn get_random_dora_message() -> &'static str {
    let messages = DORA_MESSAGES_RU; // Use the default message set
    let index = rand::thread_rng().gen_range(0..messages.len());
    messages[index]
}

/// Formats a copyright signature for media captions
///
/// Returns a string like:
/// "We did it! ¡Lo hicimos!
/// Yours, @SaveAsBot"
pub fn format_copyright_signature() -> String {
    if !*COPYRIGHT_ENABLED {
        return String::new();
    }

    let message = get_random_dora_message();
    let tag = get_bot_tag();

    format!(
        "\n\n_{}_\nYours, {}",
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
        // Should contain "Yours," and be non-empty (when enabled) or empty (when disabled)
        assert!(sig.contains("Yours,") || sig.is_empty());
    }
}
