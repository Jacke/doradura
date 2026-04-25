use moka::future::Cache;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use url::Url;

/// Metadata cache with TTL
/// Uses moka for lock-free concurrent access with built-in LRU eviction.
/// Atomic counters track hit/miss for the CACHE_HIT_RATIO Prometheus metric.
pub struct MetadataCache {
    cache: Cache<String, (String, String)>,
    hit_count: AtomicU64,
    miss_count: AtomicU64,
}

impl MetadataCache {
    /// Creates a new cache with the specified TTL
    pub fn new(ttl: Duration) -> Self {
        Self {
            cache: Cache::builder().max_capacity(10_000).time_to_live(ttl).build(),
            hit_count: AtomicU64::new(0),
            miss_count: AtomicU64::new(0),
        }
    }

    fn update_hit_ratio(&self) {
        let hits = self.hit_count.load(Ordering::Relaxed);
        let misses = self.miss_count.load(Ordering::Relaxed);
        let total = hits + misses;
        if total > 0 {
            let ratio = hits as f64 / total as f64;
            crate::core::metrics::CACHE_HIT_RATIO
                .with_label_values(&["metadata"])
                .set(ratio);
        }
    }

    /// Gets metadata from the cache or returns None if absent or expired.
    pub async fn get(&self, url: &Url) -> Option<(String, String)> {
        match self.cache.get(url.as_str()).await {
            Some(value) => {
                self.hit_count.fetch_add(1, Ordering::Relaxed);
                self.update_hit_ratio();
                Some(value)
            }
            None => {
                self.miss_count.fetch_add(1, Ordering::Relaxed);
                self.update_hit_ratio();
                None
            }
        }
    }

    /// Stores metadata in the cache, rejecting invalid values.
    pub async fn set(&self, url: &Url, title: String, artist: String) {
        if title.trim().is_empty() || title.trim() == "Unknown Track" {
            log::warn!("Not caching invalid metadata: title='{}'", title);
            return;
        }
        if artist.trim() == "NA" || artist.trim().is_empty() {
            log::debug!("Not caching metadata with NA/empty artist for URL: {}", url);
            return;
        }
        self.cache.insert(url.as_str().to_string(), (title, artist)).await;
    }

    /// Stores extended metadata in the cache.
    ///
    /// Extra parameters (`thumbnail_url`, `duration`, `filesize`) are accepted
    /// for API compatibility but not persisted — only `title` and `artist` are cached.
    pub async fn set_extended(
        &self,
        url: &Url,
        title: String,
        artist: String,
        _thumbnail_url: Option<String>,
        _duration: Option<u32>,
        _filesize: Option<u64>,
    ) {
        if title.trim().is_empty() || title.trim() == "Unknown Track" {
            log::warn!("Not caching invalid extended metadata: title='{}'", title);
            return;
        }
        self.cache.insert(url.as_str().to_string(), (title, artist)).await;
    }

    /// Triggers moka's internal eviction pass. moka evicts TTL-expired entries
    /// lazily on access, so explicit cleanup is rarely needed.
    pub async fn cleanup(&self) -> usize {
        let before = self.cache.entry_count();
        self.cache.run_pending_tasks().await;
        let after = self.cache.entry_count();
        let removed = before.saturating_sub(after) as usize;
        log::debug!("Cleaned up {} expired cache entries", removed);
        removed
    }

    /// Gets cache statistics.
    pub async fn stats(&self) -> CacheStats {
        self.cache.run_pending_tasks().await;
        let hits = self.hit_count.load(Ordering::Relaxed);
        let misses = self.miss_count.load(Ordering::Relaxed);
        let total = hits + misses;
        let hit_rate = if total > 0 {
            (hits as f64 / total as f64) * 100.0
        } else {
            0.0
        };
        CacheStats {
            size: self.cache.entry_count() as usize,
            hits,
            misses,
            hit_rate,
        }
    }

    /// Clears the entire cache and resets counters.
    pub async fn clear(&self) {
        self.cache.invalidate_all();
        self.cache.run_pending_tasks().await;
        self.hit_count.store(0, Ordering::Relaxed);
        self.miss_count.store(0, Ordering::Relaxed);
        log::info!("Cache cleared");
    }
}

/// Cache statistics
#[derive(Debug, Clone)]
pub struct CacheStats {
    pub size: usize,
    pub hits: u64,
    pub misses: u64,
    pub hit_rate: f64,
}

/// Global cache instance (singleton)
static METADATA_CACHE: std::sync::LazyLock<MetadataCache> = std::sync::LazyLock::new(|| {
    MetadataCache::new(Duration::from_secs(24 * 60 * 60)) // 24 hours
});

/// Gets metadata from the cache or None
pub async fn get_cached_metadata(url: &Url) -> Option<(String, String)> {
    METADATA_CACHE.get(url).await
}

/// Saves metadata to the cache
pub async fn cache_metadata(url: &Url, title: String, artist: String) {
    METADATA_CACHE.set(url, title, artist).await;
}

/// Saves extended metadata to the cache
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

/// Gets cache statistics
pub async fn get_cache_stats() -> CacheStats {
    METADATA_CACHE.stats().await
}

/// Clears expired entries from the cache
pub async fn cleanup_cache() -> usize {
    METADATA_CACHE.cleanup().await
}

use crate::storage::SharedStorage;
use crate::storage::db::{DbPool, get_connection};

/// Generates a short ID from a URL (first 12 characters of hash)
fn generate_url_id(url: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    url.hash(&mut hasher);
    let hash = hasher.finish();
    format!("{:x}", hash)[..12].to_string()
}

/// Stores a URL in the DB and returns a short ID for use in callback_data
///
/// The URL is stored in the url_cache table with a 7-day TTL.
/// This allows buttons to work even after a bot restart.
pub async fn store_url(db_pool: &DbPool, shared_storage: Option<&SharedStorage>, url: &str) -> String {
    let id = generate_url_id(url);
    let ttl_seconds = 7 * 24 * 60 * 60; // 7 days - long enough for buttons to work after restart

    // Calculate expires_at
    let expires_at = chrono::Utc::now() + chrono::Duration::seconds(ttl_seconds);
    let expires_at_str = expires_at.format("%Y-%m-%d %H:%M:%S").to_string();

    if let Some(storage) = shared_storage {
        match storage.store_cached_url(&id, url, &expires_at_str).await {
            Err(e) => {
                log::error!("Failed to store URL in shared cache: {}", e);
            }
            _ => {
                log::debug!("Stored URL in shared cache: {} -> {}", id, url);
            }
        }
        return id;
    }

    match get_connection(db_pool) {
        Ok(conn) => {
            match conn.execute(
                "INSERT OR REPLACE INTO url_cache (id, url, expires_at) VALUES (?1, ?2, ?3)",
                rusqlite::params![id, url, expires_at_str],
            ) {
                Err(e) => {
                    log::warn!("Failed to store URL in cache: {}", e);
                }
                _ => {
                    log::debug!("Stored URL in DB cache: {} -> {}", id, url);
                }
            }
        }
        Err(e) => {
            log::warn!("Failed to get DB connection for URL cache: {}", e);
        }
    }

    id
}

/// Gets a URL by its short ID from the DB
///
/// Returns None if the ID is not found or the record has expired.
pub async fn get_url(db_pool: &DbPool, shared_storage: Option<&SharedStorage>, id: &str) -> Option<String> {
    if let Some(storage) = shared_storage {
        match storage.get_cached_url(id).await {
            Ok(Some(url)) => {
                log::debug!("Retrieved URL from shared cache: {} -> {}", id, url);
                return Some(url);
            }
            Ok(None) => {
                log::debug!("URL not found in shared cache for ID: {}", id);
                // Fall through to SQLite lookup below
            }
            Err(e) => {
                log::warn!("Failed to get URL from shared cache: {}, falling back to SQLite", e);
                // Fall through to SQLite lookup below
            }
        }
    }

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

/// Clears expired entries from the URL cache in the DB
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
