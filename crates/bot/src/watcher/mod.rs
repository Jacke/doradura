//! Content watcher system for monitoring sources (Instagram, YouTube, etc.)
//! for new posts, stories, and other content.
//!
//! Architecture: The watcher module is independent from teloxide. It emits
//! `WatchNotification` structs through a `tokio::mpsc` channel. The Telegram
//! layer (`telegram/subscriptions.rs`) receives and formats them.

pub mod db;
pub mod instagram;
pub mod scheduler;
pub mod traits;

pub use traits::{CheckResult, ContentWatcher, WatchNotification, WatchUpdate};

use std::collections::HashMap;

/// Registry of available content watchers.
pub struct WatcherRegistry {
    watchers: HashMap<String, Box<dyn ContentWatcher>>,
}

impl WatcherRegistry {
    pub fn new() -> Self {
        Self {
            watchers: HashMap::new(),
        }
    }

    /// Register a watcher for a source type.
    pub fn register(&mut self, watcher: Box<dyn ContentWatcher>) {
        let key = watcher.source_type().to_string();
        self.watchers.insert(key, watcher);
    }

    /// Get a watcher by source type.
    pub fn get(&self, source_type: &str) -> Option<&dyn ContentWatcher> {
        self.watchers.get(source_type).map(|w| w.as_ref())
    }

    /// List all registered source types.
    pub fn source_types(&self) -> Vec<&str> {
        self.watchers.keys().map(|s| s.as_str()).collect()
    }

    /// Create the default registry with all available watchers.
    pub fn default_registry() -> Self {
        let mut registry = Self::new();
        registry.register(Box::new(instagram::InstagramWatcher::new()));
        registry
    }
}

impl Default for WatcherRegistry {
    fn default() -> Self {
        Self::default_registry()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Registry key correctness ─────────────────────────────────────────────

    /// CRITICAL: registry must respond to "instagram", NOT "ig".
    /// This was the exact bug: old code used "cw:ok:ig:..." in button callbacks,
    /// but registry only knows "instagram" → get("ig") == None → "Watcher not available".
    #[test]
    fn registry_get_instagram_returns_some() {
        let r = WatcherRegistry::default_registry();
        assert!(
            r.get("instagram").is_some(),
            "WatcherRegistry must have 'instagram' key — buttons must generate cw:ok:instagram:..."
        );
    }

    #[test]
    fn registry_get_ig_returns_none() {
        let r = WatcherRegistry::default_registry();
        assert!(
            r.get("ig").is_none(),
            "'ig' is not a valid registry key — this is the bug that caused 'Watcher not available'"
        );
    }

    #[test]
    fn registry_get_empty_returns_none() {
        let r = WatcherRegistry::default_registry();
        assert!(r.get("").is_none());
    }

    #[test]
    fn registry_get_unknown_returns_none() {
        let r = WatcherRegistry::default_registry();
        assert!(r.get("youtube").is_none());
        assert!(r.get("tiktok").is_none());
    }

    // ── Watcher metadata ─────────────────────────────────────────────────────

    #[test]
    fn instagram_watcher_source_type_is_instagram() {
        let r = WatcherRegistry::default_registry();
        let w = r.get("instagram").expect("instagram watcher must exist");
        assert_eq!(w.source_type(), "instagram");
    }

    #[test]
    fn instagram_watcher_has_posts_and_stories_content_types() {
        let r = WatcherRegistry::default_registry();
        let w = r.get("instagram").unwrap();
        let types = w.content_types();
        // bit 1 = Posts, bit 2 = Stories
        let masks: Vec<u32> = types.iter().map(|(m, _)| *m).collect();
        assert!(masks.contains(&1), "Posts (mask=1) must be a content type");
        assert!(masks.contains(&2), "Stories (mask=2) must be a content type");
    }

    #[test]
    fn instagram_watcher_default_mask_covers_posts() {
        let r = WatcherRegistry::default_registry();
        let w = r.get("instagram").unwrap();
        assert!(
            w.default_watch_mask() & 1 != 0,
            "default mask must include Posts (bit 1)"
        );
    }

    #[test]
    fn source_types_list_contains_instagram() {
        let r = WatcherRegistry::default_registry();
        let types = r.source_types();
        assert!(
            types.contains(&"instagram"),
            "source_types() must include 'instagram', got: {:?}",
            types
        );
    }

    // ── Callback data parsing — the exact logic from handle_subscription_callback ──

    /// Simulate parsing "cw:ok:instagram:dashaostro:3" the same way
    /// handle_subscription_callback does it.
    #[test]
    fn callback_ok_with_instagram_source_type_resolves_in_registry() {
        let callback_data = "cw:ok:instagram:dashaostro:3";
        let parts: Vec<&str> = callback_data.split(':').collect();

        assert_eq!(parts.len(), 5);
        assert_eq!(parts[0], "cw");
        assert_eq!(parts[1], "ok");
        let source_type = parts[2]; // "instagram"
        let source_id = parts[3]; // "dashaostro"
        let mask: u32 = parts[4].parse().unwrap_or(3);

        assert_eq!(source_type, "instagram");
        assert_eq!(source_id, "dashaostro");
        assert_eq!(mask, 3);

        // THE critical check: registry must find this source_type
        let r = WatcherRegistry::default_registry();
        assert!(
            r.get(source_type).is_some(),
            "registry.get('{}') must return Some — otherwise we get 'Watcher not available'",
            source_type
        );
    }

    /// The OLD broken callback: "cw:ok:ig:dashaostro:3" — source_type="ig" fails.
    #[test]
    fn callback_ok_with_old_ig_source_type_fails_registry_lookup() {
        let old_callback_data = "cw:ok:ig:dashaostro:3";
        let parts: Vec<&str> = old_callback_data.split(':').collect();
        let source_type = parts[2]; // "ig" — the bug

        let r = WatcherRegistry::default_registry();
        assert!(
            r.get(source_type).is_none(),
            "OLD callback 'cw:ok:ig:...' MUST fail — proves the bug exists when 'ig' is used"
        );
    }

    /// Simulate "cw:ptog:username:1:3" parsing (toggle Posts mask).
    #[test]
    fn callback_ptog_parsing() {
        let data = "cw:ptog:cristiano:1:3";
        let parts: Vec<&str> = data.split(':').collect();

        assert_eq!(parts.len(), 5);
        assert_eq!(parts[1], "ptog");
        let username = parts[2]; // "cristiano"
        let bit: u32 = parts[3].parse().unwrap(); // 1 (Posts)
        let current_mask: u32 = parts[4].parse().unwrap(); // 3

        assert_eq!(username, "cristiano");
        assert_eq!(bit, 1);
        assert_eq!(current_mask, 3);

        // Toggle: mask XOR bit
        let new_mask = current_mask ^ bit; // 3 ^ 1 = 2 (Stories only)
        assert_eq!(new_mask, 2);
        // Don't allow mask=0
        let new_mask = if new_mask == 0 { bit } else { new_mask };
        assert_eq!(new_mask, 2);
    }

    /// After toggling, the new Confirm button must STILL use "instagram".
    #[test]
    fn confirm_button_after_toggle_uses_instagram_source_type() {
        // Simulate update_toggle_keyboard generating new Confirm button data
        let username = "cristiano";
        let new_mask = 2u32;
        let confirm_cb = format!("cw:ok:instagram:{}:{}", username, new_mask);
        assert_eq!(confirm_cb, "cw:ok:instagram:cristiano:2");

        // Parse it back the same way handle_subscription_callback does
        let parts: Vec<&str> = confirm_cb.split(':').collect();
        assert_eq!(parts[2], "instagram");

        let r = WatcherRegistry::default_registry();
        assert!(r.get(parts[2]).is_some(), "toggle → confirm must still find watcher");
    }

    /// Simulate "cw:tog:42:1" parsing (toggle mask for existing subscription).
    #[test]
    fn callback_tog_parsing() {
        let data = "cw:tog:42:1";
        let parts: Vec<&str> = data.split(':').collect();

        assert_eq!(parts.len(), 4);
        assert_eq!(parts[1], "tog");
        let sub_id: i64 = parts[2].parse().unwrap();
        let bit: u32 = parts[3].parse().unwrap();

        assert_eq!(sub_id, 42);
        assert_eq!(bit, 1);
    }

    /// Simulate "cw:unsub:42" parsing.
    #[test]
    fn callback_unsub_parsing() {
        let data = "cw:unsub:42";
        let parts: Vec<&str> = data.split(':').collect();

        assert_eq!(parts.len(), 3);
        assert_eq!(parts[1], "unsub");
        let sub_id: i64 = parts[2].parse().unwrap();
        assert_eq!(sub_id, 42);
    }

    /// Verify mask toggle never produces 0.
    #[test]
    fn mask_toggle_never_zero() {
        // Toggling Posts when only Posts selected → would be 0, fallback to bit
        let current = 1u32; // Posts only
        let bit = 1u32; // toggle Posts
        let new_mask = current ^ bit; // 0
        let new_mask = if new_mask == 0 { bit } else { new_mask };
        assert_eq!(new_mask, 1, "mask must not become 0; falls back to bit");

        // Toggling Stories when both selected → 1 (Posts only)
        let current = 3u32;
        let bit = 2u32;
        let new_mask = current ^ bit; // 1
        let new_mask = if new_mask == 0 { bit } else { new_mask };
        assert_eq!(new_mask, 1);
    }

    // ── Register/lookup consistency ───────────────────────────────────────────

    #[test]
    fn registered_source_type_matches_get_key() {
        let mut r = WatcherRegistry::new();
        r.register(Box::new(instagram::InstagramWatcher::new()));
        // Whatever source_type() returns must be the key that get() accepts
        let w = r.get("instagram").expect("must be registered");
        assert_eq!(w.source_type(), "instagram");
    }
}
