//! Extension system for Doradura bot features.
//!
//! Each extension represents a distinct capability (downloading, converting, audio effects, etc.)
//! with metadata used for the UI (icons, descriptions, status).

pub mod audio_effects;
pub mod converter;
pub mod http_downloader;
pub mod ytdlp_downloader;

use unic_langid::LanguageIdentifier;

/// Categories of extensions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtensionCategory {
    Downloader,
    Converter,
    AudioProcessor,
}

/// A capability provided by an extension.
#[derive(Debug, Clone)]
pub struct Capability {
    pub name: String,
    pub description: String,
}

/// Trait for bot extensions that provide UI metadata.
pub trait BotExtension: Send + Sync {
    /// Unique identifier (e.g., "ytdlp", "http", "converter", "audio_effects").
    fn id(&self) -> &str;

    /// Locale key prefix for name/description lookup.
    fn locale_key(&self) -> &str;

    /// Icon emoji.
    fn icon(&self) -> &str;

    /// List of capabilities this extension provides.
    fn capabilities(&self) -> Vec<Capability>;

    /// Whether the extension is currently available.
    fn is_available(&self) -> bool;

    /// Category for grouping in UI.
    fn category(&self) -> ExtensionCategory;

    /// Get localized name using the locale key.
    fn localized_name(&self, lang: &LanguageIdentifier) -> String {
        crate::i18n::t(lang, &format!("{}.name", self.locale_key()))
    }

    /// Get localized description using the locale key.
    fn localized_description(&self, lang: &LanguageIdentifier) -> String {
        crate::i18n::t(lang, &format!("{}.description", self.locale_key()))
    }
}

/// Registry of all available extensions.
pub struct ExtensionRegistry {
    extensions: Vec<Box<dyn BotExtension>>,
}

impl ExtensionRegistry {
    /// Create the default registry with all built-in extensions.
    pub fn default_registry() -> Self {
        let extensions: Vec<Box<dyn BotExtension>> = vec![
            Box::new(ytdlp_downloader::YtDlpExtension),
            Box::new(http_downloader::HttpExtension),
            Box::new(converter::ConverterExtension),
            Box::new(audio_effects::AudioEffectsExtension),
        ];
        Self { extensions }
    }

    /// Get all extensions.
    pub fn all(&self) -> &[Box<dyn BotExtension>] {
        &self.extensions
    }

    /// Get extensions filtered by category.
    pub fn by_category(&self, category: ExtensionCategory) -> Vec<&dyn BotExtension> {
        self.extensions
            .iter()
            .filter(|e| e.category() == category)
            .map(|e| e.as_ref())
            .collect()
    }

    /// Find an extension by ID.
    pub fn get(&self, id: &str) -> Option<&dyn BotExtension> {
        self.extensions.iter().find(|e| e.id() == id).map(|e| e.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Registry basics ──

    #[test]
    fn test_default_registry_has_all_extensions() {
        let reg = ExtensionRegistry::default_registry();
        assert_eq!(reg.all().len(), 4);
    }

    #[test]
    fn test_registry_get_by_id() {
        let reg = ExtensionRegistry::default_registry();
        assert!(reg.get("ytdlp").is_some());
        assert!(reg.get("http").is_some());
        assert!(reg.get("converter").is_some());
        assert!(reg.get("audio_effects").is_some());
        assert!(reg.get("nonexistent").is_none());
        assert!(reg.get("").is_none());
    }

    #[test]
    fn test_registry_by_category_downloaders() {
        let reg = ExtensionRegistry::default_registry();
        let downloaders = reg.by_category(ExtensionCategory::Downloader);
        assert_eq!(downloaders.len(), 2);
        let ids: Vec<&str> = downloaders.iter().map(|e| e.id()).collect();
        assert!(ids.contains(&"ytdlp"));
        assert!(ids.contains(&"http"));
    }

    #[test]
    fn test_registry_by_category_converters() {
        let reg = ExtensionRegistry::default_registry();
        let converters = reg.by_category(ExtensionCategory::Converter);
        assert_eq!(converters.len(), 1);
        assert_eq!(converters[0].id(), "converter");
    }

    #[test]
    fn test_registry_by_category_audio_processors() {
        let reg = ExtensionRegistry::default_registry();
        let processors = reg.by_category(ExtensionCategory::AudioProcessor);
        assert_eq!(processors.len(), 1);
        assert_eq!(processors[0].id(), "audio_effects");
    }

    // ── Extension IDs are unique ──

    #[test]
    fn test_all_extension_ids_unique() {
        let reg = ExtensionRegistry::default_registry();
        let ids: Vec<&str> = reg.all().iter().map(|e| e.id()).collect();
        let mut unique = ids.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(ids.len(), unique.len(), "Extension IDs must be unique");
    }

    // ── Extension locale keys are unique ──

    #[test]
    fn test_all_locale_keys_unique() {
        let reg = ExtensionRegistry::default_registry();
        let keys: Vec<&str> = reg.all().iter().map(|e| e.locale_key()).collect();
        let mut unique = keys.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(keys.len(), unique.len(), "Locale keys must be unique");
    }

    // ── All extensions available by default ──

    #[test]
    fn test_all_extensions_available() {
        let reg = ExtensionRegistry::default_registry();
        for ext in reg.all() {
            assert!(ext.is_available(), "Extension '{}' should be available", ext.id());
        }
    }

    // ── All extensions have non-empty capabilities ──

    #[test]
    fn test_all_extensions_have_capabilities() {
        let reg = ExtensionRegistry::default_registry();
        for ext in reg.all() {
            let caps = ext.capabilities();
            assert!(!caps.is_empty(), "Extension '{}' should have capabilities", ext.id());
            for cap in &caps {
                assert!(!cap.name.is_empty(), "Capability name for '{}' is empty", ext.id());
                assert!(
                    !cap.description.is_empty(),
                    "Capability desc for '{}' is empty",
                    ext.id()
                );
            }
        }
    }

    // ── All extensions have icons ──

    #[test]
    fn test_all_extensions_have_icons() {
        let reg = ExtensionRegistry::default_registry();
        for ext in reg.all() {
            assert!(!ext.icon().is_empty(), "Extension '{}' should have an icon", ext.id());
        }
    }

    // ── YtDlp extension specifics ──

    #[test]
    fn test_ytdlp_extension_metadata() {
        let ext = ytdlp_downloader::YtDlpExtension;
        assert_eq!(ext.id(), "ytdlp");
        assert_eq!(ext.locale_key(), "ext_ytdlp");
        assert_eq!(ext.category(), ExtensionCategory::Downloader);
        assert!(ext.is_available());
        assert_eq!(ext.capabilities().len(), 5);
    }

    // ── Http extension specifics ──

    #[test]
    fn test_http_extension_metadata() {
        let ext = http_downloader::HttpExtension;
        assert_eq!(ext.id(), "http");
        assert_eq!(ext.locale_key(), "ext_http");
        assert_eq!(ext.category(), ExtensionCategory::Downloader);
        assert!(ext.is_available());
        assert_eq!(ext.capabilities().len(), 3);
    }

    // ── Converter extension specifics ──

    #[test]
    fn test_converter_extension_metadata() {
        let ext = converter::ConverterExtension;
        assert_eq!(ext.id(), "converter");
        assert_eq!(ext.locale_key(), "ext_converter");
        assert_eq!(ext.category(), ExtensionCategory::Converter);
        assert!(ext.is_available());
        assert_eq!(ext.capabilities().len(), 5);
    }

    // ── Audio effects extension specifics ──

    #[test]
    fn test_audio_effects_extension_metadata() {
        let ext = audio_effects::AudioEffectsExtension;
        assert_eq!(ext.id(), "audio_effects");
        assert_eq!(ext.locale_key(), "ext_audio_effects");
        assert_eq!(ext.category(), ExtensionCategory::AudioProcessor);
        assert!(ext.is_available());
        assert_eq!(ext.capabilities().len(), 4);
    }

    // ── Localized names resolve (not fallback to raw key) ──

    #[test]
    fn test_localized_names_resolve_en() {
        let lang = crate::i18n::lang_from_code("en");
        let reg = ExtensionRegistry::default_registry();
        for ext in reg.all() {
            let name = ext.localized_name(&lang);
            // Name should NOT be the raw key like "ext_ytdlp.name"
            assert!(
                !name.contains('.'),
                "Extension '{}' name not localized: '{}'",
                ext.id(),
                name
            );
            assert!(!name.is_empty());
        }
    }

    #[test]
    fn test_localized_descriptions_resolve_en() {
        let lang = crate::i18n::lang_from_code("en");
        let reg = ExtensionRegistry::default_registry();
        for ext in reg.all() {
            let desc = ext.localized_description(&lang);
            assert!(
                !desc.contains('.'),
                "Extension '{}' description not localized: '{}'",
                ext.id(),
                desc
            );
            assert!(!desc.is_empty());
        }
    }

    #[test]
    fn test_localized_names_resolve_ru() {
        let lang = crate::i18n::lang_from_code("ru");
        let reg = ExtensionRegistry::default_registry();
        for ext in reg.all() {
            let name = ext.localized_name(&lang);
            assert!(!name.contains('.'), "RU name not localized for '{}'", ext.id());
        }
    }

    #[test]
    fn test_localized_names_resolve_fr() {
        let lang = crate::i18n::lang_from_code("fr");
        let reg = ExtensionRegistry::default_registry();
        for ext in reg.all() {
            let name = ext.localized_name(&lang);
            assert!(!name.contains('.'), "FR name not localized for '{}'", ext.id());
        }
    }

    #[test]
    fn test_localized_names_resolve_de() {
        let lang = crate::i18n::lang_from_code("de");
        let reg = ExtensionRegistry::default_registry();
        for ext in reg.all() {
            let name = ext.localized_name(&lang);
            assert!(!name.contains('.'), "DE name not localized for '{}'", ext.id());
        }
    }

    // ── Extension locale keys in all 4 languages ──

    #[test]
    fn test_extension_locale_keys_all_languages() {
        let languages = ["en", "ru", "fr", "de"];
        let keys = [
            "extensions.header",
            "extensions.category_download",
            "extensions.category_convert",
            "extensions.category_process",
            "extensions.status_active",
            "extensions.status_unavailable",
            "extensions.footer",
            "extensions.detail_back",
        ];

        for lang_code in &languages {
            let lang = crate::i18n::lang_from_code(lang_code);
            for key in &keys {
                let val = crate::i18n::t(&lang, key);
                assert!(
                    !val.is_empty() && val != *key,
                    "Locale key '{}' missing for language '{}'",
                    key,
                    lang_code
                );
            }
        }
    }

    // ── Category coverage ──

    #[test]
    fn test_every_category_has_extensions() {
        let reg = ExtensionRegistry::default_registry();
        assert!(!reg.by_category(ExtensionCategory::Downloader).is_empty());
        assert!(!reg.by_category(ExtensionCategory::Converter).is_empty());
        assert!(!reg.by_category(ExtensionCategory::AudioProcessor).is_empty());
    }

    // ── Capability content validation ──

    #[test]
    fn test_ytdlp_capabilities_contain_youtube() {
        let ext = ytdlp_downloader::YtDlpExtension;
        let caps = ext.capabilities();
        let names: Vec<&str> = caps.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"YouTube"), "YtDlp must support YouTube");
        assert!(names.contains(&"TikTok"), "YtDlp must support TikTok");
    }

    #[test]
    fn test_converter_capabilities_contain_gif() {
        let ext = converter::ConverterExtension;
        let caps = ext.capabilities();
        let names: Vec<&str> = caps.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"GIF"), "Converter must support GIF");
        assert!(names.contains(&"Compress"), "Converter must support Compress");
    }

    #[test]
    fn test_audio_effects_capabilities() {
        let ext = audio_effects::AudioEffectsExtension;
        let caps = ext.capabilities();
        let names: Vec<&str> = caps.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"Pitch"));
        assert!(names.contains(&"Tempo"));
        assert!(names.contains(&"Bass Boost"));
        assert!(names.contains(&"Morph"));
    }
}
