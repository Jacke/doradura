//! Instagram content watcher — monitors profiles for new posts and stories.
//!
//! Delegates to the existing `InstagramSource` for API calls (no code duplication).

use crate::download::source::instagram::InstagramSource;
use crate::watcher::traits::{CheckResult, ContentWatcher, MediaAttachment, WatchUpdate};
use async_trait::async_trait;
use serde_json::{json, Value as JsonValue};

/// Bitmask constants for Instagram content types.
pub const MASK_POSTS: u32 = 1;
pub const MASK_STORIES: u32 = 2;

pub struct InstagramWatcher {
    source: InstagramSource,
}

impl InstagramWatcher {
    pub fn new() -> Self {
        Self {
            source: InstagramSource::new(),
        }
    }
}

impl Default for InstagramWatcher {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ContentWatcher for InstagramWatcher {
    fn source_type(&self) -> &str {
        "instagram"
    }

    fn display_name(&self) -> &str {
        "Instagram"
    }

    fn content_types(&self) -> Vec<(u32, &str)> {
        vec![(MASK_POSTS, "Posts"), (MASK_STORIES, "Stories")]
    }

    fn default_watch_mask(&self) -> u32 {
        MASK_POSTS | MASK_STORIES
    }

    async fn check(
        &self,
        source_id: &str,
        watch_mask: u32,
        last_state: Option<&JsonValue>,
        source_meta: Option<&JsonValue>,
    ) -> anyhow::Result<CheckResult> {
        let user_id = source_meta
            .and_then(|m| m.get("ig_user_id"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing ig_user_id in source_meta"))?;

        let is_first_check = last_state.is_none();
        let mut updates = Vec::new();

        // Current state from DB (or defaults)
        let prev_shortcode = last_state
            .and_then(|s| s.get("last_shortcode"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let prev_story_ts = last_state
            .and_then(|s| s.get("last_story_ts"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0);

        let mut new_last_shortcode = prev_shortcode.to_string();
        let mut new_last_story_ts = prev_story_ts;

        // Check posts
        if watch_mask & MASK_POSTS != 0 {
            match self.source.fetch_profile(source_id).await {
                Ok(profile) => {
                    if let Some(first_post) = profile.posts.first() {
                        if !is_first_check && first_post.shortcode != prev_shortcode && !prev_shortcode.is_empty() {
                            // Find all new posts (those before the last-seen shortcode), cap at 5
                            for post in profile.posts.iter().take(5) {
                                if post.shortcode == prev_shortcode {
                                    break;
                                }
                                updates.push(WatchUpdate {
                                    content_type: "post".to_string(),
                                    url: format!("https://www.instagram.com/p/{}/", post.shortcode),
                                    description: format!(
                                        "New {} by @{}",
                                        if post.is_video { "reel" } else { "post" },
                                        source_id
                                    ),
                                    shortcode: Some(post.shortcode.clone()),
                                    media: vec![],
                                });
                            }
                        }
                        new_last_shortcode = first_post.shortcode.clone();
                    }
                }
                Err(e) => {
                    log::warn!("InstagramWatcher: failed to fetch profile @{}: {}", source_id, e);
                    anyhow::bail!("Posts check failed: {}", e);
                }
            }
        }

        // Check stories (requires cookies)
        if watch_mask & MASK_STORIES != 0 {
            match self.source.fetch_stories(user_id).await {
                Ok(stories) => {
                    if let Some(latest) = stories.iter().filter_map(|s| s.taken_at).max() {
                        if !is_first_check && latest > prev_story_ts {
                            let new_stories: Vec<_> = stories
                                .iter()
                                .filter(|s| s.taken_at.unwrap_or(0) > prev_story_ts)
                                .take(10) // Telegram media group max
                                .collect();
                            let new_count = new_stories.len();
                            if new_count > 0 {
                                updates.push(WatchUpdate {
                                    content_type: "story".to_string(),
                                    url: format!("https://www.instagram.com/stories/{}/", source_id),
                                    description: format!(
                                        "@{} posted {} new {}",
                                        source_id,
                                        new_count,
                                        if new_count == 1 { "story" } else { "stories" }
                                    ),
                                    shortcode: None,
                                    media: new_stories
                                        .iter()
                                        .map(|s| MediaAttachment {
                                            media_url: s.media_url.clone(),
                                            is_video: s.is_video,
                                            duration_secs: s.duration_secs,
                                        })
                                        .collect(),
                                });
                            }
                        }
                        new_last_story_ts = latest;
                    }
                }
                Err(e) => {
                    // Stories failing is not fatal — posts may still succeed
                    log::warn!("InstagramWatcher: failed to fetch stories for @{}: {}", source_id, e);
                }
            }
        }

        Ok(CheckResult {
            updates,
            new_state: json!({
                "last_shortcode": new_last_shortcode,
                "last_story_ts": new_last_story_ts,
            }),
            new_meta: None,
        })
    }

    async fn resolve_source(&self, source_id: &str) -> anyhow::Result<(String, Option<JsonValue>)> {
        let profile = self
            .source
            .fetch_profile(source_id)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to resolve Instagram profile @{}: {}", source_id, e))?;

        if profile.is_private {
            anyhow::bail!("@{} is a private account", source_id);
        }

        let display_name = format!("@{}", profile.username);
        let meta = profile.user_id.map(|uid| json!({"ig_user_id": uid}));

        Ok((display_name, meta))
    }
}
