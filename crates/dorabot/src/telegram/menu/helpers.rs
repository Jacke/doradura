use crate::core::rate_limiter::RateLimiter;
use crate::core::types::Plan;
use crate::download::queue::{DownloadQueue, DownloadTask};
use crate::i18n;
use crate::storage::cache;
use crate::storage::db::{self, DbPool};
use crate::telegram::cache as tg_cache;
use crate::telegram::Bot;
use fluent_templates::fluent_bundle::FluentArgs;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{CallbackQueryId, InlineKeyboardMarkup, MessageId, ParseMode};
use teloxide::RequestError;
use url::Url;

/// Edit caption if present, else fallback to editing text.
pub(crate) async fn edit_caption_or_text(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    text: String,
    keyboard: Option<InlineKeyboardMarkup>,
) -> ResponseResult<()> {
    let mut caption_req = bot
        .edit_message_caption(chat_id, message_id)
        .caption(text.clone())
        .parse_mode(teloxide::types::ParseMode::MarkdownV2);

    if let Some(ref kb) = keyboard {
        caption_req = caption_req.reply_markup(kb.clone());
    }

    match caption_req.await {
        Ok(_) => Ok(()),
        Err(_) => {
            let mut text_req = bot
                .edit_message_text(chat_id, message_id, text)
                .parse_mode(teloxide::types::ParseMode::MarkdownV2);
            if let Some(kb) = keyboard {
                text_req = text_req.reply_markup(kb);
            }
            text_req.await?;
            Ok(())
        }
    }
}

pub(crate) async fn start_download_from_preview(
    bot: &Bot,
    callback_id: &CallbackQueryId,
    chat_id: ChatId,
    message_id: MessageId,
    preview_msg_id: Option<MessageId>,
    url_id: &str,
    format: &str,
    selected_quality: Option<String>,
    db_pool: Arc<DbPool>,
    download_queue: Arc<DownloadQueue>,
    rate_limiter: Arc<RateLimiter>,
) -> ResponseResult<()> {
    let url_str = match cache::get_url(&db_pool, url_id).await {
        Some(url_str) => url_str,
        None => {
            log::warn!("URL not found in cache for ID: {} (expired or invalid)", url_id);
            bot.answer_callback_query(callback_id.clone())
                .text("Link expired, please send it again")
                .await?;
            return Ok(());
        }
    };

    let url = match Url::parse(&url_str) {
        Ok(url) => url,
        Err(e) => {
            log::error!("Failed to parse URL from cache: {}", e);
            bot.answer_callback_query(callback_id.clone())
                .text("Error: invalid link")
                .await?;
            return Ok(());
        }
    };

    let original_message_id = tg_cache::get_link_message_id(&url_str).await;
    let time_range = tg_cache::get_time_range(&url_str).await;
    let conn = db::get_connection(&db_pool)
        .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;
    let plan = match db::get_user(&conn, chat_id.0) {
        Ok(Some(ref user)) => user.plan,
        _ => Plan::default(),
    };

    // Rate limit disabled
    let _ = (rate_limiter, &plan);

    let _ = bot
        .answer_callback_query(callback_id.clone())
        .text("⏳ Processing...")
        .await;

    if let Err(e) = bot.delete_message(chat_id, message_id).await {
        log::warn!("Failed to delete preview message: {:?}", e);
    }
    if let Some(prev_msg_id) = preview_msg_id {
        if prev_msg_id != message_id {
            if let Err(e) = bot.delete_message(chat_id, prev_msg_id).await {
                log::warn!("Failed to delete preview message: {:?}", e);
            }
        }
    }

    if format == "mp4+mp3" {
        let video_quality = if let Some(quality) = selected_quality {
            Some(quality)
        } else {
            Some(db::get_user_video_quality(&conn, chat_id.0).unwrap_or_else(|_| "best".to_string()))
        };
        let mut task_mp4 = DownloadTask::from_plan(
            url.as_str().to_string(),
            chat_id,
            original_message_id,
            true,
            "mp4".to_string(),
            video_quality,
            None,
            plan.as_str(),
        );
        task_mp4.time_range = time_range.clone();
        download_queue.add_task(task_mp4, Some(Arc::clone(&db_pool))).await;

        let audio_bitrate = Some(db::get_user_audio_bitrate(&conn, chat_id.0).unwrap_or_else(|_| "320k".to_string()));
        let mut task_mp3 = DownloadTask::from_plan(
            url.as_str().to_string(),
            chat_id,
            original_message_id,
            false,
            "mp3".to_string(),
            None,
            audio_bitrate,
            plan.as_str(),
        );
        task_mp3.time_range = time_range.clone();
        download_queue.add_task(task_mp3, Some(Arc::clone(&db_pool))).await;
    } else {
        let video_quality = if format == "mp4" {
            if let Some(quality) = selected_quality {
                Some(quality)
            } else {
                Some(db::get_user_video_quality(&conn, chat_id.0).unwrap_or_else(|_| "best".to_string()))
            }
        } else {
            None
        };
        let audio_bitrate = if format == "mp3" {
            Some(db::get_user_audio_bitrate(&conn, chat_id.0).unwrap_or_else(|_| "320k".to_string()))
        } else {
            None
        };

        let is_video = format == "mp4";
        let mut task = DownloadTask::from_plan(
            url.as_str().to_string(),
            chat_id,
            original_message_id,
            is_video,
            format.to_string(),
            video_quality,
            audio_bitrate,
            plan.as_str(),
        );
        task.time_range = time_range.clone();
        download_queue.add_task(task, Some(Arc::clone(&db_pool))).await;
    }

    // Send queue position notification and store message ID for later deletion
    if let Some(msg_id) = send_queue_position_message(bot, chat_id, plan.as_str(), &download_queue, &db_pool).await {
        download_queue.set_queue_message_id(chat_id, msg_id.0).await;
    }

    Ok(())
}

/// Sends a queue position notification to the user after a task is added.
///
/// Shows the user's position in the queue and suggests upgrading if:
/// - Queue size > 5 and user has free plan
pub(crate) async fn send_queue_position_message(
    bot: &Bot,
    chat_id: ChatId,
    plan: &str,
    download_queue: &Arc<DownloadQueue>,
    db_pool: &Arc<DbPool>,
) -> Option<MessageId> {
    let queue_size = download_queue.size().await;
    let position = download_queue.get_queue_position(chat_id).await;
    let lang = i18n::user_lang_from_pool(db_pool, chat_id.0);

    let message = if queue_size > 0 {
        // Show position in queue
        let mut args = FluentArgs::new();
        args.set("position", position.unwrap_or(queue_size) as i64);
        args.set("total", queue_size as i64);

        let mut msg = i18n::t_args(&lang, "commands.task_added_position", &args);

        // If queue > 5 and user is on free plan, suggest upgrade
        if queue_size > 5 && plan == "free" {
            msg.push_str(&i18n::t(&lang, "commands.queue_upgrade_hint"));
        }

        msg
    } else {
        // Queue is empty (task is being processed immediately)
        i18n::t(&lang, "commands.task_added")
    };

    match bot.send_message(chat_id, message).parse_mode(ParseMode::Html).await {
        Ok(msg) => Some(msg.id),
        Err(e) => {
            log::warn!("Failed to send queue position message: {:?}", e);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::core::escape_markdown;

    // ==================== escape_markdown Tests ====================

    #[test]
    fn test_escape_markdown_basic() {
        assert_eq!(escape_markdown("hello"), "hello");
        assert_eq!(escape_markdown("hello world"), "hello world");
    }

    #[test]
    fn test_escape_markdown_special_chars() {
        assert_eq!(escape_markdown("hello_world"), "hello\\_world");
        assert_eq!(escape_markdown("hello*world"), "hello\\*world");
        assert_eq!(escape_markdown("hello.world"), "hello\\.world");
    }

    #[test]
    fn test_escape_markdown_brackets() {
        assert_eq!(escape_markdown("(test)"), "\\(test\\)");
        assert_eq!(escape_markdown("[test]"), "\\[test\\]");
        assert_eq!(escape_markdown("{test}"), "\\{test\\}");
    }

    #[test]
    fn test_escape_markdown_all_special() {
        let input = r"_*[]()~`>#+-=|{}.!";
        let expected = r"\_\*\[\]\(\)\~\`\>\#\+\-\=\|\{\}\.\!";
        assert_eq!(escape_markdown(input), expected);
    }

    #[test]
    fn test_escape_markdown_empty() {
        assert_eq!(escape_markdown(""), "");
        assert_eq!(escape_markdown("Hello world!"), "Hello world\\!");
    }
}
