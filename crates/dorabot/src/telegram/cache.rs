use crate::telegram::types::PreviewMetadata;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{Duration, Instant};

/// Maximum number of entries in the preview cache
const MAX_PREVIEW_CACHE_SIZE: usize = 5_000;

/// Number of entries to evict when cache is full
const PREVIEW_EVICTION_BATCH: usize = 500;

/// Structure for storing cached data
struct CachedItem {
    data: PreviewMetadata,
    cached_at: Instant,
}

/// Cache for PreviewMetadata with size limit
pub struct PreviewCache {
    cache: Arc<Mutex<HashMap<String, CachedItem>>>,
    ttl: Duration,
}

impl PreviewCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            cache: Arc::new(Mutex::new(HashMap::new())),
            ttl,
        }
    }

    pub async fn get(&self, key: &str) -> Option<PreviewMetadata> {
        let mut cache = self.cache.lock().await;
        if let Some(item) = cache.get(key) {
            if item.cached_at.elapsed() < self.ttl {
                return Some(item.data.clone());
            } else {
                cache.remove(key);
            }
        }
        None
    }

    /// Evict oldest entries using random sampling when cache is full
    fn evict_if_needed(cache: &mut HashMap<String, CachedItem>) {
        if cache.len() <= MAX_PREVIEW_CACHE_SIZE {
            return;
        }

        use rand::seq::IteratorRandom;
        let mut rng = rand::thread_rng();

        // Sample random entries and evict oldest ones
        const SAMPLE_SIZE: usize = 300;
        let sample: Vec<_> = cache
            .iter()
            .choose_multiple(&mut rng, SAMPLE_SIZE.min(cache.len()))
            .into_iter()
            .map(|(k, v)| (k.clone(), v.cached_at))
            .collect();

        let mut sorted_sample = sample;
        sorted_sample.sort_by_key(|(_, time)| *time);

        let to_remove: Vec<_> = sorted_sample
            .iter()
            .take(PREVIEW_EVICTION_BATCH)
            .map(|(k, _)| k.clone())
            .collect();

        for key in &to_remove {
            cache.remove(key);
        }

        log::debug!(
            "🗑️ PreviewCache eviction: removed {} entries, size now {}",
            to_remove.len(),
            cache.len()
        );
    }

    pub async fn set(&self, key: String, data: PreviewMetadata) {
        let mut cache = self.cache.lock().await;

        // Evict if cache is full
        Self::evict_if_needed(&mut cache);

        cache.insert(
            key,
            CachedItem {
                data,
                cached_at: Instant::now(),
            },
        );
    }

    /// Remove expired entries
    pub async fn cleanup(&self) {
        let mut cache = self.cache.lock().await;
        let before = cache.len();
        cache.retain(|_, item| item.cached_at.elapsed() < self.ttl);
        let removed = before - cache.len();
        if removed > 0 {
            log::debug!("PreviewCache cleanup: removed {} expired entries", removed);
        }
    }

    /// Returns current cache size
    pub async fn len(&self) -> usize {
        self.cache.lock().await.len()
    }

    /// Returns true if cache has no entries
    pub async fn is_empty(&self) -> bool {
        self.cache.lock().await.is_empty()
    }
}

/// Global preview cache instance (singleton)
/// TTL = 1 hour (sufficient for a user session)
pub static PREVIEW_CACHE: std::sync::LazyLock<PreviewCache> =
    std::sync::LazyLock::new(|| PreviewCache::new(Duration::from_secs(3600)));

/// Maximum number of entries in the link message cache
const MAX_LINK_CACHE_SIZE: usize = 2_000;

/// Number of entries to evict when cache is full
const LINK_EVICTION_BATCH: usize = 200;

struct LinkMessageItem {
    message_id: i32,
    cached_at: Instant,
}

pub struct LinkMessageCache {
    cache: Arc<Mutex<HashMap<String, LinkMessageItem>>>,
    ttl: Duration,
}

impl LinkMessageCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            cache: Arc::new(Mutex::new(HashMap::new())),
            ttl,
        }
    }

    /// Evict oldest entries using random sampling when cache is full
    fn evict_if_needed(cache: &mut HashMap<String, LinkMessageItem>) {
        if cache.len() <= MAX_LINK_CACHE_SIZE {
            return;
        }

        use rand::seq::IteratorRandom;
        let mut rng = rand::thread_rng();

        // Sample random entries and evict oldest ones
        const SAMPLE_SIZE: usize = 200;
        let sample: Vec<_> = cache
            .iter()
            .choose_multiple(&mut rng, SAMPLE_SIZE.min(cache.len()))
            .into_iter()
            .map(|(k, v)| (k.clone(), v.cached_at))
            .collect();

        let mut sorted_sample = sample;
        sorted_sample.sort_by_key(|(_, time)| *time);

        let to_remove: Vec<_> = sorted_sample
            .iter()
            .take(LINK_EVICTION_BATCH)
            .map(|(k, _)| k.clone())
            .collect();

        for key in &to_remove {
            cache.remove(key);
        }

        log::debug!(
            "🗑️ LinkMessageCache eviction: removed {} entries, size now {}",
            to_remove.len(),
            cache.len()
        );
    }

    pub async fn set(&self, url: &str, message_id: i32) {
        let mut cache = self.cache.lock().await;

        // Evict if cache is full
        Self::evict_if_needed(&mut cache);

        cache.insert(
            url.to_string(),
            LinkMessageItem {
                message_id,
                cached_at: Instant::now(),
            },
        );
    }

    pub async fn get(&self, url: &str) -> Option<i32> {
        let mut cache = self.cache.lock().await;
        if let Some(item) = cache.get(url) {
            if item.cached_at.elapsed() < self.ttl {
                return Some(item.message_id);
            }
            cache.remove(url);
        }
        None
    }
}

pub static LINK_MESSAGE_CACHE: std::sync::LazyLock<LinkMessageCache> =
    std::sync::LazyLock::new(|| LinkMessageCache::new(Duration::from_secs(3600)));

pub async fn store_link_message_id(url: &str, message_id: i32) {
    LINK_MESSAGE_CACHE.set(url, message_id).await;
}

pub async fn get_link_message_id(url: &str) -> Option<i32> {
    LINK_MESSAGE_CACHE.get(url).await
}

/// Maximum number of entries in the time range cache
const MAX_TIME_RANGE_CACHE_SIZE: usize = 1_000;

/// Number of entries to evict when cache is full
const TIME_RANGE_EVICTION_BATCH: usize = 100;

struct TimeRangeItem {
    range: (String, String),
    cached_at: Instant,
}

pub struct TimeRangeCache {
    cache: Arc<Mutex<HashMap<String, TimeRangeItem>>>,
    ttl: Duration,
}

impl TimeRangeCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            cache: Arc::new(Mutex::new(HashMap::new())),
            ttl,
        }
    }

    fn evict_if_needed(cache: &mut HashMap<String, TimeRangeItem>) {
        if cache.len() <= MAX_TIME_RANGE_CACHE_SIZE {
            return;
        }

        use rand::seq::IteratorRandom;
        let mut rng = rand::thread_rng();

        const SAMPLE_SIZE: usize = 150;
        let sample: Vec<_> = cache
            .iter()
            .choose_multiple(&mut rng, SAMPLE_SIZE.min(cache.len()))
            .into_iter()
            .map(|(k, v)| (k.clone(), v.cached_at))
            .collect();

        let mut sorted_sample = sample;
        sorted_sample.sort_by_key(|(_, time)| *time);

        let to_remove: Vec<_> = sorted_sample
            .iter()
            .take(TIME_RANGE_EVICTION_BATCH)
            .map(|(k, _)| k.clone())
            .collect();

        for key in &to_remove {
            cache.remove(key);
        }

        log::debug!(
            "🗑️ TimeRangeCache eviction: removed {} entries, size now {}",
            to_remove.len(),
            cache.len()
        );
    }

    pub async fn set(&self, url: &str, range: (String, String)) {
        let mut cache = self.cache.lock().await;
        Self::evict_if_needed(&mut cache);
        cache.insert(
            url.to_string(),
            TimeRangeItem {
                range,
                cached_at: Instant::now(),
            },
        );
    }

    pub async fn get(&self, url: &str) -> Option<(String, String)> {
        let mut cache = self.cache.lock().await;
        if let Some(item) = cache.get(url) {
            if item.cached_at.elapsed() < self.ttl {
                return Some(item.range.clone());
            }
            cache.remove(url);
        }
        None
    }
}

pub static TIME_RANGE_CACHE: std::sync::LazyLock<TimeRangeCache> =
    std::sync::LazyLock::new(|| TimeRangeCache::new(Duration::from_secs(3600)));

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
/// No TTL needed — entries are ephemeral per-session and the map is bounded.
pub struct BurnSubLangCache {
    cache: Mutex<HashMap<String, String>>,
}

/// Maximum entries in the burn subtitle language cache
const MAX_BURN_SUB_LANG_CACHE_SIZE: usize = 500;

impl Default for BurnSubLangCache {
    fn default() -> Self {
        Self::new()
    }
}

impl BurnSubLangCache {
    pub fn new() -> Self {
        Self {
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Store the subtitle language for a URL, or remove it if `lang` is `None`.
    pub async fn set(&self, url: &str, lang: Option<String>) {
        let mut cache = self.cache.lock().await;
        match lang {
            Some(l) => {
                // Simple eviction: if full, clear the oldest half
                if cache.len() >= MAX_BURN_SUB_LANG_CACHE_SIZE {
                    let keys: Vec<String> = cache.keys().take(MAX_BURN_SUB_LANG_CACHE_SIZE / 2).cloned().collect();
                    for k in &keys {
                        cache.remove(k);
                    }
                }
                cache.insert(url.to_string(), l);
            }
            None => {
                cache.remove(url);
            }
        }
    }

    /// Get the cached subtitle language for a URL, if any.
    pub async fn get(&self, url: &str) -> Option<String> {
        self.cache.lock().await.get(url).cloned()
    }
}

pub static BURN_SUB_LANG_CACHE: std::sync::LazyLock<BurnSubLangCache> = std::sync::LazyLock::new(BurnSubLangCache::new);

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
