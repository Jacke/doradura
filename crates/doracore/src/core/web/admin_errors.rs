//! Error management admin handlers.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use secrecy::ExposeSecret;
use serde_json::json;

use crate::storage::get_connection;
use crate::storage::shared::QueueTaskInput;

// `json!` is still used by `send_telegram_message` below for the outgoing
// Telegram Bot API payload; only the response-side `json!` usages were
// replaced with typed structs.
use super::auth::{RequireAdmin, RequireAdminPost};
use super::helpers::{like_param, log_audit};
use super::types::*;

const ERRORS_PER_PAGE: u32 = 50;

/// GET /admin/api/errors — paginated, filterable error log.
pub(super) async fn admin_api_errors(
    _admin: RequireAdmin,
    State(state): State<WebState>,
    Query(q): Query<ErrorQuery>,
) -> Response {
    let page = q.page.unwrap_or(1).max(1);
    let type_filter = q.error_type.unwrap_or_default();
    let resolved_filter = q.resolved.unwrap_or_default();
    let search_filter = q.search.unwrap_or_default();
    let offset = ((page - 1) * ERRORS_PER_PAGE) as i64;
    let db = state.shared_storage.sqlite_pool();

    let result = tokio::task::spawn_blocking(move || -> Result<PaginatedResponse<ApiError>, rusqlite::Error> {
        let conn = get_connection(&db).map_err(|_| rusqlite::Error::InvalidQuery)?;

        let mut conditions = Vec::new();
        let search_param = if !type_filter.is_empty() {
            conditions.push("error_type LIKE ?1 ESCAPE '\\'".to_string());
            Some(like_param(&type_filter))
        } else if !search_filter.is_empty() {
            conditions.push(
                "(error_message LIKE ?1 ESCAPE '\\' OR COALESCE(error_type,'') LIKE ?1 ESCAPE '\\' \
                 OR COALESCE(url,'') LIKE ?1 ESCAPE '\\' OR CAST(COALESCE(user_id,0) AS TEXT) LIKE ?1 ESCAPE '\\')"
                    .to_string(),
            );
            Some(like_param(&search_filter))
        } else {
            None
        };
        match resolved_filter.as_str() {
            "yes" => conditions.push("resolved = 1".to_string()),
            "no" => conditions.push("COALESCE(resolved, 0) = 0".to_string()),
            _ => {}
        }
        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        let count_sql = format!("SELECT COUNT(*) FROM error_log {}", where_clause);
        let total: i64 = if let Some(ref sp) = search_param {
            conn.query_row(&count_sql, rusqlite::params![sp], |r| r.get(0))
                .unwrap_or(0)
        } else {
            conn.query_row(&count_sql, [], |r| r.get(0)).unwrap_or(0)
        };
        let total_pages = ((total as f64) / ERRORS_PER_PAGE as f64).ceil() as u32;

        let sql = format!(
            "SELECT id, COALESCE(timestamp, ''), user_id, COALESCE(username, ''), \
                    COALESCE(error_type, ''), COALESCE(error_message, ''), COALESCE(url, ''), \
                    COALESCE(context, ''), COALESCE(resolved, 0) \
             FROM error_log {} ORDER BY timestamp DESC LIMIT {} OFFSET {}",
            where_clause, ERRORS_PER_PAGE, offset
        );

        let map_row = |r: &rusqlite::Row<'_>| -> rusqlite::Result<ApiError> {
            Ok(ApiError {
                id: r.get(0)?,
                timestamp: r.get(1)?,
                user_id: r.get(2)?,
                username: r.get(3)?,
                error_type: r.get(4)?,
                error_message: r.get(5)?,
                url: r.get(6)?,
                context: r.get(7)?,
                resolved: r.get::<_, i64>(8)? != 0,
            })
        };

        let errors: Vec<ApiError> = if let Some(ref sp) = search_param {
            conn.prepare(&sql)
                .and_then(|mut s| {
                    let rows = s.query_map(rusqlite::params![sp], map_row)?;
                    Ok(rows.filter_map(|r| r.ok()).collect())
                })
                .unwrap_or_default()
        } else {
            conn.prepare(&sql)
                .and_then(|mut s| {
                    let rows = s.query_map([], map_row)?;
                    Ok(rows.filter_map(|r| r.ok()).collect())
                })
                .unwrap_or_default()
        };

        Ok(PaginatedResponse {
            items: errors,
            total,
            page,
            per_page: ERRORS_PER_PAGE,
            total_pages,
        })
    })
    .await;

    match result {
        Ok(Ok(data)) => Json(data).into_response(),
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "DB error").into_response(),
    }
}

/// POST /admin/api/errors/:id/resolve — mark error as resolved.
pub(super) async fn admin_api_error_resolve(
    RequireAdminPost(admin_id): RequireAdminPost,
    State(state): State<WebState>,
    Path(error_id): Path<i64>,
) -> Response {
    let db = state.shared_storage.sqlite_pool();
    let result = tokio::task::spawn_blocking(move || {
        let conn = get_connection(&db).map_err(|_| rusqlite::Error::InvalidQuery)?;
        let n = conn.execute(
            "UPDATE error_log SET resolved = 1 WHERE id = ?1",
            rusqlite::params![error_id],
        )?;
        if n > 0 {
            log_audit(&conn, admin_id, "resolve_error", "error", &error_id.to_string(), None);
        }
        Ok::<_, rusqlite::Error>(n)
    })
    .await;

    match result {
        Ok(Ok(0)) => (StatusCode::NOT_FOUND, "Error not found").into_response(),
        Ok(Ok(_)) => {
            log::info!("Admin {} resolved error {}", admin_id, error_id);
            Json(OkResponse::ok()).into_response()
        }
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "DB error").into_response(),
    }
}

/// POST /admin/api/errors/:id/retry — re-queue the failed download for the affected user.
///
/// Flow:
///   1. Load the error_log row and extract user_id + url.
///   2. Fetch the user's preferred format / quality / bitrate from the users table.
///   3. Enqueue a new download task via save_task_to_queue (new idempotency key
///      so it bypasses the old failed-task guard).
///   4. Mark the error_log row as resolved.
///   5. Send the user a Telegram message that their download has been re-queued.
pub(super) async fn admin_api_error_retry(
    RequireAdminPost(admin_id): RequireAdminPost,
    State(state): State<WebState>,
    Path(error_id): Path<i64>,
) -> Response {
    // Step 1 + 2: load error row + user preferences in the blocking pool.
    let db = state.shared_storage.sqlite_pool();
    let loaded = tokio::task::spawn_blocking(move || -> Result<Option<RetryContext>, rusqlite::Error> {
        let conn = get_connection(&db).map_err(|_| rusqlite::Error::InvalidQuery)?;
        let row: Option<(Option<i64>, Option<String>)> = conn
            .query_row(
                "SELECT user_id, url FROM error_log WHERE id = ?1",
                rusqlite::params![error_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .ok();
        let (user_id, url) = match row {
            Some((Some(uid), Some(url))) if !url.is_empty() => (uid, url),
            _ => return Ok(None),
        };

        // Fetch the user's current download preferences.
        let prefs: (String, String, String) = conn
            .query_row(
                "SELECT COALESCE(download_format, 'mp3'), \
                        COALESCE(video_quality, 'best'), \
                        COALESCE(audio_bitrate, '320k') \
                 FROM users WHERE telegram_id = ?1",
                rusqlite::params![user_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap_or_else(|_| ("mp3".into(), "best".into(), "320k".into()));

        Ok(Some(RetryContext {
            user_id,
            url,
            format: prefs.0,
            video_quality: prefs.1,
            audio_bitrate: prefs.2,
        }))
    })
    .await;

    let ctx = match loaded {
        Ok(Ok(Some(ctx))) => ctx,
        Ok(Ok(None)) => return (StatusCode::NOT_FOUND, "Error has no user/URL context").into_response(),
        _ => return (StatusCode::INTERNAL_SERVER_ERROR, "DB error").into_response(),
    };

    // Step 3: enqueue a new download task.
    let task_id = uuid::Uuid::new_v4().to_string();
    let idempotency_key = format!("admin_retry_err_{}_{}", error_id, task_id);
    let is_video = ctx.format == "mp4";
    let input = QueueTaskInput {
        task_id: &task_id,
        user_id: ctx.user_id,
        url: &ctx.url,
        message_id: None,
        format: &ctx.format,
        is_video,
        video_quality: if is_video {
            Some(ctx.video_quality.as_str())
        } else {
            None
        },
        audio_bitrate: if !is_video {
            Some(ctx.audio_bitrate.as_str())
        } else {
            None
        },
        time_range_start: None,
        time_range_end: None,
        carousel_mask: None,
        priority: 10, // higher than default so admin retries jump the queue
        idempotency_key: &idempotency_key,
    };

    if let Err(e) = state.shared_storage.save_task_to_queue(input).await {
        log::error!("Admin retry enqueue failed for error {}: {}", error_id, e);
        return (StatusCode::INTERNAL_SERVER_ERROR, "enqueue failed").into_response();
    }

    // Step 4: mark the error resolved + audit.
    let db2 = state.shared_storage.sqlite_pool();
    let err_id = error_id;
    let admin = admin_id;
    let _ = tokio::task::spawn_blocking(move || {
        if let Ok(conn) = get_connection(&db2) {
            let _ = conn.execute(
                "UPDATE error_log SET resolved = 1 WHERE id = ?1",
                rusqlite::params![err_id],
            );
            log_audit(&conn, admin, "retry_task", "error", &err_id.to_string(), None);
        }
    })
    .await;

    // Step 5: DM the user.
    let user_text = format!(
        "✅ Good news! The admin has retried your failed download:\n\n{}\n\nYour file will arrive shortly.",
        ctx.url
    );
    if let Err(e) = send_telegram_message(state.bot_token.expose_secret(), ctx.user_id, &user_text).await {
        log::warn!("Failed to notify user {} of retry: {}", ctx.user_id, e);
    }

    log::info!(
        "Admin {} retried failed download (error_id={}, user={}, url={})",
        admin_id,
        error_id,
        ctx.user_id,
        ctx.url
    );
    Json(RetryOk::new(task_id, ctx.user_id)).into_response()
}

/// POST /admin/api/errors/:id/notify — send a message to the affected user that
/// their issue has been addressed (without re-queuing the task).
pub(super) async fn admin_api_error_notify(
    RequireAdminPost(admin_id): RequireAdminPost,
    State(state): State<WebState>,
    Path(error_id): Path<i64>,
    Json(body): Json<NotifyUserReq>,
) -> Response {
    // Look up user_id.
    let db = state.shared_storage.sqlite_pool();
    let user_id: Option<i64> = tokio::task::spawn_blocking(move || {
        let conn = get_connection(&db).ok()?;
        conn.query_row(
            "SELECT user_id FROM error_log WHERE id = ?1",
            rusqlite::params![error_id],
            |r| r.get::<_, Option<i64>>(0),
        )
        .ok()
        .flatten()
    })
    .await
    .ok()
    .flatten();

    let Some(user_id) = user_id else {
        return (StatusCode::NOT_FOUND, "Error has no associated user").into_response();
    };

    let text = if body.message.trim().is_empty() {
        "✅ The issue with your recent download has been resolved. Feel free to try again!".to_string()
    } else {
        body.message.clone()
    };

    if let Err(e) = send_telegram_message(state.bot_token.expose_secret(), user_id, &text).await {
        log::error!("Failed to notify user {} about error {}: {}", user_id, error_id, e);
        return (StatusCode::BAD_GATEWAY, "failed to send message").into_response();
    }

    // Audit log + mark resolved if requested.
    let db2 = state.shared_storage.sqlite_pool();
    let eid = error_id;
    let admin = admin_id;
    let mark_resolved = body.mark_resolved;
    let _ = tokio::task::spawn_blocking(move || {
        if let Ok(conn) = get_connection(&db2) {
            if mark_resolved {
                let _ = conn.execute(
                    "UPDATE error_log SET resolved = 1 WHERE id = ?1",
                    rusqlite::params![eid],
                );
            }
            log_audit(&conn, admin, "notify_user", "error", &eid.to_string(), None);
        }
    })
    .await;

    Json(NotifyOk::new(user_id)).into_response()
}

/// Context loaded from the error_log row for retry.
struct RetryContext {
    user_id: i64,
    url: String,
    format: String,
    video_quality: String,
    audio_bitrate: String,
}

/// Minimal direct Telegram Bot API call — we can't depend on teloxide inside
/// doracore, so we use reqwest against the official endpoint.
async fn send_telegram_message(bot_token: &str, chat_id: i64, text: &str) -> anyhow::Result<()> {
    let url = format!("https://api.telegram.org/bot{}/sendMessage", bot_token);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()?;
    let resp = client
        .post(&url)
        .json(&json!({
            "chat_id": chat_id,
            "text": text,
            "disable_web_page_preview": true,
        }))
        .send()
        .await?;
    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("telegram {}: {}", body.chars().take(200).collect::<String>(), "");
    }
    Ok(())
}

/// POST /admin/api/errors/bulk-resolve — resolve all unresolved errors, optionally by type.
pub(super) async fn admin_api_errors_bulk_resolve(
    RequireAdminPost(admin_id): RequireAdminPost,
    State(state): State<WebState>,
    Json(body): Json<BulkResolveReq>,
) -> Response {
    let db = state.shared_storage.sqlite_pool();
    let error_type = body.error_type.clone();
    let result = tokio::task::spawn_blocking(move || {
        let conn = get_connection(&db).map_err(|_| rusqlite::Error::InvalidQuery)?;
        let n = if let Some(ref et) = error_type {
            conn.execute(
                "UPDATE error_log SET resolved = 1 WHERE COALESCE(resolved, 0) = 0 AND error_type = ?1",
                rusqlite::params![et],
            )?
        } else {
            conn.execute("UPDATE error_log SET resolved = 1 WHERE COALESCE(resolved, 0) = 0", [])?
        };
        log_audit(
            &conn,
            admin_id,
            "bulk_resolve",
            "error",
            error_type.as_deref().unwrap_or("all"),
            Some(&format!("count={}", n)),
        );
        Ok::<_, rusqlite::Error>(n)
    })
    .await;

    match result {
        Ok(Ok(n)) => {
            log::info!("Admin {} bulk-resolved {} errors", admin_id, n);
            Json(BulkCountOk::new("resolved", n as i64)).into_response()
        }
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "DB error").into_response(),
    }
}
