//! User management admin handlers.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use serde_json::json;

use crate::storage::get_connection;

use super::auth::{verify_admin, verify_admin_post};
use super::helpers::{like_param, log_audit};
use super::types::*;

const USERS_PER_PAGE: u32 = 50;
const DOWNLOADS_PER_PAGE: u32 = 50;

/// GET /admin/api/users — paginated, filterable user list.
pub(super) async fn admin_api_users(
    State(state): State<WebState>,
    header_map: HeaderMap,
    Query(q): Query<UserQuery>,
) -> Response {
    if let Err(resp) = verify_admin(&header_map, &state.bot_token) {
        return resp;
    }
    let page = q.page.unwrap_or(1).max(1);
    let filter = q.filter.unwrap_or_else(|| "all".to_string());
    let search = q.search.unwrap_or_default();
    let offset = ((page - 1) * USERS_PER_PAGE) as i64;
    let db = state.shared_storage.sqlite_pool();

    let result = tokio::task::spawn_blocking(move || -> Result<PaginatedResponse<ApiUser>, rusqlite::Error> {
        let conn = get_connection(&db).map_err(|_| rusqlite::Error::InvalidQuery)?;

        // Build WHERE clause with owned string parameters
        let mut conditions = Vec::new();

        match filter.as_str() {
            "free" => conditions.push("COALESCE(u.plan, 'free') = 'free'".to_string()),
            "premium" => conditions.push("u.plan = 'premium'".to_string()),
            "vip" => conditions.push("u.plan = 'vip'".to_string()),
            "blocked" => conditions.push("u.is_blocked = 1".to_string()),
            _ => {}
        }

        let search_param = if !search.is_empty() {
            conditions.push(
                "(u.username LIKE ?1 ESCAPE '\\' OR CAST(u.telegram_id AS TEXT) LIKE ?1 ESCAPE '\\')".to_string(),
            );
            Some(like_param(&search))
        } else {
            None
        };

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        let count_sql = format!("SELECT COUNT(*) FROM users u {}", where_clause);
        let total: i64 = if let Some(ref sp) = search_param {
            conn.query_row(&count_sql, rusqlite::params![sp], |r| r.get(0))
                .unwrap_or(0)
        } else {
            conn.query_row(&count_sql, [], |r| r.get(0)).unwrap_or(0)
        };

        let total_pages = ((total as f64) / USERS_PER_PAGE as f64).ceil() as u32;

        let query_sql = format!(
            "SELECT u.telegram_id, COALESCE(u.username, ''), COALESCE(u.plan, 'free'), \
                    COALESCE(u.is_blocked, 0), COALESCE(u.language, 'ru'), \
                    COUNT(d.id) AS dl_count \
             FROM users u \
             LEFT JOIN download_history d ON d.user_id = u.telegram_id \
             {} \
             GROUP BY u.telegram_id \
             ORDER BY dl_count DESC \
             LIMIT {} OFFSET {}",
            where_clause, USERS_PER_PAGE, offset
        );

        let users: Vec<ApiUser> = if let Some(ref sp) = search_param {
            conn.prepare(&query_sql)
                .and_then(|mut s| {
                    let rows = s.query_map(rusqlite::params![sp], |r| {
                        Ok(ApiUser {
                            telegram_id: r.get(0)?,
                            username: r.get(1)?,
                            plan: r.get(2)?,
                            is_blocked: r.get::<_, i64>(3)? != 0,
                            language: r.get(4)?,
                            download_count: r.get(5)?,
                        })
                    })?;
                    Ok(rows.filter_map(|r| r.ok()).collect())
                })
                .unwrap_or_default()
        } else {
            conn.prepare(&query_sql)
                .and_then(|mut s| {
                    let rows = s.query_map([], |r| {
                        Ok(ApiUser {
                            telegram_id: r.get(0)?,
                            username: r.get(1)?,
                            plan: r.get(2)?,
                            is_blocked: r.get::<_, i64>(3)? != 0,
                            language: r.get(4)?,
                            download_count: r.get(5)?,
                        })
                    })?;
                    Ok(rows.filter_map(|r| r.ok()).collect())
                })
                .unwrap_or_default()
        };

        Ok(PaginatedResponse {
            items: users,
            total,
            page,
            per_page: USERS_PER_PAGE,
            total_pages,
        })
    })
    .await;

    match result {
        Ok(Ok(data)) => Json(data).into_response(),
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "DB error").into_response(),
    }
}

/// POST /admin/api/users/:id/plan — change user plan with optional expiry + notification.
pub(super) async fn admin_api_user_plan(
    State(state): State<WebState>,
    header_map: HeaderMap,
    Path(user_id): Path<i64>,
    Json(body): Json<PlanUpdateReq>,
) -> Response {
    let admin_id = match verify_admin_post(&header_map, &state.bot_token) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let valid_plans = ["free", "premium", "vip"];
    if !valid_plans.contains(&body.plan.as_str()) {
        return (StatusCode::BAD_REQUEST, "Invalid plan").into_response();
    }
    let db = state.shared_storage.sqlite_pool();
    let plan = body.plan.clone();
    let expires_days = body.expires_days;
    let plan_notifier = state.plan_notifier.clone();
    let result = tokio::task::spawn_blocking(move || {
        let conn = get_connection(&db).map_err(|_| rusqlite::Error::InvalidQuery)?;

        // Get old plan for comparison
        let old_plan: String = conn
            .query_row(
                "SELECT COALESCE(plan, 'free') FROM users WHERE telegram_id = ?1",
                rusqlite::params![user_id],
                |r| r.get(0),
            )
            .unwrap_or_else(|_| "free".to_string());

        // Update subscriptions table with optional expiry
        let expires_at = if let Some(days) = expires_days {
            let clamped = days.clamp(1, 3650);
            let expires_expr = format!("datetime('now', '+{} days')", clamped);
            conn.execute(
                &format!(
                    "INSERT OR REPLACE INTO subscriptions (user_id, plan, expires_at) \
                     VALUES (?1, ?2, {})",
                    expires_expr
                ),
                rusqlite::params![user_id, plan],
            )?;
            conn.query_row(
                "SELECT expires_at FROM subscriptions WHERE user_id = ?1",
                rusqlite::params![user_id],
                |r| r.get::<_, Option<String>>(0),
            )
            .ok()
            .flatten()
        } else {
            // No expiry — update or create subscription without expires_at
            conn.execute(
                "INSERT OR REPLACE INTO subscriptions (user_id, plan) VALUES (?1, ?2)",
                rusqlite::params![user_id, plan],
            )?;
            None
        };

        // Update users table
        let n = conn.execute(
            "UPDATE users SET plan = ?1 WHERE telegram_id = ?2",
            rusqlite::params![plan, user_id],
        )?;

        if n > 0 {
            log_audit(
                &conn,
                admin_id,
                "plan_change",
                "user",
                &user_id.to_string(),
                Some(&format!(
                    "plan={}, expires={}",
                    plan,
                    expires_at.as_deref().unwrap_or("unlimited")
                )),
            );
        }
        Ok::<_, rusqlite::Error>((n, old_plan, plan, expires_at))
    })
    .await;

    match result {
        Ok(Ok((0, _, _, _))) => (StatusCode::NOT_FOUND, "User not found").into_response(),
        Ok(Ok((_, old_plan, new_plan, expires_at))) => {
            log::info!(
                "Admin {} changed plan for user {} to {} (expires: {:?})",
                admin_id,
                user_id,
                new_plan,
                expires_at
            );
            // Send plan change notification to user via Telegram
            if old_plan != new_plan {
                if let Some(ref tx) = plan_notifier {
                    use std::str::FromStr;
                    let old = crate::core::Plan::from_str(&old_plan).unwrap_or_default();
                    let new = crate::core::Plan::from_str(&new_plan).unwrap_or_default();
                    let _ = tx.send(crate::core::PlanChangeEvent {
                        user_id,
                        old_plan: old,
                        new_plan: new,
                        reason: crate::core::PlanChangeReason::Admin,
                        expires_at: expires_at.clone(),
                    });
                }
            }
            Json(json!({"ok": true, "plan": new_plan, "expires_at": expires_at})).into_response()
        }
        Ok(Err(e)) => {
            log::error!("Failed to update plan: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "DB error").into_response()
        }
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "DB error").into_response(),
    }
}

/// POST /admin/api/users/:id/block — block/unblock user.
pub(super) async fn admin_api_user_block(
    State(state): State<WebState>,
    header_map: HeaderMap,
    Path(user_id): Path<i64>,
    Json(body): Json<BlockUpdateReq>,
) -> Response {
    let admin_id = match verify_admin_post(&header_map, &state.bot_token) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let db = state.shared_storage.sqlite_pool();
    let blocked = body.blocked;
    let result = tokio::task::spawn_blocking(move || {
        let conn = get_connection(&db).map_err(|_| rusqlite::Error::InvalidQuery)?;
        let blocked_val: i64 = if blocked { 1 } else { 0 };
        let n = conn.execute(
            "UPDATE users SET is_blocked = ?1 WHERE telegram_id = ?2",
            rusqlite::params![blocked_val, user_id],
        )?;
        if n > 0 {
            let action = if blocked { "block" } else { "unblock" };
            log_audit(&conn, admin_id, action, "user", &user_id.to_string(), None);
        }
        Ok::<_, rusqlite::Error>(n)
    })
    .await;

    match result {
        Ok(Ok(0)) => (StatusCode::NOT_FOUND, "User not found").into_response(),
        Ok(Ok(_)) => {
            log::info!(
                "Admin {} {} user {}",
                admin_id,
                if body.blocked { "blocked" } else { "unblocked" },
                user_id
            );
            Json(json!({"ok": true, "blocked": body.blocked})).into_response()
        }
        Ok(Err(e)) => {
            log::error!("Failed to update block status: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "DB error").into_response()
        }
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "DB error").into_response(),
    }
}

/// GET /admin/api/downloads — paginated download history with full details.
pub(super) async fn admin_api_downloads(
    State(state): State<WebState>,
    header_map: HeaderMap,
    Query(q): Query<DownloadQuery>,
) -> Response {
    if let Err(resp) = verify_admin(&header_map, &state.bot_token) {
        return resp;
    }
    let page = q.page.unwrap_or(1).max(1);
    let search = q.search.unwrap_or_default();
    let offset = ((page - 1) * DOWNLOADS_PER_PAGE) as i64;
    let db = state.shared_storage.sqlite_pool();

    let result = tokio::task::spawn_blocking(move || -> Result<PaginatedResponse<ApiDownload>, rusqlite::Error> {
        let conn = get_connection(&db).map_err(|_| rusqlite::Error::InvalidQuery)?;

        let search_param: Option<String> = if search.is_empty() {
            None
        } else {
            Some(like_param(&search))
        };

        let where_clause = if search_param.is_some() {
            "WHERE d.title LIKE ?1 ESCAPE '\\' OR COALESCE(d.author,'') LIKE ?1 ESCAPE '\\' OR COALESCE(u.username,'') LIKE ?1 ESCAPE '\\' OR CAST(d.user_id AS TEXT) LIKE ?1 ESCAPE '\\'"
        } else {
            ""
        };

        let count_sql = format!(
            "SELECT COUNT(*) FROM download_history d LEFT JOIN users u ON u.telegram_id = d.user_id {}",
            where_clause
        );
        let total: i64 = if let Some(ref sp) = search_param {
            conn.query_row(&count_sql, rusqlite::params![sp], |r| r.get(0)).unwrap_or(0)
        } else {
            conn.query_row(&count_sql, [], |r| r.get(0)).unwrap_or(0)
        };

        let total_pages = ((total as f64) / DOWNLOADS_PER_PAGE as f64).ceil() as u32;

        let query_sql = format!(
            "SELECT d.id, COALESCE(d.title, ''), COALESCE(d.author, ''), \
                    COALESCE(u.username, CAST(d.user_id AS TEXT)), d.user_id, \
                    COALESCE(d.format, '?'), d.file_size, d.duration, \
                    COALESCE(d.video_quality, ''), COALESCE(d.audio_bitrate, ''), \
                    d.downloaded_at, COALESCE(d.url, '') \
             FROM download_history d \
             LEFT JOIN users u ON u.telegram_id = d.user_id \
             {} \
             ORDER BY d.downloaded_at DESC \
             LIMIT {} OFFSET {}",
            where_clause, DOWNLOADS_PER_PAGE, offset
        );

        let map_row = |r: &rusqlite::Row<'_>| -> rusqlite::Result<ApiDownload> {
            Ok(ApiDownload {
                id: r.get(0)?,
                title: r.get(1)?,
                author: r.get(2)?,
                user: r.get(3)?,
                user_id: r.get(4)?,
                format: r.get(5)?,
                file_size: r.get(6)?,
                duration: r.get(7)?,
                video_quality: r.get(8)?,
                audio_bitrate: r.get(9)?,
                downloaded_at: r.get(10)?,
                url: r.get(11)?,
            })
        };

        let downloads: Vec<ApiDownload> = if let Some(ref sp) = search_param {
            conn.prepare(&query_sql)
                .and_then(|mut s| {
                    let rows = s.query_map(rusqlite::params![sp], map_row)?;
                    Ok(rows.filter_map(|r| r.ok()).collect())
                })
                .unwrap_or_default()
        } else {
            conn.prepare(&query_sql)
                .and_then(|mut s| {
                    let rows = s.query_map([], map_row)?;
                    Ok(rows.filter_map(|r| r.ok()).collect())
                })
                .unwrap_or_default()
        };

        Ok(PaginatedResponse { items: downloads, total, page, per_page: DOWNLOADS_PER_PAGE, total_pages })
    })
    .await;

    match result {
        Ok(Ok(data)) => Json(data).into_response(),
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "DB error").into_response(),
    }
}

/// GET /admin/api/users/:id — detailed user profile with stats, downloads, charges, errors.
pub(super) async fn admin_api_user_details(
    State(state): State<WebState>,
    header_map: HeaderMap,
    Path(user_id): Path<i64>,
) -> Response {
    if let Err(resp) = verify_admin(&header_map, &state.bot_token) {
        return resp;
    }
    let db = state.shared_storage.sqlite_pool();

    let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, rusqlite::Error> {
        let conn = get_connection(&db).map_err(|_| rusqlite::Error::InvalidQuery)?;

        // User info (extended with preferences)
        let user = conn.query_row(
            "SELECT u.telegram_id, COALESCE(u.username, ''), COALESCE(u.plan, 'free'), \
                    COALESCE(u.is_blocked, 0), COALESCE(u.language, 'ru'), \
                    (SELECT COUNT(*) FROM download_history WHERE user_id = u.telegram_id), \
                    COALESCE(u.download_format, 'mp3'), COALESCE(u.video_quality, 'best'), \
                    COALESCE(u.audio_bitrate, '320k'), COALESCE(u.send_as_document, 0), \
                    COALESCE(u.burn_subtitles, 0), COALESCE(u.progress_bar_style, 'classic') \
             FROM users u WHERE u.telegram_id = ?1",
            rusqlite::params![user_id],
            |r| {
                Ok(json!({
                    "telegram_id": r.get::<_, i64>(0)?,
                    "username": r.get::<_, String>(1)?,
                    "plan": r.get::<_, String>(2)?,
                    "is_blocked": r.get::<_, i64>(3)? != 0,
                    "language": r.get::<_, String>(4)?,
                    "download_count": r.get::<_, i64>(5)?,
                    "download_format": r.get::<_, String>(6)?,
                    "video_quality": r.get::<_, String>(7)?,
                    "audio_bitrate": r.get::<_, String>(8)?,
                    "send_as_document": r.get::<_, i64>(9)? != 0,
                    "burn_subtitles": r.get::<_, i64>(10)? != 0,
                    "progress_bar_style": r.get::<_, String>(11)?,
                }))
            },
        );
        let user = match user {
            Ok(u) => u,
            Err(_) => return Ok(json!({"error": "not_found"})),
        };

        // Subscription
        let sub = conn
            .query_row(
                "SELECT COALESCE(plan, 'free'), COALESCE(expires_at, ''), \
                    COALESCE(telegram_charge_id, ''), COALESCE(is_recurring, 0) \
             FROM subscriptions WHERE user_id = ?1",
                rusqlite::params![user_id],
                |r| {
                    Ok(json!({
                        "plan": r.get::<_, String>(0)?,
                        "expires_at": r.get::<_, String>(1)?,
                        "charge_id": r.get::<_, String>(2)?,
                        "is_recurring": r.get::<_, i64>(3)? != 0,
                    }))
                },
            )
            .ok();

        // Stats
        let total_downloads: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM download_history WHERE user_id = ?1",
                rusqlite::params![user_id],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let total_size: i64 = conn
            .query_row(
                "SELECT COALESCE(SUM(COALESCE(file_size, 0)), 0) FROM download_history WHERE user_id = ?1",
                rusqlite::params![user_id],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let active_days: i64 = conn
            .query_row(
                "SELECT COUNT(DISTINCT date(downloaded_at)) FROM download_history WHERE user_id = ?1",
                rusqlite::params![user_id],
                |r| r.get(0),
            )
            .unwrap_or(0);

        let top_artists: Vec<serde_json::Value> = conn
            .prepare(
                "SELECT COALESCE(author, 'Unknown'), COUNT(*) FROM download_history \
             WHERE user_id = ?1 AND author IS NOT NULL AND author != '' \
             GROUP BY author ORDER BY COUNT(*) DESC LIMIT 5",
            )
            .and_then(|mut s| {
                let rows = s.query_map(rusqlite::params![user_id], |r| {
                    Ok(json!([r.get::<_, String>(0)?, r.get::<_, i64>(1)?]))
                })?;
                Ok(rows.filter_map(|r| r.ok()).collect())
            })
            .unwrap_or_default();

        let top_formats: Vec<serde_json::Value> = conn
            .prepare(
                "SELECT COALESCE(format, 'unknown'), COUNT(*) FROM download_history \
             WHERE user_id = ?1 GROUP BY format ORDER BY COUNT(*) DESC",
            )
            .and_then(|mut s| {
                let rows = s.query_map(rusqlite::params![user_id], |r| {
                    Ok(json!([r.get::<_, String>(0)?, r.get::<_, i64>(1)?]))
                })?;
                Ok(rows.filter_map(|r| r.ok()).collect())
            })
            .unwrap_or_default();

        // Recent downloads (last 20)
        let downloads: Vec<serde_json::Value> = conn
            .prepare(
                "SELECT COALESCE(title, ''), COALESCE(author, ''), COALESCE(format, ''), \
                    file_size, duration, downloaded_at \
             FROM download_history WHERE user_id = ?1 ORDER BY downloaded_at DESC LIMIT 20",
            )
            .and_then(|mut s| {
                let rows = s.query_map(rusqlite::params![user_id], |r| {
                    Ok(json!({
                        "title": r.get::<_, String>(0)?,
                        "author": r.get::<_, String>(1)?,
                        "format": r.get::<_, String>(2)?,
                        "file_size": r.get::<_, Option<i64>>(3)?,
                        "duration": r.get::<_, Option<i64>>(4)?,
                        "downloaded_at": r.get::<_, String>(5)?,
                    }))
                })?;
                Ok(rows.filter_map(|r| r.ok()).collect())
            })
            .unwrap_or_default();

        // Charges
        let charges: Vec<serde_json::Value> = conn
            .prepare(
                "SELECT COALESCE(plan, ''), total_amount, COALESCE(currency, 'XTR'), \
                    COALESCE(is_recurring, 0), COALESCE(payment_date, '') \
             FROM charges WHERE user_id = ?1 ORDER BY payment_date DESC LIMIT 20",
            )
            .and_then(|mut s| {
                let rows = s.query_map(rusqlite::params![user_id], |r| {
                    Ok(json!({
                        "plan": r.get::<_, String>(0)?,
                        "amount": r.get::<_, i64>(1)?,
                        "currency": r.get::<_, String>(2)?,
                        "is_recurring": r.get::<_, i64>(3)? != 0,
                        "payment_date": r.get::<_, String>(4)?,
                    }))
                })?;
                Ok(rows.filter_map(|r| r.ok()).collect())
            })
            .unwrap_or_default();

        // Recent errors
        let errors: Vec<serde_json::Value> = conn
            .prepare(
                "SELECT COALESCE(timestamp, ''), COALESCE(error_type, ''), COALESCE(error_message, '') \
             FROM error_log WHERE user_id = ?1 ORDER BY timestamp DESC LIMIT 10",
            )
            .and_then(|mut s| {
                let rows = s.query_map(rusqlite::params![user_id], |r| {
                    Ok(json!({
                        "timestamp": r.get::<_, String>(0)?,
                        "error_type": r.get::<_, String>(1)?,
                        "error_message": r.get::<_, String>(2)?,
                    }))
                })?;
                Ok(rows.filter_map(|r| r.ok()).collect())
            })
            .unwrap_or_default();

        Ok(json!({
            "user": user,
            "subscription": sub,
            "stats": {
                "total_downloads": total_downloads,
                "total_size": total_size,
                "active_days": active_days,
                "top_artists": top_artists,
                "top_formats": top_formats,
            },
            "recent_downloads": downloads,
            "charges": charges,
            "errors": errors,
        }))
    })
    .await;

    match result {
        Ok(Ok(ref data)) if data.get("error").is_some() => (StatusCode::NOT_FOUND, "User not found").into_response(),
        Ok(Ok(data)) => Json(data).into_response(),
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "DB error").into_response(),
    }
}

/// POST /admin/api/users/:id/settings — update user settings (language, plan, blocked).
pub(super) async fn admin_api_user_settings(
    State(state): State<WebState>,
    header_map: HeaderMap,
    Path(user_id): Path<i64>,
    Json(body): Json<UserSettingsReq>,
) -> Response {
    let admin_id = match verify_admin_post(&header_map, &state.bot_token) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let plan_notifier = state.plan_notifier.clone();
    let db = state.shared_storage.sqlite_pool();
    let result = tokio::task::spawn_blocking(move || {
        let conn = get_connection(&db).map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut updated = Vec::new();
        let mut plan_change: Option<(String, String, Option<String>)> = None; // (old, new, expires_at)

        if let Some(ref lang) = body.language {
            let valid = ["en", "ru", "fr", "de"];
            if valid.contains(&lang.as_str()) {
                conn.execute(
                    "UPDATE users SET language = ?1 WHERE telegram_id = ?2",
                    rusqlite::params![lang, user_id],
                )?;
                updated.push("language");
            }
        }
        if let Some(ref plan) = body.plan {
            let valid = ["free", "premium", "vip"];
            if valid.contains(&plan.as_str()) {
                // Read old plan before updating
                let old_plan: String = conn
                    .query_row(
                        "SELECT COALESCE(plan, 'free') FROM users WHERE telegram_id = ?1",
                        rusqlite::params![user_id],
                        |r| r.get(0),
                    )
                    .unwrap_or_else(|_| "free".to_string());

                let expires_at = if let Some(days) = body.plan_days {
                    let clamped = days.clamp(1, 3650);
                    let expires = format!("datetime('now', '+{} days')", clamped);
                    conn.execute(
                        &format!(
                            "INSERT OR REPLACE INTO subscriptions (user_id, plan, expires_at) \
                             VALUES (?1, ?2, {})",
                            expires
                        ),
                        rusqlite::params![user_id, plan],
                    )?;
                    let exp_str: Option<String> = conn
                        .query_row(
                            "SELECT expires_at FROM subscriptions WHERE user_id = ?1",
                            rusqlite::params![user_id],
                            |r| r.get(0),
                        )
                        .ok();
                    exp_str
                } else {
                    None
                };
                conn.execute(
                    "UPDATE users SET plan = ?1 WHERE telegram_id = ?2",
                    rusqlite::params![plan, user_id],
                )?;
                updated.push("plan");

                if old_plan != *plan {
                    plan_change = Some((old_plan, plan.clone(), expires_at));
                }
            }
        }
        if let Some(blocked) = body.is_blocked {
            let v: i64 = if blocked { 1 } else { 0 };
            conn.execute(
                "UPDATE users SET is_blocked = ?1 WHERE telegram_id = ?2",
                rusqlite::params![v, user_id],
            )?;
            updated.push("is_blocked");
        }

        if !updated.is_empty() {
            log_audit(
                &conn,
                admin_id,
                "user_settings",
                "user",
                &user_id.to_string(),
                Some(&updated.join(",")),
            );
        }

        Ok::<_, rusqlite::Error>((updated, plan_change))
    })
    .await;

    match result {
        Ok(Ok((updated, _))) if updated.is_empty() => {
            (StatusCode::BAD_REQUEST, "No valid fields to update").into_response()
        }
        Ok(Ok((updated, plan_change))) => {
            log::info!("Admin {} updated user {} settings: {:?}", admin_id, user_id, updated);
            // Emit plan change event for notification
            if let Some((old_plan_str, new_plan_str, expires_at)) = plan_change {
                if let Some(ref tx) = plan_notifier {
                    use std::str::FromStr;
                    let old_plan = crate::core::Plan::from_str(&old_plan_str).unwrap_or_default();
                    let new_plan = crate::core::Plan::from_str(&new_plan_str).unwrap_or_default();
                    let _ = tx.send(crate::core::PlanChangeEvent {
                        user_id,
                        old_plan,
                        new_plan,
                        reason: crate::core::PlanChangeReason::Admin,
                        expires_at,
                    });
                }
            }
            Json(json!({"ok": true, "updated": updated})).into_response()
        }
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "DB error").into_response(),
    }
}
