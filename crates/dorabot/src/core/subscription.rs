use crate::core::metrics;
use crate::core::types::Plan;
use crate::storage::db::{self, DbPool};
use crate::telegram::Bot;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardMarkup, Seconds};
use teloxide::RequestError;
use url::Url;

/// Subscription plan limits structure
#[derive(Debug, Clone)]
pub struct PlanLimits {
    /// Interval between requests in seconds
    pub rate_limit_seconds: u64,
    /// Daily download limit (None = unlimited)
    pub daily_download_limit: Option<u32>,
    /// Maximum file size in MB
    pub max_file_size_mb: u32,
    /// Available formats
    pub allowed_formats: Vec<String>,
    /// Queue priority (0-100, where 100 is maximum)
    pub queue_priority: u8,
    /// Whether video quality selection is available
    pub can_choose_video_quality: bool,
    /// Whether audio bitrate selection is available
    pub can_choose_audio_bitrate: bool,
    /// Whether media file upload for conversion is available
    pub can_upload_media: bool,
}

impl PlanLimits {
    /// Returns the limits for the given plan
    pub fn for_plan(plan: Plan) -> Self {
        match plan {
            Plan::Premium => PlanLimits {
                rate_limit_seconds: 10,
                daily_download_limit: None, // Unlimited
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
                can_upload_media: true,
            },
            Plan::Vip => PlanLimits {
                rate_limit_seconds: 5,
                daily_download_limit: None, // Unlimited
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
                can_upload_media: true,
            },
            Plan::Free => PlanLimits {
                rate_limit_seconds: 30,
                daily_download_limit: Some(5),
                max_file_size_mb: 49,
                allowed_formats: vec!["mp3".to_string(), "mp4".to_string()],
                queue_priority: 0,
                can_choose_video_quality: false,
                can_choose_audio_bitrate: false,
                can_upload_media: true,
            },
        }
    }
}

/// Formats a subscription period into a human-readable string for logs
fn format_subscription_period_for_log(period: &Seconds) -> String {
    let seconds = period.seconds();
    let days = seconds as f64 / 86_400.0;
    let months = days / 30.0;

    format!("{seconds} seconds (~{days:.2} days, ~{months:.2} months)")
}

/// Shows information about the user's current plan and available subscriptions
pub async fn show_subscription_info(bot: &Bot, chat_id: ChatId, db_pool: Arc<DbPool>) -> ResponseResult<Message> {
    log::info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    log::info!("📊 SHOW SUBSCRIPTION INFO REQUEST");
    log::info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    log::info!("  • User ID: {}", chat_id.0);

    let conn = db::get_connection(&db_pool)
        .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

    let user = match db::get_user(&conn, chat_id.0) {
        Ok(Some(u)) => u,
        Ok(None) => {
            // Create user if not found
            if let Err(e) = db::create_user(&conn, chat_id.0, None) {
                log::error!("Failed to create user: {}", e);
            }
            // Try to fetch again
            db::get_user(&conn, chat_id.0)
                .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
                .unwrap_or_else(|| {
                    // Fallback to free plan
                    crate::storage::db::User {
                        telegram_id: chat_id.0,
                        username: None,
                        plan: Plan::Free,
                        download_format: "mp3".to_string(),
                        download_subtitles: 0,
                        video_quality: "best".to_string(),
                        language: "ru".to_string(),
                        send_as_document: 0,
                        send_audio_as_document: 0,
                        audio_bitrate: "320k".to_string(),
                        subscription_expires_at: None,
                        telegram_charge_id: None,
                        is_recurring: false,
                        burn_subtitles: 0,
                        progress_bar_style: "classic".to_string(),
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

    let subscription = db::get_subscription(&conn, chat_id.0).ok().flatten();
    let is_subscription_active = db::is_subscription_active(&conn, chat_id.0).unwrap_or(false);
    let subscription_plan = subscription.as_ref().map(|s| s.plan).unwrap_or(user.plan);
    let subscription_expires_at = subscription
        .as_ref()
        .and_then(|s| s.expires_at.clone())
        .or_else(|| user.subscription_expires_at.clone());
    let subscription_charge_id = subscription
        .as_ref()
        .and_then(|s| s.telegram_charge_id.clone())
        .or_else(|| user.telegram_charge_id.clone());
    let subscription_is_recurring = subscription
        .as_ref()
        .map(|s| s.is_recurring)
        .unwrap_or(user.is_recurring);

    log::info!("📋 User data from database:");
    log::info!("  • Plan: {}", subscription_plan);
    log::info!("  • Is recurring: {}", subscription_is_recurring);
    log::info!("  • Expires at: {:?}", subscription_expires_at);
    log::info!("  • Charge ID: {:?}", subscription_charge_id);
    log::info!("  • Active: {}", is_subscription_active);

    // If charge_id is present, try to fetch subscription info from the Telegram API
    if let Some(ref charge_id) = subscription_charge_id {
        log::info!("🔍 Fetching subscription info from Telegram API...");
        log::info!("  • Charge ID: {}", charge_id);

        // Fetch bot transactions (without parameters - get all available)
        match bot.get_star_transactions().await {
            Ok(star_transactions) => {
                log::info!("✅ Successfully fetched star transactions");
                log::info!("  • Total transactions count: {}", star_transactions.transactions.len());

                // Find the transaction matching our charge_id (compare transaction id)
                let matching_transaction = star_transactions.transactions.iter().find(|t| t.id.0 == *charge_id);

                if let Some(transaction) = matching_transaction {
                    log::info!("💳 Found matching transaction:");
                    log::info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
                    log::info!("");
                    log::info!("🔑 Transaction ID: {}", transaction.id.0);
                    log::info!("💰 Amount: {} Stars", transaction.amount);
                    if let Some(nanostar) = transaction.nanostar_amount {
                        log::info!("💫 Nanostar amount: {} (1/1000000000 shares)", nanostar);
                    }
                    log::info!("📅 Date: {}", transaction.date.format("%Y-%m-%d %H:%M:%S UTC"));
                    log::info!("");

                    // Parse Source (where the payment came from)
                    log::info!("📥 Source (payment from):");
                    if let Some(source) = &transaction.source {
                        use teloxide::types::TransactionPartner;
                        match source {
                            TransactionPartner::User(user_partner) => {
                                log::info!("  • Type: User payment");
                                log::info!("  • User ID: {}", user_partner.user.id.0);
                                log::info!("  • First name: {}", user_partner.user.first_name);
                                if let Some(last_name) = &user_partner.user.last_name {
                                    log::info!("  • Last name: {}", last_name);
                                }
                                if let Some(username) = &user_partner.user.username {
                                    log::info!("  • Username: @{}", username);
                                }
                                if let Some(lang) = &user_partner.user.language_code {
                                    log::info!("  • Language: {}", lang);
                                }
                                log::info!("  • Is premium: {}", user_partner.user.is_premium);
                                log::info!("  • Is bot: {}", user_partner.user.is_bot);

                                // Parse payment type
                                log::info!("");
                                log::info!("  📋 Payment details:");
                                use teloxide::types::TransactionPartnerUserKind;
                                match &user_partner.kind {
                                    TransactionPartnerUserKind::InvoicePayment(invoice) => {
                                        log::info!("    • Payment type: Invoice payment (subscription or one-time)");

                                        if let Some(payload) = &invoice.invoice_payload {
                                            log::info!("    • Invoice payload: {}", payload);
                                        }

                                        if let Some(period) = &invoice.subscription_period {
                                            log::info!(
                                                "    • Subscription period: {:?} -> {}",
                                                period,
                                                format_subscription_period_for_log(period)
                                            );
                                        } else {
                                            log::info!("    • Subscription period: None (one-time payment)");
                                        }

                                        if let Some(affiliate) = &invoice.affiliate {
                                            log::info!("    • Affiliate info: {:?}", affiliate);
                                        }
                                    }
                                    TransactionPartnerUserKind::PaidMediaPayment(media) => {
                                        log::info!("    • Payment type: Paid media payment");
                                        log::info!("    • Media data: {:?}", media);
                                    }
                                    TransactionPartnerUserKind::GiftPurchase(gift) => {
                                        log::info!("    • Payment type: Gift purchase");
                                        log::info!("    • Gift data: {:?}", gift);
                                    }
                                    TransactionPartnerUserKind::PremiumPurchase(premium) => {
                                        log::info!("    • Payment type: Premium purchase");
                                        log::info!("    • Premium data: {:?}", premium);
                                    }
                                    TransactionPartnerUserKind::BusinessAccountTransfer => {
                                        log::info!("    • Payment type: Business account transfer");
                                    }
                                }
                            }
                            TransactionPartner::Fragment(fragment) => {
                                log::info!("  • Type: Fragment withdrawal");
                                log::info!("  • Details: {:?}", fragment);
                            }
                            TransactionPartner::TelegramAds => {
                                log::info!("  • Type: Telegram Ads payment");
                            }
                            TransactionPartner::TelegramApi(_) => {
                                log::info!("  • Type: Telegram API service");
                            }
                            TransactionPartner::Chat(chat) => {
                                log::info!("  • Type: Chat transaction");
                                log::info!("  • Details: {:?}", chat);
                            }
                            TransactionPartner::AffiliateProgram(program) => {
                                log::info!("  • Type: Affiliate program");
                                log::info!("  • Details: {:?}", program);
                            }
                            TransactionPartner::Other => {
                                log::info!("  • Type: Other");
                            }
                        }
                    } else {
                        log::info!("  • No source information");
                    }

                    log::info!("");

                    // Parse Receiver (where the payment goes)
                    log::info!("📤 Receiver (payment to):");
                    if let Some(receiver) = &transaction.receiver {
                        log::info!("  • Receiver data: {:?}", receiver);
                    } else {
                        log::info!("  • None (incoming payment to bot)");
                    }

                    log::info!("");
                    log::info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
                    log::info!("📦 Full transaction data (raw debug):");
                    log::info!("{:#?}", transaction);
                } else {
                    log::warn!("⚠️ No matching transaction found for charge_id: {}", charge_id);
                    log::info!("📝 First 5 available transactions:");
                    for (idx, t) in star_transactions.transactions.iter().take(5).enumerate() {
                        log::info!(
                            "  Transaction #{}: ID={}, Amount={} Stars, Date={}",
                            idx + 1,
                            t.id.0,
                            t.amount,
                            t.date.format("%Y-%m-%d %H:%M:%S")
                        );
                    }
                }
            }
            Err(e) => {
                log::error!("❌ Failed to fetch star transactions: {:?}", e);
            }
        }
    } else {
        log::info!("ℹ️  No charge_id in database - user has no active subscription");
    }

    log::info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

    let current_plan = subscription_plan;
    let limits = PlanLimits::for_plan(current_plan);

    // Build the message text
    let plan_emoji = current_plan.emoji();

    let plan_name = current_plan.display_name();

    let mut text = "💳 *Subscription Info*\n\n".to_string();
    text.push_str(&format!("📊 *Your current plan:* {} {}\n", plan_emoji, plan_name));

    // Show subscription expiry date
    if let Some(expires_at) = &subscription_expires_at {
        // Format the date for display (from "2025-12-03 01:29:24" to "03.12.2025")
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
        text.push_str(&format!("📅 *Valid until:* {}\n\n", formatted_date));
    } else {
        text.push_str("📅 *Valid until:* unlimited\n\n");
    }

    text.push_str("━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n\n");
    text.push_str("*Your limits:*\n");
    text.push_str(&format!(
        "⏱️ Interval between requests: {} sec\n",
        limits.rate_limit_seconds
    ));

    if let Some(limit) = limits.daily_download_limit {
        text.push_str(&format!("📥 Daily download limit: {}\n", limit));
    } else {
        text.push_str("📥 Daily download limit: unlimited\n");
    }

    text.push_str(&format!("📦 Maximum file size: {} MB\n", limits.max_file_size_mb));

    if limits.can_choose_video_quality {
        text.push_str("🎬 Video quality selection: ✅\n");
    } else {
        text.push_str("🎬 Video quality selection: ❌\n");
    }

    if limits.can_choose_audio_bitrate {
        text.push_str("🎵 Audio bitrate selection: ✅\n");
    } else {
        text.push_str("🎵 Audio bitrate selection: ❌\n");
    }

    text.push_str("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n\n");

    // Check whether an active subscription exists
    let has_subscription = is_subscription_active;

    if !has_subscription {
        text.push_str("*Available plans:*\n\n");

        // Premium plan
        text.push_str("⭐ *Premium* \\- 350 Stars \\(~$6\\) every 30 days\n");
        text.push_str("• 10 seconds between requests\n");
        text.push_str("• Unlimited downloads\n");
        text.push_str("• Files up to 100 MB\n");
        text.push_str("• All formats \\+ quality selection\n");
        text.push_str("• Priority queue\n\n");

        // VIP plan
        text.push_str("👑 *VIP* \\- 850 Stars \\(~$15\\) every 30 days\n");
        text.push_str("• 5 seconds between requests\n");
        text.push_str("• Unlimited downloads\n");
        text.push_str("• Files up to 200 MB\n");
        text.push_str("• All formats \\+ quality selection\n");
        text.push_str("• Maximum priority\n");
        text.push_str("• Playlists up to 100 tracks\n");
        text.push_str("• Voice commands\n\n");

        text.push_str("💫 *Subscription with auto-renewal*\n");
        text.push_str("Charged automatically every 30 days\\.\n");
        text.push_str("Can be cancelled at any time\\!\n");
    } else {
        text.push_str("✅ *You have an active subscription\\!*\n\n");
        text.push_str("Subscription renews automatically every 30 days\\.\n");
        text.push_str("Can be cancelled at any time without losing the current period\\.\n");
    }

    // Build keyboard depending on subscription status
    let mut keyboard_rows = Vec::new();

    if !has_subscription {
        // Show subscription buttons only if there is no active subscription
        keyboard_rows.push(vec![crate::telegram::cb("⭐ Premium".to_string(), "subscribe:premium")]);
        keyboard_rows.push(vec![crate::telegram::cb("👑 VIP".to_string(), "subscribe:vip")]);
    } else if subscription_is_recurring && subscription_charge_id.is_some() {
        // Show cancel auto-renewal button only for recurring subscriptions
        keyboard_rows.push(vec![crate::telegram::cb(
            "❌ Cancel auto-renewal".to_string(),
            "subscription:cancel",
        )]);
    }

    keyboard_rows.push(vec![crate::telegram::cb("🔙 Back".to_string(), "back:main")]);

    let keyboard = InlineKeyboardMarkup::new(keyboard_rows);

    bot.send_message(chat_id, text)
        .parse_mode(teloxide::types::ParseMode::MarkdownV2)
        .reply_markup(keyboard)
        .await
}

/// Creates a subscription payment invoice via Telegram Stars
///
/// Creates a recurring invoice with automatic monthly Star charges.
/// Telegram will automatically charge the specified amount every 30 days.
pub async fn create_subscription_invoice(bot: &Bot, chat_id: ChatId, plan: &str) -> ResponseResult<Message> {
    log::info!(
        "🎯 create_subscription_invoice called for chat_id: {}, plan: {}",
        chat_id.0,
        plan
    );

    let (title, description, price_stars) = match plan {
        "premium" => {
            let price = *crate::core::config::subscription::PREMIUM_PRICE_STARS;
            (
                "⭐ Premium plan",
                format!(
                    "Premium subscription with auto-renewal every 30 days\n\n• 10 seconds between requests\n• Unlimited downloads\n• Files up to 100 MB\n• All formats + quality selection\n• Priority queue\n\n💫 Automatic charge of {} Star{} every 30 days",
                    price,
                    if price == 1 { "" } else { "s" }
                ),
                price,
            )
        }
        "vip" => {
            let price = *crate::core::config::subscription::VIP_PRICE_STARS;
            (
                "👑 VIP plan",
                format!(
                    "VIP subscription with auto-renewal every 30 days\n\n• 5 seconds between requests\n• Unlimited downloads\n• Files up to 200 MB\n• All formats + quality selection\n• Maximum priority\n• Playlists up to 100 tracks\n\n💫 Automatic charge of {} Stars every 30 days",
                    price
                ),
                price,
            )
        }
        _ => {
            log::error!("❌ Invalid plan requested: {}", plan);
            return Err(RequestError::from(std::sync::Arc::new(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Invalid plan",
            ))));
        }
    };

    // Create payload to identify the payment
    let payload = format!("subscription:{}:{}", plan, chat_id.0);
    log::info!("📦 Invoice payload: {}", payload);

    // Create invoice with subscription support
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

    // Create invoice link with subscription_period
    let invoice_link_result = bot
        .create_invoice_link(
            title,
            description.clone(),
            payload,
            "XTR".to_string(), // Only XTR (Telegram Stars) for subscriptions
            vec![LabeledPrice::new(
                format!(
                    "{} subscription",
                    if plan == "premium" { "Premium" } else { "VIP" }
                ),
                price_stars, // Price in Stars
            )],
        )
        .subscription_period(Seconds::from_seconds(crate::core::config::subscription::SUBSCRIPTION_PERIOD_SECONDS)) // 30 days in seconds - AUTO-RENEWAL EVERY 30 DAYS
        .await;

    match invoice_link_result {
        Ok(invoice_link) => {
            log::info!("✅ Invoice link created successfully: {}", invoice_link);

            // Track invoice creation for conversion funnel
            metrics::PAYMENT_CHECKOUT_STARTED.with_label_values(&[plan]).inc();

            // Send the link to the user with an inline button
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
                    "💳 Pay {} ({}⭐)",
                    if plan == "premium" { "Premium" } else { "VIP" },
                    price_stars
                ),
                invoice_url,
            )]]);

            // Escape all MarkdownV2 special characters
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
                    "💫 *{} Subscription*\n\n{}\n\n✨ Tap the button below to pay:",
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

/// Activates a subscription for a user
pub async fn activate_subscription(
    db_pool: Arc<DbPool>,
    telegram_id: i64,
    plan: &str,
    days: i32,
) -> Result<(), String> {
    let conn = db::get_connection(&db_pool).map_err(|e| format!("Failed to get connection: {}", e))?;

    // Update the user's plan with an expiry date
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

/// Handles a successful payment and activates/renews a subscription
///
/// # Arguments
///
/// * `bot` - Telegram bot instance
/// * `msg` - Message containing payment information
/// * `db_pool` - Database connection pool
///
/// # Returns
///
/// Returns `ResponseResult<()>` or an error if processing fails.
pub async fn handle_successful_payment(
    bot: &Bot,
    msg: &teloxide::types::Message,
    db_pool: Arc<DbPool>,
) -> ResponseResult<()> {
    if let Some(payment) = msg.successful_payment() {
        log::info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        log::info!("💳 SUCCESSFUL PAYMENT EVENT");
        log::info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        log::info!("Full payment object: {:?}", payment);
        log::info!("");
        log::info!("Payment breakdown:");
        log::info!("  • Currency: {}", payment.currency);
        log::info!("  • Total amount: {}", payment.total_amount);
        log::info!("  • Invoice payload: {}", payment.invoice_payload);
        log::info!(
            "  • Telegram payment charge ID: {}",
            payment.telegram_payment_charge_id.0
        );
        log::info!(
            "  • Provider payment charge ID: {:?}",
            payment.provider_payment_charge_id
        );
        log::info!("");
        log::info!("Subscription details:");
        log::info!("  • is_recurring: {}", payment.is_recurring);
        log::info!("  • is_first_recurring: {}", payment.is_first_recurring);
        log::info!(
            "  • subscription_expiration_date: {:?}",
            payment.subscription_expiration_date
        );
        log::info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

        // Parse payload: "subscription:premium:12345678"
        let parts: Vec<&str> = payment.invoice_payload.split(':').collect();
        if parts.len() == 3 && parts[0] == "subscription" {
            let plan = parts[1];
            let telegram_id = parts[2].parse::<i64>().unwrap_or(0);

            if telegram_id == 0 {
                log::error!("Invalid telegram_id in payment payload: {}", payment.invoice_payload);
                return Ok(());
            }

            let chat_id = msg.chat.id;

            // Process the subscription payment
            log::info!(
                "Processing subscription payment for user {}, plan: {}",
                telegram_id,
                plan
            );

            // Get database connection
            let conn = db::get_connection(&db_pool)
                .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

            // Save charge_id from payment (convert to string)
            let charge_id_str = payment.telegram_payment_charge_id.0.clone();

            // Determine subscription parameters
            let is_recurring = payment.is_recurring;
            let is_first_recurring = payment.is_first_recurring;

            // Get subscription expiry date from payment or compute it
            let subscription_expires_at = if let Some(expiration_date) = payment.subscription_expiration_date {
                // Telegram sends DateTime<Utc> directly, just format it
                expiration_date.format("%Y-%m-%d %H:%M:%S").to_string()
            } else {
                // If no expiration_date, use 30 days from now
                let dt = chrono::Utc::now() + chrono::Duration::days(30);
                dt.format("%Y-%m-%d %H:%M:%S").to_string()
            };

            log::info!("");
            log::info!("📊 Processing subscription:");
            log::info!("  • User ID: {}", telegram_id);
            log::info!("  • Plan: {}", plan);
            log::info!("  • Charge ID: {}", charge_id_str);
            log::info!("  • Expires at: {}", subscription_expires_at);
            log::info!("  • Is recurring: {}", is_recurring);
            log::info!("  • Is first recurring: {}", is_first_recurring);

            // Save payment (charge) information to DB for accounting
            log::info!("💾 Saving charge data for accounting...");
            if let Err(e) = db::save_charge(
                &conn,
                telegram_id,
                plan,
                &charge_id_str,
                Some(&payment.provider_payment_charge_id),
                &payment.currency,
                payment.total_amount as i64,
                &payment.invoice_payload,
                is_recurring,
                is_first_recurring,
                Some(&subscription_expires_at),
            ) {
                log::error!("❌ Failed to save charge data: {}", e);
                // Continue execution as this is not a critical error
            } else {
                log::info!("✅ Charge data saved successfully");
            }

            // Track payment success metrics
            metrics::record_payment_success(plan, is_recurring);
            metrics::record_revenue(plan, payment.total_amount as f64);

            // Track new subscription or renewal
            if is_first_recurring {
                let is_recurring_str = if is_recurring { "true" } else { "false" };
                metrics::NEW_SUBSCRIPTIONS_TOTAL
                    .with_label_values(&[plan, is_recurring_str])
                    .inc();
            }

            // Update subscription data in the DB
            log::info!("💾 Updating subscription data in database...");
            if let Err(e) = db::update_subscription_data(
                &conn,
                telegram_id,
                plan,
                &charge_id_str,
                &subscription_expires_at,
                is_recurring,
            ) {
                log::error!("❌ Failed to update subscription data: {}", e);

                // Track payment failure (database error)
                metrics::record_payment_failure(plan, "database_error");

                crate::telegram::notifications::notify_admin_text(
                    bot,
                    &format!(
                        "PAYMENT FAILURE (db update)\nuser_id: {}\nplan: {}\ncharge_id: {}\nerror: {}",
                        telegram_id, plan, charge_id_str, e
                    ),
                )
                .await;

                bot.send_message(
                    chat_id,
                    "❌ An error occurred while activating the subscription. Please contact the administrator.",
                )
                .await?;
                return Ok(());
            }
            log::info!("✅ Subscription data updated successfully");

            // Determine subscription type for the message
            let subscription_type_msg = if is_recurring {
                if is_first_recurring {
                    log::info!("🔄 Subscription type: NEW recurring subscription (first payment)");
                    "subscription with auto-renewal every 30 days"
                } else {
                    log::info!("🔄 Subscription type: RENEWAL of recurring subscription");
                    "subscription renewal"
                }
            } else {
                log::info!("💳 Subscription type: ONE-TIME payment (no auto-renewal)");
                "one-time subscription for 30 days"
            };

            let plan_emoji = if plan == "premium" { "⭐" } else { "👑" };
            let plan_name = if plan == "premium" { "Premium" } else { "VIP" };

            let renewal_info = if is_recurring {
                format!(
                    "🔄 Auto-renewal enabled\\.\nNext charge: {}",
                    subscription_expires_at.replace("-", "\\-").replace(":", "\\:")
                )
            } else {
                format!(
                    "📅 Valid until: {}",
                    subscription_expires_at.replace("-", "\\-").replace(":", "\\:")
                )
            };

            log::info!("📤 Sending confirmation message to user...");
            bot.send_message(
                chat_id,
                format!(
                    "✅ {} {} plan successfully activated\\!\n\n\
                    Type: {}\n\
                    {}\n\n\
                    Your new capabilities:\n\
                    • Rate limit: {} sec\n\
                    • Max\\. size: {} MB\n\
                    • {} quality selection\n\n\
                    Enjoy\\! 🎉",
                    plan_emoji,
                    plan_name,
                    subscription_type_msg.replace("-", "\\-"),
                    renewal_info,
                    if plan == "premium" { "10" } else { "5" },
                    if plan == "premium" { "100" } else { "200" },
                    if plan == "premium" { "✅" } else { "✅✅" }
                ),
            )
            .parse_mode(teloxide::types::ParseMode::MarkdownV2)
            .await?;

            log::info!("✅ Payment processed successfully");
            log::info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        } else {
            log::warn!("⚠️ Invalid payment payload format: {}", payment.invoice_payload);
        }
    }

    Ok(())
}

/// Cancels a user's subscription (bot-side)
///
/// # Arguments
///
/// * `bot` - Telegram bot instance
/// * `telegram_id` - User's Telegram ID
/// * `db_pool` - Database connection pool
///
/// # Returns
///
/// Returns `Result<(), String>` or an error if cancellation fails.
pub async fn cancel_subscription(bot: &Bot, telegram_id: i64, db_pool: Arc<DbPool>) -> Result<(), String> {
    log::info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    log::info!("🚫 SUBSCRIPTION CANCELLATION REQUEST");
    log::info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    log::info!("  • User ID: {}", telegram_id);

    let conn = db::get_connection(&db_pool).map_err(|e| {
        log::error!("❌ Failed to get database connection: {}", e);
        format!("Failed to get connection: {}", e)
    })?;

    // Get the user's charge_id
    log::info!("📋 Fetching user data...");
    let user = db::get_user(&conn, telegram_id)
        .map_err(|e| {
            log::error!("❌ Failed to get user: {}", e);
            format!("Failed to get user: {}", e)
        })?
        .ok_or_else(|| {
            log::error!("❌ User not found");
            "User not found".to_string()
        })?;

    log::info!("  • Current plan: {}", user.plan);
    log::info!("  • Is recurring: {}", user.is_recurring);
    log::info!("  • Expires at: {:?}", user.subscription_expires_at);

    // Check if subscription is already non-recurring
    if !user.is_recurring {
        log::info!("ℹ️  Subscription is already non-recurring (no auto-renewal)");
        log::info!("ℹ️  User retains access until: {:?}", user.subscription_expires_at);
        log::info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        return Err("Subscription is already non-recurring".to_string());
    }

    let charge_id = user.telegram_charge_id.ok_or_else(|| {
        log::error!("❌ No active subscription found");
        "No active subscription found".to_string()
    })?;

    log::info!("  • Charge ID: {}", charge_id);

    // Cancel subscription via Bot API
    log::info!("🔄 Calling Telegram Bot API to cancel subscription...");
    use teloxide::types::TelegramTransactionId;
    bot.edit_user_star_subscription(
        teloxide::types::UserId(telegram_id as u64),
        TelegramTransactionId(charge_id.clone()),
        true, // is_canceled = true
    )
    .await
    .map_err(|e| {
        log::error!("❌ Failed to cancel subscription via Bot API: {:?}", e);
        format!("Failed to cancel subscription: {:?}", e)
    })?;

    log::info!("✅ Subscription canceled via Telegram Bot API");

    // Track subscription cancellation
    metrics::SUBSCRIPTION_CANCELLATIONS_TOTAL
        .with_label_values(&[user.plan.as_str()])
        .inc();

    // Update the is_recurring flag in the DB (user retains access until expiry date)
    log::info!("💾 Updating database (removing recurring flag)...");
    db::cancel_subscription(&conn, telegram_id).map_err(|e| {
        log::error!("❌ Failed to update subscription status in DB: {}", e);
        format!("Failed to update subscription status: {}", e)
    })?;

    log::info!("✅ Subscription cancellation completed successfully");
    log::info!("ℹ️  User retains access until: {:?}", user.subscription_expires_at);
    log::info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

    Ok(())
}

/// Restores a user's subscription
///
/// # Arguments
///
/// * `bot` - Telegram bot instance
/// * `telegram_id` - User's Telegram ID
/// * `db_pool` - Database connection pool
///
/// # Returns
///
/// Returns `Result<(), String>` or an error if restoration fails.
pub async fn restore_subscription(bot: &Bot, telegram_id: i64, db_pool: Arc<DbPool>) -> Result<(), String> {
    let conn = db::get_connection(&db_pool).map_err(|e| format!("Failed to get connection: {}", e))?;

    // Get the user's charge_id
    let user = db::get_user(&conn, telegram_id)
        .map_err(|e| format!("Failed to get user: {}", e))?
        .ok_or_else(|| "User not found".to_string())?;

    let charge_id = user
        .telegram_charge_id
        .ok_or_else(|| "No subscription found".to_string())?;

    // Restore subscription via Bot API
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plan_limits_for_free() {
        let limits = PlanLimits::for_plan(Plan::Free);
        assert_eq!(limits.rate_limit_seconds, 30);
        assert_eq!(limits.daily_download_limit, Some(5));
        assert_eq!(limits.max_file_size_mb, 49);
        assert_eq!(limits.queue_priority, 0);
        assert!(!limits.can_choose_video_quality);
        assert!(!limits.can_choose_audio_bitrate);
        assert!(limits.can_upload_media);
        assert_eq!(limits.allowed_formats.len(), 2);
        assert!(limits.allowed_formats.contains(&"mp3".to_string()));
        assert!(limits.allowed_formats.contains(&"mp4".to_string()));
    }

    #[test]
    fn test_plan_limits_for_premium() {
        let limits = PlanLimits::for_plan(Plan::Premium);
        assert_eq!(limits.rate_limit_seconds, 10);
        assert_eq!(limits.daily_download_limit, None);
        assert_eq!(limits.max_file_size_mb, 100);
        assert_eq!(limits.queue_priority, 70);
        assert!(limits.can_choose_video_quality);
        assert!(limits.can_choose_audio_bitrate);
        assert!(limits.can_upload_media);
        assert_eq!(limits.allowed_formats.len(), 4);
    }

    #[test]
    fn test_plan_limits_for_vip() {
        let limits = PlanLimits::for_plan(Plan::Vip);
        assert_eq!(limits.rate_limit_seconds, 5);
        assert_eq!(limits.daily_download_limit, None);
        assert_eq!(limits.max_file_size_mb, 200);
        assert_eq!(limits.queue_priority, 100);
        assert!(limits.can_choose_video_quality);
        assert!(limits.can_choose_audio_bitrate);
        assert!(limits.can_upload_media);
        assert_eq!(limits.allowed_formats.len(), 4);
        assert!(limits.allowed_formats.contains(&"srt".to_string()));
        assert!(limits.allowed_formats.contains(&"txt".to_string()));
    }

    #[test]
    fn test_plan_limits_for_default_is_free() {
        let limits = PlanLimits::for_plan(Plan::default());
        assert_eq!(limits.rate_limit_seconds, 30);
        assert_eq!(limits.daily_download_limit, Some(5));
        assert_eq!(limits.max_file_size_mb, 49);
    }

    #[test]
    fn test_plan_limits_clone() {
        let limits = PlanLimits::for_plan(Plan::Premium);
        let cloned = limits.clone();
        assert_eq!(limits.rate_limit_seconds, cloned.rate_limit_seconds);
        assert_eq!(limits.max_file_size_mb, cloned.max_file_size_mb);
    }

    #[test]
    fn test_plan_limits_debug() {
        let limits = PlanLimits::for_plan(Plan::Vip);
        let debug_str = format!("{:?}", limits);
        assert!(debug_str.contains("PlanLimits"));
        assert!(debug_str.contains("rate_limit_seconds"));
        assert!(debug_str.contains("5"));
    }

    #[test]
    fn test_format_subscription_period_for_log_30_days() {
        let period = Seconds::from_seconds(2592000); // 30 days in seconds
        let formatted = format_subscription_period_for_log(&period);
        assert!(formatted.contains("2592000 seconds"));
        assert!(formatted.contains("30.00 days"));
        assert!(formatted.contains("1.00 months"));
    }

    #[test]
    fn test_format_subscription_period_for_log_1_day() {
        let period = Seconds::from_seconds(86400); // 1 day in seconds
        let formatted = format_subscription_period_for_log(&period);
        assert!(formatted.contains("86400 seconds"));
        assert!(formatted.contains("1.00 days"));
    }

    #[test]
    fn test_format_subscription_period_for_log_90_days() {
        let period = Seconds::from_seconds(7776000); // 90 days in seconds
        let formatted = format_subscription_period_for_log(&period);
        assert!(formatted.contains("7776000 seconds"));
        assert!(formatted.contains("90.00 days"));
        assert!(formatted.contains("3.00 months"));
    }

    #[test]
    fn test_format_subscription_period_for_log_zero() {
        let period = Seconds::from_seconds(0);
        let formatted = format_subscription_period_for_log(&period);
        assert!(formatted.contains("0 seconds"));
        assert!(formatted.contains("0.00 days"));
    }

    #[test]
    fn test_premium_vs_vip_rate_limits() {
        let premium = PlanLimits::for_plan(Plan::Premium);
        let vip = PlanLimits::for_plan(Plan::Vip);
        let free = PlanLimits::for_plan(Plan::Free);

        // VIP has lower rate limit than premium
        assert!(vip.rate_limit_seconds < premium.rate_limit_seconds);
        // Premium has lower rate limit than free
        assert!(premium.rate_limit_seconds < free.rate_limit_seconds);
    }

    #[test]
    fn test_premium_vs_vip_file_size() {
        let premium = PlanLimits::for_plan(Plan::Premium);
        let vip = PlanLimits::for_plan(Plan::Vip);
        let free = PlanLimits::for_plan(Plan::Free);

        // VIP has higher max file size than premium
        assert!(vip.max_file_size_mb > premium.max_file_size_mb);
        // Premium has higher max file size than free
        assert!(premium.max_file_size_mb > free.max_file_size_mb);
    }

    #[test]
    fn test_premium_vs_vip_queue_priority() {
        let premium = PlanLimits::for_plan(Plan::Premium);
        let vip = PlanLimits::for_plan(Plan::Vip);
        let free = PlanLimits::for_plan(Plan::Free);

        // VIP has highest priority
        assert_eq!(vip.queue_priority, 100);
        // Premium has medium priority
        assert!(premium.queue_priority > 0 && premium.queue_priority < 100);
        // Free has lowest priority
        assert_eq!(free.queue_priority, 0);
    }

    #[test]
    fn test_allowed_formats_subset() {
        let premium = PlanLimits::for_plan(Plan::Premium);
        let free = PlanLimits::for_plan(Plan::Free);

        // Free has fewer formats than premium
        assert!(free.allowed_formats.len() < premium.allowed_formats.len());

        // All free formats are in premium
        for format in &free.allowed_formats {
            assert!(premium.allowed_formats.contains(format));
        }
    }
}
