use crate::telegram::types::PreviewMetadata;
use moka::future::Cache;
use std::sync::LazyLock;
use std::time::Duration;

/// Cache for PreviewMetadata with TTL and size-bound.
pub struct PreviewCache {
    cache: Cache<String, PreviewMetadata>,
}

impl PreviewCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            cache: Cache::builder().max_capacity(5_000).time_to_live(ttl).build(),
        }
    }

    pub async fn get(&self, key: &str) -> Option<PreviewMetadata> {
        self.cache.get(key).await
    }

    pub async fn set(&self, key: String, data: PreviewMetadata) {
        self.cache.insert(key, data).await;
    }

    pub async fn cleanup(&self) {
        self.cache.run_pending_tasks().await;
    }

    pub fn len(&self) -> u64 {
        self.cache.entry_count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Global preview cache instance (singleton). TTL = 1 hour.
pub static PREVIEW_CACHE: LazyLock<PreviewCache> = LazyLock::new(|| PreviewCache::new(Duration::from_secs(3600)));

/// Cache of Telegram `message_id`s keyed by the originating URL.
pub struct LinkMessageCache {
    cache: Cache<String, i32>,
}

impl LinkMessageCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            cache: Cache::builder().max_capacity(2_000).time_to_live(ttl).build(),
        }
    }

    pub async fn set(&self, url: &str, message_id: i32) {
        self.cache.insert(url.to_string(), message_id).await;
    }

    pub async fn get(&self, url: &str) -> Option<i32> {
        self.cache.get(url).await
    }
}

pub static LINK_MESSAGE_CACHE: LazyLock<LinkMessageCache> =
    LazyLock::new(|| LinkMessageCache::new(Duration::from_secs(3600)));

pub async fn store_link_message_id(url: &str, message_id: i32) {
    LINK_MESSAGE_CACHE.set(url, message_id).await;
}

pub async fn get_link_message_id(url: &str) -> Option<i32> {
    LINK_MESSAGE_CACHE.get(url).await
}

/// Cache of user-picked time ranges keyed by URL.
pub struct TimeRangeCache {
    cache: Cache<String, (String, String)>,
}

impl TimeRangeCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            cache: Cache::builder().max_capacity(1_000).time_to_live(ttl).build(),
        }
    }

    pub async fn set(&self, url: &str, range: (String, String)) {
        self.cache.insert(url.to_string(), range).await;
    }

    pub async fn get(&self, url: &str) -> Option<(String, String)> {
        self.cache.get(url).await
    }
}

pub static TIME_RANGE_CACHE: LazyLock<TimeRangeCache> =
    LazyLock::new(|| TimeRangeCache::new(Duration::from_secs(3600)));

pub async fn store_time_range(url: &str, range: (String, String)) {
    TIME_RANGE_CACHE.set(url, range).await;
}

pub async fn get_time_range(url: &str) -> Option<(String, String)> {
    TIME_RANGE_CACHE.get(url).await
}

/// Cache for per-URL burn subtitle language selection (from preview button).
///
/// Stores the user-chosen subtitle language (e.g. "en", "ru") keyed by URL string.
/// When present, the download pipeline uses this language instead of DB settings.
pub struct BurnSubLangCache {
    cache: Cache<String, String>,
}

impl Default for BurnSubLangCache {
    fn default() -> Self {
        Self::new()
    }
}

impl BurnSubLangCache {
    pub fn new() -> Self {
        Self {
            cache: Cache::builder().max_capacity(500).build(),
        }
    }

    /// Store the subtitle language for a URL, or remove it if `lang` is `None`.
    pub async fn set(&self, url: &str, lang: Option<String>) {
        match lang {
            Some(l) => self.cache.insert(url.to_string(), l).await,
            None => self.cache.invalidate(url).await,
        }
    }

    /// Get the cached subtitle language for a URL, if any.
    pub async fn get(&self, url: &str) -> Option<String> {
        self.cache.get(url).await
    }
}

pub static BURN_SUB_LANG_CACHE: LazyLock<BurnSubLangCache> = LazyLock::new(BurnSubLangCache::new);

/// Store the burn subtitle language for a URL (or clear it with `None`).
pub async fn store_burn_sub_lang(url: &str, lang: Option<String>) {
    BURN_SUB_LANG_CACHE.set(url, lang).await;
}

/// Get the cached burn subtitle language for a URL.
pub async fn get_burn_sub_lang(url: &str) -> Option<String> {
    BURN_SUB_LANG_CACHE.get(url).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_burn_sub_lang_cache_store_and_get() {
        let cache = BurnSubLangCache::new();
        assert_eq!(cache.get("https://example.com/video1").await, None);

        cache.set("https://example.com/video1", Some("en".to_string())).await;
        assert_eq!(cache.get("https://example.com/video1").await, Some("en".to_string()));
    }

    #[tokio::test]
    async fn test_burn_sub_lang_cache_overwrite() {
        let cache = BurnSubLangCache::new();
        cache.set("https://example.com/video1", Some("en".to_string())).await;
        cache.set("https://example.com/video1", Some("ru".to_string())).await;
        assert_eq!(cache.get("https://example.com/video1").await, Some("ru".to_string()));
    }

    #[tokio::test]
    async fn test_burn_sub_lang_cache_clear_with_none() {
        let cache = BurnSubLangCache::new();
        cache.set("https://example.com/video1", Some("en".to_string())).await;
        assert_eq!(cache.get("https://example.com/video1").await, Some("en".to_string()));

        cache.set("https://example.com/video1", None).await;
        assert_eq!(cache.get("https://example.com/video1").await, None);
    }

    #[tokio::test]
    async fn test_burn_sub_lang_cache_independent_urls() {
        let cache = BurnSubLangCache::new();
        cache.set("https://example.com/video1", Some("en".to_string())).await;
        cache.set("https://example.com/video2", Some("fr".to_string())).await;

        assert_eq!(cache.get("https://example.com/video1").await, Some("en".to_string()));
        assert_eq!(cache.get("https://example.com/video2").await, Some("fr".to_string()));
    }

    #[tokio::test]
    async fn test_burn_sub_lang_cache_missing_url_returns_none() {
        let cache = BurnSubLangCache::new();
        assert_eq!(cache.get("https://nonexistent.com/video").await, None);
    }
}
