//! InstagramSource — dedicated Instagram download backend using the internal GraphQL API.
//!
//! Provides reliable downloads of reels, posts, and photos by calling Instagram's
//! internal GraphQL endpoint directly. Falls back to yt-dlp on GraphQL failure.
//!
//! Features:
//! - Public posts/reels without login via GraphQL API
//! - `doc_id` is configurable via `INSTAGRAM_DOC_ID` env var (rotates every 2-4 weeks)
//! - WARP proxy support for datacenter IP protection
//! - Internal sliding-window rate limiter (180 req/hr, conservative under 200 limit)
//! - Photo post support (`image/jpeg` mime hint)
//! - Automatic yt-dlp fallback on any GraphQL failure

use crate::core::config;
use crate::core::error::AppError;
use crate::download::error::DownloadError;
use crate::download::source::{DownloadOutput, DownloadRequest, DownloadSource, MediaMetadata, SourceProgress};
use async_trait::async_trait;
use std::collections::HashMap;
use std::io::Write;
use std::sync::Mutex;
use std::time::Instant;
use tokio::sync::mpsc;
use url::Url;

/// Thread-safe cache for carousel download masks.
/// Set by the download dispatcher before starting a carousel download,
/// read by `InstagramSource::download()` to filter items.
static CAROUSEL_MASKS: std::sync::LazyLock<Mutex<HashMap<String, u32>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/// Store a carousel bitmask for a URL (called before dispatching the download).
pub fn set_carousel_mask(url: &str, mask: u32) {
    CAROUSEL_MASKS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(url.to_string(), mask);
}

/// Take (remove) the carousel bitmask for a URL (called during download).
fn take_carousel_mask(url: &str) -> Option<u32> {
    CAROUSEL_MASKS.lock().unwrap_or_else(|e| e.into_inner()).remove(url)
}

/// Instagram GraphQL API endpoint.
const GRAPHQL_ENDPOINT: &str = "https://www.instagram.com/api/graphql";

/// Instagram internal app ID (public, embedded in the web app).
const IG_APP_ID: &str = "936619743392459";

/// Facebook LSD token (anti-CSRF, public static value used by web scrapers).
const FB_LSD_TOKEN: &str = "AVqbxe3J_YA";

/// Facebook ASBD ID (public, embedded in the web app).
const FB_ASBD_ID: &str = "129477";

/// Maximum requests per hour (conservative, under Instagram's ~200 limit).
const RATE_LIMIT_PER_HOUR: usize = 180;

/// Sliding-window rate limiter for Instagram GraphQL API calls.
/// Tracks timestamps of recent requests, global per-IP.
struct RateLimiter {
    timestamps: Mutex<Vec<Instant>>,
}

impl RateLimiter {
    fn new() -> Self {
        Self {
            timestamps: Mutex::new(Vec::new()),
        }
    }

    /// Try to acquire a slot. Returns true if under limit, false if rate-limited.
    fn acquire(&self) -> bool {
        let mut ts = self.timestamps.lock().unwrap_or_else(|e| e.into_inner());
        let cutoff = Instant::now() - std::time::Duration::from_secs(3600);
        ts.retain(|t| *t > cutoff);
        if ts.len() >= RATE_LIMIT_PER_HOUR {
            return false;
        }
        ts.push(Instant::now());
        true
    }
}

/// Instagram download source using the internal GraphQL API.
pub struct InstagramSource {
    client: reqwest::Client,
    rate_limiter: RateLimiter,
}

impl Default for InstagramSource {
    fn default() -> Self {
        Self::new()
    }
}

impl InstagramSource {
    /// Get the curl binary name — prefer curl-impersonate, fall back to curl.
    ///
    /// curl-impersonate mimics Chrome's TLS fingerprint (BoringSSL, cipher suites,
    /// extensions order), bypassing Instagram's JA3 fingerprinting on datacenter IPs.
    /// The lexiforest fork uses a single binary with `--impersonate chrome131` flag.
    fn curl_binary() -> &'static str {
        use std::sync::OnceLock;
        static BINARY: OnceLock<&str> = OnceLock::new();
        BINARY.get_or_init(|| {
            if std::process::Command::new("curl-impersonate")
                .arg("--version")
                .output()
                .is_ok()
            {
                log::info!("InstagramSource: using curl-impersonate for TLS fingerprint spoofing");
                "curl-impersonate"
            } else {
                log::warn!("InstagramSource: curl-impersonate not found, falling back to curl");
                "curl"
            }
        })
    }

    pub fn new() -> Self {
        let mut client_builder = reqwest::Client::builder()
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36")
            .timeout(std::time::Duration::from_secs(60))
            .connect_timeout(std::time::Duration::from_secs(15));

        // Use WARP proxy if available (datacenter IP protection)
        if let Some(ref proxy_url) = *config::proxy::WARP_PROXY {
            let trimmed = proxy_url.trim();
            if !trimmed.is_empty() && trimmed != "none" && trimmed != "disabled" {
                match reqwest::Proxy::all(trimmed) {
                    Ok(proxy) => {
                        log::info!("InstagramSource: using proxy for GraphQL API");
                        client_builder = client_builder.proxy(proxy);
                    }
                    Err(e) => {
                        log::warn!("InstagramSource: failed to configure proxy: {}", e);
                    }
                }
            }
        }

        let client = client_builder
            .build()
            .expect("InstagramSource HTTP client build should succeed");

        Self {
            client,
            rate_limiter: RateLimiter::new(),
        }
    }

    /// Extract the shortcode from an Instagram URL.
    ///
    /// Supports:
    /// - `/p/<code>/`, `/reel/<code>/`, `/reels/<code>/`, `/tv/<code>/`
    /// - `/<username>/p/<code>/`, `/<username>/reel/<code>/` (with username prefix)
    fn extract_shortcode(url: &Url) -> Option<String> {
        let segments: Vec<&str> = url.path_segments()?.filter(|s| !s.is_empty()).collect();
        const CONTENT_TYPES: &[&str] = &["p", "reel", "reels", "tv"];
        // Format: /reel/<code>/ (2 segments)
        if segments.len() >= 2 && CONTENT_TYPES.contains(&segments[0]) {
            return Some(segments[1].to_string());
        }
        // Format: /<username>/reel/<code>/ (3 segments)
        if segments.len() >= 3 && CONTENT_TYPES.contains(&segments[1]) {
            return Some(segments[2].to_string());
        }
        None
    }

    /// Check if URL is an Instagram content URL (post, reel, etc.) — NOT a profile.
    fn is_content_url(url: &Url) -> bool {
        Self::extract_shortcode(url).is_some()
    }

    /// Public shortcode extraction for use by the preview system.
    pub fn extract_shortcode_public(url: &Url) -> Option<String> {
        let host = url.host_str()?.to_lowercase();
        if host != "instagram.com" && host != "www.instagram.com" {
            return None;
        }
        Self::extract_shortcode(url)
    }

    /// Authenticated GET request via curl for Instagram REST API endpoints.
    ///
    /// `use_cookies`: if true AND cookies are available, sends session cookies.
    /// Used for profile info, feed, highlights, stories.
    async fn curl_get(endpoint: &str, use_cookies: bool) -> Result<String, AppError> {
        let binary = Self::curl_binary();
        let mut cmd = tokio::process::Command::new(binary);
        if binary == "curl-impersonate" {
            cmd.arg("--impersonate").arg("chrome131");
        }

        cmd.arg("-s")
            .arg("--compressed")
            .arg(endpoint)
            .arg("-H")
            .arg(format!("X-IG-App-ID: {}", IG_APP_ID))
            .arg("-H")
            .arg("User-Agent: Instagram 275.0.0.27.98 Android")
            .arg("-H")
            .arg("X-Requested-With: XMLHttpRequest")
            .arg("--max-time")
            .arg("30");

        let has_cookies = if use_cookies {
            if let Some(cookie_header) = crate::download::cookies::load_instagram_cookie_header() {
                cmd.arg("-H").arg(format!("Cookie: {}", cookie_header));
                true
            } else {
                false
            }
        } else {
            false
        };

        if let Some(ref proxy_url) = *config::proxy::WARP_PROXY {
            let trimmed = proxy_url.trim();
            if !trimmed.is_empty() && trimmed != "none" && trimmed != "disabled" {
                cmd.arg("--proxy").arg(trimmed);
            }
        }

        log::info!("InstagramSource: curl GET {} (cookies={})", endpoint, has_cookies);

        let output = cmd
            .output()
            .await
            .map_err(|e| AppError::Download(DownloadError::Instagram(format!("curl GET failed: {}", e))))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(AppError::Download(DownloadError::Instagram(format!(
                "curl GET error (exit {}): {}",
                output.status.code().unwrap_or(-1),
                stderr.chars().take(300).collect::<String>()
            ))));
        }

        let text = String::from_utf8_lossy(&output.stdout).to_string();
        if text.is_empty() {
            return Err(AppError::Download(DownloadError::Instagram(
                "curl GET returned empty response".to_string(),
            )));
        }

        Ok(text)
    }

    /// Anonymous POST request via curl for Instagram GraphQL API.
    ///
    /// Uses web headers (Chrome UA, LSD token, ASBD-ID). No cookies — expired/invalid
    /// cookies cause Instagram to return HTML login page instead of JSON.
    async fn curl_graphql(body: &str) -> Result<String, AppError> {
        let binary = Self::curl_binary();
        let mut cmd = tokio::process::Command::new(binary);
        if binary == "curl-impersonate" {
            cmd.arg("--impersonate").arg("chrome131");
        }

        cmd.arg("-s")
            .arg("--compressed")
            .arg("-X")
            .arg("POST")
            .arg(GRAPHQL_ENDPOINT)
            .arg("-H")
            .arg(format!("X-IG-App-ID: {}", IG_APP_ID))
            .arg("-H")
            .arg(format!("X-FB-LSD: {}", FB_LSD_TOKEN))
            .arg("-H")
            .arg(format!("X-ASBD-ID: {}", FB_ASBD_ID))
            .arg("-H")
            .arg("X-Requested-With: XMLHttpRequest")
            .arg("-H")
            .arg("Content-Type: application/x-www-form-urlencoded")
            .arg("-H")
            .arg("Referer: https://www.instagram.com/")
            .arg("-H")
            .arg("Origin: https://www.instagram.com")
            .arg("--max-time")
            .arg("30");

        if let Some(ref proxy_url) = *config::proxy::WARP_PROXY {
            let trimmed = proxy_url.trim();
            if !trimmed.is_empty() && trimmed != "none" && trimmed != "disabled" {
                cmd.arg("--proxy").arg(trimmed);
            }
        }

        cmd.arg("-d").arg(body);

        log::info!("InstagramSource: curl GraphQL POST (anonymous)");

        let output = cmd
            .output()
            .await
            .map_err(|e| AppError::Download(DownloadError::Instagram(format!("curl GraphQL failed: {}", e))))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(AppError::Download(DownloadError::Instagram(format!(
                "curl GraphQL error (exit {}): {}",
                output.status.code().unwrap_or(-1),
                stderr.chars().take(300).collect::<String>()
            ))));
        }

        let text = String::from_utf8_lossy(&output.stdout).to_string();
        if text.is_empty() {
            return Err(AppError::Download(DownloadError::Instagram(
                "curl GraphQL returned empty response".to_string(),
            )));
        }

        Ok(text)
    }

    /// Fetch media data from Instagram's GraphQL API.
    ///
    /// Uses curl to bypass TLS fingerprinting that blocks reqwest on datacenter IPs.
    async fn fetch_graphql_media(&self, shortcode: &str) -> Result<GraphQLMedia, AppError> {
        if !self.rate_limiter.acquire() {
            log::warn!("InstagramSource: rate limited, falling back to yt-dlp");
            return Err(AppError::Download(DownloadError::Instagram("Rate limited".to_string())));
        }

        let doc_id = config::INSTAGRAM_DOC_ID.as_str();
        let variables = format!(r#"{{"shortcode":"{}"}}"#, shortcode);
        let body = format!(
            "doc_id={}&variables={}&lsd={}",
            doc_id,
            urlencoding::encode(&variables),
            FB_LSD_TOKEN
        );
        let response_text = Self::curl_graphql(&body).await?;

        let body: serde_json::Value = serde_json::from_str(&response_text).map_err(|e| {
            log::error!(
                "InstagramSource: GraphQL returned non-JSON ({}): {}",
                e,
                &response_text[..response_text.len().min(500)]
            );
            AppError::Download(DownloadError::Instagram(format!(
                "Failed to parse GraphQL response: {}",
                e
            )))
        })?;

        // Detect doc_id expiry or error responses
        if let Some(message) = body.get("message").and_then(|v| v.as_str()) {
            if message.contains("useragent mismatch") || message.contains("doc_id") {
                log::error!("InstagramSource: possible doc_id expiry: {}", message);
                return Err(AppError::Download(DownloadError::Instagram(format!(
                    "doc_id may be expired: {}",
                    message
                ))));
            }
        }

        // Navigate the GraphQL response structure
        let media = body
            .pointer("/data/xdt_shortcode_media")
            .or_else(|| body.pointer("/data/shortcode_media"))
            .ok_or_else(|| {
                // Check for specific error patterns
                if let Some(message) = body.pointer("/message").and_then(|v| v.as_str()) {
                    if message.contains("checkpoint_required") || message.contains("login_required") {
                        return AppError::Download(DownloadError::Instagram(
                            "Private account or login required".to_string(),
                        ));
                    }
                }
                AppError::Download(DownloadError::Instagram(
                    "Post not found or media unavailable".to_string(),
                ))
            })?;

        let is_video = media.get("is_video").and_then(|v| v.as_bool()).unwrap_or(false);
        let video_url = media.get("video_url").and_then(|v| v.as_str()).map(String::from);
        let display_url = media.get("display_url").and_then(|v| v.as_str()).map(String::from);
        let duration_secs = media.get("video_duration").and_then(|v| v.as_f64());
        let thumbnail_url = media
            .get("thumbnail_src")
            .or_else(|| media.get("display_url"))
            .and_then(|v| v.as_str())
            .map(String::from);

        let caption = media
            .pointer("/edge_media_to_caption/edges/0/node/text")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let username = media
            .pointer("/owner/username")
            .and_then(|v| v.as_str())
            .unwrap_or("instagram")
            .to_string();

        // Check for carousel (sidecar)
        let sidecar_edges = media
            .pointer("/edge_sidecar_to_children/edges")
            .and_then(|v| v.as_array());

        let media_items = if let Some(edges) = sidecar_edges {
            // Carousel: collect all items
            edges
                .iter()
                .filter_map(|edge| {
                    let node = edge.get("node")?;
                    let item_is_video = node.get("is_video").and_then(|v| v.as_bool()).unwrap_or(false);
                    let item_video_url = node.get("video_url").and_then(|v| v.as_str()).map(String::from);
                    let item_display_url = node.get("display_url").and_then(|v| v.as_str()).map(String::from);
                    Some(MediaItem {
                        is_video: item_is_video,
                        video_url: item_video_url,
                        display_url: item_display_url,
                    })
                })
                .collect()
        } else {
            // Single item
            vec![MediaItem {
                is_video,
                video_url,
                display_url,
            }]
        };

        if media_items.is_empty() {
            return Err(AppError::Download(DownloadError::Instagram(
                "No media found in post".to_string(),
            )));
        }

        Ok(GraphQLMedia {
            items: media_items,
            caption,
            username,
            thumbnail_url,
            duration_secs,
        })
    }

    /// Download a single media URL (video or photo) to the output path.
    async fn download_media_url(
        &self,
        media_url: &str,
        output_path: &str,
        progress_tx: &mpsc::UnboundedSender<SourceProgress>,
    ) -> Result<u64, AppError> {
        let response =
            self.client.get(media_url).send().await.map_err(|e| {
                AppError::Download(DownloadError::Instagram(format!("Failed to download media: {}", e)))
            })?;

        if !response.status().is_success() {
            return Err(AppError::Download(DownloadError::Instagram(format!(
                "Media download HTTP {}",
                response.status()
            ))));
        }

        let total_size = response.content_length();

        // Ensure parent directory exists (DOWNLOAD_FOLDER may not exist yet)
        if let Some(parent) = std::path::Path::new(output_path).parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                AppError::Download(DownloadError::Instagram(format!("Failed to create directory: {}", e)))
            })?;
        }
        let mut file = std::fs::File::create(output_path)
            .map_err(|e| AppError::Download(DownloadError::Instagram(format!("Failed to create file: {}", e))))?;

        let mut downloaded: u64 = 0;
        let mut last_progress_percent = 0u8;
        let mut stream = response.bytes_stream();
        use futures_util::StreamExt;

        while let Some(chunk_result) = stream.next().await {
            let chunk = chunk_result
                .map_err(|e| AppError::Download(DownloadError::Instagram(format!("Error reading chunk: {}", e))))?;

            file.write_all(&chunk)
                .map_err(|e| AppError::Download(DownloadError::Instagram(format!("Error writing to file: {}", e))))?;

            downloaded += chunk.len() as u64;

            let percent = total_size
                .map(|total| {
                    if total > 0 {
                        ((downloaded as f64 / total as f64) * 100.0) as u8
                    } else {
                        0
                    }
                })
                .unwrap_or(0);

            if percent >= last_progress_percent + 10 || percent == 100 {
                last_progress_percent = percent;
                let _ = progress_tx.send(SourceProgress {
                    percent,
                    speed_bytes_sec: None,
                    eta_seconds: None,
                    downloaded_bytes: Some(downloaded),
                    total_bytes: total_size,
                });
            }
        }

        file.flush()
            .map_err(|e| AppError::Download(DownloadError::Instagram(format!("Failed to flush file: {}", e))))?;

        Ok(downloaded)
    }

    /// Check if URL is an Instagram profile URL (e.g., `instagram.com/username`).
    ///
    /// Returns the username if it matches, excluding reserved paths.
    pub fn extract_profile_username(url: &Url) -> Option<String> {
        let host = url.host_str()?.to_lowercase();
        if host != "instagram.com" && host != "www.instagram.com" {
            return None;
        }

        let segments: Vec<&str> = url.path_segments()?.filter(|s| !s.is_empty()).collect();
        if segments.len() != 1 {
            return None;
        }

        let username = segments[0];
        // Exclude reserved paths
        const RESERVED: &[&str] = &[
            "p",
            "reel",
            "reels",
            "tv",
            "stories",
            "explore",
            "accounts",
            "about",
            "legal",
            "developer",
            "directory",
            "api",
            "static",
            "favicon.ico",
        ];
        if RESERVED.contains(&username) {
            return None;
        }

        // Basic username validation: alphanumeric + dots + underscores, 1-30 chars
        if username.len() > 30 || !username.chars().all(|c| c.is_alphanumeric() || c == '.' || c == '_') {
            return None;
        }

        Some(username.to_string())
    }

    /// Fetch profile data from Instagram's REST API.
    ///
    /// Returns profile info and recent posts for the profile browsing UI.
    pub async fn fetch_profile(&self, username: &str) -> Result<InstagramProfile, AppError> {
        if !self.rate_limiter.acquire() {
            return Err(AppError::Download(DownloadError::Instagram("Rate limited".to_string())));
        }

        // Send cookies when available — Railway IPs get "require_login" without auth
        let profile_endpoint = format!(
            "https://i.instagram.com/api/v1/users/web_profile_info/?username={}",
            urlencoding::encode(username)
        );
        let response_text = Self::curl_get(&profile_endpoint, true).await?;

        let body: serde_json::Value = serde_json::from_str(&response_text).map_err(|e| {
            AppError::Download(DownloadError::Instagram(format!(
                "Failed to parse profile response: {}",
                e
            )))
        })?;

        // REST API returns /data/user with full profile info + edge_owner_to_timeline_media/edges
        let (full_name, biography, profile_pic_url, is_private, user_id, post_count, follower_count, posts, end_cursor) =
            if let Some(user) = body.pointer("/data/user") {
                let full_name = user.get("full_name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let biography = user.get("biography").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let profile_pic_url = user
                    .get("profile_pic_url_hd")
                    .or_else(|| user.get("profile_pic_url"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let is_private = user.get("is_private").and_then(|v| v.as_bool()).unwrap_or(false);
                let user_id = user.get("id").and_then(|v| v.as_str()).map(String::from);
                let post_count = user
                    .pointer("/edge_owner_to_timeline_media/count")
                    .and_then(|v| v.as_u64())
                    .or_else(|| user.get("media_count").and_then(|v| v.as_u64()))
                    .unwrap_or(0) as u32;
                let follower_count = user
                    .pointer("/edge_followed_by/count")
                    .and_then(|v| v.as_u64())
                    .or_else(|| user.get("follower_count").and_then(|v| v.as_u64()))
                    .unwrap_or(0) as u32;
                let posts: Vec<ProfilePost> = user
                    .pointer("/edge_owner_to_timeline_media/edges")
                    .and_then(|v| v.as_array())
                    .map(|edges| {
                        edges
                            .iter()
                            .take(12)
                            .filter_map(|edge| {
                                let node = edge.get("node")?;
                                let shortcode = node.get("shortcode")?.as_str()?.to_string();
                                let is_video = node.get("is_video").and_then(|v| v.as_bool()).unwrap_or(false);
                                let typename = node.get("__typename").and_then(|v| v.as_str()).unwrap_or("");
                                let is_carousel = typename == "GraphSidecar"
                                    || node.get("carousel_media_count").and_then(|v| v.as_u64()).unwrap_or(0) > 0;
                                let thumbnail = node
                                    .get("thumbnail_src")
                                    .or_else(|| node.get("display_url"))
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                Some(ProfilePost {
                                    shortcode,
                                    is_video,
                                    is_carousel,
                                    thumbnail_url: thumbnail,
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let end_cursor = user
                    .pointer("/edge_owner_to_timeline_media/page_info/end_cursor")
                    .and_then(|v| v.as_str())
                    .map(String::from);

                (
                    full_name,
                    biography,
                    profile_pic_url,
                    is_private,
                    user_id,
                    post_count,
                    follower_count,
                    posts,
                    end_cursor,
                )
            } else {
                log::error!(
                    "InstagramSource: profile response has no recognizable data: {}",
                    &response_text[..response_text.len().min(500)]
                );
                return Err(AppError::Download(DownloadError::Instagram(
                    "Profile not found".to_string(),
                )));
            };

        // REST API web_profile_info returns edges=[] — fetch posts via feed API
        let (posts, end_cursor) = if posts.is_empty() && !is_private {
            if let Some(ref uid) = user_id {
                match Self::fetch_user_feed(uid).await {
                    Ok((feed_posts, feed_cursor)) => (feed_posts, feed_cursor),
                    Err(e) => {
                        log::warn!("InstagramSource: feed API failed for {}: {}", uid, e);
                        (posts, end_cursor)
                    }
                }
            } else {
                (posts, end_cursor)
            }
        } else {
            (posts, end_cursor)
        };

        Ok(InstagramProfile {
            username: username.to_string(),
            full_name,
            biography,
            profile_pic_url,
            is_private,
            user_id,
            post_count,
            follower_count,
            posts,
            end_cursor,
        })
    }

    /// Fetch user posts via the feed API (`/api/v1/feed/user/{user_id}/`).
    ///
    /// Returns up to 12 posts with shortcode, is_video, is_carousel, thumbnail.
    /// Requires cookies for authenticated access.
    async fn fetch_user_feed(user_id: &str) -> Result<(Vec<ProfilePost>, Option<String>), AppError> {
        let endpoint = format!("https://i.instagram.com/api/v1/feed/user/{}/", user_id);
        log::info!("InstagramSource: fetching user feed for user_id={}", user_id);

        let text = Self::curl_get(&endpoint, true).await?;
        let body: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| AppError::Download(DownloadError::Instagram(format!("Feed JSON parse error: {}", e))))?;

        let items = body.get("items").and_then(|v| v.as_array());
        let posts: Vec<ProfilePost> = items
            .map(|arr| {
                arr.iter()
                    .take(12)
                    .filter_map(|item| {
                        let shortcode = item.get("code")?.as_str()?.to_string();
                        let media_type = item.get("media_type").and_then(|v| v.as_u64()).unwrap_or(1);
                        let is_video = media_type == 2;
                        let is_carousel = media_type == 8
                            || item.get("carousel_media_count").and_then(|v| v.as_u64()).unwrap_or(0) > 0;
                        let thumbnail = item
                            .pointer("/image_versions2/candidates/0/url")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        Some(ProfilePost {
                            shortcode,
                            is_video,
                            is_carousel,
                            thumbnail_url: thumbnail,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        let end_cursor = body
            .get("next_max_id")
            .and_then(|v| v.as_str().map(String::from).or_else(|| Some(v.to_string())));

        log::info!(
            "InstagramSource: feed returned {} posts for user_id={}",
            posts.len(),
            user_id
        );
        Ok((posts, end_cursor))
    }

    /// Fetch highlights tray (list of highlight reels) for a user.
    ///
    /// Requires authenticated session cookies.
    pub async fn fetch_highlights(&self, user_id: &str) -> Result<Vec<HighlightReel>, AppError> {
        if !self.rate_limiter.acquire() {
            return Err(AppError::Download(DownloadError::Instagram("Rate limited".to_string())));
        }

        let endpoint = format!("https://i.instagram.com/api/v1/highlights/{}/highlights_tray/", user_id);
        log::info!("InstagramSource: fetching highlights tray for user_id={}", user_id);

        let text = Self::curl_get(&endpoint, true).await?;
        let body: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| AppError::Download(DownloadError::Instagram(format!("Highlights JSON parse error: {}", e))))?;

        let tray = body.get("tray").and_then(|v| v.as_array()).ok_or_else(|| {
            AppError::Download(DownloadError::Instagram("No highlights tray in response".to_string()))
        })?;

        let mut highlights = Vec::new();
        for item in tray {
            // id can be numeric — always stringify
            let id_str = item
                .get("id")
                .map(|v| match v {
                    serde_json::Value::String(s) => s.clone(),
                    serde_json::Value::Number(n) => n.to_string(),
                    _ => String::new(),
                })
                .unwrap_or_default();
            // Strip "highlight:" prefix — we re-add it when calling fetch_reel_media,
            // and the colon conflicts with callback data `:` separator.
            let id_str = id_str.strip_prefix("highlight:").unwrap_or(&id_str).to_string();
            if id_str.is_empty() {
                continue;
            }
            let title = item
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("Untitled")
                .to_string();
            let cover_url = item
                .pointer("/cover_media/cropped_image_version/url")
                .or_else(|| item.pointer("/cover_media/media_url"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let item_count = item.get("media_count").and_then(|v| v.as_u64()).unwrap_or(0) as u32;

            highlights.push(HighlightReel {
                id: id_str,
                title,
                cover_url,
                item_count,
            });
        }

        log::info!(
            "InstagramSource: found {} highlights for user_id={}",
            highlights.len(),
            user_id
        );
        Ok(highlights)
    }

    /// Fetch highlight items by reel ID.
    ///
    /// For highlights: reel_id = "highlight:12345678"
    /// Uses `/api/v1/feed/reels_media/` which is the correct endpoint for highlights.
    /// For user stories, use `fetch_stories()` instead.
    pub async fn fetch_reel_media(&self, reel_id: &str) -> Result<Vec<StoryItem>, AppError> {
        if !self.rate_limiter.acquire() {
            return Err(AppError::Download(DownloadError::Instagram("Rate limited".to_string())));
        }

        let endpoint = format!(
            "https://i.instagram.com/api/v1/feed/reels_media/?reel_ids={}",
            urlencoding::encode(reel_id)
        );
        log::info!("InstagramSource: fetching reel media for reel_id={}", reel_id);

        let text = Self::curl_get(&endpoint, true).await?;
        let body: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| AppError::Download(DownloadError::Instagram(format!("Reel media JSON parse error: {}", e))))?;

        // Response: { "reels_media": [ { "items": [...] } ] }
        // or: { "reels": { "<reel_id>": { "items": [...] } } }
        let items_array = body
            .pointer("/reels_media/0/items")
            .and_then(|v| v.as_array())
            .or_else(|| {
                body.get("reels")
                    .and_then(|reels| reels.get(reel_id))
                    .and_then(|reel| reel.get("items"))
                    .and_then(|v| v.as_array())
            });

        let items = match items_array {
            Some(arr) => arr,
            None => {
                log::warn!(
                    "InstagramSource: no items in reel response for {}. Keys: {:?}",
                    reel_id,
                    body.as_object().map(|o| o.keys().collect::<Vec<_>>())
                );
                return Ok(Vec::new());
            }
        };

        let mut story_items = Vec::new();
        for item in items {
            let id = item
                .get("id")
                .or_else(|| item.get("pk"))
                .map(|v| match v {
                    serde_json::Value::String(s) => s.clone(),
                    serde_json::Value::Number(n) => n.to_string(),
                    _ => String::new(),
                })
                .unwrap_or_default();
            if id.is_empty() {
                continue;
            }

            let media_type = item.get("media_type").and_then(|v| v.as_u64()).unwrap_or(1);
            let is_video = media_type == 2;

            let media_url = if is_video {
                // Video: get best quality video version
                item.get("video_versions")
                    .and_then(|v| v.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|v| v.get("url"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            } else {
                // Photo: get best quality image
                item.get("image_versions2")
                    .and_then(|v| v.get("candidates"))
                    .and_then(|v| v.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|v| v.get("url"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            };

            if media_url.is_empty() {
                continue;
            }

            let thumbnail_url = if is_video {
                item.get("image_versions2")
                    .and_then(|v| v.get("candidates"))
                    .and_then(|v| v.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|v| v.get("url"))
                    .and_then(|v| v.as_str())
                    .map(String::from)
            } else {
                None
            };

            let duration_secs = item.get("video_duration").and_then(|v| v.as_f64());
            let taken_at = item.get("taken_at").and_then(|v| v.as_i64());

            story_items.push(StoryItem {
                id,
                is_video,
                media_url,
                thumbnail_url,
                duration_secs,
                taken_at,
            });
        }

        log::info!("InstagramSource: found {} items in reel {}", story_items.len(), reel_id);
        Ok(story_items)
    }

    /// Fetch active stories for a user via the dedicated stories endpoint.
    ///
    /// Uses `GET /api/v1/feed/user/{user_id}/story/` which returns `{ "reel": { "items": [...] } }`.
    /// This is the correct endpoint for user stories — `reels_media` is for highlights.
    pub async fn fetch_stories(&self, user_id: &str) -> Result<Vec<StoryItem>, AppError> {
        if !self.rate_limiter.acquire() {
            return Err(AppError::Download(DownloadError::Instagram("Rate limited".to_string())));
        }

        let endpoint = format!("https://i.instagram.com/api/v1/feed/user/{}/story/", user_id);
        log::info!("InstagramSource: fetching stories for user_id={}", user_id);

        let text = Self::curl_get(&endpoint, true).await?;
        let body: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| AppError::Download(DownloadError::Instagram(format!("Stories JSON parse error: {}", e))))?;

        // Response: { "reel": { "items": [...] }, "status": "ok" }
        // When no active stories: { "reel": null, "status": "ok" }
        let items_array = body.pointer("/reel/items").and_then(|v| v.as_array());

        let items = match items_array {
            Some(arr) => arr,
            None => {
                log::info!("InstagramSource: no active stories for user_id={}", user_id);
                return Ok(Vec::new());
            }
        };

        let mut story_items = Vec::new();
        for item in items {
            let id = item
                .get("id")
                .or_else(|| item.get("pk"))
                .map(|v| match v {
                    serde_json::Value::String(s) => s.clone(),
                    serde_json::Value::Number(n) => n.to_string(),
                    _ => String::new(),
                })
                .unwrap_or_default();
            if id.is_empty() {
                continue;
            }

            let media_type = item.get("media_type").and_then(|v| v.as_u64()).unwrap_or(1);
            let is_video = media_type == 2;

            let media_url = if is_video {
                item.get("video_versions")
                    .and_then(|v| v.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|v| v.get("url"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            } else {
                item.get("image_versions2")
                    .and_then(|v| v.get("candidates"))
                    .and_then(|v| v.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|v| v.get("url"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            };

            if media_url.is_empty() {
                continue;
            }

            let thumbnail_url = if is_video {
                item.get("image_versions2")
                    .and_then(|v| v.get("candidates"))
                    .and_then(|v| v.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|v| v.get("url"))
                    .and_then(|v| v.as_str())
                    .map(String::from)
            } else {
                None
            };

            let duration_secs = item.get("video_duration").and_then(|v| v.as_f64());
            let taken_at = item.get("taken_at").and_then(|v| v.as_i64());

            story_items.push(StoryItem {
                id,
                is_video,
                media_url,
                thumbnail_url,
                duration_secs,
                taken_at,
            });
        }

        log::info!(
            "InstagramSource: found {} stories for user_id={}",
            story_items.len(),
            user_id
        );
        Ok(story_items)
    }

    /// Get preview metadata for an Instagram content URL (post/reel/tv).
    ///
    /// Used by the preview system to build format selection UI without yt-dlp.
    pub async fn get_media_preview(&self, url: &Url) -> Result<InstagramPreviewInfo, AppError> {
        let shortcode = Self::extract_shortcode(url).ok_or_else(|| {
            AppError::Download(DownloadError::Instagram(
                "Cannot extract shortcode from URL".to_string(),
            ))
        })?;

        let media = self.fetch_graphql_media(&shortcode).await?;
        let primary = &media.items[0];
        let is_carousel = media.items.len() > 1;

        let title = if media.caption.is_empty() {
            format!("Instagram post by @{}", media.username)
        } else {
            let first_line = media.caption.lines().next().unwrap_or(&media.caption);
            if first_line.len() > 100 {
                let truncated = first_line
                    .char_indices()
                    .nth(97)
                    .map(|(i, _)| &first_line[..i])
                    .unwrap_or(first_line);
                format!("{}...", truncated)
            } else {
                first_line.to_string()
            }
        };

        let carousel_count = if is_carousel {
            media.items.len().min(10) as u8
        } else {
            0
        };

        Ok(InstagramPreviewInfo {
            title,
            artist: format!("@{}", media.username),
            thumbnail_url: media.thumbnail_url,
            duration_secs: media.duration_secs.map(|d| d as u32),
            is_video: primary.is_video,
            is_carousel,
            carousel_count,
        })
    }
}

/// Instagram user profile data.
pub struct InstagramProfile {
    pub username: String,
    pub full_name: String,
    pub biography: String,
    pub profile_pic_url: String,
    pub is_private: bool,
    pub user_id: Option<String>,
    pub post_count: u32,
    pub follower_count: u32,
    pub posts: Vec<ProfilePost>,
    pub end_cursor: Option<String>,
}

/// A post in the profile grid.
pub struct ProfilePost {
    pub shortcode: String,
    pub is_video: bool,
    pub is_carousel: bool,
    pub thumbnail_url: String,
}

/// Preview info returned by `get_media_preview` for the Telegram preview UI.
pub struct InstagramPreviewInfo {
    pub title: String,
    pub artist: String,
    pub thumbnail_url: Option<String>,
    pub duration_secs: Option<u32>,
    pub is_video: bool,
    pub is_carousel: bool,
    pub carousel_count: u8,
}

/// An Instagram highlight reel summary (from highlights tray).
pub struct HighlightReel {
    pub id: String,
    pub title: String,
    pub cover_url: String,
    pub item_count: u32,
}

/// A single story or highlight item (photo or video).
pub struct StoryItem {
    pub id: String,
    pub is_video: bool,
    /// Direct media URL (photo or video)
    pub media_url: String,
    /// Thumbnail URL for videos
    pub thumbnail_url: Option<String>,
    /// Duration in seconds (for videos)
    pub duration_secs: Option<f64>,
    /// Unix timestamp when this story was taken
    pub taken_at: Option<i64>,
}

/// Parsed media data from Instagram's GraphQL response.
struct GraphQLMedia {
    items: Vec<MediaItem>,
    caption: String,
    username: String,
    thumbnail_url: Option<String>,
    duration_secs: Option<f64>,
}

/// A single media item (video or photo) within a post.
struct MediaItem {
    is_video: bool,
    video_url: Option<String>,
    display_url: Option<String>,
}

#[async_trait]
impl DownloadSource for InstagramSource {
    fn name(&self) -> &str {
        "instagram"
    }

    fn supports_url(&self, url: &Url) -> bool {
        let host = match url.host_str() {
            Some(h) => h.to_lowercase(),
            None => return false,
        };

        if host != "instagram.com" && host != "www.instagram.com" {
            return false;
        }

        // Only handle content URLs (posts, reels, tv)
        Self::is_content_url(url)
    }

    async fn get_metadata(&self, url: &Url) -> Result<MediaMetadata, AppError> {
        let shortcode = Self::extract_shortcode(url).ok_or_else(|| {
            AppError::Download(DownloadError::Instagram(
                "Cannot extract shortcode from URL".to_string(),
            ))
        })?;

        match self.fetch_graphql_media(&shortcode).await {
            Ok(media) => {
                let title = if media.caption.is_empty() {
                    format!("Instagram post by @{}", media.username)
                } else {
                    // Truncate caption for title (first line, max 100 chars)
                    let first_line = media.caption.lines().next().unwrap_or(&media.caption);
                    if first_line.len() > 100 {
                        format!("{}...", &first_line[..97])
                    } else {
                        first_line.to_string()
                    }
                };
                Ok(MediaMetadata {
                    title,
                    artist: format!("@{}", media.username),
                })
            }
            Err(_) => {
                // Fallback: use yt-dlp for metadata
                log::info!("InstagramSource: GraphQL metadata failed, falling back to yt-dlp");
                let ytdlp = super::ytdlp::YtDlpSource::new();
                ytdlp.get_metadata(url).await
            }
        }
    }

    async fn estimate_size(&self, _url: &Url) -> Option<u64> {
        None // Instagram doesn't expose size in GraphQL
    }

    async fn is_livestream(&self, _url: &Url) -> bool {
        false // Instagram content URLs are never livestreams
    }

    async fn download(
        &self,
        request: &DownloadRequest,
        progress_tx: mpsc::UnboundedSender<SourceProgress>,
    ) -> Result<DownloadOutput, AppError> {
        let shortcode = match Self::extract_shortcode(&request.url) {
            Some(sc) => sc,
            None => {
                // Shouldn't happen since supports_url checks this
                return Err(AppError::Download(DownloadError::Instagram(
                    "Cannot extract shortcode".to_string(),
                )));
            }
        };

        log::info!("InstagramSource: downloading shortcode={}", shortcode);

        // Try GraphQL first
        let graphql_result = self.fetch_graphql_media(&shortcode).await;

        match graphql_result {
            Ok(media) => {
                // Apply carousel mask filter: only download selected items
                let mask = request
                    .carousel_mask
                    .or_else(|| take_carousel_mask(request.url.as_str()));
                let selected_items: Vec<(usize, &MediaItem)> = if let Some(m) = mask {
                    media
                        .items
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| m & (1u32 << i) != 0)
                        .collect()
                } else {
                    // No mask = download all items
                    media.items.iter().enumerate().collect()
                };

                if selected_items.is_empty() {
                    return Err(AppError::Download(DownloadError::Instagram(
                        "No carousel items selected".to_string(),
                    )));
                }

                let primary = selected_items[0].1;

                // Determine download URL and mime type for primary item
                let (download_url, mime_hint) = if primary.is_video {
                    match &primary.video_url {
                        Some(url) => (url.as_str(), "video/mp4"),
                        None => {
                            log::warn!("InstagramSource: video post but no video_url, falling back");
                            return self.fallback_ytdlp(request, progress_tx).await;
                        }
                    }
                } else {
                    match &primary.display_url {
                        Some(url) => (url.as_str(), "image/jpeg"),
                        None => {
                            log::warn!("InstagramSource: photo post but no display_url, falling back");
                            return self.fallback_ytdlp(request, progress_tx).await;
                        }
                    }
                };

                let total_items = media.items.len();
                let selected_count = selected_items.len();
                log::info!(
                    "InstagramSource: downloading {} ({}{}) by @{}",
                    if primary.is_video { "video" } else { "photo" },
                    mime_hint,
                    if total_items > 1 {
                        if mask.is_some() {
                            format!(", carousel {}/{} items selected", selected_count, total_items)
                        } else {
                            format!(", carousel with {} items", total_items)
                        }
                    } else {
                        String::new()
                    },
                    media.username
                );

                // Download primary item
                let file_size = self
                    .download_media_url(download_url, &request.output_path, &progress_tx)
                    .await?;

                let duration_secs = if primary.is_video {
                    crate::download::metadata::probe_duration_seconds(&request.output_path)
                } else {
                    None
                };

                // Download additional selected carousel items (if any)
                let additional_files = if selected_items.len() > 1 {
                    let mut extras = Vec::new();
                    for (orig_idx, item) in selected_items.iter().skip(1) {
                        let (item_url, item_mime) = if item.is_video {
                            match &item.video_url {
                                Some(u) => (u.as_str(), "video/mp4"),
                                None => continue,
                            }
                        } else {
                            match &item.display_url {
                                Some(u) => (u.as_str(), "image/jpeg"),
                                None => continue,
                            }
                        };

                        let ext = if item.is_video { "mp4" } else { "jpg" };
                        let item_path = format!(
                            "{}_carousel_{}.{}",
                            request
                                .output_path
                                .trim_end_matches(|c: char| c == '.' || c.is_alphanumeric()),
                            orig_idx + 1, // 1-based item number from original carousel
                            ext
                        );
                        // Use a no-op progress sender for additional items
                        let (extra_tx, _extra_rx) = mpsc::unbounded_channel();
                        match self.download_media_url(item_url, &item_path, &extra_tx).await {
                            Ok(_) => {
                                let item_duration = if item.is_video {
                                    crate::download::metadata::probe_duration_seconds(&item_path)
                                } else {
                                    None
                                };
                                extras.push(super::AdditionalFile {
                                    file_path: item_path,
                                    mime_type: item_mime.to_string(),
                                    duration_secs: item_duration,
                                });
                            }
                            Err(e) => {
                                log::warn!(
                                    "InstagramSource: failed to download carousel item {}: {}",
                                    orig_idx + 1,
                                    e
                                );
                            }
                        }
                    }
                    if extras.is_empty() {
                        None
                    } else {
                        Some(extras)
                    }
                } else {
                    None
                };

                Ok(DownloadOutput {
                    file_path: request.output_path.clone(),
                    duration_secs,
                    file_size,
                    mime_hint: Some(mime_hint.to_string()),
                    additional_files,
                })
            }
            Err(e) => {
                log::warn!("InstagramSource: GraphQL failed ({}), falling back to yt-dlp", e);
                self.fallback_ytdlp(request, progress_tx).await
            }
        }
    }
}

impl InstagramSource {
    /// Fallback to yt-dlp when GraphQL fails.
    async fn fallback_ytdlp(
        &self,
        request: &DownloadRequest,
        progress_tx: mpsc::UnboundedSender<SourceProgress>,
    ) -> Result<DownloadOutput, AppError> {
        log::info!("InstagramSource: using yt-dlp fallback for {}", request.url);
        let ytdlp = super::ytdlp::YtDlpSource::new();
        ytdlp.download(request, progress_tx).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_shortcode_reel() {
        let url = Url::parse("https://www.instagram.com/reel/ABC123xyz/").unwrap();
        assert_eq!(InstagramSource::extract_shortcode(&url), Some("ABC123xyz".to_string()));
    }

    #[test]
    fn test_extract_shortcode_post() {
        let url = Url::parse("https://www.instagram.com/p/DEF456/").unwrap();
        assert_eq!(InstagramSource::extract_shortcode(&url), Some("DEF456".to_string()));
    }

    #[test]
    fn test_extract_shortcode_reels() {
        let url = Url::parse("https://www.instagram.com/reels/GHI789/").unwrap();
        assert_eq!(InstagramSource::extract_shortcode(&url), Some("GHI789".to_string()));
    }

    #[test]
    fn test_extract_shortcode_tv() {
        let url = Url::parse("https://www.instagram.com/tv/JKL012/").unwrap();
        assert_eq!(InstagramSource::extract_shortcode(&url), Some("JKL012".to_string()));
    }

    #[test]
    fn test_extract_shortcode_no_match() {
        let url = Url::parse("https://www.instagram.com/username/").unwrap();
        assert_eq!(InstagramSource::extract_shortcode(&url), None);
    }

    #[test]
    fn test_extract_shortcode_with_query() {
        let url = Url::parse("https://www.instagram.com/reel/ABC123/?igsh=xxx").unwrap();
        assert_eq!(InstagramSource::extract_shortcode(&url), Some("ABC123".to_string()));
    }

    #[test]
    fn test_supports_url_reel() {
        let source = InstagramSource::new();
        let url = Url::parse("https://www.instagram.com/reel/ABC123/").unwrap();
        assert!(source.supports_url(&url));
    }

    #[test]
    fn test_supports_url_post() {
        let source = InstagramSource::new();
        let url = Url::parse("https://www.instagram.com/p/DEF456/").unwrap();
        assert!(source.supports_url(&url));
    }

    #[test]
    fn test_rejects_profile_url() {
        let source = InstagramSource::new();
        let url = Url::parse("https://www.instagram.com/username/").unwrap();
        assert!(!source.supports_url(&url));
    }

    #[test]
    fn test_rejects_non_instagram() {
        let source = InstagramSource::new();
        let url = Url::parse("https://www.youtube.com/watch?v=abc").unwrap();
        assert!(!source.supports_url(&url));
    }

    #[test]
    fn test_supports_url_no_www() {
        let source = InstagramSource::new();
        let url = Url::parse("https://instagram.com/reel/ABC123/").unwrap();
        assert!(source.supports_url(&url));
    }

    #[test]
    fn test_extract_shortcode_with_username_prefix() {
        let url = Url::parse("https://www.instagram.com/kologoidaa/reel/B58TfHTnY2u/").unwrap();
        assert_eq!(
            InstagramSource::extract_shortcode(&url),
            Some("B58TfHTnY2u".to_string())
        );
    }

    #[test]
    fn test_supports_url_with_username_prefix() {
        let source = InstagramSource::new();
        let url = Url::parse("https://www.instagram.com/someuser/p/ABC123/").unwrap();
        assert!(source.supports_url(&url));
    }

    #[test]
    fn test_rate_limiter_allows_under_limit() {
        let limiter = RateLimiter::new();
        for _ in 0..10 {
            assert!(limiter.acquire());
        }
    }

    #[test]
    fn test_rate_limiter_blocks_at_limit() {
        let limiter = RateLimiter::new();
        for _ in 0..RATE_LIMIT_PER_HOUR {
            assert!(limiter.acquire());
        }
        // Should be blocked now
        assert!(!limiter.acquire());
    }

    #[test]
    fn test_extract_profile_username() {
        let url = Url::parse("https://www.instagram.com/cristiano/").unwrap();
        assert_eq!(
            InstagramSource::extract_profile_username(&url),
            Some("cristiano".to_string())
        );
    }

    #[test]
    fn test_extract_profile_username_no_trailing_slash() {
        let url = Url::parse("https://www.instagram.com/cristiano").unwrap();
        assert_eq!(
            InstagramSource::extract_profile_username(&url),
            Some("cristiano".to_string())
        );
    }

    #[test]
    fn test_extract_profile_rejects_content_paths() {
        let url = Url::parse("https://www.instagram.com/p/ABC123/").unwrap();
        assert_eq!(InstagramSource::extract_profile_username(&url), None);

        let url = Url::parse("https://www.instagram.com/reel/ABC123/").unwrap();
        assert_eq!(InstagramSource::extract_profile_username(&url), None);
    }

    #[test]
    fn test_extract_profile_rejects_reserved() {
        let url = Url::parse("https://www.instagram.com/explore/").unwrap();
        assert_eq!(InstagramSource::extract_profile_username(&url), None);

        let url = Url::parse("https://www.instagram.com/accounts/").unwrap();
        assert_eq!(InstagramSource::extract_profile_username(&url), None);
    }

    #[test]
    fn test_extract_profile_rejects_non_instagram() {
        let url = Url::parse("https://www.youtube.com/cristiano").unwrap();
        assert_eq!(InstagramSource::extract_profile_username(&url), None);
    }

    /// Live integration test: calls Instagram GraphQL API for a known public photo post.
    /// Run with: cargo test test_live_graphql_photo -- --ignored --nocapture
    #[tokio::test]
    #[ignore] // requires network access
    async fn test_live_graphql_photo() {
        let _ = pretty_env_logger::try_init();
        let source = InstagramSource::new();
        // BXi1BxjFebG is a known public photo post by @instagram
        let result = source.fetch_graphql_media("BXi1BxjFebG").await;
        match &result {
            Ok(media) => {
                println!("SUCCESS: got {} items", media.items.len());
                println!("  username: @{}", media.username);
                println!("  is_video: {}", media.items[0].is_video);
                println!(
                    "  display_url: {:?}",
                    media.items[0].display_url.as_deref().map(|u| &u[..u.len().min(80)])
                );
                assert!(!media.items[0].is_video, "BXi1BxjFebG should be a photo");
                // GraphQL returned JSON (not HTML login page) — auth works!
            }
            Err(e) => {
                panic!("GraphQL request failed: {}", e);
            }
        }
    }

    /// Live integration test: calls Instagram GraphQL for a known public reel.
    /// Run with: cargo test test_live_graphql_reel -- --ignored --nocapture
    #[tokio::test]
    #[ignore] // requires network access
    async fn test_live_graphql_reel() {
        let _ = pretty_env_logger::try_init();
        let source = InstagramSource::new();
        // A popular public reel
        let result = source.fetch_graphql_media("C1234567890").await;
        match &result {
            Ok(media) => {
                println!("SUCCESS: got {} items", media.items.len());
                println!("  username: @{}", media.username);
                println!("  is_video: {}", media.items[0].is_video);
                println!(
                    "  video_url: {:?}",
                    media.items[0].video_url.as_deref().map(|u| &u[..u.len().min(80)])
                );
            }
            Err(e) => {
                // Reel may not exist, but if it returns JSON error (not HTML) — auth works
                let err_str = e.to_string();
                println!("GraphQL returned error (expected for test shortcode): {}", err_str);
                assert!(
                    !err_str.contains("Failed to parse GraphQL response"),
                    "Should not get HTML login page — got JSON error instead"
                );
            }
        }
    }
}
