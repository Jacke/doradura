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

#[cfg(test)]
pub(crate) fn create_test_schema(conn: &Connection) {
    conn.execute_batch("PRAGMA foreign_keys = OFF;").unwrap();
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS content_subscriptions (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id INTEGER NOT NULL,
            source_type TEXT NOT NULL,
            source_id TEXT NOT NULL,
            display_name TEXT NOT NULL DEFAULT '',
            watch_mask INTEGER NOT NULL DEFAULT 3,
            last_seen_state TEXT DEFAULT NULL,
            source_meta TEXT DEFAULT NULL,
            is_active INTEGER NOT NULL DEFAULT 1,
            last_checked_at DATETIME DEFAULT NULL,
            last_error TEXT DEFAULT NULL,
            consecutive_errors INTEGER NOT NULL DEFAULT 0,
            created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
            updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
            UNIQUE(user_id, source_type, source_id)
        );",
    )
    .unwrap();
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

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use serde_json::json;

    fn make_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        create_test_schema(&conn);
        conn
    }

    // ── upsert_subscription ──────────────────────────────────────────────────

    #[test]
    fn upsert_creates_new_subscription() {
        let conn = make_conn();
        let id = upsert_subscription(&conn, 100, "instagram", "dashaostro", "@dashaostro", 3, None);
        assert!(id.is_ok(), "upsert should succeed: {:?}", id.err());
        assert!(id.unwrap() > 0);
    }

    #[test]
    fn upsert_is_idempotent_returns_same_id() {
        let conn = make_conn();
        let id1 = upsert_subscription(&conn, 100, "instagram", "cristiano", "@cristiano", 3, None).unwrap();
        let id2 = upsert_subscription(&conn, 100, "instagram", "cristiano", "@cristiano", 1, None).unwrap();
        assert_eq!(
            id1, id2,
            "upsert of same (user, source_type, source_id) must return same id"
        );
    }

    #[test]
    fn upsert_updates_watch_mask_on_conflict() {
        let conn = make_conn();
        upsert_subscription(&conn, 100, "instagram", "cristiano", "@cristiano", 3, None).unwrap();
        // Re-upsert with mask=1
        upsert_subscription(&conn, 100, "instagram", "cristiano", "@cristiano", 1, None).unwrap();

        let sub = has_subscription(&conn, 100, "instagram", "cristiano").unwrap().unwrap();
        assert_eq!(sub.watch_mask, 1, "watch_mask must be updated on conflict");
    }

    #[test]
    fn upsert_reactivates_inactive_subscription() {
        let conn = make_conn();
        let id = upsert_subscription(&conn, 100, "instagram", "test_user", "@test_user", 3, None).unwrap();
        deactivate_subscription(&conn, id).unwrap();

        // Re-upsert: should reactivate
        upsert_subscription(&conn, 100, "instagram", "test_user", "@test_user", 3, None).unwrap();

        let sub = has_subscription(&conn, 100, "instagram", "test_user").unwrap().unwrap();
        assert!(sub.is_active, "subscription must be reactivated on upsert");
    }

    #[test]
    fn upsert_with_source_meta() {
        let conn = make_conn();
        let meta = json!({"ig_user_id": "3494148660"});
        let id = upsert_subscription(&conn, 100, "instagram", "dashaostro", "@dashaostro", 3, Some(&meta));
        assert!(id.is_ok());

        let sub = get_subscription(&conn, id.unwrap()).unwrap().unwrap();
        let stored_meta = sub.source_meta.unwrap();
        assert_eq!(stored_meta["ig_user_id"], "3494148660");
    }

    // ── get_subscription ─────────────────────────────────────────────────────

    #[test]
    fn get_subscription_returns_correct_fields() {
        let conn = make_conn();
        let id = upsert_subscription(&conn, 42, "instagram", "dashaostro", "@dashaostro", 3, None).unwrap();

        let sub = get_subscription(&conn, id).unwrap().expect("must exist");
        assert_eq!(sub.user_id, 42);
        assert_eq!(sub.source_type, "instagram");
        assert_eq!(sub.source_id, "dashaostro");
        assert_eq!(sub.display_name, "@dashaostro");
        assert_eq!(sub.watch_mask, 3);
        assert!(sub.is_active);
        assert_eq!(sub.consecutive_errors, 0);
        assert!(sub.last_error.is_none());
    }

    #[test]
    fn get_subscription_nonexistent_returns_none() {
        let conn = make_conn();
        let result = get_subscription(&conn, 99999).unwrap();
        assert!(result.is_none());
    }

    // ── get_user_subscriptions ───────────────────────────────────────────────

    #[test]
    fn get_user_subscriptions_returns_only_active() {
        let conn = make_conn();
        let id1 = upsert_subscription(&conn, 10, "instagram", "user_a", "@user_a", 3, None).unwrap();
        let _id2 = upsert_subscription(&conn, 10, "instagram", "user_b", "@user_b", 1, None).unwrap();
        // Deactivate first
        deactivate_subscription(&conn, id1).unwrap();

        let subs = get_user_subscriptions(&conn, 10).unwrap();
        assert_eq!(subs.len(), 1, "only active subscriptions must be returned");
        assert_eq!(subs[0].source_id, "user_b");
    }

    #[test]
    fn get_user_subscriptions_empty_for_new_user() {
        let conn = make_conn();
        let subs = get_user_subscriptions(&conn, 999).unwrap();
        assert!(subs.is_empty());
    }

    #[test]
    fn get_user_subscriptions_isolates_users() {
        let conn = make_conn();
        upsert_subscription(&conn, 1, "instagram", "cristiano", "@cristiano", 3, None).unwrap();
        upsert_subscription(&conn, 2, "instagram", "cristiano", "@cristiano", 3, None).unwrap();

        let subs_user1 = get_user_subscriptions(&conn, 1).unwrap();
        let subs_user2 = get_user_subscriptions(&conn, 2).unwrap();
        assert_eq!(subs_user1.len(), 1);
        assert_eq!(subs_user2.len(), 1);
    }

    // ── count_user_subscriptions ─────────────────────────────────────────────

    #[test]
    fn count_user_subscriptions_correct() {
        let conn = make_conn();
        assert_eq!(count_user_subscriptions(&conn, 5).unwrap(), 0);

        upsert_subscription(&conn, 5, "instagram", "a", "@a", 3, None).unwrap();
        assert_eq!(count_user_subscriptions(&conn, 5).unwrap(), 1);

        upsert_subscription(&conn, 5, "instagram", "b", "@b", 3, None).unwrap();
        assert_eq!(count_user_subscriptions(&conn, 5).unwrap(), 2);
    }

    #[test]
    fn count_does_not_include_inactive() {
        let conn = make_conn();
        let id = upsert_subscription(&conn, 7, "instagram", "x", "@x", 3, None).unwrap();
        assert_eq!(count_user_subscriptions(&conn, 7).unwrap(), 1);
        deactivate_subscription(&conn, id).unwrap();
        assert_eq!(count_user_subscriptions(&conn, 7).unwrap(), 0);
    }

    // ── has_subscription ─────────────────────────────────────────────────────

    #[test]
    fn has_subscription_returns_some_when_exists() {
        let conn = make_conn();
        upsert_subscription(&conn, 10, "instagram", "dashaostro", "@dashaostro", 3, None).unwrap();

        let result = has_subscription(&conn, 10, "instagram", "dashaostro").unwrap();
        assert!(result.is_some());
    }

    #[test]
    fn has_subscription_returns_none_when_absent() {
        let conn = make_conn();
        let result = has_subscription(&conn, 10, "instagram", "nobody").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn has_subscription_returns_inactive_too() {
        // has_subscription checks any row (active or not) — used for duplicate detection
        let conn = make_conn();
        let id = upsert_subscription(&conn, 10, "instagram", "ghost", "@ghost", 3, None).unwrap();
        deactivate_subscription(&conn, id).unwrap();

        let result = has_subscription(&conn, 10, "instagram", "ghost").unwrap();
        assert!(result.is_some(), "has_subscription must return inactive subs too");
        assert!(!result.unwrap().is_active);
    }

    // ── deactivate_subscription ──────────────────────────────────────────────

    #[test]
    fn deactivate_subscription_sets_inactive() {
        let conn = make_conn();
        let id = upsert_subscription(&conn, 20, "instagram", "test", "@test", 3, None).unwrap();

        deactivate_subscription(&conn, id).unwrap();

        let sub = get_subscription(&conn, id).unwrap().unwrap();
        assert!(!sub.is_active, "subscription must be inactive after deactivate");
    }

    #[test]
    fn deactivate_nonexistent_does_not_error() {
        let conn = make_conn();
        let result = deactivate_subscription(&conn, 99999);
        assert!(result.is_ok(), "deactivating nonexistent sub must not error");
    }

    // ── deactivate_all_for_user ──────────────────────────────────────────────

    #[test]
    fn deactivate_all_for_user_deactivates_all() {
        let conn = make_conn();
        upsert_subscription(&conn, 30, "instagram", "a", "@a", 3, None).unwrap();
        upsert_subscription(&conn, 30, "instagram", "b", "@b", 3, None).unwrap();

        let count = deactivate_all_for_user(&conn, 30).unwrap();
        assert_eq!(count, 2);
        assert_eq!(count_user_subscriptions(&conn, 30).unwrap(), 0);
    }

    #[test]
    fn deactivate_all_does_not_affect_other_users() {
        let conn = make_conn();
        upsert_subscription(&conn, 31, "instagram", "a", "@a", 3, None).unwrap();
        upsert_subscription(&conn, 32, "instagram", "a", "@a", 3, None).unwrap();

        deactivate_all_for_user(&conn, 31).unwrap();

        assert_eq!(count_user_subscriptions(&conn, 31).unwrap(), 0);
        assert_eq!(
            count_user_subscriptions(&conn, 32).unwrap(),
            1,
            "user 32 must be unaffected"
        );
    }

    // ── update_watch_mask ────────────────────────────────────────────────────

    #[test]
    fn update_watch_mask_changes_mask() {
        let conn = make_conn();
        let id = upsert_subscription(&conn, 40, "instagram", "test", "@test", 3, None).unwrap();

        update_watch_mask(&conn, id, 1).unwrap();

        let sub = get_subscription(&conn, id).unwrap().unwrap();
        assert_eq!(sub.watch_mask, 1, "watch_mask must be updated to 1 (Posts only)");
    }

    #[test]
    fn update_watch_mask_posts_only() {
        let conn = make_conn();
        let id = upsert_subscription(&conn, 40, "instagram", "u1", "@u1", 3, None).unwrap();
        update_watch_mask(&conn, id, 1).unwrap(); // Posts only
        assert_eq!(get_subscription(&conn, id).unwrap().unwrap().watch_mask, 1);
    }

    #[test]
    fn update_watch_mask_stories_only() {
        let conn = make_conn();
        let id = upsert_subscription(&conn, 40, "instagram", "u2", "@u2", 3, None).unwrap();
        update_watch_mask(&conn, id, 2).unwrap(); // Stories only
        assert_eq!(get_subscription(&conn, id).unwrap().unwrap().watch_mask, 2);
    }

    // ── update_check_success / update_check_error ────────────────────────────

    #[test]
    fn update_check_success_clears_error() {
        let conn = make_conn();
        upsert_subscription(&conn, 50, "instagram", "user_x", "@user_x", 3, None).unwrap();
        // First record an error
        update_check_error(&conn, "instagram", "user_x", "some error").unwrap();

        // Then record success
        let new_state = json!({"last_shortcode": "ABC123", "last_story_ts": 0});
        update_check_success(&conn, "instagram", "user_x", &new_state, None).unwrap();

        let sub = has_subscription(&conn, 50, "instagram", "user_x").unwrap().unwrap();
        assert!(sub.last_error.is_none(), "last_error must be cleared after success");
        assert_eq!(sub.consecutive_errors, 0, "consecutive_errors must be reset");
        let state = sub.last_seen_state.unwrap();
        assert_eq!(state["last_shortcode"], "ABC123");
    }

    #[test]
    fn update_check_error_increments_consecutive_errors() {
        let conn = make_conn();
        upsert_subscription(&conn, 51, "instagram", "err_user", "@err_user", 3, None).unwrap();

        update_check_error(&conn, "instagram", "err_user", "timeout").unwrap();
        update_check_error(&conn, "instagram", "err_user", "timeout").unwrap();

        let sub = has_subscription(&conn, 51, "instagram", "err_user").unwrap().unwrap();
        assert_eq!(sub.consecutive_errors, 2);
        assert_eq!(sub.last_error.as_deref(), Some("timeout"));
    }

    // ── get_active_source_groups ─────────────────────────────────────────────

    #[test]
    fn get_active_source_groups_groups_by_source() {
        let conn = make_conn();
        // Two users subscribed to the same Instagram profile
        upsert_subscription(&conn, 60, "instagram", "celebrity", "@celebrity", 1, None).unwrap();
        upsert_subscription(&conn, 61, "instagram", "celebrity", "@celebrity", 2, None).unwrap();

        let groups = get_active_source_groups(&conn).unwrap();
        assert_eq!(groups.len(), 1, "same source must be grouped into one entry");
        let g = &groups[0];
        assert_eq!(g.source_type, "instagram");
        assert_eq!(g.source_id, "celebrity");
        assert_eq!(g.combined_mask, 3, "combined_mask must be OR of all subscribers: 1|2=3");
        assert_eq!(g.subscriptions.len(), 2);
    }

    #[test]
    fn get_active_source_groups_excludes_inactive() {
        let conn = make_conn();
        let id = upsert_subscription(&conn, 62, "instagram", "inactive_star", "@inactive_star", 3, None).unwrap();
        deactivate_subscription(&conn, id).unwrap();

        let groups = get_active_source_groups(&conn).unwrap();
        assert!(groups.is_empty(), "inactive subscriptions must not appear in groups");
    }

    // ── auto_disable_errored ─────────────────────────────────────────────────

    #[test]
    fn auto_disable_errored_disables_at_threshold() {
        let conn = make_conn();
        upsert_subscription(&conn, 70, "instagram", "flaky", "@flaky", 3, None).unwrap();

        // Simulate 5 consecutive errors
        for _ in 0..5 {
            update_check_error(&conn, "instagram", "flaky", "network error").unwrap();
        }

        let disabled = auto_disable_errored(&conn, "instagram", "flaky", 5).unwrap();
        assert_eq!(disabled, 1, "one subscription must be disabled");
        assert_eq!(count_user_subscriptions(&conn, 70).unwrap(), 0, "must be deactivated");
    }

    #[test]
    fn auto_disable_errored_does_not_disable_below_threshold() {
        let conn = make_conn();
        upsert_subscription(&conn, 71, "instagram", "reliable", "@reliable", 3, None).unwrap();
        update_check_error(&conn, "instagram", "reliable", "one error").unwrap();

        let disabled = auto_disable_errored(&conn, "instagram", "reliable", 5).unwrap();
        assert_eq!(disabled, 0, "must not disable with only 1 error (threshold=5)");
        assert_eq!(count_user_subscriptions(&conn, 71).unwrap(), 1);
    }
}
