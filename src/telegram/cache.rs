use crate::telegram::types::PreviewMetadata;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{Duration, Instant};

/// Maximum number of entries in the preview cache
const MAX_PREVIEW_CACHE_SIZE: usize = 5_000;

/// Number of entries to evict when cache is full
const PREVIEW_EVICTION_BATCH: usize = 500;

/// Структура для хранения кэшированных данных
struct CachedItem {
    data: PreviewMetadata,
    cached_at: Instant,
}

/// Кэш для PreviewMetadata with size limit
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

    /// Очистка устаревших записей
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

/// Глобальный экземпляр кэша превью (singleton)
/// TTL = 1 час (достаточно для сессии пользователя)
pub static PREVIEW_CACHE: once_cell::sync::Lazy<PreviewCache> =
    once_cell::sync::Lazy::new(|| PreviewCache::new(Duration::from_secs(3600)));

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

pub static LINK_MESSAGE_CACHE: once_cell::sync::Lazy<LinkMessageCache> =
    once_cell::sync::Lazy::new(|| LinkMessageCache::new(Duration::from_secs(3600)));

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

pub static TIME_RANGE_CACHE: once_cell::sync::Lazy<TimeRangeCache> =
    once_cell::sync::Lazy::new(|| TimeRangeCache::new(Duration::from_secs(3600)));

pub async fn store_time_range(url: &str, range: (String, String)) {
    TIME_RANGE_CACHE.set(url, range).await;
}

pub async fn get_time_range(url: &str) -> Option<(String, String)> {
    TIME_RANGE_CACHE.get(url).await
}
