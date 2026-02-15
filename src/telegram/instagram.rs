//! Instagram profile browsing UI and callback handlers.
//!
//! When a user sends `instagram.com/<username>`, shows a profile card with
//! an inline keyboard grid of recent posts. Each button downloads that post.

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

    // Build caption
    let bio_preview = if profile.biography.len() > 200 {
        format!("{}...", &profile.biography[..197])
    } else {
        profile.biography.clone()
    };

    let mut stats_args = FluentArgs::new();
    stats_args.set("posts", format_count(profile.post_count));
    stats_args.set("followers", format_count(profile.follower_count));
    let stats_line = i18n::t_args(lang, "instagram-profile-posts", &stats_args);

    let caption = format!(
        "{name} (@{username})\n\
         {bio}\n\n\
         {stats}",
        name = profile.full_name,
        username = profile.username,
        bio = bio_preview,
        stats = stats_line,
    );

    // Build grid keyboard (4 columns x 3 rows = 12 buttons)
    let mut rows: Vec<Vec<InlineKeyboardButton>> = Vec::new();
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

    // Add pagination button if there are more posts
    if profile.end_cursor.is_some() && profile.posts.len() >= 12 {
        rows.push(vec![InlineKeyboardButton::callback(
            i18n::t(lang, "instagram-more"),
            format!("ig:page:{}:{}", username, profile.end_cursor.as_deref().unwrap_or("")),
        )]);
    }

    let keyboard = InlineKeyboardMarkup::new(rows);

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

/// Handle Instagram callback queries (ig:dl:<shortcode>, ig:page:<username>:<cursor>).
pub async fn handle_instagram_callback(
    bot: &Bot,
    callback_id: &teloxide::types::CallbackQueryId,
    chat_id: ChatId,
    data: &str,
) -> Result<(), teloxide::RequestError> {
    let _ = bot.answer_callback_query(callback_id.clone()).await;

    let parts: Vec<&str> = data.splitn(4, ':').collect();
    if parts.len() < 3 {
        return Ok(());
    }

    match parts[1] {
        "dl" => {
            let shortcode = parts[2];
            let url = format!("https://www.instagram.com/p/{}/", shortcode);
            // Send the URL as a message — the message handler will pick it up
            // and route it through the normal download pipeline
            let _ = bot.send_message(chat_id, &url).await;
        }
        "page" => {
            if parts.len() >= 4 {
                let username = parts[2];
                // Default to Russian for callback context (no DB access here)
                let lang = i18n::lang_from_code("ru");
                show_instagram_profile(bot, chat_id, username, &lang).await;
            }
        }
        _ => {}
    }

    Ok(())
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
