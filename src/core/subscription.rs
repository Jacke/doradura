use crate::storage::db::{self, DbPool};
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup, Seconds};
use teloxide::RequestError;
use url::Url;

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
                allowed_formats: vec![
                    "mp3".to_string(),
                    "mp4".to_string(),
                    "srt".to_string(),
                    "txt".to_string(),
                ],
                queue_priority: 70,
                can_choose_video_quality: true,
                can_choose_audio_bitrate: true,
            },
            "vip" => PlanLimits {
                rate_limit_seconds: 5,
                daily_download_limit: None, // Неограниченно
                max_file_size_mb: 200,
                allowed_formats: vec![
                    "mp3".to_string(),
                    "mp4".to_string(),
                    "srt".to_string(),
                    "txt".to_string(),
                ],
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
        .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

    let user = match db::get_user(&conn, chat_id.0) {
        Ok(Some(u)) => u,
        Ok(None) => {
            // Создаем пользователя если его нет
            if let Err(e) = db::create_user(&conn, chat_id.0, None) {
                log::error!("Failed to create user: {}", e);
            }
            // Пробуем получить снова
            db::get_user(&conn, chat_id.0)
                .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
                .unwrap_or_else(|| {
                    // Fallback к free плану
                    crate::storage::db::User {
                        telegram_id: chat_id.0,
                        username: None,
                        plan: "free".to_string(),
                        download_format: "mp3".to_string(),
                        download_subtitles: 0,
                        video_quality: "best".to_string(),
                        language: "ru".to_string(),
                        send_as_document: 0,
                        send_audio_as_document: 0,
                        audio_bitrate: "320k".to_string(),
                        subscription_expires_at: None,
                        telegram_charge_id: None,
                    }
                })
        }
        Err(e) => {
            log::error!("Failed to get user: {}", e);
            return Err(RequestError::from(std::sync::Arc::new(std::io::Error::other(
                e.to_string(),
            ))));
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

    let mut text = "💳 *Информация о подписке*\n\n".to_string();
    text.push_str(&format!("📊 *Твой текущий план:* {} {}\n", plan_emoji, plan_name));

    // Показываем дату окончания подписки
    if let Some(expires_at) = &user.subscription_expires_at {
        // Форматируем дату для отображения (из формата "2025-12-03 01:29:24" в "03.12.2025")
        let formatted_date = if let Some(date_part) = expires_at.split(' ').next() {
            let parts: Vec<&str> = date_part.split('-').collect();
            if parts.len() == 3 {
                format!("{}\\.{}\\.{}", parts[2], parts[1], parts[0])
            } else {
                expires_at.replace("-", "\\-").replace(":", "\\:")
            }
        } else {
            expires_at.replace("-", "\\-").replace(":", "\\:")
        };
        text.push_str(&format!("📅 *Действует до:* {}\n\n", formatted_date));
    } else {
        text.push_str("📅 *Действует до:* бессрочно\n\n");
    }

    text.push_str("━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n\n");
    text.push_str("*Твои лимиты:*\n");
    text.push_str(&format!(
        "⏱️ Интервал между запросами: {} сек\n",
        limits.rate_limit_seconds
    ));

    if let Some(limit) = limits.daily_download_limit {
        text.push_str(&format!("📥 Лимит загрузок в день: {}\n", limit));
    } else {
        text.push_str("📥 Лимит загрузок в день: неограниченно\n");
    }

    text.push_str(&format!(
        "📦 Максимальный размер файла: {} MB\n",
        limits.max_file_size_mb
    ));

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

    // Проверяем, есть ли активная подписка
    let has_subscription = user.telegram_charge_id.is_some();

    if !has_subscription {
        text.push_str("*Доступные планы:*\n\n");

        // Premium план
        text.push_str("⭐ *Premium* \\- 1 Star каждые 30 дней\n");
        text.push_str("• 10 секунд между запросами\n");
        text.push_str("• Неограниченные загрузки\n");
        text.push_str("• Файлы до 100 MB\n");
        text.push_str("• Все форматы \\+ выбор качества\n");
        text.push_str("• Приоритетная очередь\n\n");

        // VIP план
        text.push_str("👑 *VIP* \\- 2 Stars каждые 30 дней\n");
        text.push_str("• 5 секунд между запросами\n");
        text.push_str("• Неограниченные загрузки\n");
        text.push_str("• Файлы до 200 MB\n");
        text.push_str("• Все форматы \\+ выбор качества\n");
        text.push_str("• Максимальный приоритет\n");
        text.push_str("• Плейлисты до 100 треков\n");
        text.push_str("• Голосовые команды\n\n");

        text.push_str("💫 *Подписка с автопродлением*\n");
        text.push_str("Списание происходит автоматически каждые 30 дней\\.\n");
        text.push_str("Можно отменить в любой момент\\!\n");
    } else {
        text.push_str("✅ *У тебя активна подписка\\!*\n\n");
        text.push_str("Подписка продлевается автоматически каждые 30 дней\\.\n");
        text.push_str("Можно отменить в любой момент без потери текущего периода\\.\n");
    }

    // Создаем клавиатуру в зависимости от наличия подписки
    let mut keyboard_rows = Vec::new();

    if !has_subscription {
        // Показываем кнопки подписки только если нет активной подписки
        keyboard_rows.push(vec![InlineKeyboardButton::callback(
            "⭐ Premium".to_string(),
            "subscribe:premium",
        )]);
        keyboard_rows.push(vec![InlineKeyboardButton::callback(
            "👑 VIP".to_string(),
            "subscribe:vip",
        )]);
    } else {
        // Показываем кнопку отмены автопродления для активной подписки
        keyboard_rows.push(vec![InlineKeyboardButton::callback(
            "❌ Отменить автопродление".to_string(),
            "subscription:cancel",
        )]);
    }

    keyboard_rows.push(vec![InlineKeyboardButton::callback(
        "🔙 Назад".to_string(),
        "back:main",
    )]);

    let keyboard = InlineKeyboardMarkup::new(keyboard_rows);

    bot.send_message(chat_id, text)
        .parse_mode(teloxide::types::ParseMode::MarkdownV2)
        .reply_markup(keyboard)
        .await
}

/// Создает инвойс для оплаты подписки через Telegram Stars
///
/// Создает рекуррентный invoice с автоматическим ежемесячным списанием Stars.
/// Telegram будет автоматически списывать указанную сумму каждые 30 дней.
pub async fn create_subscription_invoice(bot: &Bot, chat_id: ChatId, plan: &str) -> ResponseResult<Message> {
    log::info!(
        "🎯 create_subscription_invoice called for chat_id: {}, plan: {}",
        chat_id.0,
        plan
    );

    let (title, description, price_stars) = match plan {
        "premium" => (
            "⭐ Premium план",
            "Premium подписка с автопродлением каждые 30 дней\n\n• 10 секунд между запросами\n• Неограниченные загрузки\n• Файлы до 100 MB\n• Все форматы + выбор качества\n• Приоритетная очередь\n\n💫 Автоматическое списание 1 Star каждые 30 дней",
            1u32, // 1 Star каждые 30 дней
        ),
        "vip" => (
            "👑 VIP план",
            "VIP подписка с автопродлением каждые 30 дней\n\n• 5 секунд между запросами\n• Неограниченные загрузки\n• Файлы до 200 MB\n• Все форматы + выбор качества\n• Максимальный приоритет\n• Плейлисты до 100 треков\n\n💫 Автоматическое списание 2 Stars каждые 30 дней",
            2u32, // 2 Stars каждые 30 дней
        ),
        _ => {
            log::error!("❌ Invalid plan requested: {}", plan);
            return Err(RequestError::from(std::sync::Arc::new(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Invalid plan",
            ))));
        }
    };

    // Создаем payload для идентификации платежа
    let payload = format!("subscription:{}:{}", plan, chat_id.0);
    log::info!("📦 Invoice payload: {}", payload);

    // Создаем инвойс с поддержкой подписок
    use teloxide::types::LabeledPrice;

    log::info!(
        "💰 Creating RECURRING subscription invoice link for {} plan - price: {} Stars every 30 days",
        plan,
        price_stars
    );
    log::info!(
        "📝 Invoice details: title='{}', currency=XTR, price={} Stars, subscription_period=2592000 sec (30 days)",
        title,
        price_stars
    );

    // Создаём invoice link с subscription_period
    let invoice_link_result = bot
        .create_invoice_link(
            title,
            description,
            payload,
            "XTR".to_string(), // Только XTR (Telegram Stars) для подписок
            vec![LabeledPrice::new(
                format!(
                    "{} подписка",
                    if plan == "premium" { "Premium" } else { "VIP" }
                ),
                price_stars, // Цена в Stars
            )],
        )
        .subscription_period(Seconds::from_seconds(2592000)) // 30 дней в секундах - АВТОПРОДЛЕНИЕ КАЖДЫЕ 30 ДНЕЙ
        .await;

    match invoice_link_result {
        Ok(invoice_link) => {
            log::info!("✅ Invoice link created successfully: {}", invoice_link);

            // Отправляем ссылку пользователю с инлайн-кнопкой
            use teloxide::types::InlineKeyboardButton;
            use teloxide::types::InlineKeyboardMarkup;

            let invoice_url = Url::parse(&invoice_link).map_err(|e| {
                RequestError::from(std::sync::Arc::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("Invalid invoice URL: {}", e),
                )))
            })?;

            let keyboard = InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::url(
                format!(
                    "💳 Оплатить {} ({}⭐)",
                    if plan == "premium" { "Premium" } else { "VIP" },
                    price_stars
                ),
                invoice_url,
            )]]);

            // Экранируем все спецсимволы MarkdownV2
            let escaped_description = description
                .replace("\\", "\\\\")
                .replace(".", "\\.")
                .replace("-", "\\-")
                .replace("(", "\\(")
                .replace(")", "\\)")
                .replace("+", "\\+")
                .replace("!", "\\!");

            bot.send_message(
                chat_id,
                format!(
                    "💫 *Подписка {}*\n\n{}\n\n✨ Нажми на кнопку ниже для оплаты:",
                    if plan == "premium" { "Premium" } else { "VIP" },
                    escaped_description
                ),
            )
            .parse_mode(teloxide::types::ParseMode::MarkdownV2)
            .reply_markup(keyboard)
            .await
        }
        Err(e) => {
            log::error!("❌ Failed to create invoice link: {:?}", e);
            log::error!("Error details: {}", e);
            Err(e)
        }
    }
}

/// Активирует подписку для пользователя
pub async fn activate_subscription(
    db_pool: Arc<DbPool>,
    telegram_id: i64,
    plan: &str,
    days: i32,
) -> Result<(), String> {
    let conn = db::get_connection(&db_pool).map_err(|e| format!("Failed to get connection: {}", e))?;

    // Обновляем план пользователя с датой окончания
    db::update_user_plan_with_expiry(&conn, telegram_id, plan, Some(days))
        .map_err(|e| format!("Failed to update plan: {}", e))?;

    log::info!(
        "Subscription activated: user_id={}, plan={}, days={}",
        telegram_id,
        plan,
        days
    );
    Ok(())
}

/// Обрабатывает успешный платеж и активирует/продлевает подписку
///
/// # Arguments
///
/// * `bot` - Экземпляр Telegram бота
/// * `msg` - Сообщение с информацией о платеже
/// * `db_pool` - Пул соединений с базой данных
///
/// # Returns
///
/// Возвращает `ResponseResult<()>` или ошибку при обработке платежа.
pub async fn handle_successful_payment(
    bot: &Bot,
    msg: &teloxide::types::Message,
    db_pool: Arc<DbPool>,
) -> ResponseResult<()> {
    if let Some(payment) = msg.successful_payment() {
        log::info!("Received payment: {:?}", payment);

        // Парсим payload: "subscription:premium:12345678"
        let parts: Vec<&str> = payment.invoice_payload.split(':').collect();
        if parts.len() == 3 && parts[0] == "subscription" {
            let plan = parts[1];
            let telegram_id = parts[2].parse::<i64>().unwrap_or(0);

            if telegram_id == 0 {
                log::error!("Invalid telegram_id in payment payload: {}", payment.invoice_payload);
                return Ok(());
            }

            let chat_id = msg.chat.id;

            // Обрабатываем платеж подписки
            log::info!(
                "Processing subscription payment for user {}, plan: {}",
                telegram_id,
                plan
            );

            // Сохраняем telegram_charge_id для управления подпиской
            let conn = db::get_connection(&db_pool)
                .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

            // Сохраняем charge_id из платежа (конвертируем в строку)
            let charge_id_str = payment.telegram_payment_charge_id.0.clone();
            if let Err(e) = db::update_telegram_charge_id(&conn, telegram_id, Some(&charge_id_str)) {
                log::error!("Failed to save telegram_charge_id: {}", e);
            }

            // Активируем подписку на 30 дней
            if let Err(e) = activate_subscription(Arc::clone(&db_pool), telegram_id, plan, 30).await {
                log::error!("Failed to activate subscription: {}", e);
                bot.send_message(
                    chat_id,
                    "❌ Произошла ошибка при активации подписки. Обратись к администратору.",
                )
                .await?;
            } else {
                let plan_emoji = if plan == "premium" { "⭐" } else { "👑" };
                let plan_name = if plan == "premium" { "Premium" } else { "VIP" };

                bot.send_message(
                    chat_id,
                    format!(
                        "✅ План {} {} успешно активирован\\!\n\n\
                        План действует 30 дней с момента покупки\\.\n\n\
                        Твои новые возможности:\n\
                        • Rate limit: {} сек\n\
                        • Макс\\. размер: {} MB\n\
                        • {} выбор качества\n\n\
                        Приятного использования\\! 🎉",
                        plan_emoji,
                        plan_name,
                        if plan == "premium" { "10" } else { "5" },
                        if plan == "premium" { "100" } else { "200" },
                        if plan == "premium" { "✅" } else { "✅✅" }
                    ),
                )
                .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                .await?;
            }
        } else {
            log::warn!("Invalid payment payload format: {}", payment.invoice_payload);
        }
    }

    Ok(())
}

/// Отменяет подписку пользователя (со стороны бота)
///
/// # Arguments
///
/// * `bot` - Экземпляр Telegram бота
/// * `telegram_id` - Telegram ID пользователя
/// * `db_pool` - Пул соединений с базой данных
///
/// # Returns
///
/// Возвращает `Result<(), String>` или ошибку при отмене подписки.
pub async fn cancel_subscription(bot: &Bot, telegram_id: i64, db_pool: Arc<DbPool>) -> Result<(), String> {
    let conn = db::get_connection(&db_pool).map_err(|e| format!("Failed to get connection: {}", e))?;

    // Получаем charge_id пользователя
    let user = db::get_user(&conn, telegram_id)
        .map_err(|e| format!("Failed to get user: {}", e))?
        .ok_or_else(|| "User not found".to_string())?;

    let charge_id = user
        .telegram_charge_id
        .ok_or_else(|| "No active subscription found".to_string())?;

    // Отменяем подписку через Bot API
    use teloxide::types::TelegramTransactionId;
    bot.edit_user_star_subscription(
        teloxide::types::UserId(telegram_id as u64),
        TelegramTransactionId(charge_id.clone()),
        true, // is_canceled = true
    )
    .await
    .map_err(|e| format!("Failed to cancel subscription: {:?}", e))?;

    log::info!("Subscription canceled for user {}", telegram_id);

    // Обнуляем charge_id в БД
    db::update_telegram_charge_id(&conn, telegram_id, None)
        .map_err(|e| format!("Failed to update charge_id: {}", e))?;

    Ok(())
}

/// Возобновляет подписку пользователя
///
/// # Arguments
///
/// * `bot` - Экземпляр Telegram бота
/// * `telegram_id` - Telegram ID пользователя
/// * `db_pool` - Пул соединений с базой данных
///
/// # Returns
///
/// Возвращает `Result<(), String>` или ошибку при возобновлении подписки.
pub async fn restore_subscription(bot: &Bot, telegram_id: i64, db_pool: Arc<DbPool>) -> Result<(), String> {
    let conn = db::get_connection(&db_pool).map_err(|e| format!("Failed to get connection: {}", e))?;

    // Получаем charge_id пользователя
    let user = db::get_user(&conn, telegram_id)
        .map_err(|e| format!("Failed to get user: {}", e))?
        .ok_or_else(|| "User not found".to_string())?;

    let charge_id = user
        .telegram_charge_id
        .ok_or_else(|| "No subscription found".to_string())?;

    // Возобновляем подписку через Bot API
    use teloxide::types::TelegramTransactionId;
    bot.edit_user_star_subscription(
        teloxide::types::UserId(telegram_id as u64),
        TelegramTransactionId(charge_id.clone()),
        false, // is_canceled = false
    )
    .await
    .map_err(|e| format!("Failed to restore subscription: {:?}", e))?;

    log::info!("Subscription restored for user {}", telegram_id);

    Ok(())
}
