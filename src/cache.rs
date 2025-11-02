use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{Duration, Instant};
use url::Url;

/// Структура для хранения метаданных в кэше
#[derive(Debug, Clone)]
struct CachedMetadata {
    title: String,
    artist: String,
    thumbnail_url: Option<String>,
    duration: Option<u32>,
    filesize: Option<u64>,
    cached_at: Instant,
}

/// Кэш метаданных с TTL
pub struct MetadataCache {
    cache: Arc<Mutex<HashMap<String, CachedMetadata>>>,
    ttl: Duration,
    hit_count: Arc<Mutex<u64>>,
    miss_count: Arc<Mutex<u64>>,
}

impl MetadataCache {
    /// Создает новый кэш с указанным TTL
    pub fn new(ttl: Duration) -> Self {
        Self {
            cache: Arc::new(Mutex::new(HashMap::new())),
            ttl,
            hit_count: Arc::new(Mutex::new(0)),
            miss_count: Arc::new(Mutex::new(0)),
        }
    }

    /// Получает метаданные из кэша или возвращает None если их нет или они устарели
    pub async fn get(&self, url: &Url) -> Option<(String, String)> {
        let url_str = url.as_str();
        let mut cache = self.cache.lock().await;
        
        if let Some(cached) = cache.get(url_str) {
            // Проверяем, не устарел ли кэш
            if Instant::now().duration_since(cached.cached_at) < self.ttl {
                *self.hit_count.lock().await += 1;
                return Some((cached.title.clone(), cached.artist.clone()));
            } else {
                // Удаляем устаревший кэш
                cache.remove(url_str);
            }
        }
        
        *self.miss_count.lock().await += 1;
        None
    }

    /// Сохраняет метаданные в кэш
    pub async fn set(&self, url: &Url, title: String, artist: String) {
        let url_str = url.as_str();
        let mut cache = self.cache.lock().await;
        
        cache.insert(
            url_str.to_string(),
            CachedMetadata {
                title,
                artist,
                thumbnail_url: None,
                duration: None,
                filesize: None,
                cached_at: Instant::now(),
            },
        );
    }

    /// Сохраняет расширенные метаданные в кэш
    pub async fn set_extended(
        &self,
        url: &Url,
        title: String,
        artist: String,
        thumbnail_url: Option<String>,
        duration: Option<u32>,
        filesize: Option<u64>,
    ) {
        let url_str = url.as_str();
        let mut cache = self.cache.lock().await;
        
        cache.insert(
            url_str.to_string(),
            CachedMetadata {
                title,
                artist,
                thumbnail_url,
                duration,
                filesize,
                cached_at: Instant::now(),
            },
        );
    }

    /// Очищает устаревшие записи из кэша
    pub async fn cleanup(&self) -> usize {
        let mut cache = self.cache.lock().await;
        let before = cache.len();
        cache.retain(|_, cached| Instant::now().duration_since(cached.cached_at) < self.ttl);
        let removed = before - cache.len();
        log::debug!("Cleaned up {} expired cache entries", removed);
        removed
    }

    /// Получает статистику кэша
    pub async fn stats(&self) -> CacheStats {
        let cache = self.cache.lock().await;
        let hits = *self.hit_count.lock().await;
        let misses = *self.miss_count.lock().await;
        let total = hits + misses;
        let hit_rate = if total > 0 {
            (hits as f64 / total as f64) * 100.0
        } else {
            0.0
        };
        
        CacheStats {
            size: cache.len(),
            hits,
            misses,
            hit_rate,
        }
    }

    /// Очищает весь кэш
    pub async fn clear(&self) {
        let mut cache = self.cache.lock().await;
        cache.clear();
        *self.hit_count.lock().await = 0;
        *self.miss_count.lock().await = 0;
        log::info!("Cache cleared");
    }
}

/// Статистика кэша
#[derive(Debug, Clone)]
pub struct CacheStats {
    pub size: usize,
    pub hits: u64,
    pub misses: u64,
    pub hit_rate: f64,
}

/// Глобальный экземпляр кэша (singleton)
static METADATA_CACHE: once_cell::sync::Lazy<MetadataCache> = once_cell::sync::Lazy::new(|| {
    MetadataCache::new(Duration::from_secs(24 * 60 * 60)) // 24 часа
});

/// Получает метаданные из кэша или None
pub async fn get_cached_metadata(url: &Url) -> Option<(String, String)> {
    METADATA_CACHE.get(url).await
}

/// Сохраняет метаданные в кэш
pub async fn cache_metadata(url: &Url, title: String, artist: String) {
    METADATA_CACHE.set(url, title, artist).await;
}

/// Сохраняет расширенные метаданные в кэш
pub async fn cache_extended_metadata(
    url: &Url,
    title: String,
    artist: String,
    thumbnail_url: Option<String>,
    duration: Option<u32>,
    filesize: Option<u64>,
) {
    METADATA_CACHE
        .set_extended(url, title, artist, thumbnail_url, duration, filesize)
        .await;
}

/// Получает статистику кэша
pub async fn get_cache_stats() -> CacheStats {
    METADATA_CACHE.stats().await
}

/// Очищает устаревшие записи из кэша
pub async fn cleanup_cache() -> usize {
    METADATA_CACHE.cleanup().await
}

