//! Subscription, charge, and payment management operations.

use super::DbConnection;
use crate::core::types::Plan;
use rusqlite::Result;

/// Structure containing user subscription data.
#[derive(Debug, Clone)]
pub struct Subscription {
    pub user_id: i64,
    pub plan: Plan,
    pub expires_at: Option<String>,
    pub telegram_charge_id: Option<String>,
    pub is_recurring: bool,
}

/// Structure containing payment (charge) data from Telegram Stars.
/// Stores complete payment information for accounting purposes.
#[derive(Debug, Clone)]
pub struct Charge {
    pub id: i64,
    pub user_id: i64,
    pub plan: Plan,
    pub telegram_charge_id: String,
    pub provider_charge_id: Option<String>,
    pub currency: String,
    pub total_amount: i64,
    pub invoice_payload: String,
    pub is_recurring: bool,
    pub is_first_recurring: bool,
    pub subscription_expiration_date: Option<String>,
    pub payment_date: String,
    pub created_at: String,
}

/// Updates the telegram_charge_id of a user (used for subscription management)
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `telegram_id` - Telegram ID of the user
/// * `charge_id` - Telegram payment charge ID from a successful payment
///
/// # Returns
///
/// Returns `Ok(())` on success or a database error.
pub fn update_telegram_charge_id(conn: &DbConnection, telegram_id: i64, charge_id: Option<&str>) -> Result<()> {
    conn.execute(
        "UPDATE subscriptions SET telegram_charge_id = ?1, updated_at = CURRENT_TIMESTAMP WHERE user_id = ?2",
        [&charge_id as &dyn rusqlite::ToSql, &telegram_id as &dyn rusqlite::ToSql],
    )?;
    Ok(())
}

// ==================== Bot Assets ====================

// ==================== Subscription Management ====================

/// Gets the subscription record for a user from the subscriptions table.
pub fn get_subscription(conn: &DbConnection, telegram_id: i64) -> Result<Option<Subscription>> {
    let mut stmt = conn.prepare(
        "SELECT user_id, plan, expires_at, telegram_charge_id, is_recurring
         FROM subscriptions
         WHERE user_id = ?1",
    )?;
    let mut rows = stmt.query([&telegram_id as &dyn rusqlite::ToSql])?;

    if let Some(row) = rows.next()? {
        Ok(Some(Subscription {
            user_id: row.get(0)?,
            plan: row.get(1)?,
            expires_at: row.get::<_, Option<String>>(2)?,
            telegram_charge_id: row.get::<_, Option<String>>(3)?,
            is_recurring: row.get::<_, i32>(4).unwrap_or(0) != 0,
        }))
    } else {
        Ok(None)
    }
}

/// Updates the subscription data for a user after a successful payment.
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `telegram_id` - Telegram ID of the user
/// * `plan` - New user plan (e.g. "premium", "vip")
/// * `charge_id` - Telegram payment charge ID from a successful payment
/// * `subscription_expires_at` - Subscription expiry date (Unix timestamp or ISO 8601 string)
/// * `is_recurring` - Recurring subscription flag (auto-renewal)
///
/// # Returns
///
/// Returns `Ok(())` on success or a database error.
pub fn update_subscription_data(
    conn: &DbConnection,
    telegram_id: i64,
    plan: &str,
    charge_id: &str,
    subscription_expires_at: &str,
    is_recurring: bool,
) -> Result<()> {
    conn.execute(
        "INSERT INTO subscriptions (user_id, plan, expires_at, telegram_charge_id, is_recurring)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(user_id) DO UPDATE SET
            plan = excluded.plan,
            expires_at = excluded.expires_at,
            telegram_charge_id = excluded.telegram_charge_id,
            is_recurring = excluded.is_recurring,
            updated_at = CURRENT_TIMESTAMP",
        [
            &telegram_id as &dyn rusqlite::ToSql,
            &plan as &dyn rusqlite::ToSql,
            &subscription_expires_at as &dyn rusqlite::ToSql,
            &charge_id as &dyn rusqlite::ToSql,
            &(if is_recurring { 1 } else { 0 }) as &dyn rusqlite::ToSql,
        ],
    )?;
    conn.execute(
        "UPDATE users SET plan = ?1 WHERE telegram_id = ?2",
        [&plan as &dyn rusqlite::ToSql, &telegram_id as &dyn rusqlite::ToSql],
    )?;
    Ok(())
}

/// Checks whether the subscription for a user is active.
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `telegram_id` - Telegram ID of the user
///
/// # Returns
///
/// Returns `Ok(true)` if the subscription is active, `Ok(false)` if not or expired.
pub fn is_subscription_active(conn: &DbConnection, telegram_id: i64) -> Result<bool> {
    let subscription = get_subscription(conn, telegram_id)?;

    let Some(subscription) = subscription else {
        return Ok(false);
    };

    if subscription.plan == Plan::Free {
        return Ok(false);
    }

    if let Some(expires_at) = subscription.expires_at {
        let mut stmt = conn.prepare("SELECT datetime('now', 'utc') < datetime(?1)")?;
        let is_active: bool = stmt.query_row([&expires_at], |row| row.get(0))?;
        Ok(is_active)
    } else {
        Ok(true)
    }
}

/// Cancels a user's subscription (clears the is_recurring flag).
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `telegram_id` - Telegram ID of the user
///
/// # Returns
///
/// Returns `Ok(())` on success or a database error.
///
/// # Note
///
/// This function only removes the auto-renewal flag. The user retains
/// access until the subscription expiry date (subscription_expires_at).
pub fn cancel_subscription(conn: &DbConnection, telegram_id: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO subscriptions (user_id, plan, is_recurring)
         VALUES (?1, 'free', 0)
         ON CONFLICT(user_id) DO UPDATE SET
            is_recurring = 0,
            updated_at = CURRENT_TIMESTAMP",
        [&telegram_id as &dyn rusqlite::ToSql],
    )?;
    conn.execute(
        "UPDATE users SET plan = 'free' WHERE telegram_id = ?1",
        [&telegram_id as &dyn rusqlite::ToSql],
    )?;
    Ok(())
}

/// Saves payment (charge) information to the database.
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `user_id` - Telegram ID of the user
/// * `plan` - Subscription plan ("premium" or "vip")
/// * `telegram_charge_id` - Payment ID from Telegram
/// * `provider_charge_id` - Payment ID from provider (optional)
/// * `currency` - Payment currency (e.g. "XTR" for Stars)
/// * `total_amount` - Total payment amount
/// * `invoice_payload` - Invoice payload
/// * `is_recurring` - Recurring subscription flag
/// * `is_first_recurring` - Flag for first recurring payment
/// * `subscription_expiration_date` - Subscription expiry date
///
/// # Returns
///
/// Returns `Result<i64>` with the ID of the created record or an error.
pub fn save_charge(
    conn: &DbConnection,
    user_id: i64,
    plan: &str,
    telegram_charge_id: &str,
    provider_charge_id: Option<&str>,
    currency: &str,
    total_amount: i64,
    invoice_payload: &str,
    is_recurring: bool,
    is_first_recurring: bool,
    subscription_expiration_date: Option<&str>,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO charges (
            user_id, plan, telegram_charge_id, provider_charge_id, currency,
            total_amount, invoice_payload, is_recurring, is_first_recurring,
            subscription_expiration_date
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        rusqlite::params![
            user_id,
            plan,
            telegram_charge_id,
            provider_charge_id,
            currency,
            total_amount,
            invoice_payload,
            is_recurring as i32,
            is_first_recurring as i32,
            subscription_expiration_date,
        ],
    )?;

    Ok(conn.last_insert_rowid())
}

/// Gets all charges for a specific user.
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `user_id` - Telegram ID of the user
///
/// # Returns
///
/// Returns `Result<Vec<Charge>>` with a list of all user payments.
pub fn get_user_charges(conn: &DbConnection, user_id: i64) -> Result<Vec<Charge>> {
    let mut stmt = conn.prepare(
        "SELECT id, user_id, plan, telegram_charge_id, provider_charge_id, currency,
                total_amount, invoice_payload, is_recurring, is_first_recurring,
                subscription_expiration_date, payment_date, created_at
         FROM charges
         WHERE user_id = ?1
         ORDER BY payment_date DESC",
    )?;

    let charges = stmt.query_map([user_id], |row| {
        Ok(Charge {
            id: row.get(0)?,
            user_id: row.get(1)?,
            plan: row.get(2)?,
            telegram_charge_id: row.get(3)?,
            provider_charge_id: row.get(4)?,
            currency: row.get(5)?,
            total_amount: row.get(6)?,
            invoice_payload: row.get(7)?,
            is_recurring: row.get::<_, i32>(8)? != 0,
            is_first_recurring: row.get::<_, i32>(9)? != 0,
            subscription_expiration_date: row.get(10)?,
            payment_date: row.get(11)?,
            created_at: row.get(12)?,
        })
    })?;

    charges.collect()
}

/// Gets all charges from the database with optional filtering and pagination.
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `plan_filter` - Filter by plan (None = all plans)
/// * `limit` - Maximum number of records (None = all)
/// * `offset` - Offset for pagination
///
/// # Returns
///
/// Returns `Result<Vec<Charge>>` with a list of all payments.
pub fn get_all_charges(
    conn: &DbConnection,
    plan_filter: Option<&str>,
    limit: Option<i64>,
    offset: i64,
) -> Result<Vec<Charge>> {
    let query = if let Some(plan) = plan_filter {
        format!(
            "SELECT id, user_id, plan, telegram_charge_id, provider_charge_id, currency,
                    total_amount, invoice_payload, is_recurring, is_first_recurring,
                    subscription_expiration_date, payment_date, created_at
             FROM charges
             WHERE plan = '{}'
             ORDER BY payment_date DESC
             LIMIT {} OFFSET {}",
            plan,
            limit.unwrap_or(-1),
            offset
        )
    } else {
        format!(
            "SELECT id, user_id, plan, telegram_charge_id, provider_charge_id, currency,
                    total_amount, invoice_payload, is_recurring, is_first_recurring,
                    subscription_expiration_date, payment_date, created_at
             FROM charges
             ORDER BY payment_date DESC
             LIMIT {} OFFSET {}",
            limit.unwrap_or(-1),
            offset
        )
    };

    let mut stmt = conn.prepare(&query)?;

    let charges = stmt.query_map([], |row| {
        Ok(Charge {
            id: row.get(0)?,
            user_id: row.get(1)?,
            plan: row.get(2)?,
            telegram_charge_id: row.get(3)?,
            provider_charge_id: row.get(4)?,
            currency: row.get(5)?,
            total_amount: row.get(6)?,
            invoice_payload: row.get(7)?,
            is_recurring: row.get::<_, i32>(8)? != 0,
            is_first_recurring: row.get::<_, i32>(9)? != 0,
            subscription_expiration_date: row.get(10)?,
            payment_date: row.get(11)?,
            created_at: row.get(12)?,
        })
    })?;

    charges.collect()
}

/// Gets payment statistics.
///
/// # Arguments
///
/// * `conn` - Database connection
///
/// # Returns
///
/// Returns a tuple (total_charges, total_amount, premium_count, vip_count, recurring_count).
pub fn get_charges_stats(conn: &DbConnection) -> Result<(i64, i64, i64, i64, i64)> {
    let mut stmt = conn.prepare(
        "SELECT
            COUNT(*) as total_charges,
            SUM(total_amount) as total_amount,
            SUM(CASE WHEN plan = 'premium' THEN 1 ELSE 0 END) as premium_count,
            SUM(CASE WHEN plan = 'vip' THEN 1 ELSE 0 END) as vip_count,
            SUM(CASE WHEN is_recurring = 1 THEN 1 ELSE 0 END) as recurring_count
         FROM charges",
    )?;

    stmt.query_row([], |row| {
        Ok((
            row.get(0)?,
            row.get::<_, Option<i64>>(1)?.unwrap_or(0),
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
        ))
    })
}
