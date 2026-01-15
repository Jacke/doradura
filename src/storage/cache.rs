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
    #[allow(dead_code)]
    thumbnail_url: Option<String>,
    #[allow(dead_code)]
    duration: Option<u32>,
    #[allow(dead_code)]
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
        // Не кэшируем "Unknown Track", пустые значения или "NA" в artist
        if title.trim().is_empty() || title.trim() == "Unknown Track" {
            log::warn!("Not caching invalid metadata: title='{}'", title);
            return;
        }

        // Если artist "NA" или пустой - не кэшируем, чтобы не сохранять плохие данные
        if artist.trim() == "NA" || artist.trim().is_empty() {
            log::debug!("Not caching metadata with NA/empty artist for URL: {}", url);
            return;
        }

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
        // Не кэшируем "Unknown Track" или пустые значения
        if title.trim().is_empty() || title.trim() == "Unknown Track" {
            log::warn!("Not caching invalid extended metadata: title='{}'", title);
            return;
        }

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

use crate::storage::db::{get_connection, DbPool};

/// Генерирует короткий ID из URL (первые 12 символов хеша)
fn generate_url_id(url: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    url.hash(&mut hasher);
    let hash = hasher.finish();
    format!("{:x}", hash)[..12].to_string()
}

/// Сохраняет URL в БД и возвращает короткий ID для использования в callback_data
///
/// URL сохраняется в таблице url_cache с TTL 7 дней.
/// Это позволяет кнопкам работать даже после рестарта бота.
pub async fn store_url(db_pool: &DbPool, url: &str) -> String {
    let id = generate_url_id(url);
    let ttl_seconds = 7 * 24 * 60 * 60; // 7 дней - достаточно долго для работы кнопок после рестарта

    // Вычисляем expires_at
    let expires_at = chrono::Utc::now() + chrono::Duration::seconds(ttl_seconds);
    let expires_at_str = expires_at.format("%Y-%m-%d %H:%M:%S").to_string();

    // Сохраняем в БД (INSERT OR REPLACE для обновления существующих записей)
    match get_connection(db_pool) {
        Ok(conn) => {
            if let Err(e) = conn.execute(
                "INSERT OR REPLACE INTO url_cache (id, url, expires_at) VALUES (?1, ?2, ?3)",
                rusqlite::params![id, url, expires_at_str],
            ) {
                log::warn!("Failed to store URL in cache: {}", e);
            } else {
                log::debug!("Stored URL in DB cache: {} -> {}", id, url);
            }
        }
        Err(e) => {
            log::warn!("Failed to get DB connection for URL cache: {}", e);
        }
    }

    id
}

/// Получает URL по короткому ID из БД
///
/// Возвращает None если ID не найден или запись устарела.
pub async fn get_url(db_pool: &DbPool, id: &str) -> Option<String> {
    match get_connection(db_pool) {
        Ok(conn) => {
            let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();

            match conn.query_row(
                "SELECT url FROM url_cache WHERE id = ?1 AND expires_at > ?2",
                rusqlite::params![id, now],
                |row| row.get::<_, String>(0),
            ) {
                Ok(url) => {
                    log::debug!("Retrieved URL from DB cache: {} -> {}", id, url);
                    Some(url)
                }
                Err(rusqlite::Error::QueryReturnedNoRows) => {
                    log::debug!("URL not found in DB cache for ID: {}", id);
                    None
                }
                Err(e) => {
                    log::warn!("Failed to get URL from cache: {}", e);
                    None
                }
            }
        }
        Err(e) => {
            log::warn!("Failed to get DB connection for URL cache: {}", e);
            None
        }
    }
}

/// Очищает устаревшие записи из URL кеша в БД
pub async fn cleanup_url_cache(db_pool: &DbPool) -> usize {
    match get_connection(db_pool) {
        Ok(conn) => {
            let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();

            match conn.execute("DELETE FROM url_cache WHERE expires_at <= ?1", rusqlite::params![now]) {
                Ok(removed) => {
                    if removed > 0 {
                        log::debug!("Cleaned up {} expired URL cache entries from DB", removed);
                    }
                    removed
                }
                Err(e) => {
                    log::warn!("Failed to cleanup URL cache: {}", e);
                    0
                }
            }
        }
        Err(e) => {
            log::warn!("Failed to get DB connection for URL cache cleanup: {}", e);
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_metadata_cache_new() {
        let cache = MetadataCache::new(Duration::from_secs(60));
        let stats = cache.stats().await;
        assert_eq!(stats.size, 0);
        assert_eq!(stats.hits, 0);
        assert_eq!(stats.misses, 0);
    }

    #[tokio::test]
    async fn test_metadata_cache_set_and_get() {
        let cache = MetadataCache::new(Duration::from_secs(60));
        let url = Url::parse("https://youtube.com/watch?v=test123").unwrap();

        // Set metadata
        cache
            .set(&url, "Test Song".to_string(), "Test Artist".to_string())
            .await;

        // Get metadata
        let result = cache.get(&url).await;
        assert!(result.is_some());
        let (title, artist) = result.unwrap();
        assert_eq!(title, "Test Song");
        assert_eq!(artist, "Test Artist");
    }

    #[tokio::test]
    async fn test_metadata_cache_miss() {
        let cache = MetadataCache::new(Duration::from_secs(60));
        let url = Url::parse("https://youtube.com/watch?v=nonexistent").unwrap();

        let result = cache.get(&url).await;
        assert!(result.is_none());

        let stats = cache.stats().await;
        assert_eq!(stats.misses, 1);
    }

    #[tokio::test]
    async fn test_metadata_cache_hit_miss_counts() {
        let cache = MetadataCache::new(Duration::from_secs(60));
        let url = Url::parse("https://youtube.com/watch?v=test456").unwrap();

        // Miss
        cache.get(&url).await;

        // Set and hit
        cache.set(&url, "Song".to_string(), "Artist".to_string()).await;
        cache.get(&url).await;
        cache.get(&url).await;

        let stats = cache.stats().await;
        assert_eq!(stats.misses, 1);
        assert_eq!(stats.hits, 2);
    }

    #[tokio::test]
    async fn test_metadata_cache_ttl_expiration() {
        let cache = MetadataCache::new(Duration::from_millis(50));
        let url = Url::parse("https://youtube.com/watch?v=expire_test").unwrap();

        cache.set(&url, "Expiring Song".to_string(), "Artist".to_string()).await;

        // Should be present immediately
        let result = cache.get(&url).await;
        assert!(result.is_some());

        // Wait for TTL to expire
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Should be expired now
        let result = cache.get(&url).await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_metadata_cache_cleanup() {
        let cache = MetadataCache::new(Duration::from_millis(50));

        let url1 = Url::parse("https://youtube.com/watch?v=cleanup1").unwrap();
        let url2 = Url::parse("https://youtube.com/watch?v=cleanup2").unwrap();

        cache.set(&url1, "Song 1".to_string(), "Artist 1".to_string()).await;
        cache.set(&url2, "Song 2".to_string(), "Artist 2".to_string()).await;

        assert_eq!(cache.stats().await.size, 2);

        // Wait for expiration
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Cleanup should remove expired entries
        let removed = cache.cleanup().await;
        assert_eq!(removed, 2);
        assert_eq!(cache.stats().await.size, 0);
    }

    #[tokio::test]
    async fn test_metadata_cache_clear() {
        let cache = MetadataCache::new(Duration::from_secs(60));

        let url = Url::parse("https://youtube.com/watch?v=clear_test").unwrap();
        cache.set(&url, "Song".to_string(), "Artist".to_string()).await;
        cache.get(&url).await;

        assert_eq!(cache.stats().await.size, 1);
        assert_eq!(cache.stats().await.hits, 1);

        cache.clear().await;

        let stats = cache.stats().await;
        assert_eq!(stats.size, 0);
        assert_eq!(stats.hits, 0);
        assert_eq!(stats.misses, 0);
    }

    #[tokio::test]
    async fn test_metadata_cache_does_not_cache_invalid_title() {
        let cache = MetadataCache::new(Duration::from_secs(60));
        let url = Url::parse("https://youtube.com/watch?v=invalid_title").unwrap();

        // Empty title should not be cached
        cache.set(&url, "".to_string(), "Artist".to_string()).await;
        assert!(cache.get(&url).await.is_none());

        // "Unknown Track" should not be cached
        cache.set(&url, "Unknown Track".to_string(), "Artist".to_string()).await;
        assert!(cache.get(&url).await.is_none());
    }

    #[tokio::test]
    async fn test_metadata_cache_does_not_cache_na_artist() {
        let cache = MetadataCache::new(Duration::from_secs(60));
        let url = Url::parse("https://youtube.com/watch?v=na_artist").unwrap();

        // "NA" artist should not be cached
        cache.set(&url, "Good Song".to_string(), "NA".to_string()).await;
        assert!(cache.get(&url).await.is_none());

        // Empty artist should not be cached
        cache.set(&url, "Good Song".to_string(), "".to_string()).await;
        assert!(cache.get(&url).await.is_none());
    }

    #[tokio::test]
    async fn test_metadata_cache_set_extended() {
        let cache = MetadataCache::new(Duration::from_secs(60));
        let url = Url::parse("https://youtube.com/watch?v=extended").unwrap();

        cache
            .set_extended(
                &url,
                "Extended Song".to_string(),
                "Artist".to_string(),
                Some("https://example.com/thumb.jpg".to_string()),
                Some(180),
                Some(5000000),
            )
            .await;

        let result = cache.get(&url).await;
        assert!(result.is_some());
        let (title, artist) = result.unwrap();
        assert_eq!(title, "Extended Song");
        assert_eq!(artist, "Artist");
    }

    #[tokio::test]
    async fn test_metadata_cache_hit_rate() {
        let cache = MetadataCache::new(Duration::from_secs(60));
        let url = Url::parse("https://youtube.com/watch?v=hit_rate").unwrap();

        // 1 miss
        cache.get(&url).await;

        // Set and get 3 times (3 hits)
        cache.set(&url, "Song".to_string(), "Artist".to_string()).await;
        cache.get(&url).await;
        cache.get(&url).await;
        cache.get(&url).await;

        let stats = cache.stats().await;
        // 3 hits, 1 miss = 75% hit rate
        assert_eq!(stats.hits, 3);
        assert_eq!(stats.misses, 1);
        assert!((stats.hit_rate - 75.0).abs() < 0.01);
    }

    #[test]
    fn test_generate_url_id() {
        let id1 = generate_url_id("https://youtube.com/watch?v=abc123");
        let id2 = generate_url_id("https://youtube.com/watch?v=def456");

        // IDs should be 12 characters
        assert_eq!(id1.len(), 12);
        assert_eq!(id2.len(), 12);

        // Different URLs should generate different IDs
        assert_ne!(id1, id2);

        // Same URL should generate same ID
        let id1_again = generate_url_id("https://youtube.com/watch?v=abc123");
        assert_eq!(id1, id1_again);
    }

    #[test]
    fn test_generate_url_id_deterministic() {
        let url = "https://example.com/video/12345";
        let id1 = generate_url_id(url);
        let id2 = generate_url_id(url);
        let id3 = generate_url_id(url);

        assert_eq!(id1, id2);
        assert_eq!(id2, id3);
    }

    #[test]
    fn test_cache_stats_debug() {
        let stats = CacheStats {
            size: 10,
            hits: 100,
            misses: 20,
            hit_rate: 83.33,
        };
        let debug_str = format!("{:?}", stats);
        assert!(debug_str.contains("CacheStats"));
        assert!(debug_str.contains("10"));
        assert!(debug_str.contains("100"));
    }

    #[test]
    fn test_cache_stats_clone() {
        let stats = CacheStats {
            size: 5,
            hits: 50,
            misses: 10,
            hit_rate: 83.33,
        };
        let cloned = stats.clone();
        assert_eq!(stats.size, cloned.size);
        assert_eq!(stats.hits, cloned.hits);
        assert_eq!(stats.misses, cloned.misses);
    }
}
