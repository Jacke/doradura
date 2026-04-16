//! Telegram UI for content subscriptions (subscribe, manage, notifications).
//!
//! Handles `/subscriptions` command and `cw:` callback prefix.

use crate::core::config;
use crate::download::source::instagram::InstagramSource;
use crate::storage::db::DbPool;
use crate::storage::SharedStorage;
use crate::telegram::cb;
use crate::telegram::{Bot, BotExt};
use crate::watcher::traits::WatchNotification;
use crate::watcher::WatcherRegistry;
use futures_util::StreamExt as _;
use sqlx::{pool::PoolConnection, Postgres};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{
    CallbackQueryId, ChatId, InlineKeyboardMarkup, InputFile, InputMedia, InputMediaPhoto, InputMediaVideo, MessageId,
};
use tokio::sync::mpsc;

/// Max subscriptions by plan.
fn max_subscriptions_for_plan(plan: &str) -> u32 {
    match plan {
        "vip" => *config::watcher::MAX_SUBS_VIP,
        "premium" => *config::watcher::MAX_SUBS_PREMIUM,
        _ => *config::watcher::MAX_SUBS_FREE,
    }
}

// ─── /subscriptions command ───

/// Handle the /subscriptions command: show list of user's subscriptions.
pub async fn handle_subscriptions_command(
    bot: &Bot,
    chat_id: ChatId,
    db_pool: &Arc<DbPool>,
    shared_storage: &Arc<SharedStorage>,
) {
    let _ = db_pool;
    let subs = match shared_storage.get_user_content_subscriptions(chat_id.0).await {
        Ok(s) => s,
        Err(e) => {
            log::error!("Failed to get subscriptions: {}", e);
            let _ = bot.send_message(chat_id, "Failed to load subscriptions").await;
            return;
        }
    };

    let plan = shared_storage
        .get_user(chat_id.0)
        .await
        .ok()
        .flatten()
        .map(|u| u.plan.to_string())
        .unwrap_or_else(|| "free".to_string());
    let max_subs = max_subscriptions_for_plan(&plan);

    if subs.is_empty() {
        let text = format!(
            "🔔 You have no active subscriptions.\n\n\
             Send an Instagram profile link to subscribe to updates.\n\
             Limit: 0/{} subscriptions",
            max_subs
        );
        let _ = bot.send_message(chat_id, text).await;
        return;
    }

    let mut text = format!("🔔 Your Subscriptions ({}/{})\n", subs.len(), max_subs);

    let mut buttons: Vec<Vec<teloxide::types::InlineKeyboardButton>> = Vec::new();

    for (i, sub) in subs.iter().enumerate() {
        let source_emoji = match sub.source_type.as_str() {
            "instagram" => "📸",
            _ => "🔗",
        };

        let types: Vec<&str> = {
            let mut t = Vec::new();
            if sub.watch_mask & 1 != 0 {
                t.push("Posts");
            }
            if sub.watch_mask & 2 != 0 {
                t.push("Stories");
            }
            t
        };
        let types_str = types.join(" + ");

        let last_check = sub.last_checked_at.as_deref().unwrap_or("never");

        text.push_str(&format!(
            "\n{}. {} {} — {}\n   Last check: {}",
            i + 1,
            source_emoji,
            sub.display_name,
            types_str,
            last_check,
        ));

        if let Some(ref err) = sub.last_error {
            let truncated_err: String = err.chars().take(50).collect();
            text.push_str(&format!("\n   ⚠️ {}", truncated_err));
        }

        buttons.push(vec![cb(
            format!("Manage {}", sub.display_name),
            format!("cw:manage:{}", sub.id),
        )]);
    }

    let keyboard = InlineKeyboardMarkup::new(buttons);
    let _ = bot.send_message(chat_id, text).reply_markup(keyboard).await;
}

// ─── Subscribe flow (triggered from ig:sub:<username>) ───

/// Show subscribe confirmation dialog.
pub async fn show_subscribe_confirm(
    bot: &Bot,
    chat_id: ChatId,
    username: &str,
    db_pool: &Arc<DbPool>,
    shared_storage: &Arc<SharedStorage>,
    registry: &WatcherRegistry,
) {
    let watcher = match registry.get("instagram") {
        Some(w) => w,
        None => {
            let _ = bot.send_message(chat_id, "Instagram watcher not available").await;
            return;
        }
    };

    let _ = db_pool;
    let plan = shared_storage
        .get_user(chat_id.0)
        .await
        .ok()
        .flatten()
        .map(|u| u.plan.to_string())
        .unwrap_or_else(|| "free".to_string());
    let current_count = match shared_storage.count_user_content_subscriptions(chat_id.0).await {
        Ok(count) => count,
        Err(e) => {
            log::error!("Failed to count subscriptions for user {}: {}", chat_id.0, e);
            let _ = bot
                .send_message(
                    chat_id,
                    "❌ Could not check your subscription count. Please try again later.",
                )
                .await;
            return;
        }
    };
    let max_subs = max_subscriptions_for_plan(&plan);

    // Check if already subscribed
    if let Ok(Some(existing)) = shared_storage
        .has_content_subscription(chat_id.0, "instagram", username)
        .await
    {
        if existing.is_active {
            let _ = bot
                .send_message(chat_id, format!("You're already subscribed to @{}!", username))
                .await;
            return;
        }
    }

    if current_count >= max_subs {
        let _ = bot
            .send_message(
                chat_id,
                format!(
                    "You've reached the subscription limit ({}/{}).\n\
                     Upgrade your plan or unsubscribe from an existing one.",
                    current_count, max_subs
                ),
            )
            .await;
        return;
    }

    // Resolve the profile to validate it exists
    let (display_name, _meta) = match watcher.resolve_source(username).await {
        Ok(r) => r,
        Err(e) => {
            let _ = bot.send_message(chat_id, format!("Cannot subscribe: {}", e)).await;
            return;
        }
    };

    let default_mask = watcher.default_watch_mask();
    let text = format!("Subscribe to {} updates?", display_name);

    let has_cookies = crate::download::cookies::load_instagram_cookie_header().is_some();
    let effective_mask = if has_cookies { default_mask } else { 1 }; // Posts only without cookies

    let mut toggle_row = vec![cb(
        format!("{}Posts", if effective_mask & 1 != 0 { "✅ " } else { "☐ " }),
        format!("cw:ptog:{}:1:{}", username, effective_mask),
    )];

    if has_cookies {
        toggle_row.push(cb(
            format!("{}Stories", if effective_mask & 2 != 0 { "✅ " } else { "☐ " }),
            format!("cw:ptog:{}:2:{}", username, effective_mask),
        ));
    }

    let keyboard = InlineKeyboardMarkup::new(vec![
        toggle_row,
        vec![
            cb("✔ Confirm", format!("cw:ok:instagram:{}:{}", username, effective_mask)),
            cb("✖ Cancel", "cw:cancel".to_string()),
        ],
    ]);

    let _ = bot.send_message(chat_id, text).reply_markup(keyboard).await;
}

// ─── Callback handler (cw: prefix) ───

/// Handle all `cw:` callbacks.
pub async fn handle_subscription_callback(
    bot: &Bot,
    callback_id: &CallbackQueryId,
    chat_id: ChatId,
    message_id: MessageId,
    data: &str,
    db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
    registry: &WatcherRegistry,
) {
    let _ = bot.answer_callback_query(callback_id.clone()).await;

    let parts: Vec<&str> = data.split(':').collect();
    if parts.len() < 2 {
        return;
    }

    match parts[1] {
        "ok" => {
            // cw:ok:ig:<username>:<mask>
            if parts.len() >= 5 {
                let source_type = parts[2];
                let source_id = parts[3];
                let mask: u32 = parts[4].parse().unwrap_or(3);
                handle_confirm_subscribe(
                    bot,
                    chat_id,
                    message_id,
                    source_type,
                    source_id,
                    mask,
                    &db_pool,
                    &shared_storage,
                    registry,
                )
                .await;
            }
        }
        "cancel" => {
            bot.try_delete(chat_id, message_id).await;
        }
        "ptog" => {
            // cw:ptog:<username>:<bit>:<current_mask>
            if parts.len() >= 5 {
                let username = parts[2];
                let bit: u32 = parts[3].parse().unwrap_or(0);
                let current_mask: u32 = parts[4].parse().unwrap_or(3);
                let new_mask = current_mask ^ bit;
                // Don't allow mask=0
                let new_mask = if new_mask == 0 { bit } else { new_mask };
                update_toggle_keyboard(bot, chat_id, message_id, username, new_mask).await;
            }
        }
        "manage" => {
            // cw:manage:<id>
            if parts.len() >= 3 {
                if let Ok(sub_id) = parts[2].parse::<i64>() {
                    show_manage_subscription(bot, chat_id, message_id, sub_id, &db_pool, &shared_storage).await;
                }
            }
        }
        "unsub" => {
            // cw:unsub:<id>
            if parts.len() >= 3 {
                if let Ok(sub_id) = parts[2].parse::<i64>() {
                    handle_unsubscribe(bot, chat_id, message_id, sub_id, &db_pool, &shared_storage).await;
                }
            }
        }
        "tog" => {
            // cw:tog:<id>:<bit>
            if parts.len() >= 4 {
                if let (Ok(sub_id), Ok(bit)) = (parts[2].parse::<i64>(), parts[3].parse::<u32>()) {
                    handle_toggle_content_type(bot, chat_id, message_id, sub_id, bit, &db_pool, &shared_storage).await;
                }
            }
        }
        "list" => {
            bot.try_delete(chat_id, message_id).await;
            handle_subscriptions_command(bot, chat_id, &db_pool, &shared_storage).await;
        }
        _ => {}
    }
}

async fn handle_confirm_subscribe(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    source_type: &str,
    source_id: &str,
    mask: u32,
    db_pool: &Arc<DbPool>,
    shared_storage: &Arc<SharedStorage>,
    registry: &WatcherRegistry,
) {
    let watcher = match registry.get(source_type) {
        Some(w) => w,
        None => {
            let _ = bot
                .edit_message_text(chat_id, message_id, "Watcher not available")
                .await;
            return;
        }
    };

    // Resolve source again to get display_name and meta
    let (display_name, meta) = match watcher.resolve_source(source_id).await {
        Ok(r) => r,
        Err(e) => {
            let _ = bot
                .edit_message_text(chat_id, message_id, format!("Failed: {}", e))
                .await;
            return;
        }
    };

    let _ = db_pool;
    match shared_storage
        .upsert_content_subscription(
            chat_id.0,
            source_type,
            source_id,
            display_name.as_str(),
            mask,
            meta.as_ref(),
        )
        .await
    {
        Ok(_id) => {
            let types: Vec<&str> = {
                let mut t = Vec::new();
                if mask & 1 != 0 {
                    t.push("Posts");
                }
                if mask & 2 != 0 {
                    t.push("Stories");
                }
                t
            };

            let text = format!(
                "🔔 Subscribed to {} ({})\n\nYou'll be notified when new content appears.",
                display_name,
                types.join(" + "),
            );

            let keyboard = InlineKeyboardMarkup::new(vec![vec![cb("📋 My Subscriptions", "cw:list".to_string())]]);

            let _ = bot
                .edit_message_text(chat_id, message_id, text)
                .reply_markup(keyboard)
                .await;

            log::info!(
                "User {} subscribed to {}:{} (mask={})",
                chat_id.0,
                source_type,
                source_id,
                mask
            );
        }
        Err(e) => {
            let _ = bot
                .edit_message_text(chat_id, message_id, format!("Failed to subscribe: {}", e))
                .await;
        }
    }
}

async fn update_toggle_keyboard(bot: &Bot, chat_id: ChatId, message_id: MessageId, username: &str, new_mask: u32) {
    let has_cookies = crate::download::cookies::load_instagram_cookie_header().is_some();

    let mut toggle_row = vec![cb(
        format!("{}Posts", if new_mask & 1 != 0 { "✅ " } else { "☐ " }),
        format!("cw:ptog:{}:1:{}", username, new_mask),
    )];

    if has_cookies {
        toggle_row.push(cb(
            format!("{}Stories", if new_mask & 2 != 0 { "✅ " } else { "☐ " }),
            format!("cw:ptog:{}:2:{}", username, new_mask),
        ));
    }

    let keyboard = InlineKeyboardMarkup::new(vec![
        toggle_row,
        vec![
            cb("✔ Confirm", format!("cw:ok:instagram:{}:{}", username, new_mask)),
            cb("✖ Cancel", "cw:cancel".to_string()),
        ],
    ]);

    let _ = bot
        .edit_message_reply_markup(chat_id, message_id)
        .reply_markup(keyboard)
        .await;
}

async fn show_manage_subscription(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    sub_id: i64,
    db_pool: &Arc<DbPool>,
    shared_storage: &Arc<SharedStorage>,
) {
    let _ = db_pool;
    let sub = match shared_storage.get_content_subscription(sub_id).await {
        Ok(Some(s)) => s,
        _ => {
            let _ = bot
                .edit_message_text(chat_id, message_id, "Subscription not found")
                .await;
            return;
        }
    };

    if sub.user_id != chat_id.0 {
        log::warn!(
            "User {} attempted to access subscription {} owned by {}",
            chat_id.0,
            sub_id,
            sub.user_id
        );
        return;
    }

    let source_emoji = match sub.source_type.as_str() {
        "instagram" => "📸",
        _ => "🔗",
    };

    let text = format!(
        "{} {} — Manage Subscription\n\n\
         Source: {}\n\
         Status: {}\n\
         Last check: {}\n\
         Errors: {}",
        source_emoji,
        sub.display_name,
        sub.source_type,
        if sub.is_active { "Active" } else { "Inactive" },
        sub.last_checked_at.as_deref().unwrap_or("never"),
        sub.consecutive_errors,
    );

    let keyboard = InlineKeyboardMarkup::new(vec![
        vec![
            cb(
                format!("{}Posts", if sub.watch_mask & 1 != 0 { "✅ " } else { "☐ " }),
                format!("cw:tog:{}:1", sub_id),
            ),
            cb(
                format!("{}Stories", if sub.watch_mask & 2 != 0 { "✅ " } else { "☐ " }),
                format!("cw:tog:{}:2", sub_id),
            ),
        ],
        vec![
            cb("🔕 Unsubscribe", format!("cw:unsub:{}", sub_id)),
            cb("🔙 Back", "cw:list".to_string()),
        ],
    ]);

    let _ = bot
        .edit_message_text(chat_id, message_id, text)
        .reply_markup(keyboard)
        .await;
}

async fn handle_unsubscribe(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    sub_id: i64,
    db_pool: &Arc<DbPool>,
    shared_storage: &Arc<SharedStorage>,
) {
    let _ = db_pool;
    // Get sub info before deactivating; verify ownership
    let sub_info = shared_storage.get_content_subscription(sub_id).await.ok().flatten();
    if let Some(ref sub) = sub_info {
        if sub.user_id != chat_id.0 {
            log::warn!(
                "User {} attempted to access subscription {} owned by {}",
                chat_id.0,
                sub_id,
                sub.user_id
            );
            return;
        }
    }
    let display_name = sub_info.map(|s| s.display_name).unwrap_or_default();

    match shared_storage.deactivate_content_subscription(sub_id).await {
        Ok(()) => {
            let text = format!("🔕 Unsubscribed from {}", display_name);
            let keyboard = InlineKeyboardMarkup::new(vec![vec![cb("📋 My Subscriptions", "cw:list".to_string())]]);
            let _ = bot
                .edit_message_text(chat_id, message_id, text)
                .reply_markup(keyboard)
                .await;
            log::info!("User {} unsubscribed from sub_id={}", chat_id.0, sub_id);
        }
        Err(e) => {
            let _ = bot
                .edit_message_text(chat_id, message_id, format!("Failed: {}", e))
                .await;
        }
    }
}

async fn handle_toggle_content_type(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    sub_id: i64,
    bit: u32,
    db_pool: &Arc<DbPool>,
    shared_storage: &Arc<SharedStorage>,
) {
    let _ = db_pool;
    let sub = match shared_storage.get_content_subscription(sub_id).await {
        Ok(Some(s)) => s,
        _ => return,
    };

    if sub.user_id != chat_id.0 {
        log::warn!(
            "User {} attempted to access subscription {} owned by {}",
            chat_id.0,
            sub_id,
            sub.user_id
        );
        return;
    }

    let new_mask = sub.watch_mask ^ bit;
    // Don't allow mask=0
    let new_mask = if new_mask == 0 { bit } else { new_mask };

    if let Err(e) = shared_storage.update_content_watch_mask(sub_id, new_mask).await {
        log::error!("Failed to toggle content type: {}", e);
        return;
    }

    // Refresh the manage view
    show_manage_subscription(bot, chat_id, message_id, sub_id, db_pool, shared_storage).await;
}

// ─── Notification dispatcher ───

/// Download a media URL to a temp file. Returns None on failure.
async fn download_media_to_temp(client: &reqwest::Client, url: &str, is_video: bool) -> Option<PathBuf> {
    const MAX_SIZE: u64 = 50 * 1024 * 1024; // 50 MB Telegram limit
    const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

    let resp = match client.get(url).timeout(TIMEOUT).send().await {
        Ok(r) => r,
        Err(e) => {
            log::warn!("Media download failed for {}: {}", url, e);
            return None;
        }
    };

    if !resp.status().is_success() {
        log::warn!("Media download HTTP {}: {}", resp.status(), url);
        return None;
    }

    // Check content-length if available
    if let Some(len) = resp.content_length() {
        if len > MAX_SIZE {
            log::warn!("Media too large ({} bytes), skipping: {}", len, url);
            return None;
        }
    }

    let ext = if is_video { "mp4" } else { "jpg" };
    let temp_path = std::env::temp_dir().join(format!("dora_sub_{}_{}.{}", std::process::id(), next_temp_id(), ext));

    // HIGH-11: Stream to disk with a running byte-count guard instead of
    // buffering the entire response body in memory with `.bytes().await`.
    // This bounds peak RSS to one chunk (~8 KiB) regardless of file size.
    let mut file = match tokio::fs::File::create(&temp_path).await {
        Ok(f) => f,
        Err(e) => {
            log::warn!("Failed to create temp media file: {}", e);
            return None;
        }
    };

    let mut total: u64 = 0;
    let mut stream = resp.bytes_stream();
    while let Some(chunk_result) = stream.next().await {
        let chunk = match chunk_result {
            Ok(c) => c,
            Err(e) => {
                log::warn!("Media download read failed: {}", e);
                fs_err::tokio::remove_file(&temp_path).await.ok();
                return None;
            }
        };
        total += chunk.len() as u64;
        if total > MAX_SIZE {
            log::warn!("Media too large (>{} bytes), skipping: {}", MAX_SIZE, url);
            fs_err::tokio::remove_file(&temp_path).await.ok();
            return None;
        }
        if let Err(e) = tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await {
            log::warn!("Failed to write temp media chunk: {}", e);
            fs_err::tokio::remove_file(&temp_path).await.ok();
            return None;
        }
    }

    Some(temp_path)
}

static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

fn next_temp_id() -> u64 {
    TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// Send a story notification with media (photos/videos from CDN).
async fn send_story_notification(
    bot: &Bot,
    client: &reqwest::Client,
    chat_id: ChatId,
    notification: &WatchNotification,
) -> Result<(), teloxide::RequestError> {
    let media = &notification.update.media;
    if media.is_empty() {
        return send_text_notification(bot, chat_id, notification).await;
    }

    let unsub_keyboard = InlineKeyboardMarkup::new(vec![vec![cb(
        "🔕 Unsubscribe",
        format!("cw:unsub:{}", notification.subscription_id),
    )]]);

    let caption = format!("📱 {}", notification.update.description);

    // Download all media in parallel
    let futures: Vec<_> = media
        .iter()
        .map(|a| download_media_to_temp(client, &a.media_url, a.is_video))
        .collect();
    let results = futures_util::future::join_all(futures).await;
    let downloaded: Vec<(PathBuf, bool)> = results
        .into_iter()
        .zip(media.iter())
        .filter_map(|(path, a)| path.map(|p| (p, a.is_video)))
        .collect();

    if downloaded.is_empty() {
        return send_text_notification(bot, chat_id, notification).await;
    }

    let result = if downloaded.len() == 1 {
        // Single item: send with caption + unsub keyboard
        let (path, is_video) = &downloaded[0];
        if *is_video {
            bot.send_video(chat_id, InputFile::file(path))
                .caption(&caption)
                .reply_markup(unsub_keyboard)
                .await
                .map(|_| ())
        } else {
            bot.send_photo(chat_id, InputFile::file(path))
                .caption(&caption)
                .reply_markup(unsub_keyboard)
                .await
                .map(|_| ())
        }
    } else {
        // Multiple items: send as media group (caption on first), then separate unsub button
        let media_group: Vec<InputMedia> = downloaded
            .iter()
            .enumerate()
            .map(|(i, (path, is_video))| {
                if *is_video {
                    let mut v = InputMediaVideo::new(InputFile::file(path));
                    if i == 0 {
                        v = v.caption(&caption);
                    }
                    InputMedia::Video(v)
                } else {
                    let mut p = InputMediaPhoto::new(InputFile::file(path));
                    if i == 0 {
                        p = p.caption(&caption);
                    }
                    InputMedia::Photo(p)
                }
            })
            .collect();

        match bot.send_media_group(chat_id, media_group).await {
            Ok(_) => {
                let _ = bot
                    .send_message(chat_id, notification.display_name.clone())
                    .reply_markup(unsub_keyboard)
                    .await;
                Ok(())
            }
            Err(e) => Err(e),
        }
    };

    // Clean up temp files
    for (path, _) in &downloaded {
        let _ = fs_err::tokio::remove_file(path).await;
    }

    if result.is_err() {
        // Fallback to text on send failure
        log::warn!("Story media send failed, falling back to text");
        return send_text_notification(bot, chat_id, notification).await;
    }

    Ok(())
}

/// Send a post notification with media (resolved via GraphQL).
async fn send_post_notification(
    bot: &Bot,
    client: &reqwest::Client,
    ig_source: &InstagramSource,
    chat_id: ChatId,
    notification: &WatchNotification,
) -> Result<(), teloxide::RequestError> {
    let shortcode = match &notification.update.shortcode {
        Some(sc) => sc.clone(),
        None => return send_text_notification(bot, chat_id, notification).await,
    };

    // Resolve post media via GraphQL
    let gql_media = match ig_source.fetch_graphql_media(&shortcode).await {
        Ok(m) => m,
        Err(e) => {
            log::warn!("Failed to resolve post media for {}: {}", shortcode, e);
            return send_text_notification(bot, chat_id, notification).await;
        }
    };

    let unsub_keyboard = InlineKeyboardMarkup::new(vec![vec![cb(
        "🔕 Unsubscribe",
        format!("cw:unsub:{}", notification.subscription_id),
    )]]);

    let caption = format!(
        "📸 New post by {}\n{}",
        notification.display_name, notification.update.url
    );

    // Download media files in parallel
    let futures: Vec<_> = gql_media
        .items
        .iter()
        .filter_map(|item| {
            let url = if item.is_video {
                item.video_url.as_deref()
            } else {
                item.display_url.as_deref()
            };
            url.map(|u| {
                let is_video = item.is_video;
                async move { (download_media_to_temp(client, u, is_video).await, is_video) }
            })
        })
        .collect();
    let results = futures_util::future::join_all(futures).await;
    let downloaded: Vec<(PathBuf, bool)> = results
        .into_iter()
        .filter_map(|(path, is_video)| path.map(|p| (p, is_video)))
        .collect();

    if downloaded.is_empty() {
        return send_text_notification(bot, chat_id, notification).await;
    }

    let result = if downloaded.len() == 1 {
        let (path, is_video) = &downloaded[0];
        if *is_video {
            bot.send_video(chat_id, InputFile::file(path))
                .caption(&caption)
                .reply_markup(unsub_keyboard)
                .await
                .map(|_| ())
        } else {
            bot.send_photo(chat_id, InputFile::file(path))
                .caption(&caption)
                .reply_markup(unsub_keyboard)
                .await
                .map(|_| ())
        }
    } else {
        // Carousel: send as media group
        let media_group: Vec<InputMedia> = downloaded
            .iter()
            .enumerate()
            .map(|(i, (path, is_video))| {
                if *is_video {
                    let mut v = InputMediaVideo::new(InputFile::file(path));
                    if i == 0 {
                        v = v.caption(&caption);
                    }
                    InputMedia::Video(v)
                } else {
                    let mut p = InputMediaPhoto::new(InputFile::file(path));
                    if i == 0 {
                        p = p.caption(&caption);
                    }
                    InputMedia::Photo(p)
                }
            })
            .collect();

        match bot.send_media_group(chat_id, media_group).await {
            Ok(_) => {
                let _ = bot.send_message(chat_id, "👆").reply_markup(unsub_keyboard).await;
                Ok(())
            }
            Err(e) => Err(e),
        }
    };

    // Clean up temp files
    for (path, _) in &downloaded {
        let _ = fs_err::tokio::remove_file(path).await;
    }

    if result.is_err() {
        log::warn!("Post media send failed, falling back to text");
        return send_text_notification(bot, chat_id, notification).await;
    }

    Ok(())
}

/// Fallback: send a plain text notification (original behavior).
async fn send_text_notification(
    bot: &Bot,
    chat_id: ChatId,
    notification: &WatchNotification,
) -> Result<(), teloxide::RequestError> {
    let keyboard = InlineKeyboardMarkup::new(vec![vec![cb(
        "🔕 Unsubscribe",
        format!("cw:unsub:{}", notification.subscription_id),
    )]]);

    let text = match notification.update.content_type.as_str() {
        "post" => format!(
            "📸 New post by {}\n{}",
            notification.display_name, notification.update.url
        ),
        "story" => format!("📱 {}", notification.update.description),
        _ => notification.update.description.clone(),
    };

    bot.send_message(chat_id, &text)
        .reply_markup(keyboard)
        .await
        .map(|_| ())
}

/// Start the notification dispatcher that receives WatchNotifications
/// and sends formatted Telegram messages with media.
pub fn start_notification_dispatcher(
    bot: Bot,
    db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
    mut rx: mpsc::Receiver<WatchNotification>,
    lock_conn: Option<PoolConnection<Postgres>>,
) {
    tokio::spawn(async move {
        let _lock_conn = lock_conn;
        let http_client = reqwest::Client::new();
        let ig_source = InstagramSource::new();

        while let Some(notification) = rx.recv().await {
            let chat_id = ChatId(notification.user_id);

            let result = match notification.update.content_type.as_str() {
                "story" => send_story_notification(&bot, &http_client, chat_id, &notification).await,
                "post" => send_post_notification(&bot, &http_client, &ig_source, chat_id, &notification).await,
                _ => send_text_notification(&bot, chat_id, &notification).await,
            };

            if let Err(e) = result {
                handle_send_error(e, notification.user_id, &db_pool, &shared_storage);
            }

            // Flood control between notifications
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        log::warn!("Notification dispatcher channel closed");
    });
}

/// Handle send errors — deactivate subscriptions if bot is blocked.
fn handle_send_error(
    err: teloxide::RequestError,
    user_id: i64,
    db_pool: &Arc<DbPool>,
    shared_storage: &Arc<SharedStorage>,
) {
    let err_str = err.to_string();
    if err_str.contains("Forbidden") || err_str.contains("blocked") || err_str.contains("deactivated") {
        log::warn!("Bot blocked by user {}, deactivating all subscriptions", user_id);
        let _ = db_pool;
        let shared_storage = Arc::clone(shared_storage);
        tokio::spawn(async move {
            let _ = shared_storage
                .deactivate_all_content_subscriptions_for_user(user_id)
                .await;
        });
    } else {
        log::error!("Failed to send notification to {}: {}", user_id, err);
    }
}
