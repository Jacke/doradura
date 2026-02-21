//! Disk-based cache for subtitle files (SRT, TXT).
//!
//! Cache files are stored as `{cache_dir}/{sha256(url+lang)}.{format}`.
//! No TTL — subtitle content is considered stable once fetched.

use sha2::{Digest, Sha256};
use std::path::PathBuf;
use tokio::fs;

pub struct SubtitleCache {
    cache_dir: PathBuf,
}

impl SubtitleCache {
    pub fn new(base_dir: &str) -> Self {
        let expanded = shellexpand::tilde(base_dir).into_owned();
        Self {
            cache_dir: PathBuf::from(expanded),
        }
    }

    pub async fn get(&self, url: &str, lang: &str, format: &str) -> Option<String> {
        let path = self.cache_path(url, lang, format);
        match fs::read_to_string(&path).await {
            Ok(content) => {
                log::debug!("SubtitleCache: hit {:?}", path);
                Some(content)
            }
            Err(_) => None,
        }
    }

    pub async fn save(&self, url: &str, lang: &str, format: &str, content: &str) {
        if let Err(e) = fs::create_dir_all(&self.cache_dir).await {
            log::warn!("SubtitleCache: failed to create dir {:?}: {}", self.cache_dir, e);
            return;
        }
        let path = self.cache_path(url, lang, format);
        if let Err(e) = fs::write(&path, content).await {
            log::warn!("SubtitleCache: failed to write {:?}: {}", path, e);
        } else {
            log::debug!("SubtitleCache: saved {:?}", path);
        }
    }

    fn cache_key(url: &str, lang: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(url.as_bytes());
        hasher.update(b"|");
        hasher.update(lang.as_bytes());
        hex::encode(hasher.finalize())
    }

    fn cache_path(&self, url: &str, lang: &str, format: &str) -> PathBuf {
        self.cache_dir
            .join(format!("{}.{}", Self::cache_key(url, lang), format))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_cache() -> (TempDir, SubtitleCache) {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let cache = SubtitleCache::new(dir.path().to_str().unwrap());
        (dir, cache)
    }

    // ==================== get() ====================

    #[tokio::test]
    async fn test_get_returns_none_on_empty_cache() {
        let (_dir, cache) = make_cache();
        let result = cache.get("https://youtube.com/watch?v=abc", "en", "srt").await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_get_returns_none_for_unknown_format() {
        let (_dir, cache) = make_cache();
        cache
            .save("https://youtube.com/watch?v=abc", "en", "srt", "some content")
            .await;
        // asking for "txt" when only "srt" was saved
        let result = cache.get("https://youtube.com/watch?v=abc", "en", "txt").await;
        assert!(result.is_none());
    }

    // ==================== save() + get() round-trip ====================

    #[tokio::test]
    async fn test_save_and_get_roundtrip() {
        let (_dir, cache) = make_cache();
        let content = "1\n00:00:01,000 --> 00:00:02,000\nHello World\n\n";
        cache
            .save("https://youtube.com/watch?v=abc", "en", "srt", content)
            .await;
        let result = cache.get("https://youtube.com/watch?v=abc", "en", "srt").await;
        assert_eq!(result.as_deref(), Some(content));
    }

    #[tokio::test]
    async fn test_save_creates_missing_directory() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        // Point cache to a nested path that doesn't exist yet
        let nested = dir.path().join("a").join("b").join("c");
        let cache = SubtitleCache::new(nested.to_str().unwrap());
        cache.save("https://youtube.com/watch?v=abc", "", "srt", "hello").await;
        assert!(nested.exists(), "cache directory should be created by save()");
        let result = cache.get("https://youtube.com/watch?v=abc", "", "srt").await;
        assert_eq!(result.as_deref(), Some("hello"));
    }

    #[tokio::test]
    async fn test_overwrite_with_new_content() {
        let (_dir, cache) = make_cache();
        let url = "https://youtube.com/watch?v=abc";
        cache.save(url, "en", "srt", "first version").await;
        cache.save(url, "en", "srt", "second version").await;
        let result = cache.get(url, "en", "srt").await;
        assert_eq!(result.as_deref(), Some("second version"));
    }

    // ==================== format isolation ====================

    #[tokio::test]
    async fn test_srt_and_txt_cached_independently() {
        let (_dir, cache) = make_cache();
        let url = "https://youtube.com/watch?v=abc";
        cache.save(url, "en", "srt", "SRT content").await;
        cache.save(url, "en", "txt", "TXT content").await;

        assert_eq!(cache.get(url, "en", "srt").await.as_deref(), Some("SRT content"));
        assert_eq!(cache.get(url, "en", "txt").await.as_deref(), Some("TXT content"));
    }

    // ==================== lang isolation ====================

    #[tokio::test]
    async fn test_different_langs_cached_separately() {
        let (_dir, cache) = make_cache();
        let url = "https://youtube.com/watch?v=abc";
        cache.save(url, "en", "srt", "English subtitles").await;
        cache.save(url, "ru", "srt", "Russian subtitles").await;

        assert_eq!(cache.get(url, "en", "srt").await.as_deref(), Some("English subtitles"));
        assert_eq!(cache.get(url, "ru", "srt").await.as_deref(), Some("Russian subtitles"));
        // Different lang: no match
        assert!(cache.get(url, "de", "srt").await.is_none());
    }

    // ==================== cache_key determinism ====================

    #[test]
    fn test_cache_key_is_deterministic() {
        let key1 = SubtitleCache::cache_key("https://youtube.com/watch?v=abc", "en");
        let key2 = SubtitleCache::cache_key("https://youtube.com/watch?v=abc", "en");
        assert_eq!(key1, key2);
    }

    #[test]
    fn test_cache_key_differs_by_url() {
        let key1 = SubtitleCache::cache_key("https://youtube.com/watch?v=aaa", "en");
        let key2 = SubtitleCache::cache_key("https://youtube.com/watch?v=bbb", "en");
        assert_ne!(key1, key2);
    }

    #[test]
    fn test_cache_key_differs_by_lang() {
        let key1 = SubtitleCache::cache_key("https://youtube.com/watch?v=abc", "en");
        let key2 = SubtitleCache::cache_key("https://youtube.com/watch?v=abc", "ru");
        assert_ne!(key1, key2);
    }

    #[test]
    fn test_cache_key_is_hex_sha256() {
        let key = SubtitleCache::cache_key("https://youtube.com/watch?v=abc", "en");
        // SHA-256 hex = 64 chars, all hex digits
        assert_eq!(key.len(), 64);
        assert!(key.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_cache_key_empty_lang_differs_from_nonempty() {
        let key_no_lang = SubtitleCache::cache_key("https://youtube.com/watch?v=abc", "");
        let key_en = SubtitleCache::cache_key("https://youtube.com/watch?v=abc", "en");
        assert_ne!(key_no_lang, key_en);
    }
}
