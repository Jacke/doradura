//! Database operations for content subscriptions.

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value as JsonValue;

/// A content subscription row from the database.
#[derive(Debug, Clone)]
pub struct ContentSubscription {
    pub id: i64,
    pub user_id: i64,
    pub source_type: String,
    pub source_id: String,
    pub display_name: String,
    pub watch_mask: u32,
    pub last_seen_state: Option<JsonValue>,
    pub source_meta: Option<JsonValue>,
    pub is_active: bool,
    pub last_checked_at: Option<String>,
    pub last_error: Option<String>,
    pub consecutive_errors: u32,
    pub created_at: String,
    pub updated_at: String,
}

/// A group of subscriptions for the same (source_type, source_id).
/// Used by the scheduler to deduplicate API calls.
#[derive(Debug, Clone)]
pub struct SourceGroup {
    pub source_type: String,
    pub source_id: String,
    /// Combined watch_mask (OR of all subscribers' masks).
    pub combined_mask: u32,
    /// Individual subscriptions in this group.
    pub subscriptions: Vec<ContentSubscription>,
}

fn parse_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ContentSubscription> {
    let state_str: Option<String> = row.get(7)?;
    let meta_str: Option<String> = row.get(8)?;
    Ok(ContentSubscription {
        id: row.get(0)?,
        user_id: row.get(1)?,
        source_type: row.get(2)?,
        source_id: row.get(3)?,
        display_name: row.get(4)?,
        watch_mask: row.get::<_, u32>(5)?,
        last_seen_state: state_str.and_then(|s| serde_json::from_str(&s).ok()),
        source_meta: meta_str.and_then(|s| serde_json::from_str(&s).ok()),
        is_active: row.get::<_, i32>(6)? != 0,
        last_checked_at: row.get(9)?,
        last_error: row.get(10)?,
        consecutive_errors: row.get::<_, u32>(11)?,
        created_at: row.get(12)?,
        updated_at: row.get(13)?,
    })
}

/// Create or reactivate a subscription.
/// Returns the subscription ID.
pub fn upsert_subscription(
    conn: &Connection,
    user_id: i64,
    source_type: &str,
    source_id: &str,
    display_name: &str,
    watch_mask: u32,
    source_meta: Option<&JsonValue>,
) -> Result<i64, String> {
    let meta_json = source_meta.map(|v| v.to_string());

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
        params![user_id, source_type, source_id, display_name, watch_mask, meta_json],
    )
    .map_err(|e| format!("Failed to upsert subscription: {}", e))?;

    let id: i64 = conn
        .query_row(
            "SELECT id FROM content_subscriptions WHERE user_id = ?1 AND source_type = ?2 AND source_id = ?3",
            params![user_id, source_type, source_id],
            |row| row.get(0),
        )
        .map_err(|e| format!("Failed to get subscription id: {}", e))?;

    Ok(id)
}

/// Get a subscription by ID.
pub fn get_subscription(conn: &Connection, id: i64) -> Result<Option<ContentSubscription>, String> {
    conn.query_row(
        "SELECT id, user_id, source_type, source_id, display_name, watch_mask, is_active,
                last_seen_state, source_meta, last_checked_at, last_error, consecutive_errors,
                created_at, updated_at
         FROM content_subscriptions WHERE id = ?1",
        params![id],
        parse_row,
    )
    .optional()
    .map_err(|e| format!("Failed to get subscription: {}", e))
}

/// Get all active subscriptions for a user.
pub fn get_user_subscriptions(conn: &Connection, user_id: i64) -> Result<Vec<ContentSubscription>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, user_id, source_type, source_id, display_name, watch_mask, is_active,
                    last_seen_state, source_meta, last_checked_at, last_error, consecutive_errors,
                    created_at, updated_at
             FROM content_subscriptions
             WHERE user_id = ?1 AND is_active = 1
             ORDER BY created_at ASC",
        )
        .map_err(|e| format!("Failed to prepare query: {}", e))?;

    let rows = stmt
        .query_map(params![user_id], parse_row)
        .map_err(|e| format!("Failed to query subscriptions: {}", e))?;

    let mut subs = Vec::new();
    for row in rows {
        subs.push(row.map_err(|e| format!("Failed to read row: {}", e))?);
    }
    Ok(subs)
}

/// Count active subscriptions for a user.
pub fn count_user_subscriptions(conn: &Connection, user_id: i64) -> Result<u32, String> {
    conn.query_row(
        "SELECT COUNT(*) FROM content_subscriptions WHERE user_id = ?1 AND is_active = 1",
        params![user_id],
        |row| row.get::<_, u32>(0),
    )
    .map_err(|e| format!("Failed to count subscriptions: {}", e))
}

/// Deactivate (soft-delete) a subscription.
pub fn deactivate_subscription(conn: &Connection, id: i64) -> Result<(), String> {
    conn.execute(
        "UPDATE content_subscriptions SET is_active = 0, updated_at = CURRENT_TIMESTAMP WHERE id = ?1",
        params![id],
    )
    .map_err(|e| format!("Failed to deactivate subscription: {}", e))?;
    Ok(())
}

/// Deactivate all subscriptions for a user (e.g. when bot is blocked).
pub fn deactivate_all_for_user(conn: &Connection, user_id: i64) -> Result<u32, String> {
    let count = conn
        .execute(
            "UPDATE content_subscriptions SET is_active = 0, updated_at = CURRENT_TIMESTAMP
             WHERE user_id = ?1 AND is_active = 1",
            params![user_id],
        )
        .map_err(|e| format!("Failed to deactivate subscriptions: {}", e))?;
    Ok(count as u32)
}

/// Update watch mask for a subscription.
pub fn update_watch_mask(conn: &Connection, id: i64, new_mask: u32) -> Result<(), String> {
    conn.execute(
        "UPDATE content_subscriptions SET watch_mask = ?1, updated_at = CURRENT_TIMESTAMP WHERE id = ?2",
        params![new_mask, id],
    )
    .map_err(|e| format!("Failed to update watch mask: {}", e))?;
    Ok(())
}

/// Update last_seen_state and last_checked_at after a successful check.
pub fn update_check_success(
    conn: &Connection,
    source_type: &str,
    source_id: &str,
    new_state: &JsonValue,
    new_meta: Option<&JsonValue>,
) -> Result<(), String> {
    let state_json = new_state.to_string();
    let meta_json = new_meta.map(|v| v.to_string());

    conn.execute(
        "UPDATE content_subscriptions
         SET last_seen_state = ?1,
             source_meta = COALESCE(?2, source_meta),
             last_checked_at = CURRENT_TIMESTAMP,
             last_error = NULL,
             consecutive_errors = 0,
             updated_at = CURRENT_TIMESTAMP
         WHERE source_type = ?3 AND source_id = ?4 AND is_active = 1",
        params![state_json, meta_json, source_type, source_id],
    )
    .map_err(|e| format!("Failed to update check success: {}", e))?;
    Ok(())
}

/// Record a check error and increment consecutive_errors.
/// Returns the new consecutive_errors count.
pub fn update_check_error(conn: &Connection, source_type: &str, source_id: &str, error: &str) -> Result<u32, String> {
    conn.execute(
        "UPDATE content_subscriptions
         SET last_checked_at = CURRENT_TIMESTAMP,
             last_error = ?1,
             consecutive_errors = consecutive_errors + 1,
             updated_at = CURRENT_TIMESTAMP
         WHERE source_type = ?2 AND source_id = ?3 AND is_active = 1",
        params![error, source_type, source_id],
    )
    .map_err(|e| format!("Failed to update check error: {}", e))?;

    // Return the max consecutive_errors for this source
    conn.query_row(
        "SELECT MAX(consecutive_errors) FROM content_subscriptions
         WHERE source_type = ?1 AND source_id = ?2 AND is_active = 1",
        params![source_type, source_id],
        |row| row.get::<_, u32>(0),
    )
    .map_err(|e| format!("Failed to get error count: {}", e))
}

/// Auto-disable subscriptions that have too many consecutive errors.
pub fn auto_disable_errored(
    conn: &Connection,
    source_type: &str,
    source_id: &str,
    max_errors: u32,
) -> Result<u32, String> {
    let count = conn
        .execute(
            "UPDATE content_subscriptions
             SET is_active = 0, updated_at = CURRENT_TIMESTAMP
             WHERE source_type = ?1 AND source_id = ?2 AND is_active = 1 AND consecutive_errors >= ?3",
            params![source_type, source_id, max_errors],
        )
        .map_err(|e| format!("Failed to auto-disable: {}", e))?;
    Ok(count as u32)
}

/// Get active subscriptions grouped by (source_type, source_id) for the scheduler.
/// Ordered by last_checked_at ASC NULLS FIRST (most stale first).
pub fn get_active_source_groups(conn: &Connection) -> Result<Vec<SourceGroup>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, user_id, source_type, source_id, display_name, watch_mask, is_active,
                    last_seen_state, source_meta, last_checked_at, last_error, consecutive_errors,
                    created_at, updated_at
             FROM content_subscriptions
             WHERE is_active = 1
             ORDER BY last_checked_at ASC NULLS FIRST, source_type, source_id",
        )
        .map_err(|e| format!("Failed to prepare query: {}", e))?;

    let rows = stmt
        .query_map([], parse_row)
        .map_err(|e| format!("Failed to query: {}", e))?;

    let mut all_subs: Vec<ContentSubscription> = Vec::new();
    for row in rows {
        all_subs.push(row.map_err(|e| format!("Failed to read row: {}", e))?);
    }

    // Group by (source_type, source_id)
    let mut groups: Vec<SourceGroup> = Vec::new();
    for sub in all_subs {
        if let Some(group) = groups
            .iter_mut()
            .find(|g| g.source_type == sub.source_type && g.source_id == sub.source_id)
        {
            group.combined_mask |= sub.watch_mask;
            group.subscriptions.push(sub);
        } else {
            let combined_mask = sub.watch_mask;
            groups.push(SourceGroup {
                source_type: sub.source_type.clone(),
                source_id: sub.source_id.clone(),
                combined_mask,
                subscriptions: vec![sub],
            });
        }
    }

    Ok(groups)
}

/// Check if a user already has a subscription for this source.
pub fn has_subscription(
    conn: &Connection,
    user_id: i64,
    source_type: &str,
    source_id: &str,
) -> Result<Option<ContentSubscription>, String> {
    conn.query_row(
        "SELECT id, user_id, source_type, source_id, display_name, watch_mask, is_active,
                last_seen_state, source_meta, last_checked_at, last_error, consecutive_errors,
                created_at, updated_at
         FROM content_subscriptions
         WHERE user_id = ?1 AND source_type = ?2 AND source_id = ?3",
        params![user_id, source_type, source_id],
        parse_row,
    )
    .optional()
    .map_err(|e| format!("Failed to check subscription: {}", e))
}
