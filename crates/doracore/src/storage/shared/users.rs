use std::str::FromStr;

use anyhow::{anyhow, Context, Result};
use sqlx::Row;

use crate::core::types::Plan;
use crate::storage::db::{self, SentFile, User, UserCounts};

use super::SharedStorage;

impl SharedStorage {
    pub async fn get_user(&self, telegram_id: i64) -> Result<Option<User>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_user connection")?;
                db::get_user(&conn, telegram_id).context("sqlite get_user")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT
                        u.telegram_id,
                        u.username,
                        COALESCE(s.plan, u.plan) AS plan,
                        u.download_format,
                        u.download_subtitles,
                        u.video_quality,
                        u.audio_bitrate,
                        u.language,
                        u.send_as_document,
                        u.send_audio_as_document,
                        CAST(s.expires_at AS TEXT) AS subscription_expires_at,
                        s.telegram_charge_id,
                        COALESCE(s.is_recurring, 0) AS is_recurring,
                        COALESCE(u.burn_subtitles, 0) AS burn_subtitles,
                        COALESCE(u.progress_bar_style, 'classic') AS progress_bar_style,
                        COALESCE(u.is_blocked, 0) AS is_blocked,
                        COALESCE(u.experimental_features, 0) AS experimental_features
                     FROM users u
                     LEFT JOIN subscriptions s ON s.user_id = u.telegram_id
                     WHERE u.telegram_id = $1",
                )
                .bind(telegram_id)
                .fetch_optional(pg_pool)
                .await
                .context("postgres get_user")?;
                row.map(map_pg_user).transpose()
            }
        }
    }

    pub async fn get_user_counts(&self) -> Result<UserCounts> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_user_counts connection")?;
                db::get_user_counts(&conn).context("sqlite get_user_counts")
            }
            Self::Postgres { pg_pool, .. } => {
                let rows = sqlx::query(
                    "SELECT
                        COALESCE(s.plan, u.plan) AS plan,
                        COALESCE(u.is_blocked, 0) AS is_blocked,
                        COUNT(*) AS count
                     FROM users u
                     LEFT JOIN subscriptions s ON s.user_id = u.telegram_id
                     GROUP BY COALESCE(s.plan, u.plan), COALESCE(u.is_blocked, 0)",
                )
                .fetch_all(pg_pool)
                .await
                .context("postgres get_user_counts")?;

                let mut counts = UserCounts::default();
                for row in rows {
                    let plan: String = row.get("plan");
                    let blocked = row.get::<i32, _>("is_blocked") != 0;
                    let count = row.get::<i64, _>("count") as usize;
                    counts.total += count;
                    if blocked {
                        counts.blocked += count;
                    }
                    match plan.as_str() {
                        "premium" => counts.premium += count,
                        "vip" => counts.vip += count,
                        _ => counts.free += count,
                    }
                }
                Ok(counts)
            }
        }
    }

    pub async fn get_users_paginated(
        &self,
        filter: Option<&str>,
        offset: usize,
        limit: usize,
    ) -> Result<(Vec<User>, usize)> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_users_paginated connection")?;
                db::get_users_paginated(&conn, filter, offset, limit).context("sqlite get_users_paginated")
            }
            Self::Postgres { pg_pool, .. } => {
                let where_sql = match filter {
                    Some("free") => "WHERE COALESCE(s.plan, u.plan) = 'free'",
                    Some("premium") => "WHERE COALESCE(s.plan, u.plan) = 'premium'",
                    Some("vip") => "WHERE COALESCE(s.plan, u.plan) = 'vip'",
                    Some("blocked") => "WHERE COALESCE(u.is_blocked, 0) = 1",
                    _ => "",
                };

                let count_sql = format!(
                    "SELECT COUNT(*) AS count
                     FROM users u
                     LEFT JOIN subscriptions s ON s.user_id = u.telegram_id
                     {}",
                    where_sql
                );
                let total = sqlx::query(&count_sql)
                    .fetch_one(pg_pool)
                    .await
                    .context("postgres get_users_paginated count")?
                    .get::<i64, _>("count") as usize;

                let query_sql = format!(
                    "SELECT
                        u.telegram_id,
                        u.username,
                        COALESCE(s.plan, u.plan) AS plan,
                        u.download_format,
                        u.download_subtitles,
                        u.video_quality,
                        u.audio_bitrate,
                        u.language,
                        u.send_as_document,
                        u.send_audio_as_document,
                        CAST(s.expires_at AS TEXT) AS subscription_expires_at,
                        s.telegram_charge_id,
                        COALESCE(s.is_recurring, 0) AS is_recurring,
                        COALESCE(u.burn_subtitles, 0) AS burn_subtitles,
                        COALESCE(u.progress_bar_style, 'classic') AS progress_bar_style,
                        COALESCE(u.is_blocked, 0) AS is_blocked,
                        COALESCE(u.experimental_features, 0) AS experimental_features
                     FROM users u
                     LEFT JOIN subscriptions s ON s.user_id = u.telegram_id
                     {}
                     ORDER BY u.telegram_id
                     LIMIT $1 OFFSET $2",
                    where_sql
                );
                let rows = sqlx::query(&query_sql)
                    .bind(limit as i64)
                    .bind(offset as i64)
                    .fetch_all(pg_pool)
                    .await
                    .context("postgres get_users_paginated rows")?;
                let users = rows.into_iter().map(map_pg_user).collect::<Result<Vec<_>>>()?;
                Ok((users, total))
            }
        }
    }

    pub async fn search_users(&self, query: &str) -> Result<Vec<User>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite search_users connection")?;
                db::search_users(&conn, query).context("sqlite search_users")
            }
            Self::Postgres { pg_pool, .. } => {
                let pattern = format!("%{}%", query);
                let rows = sqlx::query(
                    "SELECT
                        u.telegram_id,
                        u.username,
                        COALESCE(s.plan, u.plan) AS plan,
                        u.download_format,
                        u.download_subtitles,
                        u.video_quality,
                        u.audio_bitrate,
                        u.language,
                        u.send_as_document,
                        u.send_audio_as_document,
                        CAST(s.expires_at AS TEXT) AS subscription_expires_at,
                        s.telegram_charge_id,
                        COALESCE(s.is_recurring, 0) AS is_recurring,
                        COALESCE(u.burn_subtitles, 0) AS burn_subtitles,
                        COALESCE(u.progress_bar_style, 'classic') AS progress_bar_style,
                        COALESCE(u.is_blocked, 0) AS is_blocked,
                        COALESCE(u.experimental_features, 0) AS experimental_features
                     FROM users u
                     LEFT JOIN subscriptions s ON s.user_id = u.telegram_id
                     WHERE CAST(u.telegram_id AS TEXT) LIKE $1
                        OR COALESCE(u.username, '') LIKE $1
                     ORDER BY u.telegram_id
                     LIMIT 20",
                )
                .bind(pattern)
                .fetch_all(pg_pool)
                .await
                .context("postgres search_users")?;
                rows.into_iter().map(map_pg_user).collect()
            }
        }
    }

    pub async fn get_all_users(&self) -> Result<Vec<User>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_all_users connection")?;
                db::get_all_users(&conn).context("sqlite get_all_users")
            }
            Self::Postgres { pg_pool, .. } => {
                let rows = sqlx::query(
                    "SELECT
                        u.telegram_id,
                        u.username,
                        COALESCE(s.plan, u.plan) AS plan,
                        u.download_format,
                        u.download_subtitles,
                        u.video_quality,
                        u.audio_bitrate,
                        u.language,
                        u.send_as_document,
                        u.send_audio_as_document,
                        CAST(s.expires_at AS TEXT) AS subscription_expires_at,
                        s.telegram_charge_id,
                        COALESCE(s.is_recurring, 0) AS is_recurring,
                        COALESCE(u.burn_subtitles, 0) AS burn_subtitles,
                        COALESCE(u.progress_bar_style, 'classic') AS progress_bar_style,
                        COALESCE(u.is_blocked, 0) AS is_blocked
                     FROM users u
                     LEFT JOIN subscriptions s ON s.user_id = u.telegram_id
                     ORDER BY u.telegram_id",
                )
                .fetch_all(pg_pool)
                .await
                .context("postgres get_all_users")?;
                rows.into_iter().map(map_pg_user).collect()
            }
        }
    }

    pub async fn get_sent_files(&self, limit: Option<i32>) -> Result<Vec<SentFile>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_sent_files connection")?;
                db::get_sent_files(&conn, limit).context("sqlite get_sent_files")
            }
            Self::Postgres { pg_pool, .. } => {
                let rows = sqlx::query(
                    "SELECT dh.id, dh.user_id, u.username, dh.url, dh.title, dh.format,
                            CAST(dh.downloaded_at AS TEXT) AS downloaded_at, dh.file_id,
                            dh.message_id, dh.chat_id
                     FROM download_history dh
                     LEFT JOIN users u ON u.telegram_id = dh.user_id
                     WHERE dh.file_id IS NOT NULL
                     ORDER BY dh.downloaded_at DESC
                     LIMIT $1",
                )
                .bind(i64::from(limit.unwrap_or(50)))
                .fetch_all(pg_pool)
                .await
                .context("postgres get_sent_files")?;
                rows.into_iter().map(map_pg_sent_file).collect()
            }
        }
    }

    pub async fn is_user_blocked(&self, telegram_id: i64) -> Result<bool> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite is_user_blocked connection")?;
                db::is_user_blocked(&conn, telegram_id).context("sqlite is_user_blocked")
            }
            Self::Postgres { pg_pool, .. } => {
                let blocked =
                    sqlx::query_scalar::<_, i32>("SELECT COALESCE(is_blocked, 0) FROM users WHERE telegram_id = $1")
                        .bind(telegram_id)
                        .fetch_optional(pg_pool)
                        .await
                        .context("postgres is_user_blocked")?
                        .unwrap_or(0);
                Ok(blocked != 0)
            }
        }
    }

    pub async fn set_user_blocked(&self, telegram_id: i64, blocked: bool) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite set_user_blocked connection")?;
                db::set_user_blocked(&conn, telegram_id, blocked).context("sqlite set_user_blocked")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query("UPDATE users SET is_blocked = $2, updated_at = NOW() WHERE telegram_id = $1")
                    .bind(telegram_id)
                    .bind(if blocked { 1 } else { 0 })
                    .execute(pg_pool)
                    .await
                    .context("postgres set_user_blocked")?;
                Ok(())
            }
        }
    }

    pub async fn create_user(&self, telegram_id: i64, username: Option<String>) -> Result<()> {
        self.create_user_with_language(telegram_id, username, None).await
    }

    pub async fn create_user_with_language(
        &self,
        telegram_id: i64,
        username: Option<String>,
        language: Option<&str>,
    ) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite create_user connection")?;
                if let Some(language) = language {
                    db::create_user_with_language(&conn, telegram_id, username, language)
                        .context("sqlite create_user_with_language")
                } else {
                    db::create_user(&conn, telegram_id, username).context("sqlite create_user")
                }
            }
            Self::Postgres { pg_pool, .. } => {
                let mut tx = pg_pool.begin().await.context("postgres create_user begin")?;
                sqlx::query(
                    "INSERT INTO users (
                        telegram_id, username, download_format, download_subtitles, video_quality,
                        audio_bitrate, language, send_as_document, send_audio_as_document
                     ) VALUES ($1, $2, 'mp3', 0, 'best', '320k', $3, 0, 0)
                     ON CONFLICT (telegram_id) DO NOTHING",
                )
                .bind(telegram_id)
                .bind(username)
                .bind(language.unwrap_or("en"))
                .execute(&mut *tx)
                .await
                .context("postgres create_user users insert")?;
                sqlx::query(
                    "INSERT INTO subscriptions (user_id, plan)
                     VALUES ($1, 'free')
                     ON CONFLICT (user_id) DO NOTHING",
                )
                .bind(telegram_id)
                .execute(&mut *tx)
                .await
                .context("postgres create_user subscriptions insert")?;
                tx.commit().await.context("postgres create_user commit")?;
                Ok(())
            }
        }
    }

    pub async fn log_request(&self, user_id: i64, request_text: &str) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite log_request connection")?;
                db::log_request(&conn, user_id, request_text).context("sqlite log_request")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query("INSERT INTO request_history (user_id, request_text) VALUES ($1, $2)")
                    .bind(user_id)
                    .bind(request_text)
                    .execute(pg_pool)
                    .await
                    .context("postgres log_request")?;
                Ok(())
            }
        }
    }
}

fn map_pg_user(row: sqlx::postgres::PgRow) -> Result<User> {
    let plan_raw: String = row.get("plan");
    let plan = Plan::from_str(plan_raw.as_str()).map_err(|err| anyhow!("parse user plan: {}", err))?;
    Ok(User {
        telegram_id: row.get("telegram_id"),
        username: row.get("username"),
        plan,
        download_format: row.get("download_format"),
        download_subtitles: row.get("download_subtitles"),
        video_quality: row.get("video_quality"),
        audio_bitrate: row.get("audio_bitrate"),
        language: row.get("language"),
        send_as_document: row.get("send_as_document"),
        send_audio_as_document: row.get("send_audio_as_document"),
        subscription_expires_at: row.get("subscription_expires_at"),
        telegram_charge_id: row.get("telegram_charge_id"),
        is_recurring: row.get::<i32, _>("is_recurring") != 0,
        burn_subtitles: row.get("burn_subtitles"),
        progress_bar_style: row.get("progress_bar_style"),
        is_blocked: row.get::<i32, _>("is_blocked") != 0,
        experimental_features: row.get("experimental_features"),
    })
}

fn map_pg_sent_file(row: sqlx::postgres::PgRow) -> Result<SentFile> {
    Ok(SentFile {
        id: row.get("id"),
        user_id: row.get("user_id"),
        username: row.get("username"),
        url: row.get("url"),
        title: row.get("title"),
        format: row.get("format"),
        downloaded_at: row.get("downloaded_at"),
        file_id: row.get("file_id"),
        message_id: row.get("message_id"),
        chat_id: row.get("chat_id"),
    })
}
