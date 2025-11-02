use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup, MessageId};
use teloxide::RequestError;
use crate::db::{self, DbPool};
use crate::queue::{DownloadTask, DownloadQueue};
use crate::rate_limiter::RateLimiter;
use crate::history::handle_history_callback;
use crate::export::handle_export;
use std::sync::Arc;
use url::Url;
use base64::{Engine as _, engine::general_purpose::STANDARD};

/// Показывает главное меню настроек режима загрузки.
/// 
/// Отображает меню с инлайн-кнопками для выбора типа загрузки и просмотра доступных сервисов.
/// 
/// # Arguments
/// 
/// * `bot` - Экземпляр Telegram бота
/// * `chat_id` - ID чата пользователя
/// * `db_pool` - Пул соединений с базой данных
/// 
/// # Returns
/// 
/// Возвращает `ResponseResult<Message>` с отправленным сообщением или ошибку.
/// 
/// # Errors
/// 
/// Возвращает ошибку если не удалось получить соединение с БД или отправить сообщение.
pub async fn show_main_menu(bot: &Bot, chat_id: ChatId, db_pool: Arc<DbPool>) -> ResponseResult<Message> {
    let conn = db::get_connection(&db_pool)
        .map_err(|e| RequestError::from(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;
    let format = db::get_user_download_format(&conn, chat_id.0).unwrap_or_else(|_| "mp3".to_string());
    let video_quality = db::get_user_video_quality(&conn, chat_id.0).unwrap_or_else(|_| "best".to_string());
    let audio_bitrate = db::get_user_audio_bitrate(&conn, chat_id.0).unwrap_or_else(|_| "320k".to_string());
    
    let format_emoji = match format.as_str() {
        "mp3" => "🎵 MP3",
        "mp4" => "🎬 MP4",
        "srt" => "📝 SRT",
        "txt" => "📄 TXT",
        _ => "🎵 MP3",
    };
    
    let quality_emoji = match video_quality.as_str() {
        "1080p" => "🎬 1080p",
        "720p" => "🎬 720p",
        "480p" => "🎬 480p",
        "360p" => "🎬 360p",
        _ => "🎬 Best",
    };
    
    let bitrate_display = match audio_bitrate.as_str() {
        "128k" => "128 kbps",
        "192k" => "192 kbps",
        "256k" => "256 kbps",
        "320k" => "320 kbps",
        _ => "320 kbps",
    };
    
    let keyboard = InlineKeyboardMarkup::new(vec![
        vec![InlineKeyboardButton::callback(
            format!("📥 Тип загрузки: {}", format_emoji),
            "mode:download_type"
        )],
        vec![InlineKeyboardButton::callback(
            if format == "mp4" {
                format!("🎬 Качество видео: {}", quality_emoji)
            } else {
                format!("🎵 Битрейт аудио: {}", bitrate_display)
            },
            if format == "mp4" {
                "mode:video_quality"
            } else {
                "mode:audio_bitrate"
            }
        )],
        vec![InlineKeyboardButton::callback(
            "🌐 Доступные сервисы".to_string(),
            "mode:services"
        )],
    ]);
    
    bot.send_message(chat_id, "🎵 *Дора \\- Режимы Загрузки*\n\nВыбери, что хочешь настроить\\!")
        .parse_mode(teloxide::types::ParseMode::MarkdownV2)
        .reply_markup(keyboard)
        .await
}

/// Показывает меню выбора типа загрузки.
/// 
/// Отображает меню с доступными форматами (MP3, MP4, SRT, TXT) и отмечает текущий выбор пользователя.
/// 
/// # Arguments
/// 
/// * `bot` - Экземпляр Telegram бота
/// * `chat_id` - ID чата пользователя
/// * `message_id` - ID сообщения для редактирования
/// * `db_pool` - Пул соединений с базой данных
/// 
/// # Returns
/// 
/// Возвращает `ResponseResult<()>` или ошибку при редактировании сообщения.
pub async fn show_download_type_menu(bot: &Bot, chat_id: ChatId, message_id: MessageId, db_pool: Arc<DbPool>) -> ResponseResult<()> {
    let conn = db::get_connection(&db_pool)
        .map_err(|e| RequestError::from(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;
    let current_format = db::get_user_download_format(&conn, chat_id.0).unwrap_or_else(|_| "mp3".to_string());
    
    let keyboard = InlineKeyboardMarkup::new(vec![
        vec![
            InlineKeyboardButton::callback(
                if current_format == "mp3" { "🎵 MP3 ✓" } else { "🎵 MP3" }.to_string(),
                "format:mp3"
            ),
            InlineKeyboardButton::callback(
                if current_format == "mp4" { "🎬 MP4 ✓" } else { "🎬 MP4" }.to_string(),
                "format:mp4"
            ),
        ],
        vec![
            InlineKeyboardButton::callback(
                if current_format == "srt" { "📝 SRT ✓" } else { "📝 SRT" }.to_string(),
                "format:srt"
            ),
            InlineKeyboardButton::callback(
                if current_format == "txt" { "📄 TXT ✓" } else { "📄 TXT" }.to_string(),
                "format:txt"
            ),
        ],
        vec![InlineKeyboardButton::callback(
            "🔙 Назад".to_string(),
            "back:main"
        )],
    ]);
    
    bot.edit_message_text(chat_id, message_id, "Выбери формат для скачивания\\:\n\n*Текущий формат\\: " 
        .to_string() + match current_format.as_str() {
            "mp3" => "🎵 MP3",
            "mp4" => "🎬 MP4",
            "srt" => "📝 SRT",
            "txt" => "📄 TXT",
            _ => "🎵 MP3",
        } + "*")
        .parse_mode(teloxide::types::ParseMode::MarkdownV2)
        .reply_markup(keyboard)
        .await?;
    Ok(())
}

/// Показывает меню выбора качества видео.
/// 
/// Отображает меню с доступными качествами (1080p, 720p, 480p, 360p, best) и отмечает текущий выбор пользователя.
/// 
/// # Arguments
/// 
/// * `bot` - Экземпляр Telegram бота
/// * `chat_id` - ID чата пользователя
/// * `message_id` - ID сообщения для редактирования
/// * `db_pool` - Пул соединений с базой данных
/// 
/// # Returns
/// 
/// Возвращает `ResponseResult<()>` или ошибку при редактировании сообщения.
pub async fn show_video_quality_menu(bot: &Bot, chat_id: ChatId, message_id: MessageId, db_pool: Arc<DbPool>) -> ResponseResult<()> {
    let conn = db::get_connection(&db_pool)
        .map_err(|e| RequestError::from(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;
    let current_quality = db::get_user_video_quality(&conn, chat_id.0).unwrap_or_else(|_| "best".to_string());
    
    let keyboard = InlineKeyboardMarkup::new(vec![
        vec![
            InlineKeyboardButton::callback(
                if current_quality == "1080p" { "🎬 1080p (Full HD) ✓" } else { "🎬 1080p (Full HD)" }.to_string(),
                "quality:1080p"
            ),
            InlineKeyboardButton::callback(
                if current_quality == "720p" { "🎬 720p (HD) ✓" } else { "🎬 720p (HD)" }.to_string(),
                "quality:720p"
            ),
        ],
        vec![
            InlineKeyboardButton::callback(
                if current_quality == "480p" { "🎬 480p (SD) ✓" } else { "🎬 480p (SD)" }.to_string(),
                "quality:480p"
            ),
            InlineKeyboardButton::callback(
                if current_quality == "360p" { "🎬 360p (Low) ✓" } else { "🎬 360p (Low)" }.to_string(),
                "quality:360p"
            ),
        ],
        vec![InlineKeyboardButton::callback(
            if current_quality == "best" { "🎬 Best (Авто) ✓" } else { "🎬 Best (Авто)" }.to_string(),
            "quality:best"
        )],
        vec![InlineKeyboardButton::callback(
            "🔙 Назад".to_string(),
            "back:main"
        )],
    ]);
    
    let quality_display = match current_quality.as_str() {
        "1080p" => "🎬 1080p (Full HD)",
        "720p" => "🎬 720p (HD)",
        "480p" => "🎬 480p (SD)",
        "360p" => "🎬 360p (Low)",
        _ => "🎬 Best (Авто)",
    };
    
    bot.edit_message_text(chat_id, message_id, format!("Выбери качество видео\\:\n\n*Текущее качество\\: {}*", quality_display))
        .parse_mode(teloxide::types::ParseMode::MarkdownV2)
        .reply_markup(keyboard)
        .await?;
    Ok(())
}

/// Показывает меню выбора битрейта аудио.
/// 
/// Отображает меню с доступными битрейтами (128kbps, 192kbps, 256kbps, 320kbps) и отмечает текущий выбор пользователя.
/// 
/// # Arguments
/// 
/// * `bot` - Экземпляр Telegram бота
/// * `chat_id` - ID чата пользователя
/// * `message_id` - ID сообщения для редактирования
/// * `db_pool` - Пул соединений с базой данных
/// 
/// # Returns
/// 
/// Возвращает `ResponseResult<()>` или ошибку при редактировании сообщения.
pub async fn show_audio_bitrate_menu(bot: &Bot, chat_id: ChatId, message_id: MessageId, db_pool: Arc<DbPool>) -> ResponseResult<()> {
    let conn = db::get_connection(&db_pool)
        .map_err(|e| RequestError::from(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;
    let current_bitrate = db::get_user_audio_bitrate(&conn, chat_id.0).unwrap_or_else(|_| "320k".to_string());
    
    let keyboard = InlineKeyboardMarkup::new(vec![
        vec![
            InlineKeyboardButton::callback(
                if current_bitrate == "128k" { "🎵 128 kbps ✓" } else { "🎵 128 kbps" }.to_string(),
                "bitrate:128k"
            ),
            InlineKeyboardButton::callback(
                if current_bitrate == "192k" { "🎵 192 kbps ✓" } else { "🎵 192 kbps" }.to_string(),
                "bitrate:192k"
            ),
        ],
        vec![
            InlineKeyboardButton::callback(
                if current_bitrate == "256k" { "🎵 256 kbps ✓" } else { "🎵 256 kbps" }.to_string(),
                "bitrate:256k"
            ),
            InlineKeyboardButton::callback(
                if current_bitrate == "320k" { "🎵 320 kbps ✓" } else { "🎵 320 kbps" }.to_string(),
                "bitrate:320k"
            ),
        ],
        vec![InlineKeyboardButton::callback(
            "🔙 Назад".to_string(),
            "back:main"
        )],
    ]);
    
    bot.edit_message_text(chat_id, message_id, format!("Выбери битрейт для аудио\\:\n\n*Текущий битрейт\\: {}*", current_bitrate))
        .parse_mode(teloxide::types::ParseMode::MarkdownV2)
        .reply_markup(keyboard)
        .await?;
    Ok(())
}

/// Показывает меню с информацией о поддерживаемых сервисах.
/// 
/// Отображает список доступных сервисов (YouTube, SoundCloud) и поддерживаемых форматов.
/// 
/// # Arguments
/// 
/// * `bot` - Экземпляр Telegram бота
/// * `chat_id` - ID чата пользователя
/// * `message_id` - ID сообщения для редактирования
/// 
/// # Returns
/// 
/// Возвращает `ResponseResult<()>` или ошибку при редактировании сообщения.
pub async fn show_services_menu(bot: &Bot, chat_id: ChatId, message_id: MessageId) -> ResponseResult<()> {
    let keyboard = InlineKeyboardMarkup::new(vec![
        vec![InlineKeyboardButton::callback(
            "🔙 Назад".to_string(),
            "back:main"
        )],
    ]);
    
    let text = "🌐 *Поддерживаемые сервисы*\n\n\
        🎥 *YouTube*\n\
        • MP3 \\(Аудио\\)\n\
        • MP4 \\(Видео\\)\n\
        • SRT \\(Субтитры\\)\n\
        • TXT \\(Текстовые субтитры\\)\n\n\
        🎵 *SoundCloud*\n\
        • MP3 \\(Аудио\\)\n\n\
        📱 *VK \\(ВКонтакте\\)*\n\
        • MP3 \\(Аудио\\)\n\
        • MP4 \\(Видео\\)\n\n\
        🎬 *TikTok*\n\
        • MP3 \\(Аудио\\)\n\
        • MP4 \\(Видео\\)\n\n\
        📸 *Instagram*\n\
        • MP3 \\(Аудио из Reels\\)\n\
        • MP4 \\(Видео Reels\\)\n\n\
        🎮 *Twitch*\n\
        • MP4 \\(Клипы\\)\n\n\
        🎧 *Spotify*\n\
        • MP3 \\(Аудио\\)\n\n\
        И многие другие сервисы, поддерживаемые yt\\-dlp\\!\n\n\
        Просто отправь мне ссылку на трек или видео\\! ❤️‍🔥";
    
    bot.edit_message_text(chat_id, message_id, text)
        .parse_mode(teloxide::types::ParseMode::MarkdownV2)
        .reply_markup(keyboard)
        .await?;
    Ok(())
}

// Edit message to show main menu (for callbacks that need to edit existing message)
// Args: bot - telegram bot instance, chat_id - user's chat ID, message_id - ID of message to edit, db_pool - database connection pool
// Functionality: Edits existing message to show main mode menu
async fn edit_main_menu(bot: &Bot, chat_id: ChatId, message_id: MessageId, db_pool: Arc<DbPool>) -> ResponseResult<()> {
    let conn = db::get_connection(&db_pool)
        .map_err(|e| RequestError::from(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;
    let format = db::get_user_download_format(&conn, chat_id.0).unwrap_or_else(|_| "mp3".to_string());
    let video_quality = db::get_user_video_quality(&conn, chat_id.0).unwrap_or_else(|_| "best".to_string());
    let audio_bitrate = db::get_user_audio_bitrate(&conn, chat_id.0).unwrap_or_else(|_| "320k".to_string());
    
    let format_emoji = match format.as_str() {
        "mp3" => "🎵 MP3",
        "mp4" => "🎬 MP4",
        "srt" => "📝 SRT",
        "txt" => "📄 TXT",
        _ => "🎵 MP3",
    };
    
    let quality_emoji = match video_quality.as_str() {
        "1080p" => "🎬 1080p",
        "720p" => "🎬 720p",
        "480p" => "🎬 480p",
        "360p" => "🎬 360p",
        _ => "🎬 Best",
    };
    
    let bitrate_display = match audio_bitrate.as_str() {
        "128k" => "128 kbps",
        "192k" => "192 kbps",
        "256k" => "256 kbps",
        "320k" => "320 kbps",
        _ => "320 kbps",
    };
    
    let keyboard = InlineKeyboardMarkup::new(vec![
        vec![InlineKeyboardButton::callback(
            format!("📥 Тип загрузки: {}", format_emoji),
            "mode:download_type"
        )],
        vec![InlineKeyboardButton::callback(
            if format == "mp4" {
                format!("🎬 Качество видео: {}", quality_emoji)
            } else {
                format!("🎵 Битрейт аудио: {}", bitrate_display)
            },
            if format == "mp4" {
                "mode:video_quality"
            } else {
                "mode:audio_bitrate"
            }
        )],
        vec![InlineKeyboardButton::callback(
            "🌐 Доступные сервисы".to_string(),
            "mode:services"
        )],
    ]);
    
    bot.edit_message_text(chat_id, message_id, "🎵 *Дора \\- Режимы Загрузки*\n\nВыбери, что хочешь настроить\\!")
        .parse_mode(teloxide::types::ParseMode::MarkdownV2)
        .reply_markup(keyboard)
        .await?;
    Ok(())
}

/// Обрабатывает callback-запросы от инлайн-клавиатур меню.
/// 
/// Обрабатывает нажатия на кнопки меню и обновляет настройки пользователя или переключает между меню.
/// 
/// # Arguments
/// 
/// * `bot` - Экземпляр Telegram бота
/// * `q` - Callback query для обработки
/// * `db_pool` - Пул соединений с базой данных
/// * `download_queue` - Очередь загрузок
/// * `rate_limiter` - Rate limiter
/// 
/// # Returns
/// 
/// Возвращает `ResponseResult<()>` или ошибку при обработке callback.
/// 
/// # Supported Callbacks
/// 
/// - `mode:download_type` - Переход к меню выбора формата
/// - `mode:services` - Показ информации о сервисах
/// - `back:main` - Возврат к главному меню
/// - `format:mp3|mp4|srt|txt` - Установка формата загрузки
/// - `download:format:url` - Начать загрузку с указанным форматом
/// - `preview:settings:url` - Показать настройки для превью
/// - `preview:cancel:url` - Отменить превью
pub async fn handle_menu_callback(
    bot: Bot, 
    q: CallbackQuery, 
    db_pool: Arc<DbPool>,
    download_queue: Arc<DownloadQueue>,
    rate_limiter: Arc<RateLimiter>,
) -> ResponseResult<()> {
    let callback_id = q.id.clone();
    if let Some(data) = q.data {
        let chat_id = q.message.as_ref().map(|m| m.chat.id);
        let message_id = q.message.as_ref().map(|m| m.id);
        
        if let (Some(chat_id), Some(message_id)) = (chat_id, message_id) {
            if data.starts_with("mode:") {
                bot.answer_callback_query(callback_id.clone()).await?;
                match data.as_str() {
                    "mode:download_type" => {
                        show_download_type_menu(&bot, chat_id, message_id, Arc::clone(&db_pool)).await?;
                    }
                    "mode:video_quality" => {
                        show_video_quality_menu(&bot, chat_id, message_id, Arc::clone(&db_pool)).await?;
                    }
                    "mode:audio_bitrate" => {
                        show_audio_bitrate_menu(&bot, chat_id, message_id, Arc::clone(&db_pool)).await?;
                    }
                    "mode:services" => {
                        show_services_menu(&bot, chat_id, message_id).await?;
                    }
                    _ => {}
                }
            } else if data.starts_with("quality:") {
                bot.answer_callback_query(callback_id.clone()).await?;
                let quality = &data[8..]; // Remove "quality:" prefix
                let conn = db::get_connection(&db_pool)
                    .map_err(|e| RequestError::from(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;
                db::set_user_video_quality(&conn, chat_id.0, quality)
                    .map_err(|e| RequestError::from(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;
                
                // Update the menu to show new selection
                show_video_quality_menu(&bot, chat_id, message_id, Arc::clone(&db_pool)).await?;
            } else if data.starts_with("bitrate:") {
                bot.answer_callback_query(callback_id.clone()).await?;
                let bitrate = &data[8..]; // Remove "bitrate:" prefix
                let conn = db::get_connection(&db_pool)
                    .map_err(|e| RequestError::from(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;
                db::set_user_audio_bitrate(&conn, chat_id.0, bitrate)
                    .map_err(|e| RequestError::from(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;
                
                // Update the menu to show new selection
                show_audio_bitrate_menu(&bot, chat_id, message_id, Arc::clone(&db_pool)).await?;
            } else if data.starts_with("back:") {
                bot.answer_callback_query(callback_id.clone()).await?;
                match data.as_str() {
                    "back:main" => {
                        edit_main_menu(&bot, chat_id, message_id, Arc::clone(&db_pool)).await?;
                    }
                    "back:start" => {
                        bot.edit_message_text(chat_id, message_id, "Хэй\\! Я Дора, дай мне ссылку и я скачаю ❤️‍🔥")
                            .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                            .await?;
                    }
                    _ => {}
                }
            } else if data.starts_with("format:") {
                bot.answer_callback_query(callback_id.clone()).await?;
                let format = &data[7..]; // Remove "format:" prefix
                let conn = db::get_connection(&db_pool)
                    .map_err(|e| RequestError::from(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;
                db::set_user_download_format(&conn, chat_id.0, format)
                    .map_err(|e| RequestError::from(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;
                
                // Update the menu to show new selection
                show_download_type_menu(&bot, chat_id, message_id, Arc::clone(&db_pool)).await?;
            } else if data.starts_with("download:") {
                // Don't answer immediately - we'll answer after processing
                // Format: download:format:base64_url
                let parts: Vec<&str> = data.splitn(3, ':').collect();
                if parts.len() == 3 {
                    let format = parts[1];
                    let url_encoded = parts[2];
                    
                    // Decode URL from base64
                    match STANDARD.decode(url_encoded) {
                        Ok(url_bytes) => {
                            match String::from_utf8(url_bytes) {
                                Ok(url_str) => {
                                    match Url::parse(&url_str) {
                                        Ok(url) => {
                                            // Get user preferences for quality/bitrate and plan
                                            let conn = db::get_connection(&db_pool)
                                                .map_err(|e| RequestError::from(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;
                                            let plan = match db::get_user(&conn, chat_id.0) {
                                                Ok(Some(ref user)) => user.plan.clone(),
                                                _ => "free".to_string(),
                                            };
                                            
                                            // Check rate limit
                                            if rate_limiter.is_rate_limited(chat_id, &plan).await {
                                                if let Some(remaining_time) = rate_limiter.get_remaining_time(chat_id).await {
                                                    let remaining_seconds = remaining_time.as_secs();
                                                    bot.answer_callback_query(callback_id)
                                                        .text(&format!("Подожди {} секунд", remaining_seconds))
                                                        .await?;
                                                } else {
                                                    bot.answer_callback_query(callback_id)
                                                        .text("Подожди немного")
                                                        .await?;
                                                }
                                                return Ok(());
                                            }
                                            
                                            bot.answer_callback_query(callback_id.clone()).await?;
                                            
                                            rate_limiter.update_rate_limit(chat_id, &plan).await;
                                            let video_quality = if format == "mp4" {
                                                Some(db::get_user_video_quality(&conn, chat_id.0).unwrap_or_else(|_| "best".to_string()))
                                            } else {
                                                None
                                            };
                                            let audio_bitrate = if format == "mp3" {
                                                Some(db::get_user_audio_bitrate(&conn, chat_id.0).unwrap_or_else(|_| "320k".to_string()))
                                            } else {
                                                None
                                            };
                                            
                                            // Add task to queue
                                            let is_video = format == "mp4";
                                            let task = DownloadTask::from_plan(url.as_str().to_string(), chat_id, is_video, format.to_string(), video_quality, audio_bitrate, &plan);
                                            download_queue.add_task(task).await;
                                            
                                            // Delete preview message
                                            if let Err(e) = bot.delete_message(chat_id, message_id).await {
                                                log::warn!("Failed to delete preview message: {:?}", e);
                                            }
                                            
                                            // Send confirmation
                                            let confirmation_msg = match format {
                                                "mp3" => "Я Дора, попробую скачать тебе трек! 🎵 Терпение!",
                                                "mp4" => "Я Дора, попробую скачать тебе видео! 🎥 Терпение!",
                                                "srt" => "Я Дора, попробую скачать тебе субтитры! 📝 Терпение!",
                                                "txt" => "Я Дора, попробую скачать тебе субтитры! 📄 Терпение!",
                                                _ => "Я Дора, попробую скачать тебе файл! ❤️‍🔥 Терпение!",
                                            };
                                            
                                            bot.send_message(chat_id, confirmation_msg).await?;
                                        }
                                        Err(e) => {
                                            log::error!("Failed to parse URL from callback: {}", e);
                                            bot.answer_callback_query(callback_id)
                                                .text("Ошибка: неверная ссылка")
                                                .await?;
                                        }
                                    }
                                }
                                Err(e) => {
                                    log::error!("Failed to decode URL string: {}", e);
                                    bot.answer_callback_query(callback_id)
                                        .text("Ошибка: не удалось декодировать ссылку")
                                        .await?;
                                }
                            }
                        }
                        Err(e) => {
                            log::error!("Failed to decode base64 URL: {}", e);
                            bot.answer_callback_query(callback_id)
                                .text("Ошибка: неверный формат данных")
                                .await?;
                        }
                    }
                }
            } else if data.starts_with("preview:") {
                // Format: preview:action:base64_url
                let parts: Vec<&str> = data.splitn(3, ':').collect();
                if parts.len() == 3 {
                    let action = parts[1];
                    match action {
                        "cancel" => {
                            bot.answer_callback_query(callback_id.clone()).await?;
                            // Delete preview message
                            if let Err(e) = bot.delete_message(chat_id, message_id).await {
                                log::warn!("Failed to delete preview message: {:?}", e);
                            }
                        }
                        "settings" => {
                            bot.answer_callback_query(callback_id.clone()).await?;
                            // Show settings menu
                            show_download_type_menu(&bot, chat_id, message_id, Arc::clone(&db_pool)).await?;
                        }
                        _ => {
                            bot.answer_callback_query(callback_id.clone())
                                .text("Неизвестное действие")
                                .await?;
                        }
                    }
                }
            } else if data.starts_with("history:") {
                // Handle history callbacks
                handle_history_callback(&bot, callback_id, chat_id, message_id, &data, Arc::clone(&db_pool), Arc::clone(&download_queue), Arc::clone(&rate_limiter)).await?;
            } else if data.starts_with("export:") {
                // Handle export callbacks
                bot.answer_callback_query(callback_id.clone()).await?;
                let format = &data[7..]; // Remove "export:" prefix
                handle_export(&bot, chat_id, format, Arc::clone(&db_pool)).await?;
            }
        }
    }
    
    Ok(())
}

