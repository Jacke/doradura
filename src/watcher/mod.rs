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
