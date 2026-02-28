//! Vlipsy API client for searching and downloading short video reactions/memes.
//!
//! All functionality is gated behind the `VLIPSY_API_KEY` environment variable.
//! If the key is not set, `is_available()` returns false and all features are disabled.

use reqwest::Client;
use serde::Deserialize;
use std::sync::LazyLock;

const BASE_URL: &str = "https://apiv2.vlipsy.com/v1";

static VLIPSY_API_KEY: LazyLock<Option<String>> =
    LazyLock::new(|| std::env::var("VLIPSY_API_KEY").ok().filter(|s| !s.is_empty()));

/// Returns true if Vlipsy API key is configured.
pub fn is_available() -> bool {
    VLIPSY_API_KEY.is_some()
}

/// A single media variant (e.g. mp4, gif, thumbnail).
#[derive(Debug, Clone, Deserialize)]
pub struct VlipMedia {
    pub url: String,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
}

/// Set of media variants for a clip.
#[derive(Debug, Clone, Deserialize)]
pub struct VlipMediaSet {
    #[serde(default)]
    pub mp4: Option<VlipMedia>,
    #[serde(default)]
    pub gif: Option<VlipMedia>,
    #[serde(default)]
    pub thumbnail: Option<VlipMedia>,
    #[serde(default, rename = "webm")]
    pub webm: Option<VlipMedia>,
}

/// A Vlipsy clip.
#[derive(Debug, Clone, Deserialize)]
pub struct Vlip {
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub slug: Option<String>,
    #[serde(default, rename = "from")]
    pub source: Option<String>,
    #[serde(default)]
    pub media: Option<VlipMediaSet>,
    #[serde(default)]
    pub duration: Option<f64>,
}

impl Vlip {
    /// Get the best available MP4 URL.
    pub fn mp4_url(&self) -> Option<&str> {
        self.media.as_ref()?.mp4.as_ref().map(|m| m.url.as_str())
    }

    /// Get the thumbnail URL.
    pub fn thumb_url(&self) -> Option<&str> {
        self.media.as_ref()?.thumbnail.as_ref().map(|m| m.url.as_str())
    }

    /// Get a display title, falling back to slug or ID.
    pub fn display_title(&self) -> &str {
        self.title.as_deref().or(self.slug.as_deref()).unwrap_or(&self.id)
    }
}

/// Response from search/trending endpoints.
#[derive(Debug, Clone, Deserialize)]
pub struct SearchResponse {
    #[serde(default)]
    pub results: Vec<Vlip>,
    #[serde(default)]
    pub total: Option<u64>,
}

/// Response from get-by-id endpoint.
#[derive(Debug, Clone, Deserialize)]
pub struct SingleVlipResponse {
    #[serde(default, alias = "result")]
    pub vlip: Option<Vlip>,
}

/// Vlipsy API client.
pub struct VlipsyClient {
    client: Client,
    api_key: String,
}

impl VlipsyClient {
    /// Create a new client. Returns None if API key is not configured.
    pub fn new() -> Option<Self> {
        let api_key = VLIPSY_API_KEY.as_ref()?.clone();
        let client = Client::builder()
            .user_agent("doradura/0.14")
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .ok()?;
        Some(Self { client, api_key })
    }

    /// Search for clips by query.
    pub async fn search(&self, query: &str, limit: u32, offset: u32) -> Result<SearchResponse, String> {
        let url = format!("{}/search", BASE_URL);
        let resp = self
            .client
            .get(&url)
            .header("X-Api-Key", &self.api_key)
            .query(&[
                ("q", query),
                ("limit", &limit.to_string()),
                ("offset", &offset.to_string()),
            ])
            .send()
            .await
            .map_err(|e| format!("Vlipsy search request failed: {}", e))?;

        if !resp.status().is_success() {
            return Err(format!("Vlipsy API error: HTTP {}", resp.status()));
        }

        resp.json::<SearchResponse>()
            .await
            .map_err(|e| format!("Vlipsy search parse error: {}", e))
    }

    /// Get a single clip by ID.
    pub async fn get_vlip(&self, id: &str) -> Result<SingleVlipResponse, String> {
        let url = format!("{}/vlips/{}", BASE_URL, id);
        let resp = self
            .client
            .get(&url)
            .header("X-Api-Key", &self.api_key)
            .send()
            .await
            .map_err(|e| format!("Vlipsy get_vlip request failed: {}", e))?;

        if !resp.status().is_success() {
            return Err(format!("Vlipsy API error: HTTP {}", resp.status()));
        }

        resp.json::<SingleVlipResponse>()
            .await
            .map_err(|e| format!("Vlipsy get_vlip parse error: {}", e))
    }

    /// Get trending clips.
    pub async fn trending(&self, limit: u32, offset: u32) -> Result<SearchResponse, String> {
        let url = format!("{}/trending", BASE_URL);
        let resp = self
            .client
            .get(&url)
            .header("X-Api-Key", &self.api_key)
            .query(&[("limit", &limit.to_string()), ("offset", &offset.to_string())])
            .send()
            .await
            .map_err(|e| format!("Vlipsy trending request failed: {}", e))?;

        if !resp.status().is_success() {
            return Err(format!("Vlipsy API error: HTTP {}", resp.status()));
        }

        resp.json::<SearchResponse>()
            .await
            .map_err(|e| format!("Vlipsy trending parse error: {}", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_available_without_key() {
        // In test environment, VLIPSY_API_KEY is typically not set
        // Just verify the function doesn't panic
        let _ = is_available();
    }

    #[test]
    fn test_deserialize_search_response() {
        let json = r#"{
            "results": [
                {
                    "id": "z2wRJ7aR",
                    "title": "Sus Dog",
                    "slug": "sus-dog-original",
                    "from": "Internet",
                    "media": {
                        "mp4": { "url": "https://cdn.vlipsy.com/clips/z2wRJ7aR.mp4", "width": 480, "height": 270 },
                        "gif": { "url": "https://cdn.vlipsy.com/clips/z2wRJ7aR.gif" },
                        "thumbnail": { "url": "https://cdn.vlipsy.com/clips/z2wRJ7aR.jpg", "width": 480, "height": 270 }
                    },
                    "duration": 3.5
                }
            ],
            "total": 42
        }"#;

        let resp: SearchResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.results.len(), 1);
        assert_eq!(resp.total, Some(42));

        let vlip = &resp.results[0];
        assert_eq!(vlip.id, "z2wRJ7aR");
        assert_eq!(vlip.display_title(), "Sus Dog");
        assert_eq!(vlip.source.as_deref(), Some("Internet"));
        assert!(vlip.mp4_url().is_some());
        assert!(vlip.thumb_url().is_some());
        assert_eq!(vlip.duration, Some(3.5));
    }

    #[test]
    fn test_deserialize_single_vlip_response() {
        let json = r#"{
            "result": {
                "id": "abc123",
                "title": "Test Clip",
                "media": {
                    "mp4": { "url": "https://cdn.vlipsy.com/clips/abc123.mp4" }
                }
            }
        }"#;

        let resp: SingleVlipResponse = serde_json::from_str(json).unwrap();
        assert!(resp.vlip.is_some());
        let vlip = resp.vlip.unwrap();
        assert_eq!(vlip.id, "abc123");
        assert_eq!(vlip.mp4_url(), Some("https://cdn.vlipsy.com/clips/abc123.mp4"));
    }

    #[test]
    fn test_deserialize_empty_response() {
        let json = r#"{ "results": [], "total": 0 }"#;
        let resp: SearchResponse = serde_json::from_str(json).unwrap();
        assert!(resp.results.is_empty());
        assert_eq!(resp.total, Some(0));
    }

    #[test]
    fn test_vlip_display_title_fallbacks() {
        // Title present
        let v = Vlip {
            id: "id1".into(),
            title: Some("My Title".into()),
            slug: Some("my-slug".into()),
            source: None,
            media: None,
            duration: None,
        };
        assert_eq!(v.display_title(), "My Title");

        // No title, falls back to slug
        let v2 = Vlip {
            id: "id2".into(),
            title: None,
            slug: Some("fallback-slug".into()),
            source: None,
            media: None,
            duration: None,
        };
        assert_eq!(v2.display_title(), "fallback-slug");

        // No title or slug, falls back to ID
        let v3 = Vlip {
            id: "id3".into(),
            title: None,
            slug: None,
            source: None,
            media: None,
            duration: None,
        };
        assert_eq!(v3.display_title(), "id3");
    }

    #[test]
    fn test_vlipsy_client_new_without_key() {
        // Without VLIPSY_API_KEY set, client creation may return None
        // (depends on test environment)
        let _ = VlipsyClient::new();
    }
}
