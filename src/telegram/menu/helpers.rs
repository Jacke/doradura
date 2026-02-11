use crate::core::rate_limiter::RateLimiter;
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
                .text("Ссылка устарела, отправь её снова")
                .await?;
            return Ok(());
        }
    };

    let url = match Url::parse(&url_str) {
        Ok(url) => url,
        Err(e) => {
            log::error!("Failed to parse URL from cache: {}", e);
            bot.answer_callback_query(callback_id.clone())
                .text("Ошибка: неверная ссылка")
                .await?;
            return Ok(());
        }
    };

    let original_message_id = tg_cache::get_link_message_id(&url_str).await;
    let time_range = tg_cache::get_time_range(&url_str).await;
    let conn = db::get_connection(&db_pool)
        .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;
    let plan = match db::get_user(&conn, chat_id.0) {
        Ok(Some(ref user)) => user.plan.clone(),
        _ => "free".to_string(),
    };

    // Rate limit disabled
    let _ = (rate_limiter, &plan);

    let _ = bot
        .answer_callback_query(callback_id.clone())
        .text("⏳ Обрабатываю...")
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
            &plan,
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
            &plan,
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
            &plan,
        );
        task.time_range = time_range.clone();
        download_queue.add_task(task, Some(Arc::clone(&db_pool))).await;
    }

    // Send queue position notification
    send_queue_position_message(bot, chat_id, &plan, &download_queue, &db_pool).await;

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
) {
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

    if let Err(e) = bot.send_message(chat_id, message).parse_mode(ParseMode::Html).await {
        log::warn!("Failed to send queue position message: {:?}", e);
    }
}

// ==================== Pure Helper Functions ====================

#[allow(dead_code)]
/// Formats video quality for display
pub(crate) fn format_video_quality_display(quality: &str) -> &'static str {
    match quality {
        "1080p" => "🎬 1080p",
        "720p" => "🎬 720p",
        "480p" => "🎬 480p",
        "360p" => "🎬 360p",
        _ => "🎬 Best",
    }
}

#[allow(dead_code)]
/// Formats audio bitrate for display
pub(crate) fn format_audio_bitrate_display(bitrate: &str) -> &'static str {
    match bitrate {
        "128k" => "128 kbps",
        "192k" => "192 kbps",
        "256k" => "256 kbps",
        "320k" => "320 kbps",
        _ => "320 kbps",
    }
}

#[allow(dead_code)]
/// Formats download format for display
pub(crate) fn format_download_format_display(format: &str) -> &'static str {
    match format {
        "mp3" => "🎵 MP3",
        "mp4" => "🎬 MP4",
        "mp4+mp3" => "🎬🎵 MP4 + MP3",
        "srt" => "📝 SRT",
        "txt" => "📄 TXT",
        _ => "🎵 MP3",
    }
}

#[allow(dead_code)]
/// Formats subscription plan for display
pub(crate) fn format_plan_display(plan: &str) -> &'static str {
    match plan {
        "vip" => "💎 VIP",
        "premium" => "⭐ Premium",
        _ => "🆓 Free",
    }
}

#[allow(dead_code)]
/// Builds a format callback string with optional preview context
pub(crate) fn build_format_callback(format: &str, url_id: Option<&str>, preview_msg_id: Option<i32>) -> String {
    match (url_id, preview_msg_id) {
        (Some(id), Some(msg_id)) => format!("format:{}:preview:{}:{}", format, id, msg_id),
        (Some(id), None) => format!("format:{}:preview:{}", format, id),
        _ => format!("format:{}", format),
    }
}

#[allow(dead_code)]
/// Builds a back callback string with optional preview context
pub(crate) fn build_back_callback(url_id: Option<&str>, preview_msg_id: Option<i32>) -> String {
    match (url_id, preview_msg_id) {
        (Some(id), Some(msg_id)) => format!("back:preview:{}:{}", id, msg_id),
        (Some(id), None) => format!("back:preview:{}", id),
        _ => "back:main".to_string(),
    }
}

#[allow(dead_code)]
/// Builds a mode callback string with optional preview context
pub(crate) fn build_mode_callback(mode: &str, url_id: Option<&str>) -> String {
    match url_id {
        Some(id) => format!("mode:{}:preview:{}", mode, id),
        None => format!("mode:{}", mode),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
    }

    #[test]
    fn test_escape_markdown_cyrillic() {
        assert_eq!(escape_markdown("Привет мир!"), "Привет мир\\!");
    }

    // ==================== Format Display Functions ====================

    #[test]
    fn test_format_video_quality_display() {
        assert_eq!(format_video_quality_display("1080p"), "🎬 1080p");
        assert_eq!(format_video_quality_display("720p"), "🎬 720p");
        assert_eq!(format_video_quality_display("480p"), "🎬 480p");
        assert_eq!(format_video_quality_display("360p"), "🎬 360p");
        assert_eq!(format_video_quality_display("best"), "🎬 Best");
        assert_eq!(format_video_quality_display("unknown"), "🎬 Best");
    }

    #[test]
    fn test_format_audio_bitrate_display() {
        assert_eq!(format_audio_bitrate_display("128k"), "128 kbps");
        assert_eq!(format_audio_bitrate_display("192k"), "192 kbps");
        assert_eq!(format_audio_bitrate_display("256k"), "256 kbps");
        assert_eq!(format_audio_bitrate_display("320k"), "320 kbps");
        assert_eq!(format_audio_bitrate_display("unknown"), "320 kbps");
    }

    #[test]
    fn test_format_download_format_display() {
        assert_eq!(format_download_format_display("mp3"), "🎵 MP3");
        assert_eq!(format_download_format_display("mp4"), "🎬 MP4");
        assert_eq!(format_download_format_display("mp4+mp3"), "🎬🎵 MP4 + MP3");
        assert_eq!(format_download_format_display("srt"), "📝 SRT");
        assert_eq!(format_download_format_display("txt"), "📄 TXT");
        assert_eq!(format_download_format_display("unknown"), "🎵 MP3");
    }

    #[test]
    fn test_format_plan_display() {
        assert_eq!(format_plan_display("vip"), "💎 VIP");
        assert_eq!(format_plan_display("premium"), "⭐ Premium");
        assert_eq!(format_plan_display("free"), "🆓 Free");
        assert_eq!(format_plan_display("unknown"), "🆓 Free");
    }

    // ==================== Callback Builders ====================

    #[test]
    fn test_build_format_callback_simple() {
        assert_eq!(build_format_callback("mp3", None, None), "format:mp3");
        assert_eq!(build_format_callback("mp4", None, None), "format:mp4");
    }

    #[test]
    fn test_build_format_callback_with_preview() {
        assert_eq!(
            build_format_callback("mp3", Some("url123"), None),
            "format:mp3:preview:url123"
        );
    }

    #[test]
    fn test_build_format_callback_with_preview_and_msg() {
        assert_eq!(
            build_format_callback("mp4", Some("url123"), Some(456)),
            "format:mp4:preview:url123:456"
        );
    }

    #[test]
    fn test_build_back_callback_simple() {
        assert_eq!(build_back_callback(None, None), "back:main");
    }

    #[test]
    fn test_build_back_callback_with_preview() {
        assert_eq!(build_back_callback(Some("url123"), None), "back:preview:url123");
        assert_eq!(
            build_back_callback(Some("url123"), Some(789)),
            "back:preview:url123:789"
        );
    }

    #[test]
    fn test_build_mode_callback_simple() {
        assert_eq!(build_mode_callback("video_quality", None), "mode:video_quality");
        assert_eq!(build_mode_callback("audio_bitrate", None), "mode:audio_bitrate");
    }

    #[test]
    fn test_build_mode_callback_with_preview() {
        assert_eq!(
            build_mode_callback("video_quality", Some("url123")),
            "mode:video_quality:preview:url123"
        );
    }
}
