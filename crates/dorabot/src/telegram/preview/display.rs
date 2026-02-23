use crate::core::escape_markdown;
use crate::storage::{cache, db::DbPool};
use crate::telegram::types::{PreviewMetadata, VideoFormatInfo};
use crate::telegram::Bot;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{InputFile, MessageId};
use unic_langid::LanguageIdentifier;
use url::Url;

use super::formats::filter_video_formats_by_size;
use super::keyboard::{
    create_carousel_keyboard, create_fallback_keyboard, create_video_format_keyboard, keyboard_stats,
};

fn parse_time_to_secs(s: &str) -> Option<u32> {
    let parts: Vec<&str> = s.split(':').collect();
    match parts.len() {
        2 => {
            let m: u32 = parts[0].parse().ok()?;
            let s: u32 = parts[1].parse().ok()?;
            Some(m * 60 + s)
        }
        3 => {
            let h: u32 = parts[0].parse().ok()?;
            let m: u32 = parts[1].parse().ok()?;
            let s: u32 = parts[2].parse().ok()?;
            Some(h * 3600 + m * 60 + s)
        }
        _ => None,
    }
}

fn format_secs_duration(secs: u32) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

fn format_bytes(size: u64) -> String {
    if size > 1024 * 1024 {
        format!("{:.1} MB", size as f64 / (1024.0 * 1024.0))
    } else if size > 1024 {
        format!("{:.1} KB", size as f64 / 1024.0)
    } else {
        format!("{} B", size)
    }
}

/// Sends a preview with metadata and confirmation buttons
///
/// For video, shows a list of formats with selection buttons
/// For other formats — standard buttons
///
/// # Arguments
/// * `bot` - Telegram bot instance
/// * `chat_id` - User's chat ID
/// * `url` - Video/audio URL
/// * `metadata` - Preview metadata with formats
/// * `default_format` - Default format (mp3, mp4, etc.)
/// * `default_quality` - Default video quality (1080p, 720p, etc.) - only for mp4
/// * `old_preview_msg_id` - Optional ID of the old preview message to delete
#[allow(clippy::too_many_arguments)]
pub async fn send_preview(
    bot: &Bot,
    chat_id: ChatId,
    url: &Url,
    metadata: &PreviewMetadata,
    default_format: &str,
    default_quality: Option<&str>,
    old_preview_msg_id: Option<MessageId>,
    db_pool: Arc<DbPool>,
    time_range: Option<&(String, String)>,
) -> ResponseResult<Message> {
    let lang = crate::i18n::user_lang_from_pool(&db_pool, chat_id.0);

    // Override format for photo posts (Instagram photos shouldn't show MP3 button)
    let default_format = if metadata.is_photo { "photo" } else { default_format };

    // Build preview text with escaping
    let escaped_title = escape_markdown(&metadata.display_title());
    let mut text = format!("🎵 *{}*\n\n", escaped_title);

    // Scale factor for time_range: trimmed_secs / full_duration
    let scale_ratio: Option<f64> = if let Some((start, end)) = time_range {
        if let (Some(s), Some(e), Some(d)) = (parse_time_to_secs(start), parse_time_to_secs(end), metadata.duration) {
            if d > 0 && e > s {
                Some(((e - s) as f64 / d as f64).min(1.0))
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    if let Some(full_dur) = metadata.duration {
        let dur_str = if let Some(ratio) = scale_ratio {
            let trimmed = (full_dur as f64 * ratio).round() as u32;
            format_secs_duration(trimmed)
        } else {
            metadata.format_duration()
        };
        text.push_str(&format!("⏱️ Duration: {}\n", escape_markdown(&dur_str)));
    }

    if let Some((start, end)) = time_range {
        let mut args = fluent_templates::fluent_bundle::FluentArgs::new();
        args.set("start", start.clone());
        args.set("end", end.clone());
        let tr_text = crate::i18n::t_args(&lang, "preview.time_range_detected", &args);
        text.push_str(&format!("{}\n", escape_markdown(&tr_text)));
    }

    // When time_range is set, skip size filter — partial downloads are much smaller than full video
    let filtered_formats = metadata.video_formats.as_ref().map(|formats| {
        if time_range.is_some() {
            formats.clone()
        } else {
            filter_video_formats_by_size(formats)
        }
    });

    let has_video_formats = filtered_formats.as_ref().is_some_and(|formats| !formats.is_empty());
    let raw_formats_len = metadata
        .video_formats
        .as_ref()
        .map(|formats| formats.len())
        .unwrap_or(0);
    let filtered_formats_len = filtered_formats.as_ref().map(|formats| formats.len()).unwrap_or(0);
    log::info!(
        "Preview formats for {}: raw={}, filtered={}, has_video_formats={}, format={}",
        url,
        raw_formats_len,
        filtered_formats_len,
        has_video_formats,
        default_format
    );

    // For video, show the list of formats with sizes
    if has_video_formats {
        if let Some(formats) = &filtered_formats {
            let display_formats: Vec<crate::telegram::types::VideoFormatInfo> = if let Some(ratio) = scale_ratio {
                formats
                    .iter()
                    .map(|f| crate::telegram::types::VideoFormatInfo {
                        size_bytes: f.size_bytes.map(|s| (s as f64 * ratio) as u64),
                        ..f.clone()
                    })
                    .collect()
            } else {
                formats.clone()
            };
            append_video_formats_text(&mut text, &display_formats, &lang);
        }
    } else if let Some(filesize) = metadata.filesize {
        let scaled_size = if let Some(ratio) = scale_ratio {
            (filesize as f64 * ratio) as u64
        } else {
            filesize
        };
        text.push_str(&format!(
            "📦 Approximate size: {}\n",
            escape_markdown(&format_bytes(scaled_size))
        ));
    }

    if let Some(desc) = &metadata.description {
        text.push_str(&format!("\n📝 {}\n", escape_markdown(desc)));
    }

    text.push_str("\nChoose a format\\:");

    // Delete the old preview message if specified
    if let Some(old_msg_id) = old_preview_msg_id {
        if let Err(e) = bot.delete_message(chat_id, old_msg_id).await {
            log::warn!("Failed to delete old preview message: {:?}", e);
        }
    }

    // Build inline keyboard
    // Store URL in cache and get a short ID (instead of base64)
    let url_id = cache::store_url(&db_pool, url.as_str()).await;
    log::debug!("Stored URL {} with ID: {}", url.as_str(), url_id);

    let (send_as_document, audio_bitrate) = match crate::storage::db::get_connection(&db_pool) {
        Ok(conn) => {
            let send_as_document = if has_video_formats {
                crate::storage::db::get_user_send_as_document(&conn, chat_id.0).unwrap_or(0)
            } else {
                0
            };
            let audio_bitrate =
                crate::storage::db::get_user_audio_bitrate(&conn, chat_id.0).unwrap_or_else(|_| "320k".to_string());
            (send_as_document, audio_bitrate)
        }
        Err(e) => {
            log::warn!("Failed to get db connection for preview settings: {}", e);
            (0, "320k".to_string())
        }
    };

    // Carousel photo selector: show toggle keyboard instead of standard photo/video buttons
    let keyboard = if metadata.carousel_count > 1 {
        let full_mask = (1u32 << metadata.carousel_count) - 1; // all selected
        log::info!(
            "Creating carousel keyboard ({} items, url_id={})",
            metadata.carousel_count,
            url_id
        );
        create_carousel_keyboard(metadata.carousel_count, full_mask, &url_id)
    } else if has_video_formats {
        if let Some(formats) = &filtered_formats {
            if formats.is_empty() {
                log::warn!(
                    "video_formats is Some but empty, using fallback button for {}",
                    default_format
                );
                // If the formats list is empty, create a standard button
                create_fallback_keyboard(default_format, default_quality, &url_id, Some(audio_bitrate.as_str()))
            } else {
                // Default to mp4+mp3 so that quality buttons queue both video and MP3
                let format_for_keyboard = if default_format == "mp4" || default_format == "mp4+mp3" {
                    "mp4+mp3"
                } else {
                    "mp4"
                };
                log::debug!(
                    "Creating video format keyboard with {} formats for {} (format_for_keyboard={})",
                    formats.len(),
                    default_format,
                    format_for_keyboard
                );
                // For video, create format-selection buttons with a Media/Document toggle
                create_video_format_keyboard(
                    formats,
                    default_quality,
                    &url_id,
                    send_as_document,
                    format_for_keyboard,
                    Some(audio_bitrate.as_str()),
                )
            }
        } else {
            // If video_formats is None — create a standard button
            create_fallback_keyboard(default_format, default_quality, &url_id, Some(audio_bitrate.as_str()))
        }
    } else {
        // For other formats or if video_formats is None — standard buttons
        log::debug!(
            "Creating fallback keyboard for format: {} (video_formats.is_some() = {})",
            default_format,
            metadata.video_formats.is_some()
        );
        create_fallback_keyboard(default_format, default_quality, &url_id, Some(audio_bitrate.as_str()))
    };

    // Instagram links don't need the settings button — remove it
    let keyboard = if url.host_str().is_some_and(|h| h.contains("instagram.com")) {
        let filtered: Vec<Vec<_>> = keyboard
            .inline_keyboard
            .into_iter()
            .filter(|row| {
                !row.iter().any(|btn| {
                    matches!(
                        &btn.kind,
                        teloxide::types::InlineKeyboardButtonKind::CallbackData(d) if d.starts_with("pv:set:")
                    )
                })
            })
            .collect();
        teloxide::types::InlineKeyboardMarkup::new(filtered)
    } else {
        keyboard
    };

    let (keyboard_rows, keyboard_buttons) = keyboard_stats(&keyboard);
    log::info!(
        "Preview keyboard built (rows={}, buttons={}, format={}, quality={:?}, url_id={}, send_as_document={})",
        keyboard_rows,
        keyboard_buttons,
        default_format,
        default_quality,
        url_id,
        send_as_document
    );

    // Send preview with thumbnail if available
    if let Some(thumb_url) = &metadata.thumbnail_url {
        // Try to send a photo with the thumbnail
        match reqwest::get(thumb_url).await {
            Ok(response) => {
                if response.status().is_success() {
                    match response.bytes().await {
                        Ok(bytes) => {
                            // Send the photo with caption
                            let bytes_vec = bytes.to_vec();
                            log::info!(
                                "Sending preview photo ({} bytes) for url_id={}",
                                bytes_vec.len(),
                                url_id
                            );
                            let send_result = bot
                                .send_photo(chat_id, InputFile::memory(bytes_vec))
                                .caption(text)
                                .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                                .reply_markup(keyboard)
                                .await;
                            if let Ok(ref message) = send_result {
                                log::info!("Preview photo sent: message_id={}", message.id);
                            }
                            return send_result;
                        }
                        Err(e) => {
                            log::warn!("Failed to get thumbnail bytes: {}", e);
                            // Do not continue — will send text message below
                        }
                    }
                } else {
                    log::warn!("Thumbnail request failed with status: {}", response.status());
                }
            }
            Err(e) => {
                log::warn!("Failed to download thumbnail: {}", e);
            }
        }
    }

    // If thumbnail is unavailable or an error occurred, send a text message
    log::info!("Sending preview text message for url_id={}", url_id);
    let send_result = bot
        .send_message(chat_id, text)
        .parse_mode(teloxide::types::ParseMode::MarkdownV2)
        .reply_markup(keyboard)
        .await;
    if let Ok(ref message) = send_result {
        log::info!("Preview text sent: message_id={}", message.id);
    }
    send_result
}

/// Updates an existing preview message (edits the text/caption and keyboard)
///
/// Used when returning from the settings menu without recreating the message
pub async fn update_preview_message(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    url: &Url,
    metadata: &PreviewMetadata,
    default_format: &str,
    default_quality: Option<&str>,
    db_pool: Arc<DbPool>,
    time_range: Option<&(String, String)>,
) -> ResponseResult<()> {
    let lang = crate::i18n::user_lang_from_pool(&db_pool, chat_id.0);

    // Override format for photo posts (Instagram photos shouldn't show MP3 button)
    let default_format = if metadata.is_photo { "photo" } else { default_format };

    // Build preview text with escaping (mirrors logic from send_preview)
    let escaped_title = escape_markdown(&metadata.display_title());
    let mut text = format!("🎵 *{}*\n\n", escaped_title);

    // Scale factor for time_range: trimmed_secs / full_duration
    let scale_ratio: Option<f64> = if let Some((start, end)) = time_range {
        if let (Some(s), Some(e), Some(d)) = (parse_time_to_secs(start), parse_time_to_secs(end), metadata.duration) {
            if d > 0 && e > s {
                Some(((e - s) as f64 / d as f64).min(1.0))
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    if let Some(full_dur) = metadata.duration {
        let dur_str = if let Some(ratio) = scale_ratio {
            let trimmed = (full_dur as f64 * ratio).round() as u32;
            format_secs_duration(trimmed)
        } else {
            metadata.format_duration()
        };
        text.push_str(&format!("⏱️ Duration: {}\n", escape_markdown(&dur_str)));
    }

    if let Some((start, end)) = time_range {
        let mut args = fluent_templates::fluent_bundle::FluentArgs::new();
        args.set("start", start.clone());
        args.set("end", end.clone());
        let tr_text = crate::i18n::t_args(&lang, "preview.time_range_detected", &args);
        text.push_str(&format!("{}\n", escape_markdown(&tr_text)));
    }

    // When time_range is set, skip size filter — partial downloads are much smaller than full video
    let filtered_formats = metadata.video_formats.as_ref().map(|formats| {
        if time_range.is_some() {
            formats.clone()
        } else {
            filter_video_formats_by_size(formats)
        }
    });

    let has_video_formats = filtered_formats.as_ref().is_some_and(|formats| !formats.is_empty());
    let raw_formats_len = metadata
        .video_formats
        .as_ref()
        .map(|formats| formats.len())
        .unwrap_or(0);
    let filtered_formats_len = filtered_formats.as_ref().map(|formats| formats.len()).unwrap_or(0);
    log::info!(
        "Update preview formats for {}: raw={}, filtered={}, has_video_formats={}, format={}",
        url,
        raw_formats_len,
        filtered_formats_len,
        has_video_formats,
        default_format
    );

    // For video, show the list of formats with sizes
    if has_video_formats {
        if let Some(formats) = &filtered_formats {
            let display_formats: Vec<crate::telegram::types::VideoFormatInfo> = if let Some(ratio) = scale_ratio {
                formats
                    .iter()
                    .map(|f| crate::telegram::types::VideoFormatInfo {
                        size_bytes: f.size_bytes.map(|s| (s as f64 * ratio) as u64),
                        ..f.clone()
                    })
                    .collect()
            } else {
                formats.clone()
            };
            append_video_formats_text(&mut text, &display_formats, &lang);
        }
    } else if let Some(filesize) = metadata.filesize {
        let scaled_size = if let Some(ratio) = scale_ratio {
            (filesize as f64 * ratio) as u64
        } else {
            filesize
        };
        text.push_str(&format!(
            "📦 Approximate size: {}\n",
            escape_markdown(&format_bytes(scaled_size))
        ));
    }

    if let Some(desc) = &metadata.description {
        text.push_str(&format!("\n📝 {}\n", escape_markdown(desc)));
    }

    text.push_str("\nChoose a format\\:");

    // Build inline keyboard
    // Store URL in cache and get a short ID
    let url_id = cache::store_url(&db_pool, url.as_str()).await;

    let mut resolved_quality = default_quality.map(|q| q.to_string());
    let mut audio_bitrate = "320k".to_string();
    let mut send_as_document = 0;
    match crate::storage::db::get_connection(&db_pool) {
        Ok(conn) => {
            audio_bitrate =
                crate::storage::db::get_user_audio_bitrate(&conn, chat_id.0).unwrap_or_else(|_| "320k".to_string());
            if has_video_formats {
                if resolved_quality.is_none() {
                    resolved_quality = Some(
                        crate::storage::db::get_user_video_quality(&conn, chat_id.0)
                            .unwrap_or_else(|_| "best".to_string()),
                    );
                }
                send_as_document = crate::storage::db::get_user_send_as_document(&conn, chat_id.0).unwrap_or(0);
            }
        }
        Err(e) => {
            log::warn!("Failed to get db connection for preview settings: {}", e);
        }
    }

    let keyboard = if metadata.carousel_count > 1 {
        let full_mask = (1u32 << metadata.carousel_count) - 1;
        create_carousel_keyboard(metadata.carousel_count, full_mask, &url_id)
    } else if has_video_formats {
        let formats = filtered_formats.as_deref().unwrap_or(&[]);
        if formats.is_empty() {
            create_fallback_keyboard(default_format, default_quality, &url_id, Some(audio_bitrate.as_str()))
        } else {
            create_video_format_keyboard(
                formats,
                resolved_quality.as_deref(),
                &url_id,
                send_as_document,
                "mp4+mp3",
                Some(audio_bitrate.as_str()),
            )
        }
    } else {
        create_fallback_keyboard(default_format, default_quality, &url_id, Some(audio_bitrate.as_str()))
    };

    // Instagram links don't need the settings button — remove it
    let keyboard = if url.host_str().is_some_and(|h| h.contains("instagram.com")) {
        let filtered: Vec<Vec<_>> = keyboard
            .inline_keyboard
            .into_iter()
            .filter(|row| {
                !row.iter().any(|btn| {
                    matches!(
                        &btn.kind,
                        teloxide::types::InlineKeyboardButtonKind::CallbackData(d) if d.starts_with("pv:set:")
                    )
                })
            })
            .collect();
        teloxide::types::InlineKeyboardMarkup::new(filtered)
    } else {
        keyboard
    };

    let (keyboard_rows, keyboard_buttons) = keyboard_stats(&keyboard);
    log::info!(
        "Preview update keyboard built (rows={}, buttons={}, format={}, quality={:?}, url_id={}, send_as_document={})",
        keyboard_rows,
        keyboard_buttons,
        default_format,
        resolved_quality.as_deref(),
        url_id,
        send_as_document
    );

    // Try to edit the caption (if this is a photo/video)
    let caption_req = bot
        .edit_message_caption(chat_id, message_id)
        .caption(text.clone())
        .parse_mode(teloxide::types::ParseMode::MarkdownV2)
        .reply_markup(keyboard.clone());

    match caption_req.await {
        Ok(_) => Ok(()),
        Err(e) => {
            log::debug!(
                "Failed to edit preview caption for message_id={}, falling back to text: {:?}",
                message_id,
                e
            );
            // If that failed (e.g. it's a text message), edit the text instead
            bot.edit_message_text(chat_id, message_id, text)
                .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                .reply_markup(keyboard)
                .await?;
            Ok(())
        }
    }
}

/// Helper to build the video formats list text (shared between send_preview and update_preview_message)
fn append_video_formats_text(text: &mut String, formats: &[VideoFormatInfo], lang: &LanguageIdentifier) {
    text.push_str("\n📹 *Available formats:*\n");
    for format_info in formats {
        let size_str = if let Some(size) = format_info.size_bytes {
            if size > 1024 * 1024 {
                format!("{:.1} MB", size as f64 / (1024.0 * 1024.0))
            } else if size > 1024 {
                format!("{:.1} KB", size as f64 / 1024.0)
            } else {
                format!("{} B", size)
            }
        } else {
            crate::i18n::t(lang, "common.unknown")
        };
        let resolution_str = format_info
            .resolution
            .as_ref()
            .map(|r| format!(" ({})", r))
            .unwrap_or_default();
        text.push_str(&format!(
            "• {}: {}{}\n",
            escape_markdown(&format_info.quality),
            escape_markdown(&size_str),
            escape_markdown(&resolution_str)
        ));
    }
}
