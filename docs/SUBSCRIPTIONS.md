# 💳 Subscription System and Referral Program

## 📋 Contents
1. [Subscription system](#subscription-system)
2. [Referral program](#referral-program)
3. [Telegram Stars integration](#telegram-stars-integration)
4. [Database schema](#database-schema)
5. [API and commands](#api-and-commands)
6. [Error handling](#error-handling)
7. [Background jobs](#background-jobs)
8. [Metrics](#metrics)
9. [Implementation roadmap](#implementation-roadmap)
10. [Notes](#notes)
11. [Resources](#resources)

---

## 💳 Subscription System

### Overview

The subscription system gives users paid tiers with expanded capabilities. Payments are handled through **Telegram Stars**, the in-app micro-payment system.

### Plans

#### 🌟 Free (no charge)

**Characteristics:**
- ⏱️ **Request interval:** 30 seconds
- 📥 **Daily download limit:** up to 5
- 📦 **Max file size:** up to 49 MB
- 🎵 **Formats:** basics (MP3, MP4)
- ⚙️ **Quality:** fixed (no selection)

**Usage:**
- Default for new users
- Assigned automatically at registration
- No payment required

---

#### ⭐ Premium (~₽299/month or equivalent in Stars)

**Price:** ~299 Telegram Stars (depends on exchange rate)

**Characteristics:**
- ⏱️ **Request interval:** 10 seconds
- 📥 **Downloads:** unlimited
- 📦 **Max file size:** up to 100 MB
- 🎵 **Formats:** all formats + quality selection
- 🎬 **Video quality:** best, 1080p, 720p, 480p, 360p
- 🎧 **Audio bitrate:** 128k, 192k, 256k, 320k
- ⚡ **Priority:** priority queue
- 📚 **History:** full access
- ⭐ **Favorites:** save tracks

**Benefits:**
- Lower wait time between requests
- Access to all formats and qualities
- Priority processing in the queue
- Download history preserved

---

#### 👑 VIP (~₽999/month or equivalent in Stars)

**Price:** ~999 Telegram Stars (depends on exchange rate)

**Characteristics:**
- ⏱️ **Request interval:** 5 seconds
- 📥 **Downloads:** unlimited
- 📦 **Max file size:** up to 200 MB
- 🎵 **Formats:** all formats + quality selection
- 🎬 **Video quality:** best, 1080p, 720p, 480p, 360p
- 🎧 **Audio bitrate:** 128k, 192k, 256k, 320k
- ⚡ **Priority:** highest queue priority
- 📚 **History:** full access
- ⭐ **Favorites:** save tracks
- 🎯 **Recommendations:** personalized suggestions based on history
- 📋 **Playlists:** playlists up to 100 tracks
- 🎤 **Voice commands:** control via voice messages

**Benefits:**
- Minimal wait time between requests
- Maximum file sizes
- Highest queue priority
- Extended functionality (playlists, voice commands)
- Early access to new features

---

### Subscription logic

#### Status check

```rust
// Subscription status pseudo-code
fn check_subscription_status(user: &User) -> SubscriptionStatus {
    if user.subscription_expires_at > now() {
        match user.plan {
            "premium" => SubscriptionStatus::Active(Plan::Premium),
            "vip" => SubscriptionStatus::Active(Plan::Vip),
            _ => SubscriptionStatus::Active(Plan::Free),
        }
    } else {
        // Subscription expired; revert to Free
        SubscriptionStatus::Expired
    }
}
```

#### Automatic plan downgrade/renewal

When a subscription expires:
1. Plan is automatically set to `free`.
2. Notify the user 3 days before expiration.
3. Notify the user on the day of expiration.
4. After expiration, send a renewal prompt.

#### Plan limits example

```rust
// Example limits structure
struct PlanLimits {
    rate_limit_seconds: u64,
    daily_download_limit: u32,
    max_file_size_mb: u32,
}
```

---

## 🤝 Referral Program

- Every user has a unique referral link: `https://t.me/your_bot?start=ref_<id>`
- Bonuses can be given in days of Premium for both referrer and friend.
- Protect against self-invites and duplicates.

**Suggested rewards:**
- Referrer: +1 day Premium per invite
- Referred user: +3 days Premium on first signup

---

## ⭐ Telegram Stars Integration

### Handling a successful payment

```rust
// Handling successful_payment
async fn handle_successful_payment(
    bot: &Bot,
    msg: &Message,
    db_pool: &DbPool,
) -> ResponseResult<()> {
    if let Some(payment) = &msg.successful_payment {
        let payload = &payment.invoice_payload;

        // Payload example: "subscription:premium:123456789"
        if payload.starts_with("subscription:") {
            let parts: Vec<&str> = payload.split(':').collect();
            if parts.len() == 3 {
                let plan = parts[1]; // "premium" or "vip"
                let user_id: i64 = parts[2].parse().unwrap_or(0);

                // Activate the subscription
                activate_subscription(db_pool, user_id, plan, 30).await?;

                // Send confirmation
                bot.send_message(
                    msg.chat.id,
                    format!("✅ Subscription {} activated for 30 days!", plan),
                )
                .await?;
            }
        }
    }

    Ok(())
}
```

### Currency conversion

**Important:** Telegram Stars have their own exchange rate.

1. **Determine the rate to local currency**
   - Use the official Telegram API or an exchange feed.
   - Refresh regularly (for example, daily).

2. **Dynamic pricing**
   ```rust
   // Conversion example
   fn convert_to_stars(price_rub: u32) -> u32 {
       // Approximate rate: 1 Star ≈ 1 RUB (must be updated)
       // Include Telegram fee (~5%)
       let stars = price_rub as f64 * 1.05;
       stars.ceil() as u32
   }
   ```

3. **Display to the user**
   - Show the price in Stars.
   - Optionally show an approximate local currency value.
   - Indicate subscription duration.

---

## 🗄️ Database Schema

### `users` table extensions

```sql
ALTER TABLE users ADD COLUMN subscription_expires_at DATETIME;
ALTER TABLE users ADD COLUMN subscription_starts_at DATETIME;
ALTER TABLE users ADD COLUMN subscription_auto_renew BOOLEAN DEFAULT 0;
ALTER TABLE users ADD COLUMN referral_code TEXT UNIQUE;
ALTER TABLE users ADD COLUMN referred_by INTEGER;
```

**Fields:**
- `subscription_expires_at` — expiration date (NULL for Free)
- `subscription_starts_at` — start date of the current subscription
- `subscription_auto_renew` — auto-renew flag (0/1)
- `referral_code` — unique referral code
- `referred_by` — Telegram ID of the inviter (NULL if none)

### `subscriptions` table

```sql
CREATE TABLE IF NOT EXISTS subscriptions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL,
    plan TEXT NOT NULL, -- 'free', 'premium', 'vip'
    starts_at DATETIME NOT NULL,
    expires_at DATETIME NOT NULL,
    payment_method TEXT, -- 'stars', 'other'
    payment_amount INTEGER, -- amount in Stars
    payment_transaction_id TEXT,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (user_id) REFERENCES users(telegram_id)
);

CREATE INDEX idx_subscriptions_user_id ON subscriptions(user_id);
CREATE INDEX idx_subscriptions_expires_at ON subscriptions(expires_at);
```

**Purpose:**
- History of all user subscriptions
- Payment tracking
- Auditing and analytics

### `referrals` table

```sql
CREATE TABLE IF NOT EXISTS referrals (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    referrer_id INTEGER NOT NULL, -- who invited
    referred_id INTEGER NOT NULL, -- who was invited
    bonus_applied BOOLEAN DEFAULT 0,
    referrer_bonus_days INTEGER DEFAULT 0,
    referred_bonus_days INTEGER DEFAULT 0,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (referrer_id) REFERENCES users(telegram_id),
    FOREIGN KEY (referred_id) REFERENCES users(telegram_id),
    UNIQUE(referrer_id, referred_id)
);

CREATE INDEX idx_referrals_referrer_id ON referrals(referrer_id);
CREATE INDEX idx_referrals_referred_id ON referrals(referred_id);
```

**Purpose:**
- Track referral relationships
- Record applied bonuses
- Prevent duplicates (UNIQUE constraint)

### `daily_downloads` table

```sql
CREATE TABLE IF NOT EXISTS daily_downloads (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL,
    download_date DATE NOT NULL,
    download_count INTEGER DEFAULT 0,
    FOREIGN KEY (user_id) REFERENCES users(telegram_id),
    UNIQUE(user_id, download_date)
);

CREATE INDEX idx_daily_downloads_user_date ON daily_downloads(user_id, download_date);
```

**Purpose:**
- Enforce the Free plan download limit
- Reset counters daily
- Check before each download

---

### DB helper queries

#### Check active subscription

```sql
SELECT plan, expires_at
FROM users
WHERE telegram_id = ?
  AND (subscription_expires_at IS NULL OR subscription_expires_at > datetime('now'));
```

#### Add days to a subscription

```sql
-- Extend current subscription (if active)
UPDATE users
SET subscription_expires_at = datetime(subscription_expires_at, '+' || ? || ' days')
WHERE telegram_id = ?
  AND subscription_expires_at > datetime('now');

-- Create a new subscription (if inactive)
UPDATE users
SET plan = ?,
    subscription_starts_at = datetime('now'),
    subscription_expires_at = datetime('now', '+' || ? || ' days')
WHERE telegram_id = ?
  AND (subscription_expires_at IS NULL OR subscription_expires_at <= datetime('now'));
```

#### Referral stats

```sql
-- Referral count
SELECT COUNT(*)
FROM referrals
WHERE referrer_id = ?;

-- Active referrals (with active subscription)
SELECT COUNT(DISTINCT r.referred_id)
FROM referrals r
JOIN users u ON r.referred_id = u.telegram_id
WHERE r.referrer_id = ?
  AND u.subscription_expires_at > datetime('now');
```

---

## 🔌 API and Commands

### `/subscribe` — purchase a subscription

**Description:** Shows available plans and lets the user pick one.

**Behavior:**
1. Display the current plan.
2. Show available plans (Premium, VIP).
3. Highlight benefits of each plan.
4. Render inline buttons to choose a plan.
5. When a plan is chosen, create an invoice for payment.

**Response example:**
```
💳 Doradura Subscriptions

📊 Your current plan: Free
📅 Valid until: unlimited

━━━━━━━━━━━━━━━━━━━━━━━━━━━━

⭐ Premium - 299 Stars/month
• 10 seconds between requests
• Unlimited downloads
• Files up to 100 MB
• All formats + quality selection
• Priority queue

━━━━━━━━━━━━━━━━━━━━━━━━━━━━

👑 VIP - 999 Stars/month
• 5 seconds between requests
• Unlimited downloads
• Files up to 200 MB
• All formats + quality selection
• Highest priority
• Playlists up to 100 tracks
• Voice commands

[⭐ Premium] [👑 VIP] [📊 Stats]
```

**Handler skeleton:**
```rust
async fn handle_subscribe_command(
    bot: &Bot,
    msg: &Message,
    db_pool: &DbPool,
) -> ResponseResult<()> {
    let user = get_user_from_db(db_pool, msg.chat.id.0).await?;
    send_subscription_menu(bot, msg.chat.id, &user).await?;
    Ok(())
}
```

### `/referral` — referral program

**Description:** Shows the referral link and referral statistics.

**Behavior:**
1. Generate or show the referral link.
2. Display number of invited friends.
3. Show earned bonuses.
4. Provide a quick copy button.

**Response example:**
```
🎁 Referral Program

📎 Your referral link:
https://t.me/your_bot?start=ref_123456789

📊 Stats:
👥 Invited friends: 5
⭐ Premium days earned: 5
🎯 Until next bonus: +1 friend

💡 How it works:
• Send the link to a friend
• They register via the link
• You get +1 day of Premium
• They get +3 days of Premium

[📋 Copy link] [📊 Detailed stats]
```

**Handler skeleton:**
```rust
async fn handle_referral_command(
    bot: &Bot,
    msg: &Message,
    db_pool: &DbPool,
) -> ResponseResult<()> {
    let user_id = msg.chat.id.0;

    let referral_code = get_or_create_referral_code(db_pool, user_id).await?;
    let referral_link = format!("https://t.me/your_bot?start=ref_{}", referral_code);

    let stats = get_referral_stats(db_pool, user_id).await?;

    send_referral_info(bot, msg.chat.id, &referral_link, &stats).await?;

    Ok(())
}
```

### Callback handlers

**Pick a subscription plan:**
```rust
// Callback: "subscribe:premium" or "subscribe:vip"
async fn handle_subscription_callback(
    bot: &Bot,
    callback: &CallbackQuery,
    plan: &str,
) -> ResponseResult<()> {
    let user_id = callback.from.id;
    create_subscription_invoice(bot, user_id, plan).await?;
    Ok(())
}
```

**Handle successful payment:**
```rust
async fn handle_payment(
    bot: &Bot,
    msg: &Message,
    db_pool: &DbPool,
) -> ResponseResult<()> {
    if let Some(payment) = &msg.successful_payment {
        activate_subscription_from_payment(db_pool, payment).await?;
        bot.send_message(
            msg.chat.id,
            "✅ Subscription activated! Thanks for the support! 🎉",
        )
        .await?;
    }

    Ok(())
}
```

---

## ⚠️ Error Handling

### Typical issues

#### Payment failure

**Scenario:** User cannot complete the payment.

**Response:**
```
❌ Failed to process payment

💡 Possible reasons:
• Not enough Stars
• Telegram payment service issues
• Invoice expired

🔄 What to do:
• Check your Stars balance
• Try again
• Contact support: @support_bot

[🔄 Retry] [💬 Support]
```

#### Subscription expiration

**Scenario:** A user subscription is expiring.

**Notifications:**
1. **3 days before expiration:**
   ```
   ⚠️ Your Premium subscription expires in 3 days

   📅 Expiration date: 2024-01-15

   To keep all benefits, renew now!

   [💳 Renew subscription]
   ```

2. **On the expiration day:**
   ```
   ⏰ Your Premium subscription has expired

   You were moved to the Free plan.
   Want to renew?

   [💳 Renew] [📊 Compare plans]
   ```

#### Referral program errors

**Scenario:** Referral link does not work or bonuses were not applied.

**Response:**
```
❌ Failed to apply referral bonus

💡 Possible reasons:
• User was already registered
• Technical error

🔄 What to do:
• Make sure the friend used the correct link
• Contact support: @support_bot

[💬 Support]
```

---

## 🔄 Background Jobs

### Subscription checks

**Frequency:** every 6 hours

```rust
async fn check_expiring_subscriptions(db_pool: &DbPool, bot: &Bot) {
    let expiring_soon = get_subscriptions_expiring_in(db_pool, 3).await;

    for subscription in expiring_soon {
        send_expiration_warning(bot, subscription.user_id, 3).await;
    }

    let expired = get_expired_subscriptions(db_pool).await;

    for subscription in expired {
        downgrade_to_free(db_pool, subscription.user_id).await;
        send_expiration_notice(bot, subscription.user_id).await;
    }
}
```

### Reset daily limits

**Frequency:** daily at 00:00 UTC

```rust
async fn reset_daily_limits(db_pool: &DbPool) {
    reset_daily_download_counters(db_pool).await;
}
```

### Apply referral bonuses

**Frequency:** on each registration via referral link

```rust
async fn apply_referral_bonuses(
    db_pool: &DbPool,
    referrer_id: i64,
    referred_id: i64,
) -> Result<()> {
    if is_new_user(db_pool, referred_id).await? {
        create_referral_record(db_pool, referrer_id, referred_id).await?;
        add_premium_days(db_pool, referrer_id, 1).await?; // +1 day to referrer
        add_premium_days(db_pool, referred_id, 3).await?; // +3 days to friend
        notify_referral_bonus(bot, referrer_id, referred_id).await?;
    }

    Ok(())
}
```

---

## 📊 Metrics

### Tracked metrics

1. **Subscription conversion:**
   - `/subscribe` views
   - Invoices created
   - Successful payments
   - Conversion = payments / views

2. **Referral program:**
   - Number of referral links
   - Registrations via links
   - Average referrals per user
   - Referral-to-paying conversion

3. **Subscriber retention:**
   - Renewal percentage
   - Average subscription duration
   - Churn rate

4. **Monetization:**
   - Total revenue in Stars
   - Average check
   - User lifetime value (LTV)

---

## 🚀 Implementation Roadmap

### Phase 1: Database preparation
- [ ] Create migrations for new tables
- [ ] Add fields to `users`
- [ ] Add performance indexes

### Phase 2: Core subscription features
- [ ] Implement `/subscribe`
- [ ] Integrate Telegram Stars API
- [ ] Handle payments
- [ ] Add subscription checks in the rate limiter

### Phase 3: Referral program
- [ ] Implement `/referral`
- [ ] Generate referral links
- [ ] Handle registrations via links
- [ ] Apply bonuses

### Phase 4: Automation
- [ ] Check expiring subscriptions
- [ ] Set up notifications
- [ ] Reset daily limits

### Phase 5: Testing
- [ ] Payment flow with Stars
- [ ] Referral program
- [ ] Background jobs
- [ ] Load testing

---

## 📝 Notes

### Currency conversion
- Use up-to-date rates when creating invoices.
- Refresh rates regularly (at least daily).
- Include the Telegram fee when calculating price.

### Security
- Verify payment authenticity via Telegram API.
- Validate all incoming data.
- Protect against SQL injection (parameterized queries).
- Prevent self-invites in the referral program.

### Scaling
- Consider PostgreSQL for subscription data as traffic grows.
- Cache subscription statuses in Redis.
- Handle payments asynchronously via queues.

---

## 📚 Resources

- [Telegram Bot API — Payments](https://core.telegram.org/bots/api#payments)
- [Telegram Stars Documentation](https://core.telegram.org/bots/api#stars)
- [Telegram Bot API — Inline Keyboard](https://core.telegram.org/bots/api#inlinekeyboardmarkup)
