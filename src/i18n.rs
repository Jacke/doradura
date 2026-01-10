use std::collections::HashMap;
use std::sync::Arc;

use fluent_templates::{
    fluent_bundle::{FluentArgs, FluentValue},
    static_loader, Loader,
};
use once_cell::sync::Lazy;
use unic_langid::LanguageIdentifier;

use crate::storage::db;

static_loader! {
    static LOCALES = {
        locales: "./locales",
        fallback_language: "ru",
    };
}

/// Supported languages (code, human-readable name).
pub static SUPPORTED_LANGS: &[(&str, &str)] = &[
    ("en", "English"),
    ("ru", "Русский"),
    ("fr", "Français"),
    ("de", "Deutsch"),
];

/// Default language identifier used as a fallback.
static DEFAULT_LANG: Lazy<LanguageIdentifier> = Lazy::new(|| "ru".parse().unwrap());

/// Normalizes a language code into a LanguageIdentifier (falls back to default).
pub fn lang_from_code(code: &str) -> LanguageIdentifier {
    // Map short codes to full identifiers where needed
    let code_normalized = code.to_lowercase();
    let normalized = match code_normalized.as_str() {
        "en" | "en-us" => "en-US",
        "ru" | "ru-ru" => "ru",
        "fr" | "fr-fr" => "fr",
        "de" | "de-de" => "de",
        other => other,
    };

    normalized.parse().unwrap_or_else(|_| DEFAULT_LANG.clone())
}

/// Resolves the language for a user from the database using an existing connection.
pub fn user_lang(conn: &db::DbConnection, telegram_id: i64) -> LanguageIdentifier {
    match db::get_user_language(conn, telegram_id) {
        Ok(lang_code) => lang_from_code(&lang_code),
        Err(_) => DEFAULT_LANG.clone(),
    }
}

/// Resolves the language for a user using a connection pool.
pub fn user_lang_from_pool(db_pool: &Arc<db::DbPool>, telegram_id: i64) -> LanguageIdentifier {
    if let Ok(conn) = db::get_connection(db_pool) {
        return user_lang(&conn, telegram_id);
    }
    DEFAULT_LANG.clone()
}

/// Resolves the language for a user, falling back to Telegram locale when DB is default.
pub fn user_lang_from_pool_with_fallback(
    db_pool: &Arc<db::DbPool>,
    telegram_id: i64,
    telegram_lang_code: Option<&str>,
) -> LanguageIdentifier {
    let db_lang = if let Ok(conn) = db::get_connection(db_pool) {
        db::get_user_language(&conn, telegram_id).ok()
    } else {
        None
    };

    if let Some(lang_code) = db_lang.as_deref() {
        let lang = lang_from_code(lang_code);
        if let Some(telegram_code) = telegram_lang_code.and_then(is_language_supported) {
            if lang_code == "ru" && telegram_code != "ru" {
                if let Ok(conn) = db::get_connection(db_pool) {
                    let _ = db::set_user_language(&conn, telegram_id, telegram_code);
                }
                return lang_from_code(telegram_code);
            }
        }
        return lang;
    }

    if let Some(telegram_code) = telegram_lang_code.and_then(is_language_supported) {
        return lang_from_code(telegram_code);
    }

    DEFAULT_LANG.clone()
}

/// Returns a localized string for the given key.
/// Converts literal `\n` sequences to actual newlines for proper Telegram formatting.
pub fn t(lang: &LanguageIdentifier, key: &str) -> String {
    let text = LOCALES
        .lookup(lang, key)
        .unwrap_or_else(|| LOCALES.lookup(&DEFAULT_LANG, key).unwrap_or_else(|| key.to_string()));
    text.replace("\\n", "\n")
}

/// Returns a localized string with arguments for interpolation.
/// Converts literal `\n` sequences to actual newlines for proper Telegram formatting.
pub fn t_args(lang: &LanguageIdentifier, key: &str, args: &FluentArgs) -> String {
    let args_map: HashMap<String, FluentValue> = args.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();

    let text = LOCALES.lookup_with_args(lang, key, &args_map).unwrap_or_else(|| {
        LOCALES
            .lookup_with_args(&DEFAULT_LANG, key, &args_map)
            .unwrap_or_else(|| key.to_string())
    });
    text.replace("\\n", "\n")
}

/// Finds a human-friendly name for a language code.
pub fn language_name(code: &str) -> &str {
    SUPPORTED_LANGS
        .iter()
        .find(|(c, _)| c.eq_ignore_ascii_case(code))
        .map(|(_, name)| *name)
        .unwrap_or("Unknown")
}

/// Checks if a language code is supported by the bot.
/// Returns the normalized language code if supported, None otherwise.
pub fn is_language_supported(code: &str) -> Option<&'static str> {
    // Normalize the code (e.g., "en-US" -> "en", "ru-RU" -> "ru")
    let normalized = code.split('-').next().unwrap_or(code).to_lowercase();

    SUPPORTED_LANGS
        .iter()
        .find(|(c, _)| c.eq_ignore_ascii_case(&normalized))
        .map(|(c, _)| *c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_known_translation() {
        let ru = lang_from_code("ru");
        let en = lang_from_code("en");

        assert_eq!(t(&ru, "commands.processing"), "⏳ Получаю информацию...");
        assert_eq!(t(&en, "commands.processing"), "⏳ Fetching info...");
    }

    #[test]
    fn converts_newlines() {
        let en = lang_from_code("en");
        let text = t(&en, "menu.services_text");

        // Should contain actual newlines, not literal \n
        assert!(text.contains('\n'));
        assert!(!text.contains("\\n"));
    }

    #[test]
    fn test_is_language_supported() {
        // Test supported languages
        assert_eq!(is_language_supported("en"), Some("en"));
        assert_eq!(is_language_supported("ru"), Some("ru"));
        assert_eq!(is_language_supported("fr"), Some("fr"));
        assert_eq!(is_language_supported("de"), Some("de"));

        // Test with language variants (should normalize to base language)
        assert_eq!(is_language_supported("en-US"), Some("en"));
        assert_eq!(is_language_supported("en-GB"), Some("en"));
        assert_eq!(is_language_supported("ru-RU"), Some("ru"));
        assert_eq!(is_language_supported("fr-FR"), Some("fr"));
        assert_eq!(is_language_supported("de-DE"), Some("de"));

        // Test case insensitivity
        assert_eq!(is_language_supported("EN"), Some("en"));
        assert_eq!(is_language_supported("RU"), Some("ru"));

        // Test unsupported languages
        assert_eq!(is_language_supported("es"), None);
        assert_eq!(is_language_supported("it"), None);
        assert_eq!(is_language_supported("ja"), None);
        assert_eq!(is_language_supported("unknown"), None);
    }
}
