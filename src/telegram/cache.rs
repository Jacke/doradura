use crate::telegram::types::PreviewMetadata;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{Duration, Instant};

/// Структура для хранения кэшированных данных
struct CachedItem {
    data: PreviewMetadata,
    cached_at: Instant,
}

/// Кэш для PreviewMetadata
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

    pub async fn set(&self, key: String, data: PreviewMetadata) {
        let mut cache = self.cache.lock().await;
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
        cache.retain(|_, item| item.cached_at.elapsed() < self.ttl);
    }
}

/// Глобальный экземпляр кэша превью (singleton)
/// TTL = 1 час (достаточно для сессии пользователя)
pub static PREVIEW_CACHE: once_cell::sync::Lazy<PreviewCache> =
    once_cell::sync::Lazy::new(|| PreviewCache::new(Duration::from_secs(3600)));
