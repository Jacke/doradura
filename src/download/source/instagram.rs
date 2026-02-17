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
use std::io::Write;
use std::sync::Mutex;
use std::time::Instant;
use tokio::sync::mpsc;
use url::Url;

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

    /// Fetch media data from Instagram's GraphQL API.
    ///
    /// Returns (video_url, display_url, caption, username, is_video).
    async fn fetch_graphql_media(&self, shortcode: &str) -> Result<GraphQLMedia, AppError> {
        if !self.rate_limiter.acquire() {
            log::warn!("InstagramSource: rate limited, falling back to yt-dlp");
            return Err(AppError::Download(DownloadError::Instagram("Rate limited".to_string())));
        }

        let doc_id = config::INSTAGRAM_DOC_ID.as_str();
        let variables = format!(r#"{{"shortcode":"{}"}}"#, shortcode);

        let mut request_builder = self
            .client
            .post(GRAPHQL_ENDPOINT)
            .header("X-IG-App-ID", IG_APP_ID)
            .header("X-FB-LSD", FB_LSD_TOKEN)
            .header("X-ASBD-ID", FB_ASBD_ID)
            .header("X-Requested-With", "XMLHttpRequest")
            .header("Content-Type", "application/x-www-form-urlencoded")
            .header("Referer", "https://www.instagram.com/")
            .header("Origin", "https://www.instagram.com")
            .header("Accept", "*/*")
            .header("Accept-Language", "en-US,en;q=0.9")
            .header("Sec-Fetch-Site", "same-origin")
            .header("Sec-Fetch-Mode", "cors")
            .header("Sec-Fetch-Dest", "empty");

        // Add Instagram cookies + CSRF token if available
        if let Some(cookie_header) = crate::download::cookies::load_instagram_cookie_header() {
            if let Some(csrf_token) = crate::download::cookies::load_ig_csrf_token() {
                log::info!(
                    "InstagramSource: GraphQL auth: cookies=yes, csrftoken=yes (len={})",
                    csrf_token.len()
                );
                request_builder = request_builder.header("X-CSRFToken", csrf_token);
            } else {
                log::warn!("InstagramSource: GraphQL auth: cookies=yes, csrftoken=NOT FOUND in cookies file");
            }
            request_builder = request_builder.header("Cookie", cookie_header);
        } else {
            log::info!("InstagramSource: GraphQL auth: no cookies available (anonymous request)");
        }

        let response = request_builder
            .body(format!(
                "doc_id={}&variables={}&lsd={}",
                doc_id,
                urlencoding::encode(&variables),
                FB_LSD_TOKEN
            ))
            .send()
            .await
            .map_err(|e| AppError::Download(DownloadError::Instagram(format!("GraphQL request failed: {}", e))))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            // Detect doc_id expiry pattern
            if status.as_u16() == 400 || body.contains("\"message\":\"useragent mismatch\"") || body.contains("doc_id")
            {
                log::error!(
                    "InstagramSource: possible doc_id expiry (HTTP {}): {}",
                    status,
                    &body[..body.len().min(300)]
                );
                return Err(AppError::Download(DownloadError::Instagram(format!(
                    "doc_id may be expired (HTTP {})",
                    status
                ))));
            }
            return Err(AppError::Download(DownloadError::Instagram(format!(
                "GraphQL HTTP {}",
                status
            ))));
        }

        let response_text = response.text().await.map_err(|e| {
            AppError::Download(DownloadError::Instagram(format!(
                "Failed to read GraphQL response: {}",
                e
            )))
        })?;

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

    /// Fetch profile data from Instagram's GraphQL API.
    ///
    /// Returns profile info and recent posts for the profile browsing UI.
    pub async fn fetch_profile(&self, username: &str) -> Result<InstagramProfile, AppError> {
        if !self.rate_limiter.acquire() {
            return Err(AppError::Download(DownloadError::Instagram("Rate limited".to_string())));
        }

        let doc_id = config::INSTAGRAM_PROFILE_DOC_ID.as_str();
        let variables = format!(r#"{{"username":"{}"}}"#, username);

        let mut profile_request = self
            .client
            .post(GRAPHQL_ENDPOINT)
            .header("X-IG-App-ID", IG_APP_ID)
            .header("X-FB-LSD", FB_LSD_TOKEN)
            .header("X-ASBD-ID", FB_ASBD_ID)
            .header("X-Requested-With", "XMLHttpRequest")
            .header("Content-Type", "application/x-www-form-urlencoded")
            .header("Referer", "https://www.instagram.com/")
            .header("Origin", "https://www.instagram.com")
            .header("Accept", "*/*")
            .header("Accept-Language", "en-US,en;q=0.9")
            .header("Sec-Fetch-Site", "same-origin")
            .header("Sec-Fetch-Mode", "cors")
            .header("Sec-Fetch-Dest", "empty");

        // Add Instagram cookies + CSRF token if available
        if let Some(cookie_header) = crate::download::cookies::load_instagram_cookie_header() {
            if let Some(csrf_token) = crate::download::cookies::load_ig_csrf_token() {
                log::info!(
                    "InstagramSource: profile GraphQL auth: cookies=yes, csrftoken=yes (len={})",
                    csrf_token.len()
                );
                profile_request = profile_request.header("X-CSRFToken", csrf_token);
            } else {
                log::warn!("InstagramSource: profile GraphQL auth: cookies=yes, csrftoken=NOT FOUND");
            }
            profile_request = profile_request.header("Cookie", cookie_header);
        } else {
            log::info!("InstagramSource: profile GraphQL auth: no cookies available");
        }

        let response = profile_request
            .body(format!(
                "doc_id={}&variables={}&lsd={}",
                doc_id,
                urlencoding::encode(&variables),
                FB_LSD_TOKEN
            ))
            .send()
            .await
            .map_err(|e| {
                AppError::Download(DownloadError::Instagram(format!(
                    "Profile GraphQL request failed: {}",
                    e
                )))
            })?;

        if !response.status().is_success() {
            return Err(AppError::Download(DownloadError::Instagram(format!(
                "Profile GraphQL HTTP {}",
                response.status()
            ))));
        }

        let body: serde_json::Value = response.json().await.map_err(|e| {
            AppError::Download(DownloadError::Instagram(format!(
                "Failed to parse profile response: {}",
                e
            )))
        })?;

        let user = body
            .pointer("/data/user")
            .ok_or_else(|| AppError::Download(DownloadError::Instagram("Profile not found".to_string())))?;

        let full_name = user.get("full_name").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let biography = user.get("biography").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let profile_pic_url = user
            .get("profile_pic_url_hd")
            .or_else(|| user.get("profile_pic_url"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let is_private = user.get("is_private").and_then(|v| v.as_bool()).unwrap_or(false);
        let post_count = user
            .pointer("/edge_owner_to_timeline_media/count")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;
        let follower_count = user
            .pointer("/edge_followed_by/count")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;

        // Extract recent posts
        let posts: Vec<ProfilePost> = user
            .pointer("/edge_owner_to_timeline_media/edges")
            .and_then(|v| v.as_array())
            .map(|edges| {
                edges
                    .iter()
                    .take(12) // Max 12 posts for the grid
                    .filter_map(|edge| {
                        let node = edge.get("node")?;
                        let shortcode = node.get("shortcode")?.as_str()?.to_string();
                        let is_video = node.get("is_video").and_then(|v| v.as_bool()).unwrap_or(false);
                        let typename = node.get("__typename").and_then(|v| v.as_str()).unwrap_or("");
                        let is_carousel = typename == "GraphSidecar";
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

        Ok(InstagramProfile {
            username: username.to_string(),
            full_name,
            biography,
            profile_pic_url,
            is_private,
            post_count,
            follower_count,
            posts,
            end_cursor,
        })
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
                format!("{}...", &first_line[..97])
            } else {
                first_line.to_string()
            }
        };

        Ok(InstagramPreviewInfo {
            title,
            artist: format!("@{}", media.username),
            thumbnail_url: media.thumbnail_url,
            duration_secs: media.duration_secs.map(|d| d as u32),
            is_video: primary.is_video,
            is_carousel,
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
                let primary = &media.items[0];

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

                let is_carousel = media.items.len() > 1;
                log::info!(
                    "InstagramSource: downloading {} ({}{}) by @{}",
                    if primary.is_video { "video" } else { "photo" },
                    mime_hint,
                    if is_carousel {
                        format!(", carousel with {} items", media.items.len())
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

                // Download additional carousel items (if any)
                let additional_files = if is_carousel {
                    let mut extras = Vec::new();
                    for (i, item) in media.items.iter().skip(1).enumerate() {
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
                            i + 2,
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
                                log::warn!("InstagramSource: failed to download carousel item {}: {}", i + 2, e);
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
