use anyhow::{Context, Result};
use rusqlite::OptionalExtension;
use serde_json::Value as JsonValue;
use sqlx::Row;

use crate::storage::db::{self, DbConnection};

use super::SharedStorage;
use super::types::{ContentSourceGroup, ContentSubscriptionRecord};

impl SharedStorage {
    pub async fn get_user_content_subscriptions(&self, user_id: i64) -> Result<Vec<ContentSubscriptionRecord>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_user_content_subscriptions connection")?;
                sqlite_get_user_content_subscriptions(&conn, user_id)
                    .map_err(anyhow::Error::msg)
                    .context("sqlite get_user_content_subscriptions")
            }
            Self::Postgres { pg_pool, .. } => {
                let rows = sqlx::query(
                    "SELECT id, user_id, source_type, source_id, display_name, watch_mask, is_active,
                            last_seen_state, source_meta, CAST(last_checked_at AS TEXT) AS last_checked_at,
                            last_error, consecutive_errors, CAST(created_at AS TEXT) AS created_at,
                            CAST(updated_at AS TEXT) AS updated_at
                     FROM content_subscriptions
                     WHERE user_id = $1 AND is_active = 1
                     ORDER BY created_at ASC",
                )
                .bind(user_id)
                .fetch_all(pg_pool)
                .await
                .context("postgres get_user_content_subscriptions")?;
                rows.into_iter().map(map_pg_content_subscription).collect()
            }
        }
    }

    pub async fn count_user_content_subscriptions(&self, user_id: i64) -> Result<u32> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite count_user_content_subscriptions connection")?;
                sqlite_count_user_content_subscriptions(&conn, user_id)
                    .map_err(anyhow::Error::msg)
                    .context("sqlite count_user_content_subscriptions")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT COUNT(*)::BIGINT AS count
                     FROM content_subscriptions
                     WHERE user_id = $1 AND is_active = 1",
                )
                .bind(user_id)
                .fetch_one(pg_pool)
                .await
                .context("postgres count_user_content_subscriptions")?;
                Ok(row.get::<i64, _>("count") as u32)
            }
        }
    }

    pub async fn get_content_subscription(&self, id: i64) -> Result<Option<ContentSubscriptionRecord>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_content_subscription connection")?;
                sqlite_get_content_subscription(&conn, id)
                    .map_err(anyhow::Error::msg)
                    .context("sqlite get_content_subscription")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT id, user_id, source_type, source_id, display_name, watch_mask, is_active,
                            last_seen_state, source_meta, CAST(last_checked_at AS TEXT) AS last_checked_at,
                            last_error, consecutive_errors, CAST(created_at AS TEXT) AS created_at,
                            CAST(updated_at AS TEXT) AS updated_at
                     FROM content_subscriptions
                     WHERE id = $1",
                )
                .bind(id)
                .fetch_optional(pg_pool)
                .await
                .context("postgres get_content_subscription")?;
                row.map(map_pg_content_subscription).transpose()
            }
        }
    }

    pub async fn has_content_subscription(
        &self,
        user_id: i64,
        source_type: &str,
        source_id: &str,
    ) -> Result<Option<ContentSubscriptionRecord>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite has_content_subscription connection")?;
                sqlite_has_content_subscription(&conn, user_id, source_type, source_id)
                    .map_err(anyhow::Error::msg)
                    .context("sqlite has_content_subscription")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT id, user_id, source_type, source_id, display_name, watch_mask, is_active,
                            last_seen_state, source_meta, CAST(last_checked_at AS TEXT) AS last_checked_at,
                            last_error, consecutive_errors, CAST(created_at AS TEXT) AS created_at,
                            CAST(updated_at AS TEXT) AS updated_at
                     FROM content_subscriptions
                     WHERE user_id = $1 AND source_type = $2 AND source_id = $3",
                )
                .bind(user_id)
                .bind(source_type)
                .bind(source_id)
                .fetch_optional(pg_pool)
                .await
                .context("postgres has_content_subscription")?;
                row.map(map_pg_content_subscription).transpose()
            }
        }
    }

    pub async fn upsert_content_subscription(
        &self,
        user_id: i64,
        source_type: &str,
        source_id: &str,
        display_name: &str,
        watch_mask: u32,
        source_meta: Option<&JsonValue>,
    ) -> Result<i64> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite upsert_content_subscription connection")?;
                sqlite_upsert_content_subscription(
                    &conn,
                    user_id,
                    source_type,
                    source_id,
                    display_name,
                    watch_mask,
                    source_meta,
                )
                .map_err(anyhow::Error::msg)
                .context("sqlite upsert_content_subscription")
            }
            Self::Postgres { pg_pool, .. } => {
                let meta_json = source_meta.map(|value| value.to_string());
                let row = sqlx::query(
                    "INSERT INTO content_subscriptions (
                        user_id, source_type, source_id, display_name, watch_mask, source_meta,
                        is_active, consecutive_errors, updated_at
                     ) VALUES ($1, $2, $3, $4, $5, $6, 1, 0, NOW())
                     ON CONFLICT (user_id, source_type, source_id) DO UPDATE SET
                        watch_mask = EXCLUDED.watch_mask,
                        display_name = EXCLUDED.display_name,
                        source_meta = COALESCE(EXCLUDED.source_meta, content_subscriptions.source_meta),
                        is_active = 1,
                        consecutive_errors = 0,
                        last_error = NULL,
                        updated_at = NOW()
                     RETURNING id",
                )
                .bind(user_id)
                .bind(source_type)
                .bind(source_id)
                .bind(display_name)
                .bind(watch_mask as i32)
                .bind(meta_json)
                .fetch_one(pg_pool)
                .await
                .context("postgres upsert_content_subscription")?;
                Ok(row.get("id"))
            }
        }
    }

    pub async fn deactivate_content_subscription(&self, id: i64) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite deactivate_content_subscription connection")?;
                sqlite_deactivate_content_subscription(&conn, id)
                    .map_err(anyhow::Error::msg)
                    .context("sqlite deactivate_content_subscription")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "UPDATE content_subscriptions
                     SET is_active = 0, updated_at = NOW()
                     WHERE id = $1",
                )
                .bind(id)
                .execute(pg_pool)
                .await
                .context("postgres deactivate_content_subscription")?;
                Ok(())
            }
        }
    }

    pub async fn deactivate_all_content_subscriptions_for_user(&self, user_id: i64) -> Result<u32> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool)
                    .context("sqlite deactivate_all_content_subscriptions_for_user connection")?;
                sqlite_deactivate_all_content_subscriptions_for_user(&conn, user_id)
                    .map_err(anyhow::Error::msg)
                    .context("sqlite deactivate_all_content_subscriptions_for_user")
            }
            Self::Postgres { pg_pool, .. } => {
                let result = sqlx::query(
                    "UPDATE content_subscriptions
                     SET is_active = 0, updated_at = NOW()
                     WHERE user_id = $1 AND is_active = 1",
                )
                .bind(user_id)
                .execute(pg_pool)
                .await
                .context("postgres deactivate_all_content_subscriptions_for_user")?;
                Ok(result.rows_affected() as u32)
            }
        }
    }

    pub async fn update_content_watch_mask(&self, id: i64, new_mask: u32) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite update_content_watch_mask connection")?;
                sqlite_update_content_watch_mask(&conn, id, new_mask)
                    .map_err(anyhow::Error::msg)
                    .context("sqlite update_content_watch_mask")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "UPDATE content_subscriptions
                     SET watch_mask = $1, updated_at = NOW()
                     WHERE id = $2",
                )
                .bind(new_mask as i32)
                .bind(id)
                .execute(pg_pool)
                .await
                .context("postgres update_content_watch_mask")?;
                Ok(())
            }
        }
    }

    pub async fn get_active_content_source_groups(&self) -> Result<Vec<ContentSourceGroup>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_active_content_source_groups connection")?;
                sqlite_get_active_content_source_groups(&conn)
                    .map_err(anyhow::Error::msg)
                    .context("sqlite get_active_content_source_groups")
            }
            Self::Postgres { pg_pool, .. } => {
                let rows = sqlx::query(
                    "SELECT id, user_id, source_type, source_id, display_name, watch_mask, is_active,
                            last_seen_state, source_meta, CAST(last_checked_at AS TEXT) AS last_checked_at,
                            last_error, consecutive_errors, CAST(created_at AS TEXT) AS created_at,
                            CAST(updated_at AS TEXT) AS updated_at
                     FROM content_subscriptions
                     WHERE is_active = 1
                     ORDER BY last_checked_at ASC NULLS FIRST, source_type, source_id",
                )
                .fetch_all(pg_pool)
                .await
                .context("postgres get_active_content_source_groups")?;
                let all_subs: Vec<ContentSubscriptionRecord> = rows
                    .into_iter()
                    .map(map_pg_content_subscription)
                    .collect::<Result<_>>()?;
                Ok(group_content_subscriptions(all_subs))
            }
        }
    }

    pub async fn update_content_check_success(
        &self,
        source_type: &str,
        source_id: &str,
        new_state: &JsonValue,
        new_meta: Option<&JsonValue>,
    ) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite update_content_check_success connection")?;
                sqlite_update_content_check_success(&conn, source_type, source_id, new_state, new_meta)
                    .map_err(anyhow::Error::msg)
                    .context("sqlite update_content_check_success")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "UPDATE content_subscriptions
                     SET last_seen_state = $1,
                         source_meta = COALESCE($2, source_meta),
                         last_checked_at = NOW(),
                         last_error = NULL,
                         consecutive_errors = 0,
                         updated_at = NOW()
                     WHERE source_type = $3 AND source_id = $4 AND is_active = 1",
                )
                .bind(new_state.to_string())
                .bind(new_meta.map(|value| value.to_string()))
                .bind(source_type)
                .bind(source_id)
                .execute(pg_pool)
                .await
                .context("postgres update_content_check_success")?;
                Ok(())
            }
        }
    }

    pub async fn update_content_check_error(&self, source_type: &str, source_id: &str, error: &str) -> Result<u32> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite update_content_check_error connection")?;
                sqlite_update_content_check_error(&conn, source_type, source_id, error)
                    .map_err(anyhow::Error::msg)
                    .context("sqlite update_content_check_error")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "UPDATE content_subscriptions
                     SET last_checked_at = NOW(),
                         last_error = $1,
                         consecutive_errors = consecutive_errors + 1,
                         updated_at = NOW()
                     WHERE source_type = $2 AND source_id = $3 AND is_active = 1",
                )
                .bind(error)
                .bind(source_type)
                .bind(source_id)
                .execute(pg_pool)
                .await
                .context("postgres update_content_check_error update")?;
                let row = sqlx::query(
                    "SELECT COALESCE(MAX(consecutive_errors), 0)::BIGINT AS max_errors
                     FROM content_subscriptions
                     WHERE source_type = $1 AND source_id = $2 AND is_active = 1",
                )
                .bind(source_type)
                .bind(source_id)
                .fetch_one(pg_pool)
                .await
                .context("postgres update_content_check_error select")?;
                Ok(row.get::<i64, _>("max_errors") as u32)
            }
        }
    }

    pub async fn auto_disable_errored_content(
        &self,
        source_type: &str,
        source_id: &str,
        max_errors: u32,
    ) -> Result<u32> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite auto_disable_errored_content connection")?;
                sqlite_auto_disable_errored_content(&conn, source_type, source_id, max_errors)
                    .map_err(anyhow::Error::msg)
                    .context("sqlite auto_disable_errored_content")
            }
            Self::Postgres { pg_pool, .. } => {
                let result = sqlx::query(
                    "UPDATE content_subscriptions
                     SET is_active = 0, updated_at = NOW()
                     WHERE source_type = $1 AND source_id = $2
                       AND is_active = 1 AND consecutive_errors >= $3",
                )
                .bind(source_type)
                .bind(source_id)
                .bind(max_errors as i32)
                .execute(pg_pool)
                .await
                .context("postgres auto_disable_errored_content")?;
                Ok(result.rows_affected() as u32)
            }
        }
    }
}

fn parse_json_value(value: Option<String>) -> Option<JsonValue> {
    value.and_then(|raw| serde_json::from_str(&raw).ok())
}

fn map_pg_content_subscription(row: sqlx::postgres::PgRow) -> Result<ContentSubscriptionRecord> {
    Ok(ContentSubscriptionRecord {
        id: row.get("id"),
        user_id: row.get("user_id"),
        source_type: row.get("source_type"),
        source_id: row.get("source_id"),
        display_name: row.get("display_name"),
        watch_mask: row.get::<i32, _>("watch_mask") as u32,
        last_seen_state: parse_json_value(row.get("last_seen_state")),
        source_meta: parse_json_value(row.get("source_meta")),
        is_active: row.get::<i32, _>("is_active") != 0,
        last_checked_at: row.get("last_checked_at"),
        last_error: row.get("last_error"),
        consecutive_errors: row.get::<i32, _>("consecutive_errors") as u32,
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

fn group_content_subscriptions(all_subs: Vec<ContentSubscriptionRecord>) -> Vec<ContentSourceGroup> {
    let mut groups: Vec<ContentSourceGroup> = Vec::new();
    for sub in all_subs {
        if let Some(group) = groups
            .iter_mut()
            .find(|group| group.source_type == sub.source_type && group.source_id == sub.source_id)
        {
            group.combined_mask |= sub.watch_mask;
            group.subscriptions.push(sub);
        } else {
            let combined_mask = sub.watch_mask;
            groups.push(ContentSourceGroup {
                source_type: sub.source_type.clone(),
                source_id: sub.source_id.clone(),
                combined_mask,
                subscriptions: vec![sub],
            });
        }
    }
    groups
}

fn sqlite_parse_content_subscription_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ContentSubscriptionRecord> {
    let last_seen_state: Option<String> = row.get(7)?;
    let source_meta: Option<String> = row.get(8)?;
    Ok(ContentSubscriptionRecord {
        id: row.get(0)?,
        user_id: row.get(1)?,
        source_type: row.get(2)?,
        source_id: row.get(3)?,
        display_name: row.get(4)?,
        watch_mask: row.get::<_, u32>(5)?,
        last_seen_state: parse_json_value(last_seen_state),
        source_meta: parse_json_value(source_meta),
        is_active: row.get::<_, i32>(6)? != 0,
        last_checked_at: row.get(9)?,
        last_error: row.get(10)?,
        consecutive_errors: row.get::<_, u32>(11)?,
        created_at: row.get(12)?,
        updated_at: row.get(13)?,
    })
}

fn sqlite_upsert_content_subscription(
    conn: &DbConnection,
    user_id: i64,
    source_type: &str,
    source_id: &str,
    display_name: &str,
    watch_mask: u32,
    source_meta: Option<&JsonValue>,
) -> rusqlite::Result<i64> {
    let meta_json = source_meta.map(|value| value.to_string());
    conn.execute(
        "INSERT INTO content_subscriptions (user_id, source_type, source_id, display_name, watch_mask, source_meta, is_active, consecutive_errors, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, 0, CURRENT_TIMESTAMP)
         ON CONFLICT(user_id, source_type, source_id) DO UPDATE SET
           watch_mask = ?5,
           display_name = ?4,
           source_meta = COALESCE(?6, source_meta),
           is_active = 1,
           consecutive_errors = 0,
           last_error = NULL,
           updated_at = CURRENT_TIMESTAMP",
        rusqlite::params![user_id, source_type, source_id, display_name, watch_mask, meta_json],
    )?;
    conn.query_row(
        "SELECT id FROM content_subscriptions WHERE user_id = ?1 AND source_type = ?2 AND source_id = ?3",
        rusqlite::params![user_id, source_type, source_id],
        |row| row.get(0),
    )
}

fn sqlite_get_content_subscription(
    conn: &DbConnection,
    id: i64,
) -> rusqlite::Result<Option<ContentSubscriptionRecord>> {
    conn.query_row(
        "SELECT id, user_id, source_type, source_id, display_name, watch_mask, is_active,
                last_seen_state, source_meta, last_checked_at, last_error, consecutive_errors,
                created_at, updated_at
         FROM content_subscriptions WHERE id = ?1",
        rusqlite::params![id],
        sqlite_parse_content_subscription_row,
    )
    .optional()
}

fn sqlite_get_user_content_subscriptions(
    conn: &DbConnection,
    user_id: i64,
) -> rusqlite::Result<Vec<ContentSubscriptionRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, user_id, source_type, source_id, display_name, watch_mask, is_active,
                last_seen_state, source_meta, last_checked_at, last_error, consecutive_errors,
                created_at, updated_at
         FROM content_subscriptions
         WHERE user_id = ?1 AND is_active = 1
         ORDER BY created_at ASC",
    )?;
    let rows = stmt.query_map(rusqlite::params![user_id], sqlite_parse_content_subscription_row)?;
    rows.collect()
}

fn sqlite_count_user_content_subscriptions(conn: &DbConnection, user_id: i64) -> rusqlite::Result<u32> {
    conn.query_row(
        "SELECT COUNT(*) FROM content_subscriptions WHERE user_id = ?1 AND is_active = 1",
        rusqlite::params![user_id],
        |row| row.get(0),
    )
}

fn sqlite_has_content_subscription(
    conn: &DbConnection,
    user_id: i64,
    source_type: &str,
    source_id: &str,
) -> rusqlite::Result<Option<ContentSubscriptionRecord>> {
    conn.query_row(
        "SELECT id, user_id, source_type, source_id, display_name, watch_mask, is_active,
                last_seen_state, source_meta, last_checked_at, last_error, consecutive_errors,
                created_at, updated_at
         FROM content_subscriptions
         WHERE user_id = ?1 AND source_type = ?2 AND source_id = ?3",
        rusqlite::params![user_id, source_type, source_id],
        sqlite_parse_content_subscription_row,
    )
    .optional()
}

fn sqlite_deactivate_content_subscription(conn: &DbConnection, id: i64) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE content_subscriptions SET is_active = 0, updated_at = CURRENT_TIMESTAMP WHERE id = ?1",
        rusqlite::params![id],
    )?;
    Ok(())
}

fn sqlite_deactivate_all_content_subscriptions_for_user(conn: &DbConnection, user_id: i64) -> rusqlite::Result<u32> {
    Ok(conn.execute(
        "UPDATE content_subscriptions SET is_active = 0, updated_at = CURRENT_TIMESTAMP
         WHERE user_id = ?1 AND is_active = 1",
        rusqlite::params![user_id],
    )? as u32)
}

fn sqlite_update_content_watch_mask(conn: &DbConnection, id: i64, new_mask: u32) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE content_subscriptions SET watch_mask = ?1, updated_at = CURRENT_TIMESTAMP WHERE id = ?2",
        rusqlite::params![new_mask, id],
    )?;
    Ok(())
}

fn sqlite_get_active_content_source_groups(conn: &DbConnection) -> rusqlite::Result<Vec<ContentSourceGroup>> {
    let mut stmt = conn.prepare(
        "SELECT id, user_id, source_type, source_id, display_name, watch_mask, is_active,
                last_seen_state, source_meta, last_checked_at, last_error, consecutive_errors,
                created_at, updated_at
         FROM content_subscriptions
         WHERE is_active = 1
         ORDER BY last_checked_at ASC NULLS FIRST, source_type, source_id",
    )?;
    let rows = stmt.query_map([], sqlite_parse_content_subscription_row)?;
    let all_subs: rusqlite::Result<Vec<_>> = rows.collect();
    Ok(group_content_subscriptions(all_subs?))
}

fn sqlite_update_content_check_success(
    conn: &DbConnection,
    source_type: &str,
    source_id: &str,
    new_state: &JsonValue,
    new_meta: Option<&JsonValue>,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE content_subscriptions
         SET last_seen_state = ?1,
             source_meta = COALESCE(?2, source_meta),
             last_checked_at = CURRENT_TIMESTAMP,
             last_error = NULL,
             consecutive_errors = 0,
             updated_at = CURRENT_TIMESTAMP
         WHERE source_type = ?3 AND source_id = ?4 AND is_active = 1",
        rusqlite::params![
            new_state.to_string(),
            new_meta.map(|value| value.to_string()),
            source_type,
            source_id
        ],
    )?;
    Ok(())
}

fn sqlite_update_content_check_error(
    conn: &DbConnection,
    source_type: &str,
    source_id: &str,
    error: &str,
) -> rusqlite::Result<u32> {
    conn.execute(
        "UPDATE content_subscriptions
         SET last_checked_at = CURRENT_TIMESTAMP,
             last_error = ?1,
             consecutive_errors = consecutive_errors + 1,
             updated_at = CURRENT_TIMESTAMP
         WHERE source_type = ?2 AND source_id = ?3 AND is_active = 1",
        rusqlite::params![error, source_type, source_id],
    )?;
    conn.query_row(
        "SELECT COALESCE(MAX(consecutive_errors), 0)
         FROM content_subscriptions
         WHERE source_type = ?1 AND source_id = ?2 AND is_active = 1",
        rusqlite::params![source_type, source_id],
        |row| row.get(0),
    )
}

fn sqlite_auto_disable_errored_content(
    conn: &DbConnection,
    source_type: &str,
    source_id: &str,
    max_errors: u32,
) -> rusqlite::Result<u32> {
    Ok(conn.execute(
        "UPDATE content_subscriptions
         SET is_active = 0, updated_at = CURRENT_TIMESTAMP
         WHERE source_type = ?1 AND source_id = ?2 AND is_active = 1 AND consecutive_errors >= ?3",
        rusqlite::params![source_type, source_id, max_errors],
    )? as u32)
}
