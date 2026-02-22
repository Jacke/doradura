use crate::i18n;
use crate::storage::db::{self, DbPool};
use crate::telegram::Bot;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::CallbackQueryId;
use teloxide::types::{InlineKeyboardMarkup, InputFile, InputMedia, InputMediaPhoto, MessageId, ParseMode};

#[derive(Debug, Clone, Copy)]
pub enum Platform {
    Iphone,
    Android,
}

impl Platform {
    pub fn as_str(&self) -> &'static str {
        match self {
            Platform::Iphone => "iphone",
            Platform::Android => "android",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "iphone" => Some(Platform::Iphone),
            "android" => Some(Platform::Android),
            _ => None,
        }
    }

    pub fn output_kind(&self) -> &'static str {
        match self {
            Platform::Iphone => "iphone_ringtone",
            Platform::Android => "android_ringtone",
        }
    }

    pub fn max_duration_secs(&self) -> u32 {
        match self {
            Platform::Iphone => crate::download::ringtone::MAX_IPHONE_DURATION_SECS,
            Platform::Android => crate::download::ringtone::MAX_ANDROID_DURATION_SECS,
        }
    }

    pub fn prompt_key(&self) -> &'static str {
        match self {
            Platform::Iphone => "ringtone-prompt-iphone",
            Platform::Android => "ringtone-prompt-android",
        }
    }

    pub fn instructions_key(&self) -> &'static str {
        match self {
            Platform::Iphone => "ringtone-instructions-iphone",
            Platform::Android => "ringtone-instructions-android",
        }
    }

    pub fn asset_key_prefix(&self) -> &'static str {
        match self {
            Platform::Iphone => "ringtone_instruction_iphone_",
            Platform::Android => "ringtone_instruction_android_",
        }
    }

    pub fn asset_dir(&self) -> &'static str {
        match self {
            Platform::Iphone => "assets/ringtone_instructions/iphone",
            Platform::Android => "assets/ringtone_instructions/android",
        }
    }
}

/// Main callback dispatcher for `ringtone:*` callbacks.
///
/// Callback formats:
/// - `ringtone:select:{source_kind}:{source_id}` — show platform selector
/// - `ringtone:platform:{platform}:{source_kind}:{source_id}` — create session and prompt
pub async fn handle_ringtone_callback(
    bot: &Bot,
    callback_id: CallbackQueryId,
    chat_id: ChatId,
    message_id: MessageId,
    data: &str,
    db_pool: Arc<DbPool>,
) -> Result<(), teloxide::RequestError> {
    let _ = bot.answer_callback_query(callback_id).await;

    let parts: Vec<&str> = data.splitn(5, ':').collect();
    // parts[0] == "ringtone"
    let action = parts.get(1).copied().unwrap_or("");

    match action {
        "select" => {
            // ringtone:select:{source_kind}:{source_id}
            let source_kind = parts.get(2).copied().unwrap_or("download");
            let source_id = parts.get(3).and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);

            // Delete the button message (options menu)
            bot.delete_message(chat_id, message_id).await.ok();

            send_platform_selector(bot, chat_id, source_kind, source_id, &db_pool).await?;
        }
        "platform" => {
            // ringtone:platform:{platform}:{source_kind}:{source_id}
            let platform_str = parts.get(2).copied().unwrap_or("iphone");
            let source_kind = parts.get(3).copied().unwrap_or("download");
            let source_id = parts.get(4).and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);

            let platform = Platform::parse(platform_str).unwrap_or(Platform::Iphone);

            // Delete the platform selector message
            bot.delete_message(chat_id, message_id).await.ok();

            start_ringtone_session(bot, chat_id, platform, source_kind, source_id, &db_pool).await?;
        }
        _ => {}
    }

    Ok(())
}

/// Send the platform selector message with [🍎 iPhone] [🤖 Android] buttons.
pub async fn send_platform_selector(
    bot: &Bot,
    chat_id: ChatId,
    source_kind: &str,
    source_id: i64,
    db_pool: &Arc<DbPool>,
) -> Result<(), teloxide::RequestError> {
    let iphone_cb = format!("ringtone:platform:iphone:{}:{}", source_kind, source_id);
    let android_cb = format!("ringtone:platform:android:{}:{}", source_kind, source_id);

    let keyboard = InlineKeyboardMarkup::new(vec![
        vec![
            crate::telegram::cb("🍎 iPhone", iphone_cb),
            crate::telegram::cb("🤖 Android", android_cb),
        ],
        vec![crate::telegram::cb("❌ Cancel", "downloads:clip_cancel")],
    ]);

    let lang = i18n::user_lang_from_pool(db_pool, chat_id.0);
    let text = i18n::t(&lang, "ringtone-platform-select");
    bot.send_message(chat_id, text).reply_markup(keyboard).await?;

    Ok(())
}

/// Create a VideoClipSession for the chosen platform and send the time range prompt.
pub async fn start_ringtone_session(
    bot: &Bot,
    chat_id: ChatId,
    platform: Platform,
    source_kind: &str,
    source_id: i64,
    db_pool: &Arc<DbPool>,
) -> Result<(), teloxide::RequestError> {
    let conn = db::get_connection(db_pool)
        .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

    // Try to send audio/video preview so the user can identify the track
    let file_id = match source_kind {
        "download" => db::get_download_history_entry(&conn, chat_id.0, source_id)
            .ok()
            .flatten()
            .and_then(|d| d.file_id),
        "cut" => db::get_cut_entry(&conn, chat_id.0, source_id)
            .ok()
            .flatten()
            .and_then(|c| c.file_id),
        _ => None,
    };

    if let Some(fid) = file_id {
        // Send as audio if possible; ignore errors
        bot.send_audio(chat_id, InputFile::file_id(teloxide::types::FileId(fid)))
            .await
            .ok();
    }

    // Create session
    let session = db::VideoClipSession {
        id: uuid::Uuid::new_v4().to_string(),
        user_id: chat_id.0,
        source_download_id: source_id,
        source_kind: source_kind.to_string(),
        source_id,
        original_url: String::new(),
        output_kind: platform.output_kind().to_string(),
        created_at: chrono::Utc::now(),
        expires_at: chrono::Utc::now() + chrono::Duration::minutes(10),
    };

    db::upsert_video_clip_session(&conn, &session)
        .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

    // Send prompt
    let lang = i18n::user_lang_from_pool(db_pool, chat_id.0);
    let prompt_text = i18n::t(&lang, platform.prompt_key());

    let keyboard = InlineKeyboardMarkup::new(vec![vec![crate::telegram::cb("❌ Cancel", "downloads:clip_cancel")]]);

    bot.send_message(chat_id, prompt_text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(keyboard)
        .await?;

    Ok(())
}

/// Send platform-specific ringtone instructions with images (if available).
///
/// Algorithm:
/// 1. Check DB for all cached file_ids for this platform.
/// 2. If all cached → send media group with cached file_ids.
/// 3. Otherwise → send from local `assets/ringtone_instructions/{platform}/` files,
///    then cache the returned file_ids.
/// 4. If no images at all → send text-only message.
pub async fn send_ringtone_instructions(
    bot: &Bot,
    chat_id: ChatId,
    platform: Platform,
    db_pool: &Arc<DbPool>,
) -> Result<(), teloxide::RequestError> {
    let lang = i18n::user_lang_from_pool(db_pool, chat_id.0);
    let instruction_text = i18n::t(&lang, platform.instructions_key());

    // Collect local image paths
    let asset_dir = std::path::Path::new(platform.asset_dir());
    let mut local_images: Vec<std::path::PathBuf> = Vec::new();

    if asset_dir.is_dir() {
        if let Ok(mut entries) = tokio::fs::read_dir(asset_dir).await {
            let mut paths = Vec::new();
            while let Ok(Some(entry)) = entries.next_entry().await {
                let p = entry.path();
                if let Some(ext) = p.extension() {
                    let ext_low = ext.to_string_lossy().to_lowercase();
                    if ext_low == "jpg" || ext_low == "jpeg" || ext_low == "png" {
                        paths.push(p);
                    }
                }
            }
            paths.sort();
            local_images = paths;
        }
    }

    // Check if we have cached file_ids in DB for ALL steps
    let conn = db::get_connection(db_pool)
        .map_err(|e| teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

    let key_prefix = platform.asset_key_prefix();
    let total = local_images.len();

    let cached_ids: Vec<Option<String>> = (1..=total)
        .map(|i| {
            let key = format!("{}{}", key_prefix, i);
            db::get_bot_asset(&conn, &key).ok().flatten()
        })
        .collect();

    let all_cached = total > 0 && cached_ids.iter().all(|id| id.is_some());

    if total == 0 {
        // No images available — fall back to text-only
        bot.send_message(chat_id, instruction_text)
            .parse_mode(ParseMode::MarkdownV2)
            .await?;
        return Ok(());
    }

    if all_cached {
        // All file_ids cached — send media group using file_ids
        let ids: Vec<String> = cached_ids.into_iter().flatten().collect();
        let media: Vec<InputMedia> = ids
            .iter()
            .enumerate()
            .map(|(i, fid)| {
                let mut photo = InputMediaPhoto::new(InputFile::file_id(teloxide::types::FileId(fid.clone())));
                if i == ids.len() - 1 {
                    photo = photo.caption(&instruction_text).parse_mode(ParseMode::MarkdownV2);
                }
                InputMedia::Photo(photo)
            })
            .collect();

        bot.send_media_group(chat_id, media).await.ok();
    } else {
        // Send from local files, then cache returned file_ids
        let media: Vec<InputMedia> = local_images
            .iter()
            .enumerate()
            .map(|(i, path)| {
                let mut photo = InputMediaPhoto::new(InputFile::file(path.clone()));
                if i == local_images.len() - 1 {
                    photo = photo.caption(&instruction_text).parse_mode(ParseMode::MarkdownV2);
                }
                InputMedia::Photo(photo)
            })
            .collect();

        match bot.send_media_group(chat_id, media).await {
            Ok(messages) => {
                // Extract file_ids from returned messages and cache them
                for (i, msg) in messages.iter().enumerate() {
                    let step = i + 1;
                    let key = format!("{}{}", key_prefix, step);
                    // The largest photo in each message
                    if let Some(photos) = msg.photo() {
                        if let Some(largest) = photos.iter().max_by_key(|p| p.width * p.height) {
                            if let Err(e) = db::set_bot_asset(&conn, &key, &largest.file.id.0) {
                                log::warn!("Failed to cache ringtone instruction file_id: {}", e);
                            }
                        }
                    }
                }
            }
            Err(e) => {
                log::warn!(
                    "Failed to send ringtone instruction images: {}, falling back to text",
                    e
                );
                // Fall back to text-only
                bot.send_message(chat_id, instruction_text)
                    .parse_mode(ParseMode::MarkdownV2)
                    .await?;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ==================== Platform::as_str ====================

    #[test]
    fn platform_iphone_as_str() {
        assert_eq!(Platform::Iphone.as_str(), "iphone");
    }

    #[test]
    fn platform_android_as_str() {
        assert_eq!(Platform::Android.as_str(), "android");
    }

    // ==================== Platform::from_str ====================

    #[test]
    fn platform_from_str_iphone() {
        assert!(matches!(Platform::parse("iphone"), Some(Platform::Iphone)));
    }

    #[test]
    fn platform_from_str_android() {
        assert!(matches!(Platform::parse("android"), Some(Platform::Android)));
    }

    #[test]
    fn platform_from_str_invalid_returns_none() {
        assert!(Platform::parse("windows").is_none());
    }

    #[test]
    fn platform_from_str_empty_returns_none() {
        assert!(Platform::parse("").is_none());
    }

    #[test]
    fn platform_from_str_uppercase_returns_none() {
        assert!(Platform::parse("iPhone").is_none());
        assert!(Platform::parse("Android").is_none());
    }

    // ==================== Platform::output_kind ====================

    #[test]
    fn platform_iphone_output_kind() {
        assert_eq!(Platform::Iphone.output_kind(), "iphone_ringtone");
    }

    #[test]
    fn platform_android_output_kind() {
        assert_eq!(Platform::Android.output_kind(), "android_ringtone");
    }

    #[test]
    fn output_kinds_are_distinct() {
        assert_ne!(Platform::Iphone.output_kind(), Platform::Android.output_kind());
    }

    // ==================== Platform::max_duration_secs ====================

    #[test]
    fn platform_iphone_max_duration_is_30() {
        assert_eq!(Platform::Iphone.max_duration_secs(), 30);
    }

    #[test]
    fn platform_android_max_duration_is_40() {
        assert_eq!(Platform::Android.max_duration_secs(), 40);
    }

    #[test]
    fn android_duration_greater_than_iphone() {
        assert!(Platform::Android.max_duration_secs() > Platform::Iphone.max_duration_secs());
    }

    // ==================== Platform::prompt_key ====================

    #[test]
    fn platform_iphone_prompt_key() {
        assert_eq!(Platform::Iphone.prompt_key(), "ringtone-prompt-iphone");
    }

    #[test]
    fn platform_android_prompt_key() {
        assert_eq!(Platform::Android.prompt_key(), "ringtone-prompt-android");
    }

    // ==================== Platform::instructions_key ====================

    #[test]
    fn platform_iphone_instructions_key() {
        assert_eq!(Platform::Iphone.instructions_key(), "ringtone-instructions-iphone");
    }

    #[test]
    fn platform_android_instructions_key() {
        assert_eq!(Platform::Android.instructions_key(), "ringtone-instructions-android");
    }

    // ==================== Platform::asset_key_prefix ====================

    #[test]
    fn platform_iphone_asset_key_prefix() {
        assert_eq!(Platform::Iphone.asset_key_prefix(), "ringtone_instruction_iphone_");
    }

    #[test]
    fn platform_android_asset_key_prefix() {
        assert_eq!(Platform::Android.asset_key_prefix(), "ringtone_instruction_android_");
    }

    #[test]
    fn asset_key_step_3_iphone() {
        let key = format!("{}3", Platform::Iphone.asset_key_prefix());
        assert_eq!(key, "ringtone_instruction_iphone_3");
    }

    #[test]
    fn asset_key_step_6_android() {
        let key = format!("{}6", Platform::Android.asset_key_prefix());
        assert_eq!(key, "ringtone_instruction_android_6");
    }

    // ==================== Platform::asset_dir ====================

    #[test]
    fn platform_iphone_asset_dir() {
        assert_eq!(Platform::Iphone.asset_dir(), "assets/ringtone_instructions/iphone");
    }

    #[test]
    fn platform_android_asset_dir() {
        assert_eq!(Platform::Android.asset_dir(), "assets/ringtone_instructions/android");
    }

    // ==================== Callback data parsing (splitn logic) ====================

    #[test]
    fn parse_select_download_callback() {
        let data = "ringtone:select:download:123";
        let parts: Vec<&str> = data.splitn(5, ':').collect();
        assert_eq!(parts[0], "ringtone");
        assert_eq!(parts[1], "select");
        assert_eq!(parts[2], "download");
        assert_eq!(parts[3].parse::<i64>().unwrap(), 123);
    }

    #[test]
    fn parse_select_cut_callback() {
        let data = "ringtone:select:cut:456";
        let parts: Vec<&str> = data.splitn(5, ':').collect();
        assert_eq!(parts[1], "select");
        assert_eq!(parts[2], "cut");
        assert_eq!(parts[3].parse::<i64>().unwrap(), 456);
    }

    #[test]
    fn parse_platform_iphone_download_callback() {
        let data = "ringtone:platform:iphone:download:123";
        let parts: Vec<&str> = data.splitn(5, ':').collect();
        assert_eq!(parts[1], "platform");
        let platform = Platform::parse(parts[2]).expect("must parse");
        assert_eq!(platform.output_kind(), "iphone_ringtone");
        assert_eq!(parts[3], "download");
        assert_eq!(parts[4].parse::<i64>().unwrap(), 123);
    }

    #[test]
    fn parse_platform_android_cut_callback() {
        let data = "ringtone:platform:android:cut:789";
        let parts: Vec<&str> = data.splitn(5, ':').collect();
        assert_eq!(parts[1], "platform");
        let platform = Platform::parse(parts[2]).expect("must parse");
        assert_eq!(platform.output_kind(), "android_ringtone");
        assert_eq!(parts[3], "cut");
        assert_eq!(parts[4].parse::<i64>().unwrap(), 789);
    }

    #[test]
    fn parse_unknown_action_is_neither_select_nor_platform() {
        let data = "ringtone:unknown_action";
        let parts: Vec<&str> = data.splitn(5, ':').collect();
        let action = parts.get(1).copied().unwrap_or("");
        assert_ne!(action, "select");
        assert_ne!(action, "platform");
    }

    #[test]
    fn parse_missing_source_id_defaults_to_zero() {
        let data = "ringtone:select:download";
        let parts: Vec<&str> = data.splitn(5, ':').collect();
        let source_id = parts.get(3).and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
        assert_eq!(source_id, 0);
    }

    // ==================== Output kind detection ====================

    #[test]
    fn output_kind_iphone_detected_correctly() {
        let ok = "iphone_ringtone";
        let is_iphone = ok == "iphone_ringtone";
        let is_android = ok == "android_ringtone";
        assert!(is_iphone && !is_android);
    }

    #[test]
    fn output_kind_android_detected_correctly() {
        let ok = "android_ringtone";
        let is_iphone = ok == "iphone_ringtone";
        let is_android = ok == "android_ringtone";
        assert!(!is_iphone && is_android);
    }

    #[test]
    fn output_kind_cut_is_not_ringtone() {
        let ok = "cut";
        assert!(!(ok == "iphone_ringtone" || ok == "android_ringtone"));
    }

    #[test]
    fn output_kind_video_note_is_not_ringtone() {
        let ok = "video_note";
        assert!(!(ok == "iphone_ringtone" || ok == "android_ringtone"));
    }

    // ==================== Platform selector callback format (round-trip) ====================

    #[test]
    fn platform_selector_iphone_callback_roundtrip() {
        let source_kind = "download";
        let source_id = 42i64;
        let cb = format!("ringtone:platform:iphone:{}:{}", source_kind, source_id);
        assert_eq!(cb, "ringtone:platform:iphone:download:42");
        let parts: Vec<&str> = cb.splitn(5, ':').collect();
        let platform = Platform::parse(parts[2]).unwrap();
        assert_eq!(platform.output_kind(), "iphone_ringtone");
        assert_eq!(parts[3], "download");
        assert_eq!(parts[4].parse::<i64>().unwrap(), 42);
    }

    #[test]
    fn platform_selector_android_callback_roundtrip() {
        let source_kind = "cut";
        let source_id = 99i64;
        let cb = format!("ringtone:platform:android:{}:{}", source_kind, source_id);
        assert_eq!(cb, "ringtone:platform:android:cut:99");
        let parts: Vec<&str> = cb.splitn(5, ':').collect();
        let platform = Platform::parse(parts[2]).unwrap();
        assert_eq!(platform.output_kind(), "android_ringtone");
        assert_eq!(parts[3], "cut");
        assert_eq!(parts[4].parse::<i64>().unwrap(), 99);
    }

    // ==================== i18n key presence ====================

    #[test]
    fn i18n_keys_present_in_en_locale() {
        let lang = crate::i18n::lang_from_code("en");
        let keys = [
            "ringtone-platform-select",
            "ringtone-prompt-iphone",
            "ringtone-prompt-android",
            "ringtone-instructions-iphone",
            "ringtone-instructions-android",
        ];
        for key in &keys {
            let result = crate::i18n::t(&lang, key);
            assert!(
                !result.is_empty() && &result != key,
                "Key '{}' missing or returned key itself in en locale",
                key
            );
        }
    }

    #[test]
    fn i18n_keys_present_in_ru_locale() {
        let lang = crate::i18n::lang_from_code("ru");
        let keys = [
            "ringtone-platform-select",
            "ringtone-prompt-iphone",
            "ringtone-prompt-android",
            "ringtone-instructions-iphone",
            "ringtone-instructions-android",
        ];
        for key in &keys {
            let result = crate::i18n::t(&lang, key);
            assert!(
                !result.is_empty() && &result != key,
                "Key '{}' missing or returned key itself in ru locale",
                key
            );
        }
    }

    #[test]
    fn iphone_prompt_mentions_30_sec() {
        let lang = crate::i18n::lang_from_code("en");
        let prompt = crate::i18n::t(&lang, "ringtone-prompt-iphone");
        assert!(prompt.contains("30"), "iPhone prompt must mention 30, got: {}", prompt);
    }

    #[test]
    fn android_prompt_mentions_40_sec() {
        let lang = crate::i18n::lang_from_code("en");
        let prompt = crate::i18n::t(&lang, "ringtone-prompt-android");
        assert!(prompt.contains("40"), "Android prompt must mention 40, got: {}", prompt);
    }

    #[test]
    fn iphone_instructions_mention_m4r() {
        let lang = crate::i18n::lang_from_code("en");
        let text = crate::i18n::t(&lang, "ringtone-instructions-iphone");
        assert!(
            text.contains("m4r") || text.contains("M4R"),
            "iPhone instructions must mention .m4r"
        );
    }

    #[test]
    fn android_instructions_mention_mp3() {
        let lang = crate::i18n::lang_from_code("en");
        let text = crate::i18n::t(&lang, "ringtone-instructions-android");
        assert!(
            text.contains("mp3") || text.contains("MP3"),
            "Android instructions must mention .mp3"
        );
    }
}
