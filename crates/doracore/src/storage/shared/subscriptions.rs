use std::str::FromStr;

use anyhow::{Context, Result, anyhow};
use sqlx::Row;

use crate::core::types::Plan;
use crate::storage::db::{self, Charge};

use super::SharedStorage;

impl SharedStorage {
    pub async fn expire_old_subscriptions(&self) -> Result<usize> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite expire_old_subscriptions connection")?;
                db::expire_old_subscriptions(&conn).context("sqlite expire_old_subscriptions")
            }
            Self::Postgres { pg_pool, .. } => {
                let result = sqlx::query(
                    "UPDATE subscriptions
                     SET plan = 'free',
                         expires_at = NULL,
                         telegram_charge_id = NULL,
                         is_recurring = FALSE,
                         updated_at = NOW()
                     WHERE plan != 'free'
                       AND expires_at IS NOT NULL
                       AND expires_at <= NOW()",
                )
                .execute(pg_pool)
                .await
                .context("postgres expire_old_subscriptions")?;
                Ok(result.rows_affected() as usize)
            }
        }
    }

    pub async fn update_user_plan_with_expiry(&self, telegram_id: i64, plan: &str, days: Option<i32>) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite update_user_plan_with_expiry connection")?;
                db::update_user_plan_with_expiry(&conn, telegram_id, plan, days)
                    .context("sqlite update_user_plan_with_expiry")
            }
            Self::Postgres { pg_pool, .. } => {
                let expires_at = days.map(|days| chrono::Utc::now() + chrono::Duration::days(i64::from(days)));
                sqlx::query(
                    "INSERT INTO subscriptions (user_id, plan, expires_at, telegram_charge_id, is_recurring, updated_at)
                     VALUES ($1, $2, $3, NULL, 0, NOW())
                     ON CONFLICT (user_id) DO UPDATE SET
                        plan = EXCLUDED.plan,
                        expires_at = EXCLUDED.expires_at,
                        telegram_charge_id = NULL,
                        is_recurring = 0,
                        updated_at = NOW()",
                )
                .bind(telegram_id)
                .bind(plan)
                .bind(expires_at)
                .execute(pg_pool)
                .await
                .context("postgres update_user_plan_with_expiry subscriptions")?;
                sqlx::query("UPDATE users SET plan = $2, updated_at = NOW() WHERE telegram_id = $1")
                    .bind(telegram_id)
                    .bind(plan)
                    .execute(pg_pool)
                    .await
                    .context("postgres update_user_plan_with_expiry users")?;
                Ok(())
            }
        }
    }

    pub async fn save_charge(
        &self,
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
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite save_charge connection")?;
                db::save_charge(
                    &conn,
                    user_id,
                    plan,
                    telegram_charge_id,
                    provider_charge_id,
                    currency,
                    total_amount,
                    invoice_payload,
                    is_recurring,
                    is_first_recurring,
                    subscription_expiration_date,
                )
                .context("sqlite save_charge")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "INSERT INTO charges (
                        user_id, plan, telegram_charge_id, provider_charge_id, currency,
                        total_amount, invoice_payload, is_recurring, is_first_recurring,
                        subscription_expiration_date
                     ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
                     RETURNING id",
                )
                .bind(user_id)
                .bind(plan)
                .bind(telegram_charge_id)
                .bind(provider_charge_id)
                .bind(currency)
                .bind(total_amount)
                .bind(invoice_payload)
                .bind(i32::from(is_recurring))
                .bind(i32::from(is_first_recurring))
                .bind(subscription_expiration_date)
                .fetch_one(pg_pool)
                .await
                .context("postgres save_charge")?;
                Ok(row.get::<i64, _>("id"))
            }
        }
    }

    /// Atomically record a successful payment: insert charge row, upsert
    /// the subscription, and update users.plan — all in ONE transaction.
    ///
    /// This closes the CRITICAL race where `save_charge` succeeded but the
    /// follow-up `update_subscription_data` failed (DB blip, process kill),
    /// leaving the `charges` row persisted while the subscription stayed
    /// inactive. Because `charges.telegram_charge_id` has a UNIQUE constraint,
    /// retry was blocked — the user paid but never got access.
    ///
    /// The old two-call pattern is still available (`save_charge` +
    /// `update_subscription_data`) for admin flows, but primary payment
    /// processing MUST use this method.
    #[allow(clippy::too_many_arguments)]
    pub async fn record_successful_payment(
        &self,
        user_id: i64,
        plan: &str,
        telegram_charge_id: &str,
        provider_charge_id: Option<&str>,
        currency: &str,
        total_amount: i64,
        invoice_payload: &str,
        is_recurring: bool,
        is_first_recurring: bool,
        subscription_expires_at: &str,
    ) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let mut conn = db::get_connection(db_pool).context("sqlite record_payment connection")?;
                let tx = conn.transaction().context("begin sqlite record_payment tx")?;

                // 1. Insert charge row (fails on duplicate telegram_charge_id).
                tx.execute(
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
                        subscription_expires_at,
                    ],
                )
                .context("sqlite record_payment insert charge")?;

                // 2. Upsert subscription.
                tx.execute(
                    "INSERT INTO subscriptions (user_id, plan, expires_at, telegram_charge_id, is_recurring)
                     VALUES (?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT(user_id) DO UPDATE SET
                        plan = excluded.plan,
                        expires_at = excluded.expires_at,
                        telegram_charge_id = excluded.telegram_charge_id,
                        is_recurring = excluded.is_recurring,
                        updated_at = CURRENT_TIMESTAMP",
                    rusqlite::params![
                        user_id,
                        plan,
                        subscription_expires_at,
                        telegram_charge_id,
                        is_recurring as i32,
                    ],
                )
                .context("sqlite record_payment upsert subscription")?;

                // 3. Update users.plan.
                tx.execute(
                    "UPDATE users SET plan = ?1 WHERE telegram_id = ?2",
                    rusqlite::params![plan, user_id],
                )
                .context("sqlite record_payment update user plan")?;

                tx.commit().context("commit sqlite record_payment")?;
                Ok(())
            }
            Self::Postgres { pg_pool, .. } => {
                let mut tx = pg_pool.begin().await.context("begin pg record_payment")?;

                sqlx::query(
                    "INSERT INTO charges (
                        user_id, plan, telegram_charge_id, provider_charge_id, currency,
                        total_amount, invoice_payload, is_recurring, is_first_recurring,
                        subscription_expiration_date
                     ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
                )
                .bind(user_id)
                .bind(plan)
                .bind(telegram_charge_id)
                .bind(provider_charge_id)
                .bind(currency)
                .bind(total_amount)
                .bind(invoice_payload)
                .bind(i32::from(is_recurring))
                .bind(i32::from(is_first_recurring))
                .bind(subscription_expires_at)
                .execute(&mut *tx)
                .await
                .context("pg record_payment insert charge")?;

                sqlx::query(
                    "INSERT INTO subscriptions (user_id, plan, expires_at, telegram_charge_id, is_recurring, updated_at)
                     VALUES ($1, $2, $3, $4, $5, NOW())
                     ON CONFLICT (user_id) DO UPDATE SET
                        plan = EXCLUDED.plan,
                        expires_at = EXCLUDED.expires_at,
                        telegram_charge_id = EXCLUDED.telegram_charge_id,
                        is_recurring = EXCLUDED.is_recurring,
                        updated_at = NOW()",
                )
                .bind(user_id)
                .bind(plan)
                .bind(subscription_expires_at)
                .bind(telegram_charge_id)
                .bind(i32::from(is_recurring))
                .execute(&mut *tx)
                .await
                .context("pg record_payment upsert subscription")?;

                sqlx::query("UPDATE users SET plan = $2, updated_at = NOW() WHERE telegram_id = $1")
                    .bind(user_id)
                    .bind(plan)
                    .execute(&mut *tx)
                    .await
                    .context("pg record_payment update user plan")?;

                tx.commit().await.context("commit pg record_payment")?;
                Ok(())
            }
        }
    }

    /// Look up the user_id associated with a given Telegram charge_id.
    /// Used by the refund handler to identify whose subscription to revoke.
    pub async fn get_user_id_by_charge(&self, telegram_charge_id: &str) -> Result<Option<i64>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_user_id_by_charge connection")?;
                let result = conn
                    .query_row(
                        "SELECT user_id FROM charges WHERE telegram_charge_id = ?1",
                        rusqlite::params![telegram_charge_id],
                        |r| r.get::<_, i64>(0),
                    )
                    .ok();
                Ok(result)
            }
            Self::Postgres { pg_pool, .. } => {
                let result = sqlx::query_scalar::<_, i64>("SELECT user_id FROM charges WHERE telegram_charge_id = $1")
                    .bind(telegram_charge_id)
                    .fetch_optional(pg_pool)
                    .await
                    .context("pg get_user_id_by_charge")?;
                Ok(result)
            }
        }
    }

    /// Revoke a subscription in response to a refund. Marks subscription as
    /// free, disables is_recurring, and sets users.plan = free — all atomic.
    pub async fn revoke_subscription_for_refund(&self, user_id: i64, telegram_charge_id: &str) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let mut conn = db::get_connection(db_pool).context("sqlite revoke_refund connection")?;
                let tx = conn.transaction().context("begin revoke_refund tx")?;
                tx.execute(
                    "UPDATE subscriptions SET plan = 'free', is_recurring = 0, \
                        expires_at = datetime('now'), updated_at = datetime('now') \
                     WHERE user_id = ?1 AND telegram_charge_id = ?2",
                    rusqlite::params![user_id, telegram_charge_id],
                )
                .context("sqlite revoke_refund update subscription")?;
                tx.execute(
                    "UPDATE users SET plan = 'free' WHERE telegram_id = ?1",
                    rusqlite::params![user_id],
                )
                .context("sqlite revoke_refund update user")?;
                tx.commit().context("commit sqlite revoke_refund")?;
                Ok(())
            }
            Self::Postgres { pg_pool, .. } => {
                let mut tx = pg_pool.begin().await.context("begin pg revoke_refund")?;
                sqlx::query(
                    "UPDATE subscriptions SET plan = 'free', is_recurring = 0, \
                        expires_at = NOW(), updated_at = NOW() \
                     WHERE user_id = $1 AND telegram_charge_id = $2",
                )
                .bind(user_id)
                .bind(telegram_charge_id)
                .execute(&mut *tx)
                .await
                .context("pg revoke_refund update subscription")?;
                sqlx::query("UPDATE users SET plan = 'free', updated_at = NOW() WHERE telegram_id = $1")
                    .bind(user_id)
                    .execute(&mut *tx)
                    .await
                    .context("pg revoke_refund update user")?;
                tx.commit().await.context("commit pg revoke_refund")?;
                Ok(())
            }
        }
    }

    pub async fn get_user_charges(&self, user_id: i64) -> Result<Vec<Charge>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_user_charges connection")?;
                db::get_user_charges(&conn, user_id).context("sqlite get_user_charges")
            }
            Self::Postgres { pg_pool, .. } => {
                let rows = sqlx::query(
                    "SELECT id, user_id, plan, telegram_charge_id, provider_charge_id, currency,
                            total_amount, invoice_payload, is_recurring, is_first_recurring,
                            CAST(subscription_expiration_date AS TEXT) AS subscription_expiration_date,
                            CAST(payment_date AS TEXT) AS payment_date,
                            CAST(created_at AS TEXT) AS created_at
                     FROM charges
                     WHERE user_id = $1
                     ORDER BY payment_date DESC",
                )
                .bind(user_id)
                .fetch_all(pg_pool)
                .await
                .context("postgres get_user_charges")?;
                rows.into_iter().map(map_pg_charge).collect()
            }
        }
    }

    pub async fn get_all_charges(
        &self,
        plan_filter: Option<&str>,
        limit: Option<i64>,
        offset: i64,
    ) -> Result<Vec<Charge>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_all_charges connection")?;
                db::get_all_charges(&conn, plan_filter, limit, offset).context("sqlite get_all_charges")
            }
            Self::Postgres { pg_pool, .. } => {
                let rows = sqlx::query(
                    "SELECT id, user_id, plan, telegram_charge_id, provider_charge_id, currency,
                            total_amount, invoice_payload, is_recurring, is_first_recurring,
                            CAST(subscription_expiration_date AS TEXT) AS subscription_expiration_date,
                            CAST(payment_date AS TEXT) AS payment_date,
                            CAST(created_at AS TEXT) AS created_at
                     FROM charges
                     WHERE ($1::text IS NULL OR plan = $1)
                     ORDER BY payment_date DESC
                     LIMIT $2 OFFSET $3",
                )
                .bind(plan_filter)
                .bind(limit.unwrap_or(-1))
                .bind(offset)
                .fetch_all(pg_pool)
                .await
                .context("postgres get_all_charges")?;
                rows.into_iter().map(map_pg_charge).collect()
            }
        }
    }

    pub async fn get_charges_stats(&self) -> Result<(i64, i64, i64, i64, i64)> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_charges_stats connection")?;
                db::get_charges_stats(&conn).context("sqlite get_charges_stats")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT
                        COUNT(*)::bigint AS total_charges,
                        COALESCE(SUM(total_amount), 0)::bigint AS total_amount,
                        COALESCE(SUM(CASE WHEN plan = 'premium' THEN 1 ELSE 0 END), 0)::bigint AS premium_count,
                        COALESCE(SUM(CASE WHEN plan = 'vip' THEN 1 ELSE 0 END), 0)::bigint AS vip_count,
                        COALESCE(SUM(CASE WHEN is_recurring = 1 THEN 1 ELSE 0 END), 0)::bigint AS recurring_count
                     FROM charges",
                )
                .fetch_one(pg_pool)
                .await
                .context("postgres get_charges_stats")?;
                Ok((
                    row.get("total_charges"),
                    row.get("total_amount"),
                    row.get("premium_count"),
                    row.get("vip_count"),
                    row.get("recurring_count"),
                ))
            }
        }
    }

    pub async fn update_subscription_data(
        &self,
        telegram_id: i64,
        plan: &str,
        charge_id: &str,
        subscription_expires_at: &str,
        is_recurring: bool,
    ) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite update_subscription_data connection")?;
                db::update_subscription_data(
                    &conn,
                    telegram_id,
                    plan,
                    charge_id,
                    subscription_expires_at,
                    is_recurring,
                )
                .context("sqlite update_subscription_data")
            }
            Self::Postgres { pg_pool, .. } => {
                // Wrap in a transaction so subscriptions + users never diverge.
                // Previously these were two independent auto-commits; a failure
                // between them left users.plan stale relative to subscriptions.
                let mut tx = pg_pool.begin().await.context("begin pg update_subscription_data")?;
                sqlx::query(
                    "INSERT INTO subscriptions (user_id, plan, expires_at, telegram_charge_id, is_recurring, updated_at)
                     VALUES ($1, $2, $3, $4, $5, NOW())
                     ON CONFLICT (user_id) DO UPDATE SET
                        plan = EXCLUDED.plan,
                        expires_at = EXCLUDED.expires_at,
                        telegram_charge_id = EXCLUDED.telegram_charge_id,
                        is_recurring = EXCLUDED.is_recurring,
                        updated_at = NOW()",
                )
                .bind(telegram_id)
                .bind(plan)
                .bind(subscription_expires_at)
                .bind(charge_id)
                .bind(i32::from(is_recurring))
                .execute(&mut *tx)
                .await
                .context("postgres update_subscription_data subscriptions")?;
                sqlx::query("UPDATE users SET plan = $2, updated_at = NOW() WHERE telegram_id = $1")
                    .bind(telegram_id)
                    .bind(plan)
                    .execute(&mut *tx)
                    .await
                    .context("postgres update_subscription_data users")?;
                tx.commit().await.context("commit pg update_subscription_data")?;
                Ok(())
            }
        }
    }

    pub async fn cancel_subscription(&self, telegram_id: i64) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite cancel_subscription connection")?;
                db::cancel_subscription(&conn, telegram_id).context("sqlite cancel_subscription")
            }
            Self::Postgres { pg_pool, .. } => {
                // Wrap in a transaction — see update_subscription_data for rationale.
                let mut tx = pg_pool.begin().await.context("begin pg cancel_subscription")?;
                sqlx::query(
                    "INSERT INTO subscriptions (user_id, plan, is_recurring, updated_at)
                     VALUES ($1, 'free', 0, NOW())
                     ON CONFLICT (user_id) DO UPDATE SET
                        is_recurring = 0,
                        updated_at = NOW()",
                )
                .bind(telegram_id)
                .execute(&mut *tx)
                .await
                .context("postgres cancel_subscription subscriptions")?;
                sqlx::query("UPDATE users SET plan = 'free', updated_at = NOW() WHERE telegram_id = $1")
                    .bind(telegram_id)
                    .execute(&mut *tx)
                    .await
                    .context("postgres cancel_subscription users")?;
                tx.commit().await.context("commit pg cancel_subscription")?;
                Ok(())
            }
        }
    }

    pub async fn count_active_subscriptions(&self) -> Result<i64> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite count_active_subscriptions connection")?;
                let count = conn
                    .query_row(
                        "SELECT COUNT(*) FROM subscriptions WHERE expires_at > datetime('now')",
                        [],
                        |row| row.get(0),
                    )
                    .context("sqlite count_active_subscriptions")?;
                Ok(count)
            }
            Self::Postgres { pg_pool, .. } => {
                let count =
                    sqlx::query_scalar::<_, i64>("SELECT COUNT(*)::bigint FROM subscriptions WHERE expires_at > NOW()")
                        .fetch_one(pg_pool)
                        .await
                        .context("postgres count_active_subscriptions")?;
                Ok(count)
            }
        }
    }
}

fn map_pg_charge(row: sqlx::postgres::PgRow) -> Result<Charge> {
    let plan = Plan::from_str(&row.get::<String, _>("plan"))
        .map_err(|err| anyhow!("invalid charge plan in postgres: {err}"))?;

    Ok(Charge {
        id: row.get("id"),
        user_id: row.get("user_id"),
        plan,
        telegram_charge_id: row.get("telegram_charge_id"),
        provider_charge_id: row.get("provider_charge_id"),
        currency: row.get("currency"),
        total_amount: row.get("total_amount"),
        invoice_payload: row.get("invoice_payload"),
        is_recurring: row.get::<i32, _>("is_recurring") != 0,
        is_first_recurring: row.get::<i32, _>("is_first_recurring") != 0,
        subscription_expiration_date: row.get("subscription_expiration_date"),
        payment_date: row.get("payment_date"),
        created_at: row.get("created_at"),
    })
}
