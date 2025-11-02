use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};
use teloxide::RequestError;
use crate::db::{self, DbPool};
use std::sync::Arc;

/// Структура с лимитами плана подписки
#[derive(Debug, Clone)]
pub struct PlanLimits {
    /// Интервал между запросами в секундах
    pub rate_limit_seconds: u64,
    /// Лимит загрузок в день (None = неограниченно)
    pub daily_download_limit: Option<u32>,
    /// Максимальный размер файла в MB
    pub max_file_size_mb: u32,
    /// Доступные форматы
    pub allowed_formats: Vec<String>,
    /// Приоритет в очереди (0-100, где 100 - максимальный)
    pub queue_priority: u8,
    /// Можно ли выбирать качество видео
    pub can_choose_video_quality: bool,
    /// Можно ли выбирать битрейт аудио
    pub can_choose_audio_bitrate: bool,
}

impl PlanLimits {
    /// Получает лимиты для указанного плана
    pub fn for_plan(plan: &str) -> Self {
        match plan {
            "premium" => PlanLimits {
                rate_limit_seconds: 10,
                daily_download_limit: None, // Неограниченно
                max_file_size_mb: 100,
                allowed_formats: vec!["mp3".to_string(), "mp4".to_string(), "srt".to_string(), "txt".to_string()],
                queue_priority: 70,
                can_choose_video_quality: true,
                can_choose_audio_bitrate: true,
            },
            "vip" => PlanLimits {
                rate_limit_seconds: 5,
                daily_download_limit: None, // Неограниченно
                max_file_size_mb: 200,
                allowed_formats: vec!["mp3".to_string(), "mp4".to_string(), "srt".to_string(), "txt".to_string()],
                queue_priority: 100,
                can_choose_video_quality: true,
                can_choose_audio_bitrate: true,
            },
            _ => PlanLimits {
                rate_limit_seconds: 30,
                daily_download_limit: Some(5),
                max_file_size_mb: 49,
                allowed_formats: vec!["mp3".to_string(), "mp4".to_string()],
                queue_priority: 0,
                can_choose_video_quality: false,
                can_choose_audio_bitrate: false,
            },
        }
    }
}

/// Показывает информацию о текущем плане пользователя и доступных подписках
pub async fn show_subscription_info(bot: &Bot, chat_id: ChatId, db_pool: Arc<DbPool>) -> ResponseResult<Message> {
    let conn = db::get_connection(&db_pool)
        .map_err(|e| RequestError::from(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;
    
    let user = match db::get_user(&conn, chat_id.0) {
        Ok(Some(u)) => u,
        Ok(None) => {
            // Создаем пользователя если его нет
            if let Err(e) = db::create_user(&conn, chat_id.0, None) {
                log::error!("Failed to create user: {}", e);
            }
            // Пробуем получить снова
            db::get_user(&conn, chat_id.0)
                .map_err(|e| RequestError::from(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?
                .unwrap_or_else(|| {
                    // Fallback к free плану
                    crate::db::User {
                        telegram_id: chat_id.0,
                        username: None,
                        plan: "free".to_string(),
                        download_format: "mp3".to_string(),
                        download_subtitles: 0,
                        video_quality: "best".to_string(),
                        audio_bitrate: "320k".to_string(),
                    }
                })
        }
        Err(e) => {
            log::error!("Failed to get user: {}", e);
            return Err(RequestError::from(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())));
        }
    };
    
    let current_plan = &user.plan;
    let limits = PlanLimits::for_plan(current_plan);
    
    // Формируем текст сообщения
    let plan_emoji = match current_plan.as_str() {
        "premium" => "⭐",
        "vip" => "👑",
        _ => "🌟",
    };
    
    let plan_name = match current_plan.as_str() {
        "premium" => "Premium",
        "vip" => "VIP",
        _ => "Free",
    };
    
    let mut text = format!("💳 *Информация о подписке*\n\n");
    text.push_str(&format!("📊 *Твой текущий план:* {} {}\n", plan_emoji, plan_name));
    text.push_str(&format!("📅 *Действует до:* бессрочно\n\n"));
    
    text.push_str("━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n\n");
    text.push_str(&format!("*Твои лимиты:*\n"));
    text.push_str(&format!("⏱️ Интервал между запросами: {} сек\n", limits.rate_limit_seconds));
    
    if let Some(limit) = limits.daily_download_limit {
        text.push_str(&format!("📥 Лимит загрузок в день: {}\n", limit));
    } else {
        text.push_str("📥 Лимит загрузок в день: неограниченно\n");
    }
    
    text.push_str(&format!("📦 Максимальный размер файла: {} MB\n", limits.max_file_size_mb));
    
    if limits.can_choose_video_quality {
        text.push_str("🎬 Выбор качества видео: ✅\n");
    } else {
        text.push_str("🎬 Выбор качества видео: ❌\n");
    }
    
    if limits.can_choose_audio_bitrate {
        text.push_str("🎵 Выбор битрейта аудио: ✅\n");
    } else {
        text.push_str("🎵 Выбор битрейта аудио: ❌\n");
    }
    
    text.push_str("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n\n");
    text.push_str("*Доступные планы:*\n\n");
    
    // Premium план
    text.push_str("⭐ *Premium* \\- 299 Stars/мес\n");
    text.push_str("• 10 секунд между запросами\n");
    text.push_str("• Неограниченные загрузки\n");
    text.push_str("• Файлы до 100 MB\n");
    text.push_str("• Все форматы \\+ выбор качества\n");
    text.push_str("• Приоритетная очередь\n\n");
    
    // VIP план
    text.push_str("👑 *VIP* \\- 999 Stars/мес\n");
    text.push_str("• 5 секунд между запросами\n");
    text.push_str("• Неограниченные загрузки\n");
    text.push_str("• Файлы до 200 MB\n");
    text.push_str("• Все форматы \\+ выбор качества\n");
    text.push_str("• Максимальный приоритет\n");
    text.push_str("• Плейлисты до 100 треков\n");
    text.push_str("• Голосовые команды\n");
    
    // Создаем клавиатуру (пока без реальной оплаты)
    let keyboard = InlineKeyboardMarkup::new(vec![
        vec![InlineKeyboardButton::callback(
            "⭐ Premium".to_string(),
            "subscribe:premium"
        )],
        vec![InlineKeyboardButton::callback(
            "👑 VIP".to_string(),
            "subscribe:vip"
        )],
        vec![InlineKeyboardButton::callback(
            "🔙 Назад".to_string(),
            "back:main"
        )],
    ]);
    
    bot.send_message(chat_id, text)
        .parse_mode(teloxide::types::ParseMode::MarkdownV2)
        .reply_markup(keyboard)
        .await
}


