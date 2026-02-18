//! Instagram profile browsing UI and callback handlers.
//!
//! When a user sends `instagram.com/<username>`, shows a profile card with
//! tabbed navigation: Posts (default), Highlights, and Stories.
//! Each tab shows an inline keyboard grid of downloadable items.

use crate::download::source::instagram::InstagramSource;
use crate::i18n;
use crate::telegram::Bot;
use fluent_templates::fluent_bundle::FluentArgs;
use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup, InputFile};
use unic_langid::LanguageIdentifier;

/// Show an Instagram profile card with a grid of downloadable posts.
pub async fn show_instagram_profile(bot: &Bot, chat_id: ChatId, username: &str, lang: &LanguageIdentifier) {
    let source = InstagramSource::new();

    let profile = match source.fetch_profile(username).await {
        Ok(p) => p,
        Err(e) => {
            log::warn!("Failed to fetch Instagram profile @{}: {}", username, e);
            let mut args = FluentArgs::new();
            args.set("username", username.to_string());
            let msg = if e.to_string().contains("Private") || e.to_string().contains("login required") {
                i18n::t_args(lang, "instagram-private", &args)
            } else {
                i18n::t_args(lang, "instagram-profile-error", &args)
            };
            let _ = bot.send_message(chat_id, msg).await;
            return;
        }
    };

    if profile.is_private {
        let mut args = FluentArgs::new();
        args.set("username", username.to_string());
        let _ = bot
            .send_message(chat_id, i18n::t_args(lang, "instagram-private", &args))
            .await;
        return;
    }

    let caption = build_profile_caption(&profile, lang);
    let keyboard = build_posts_keyboard(&profile, lang);

    // Try to send profile pic as photo with caption
    if !profile.profile_pic_url.is_empty() {
        match bot
            .send_photo(
                chat_id,
                InputFile::url(
                    profile
                        .profile_pic_url
                        .parse()
                        .unwrap_or_else(|_| "https://instagram.com/favicon.ico".parse().unwrap()),
                ),
            )
            .caption(&caption)
            .reply_markup(keyboard.clone())
            .await
        {
            Ok(_) => return,
            Err(e) => {
                log::warn!("Failed to send profile photo: {}, falling back to text", e);
            }
        }
    }

    // Fallback: send as text message
    let _ = bot.send_message(chat_id, &caption).reply_markup(keyboard).await;
}

/// Build profile caption text.
fn build_profile_caption(
    profile: &crate::download::source::instagram::InstagramProfile,
    lang: &LanguageIdentifier,
) -> String {
    let bio_preview = if profile.biography.len() > 200 {
        format!("{}...", &profile.biography[..197])
    } else {
        profile.biography.clone()
    };

    let mut stats_args = FluentArgs::new();
    stats_args.set("posts", format_count(profile.post_count));
    stats_args.set("followers", format_count(profile.follower_count));
    let stats_line = i18n::t_args(lang, "instagram-profile-posts", &stats_args);

    format!(
        "{name} (@{username})\n\
         {bio}\n\n\
         {stats}",
        name = profile.full_name,
        username = profile.username,
        bio = bio_preview,
        stats = stats_line,
    )
}

/// Build the Posts tab keyboard with grid + tab bar.
fn build_posts_keyboard(
    profile: &crate::download::source::instagram::InstagramProfile,
    lang: &LanguageIdentifier,
) -> InlineKeyboardMarkup {
    let username = &profile.username;
    let has_cookies = crate::download::cookies::load_instagram_cookie_header().is_some();

    let mut rows: Vec<Vec<InlineKeyboardButton>> = Vec::new();

    // Tab bar (Posts is active)
    let mut tabs = vec![InlineKeyboardButton::callback(
        "[ Posts ]".to_string(),
        "ig:noop".to_string(), // active tab — no-op
    )];
    if has_cookies {
        tabs.push(InlineKeyboardButton::callback(
            "Highlights".to_string(),
            format!("ig:tab:hl:{}", username),
        ));
        tabs.push(InlineKeyboardButton::callback(
            "Stories".to_string(),
            format!("ig:tab:stories:{}", username),
        ));
    }
    rows.push(tabs);

    // Post grid (4 columns x 3 rows)
    let mut current_row: Vec<InlineKeyboardButton> = Vec::new();
    for (i, post) in profile.posts.iter().enumerate() {
        let emoji = if post.is_carousel {
            "🎠"
        } else if post.is_video {
            "🎬"
        } else {
            "📷"
        };
        let label = format!("{} {}", emoji, i + 1);
        let callback = format!("ig:dl:{}", post.shortcode);
        current_row.push(InlineKeyboardButton::callback(label, callback));
        if current_row.len() == 4 || i == profile.posts.len() - 1 {
            rows.push(std::mem::take(&mut current_row));
        }
    }

    // Pagination
    if profile.end_cursor.is_some() && profile.posts.len() >= 12 {
        rows.push(vec![InlineKeyboardButton::callback(
            i18n::t(lang, "instagram-more"),
            format!("ig:page:{}", username),
        )]);
    }

    InlineKeyboardMarkup::new(rows)
}

/// Handle Instagram callback queries.
///
/// Callback formats:
/// - `ig:dl:<shortcode>` — download a post
/// - `ig:page:<username>` — load more posts
/// - `ig:tab:hl:<username>` — switch to highlights tab
/// - `ig:tab:stories:<username>` — switch to stories tab
/// - `ig:tab:posts:<username>` — switch back to posts tab
/// - `ig:hl:<highlight_id>` — browse highlight items
/// - `ig:story:<user_id>:<item_index>` — download a story item
/// - `ig:hldl:<highlight_id>:<item_index>` — download a highlight item
/// - `ig:noop` — no-op (active tab)
pub async fn handle_instagram_callback(
    bot: &Bot,
    callback_id: &teloxide::types::CallbackQueryId,
    chat_id: ChatId,
    data: &str,
) -> Result<(), teloxide::RequestError> {
    let _ = bot.answer_callback_query(callback_id.clone()).await;

    let parts: Vec<&str> = data.splitn(5, ':').collect();
    if parts.len() < 2 {
        return Ok(());
    }

    match parts[1] {
        "dl" if parts.len() >= 3 => {
            let shortcode = parts[2];
            let url = format!("https://www.instagram.com/p/{}/", shortcode);
            let _ = bot.send_message(chat_id, &url).await;
        }
        "page" if parts.len() >= 3 => {
            let username = parts[2];
            let lang = i18n::lang_from_code("ru");
            show_instagram_profile(bot, chat_id, username, &lang).await;
        }
        "tab" if parts.len() >= 4 => {
            let tab = parts[2];
            let username = parts[3];
            handle_tab_switch(bot, chat_id, data, tab, username).await;
        }
        "hl" if parts.len() >= 3 => {
            // Browse highlight items: ig:hl:<highlight_id>
            let highlight_id = parts[2];
            handle_highlight_browse(bot, chat_id, data, highlight_id).await;
        }
        "hldl" if parts.len() >= 4 => {
            // Download highlight item: ig:hldl:<highlight_id>:<index>
            let highlight_id = parts[2];
            let index: usize = parts[3].parse().unwrap_or(0);
            handle_story_download(bot, chat_id, &format!("highlight:{}", highlight_id), index).await;
        }
        "storydl" if parts.len() >= 4 => {
            // Download story item: ig:storydl:<user_id>:<index>
            let user_id = parts[2];
            let index: usize = parts[3].parse().unwrap_or(0);
            handle_story_download(bot, chat_id, user_id, index).await;
        }
        "noop" => {} // Active tab — do nothing
        _ => {}
    }

    Ok(())
}

/// Handle tab switching (edit existing message keyboard).
async fn handle_tab_switch(bot: &Bot, chat_id: ChatId, _data: &str, tab: &str, username: &str) {
    let source = InstagramSource::new();
    let lang = i18n::lang_from_code("ru");

    // Try to get message_id from callback context — we'll send a new message
    // since we may need to update the caption too
    match tab {
        "posts" => {
            show_instagram_profile(bot, chat_id, username, &lang).await;
        }
        "hl" => {
            // Load highlights tray
            let profile = match source.fetch_profile(username).await {
                Ok(p) => p,
                Err(e) => {
                    log::warn!("Failed to fetch profile for highlights: {}", e);
                    let _ = bot.send_message(chat_id, "Failed to load profile for highlights").await;
                    return;
                }
            };

            let user_id = match &profile.user_id {
                Some(id) => id.clone(),
                None => {
                    let _ = bot
                        .send_message(chat_id, "Cannot load highlights (user_id not available)")
                        .await;
                    return;
                }
            };

            let highlights = match source.fetch_highlights(&user_id).await {
                Ok(h) => h,
                Err(e) => {
                    log::warn!("Failed to fetch highlights for @{}: {}", username, e);
                    let _ = bot
                        .send_message(chat_id, "Failed to load highlights (cookies may be expired)")
                        .await;
                    return;
                }
            };

            let caption = format!(
                "{} (@{})\n\nHighlights ({}):",
                profile.full_name,
                username,
                highlights.len()
            );

            let keyboard = build_highlights_keyboard(&highlights, username);
            let _ = bot.send_message(chat_id, &caption).reply_markup(keyboard).await;
        }
        "stories" => {
            // Load stories
            let profile = match source.fetch_profile(username).await {
                Ok(p) => p,
                Err(e) => {
                    log::warn!("Failed to fetch profile for stories: {}", e);
                    let _ = bot.send_message(chat_id, "Failed to load profile for stories").await;
                    return;
                }
            };

            let user_id = match &profile.user_id {
                Some(id) => id.clone(),
                None => {
                    let _ = bot
                        .send_message(chat_id, "Cannot load stories (user_id not available)")
                        .await;
                    return;
                }
            };

            let stories = match source.fetch_reel_media(&user_id).await {
                Ok(s) => s,
                Err(e) => {
                    log::warn!("Failed to fetch stories for @{}: {}", username, e);
                    let _ = bot
                        .send_message(chat_id, "Failed to load stories (cookies may be expired)")
                        .await;
                    return;
                }
            };

            if stories.is_empty() {
                let _ = bot
                    .send_message(chat_id, format!("@{} has no active stories", username))
                    .await;
                return;
            }

            let caption = format!("{} (@{})\n\nStories ({}):", profile.full_name, username, stories.len());
            let keyboard = build_stories_keyboard(&stories, &user_id, username);
            let _ = bot.send_message(chat_id, &caption).reply_markup(keyboard).await;
        }
        _ => {}
    }
}

/// Build keyboard for highlights tray.
fn build_highlights_keyboard(
    highlights: &[crate::download::source::instagram::HighlightReel],
    username: &str,
) -> InlineKeyboardMarkup {
    let mut rows: Vec<Vec<InlineKeyboardButton>> = Vec::new();

    // Tab bar (Highlights is active)
    rows.push(vec![
        InlineKeyboardButton::callback("Posts".to_string(), format!("ig:tab:posts:{}", username)),
        InlineKeyboardButton::callback("[ Highlights ]".to_string(), "ig:noop".to_string()),
        InlineKeyboardButton::callback("Stories".to_string(), format!("ig:tab:stories:{}", username)),
    ]);

    // Highlight reels (2 per row, max 10)
    let mut current_row: Vec<InlineKeyboardButton> = Vec::new();
    for (i, hl) in highlights.iter().take(10).enumerate() {
        let label = format!("{} ({})", hl.title, hl.item_count);
        // Truncate label to fit callback data
        let display_label = if label.len() > 30 {
            format!("{}...", &label[..27])
        } else {
            label
        };
        let callback = format!("ig:hl:{}", hl.id);
        current_row.push(InlineKeyboardButton::callback(display_label, callback));
        if current_row.len() == 2 || i == highlights.len().min(10) - 1 {
            rows.push(std::mem::take(&mut current_row));
        }
    }

    if highlights.is_empty() {
        rows.push(vec![InlineKeyboardButton::callback(
            "No highlights".to_string(),
            "ig:noop".to_string(),
        )]);
    }

    InlineKeyboardMarkup::new(rows)
}

/// Build keyboard for stories list.
fn build_stories_keyboard(
    stories: &[crate::download::source::instagram::StoryItem],
    user_id: &str,
    username: &str,
) -> InlineKeyboardMarkup {
    let mut rows: Vec<Vec<InlineKeyboardButton>> = Vec::new();

    // Tab bar (Stories is active)
    rows.push(vec![
        InlineKeyboardButton::callback("Posts".to_string(), format!("ig:tab:posts:{}", username)),
        InlineKeyboardButton::callback("Highlights".to_string(), format!("ig:tab:hl:{}", username)),
        InlineKeyboardButton::callback("[ Stories ]".to_string(), "ig:noop".to_string()),
    ]);

    // Story items (4 per row)
    let mut current_row: Vec<InlineKeyboardButton> = Vec::new();
    for (i, item) in stories.iter().enumerate() {
        let emoji = if item.is_video { "🎬" } else { "📷" };
        let label = format!("{} {}", emoji, i + 1);
        let callback = format!("ig:storydl:{}:{}", user_id, i);
        current_row.push(InlineKeyboardButton::callback(label, callback));
        if current_row.len() == 4 || i == stories.len() - 1 {
            rows.push(std::mem::take(&mut current_row));
        }
    }

    InlineKeyboardMarkup::new(rows)
}

/// Handle browsing a highlight reel (show items inside).
async fn handle_highlight_browse(bot: &Bot, chat_id: ChatId, _data: &str, highlight_id: &str) {
    let source = InstagramSource::new();

    let reel_id = format!("highlight:{}", highlight_id);
    let items = match source.fetch_reel_media(&reel_id).await {
        Ok(items) => items,
        Err(e) => {
            log::warn!("Failed to fetch highlight {}: {}", highlight_id, e);
            let _ = bot.send_message(chat_id, "Failed to load highlight items").await;
            return;
        }
    };

    if items.is_empty() {
        let _ = bot.send_message(chat_id, "This highlight has no items").await;
        return;
    }

    let caption = format!("Highlight ({} items):", items.len());

    let mut rows: Vec<Vec<InlineKeyboardButton>> = Vec::new();
    let mut current_row: Vec<InlineKeyboardButton> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let emoji = if item.is_video { "🎬" } else { "📷" };
        let label = format!("{} {}", emoji, i + 1);
        let callback = format!("ig:hldl:{}:{}", highlight_id, i);
        current_row.push(InlineKeyboardButton::callback(label, callback));
        if current_row.len() == 4 || i == items.len() - 1 {
            rows.push(std::mem::take(&mut current_row));
        }
    }

    let keyboard = InlineKeyboardMarkup::new(rows);
    let _ = bot.send_message(chat_id, &caption).reply_markup(keyboard).await;
}

/// Download a story or highlight item by fetching the reel and sending the media URL.
async fn handle_story_download(bot: &Bot, chat_id: ChatId, reel_id: &str, index: usize) {
    let source = InstagramSource::new();

    let items = match source.fetch_reel_media(reel_id).await {
        Ok(items) => items,
        Err(e) => {
            log::warn!("Failed to fetch reel {} for download: {}", reel_id, e);
            let _ = bot.send_message(chat_id, "Failed to download item").await;
            return;
        }
    };

    let item = match items.get(index) {
        Some(item) => item,
        None => {
            let _ = bot.send_message(chat_id, "Item not found").await;
            return;
        }
    };

    // Send the media URL as a message — the normal download pipeline picks it up
    // For direct CDN URLs, HttpSource will handle the download
    let _ = bot.send_message(chat_id, &item.media_url).await;
}

/// Format a count with K/M suffixes for display.
fn format_count(count: u32) -> String {
    if count >= 1_000_000 {
        format!("{:.1}M", count as f64 / 1_000_000.0)
    } else if count >= 10_000 {
        format!("{:.1}K", count as f64 / 1_000.0)
    } else {
        count.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_count() {
        assert_eq!(format_count(500), "500");
        assert_eq!(format_count(9999), "9999");
        assert_eq!(format_count(10000), "10.0K");
        assert_eq!(format_count(1500000), "1.5M");
    }
}
