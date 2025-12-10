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
}
