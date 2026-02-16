//! Media upload handler for premium/vip users

use teloxide::prelude::*;
use teloxide::types::Message;

use super::types::{HandlerDeps, HandlerError};
use crate::storage::db::{self, create_user, get_user};
use crate::storage::get_connection;
use crate::telegram::Bot;

/// Handler for media uploads (photo/video/document) from premium/vip users
pub(super) fn media_upload_handler(deps: HandlerDeps) -> teloxide::dispatching::UpdateHandler<HandlerError> {
    use crate::core::subscription::PlanLimits;
    use crate::storage::uploads::{find_duplicate_upload, save_upload, NewUpload};
    use teloxide::dispatching::UpdateFilterExt;
    use teloxide::types::ParseMode;

    let deps_filter = deps.clone();

    Update::filter_message()
        .filter(|msg: Message| {
            // Only handle messages with media (photo, video, document, audio)
            msg.photo().is_some() || msg.video().is_some() || msg.document().is_some() || msg.audio().is_some()
        })
        .filter(move |msg: Message| {
            // Skip if user has active cookies upload session (let message_handler process it)
            let user_id = msg.from.as_ref().and_then(|u| i64::try_from(u.id.0).ok()).unwrap_or(0);
            if let Ok(conn) = get_connection(&deps_filter.db_pool) {
                if let Ok(Some(_)) = db::get_active_cookies_upload_session(&conn, user_id) {
                    log::info!(
                        "📤 Filter: skipping media_upload_handler - user {} has active cookies session",
                        user_id
                    );
                    return false; // Don't handle - let it fall through to message_handler
                }
                if let Ok(Some(_)) = db::get_active_ig_cookies_upload_session(&conn, user_id) {
                    log::info!(
                        "📤 Filter: skipping media_upload_handler - user {} has active IG cookies session",
                        user_id
                    );
                    return false; // Don't handle - let it fall through to message_handler
                }
            }
            true // Handle this message
        })
        .endpoint(move |bot: Bot, msg: Message| {
            let deps = deps.clone();
            async move {
                let chat_id = msg.chat.id;

                // Get user and check plan
                let conn = match get_connection(&deps.db_pool) {
                    Ok(c) => c,
                    Err(e) => {
                        log::error!("Failed to get DB connection: {}", e);
                        return Ok(());
                    }
                };

                let user = match get_user(&conn, chat_id.0) {
                    Ok(Some(u)) => u,
                    Ok(None) => {
                        // User doesn't exist, create them
                        let username = msg.from.as_ref().and_then(|u| u.username.clone());
                        if let Err(e) = create_user(&conn, chat_id.0, username) {
                            log::error!("Failed to create user: {}", e);
                            return Ok(());
                        }

                        // Fetch the newly created user
                        match get_user(&conn, chat_id.0) {
                            Ok(Some(u)) => u,
                            _ => {
                                log::error!("Failed to get created user");
                                return Ok(());
                            }
                        }
                    }
                    Err(e) => {
                        log::error!("Failed to get user: {}", e);
                        return Ok(());
                    }
                };

                // Check if user can upload media
                let limits = PlanLimits::for_plan(user.plan);
                if !limits.can_upload_media {
                    // Notify user that they can't upload media
                    bot.send_message(
                        chat_id,
                        "❌ Твой тарифный план не позволяет загружать файлы.\n\nИспользуй /plan, чтобы узнать подробнее о тарифах."
                    )
                    .await?;
                    return Ok(());
                }

                // Extract file info from the message
                #[allow(clippy::type_complexity)]
                let (
                    media_type,
                    file_id,
                    file_unique_id,
                    file_size,
                    duration,
                    width,
                    height,
                    mime_type,
                    filename,
                    thumbnail_file_id,
                ): (
                    &str,
                    String,
                    Option<String>,
                    Option<i64>,
                    Option<i64>,
                    Option<i32>,
                    Option<i32>,
                    Option<String>,
                    Option<String>,
                    Option<String>,
                ) = if let Some(photos) = msg.photo() {
                    // Get the largest photo
                    let photo = photos.iter().max_by_key(|p| p.width * p.height);
                    if let Some(p) = photo {
                        (
                            "photo",
                            p.file.id.0.clone(),
                            Some(p.file.unique_id.0.clone()),
                            Some(p.file.size as i64),
                            None,
                            Some(p.width as i32),
                            Some(p.height as i32),
                            Some("image/jpeg".to_string()),
                            None,
                            None,
                        )
                    } else {
                        return Ok(());
                    }
                } else if let Some(video) = msg.video() {
                    (
                        "video",
                        video.file.id.0.clone(),
                        Some(video.file.unique_id.0.clone()),
                        Some(video.file.size as i64),
                        Some(video.duration.seconds() as i64),
                        Some(video.width as i32),
                        Some(video.height as i32),
                        video.mime_type.as_ref().map(|m| m.to_string()),
                        video.file_name.clone(),
                        video.thumbnail.as_ref().map(|t| t.file.id.0.clone()),
                    )
                } else if let Some(doc) = msg.document() {
                    (
                        "document",
                        doc.file.id.0.clone(),
                        Some(doc.file.unique_id.0.clone()),
                        Some(doc.file.size as i64),
                        None,
                        None,
                        None,
                        doc.mime_type.as_ref().map(|m| m.to_string()),
                        doc.file_name.clone(),
                        doc.thumbnail.as_ref().map(|t| t.file.id.0.clone()),
                    )
                } else if let Some(audio) = msg.audio() {
                    (
                        "audio",
                        audio.file.id.0.clone(),
                        Some(audio.file.unique_id.0.clone()),
                        Some(audio.file.size as i64),
                        Some(audio.duration.seconds() as i64),
                        None,
                        None,
                        audio.mime_type.as_ref().map(|m| m.to_string()),
                        audio.file_name.clone(),
                        audio.thumbnail.as_ref().map(|t| t.file.id.0.clone()),
                    )
                } else {
                    return Ok(());
                };

                // Check file size limit
                if let Some(size) = file_size {
                    let max_size_bytes = (limits.max_file_size_mb as i64) * 1024 * 1024;
                    if size > max_size_bytes {
                        bot.send_message(
                            chat_id,
                            format!(
                                "❌ Файл слишком большой ({} MB). Максимальный размер для твоего плана: {} MB.",
                                size / 1024 / 1024,
                                limits.max_file_size_mb
                            ),
                        )
                        .await?;
                        return Ok(());
                    }
                }

                // Check for duplicates
                if let Some(ref unique_id) = file_unique_id {
                    if let Ok(Some(existing)) = find_duplicate_upload(&conn, chat_id.0, unique_id) {
                        bot.send_message(
                            chat_id,
                            format!(
                                "ℹ️ Этот файл уже загружен: *{}*\n\nИспользуй /videos чтобы найти его.",
                                crate::core::escape_markdown(&existing.title)
                            ),
                        )
                        .parse_mode(ParseMode::MarkdownV2)
                        .await?;
                        return Ok(());
                    }
                }

                // Extract file format from mime type or filename
                let file_format = mime_type
                    .as_ref()
                    .and_then(|m| m.split('/').next_back().map(|s| s.to_string()))
                    .or_else(|| {
                        filename
                            .as_ref()
                            .and_then(|f| f.rsplit('.').next().map(|s| s.to_lowercase()))
                    });

                // Generate title: filename > caption > fallback
                let title = filename
                    .as_ref()
                    .map(|f| {
                        // Remove extension from filename
                        f.rsplit_once('.')
                            .map(|(name, _)| name.to_string())
                            .unwrap_or_else(|| f.clone())
                    })
                    .or_else(|| {
                        // Use message caption as title if no filename
                        msg.caption().map(|c| {
                            let trimmed = c.trim();
                            if trimmed.len() > 100 {
                                trimmed.chars().take(100).collect()
                            } else {
                                trimmed.to_string()
                            }
                        }).filter(|s| !s.is_empty())
                    })
                    .unwrap_or_else(|| {
                        format!(
                            "{} {}",
                            match media_type {
                                "photo" => "Фото",
                                "video" => "Видео",
                                "audio" => "Аудио",
                                _ => "Документ",
                            },
                            chrono::Utc::now().format("%d.%m.%Y %H:%M")
                        )
                    });

                // Save upload to database
                let upload = NewUpload {
                    user_id: chat_id.0,
                    original_filename: filename.as_deref(),
                    title: &title,
                    media_type,
                    file_format: file_format.as_deref(),
                    file_id: &file_id,
                    file_unique_id: file_unique_id.as_deref(),
                    file_size,
                    duration,
                    width,
                    height,
                    mime_type: mime_type.as_deref(),
                    message_id: Some(msg.id.0),
                    chat_id: Some(chat_id.0),
                    thumbnail_file_id: thumbnail_file_id.as_deref(),
                };

                match save_upload(&conn, &upload) {
                    Ok(upload_id) => {
                        log::info!(
                            "Upload saved: id={}, user={}, type={}, title={}",
                            upload_id,
                            chat_id.0,
                            media_type,
                            title
                        );

                        // Format file info for display
                        let size_str = file_size
                            .map(|s| {
                                if s < 1024 * 1024 {
                                    format!("{:.1} KB", s as f64 / 1024.0)
                                } else {
                                    format!("{:.1} MB", s as f64 / 1024.0 / 1024.0)
                                }
                            })
                            .unwrap_or_else(|| "—".to_string());

                        let duration_str = duration.map(|d| {
                            let mins = d / 60;
                            let secs = d % 60;
                            format!("{}:{:02}", mins, secs)
                        });

                        let media_icon = match media_type {
                            "photo" => "📷",
                            "video" => "🎬",
                            "audio" => "🎵",
                            _ => "📄",
                        };

                        let mut info_parts = vec![size_str];
                        if let Some(dur) = duration_str {
                            info_parts.push(dur);
                        }
                        if let Some(w) = width {
                            if let Some(h) = height {
                                info_parts.push(format!("{}x{}", w, h));
                            }
                        }

                        let escaped_title = crate::core::escape_markdown(&title);
                        let escaped_info = crate::core::escape_markdown(&info_parts.join(" · "));

                        let keyboard = build_upload_keyboard(media_type, upload_id);
                        let upload_text = build_upload_text(media_type, media_icon, &escaped_title, &escaped_info);

                        bot.send_message(chat_id, upload_text)
                        .parse_mode(ParseMode::MarkdownV2)
                        .reply_markup(keyboard)
                        .await?;
                    }
                    Err(e) => {
                        log::error!("Failed to save upload: {}", e);
                        bot.send_message(chat_id, "❌ Не удалось сохранить файл. Попробуй ещё раз.")
                            .await?;
                    }
                }

                Ok(())
            }
        })
}

/// Build inline keyboard for upload response based on media type (Level 1).
pub(super) fn build_upload_keyboard(media_type: &str, upload_id: i64) -> teloxide::types::InlineKeyboardMarkup {
    use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};

    let mut rows = Vec::new();

    match media_type {
        "video" => {
            rows.push(vec![
                InlineKeyboardButton::callback("📤 Отправить", format!("videos:submenu:send:{}", upload_id)),
                InlineKeyboardButton::callback("🔄 Конвертировать", format!("videos:submenu:convert:{}", upload_id)),
            ]);
        }
        "photo" | "audio" => {
            rows.push(vec![InlineKeyboardButton::callback(
                "📤 Отправить",
                format!("videos:submenu:send:{}", upload_id),
            )]);
        }
        _ => {
            // Document: send directly
            rows.push(vec![InlineKeyboardButton::callback(
                "📤 Отправить",
                format!("videos:send:document:{}", upload_id),
            )]);
        }
    }

    rows.push(vec![
        InlineKeyboardButton::callback("🗑️ Удалить", format!("videos:delete:{}", upload_id)),
        InlineKeyboardButton::callback("📂 Все загрузки", "videos:page:0:all:".to_string()),
    ]);

    InlineKeyboardMarkup::new(rows)
}

/// Build upload response text based on media type.
pub(super) fn build_upload_text(media_type: &str, media_icon: &str, escaped_title: &str, escaped_info: &str) -> String {
    let _ = media_type; // all types use same format now
    format!("{} *Файл загружен:* {}\n└ {}", media_icon, escaped_title, escaped_info)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: extract all callback_data strings from a keyboard
    fn callback_data(keyboard: &teloxide::types::InlineKeyboardMarkup) -> Vec<Vec<String>> {
        keyboard
            .inline_keyboard
            .iter()
            .map(|row| {
                row.iter()
                    .filter_map(|btn| match &btn.kind {
                        teloxide::types::InlineKeyboardButtonKind::CallbackData(data) => Some(data.clone()),
                        _ => None,
                    })
                    .collect()
            })
            .collect()
    }

    /// Helper: extract all button labels from a keyboard
    fn button_labels(keyboard: &teloxide::types::InlineKeyboardMarkup) -> Vec<Vec<String>> {
        keyboard
            .inline_keyboard
            .iter()
            .map(|row| row.iter().map(|btn| btn.text.clone()).collect())
            .collect()
    }

    #[test]
    fn test_video_keyboard_level1_categories() {
        let kb = build_upload_keyboard("video", 42);
        let data = callback_data(&kb);

        assert_eq!(data.len(), 2, "video keyboard should have 2 rows (Level 1)");

        // Row 1: Send + Convert category buttons
        assert_eq!(data[0], vec!["videos:submenu:send:42", "videos:submenu:convert:42"]);
        // Row 2: Delete + All uploads
        assert_eq!(data[1], vec!["videos:delete:42", "videos:page:0:all:"]);
    }

    #[test]
    fn test_video_keyboard_labels() {
        let kb = build_upload_keyboard("video", 1);
        let labels = button_labels(&kb);

        assert_eq!(labels[0], vec!["📤 Отправить", "🔄 Конвертировать"]);
        assert_eq!(labels[1], vec!["🗑️ Удалить", "📂 Все загрузки"]);
    }

    #[test]
    fn test_photo_keyboard_level1_send_only() {
        let kb = build_upload_keyboard("photo", 99);
        let data = callback_data(&kb);

        assert_eq!(data.len(), 2, "photo keyboard should have 2 rows");
        // Row 1: Send submenu only (no convert)
        assert_eq!(data[0], vec!["videos:submenu:send:99"]);
        // Row 2: Delete + All uploads
        assert_eq!(data[1], vec!["videos:delete:99", "videos:page:0:all:"]);
    }

    #[test]
    fn test_audio_keyboard_level1_send_only() {
        let kb = build_upload_keyboard("audio", 99);
        let data = callback_data(&kb);

        assert_eq!(data.len(), 2, "audio keyboard should have 2 rows");
        assert_eq!(data[0], vec!["videos:submenu:send:99"]);
        assert_eq!(data[1], vec!["videos:delete:99", "videos:page:0:all:"]);
    }

    #[test]
    fn test_document_keyboard_sends_directly() {
        let kb = build_upload_keyboard("document", 99);
        let data = callback_data(&kb);

        assert_eq!(data.len(), 2, "document keyboard should have 2 rows");
        // Document: direct send (no submenu)
        assert_eq!(data[0], vec!["videos:send:document:99"]);
        assert_eq!(data[1], vec!["videos:delete:99", "videos:page:0:all:"]);
    }

    #[test]
    fn test_non_video_keyboard_no_conversion_buttons() {
        for media_type in &["photo", "audio", "document"] {
            let kb = build_upload_keyboard(media_type, 5);
            let all_data: Vec<String> = callback_data(&kb).into_iter().flatten().collect();

            assert!(
                !all_data.iter().any(|d| d.contains("convert:")),
                "{} keyboard must not have convert buttons",
                media_type
            );
        }
    }

    #[test]
    fn test_upload_text_no_videos_hint() {
        // All media types now use same format (no /videos hint)
        for media_type in &["video", "photo", "audio", "document"] {
            let text = build_upload_text(media_type, "📷", "test\\.jpg", "2\\.0 MB");
            assert!(
                !text.contains("/videos"),
                "{} upload text should not contain /videos hint",
                media_type
            );
            assert!(text.contains("Файл загружен"));
        }
    }

    #[test]
    fn test_upload_id_embedded_in_callbacks() {
        let kb = build_upload_keyboard("video", 12345);
        let all_data: Vec<String> = callback_data(&kb).into_iter().flatten().collect();

        for data in &all_data {
            assert!(
                data.contains("12345") || data.starts_with("videos:page:"),
                "callback '{}' should contain the upload_id",
                data
            );
        }
    }
}
