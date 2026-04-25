use crate::core::rate_limiter::RateLimiter;
use crate::download::queue::{DownloadFormat, DownloadQueue, DownloadTask};
use crate::i18n;
use crate::storage::SharedStorage;
use crate::storage::cache;
use crate::storage::db::DbPool;
use crate::telegram::Bot;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{CallbackQueryId, InlineKeyboardMarkup, MessageId, ParseMode};
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
    shared_storage: Arc<SharedStorage>,
    download_queue: Arc<DownloadQueue>,
    rate_limiter: Arc<RateLimiter>,
) -> ResponseResult<()> {
    let url_str = match cache::get_url(&db_pool, Some(shared_storage.as_ref()), url_id).await {
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

    let preview_context = shared_storage
        .get_preview_context(chat_id.0, &url_str)
        .await
        .ok()
        .flatten();
    let original_message_id = preview_context.as_ref().and_then(|context| context.original_message_id);
    let time_range = preview_context.and_then(|context| context.time_range);
    let plan = shared_storage
        .get_user(chat_id.0)
        .await
        .ok()
        .flatten()
        .map(|user| user.plan)
        .unwrap_or_default();

    // Rate limit disabled
    let _ = (rate_limiter, &plan);

    let _ = bot
        .answer_callback_query(callback_id.clone())
        .text("⏳ Processing...")
        .await;

    if let Err(e) = bot.delete_message(chat_id, message_id).await {
        log::warn!("Failed to delete preview message: {:?}", e);
    }
    if let Some(prev_msg_id) = preview_msg_id
        && prev_msg_id != message_id
        && let Err(e) = bot.delete_message(chat_id, prev_msg_id).await
    {
        log::warn!("Failed to delete preview message: {:?}", e);
    }

    enqueue_download_tasks(
        bot,
        &url,
        chat_id,
        format,
        selected_quality,
        original_message_id,
        time_range,
        plan.as_str(),
        db_pool,
        shared_storage,
        download_queue,
    )
    .await;
    Ok(())
}

/// Build `DownloadTask`(s) from format + settings and enqueue them.
///
/// Shared by the preview flow (via `start_download_from_preview`) and the
/// one-tap direct flow (via `commands::mod`). Caller is responsible for all
/// UI cleanup — this function only touches the queue and sends the optional
/// queue-position message.
///
/// For `format == "mp4+mp3"` two tasks are enqueued (MP4 then MP3 fallback).
pub(crate) async fn enqueue_download_tasks(
    bot: &Bot,
    url: &Url,
    chat_id: ChatId,
    format: &str,
    selected_quality: Option<String>,
    original_message_id: Option<i32>,
    time_range: Option<(String, String)>,
    plan: &str,
    db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
    download_queue: Arc<DownloadQueue>,
) {
    let action_start = std::time::Instant::now();

    if format == "mp4+mp3" {
        let video_quality = if let Some(quality) = selected_quality {
            Some(quality)
        } else {
            Some(
                shared_storage
                    .get_user_video_quality(chat_id.0)
                    .await
                    .unwrap_or_else(|_| "best".to_string()),
            )
        };
        let mut task_mp4 = DownloadTask::builder()
            .url(url.as_str().to_string())
            .chat_id(chat_id)
            .maybe_message_id(original_message_id)
            .is_video(true)
            .format(DownloadFormat::Mp4)
            .maybe_video_quality(video_quality)
            .maybe_audio_bitrate(None)
            .priority(crate::download::queue::TaskPriority::from_plan(plan))
            .build();
        task_mp4.time_range = time_range.clone();
        download_queue.add_task(task_mp4, Some(Arc::clone(&db_pool))).await;

        let audio_bitrate = Some(
            shared_storage
                .get_user_audio_bitrate(chat_id.0)
                .await
                .unwrap_or_else(|_| "320k".to_string()),
        );
        let mut task_mp3 = DownloadTask::builder()
            .url(url.as_str().to_string())
            .chat_id(chat_id)
            .maybe_message_id(original_message_id)
            .is_video(false)
            .format(DownloadFormat::Mp3)
            .maybe_video_quality(None)
            .maybe_audio_bitrate(audio_bitrate)
            .priority(crate::download::queue::TaskPriority::from_plan(plan))
            .build();
        task_mp3.time_range = time_range.clone();
        download_queue.add_task(task_mp3, Some(Arc::clone(&db_pool))).await;
    } else {
        let video_quality = if format == "mp4" {
            if let Some(quality) = selected_quality {
                Some(quality)
            } else {
                Some(
                    shared_storage
                        .get_user_video_quality(chat_id.0)
                        .await
                        .unwrap_or_else(|_| "best".to_string()),
                )
            }
        } else {
            None
        };
        let audio_bitrate = if format == "mp3" {
            Some(
                shared_storage
                    .get_user_audio_bitrate(chat_id.0)
                    .await
                    .unwrap_or_else(|_| "320k".to_string()),
            )
        } else {
            None
        };

        let is_video = format == "mp4";
        let dl_format = format.parse::<DownloadFormat>().unwrap_or(DownloadFormat::Mp3);
        let mut task = DownloadTask::builder()
            .url(url.as_str().to_string())
            .chat_id(chat_id)
            .maybe_message_id(original_message_id)
            .is_video(is_video)
            .format(dl_format)
            .maybe_video_quality(video_quality)
            .maybe_audio_bitrate(audio_bitrate)
            .priority(crate::download::queue::TaskPriority::from_plan(plan))
            .build();
        task.time_range = time_range.clone();
        download_queue.add_task(task, Some(Arc::clone(&db_pool))).await;
    }

    // Send queue position notification and store message ID for later deletion
    if let Some(msg_id) =
        send_queue_position_message(bot, chat_id, plan, &download_queue, &db_pool, &shared_storage).await
    {
        download_queue.set_queue_message_id(chat_id, msg_id.0).await;
    }

    log::info!(
        "⏱️ [TASK_QUEUED] done in {:.1}s (chat {}, format {})",
        action_start.elapsed().as_secs_f64(),
        chat_id.0,
        format
    );
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
    shared_storage: &Arc<SharedStorage>,
) -> Option<MessageId> {
    let queue_size = download_queue.size().await;
    let position = download_queue.get_queue_position(chat_id).await;
    let _ = db_pool;
    let lang = i18n::user_lang_from_storage(shared_storage, chat_id.0).await;

    let message = if queue_size > 0 {
        // Show position in queue
        let args =
            doracore::fluent_args!("position" => position.unwrap_or(queue_size) as i64, "total" => queue_size as i64);

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
