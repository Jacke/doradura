use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup, ParseMode};

use super::is_youtube_url;
use super::CallbackCtx;

pub(super) async fn handle(ctx: &CallbackCtx, action: &str, parts: &[&str]) -> ResponseResult<()> {
    match action {
        "setcat" => {
            if parts.len() < 3 {
                return Ok(());
            }
            let download_id = parts[2].parse::<i64>().unwrap_or(0);
            let user_cats = ctx
                .shared_storage
                .get_user_categories(ctx.chat_id.0)
                .await
                .unwrap_or_default();
            let download = ctx
                .shared_storage
                .get_download_history_entry(ctx.chat_id.0, download_id)
                .await
                .ok()
                .flatten();
            let mut rows: Vec<Vec<InlineKeyboardButton>> = user_cats
                .iter()
                .map(|c| {
                    vec![crate::telegram::cb(
                        c.clone(),
                        format!("downloads:savecat:{}:{}", download_id, urlencoding::encode(c)),
                    )]
                })
                .collect();
            rows.push(vec![crate::telegram::cb(
                "➕ New category".to_string(),
                format!("downloads:newcat:{}", download_id),
            )]);
            if download.as_ref().and_then(|d| d.category.as_ref()).is_some() {
                rows.push(vec![crate::telegram::cb(
                    "✖ Remove category".to_string(),
                    format!("downloads:savecat:{}:", download_id),
                )]);
            }
            rows.push(vec![crate::telegram::cb(
                "« Back".to_string(),
                format!("downloads:resend:{}", download_id),
            )]);
            ctx.bot
                .edit_message_reply_markup(ctx.chat_id, ctx.message_id)
                .reply_markup(InlineKeyboardMarkup::new(rows))
                .await
                .ok();
        }
        // Save category assignment
        "savecat" => {
            if parts.len() < 3 {
                return Ok(());
            }
            let download_id = parts[2].parse::<i64>().unwrap_or(0);
            let category = parts
                .get(3)
                .filter(|s| !s.is_empty())
                .map(|s| urlencoding::decode(s).unwrap_or_default().to_string());
            let _ = ctx
                .shared_storage
                .set_download_category(ctx.chat_id.0, download_id, category.as_deref())
                .await;

            // Reload download and rebuild resend keyboard with updated category button
            if let Ok(Some(download)) = ctx
                .shared_storage
                .get_download_history_entry(ctx.chat_id.0, download_id)
                .await
            {
                let mut options: Vec<Vec<InlineKeyboardButton>> = Vec::new();
                if download.format == "mp3" {
                    options.push(vec![
                        crate::telegram::cb(
                            "🎵 As audio".to_string(),
                            format!("downloads:send:audio:{}", download_id),
                        ),
                        crate::telegram::cb(
                            "📎 As document".to_string(),
                            format!("downloads:send:document:{}", download_id),
                        ),
                    ]);
                    options.push(vec![
                        crate::telegram::cb("✂️ Clip".to_string(), format!("downloads:clip:{}", download_id)),
                        crate::telegram::cb("⭕️ Circle".to_string(), format!("downloads:circle:{}", download_id)),
                        crate::telegram::cb(
                            "🔔 Make ringtone".to_string(),
                            format!("ringtone:select:download:{}", download_id),
                        ),
                    ]);
                    options.push(vec![crate::telegram::cb(
                        "⚙️ Change speed".to_string(),
                        format!("downloads:speed:{}", download_id),
                    )]);
                } else {
                    options.push(vec![
                        crate::telegram::cb(
                            "🎬 As video".to_string(),
                            format!("downloads:send:video:{}", download_id),
                        ),
                        crate::telegram::cb(
                            "📎 As document".to_string(),
                            format!("downloads:send:document:{}", download_id),
                        ),
                    ]);
                    options.push(vec![
                        crate::telegram::cb("✂️ Clip".to_string(), format!("downloads:clip:{}", download_id)),
                        crate::telegram::cb("⭕️ Circle".to_string(), format!("downloads:circle:{}", download_id)),
                        crate::telegram::cb(
                            "🔔 Make ringtone".to_string(),
                            format!("ringtone:select:download:{}", download_id),
                        ),
                    ]);
                    options.push(vec![crate::telegram::cb(
                        "⚙️ Change speed".to_string(),
                        format!("downloads:speed:{}", download_id),
                    )]);
                }
                if is_youtube_url(&download.url) {
                    options.push(vec![crate::telegram::cb(
                        "📝 Subtitles".to_string(),
                        format!("downloads:subtitles:{}", download_id),
                    )]);
                    if download.format == "mp4" {
                        options.push(vec![crate::telegram::cb(
                            "🔤 Burn subtitles".to_string(),
                            format!("downloads:burn_subs:{}", download_id),
                        )]);
                    }
                }
                let cat_label = match &download.category {
                    Some(c) => format!("🏷 {}", c),
                    None => "🏷 Add to Category".to_string(),
                };
                options.push(vec![crate::telegram::cb(
                    cat_label,
                    format!("downloads:setcat:{}", download_id),
                )]);
                options.push(vec![crate::telegram::cb(
                    "❌ Cancel".to_string(),
                    "downloads:cancel".to_string(),
                )]);
                ctx.bot
                    .edit_message_reply_markup(ctx.chat_id, ctx.message_id)
                    .reply_markup(InlineKeyboardMarkup::new(options))
                    .await
                    .ok();
            }
        }
        // Start new-category text session
        "newcat" => {
            if parts.len() < 3 {
                return Ok(());
            }
            let download_id = parts[2].parse::<i64>().unwrap_or(0);
            let _ = ctx
                .shared_storage
                .create_new_category_session(ctx.chat_id.0, download_id)
                .await;
            ctx.bot
                .edit_message_text(
                    ctx.chat_id,
                    ctx.message_id,
                    "📝 *New Category*\n\nSend a name for the new category:",
                )
                .parse_mode(ParseMode::MarkdownV2)
                .reply_markup(InlineKeyboardMarkup::new(vec![vec![crate::telegram::cb(
                    "« Cancel".to_string(),
                    format!("downloads:setcat:{}", download_id),
                )]]))
                .await?;
        }
        _ => {}
    }
    Ok(())
}
