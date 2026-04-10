use crate::core::config;
use crate::downsub::DownsubGateway;
use crate::storage::{DbPool, SharedStorage, SubtitleCache};
use crate::telegram::Bot;
use anyhow::Context;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardMarkup, MessageId};

/// Fetches both SRT and TXT subtitle formats, using cache when available.
/// Returns (srt_content, txt_content, segment_count).
/// Used by both the downloads callback and the /downsub command.
pub async fn fetch_subtitles_for_command(
    gateway: &DownsubGateway,
    cache: &SubtitleCache,
    user_id: i64,
    url: &str,
    lang: &str,
) -> anyhow::Result<(String, String, usize)> {
    // Treat empty string same as "no preference" (None) for the gateway
    let lang_opt = if lang.is_empty() { None } else { Some(lang.to_string()) };

    // Fetch SRT
    let srt = if let Some(cached) = cache.get(url, lang, "srt").await {
        cached
    } else {
        let result = gateway
            .fetch_subtitles(
                user_id,
                None,
                url.to_string(),
                Some("srt".to_string()),
                lang_opt.clone(),
            )
            .await
            .map_err(|e| anyhow::anyhow!(e))?;
        cache.save(url, lang, "srt", &result.raw_subtitles).await;
        result.raw_subtitles
    };

    // Fetch TXT
    let txt = if let Some(cached) = cache.get(url, lang, "txt").await {
        cached
    } else {
        let result = gateway
            .fetch_subtitles(user_id, None, url.to_string(), Some("txt".to_string()), lang_opt)
            .await
            .map_err(|e| anyhow::anyhow!(e))?;
        cache.save(url, lang, "txt", &result.raw_subtitles).await;
        result.raw_subtitles
    };

    // Count subtitle segments: each SRT segment has exactly one "-->" timestamp line
    let segment_count = srt.matches("-->").count();

    Ok((srt, txt, segment_count))
}

pub fn request_error_from_text(text: String) -> teloxide::RequestError {
    teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(text)))
}

pub async fn add_audio_tools_buttons_from_history(
    bot: &Bot,
    _db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
    chat_id: ChatId,
    message_id: MessageId,
    telegram_file_id: &str,
    title: String,
    duration: u32,
) -> anyhow::Result<()> {
    use crate::core::config;
    use crate::download::audio_effects::{self, AudioEffectSession};
    use std::path::Path;

    let session_id = uuid::Uuid::new_v4().to_string();
    let session_file_path_raw = audio_effects::get_original_file_path(&session_id, &config::DOWNLOAD_FOLDER);
    let session_file_path = shellexpand::tilde(&session_file_path_raw).into_owned();
    if let Some(parent) = Path::new(&session_file_path).parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| anyhow::anyhow!(e))?;
    }

    crate::telegram::download_file_from_telegram(
        bot,
        telegram_file_id,
        Some(std::path::PathBuf::from(&session_file_path)),
    )
    .await
    .map_err(|e| anyhow::anyhow!(e))?;

    let session = AudioEffectSession::new(
        session_id.clone(),
        chat_id.0,
        session_file_path,
        message_id.0,
        title,
        duration,
    );
    shared_storage
        .create_audio_effect_session(&session)
        .await
        .map_err(|e| anyhow::anyhow!(e))?;

    let keyboard = InlineKeyboardMarkup::new(vec![vec![
        crate::telegram::cb("🎛️ Edit Audio", format!("ae:open:{}", session_id)),
        crate::telegram::cb("✂️ Cut Audio", format!("ac:open:{}", session_id)),
    ]]);

    bot.edit_message_reply_markup(chat_id, message_id)
        .reply_markup(keyboard)
        .await
        .map_err(|e| anyhow::anyhow!(e))?;

    Ok(())
}

pub async fn add_video_cut_button_from_history(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    download_id: i64,
) -> anyhow::Result<()> {
    let keyboard = InlineKeyboardMarkup::new(vec![vec![crate::telegram::cb(
        "✂️ Cut Video",
        format!("downloads:clip:{}", download_id),
    )]]);

    bot.edit_message_reply_markup(chat_id, message_id)
        .reply_markup(keyboard)
        .await
        .map_err(|e| anyhow::anyhow!(e))?;

    Ok(())
}

pub fn is_file_too_big_error(e: &teloxide::RequestError) -> bool {
    e.to_string().to_lowercase().contains("file is too big")
}

pub fn bot_api_source_hint() -> String {
    match std::env::var("BOT_API_URL").ok() {
        Some(url) if !url.contains("api.telegram.org") => format!("Bot API: local ({})", url),
        Some(url) => format!("Bot API: {}", url),
        None => "Bot API: https://api.telegram.org".to_string(),
    }
}

// is_local_bot_api_env is now crate::core::config::bot_api::is_local()

pub fn short_error_text(text: &str, max_chars: usize) -> String {
    let t = text.trim().replace('\n', " ");
    if t.chars().count() <= max_chars {
        return t;
    }
    let truncated: String = t.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{}…", truncated)
}

pub fn forced_document_unavailable_notice(download_error_text: &str) -> Option<String> {
    let lower = download_error_text.to_lowercase();
    if lower.contains("not available on local bot api server") {
        return Some(format!(
            "⚠️ Cannot force-send as document: the local Bot API cannot see this file via /file (not in local cache/dir).\nLeft as video.\n\n{}",
            bot_api_source_hint()
        ));
    }
    if lower.contains("local bot api file availability check failed")
        || lower.contains("local bot api file check failed")
    {
        return Some(format!(
            "⚠️ Cannot force-send as document: error checking file on local Bot API.\nLeft as video.\n\nReason: {}\n{}",
            short_error_text(download_error_text, 180),
            bot_api_source_hint()
        ));
    }
    if lower.contains("file is too big") {
        if config::bot_api::is_local() {
            return Some(format!(
                "⚠️ Cannot force-send as document: local Bot API returned `file is too big` at `getFile`.\nThis usually means the server is NOT running in `--local` mode (and inherits the official Bot API limit of ~20 MB), or a server-side limit is in effect.\nLeft as video.\n\nReason: {}\n{}",
                short_error_text(download_error_text, 180),
                bot_api_source_hint()
            ));
        }
        return Some(format!(
            "⚠️ Cannot force-send as document: to \"make a document\", the bot needs to download the file and re-upload it.\nOn the official Bot API, downloads are limited to ~20 MB; on the local Bot API this only works if the file is accessible via /file.\nLeft as video.\n\nReason: {}\n{}",
            short_error_text(download_error_text, 180),
            bot_api_source_hint()
        ));
    }
    if lower.contains("telegram file download failed") {
        return Some(format!(
            "⚠️ Cannot force-send as document: failed to download file from Bot API file endpoint.\nLeft as video.\n\nReason: {}\n{}",
            short_error_text(download_error_text, 180),
            bot_api_source_hint()
        ));
    }
    None
}

pub async fn send_document_forced(
    bot: &Bot,
    chat_id: ChatId,
    telegram_file_id: &str,
    upload_file_name: &str,
    caption: String,
) -> ResponseResult<teloxide::types::Message> {
    let first_msg = bot
        .send_document(
            chat_id,
            teloxide::types::InputFile::file_id(teloxide::types::FileId(telegram_file_id.to_string())),
        )
        .disable_content_type_detection(true)
        .caption(caption.clone())
        .await?;

    if first_msg.document().is_some() {
        return Ok(first_msg);
    }

    // If Telegram still renders it as media, try to force a re-upload as a document.
    // Important: do NOT delete the first message unless the re-upload succeeds, otherwise user gets nothing.

    let guard = crate::core::utils::TempDirGuard::new("doradura_telegram")
        .await
        .map_err(|e| request_error_from_text(e.to_string()))?;
    let temp_path = guard.path().join(format!("{}_{}", chat_id.0, upload_file_name));

    match crate::telegram::download_file_from_telegram(bot, telegram_file_id, Some(temp_path.clone())).await {
        Ok(_) => {}
        Err(e) => {
            let msg = e.to_string();
            if let Some(notice) = forced_document_unavailable_notice(&msg) {
                log::warn!("Forced document re-upload is not possible: {}", msg);
                bot.send_message(chat_id, notice).await.ok();
                return Ok(first_msg);
            }
            return Err(request_error_from_text(msg));
        }
    }

    let result = bot
        .send_document(chat_id, teloxide::types::InputFile::file(temp_path.clone()))
        .disable_content_type_detection(true)
        .caption(caption)
        .await;

    match result {
        Ok(msg) => {
            bot.delete_message(chat_id, first_msg.id).await.ok();
            Ok(msg)
        }
        Err(e) => {
            if is_file_too_big_error(&e) {
                bot.send_message(
                    chat_id,
                    "⚠️ Could not force-send as document: Telegram rejected the file due to size. Left as video.",
                )
                .await
                .ok();
                return Ok(first_msg);
            }
            Err(e)
        }
    }
}

/// Change video speed using ffmpeg
pub async fn change_video_speed(
    bot: &Bot,
    chat_id: ChatId,
    file_id: &str,
    speed: f32,
    title: &str,
) -> Result<(teloxide::types::Message, i64), Box<dyn std::error::Error + Send + Sync>> {
    use tokio::fs;
    use tokio::process::Command;

    let guard = crate::core::utils::TempDirGuard::new("doradura_speed").await?;

    let input_path = guard
        .path()
        .join(format!("input_{}_{}.mp4", chat_id.0, uuid::Uuid::new_v4()));
    crate::telegram::download_file_from_telegram(bot, file_id, Some(input_path.clone()))
        .await
        .with_context(|| "Failed to download file from Telegram")?;

    let output_path = guard.path().join(format!("output_{}_{}.mp4", chat_id.0, speed));

    // Calculate audio tempo (pitch correction)
    let atempo = speed;

    // Build ffmpeg command
    // For speed > 2.0, we need to chain multiple atempo filters (max is 2.0 per filter)
    let atempo_filter = if speed > 2.0 {
        format!("atempo=2.0,atempo={}", speed / 2.0)
    } else if speed < 0.5 {
        format!("atempo=0.5,atempo={}", speed / 0.5)
    } else {
        format!("atempo={}", atempo)
    };

    let filter_complex = format!("[0:v]setpts={}*PTS[v];[0:a]{}[a]", 1.0 / speed, atempo_filter);

    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-i")
        .arg(&input_path)
        .arg("-filter_complex")
        .arg(&filter_complex)
        .arg("-map")
        .arg("[v]")
        .arg("-map")
        .arg("[a]")
        .arg("-c:v")
        .arg("libx264")
        .arg("-preset")
        .arg("fast")
        .arg("-crf")
        .arg("23")
        .arg("-c:a")
        .arg("aac")
        .arg("-b:a")
        .arg("192k")
        .arg("-y")
        .arg(&output_path);

    let output = cmd.output().await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("ffmpeg failed: {}", stderr).into());
    }

    let file_size = fs::metadata(&output_path).await.map(|m| m.len() as i64).unwrap_or(0);
    let sent = bot
        .send_video(chat_id, teloxide::types::InputFile::file(output_path.clone()))
        .caption(format!("{} (speed {}x)", title, speed))
        .await?;

    // guard drops here, cleaning up the temp dir
    Ok((sent, file_size))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::escape_markdown;

    // ==================== short_error_text tests ====================

    #[test]
    fn test_short_error_text_fits() {
        assert_eq!(short_error_text("Short text", 50), "Short text");
        assert_eq!(short_error_text("  Trimmed  ", 50), "Trimmed");
    }

    #[test]
    fn test_short_error_text_truncated() {
        let text = "This is a very long error message that should be truncated";
        let result = short_error_text(text, 20);
        assert!(result.len() <= 22); // 20 chars + ellipsis (UTF-8)
        assert!(result.ends_with('\u{2026}'));
    }

    #[test]
    fn test_short_error_text_newlines() {
        assert_eq!(short_error_text("Line1\nLine2\nLine3", 50), "Line1 Line2 Line3");
    }

    #[test]
    fn test_short_error_text_empty() {
        assert_eq!(short_error_text("", 10), "");
        assert_eq!(short_error_text("   ", 10), "");
    }

    // ==================== is_file_too_big_error tests ====================

    #[test]
    fn test_is_file_too_big_error_true() {
        // Create a mock error containing "file is too big"
        let err = teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other("Error: file is too big")));
        assert!(is_file_too_big_error(&err));
    }

    #[test]
    fn test_is_file_too_big_error_case_insensitive() {
        let err = teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other("FILE IS TOO BIG")));
        assert!(is_file_too_big_error(&err));
    }

    #[test]
    fn test_is_file_too_big_error_false() {
        let err = teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other("Network error occurred")));
        assert!(!is_file_too_big_error(&err));
    }

    // ==================== escape_markdown tests ====================

    #[test]
    fn test_escape_markdown_underscore() {
        assert_eq!(escape_markdown("hello_world"), "hello\\_world");
    }

    #[test]
    fn test_escape_markdown_asterisk() {
        assert_eq!(escape_markdown("*bold*"), "\\*bold\\*");
    }

    #[test]
    fn test_escape_markdown_brackets() {
        assert_eq!(escape_markdown("[link](url)"), "\\[link\\]\\(url\\)");
    }

    #[test]
    fn test_escape_markdown_all_special() {
        let all_special = "_*[]()~`>#+-=|{}.!";
        let escaped = escape_markdown(all_special);
        assert_eq!(escaped, "\\_\\*\\[\\]\\(\\)\\~\\`\\>\\#\\+\\-\\=\\|\\{\\}\\.\\!");
    }

    #[test]
    fn test_escape_markdown_no_special() {
        assert_eq!(escape_markdown("hello world 123"), "hello world 123");
    }

    #[test]
    fn test_escape_markdown_empty() {
        assert_eq!(escape_markdown(""), "");
    }

    // ==================== forced_document_unavailable_notice tests ====================

    #[test]
    fn test_forced_document_notice_local_api_unavailable() {
        let error = "Not available on local bot api server";
        let notice = forced_document_unavailable_notice(error);
        assert!(notice.is_some());
        assert!(notice.unwrap().contains("local Bot API"));
    }

    #[test]
    fn test_forced_document_notice_file_too_big() {
        let error = "file is too big";
        let notice = forced_document_unavailable_notice(error);
        assert!(notice.is_some());
        assert!(notice.unwrap().contains("Cannot force-send as document"));
    }

    #[test]
    fn test_forced_document_notice_download_failed() {
        let error = "telegram file download failed";
        let notice = forced_document_unavailable_notice(error);
        assert!(notice.is_some());
        assert!(notice.unwrap().contains("Cannot force-send as document"));
    }

    #[test]
    fn test_forced_document_notice_none() {
        let error = "Some random error";
        let notice = forced_document_unavailable_notice(error);
        assert!(notice.is_none());
    }

    // ==================== fetch_subtitles_for_command tests ====================

    /// When both SRT and TXT are already in cache, the gateway is never called.
    /// We verify this by using an "unavailable" gateway (no DOWNSUB_GRPC_ENDPOINT)
    /// and expecting success purely from cache reads.
    #[tokio::test]
    async fn test_fetch_subtitles_cache_hit_bypasses_gateway() {
        use crate::downsub::DownsubGateway;
        use crate::storage::SubtitleCache;

        let dir = tempfile::tempdir().unwrap();
        let cache = SubtitleCache::new(dir.path().to_str().unwrap());

        let srt_data = "1\n00:00:01,000 --> 00:00:02,000\nHello\n\n2\n00:00:02,000 --> 00:00:03,000\nWorld\n\n";
        let txt_data = "Hello\nWorld\n";
        let url = "https://www.youtube.com/watch?v=test123";

        cache.save(url, "", "srt", srt_data).await;
        cache.save(url, "", "txt", txt_data).await;

        // Gateway with no endpoint configured -> would return Unavailable if called
        let gateway = DownsubGateway::from_env();

        let result = fetch_subtitles_for_command(&gateway, &cache, 12345, url, "").await;
        assert!(result.is_ok(), "Expected cache hit, got: {:?}", result.err());

        let (srt, txt, count) = result.unwrap();
        assert_eq!(srt, srt_data);
        assert_eq!(txt, txt_data);
        assert_eq!(count, 2, "Expected 2 '-->' markers in SRT");
    }

    /// When the cache is empty and the gateway is unavailable, an error is returned.
    #[tokio::test]
    async fn test_fetch_subtitles_gateway_unavailable_returns_error() {
        use crate::downsub::DownsubGateway;
        use crate::storage::SubtitleCache;

        let dir = tempfile::tempdir().unwrap();
        let cache = SubtitleCache::new(dir.path().to_str().unwrap());

        let gateway = DownsubGateway::from_env();
        if gateway.is_available() {
            // Gateway is actually configured in this env -- skip to avoid hitting real server
            return;
        }

        let result =
            fetch_subtitles_for_command(&gateway, &cache, 12345, "https://www.youtube.com/watch?v=test456", "").await;
        assert!(result.is_err());
        let err_str = result.unwrap_err().to_string();
        // DownsubError::Unavailable maps to "Downsub is disabled"
        assert!(
            err_str.contains("disabled") || err_str.contains("Unavailable"),
            "Unexpected error: {}",
            err_str
        );
    }

    /// Segment count is derived from the number of "-->" markers in the SRT.
    #[test]
    fn test_segment_count_from_srt() {
        let srt = "1\n00:00:01,000 --> 00:00:02,000\nLine one\n\n\
                   2\n00:00:02,000 --> 00:00:03,000\nLine two\n\n\
                   3\n00:00:03,000 --> 00:00:04,000\nLine three\n\n";
        let count = srt.matches("-->").count();
        assert_eq!(count, 3);
    }

    #[test]
    fn test_segment_count_empty_srt() {
        let srt = "";
        assert_eq!(srt.matches("-->").count(), 0);
    }
}
