use crate::core::escape_markdown;
use crate::download::ytdlp_errors::sanitize_user_error_message;
use crate::downsub::{DownsubError, DownsubGateway};
use crate::i18n;
use crate::storage::db::DbPool;
use crate::storage::SharedStorage;
use crate::telegram::preview::get_preview_metadata;
use crate::telegram::Bot;
use crate::telegram::BotExt;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::InputFile;
use url::Url;

/// Handle /info command to show available formats for a URL
///
/// Parses URL from command text and displays detailed information about available formats,
/// sizes, quality options, and types (mp4, mp3).
///
/// # Arguments
///
/// * `bot` - Telegram bot instance
/// * `msg` - Message containing the /info command and URL
///
/// # Returns
///
/// Returns `ResponseResult<()>` indicating success or failure
///
/// # Behavior
///
/// - Extracts URL from message text (format: /info <URL>)
/// - Fetches metadata using yt-dlp
/// - Displays available video formats with quality and sizes
/// - Shows audio format information
/// - Sends formatted message to user
pub async fn handle_info_command(
    bot: Bot,
    msg: Message,
    db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
) -> ResponseResult<()> {
    log::info!("════════════════════════════════════════════════════════");
    log::info!("📋 /info command called");
    log::info!("Chat ID: {}", msg.chat.id);
    log::info!("════════════════════════════════════════════════════════");

    if let Some(text) = msg.text() {
        log::info!("✅ Message text found: '{}'", text);

        // Extract URL from command text
        let parts: Vec<&str> = text.split_whitespace().collect();
        log::info!("📊 Parts count: {} - Parts: {:?}", parts.len(), parts);

        if parts.len() < 2 {
            log::warn!("⚠️  No URL provided, sending usage instructions");
            let _ = &db_pool;
            let lang = i18n::user_lang_from_storage(&shared_storage, msg.chat.id.0).await;
            match bot
                .send_message(msg.chat.id, i18n::t(&lang, "commands.info_usage"))
                .await
            {
                Ok(_) => log::info!("✅ Usage message sent successfully"),
                Err(e) => log::error!("❌ Failed to send usage message: {:?}", e),
            }
            return Ok(());
        }

        let url_text = parts[1];
        log::info!("🔗 Extracted URL: {}", url_text);

        // Validate URL
        let url = match Url::parse(url_text) {
            Ok(parsed_url) => {
                log::info!("✅ URL parsed successfully: {}", parsed_url);
                parsed_url
            }
            Err(e) => {
                log::error!("❌ Failed to parse URL '{}': {}", url_text, e);
                let lang = i18n::user_lang_from_storage(&shared_storage, msg.chat.id.0).await;
                match bot
                    .send_message(msg.chat.id, i18n::t(&lang, "commands.invalid_url"))
                    .await
                {
                    Ok(_) => log::info!("✅ Error message sent successfully"),
                    Err(e) => log::error!("❌ Failed to send error message: {:?}", e),
                }
                return Ok(());
            }
        };

        // Send "processing" message
        log::info!("📤 Sending 'processing' message...");
        let lang = i18n::user_lang_from_storage(&shared_storage, msg.chat.id.0).await;
        let processing_msg = match bot
            .send_message(msg.chat.id, i18n::t(&lang, "commands.processing"))
            .await
        {
            Ok(msg) => {
                log::info!("✅ Processing message sent, ID: {}", msg.id);
                msg
            }
            Err(e) => {
                log::error!("❌ Failed to send processing message: {:?}", e);
                return Err(e);
            }
        };

        // Get metadata with video formats
        log::info!("🔍 Fetching metadata from yt-dlp...");
        // Experimental features graduated to main workflow
        match get_preview_metadata(&url, Some("mp4"), Some("best")).await {
            Ok(metadata) => {
                log::info!("✅ Metadata fetched successfully");
                log::info!("📝 Title: {}", metadata.display_title());
                log::info!("⏱ Duration: {:?}", metadata.duration);
                log::info!("📦 File size: {:?}", metadata.filesize);
                log::info!(
                    "🎬 Video formats count: {:?}",
                    metadata.video_formats.as_ref().map(|f| f.len())
                );

                // Log detailed format information
                if let Some(ref formats) = metadata.video_formats {
                    log::info!("📋 Available video formats:");
                    for (idx, format) in formats.iter().enumerate() {
                        log::info!(
                            "  [{}] Quality: {}, Resolution: {:?}, Size: {:?} bytes ({:.2} MB)",
                            idx,
                            format.quality,
                            format.resolution,
                            format.size_bytes,
                            format.size_bytes.unwrap_or(0) as f64 / (1024.0 * 1024.0)
                        );
                    }
                } else {
                    log::warn!("⚠️  No video formats available in metadata");
                }

                let mut response = String::new();

                // Title and artist
                response.push_str(&format!("🎵 *{}*\n\n", escape_markdown(&metadata.display_title())));

                // Duration
                if let Some(duration) = metadata.duration {
                    let minutes = duration / 60;
                    let seconds = duration % 60;
                    response.push_str(&format!("⏱ Duration: {}:{:02}\n\n", minutes, seconds));
                }

                // Video formats section
                if let Some(ref formats) = metadata.video_formats {
                    response.push_str("📹 *Video formats \\(MP4\\):*\n");

                    // Filter and sort formats by quality
                    let quality_order = [
                        "4320p", "2160p", "1440p", "1080p", "720p", "480p", "360p", "240p", "144p",
                    ];
                    let available_formats: Vec<_> = quality_order
                        .iter()
                        .filter_map(|&quality| formats.iter().find(|f| f.quality == quality))
                        .collect();

                    log::info!(
                        "📊 Filtered formats for display: {} out of {} total",
                        available_formats.len(),
                        formats.len()
                    );
                    for format in &available_formats {
                        log::info!(
                            "  ✓ Will display: {} - {:?} - {:.2} MB",
                            format.quality,
                            format.resolution,
                            format.size_bytes.unwrap_or(0) as f64 / (1024.0 * 1024.0)
                        );
                    }

                    if available_formats.is_empty() {
                        log::warn!("⚠️  No formats matched quality_order filter");
                        response.push_str("  • No available formats\n");
                    } else {
                        for format in available_formats {
                            let quality = escape_markdown(&format.quality);

                            if let Some(size) = format.size_bytes {
                                let size_str = escape_markdown(&doracore::core::utils::format_bytes(size));
                                response.push_str(&format!("  • {} \\- {}", quality, size_str));

                                if let Some(ref resolution) = format.resolution {
                                    let res = escape_markdown(resolution);
                                    response.push_str(&format!(" \\({}\\)", res));
                                }
                                response.push('\n');
                            } else {
                                response.push_str(&format!("  • {} \\- size unknown", quality));

                                if let Some(ref resolution) = format.resolution {
                                    let res = escape_markdown(resolution);
                                    response.push_str(&format!(" \\({}\\)", res));
                                }
                                response.push('\n');
                            }
                        }
                    }
                    response.push('\n');
                }

                // Audio format section
                response.push_str("🎧 *Audio format \\(MP3\\):*\n");
                if let Some(size) = metadata.filesize {
                    let size_str = escape_markdown(&doracore::core::utils::format_bytes(size));
                    response.push_str(&format!("  • 320 kbps \\- {}\n", size_str));
                } else {
                    response.push_str("  • 320 kbps \\- size unknown\n");
                }
                response.push('\n');

                // Additional info
                response.push_str("💡 *How to download:*\n");
                response.push_str("1\\. Send me a link\n");
                response.push_str("2\\. Choose format and quality from the menu\n");
                response.push_str("3\\. Get your file\\!");

                log::info!("📝 Response formatted, length: {} chars", response.len());
                log::debug!("Response preview: {}", &response[..response.len().min(200)]);

                // Delete processing message and send result
                log::info!("🗑 Deleting processing message...");
                match bot.delete_message(msg.chat.id, processing_msg.id).await {
                    Ok(_) => log::info!("✅ Processing message deleted"),
                    Err(e) => log::warn!("⚠️  Failed to delete processing message: {:?}", e),
                }

                log::info!("📤 Sending formatted response with MarkdownV2...");
                match bot.send_md(msg.chat.id, response).await {
                    Ok(_) => {
                        log::info!("✅ Response sent successfully!");
                        log::info!("════════════════════════════════════════════════════════");
                    }
                    Err(e) => {
                        log::error!("❌ Failed to send response: {:?}", e);
                        log::info!("════════════════════════════════════════════════════════");
                        return Err(e);
                    }
                }
            }
            Err(e) => {
                log::error!("❌ Failed to get metadata: {:?}", e);

                log::info!("🗑 Deleting processing message...");
                match bot.delete_message(msg.chat.id, processing_msg.id).await {
                    Ok(_) => log::info!("✅ Processing message deleted"),
                    Err(e) => log::warn!("⚠️  Failed to delete processing message: {:?}", e),
                }

                let user_error = sanitize_user_error_message(&e.to_string());
                let error_msg = format!("❌ Failed to get file information:\n{}", user_error);
                log::info!("📤 Sending error message...");
                match bot.send_message(msg.chat.id, error_msg).await {
                    Ok(_) => {
                        log::info!("✅ Error message sent successfully");
                        log::info!("════════════════════════════════════════════════════════");
                    }
                    Err(e) => {
                        log::error!("❌ Failed to send error message: {:?}", e);
                        log::info!("════════════════════════════════════════════════════════");
                        return Err(e);
                    }
                }
            }
        }
    } else {
        log::error!("❌ No text in message!");
        log::info!("════════════════════════════════════════════════════════");
    }

    log::info!("✅ handle_info_command completed");
    Ok(())
}

pub async fn handle_downsub_command(
    bot: Bot,
    msg: Message,
    db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
    downsub_gateway: Arc<DownsubGateway>,
    subtitle_cache: Arc<crate::storage::SubtitleCache>,
) -> ResponseResult<()> {
    let _ = db_pool;
    let lang = i18n::user_lang_from_storage(&shared_storage, msg.chat.id.0).await;
    let usage_text = i18n::t(&lang, "commands.downsub_usage");
    let disabled_text = i18n::t(&lang, "commands.downsub_disabled");

    let message_text = match msg.text() {
        Some(text) => text.trim(),
        None => {
            bot.send_message(msg.chat.id, usage_text.clone()).await?;
            return Ok(());
        }
    };

    let tokens: Vec<&str> = message_text.split_whitespace().collect();
    if tokens.len() < 2 {
        bot.send_message(msg.chat.id, usage_text.clone()).await?;
        return Ok(());
    }

    let action = tokens[1].to_lowercase();
    let options = parse_downsub_options(tokens.get(3..).unwrap_or(&[]));

    match action.as_str() {
        "summary" => {
            if tokens.len() < 3 {
                bot.send_message(msg.chat.id, usage_text.clone()).await?;
                return Ok(());
            }

            let url = tokens[2].to_string();
            let loading_msg = bot.send_message(msg.chat.id, "⏳ Generating summary…").await?;

            match downsub_gateway
                .summarize_url(msg.chat.id.0, options.phone.clone(), url, options.language.clone())
                .await
            {
                Ok(summary) => {
                    bot.delete_message(msg.chat.id, loading_msg.id).await.ok();

                    let mut response = String::new();
                    response.push_str(&i18n::t(&lang, "commands.downsub_summary_header"));
                    response.push('\n');
                    response.push_str(&summary.summary);

                    if !summary.highlights.is_empty() {
                        response.push_str("\n\nHighlights:\n");
                        for highlight in summary.highlights {
                            response.push_str("- ");
                            response.push_str(&highlight);
                            response.push('\n');
                        }
                    }

                    if !summary.sections.is_empty() {
                        for section in summary.sections {
                            if let Some(title) = section.title {
                                response.push_str("\n*");
                                response.push_str(&title);
                                response.push_str("*\n");
                            }
                            response.push_str(&section.text);
                            response.push('\n');
                        }
                    }

                    bot.send_message(msg.chat.id, response).await?;
                }
                Err(DownsubError::Unavailable) => {
                    bot.delete_message(msg.chat.id, loading_msg.id).await.ok();
                    bot.send_message(msg.chat.id, disabled_text.clone()).await?;
                }
                Err(err) => {
                    log::warn!("Downsub summary request failed: {}", err);
                    bot.edit_message_text(msg.chat.id, loading_msg.id, format!("❌ Error: {}", err))
                        .await
                        .ok();
                }
            }
        }
        "subtitles" => {
            if tokens.len() < 3 {
                bot.send_message(msg.chat.id, usage_text.clone()).await?;
                return Ok(());
            }

            let url = tokens[2].to_string();
            let loading_msg = bot
                .send_message(msg.chat.id, "⏳ Fetching subtitles (SRT + TXT)…")
                .await?;

            let lang_str = options.language.clone().unwrap_or_default();
            use crate::telegram::downloads::fetch_subtitles_for_command;
            match fetch_subtitles_for_command(&downsub_gateway, &subtitle_cache, msg.chat.id.0, &url, &lang_str).await {
                Ok((srt_content, txt_content, segment_count)) => {
                    bot.edit_message_text(
                        msg.chat.id,
                        loading_msg.id,
                        format!("✅ {} segments fetched", segment_count),
                    )
                    .await
                    .ok();

                    bot.send_document(
                        msg.chat.id,
                        InputFile::memory(srt_content.into_bytes()).file_name("subtitles.srt"),
                    )
                    .await
                    .ok();
                    bot.send_document(
                        msg.chat.id,
                        InputFile::memory(txt_content.into_bytes()).file_name("subtitles.txt"),
                    )
                    .await
                    .ok();
                }
                Err(err) => {
                    log::warn!("Downsub subtitles request failed: {}", err);
                    bot.edit_message_text(msg.chat.id, loading_msg.id, format!("❌ Error: {}", err))
                        .await
                        .ok();
                }
            }
        }
        _ => {
            bot.send_message(msg.chat.id, usage_text.clone()).await?;
        }
    }

    Ok(())
}

#[derive(Clone, Default)]
struct DownsubOptions {
    language: Option<String>,
    format: Option<String>,
    phone: Option<String>,
}

fn parse_downsub_options(tokens: &[&str]) -> DownsubOptions {
    let mut options = DownsubOptions::default();

    for &token in tokens {
        if let Some((key, value)) = token.split_once('=') {
            match key.to_lowercase().as_str() {
                "lang" | "language" => {
                    options.language = Some(value.to_string());
                }
                "format" => {
                    options.format = Some(value.to_string());
                }
                "phone" => {
                    options.phone = Some(value.to_string());
                }
                _ => {}
            }
        }
    }

    options
}

#[cfg(test)]
mod tests {
    use super::*;

    // ==================== parse_downsub_options tests ====================

    #[test]
    fn test_parse_downsub_options_language() {
        let tokens = vec!["lang=en"];
        let options = parse_downsub_options(&tokens);
        assert_eq!(options.language, Some("en".to_string()));
        assert!(options.format.is_none());
        assert!(options.phone.is_none());
    }

    #[test]
    fn test_parse_downsub_options_format() {
        let tokens = vec!["format=srt"];
        let options = parse_downsub_options(&tokens);
        assert_eq!(options.format, Some("srt".to_string()));
    }

    #[test]
    fn test_parse_downsub_options_multiple() {
        let tokens = vec!["lang=ru", "format=vtt", "phone=+1234567890"];
        let options = parse_downsub_options(&tokens);
        assert_eq!(options.language, Some("ru".to_string()));
        assert_eq!(options.format, Some("vtt".to_string()));
        assert_eq!(options.phone, Some("+1234567890".to_string()));
    }

    #[test]
    fn test_parse_downsub_options_case_insensitive() {
        let tokens = vec!["LANG=en", "Language=fr", "FORMAT=txt"];
        let options = parse_downsub_options(&tokens);
        // Last matching key wins
        assert_eq!(options.language, Some("fr".to_string()));
        assert_eq!(options.format, Some("txt".to_string()));
    }

    #[test]
    fn test_parse_downsub_options_empty() {
        let tokens: Vec<&str> = vec![];
        let options = parse_downsub_options(&tokens);
        assert!(options.language.is_none());
        assert!(options.format.is_none());
        assert!(options.phone.is_none());
    }

    #[test]
    fn test_parse_downsub_options_invalid_tokens() {
        let tokens = vec!["invalid", "no_equals_sign"];
        let options = parse_downsub_options(&tokens);
        assert!(options.language.is_none());
        assert!(options.format.is_none());
    }
}
