//! Queue management admin handlers.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};

use crate::storage::get_connection;

use super::auth::{RequireAdmin, RequireAdminPost};
use super::helpers::{like_param, log_audit};
use super::types::*;

const QUEUE_PER_PAGE: u32 = 50;

/// GET /admin/api/queue — paginated task queue with status filter.
pub(super) async fn admin_api_queue(
    _admin: RequireAdmin,
    State(state): State<WebState>,
    Query(q): Query<QueueQuery>,
) -> Response {
    let page = q.page.unwrap_or(1).max(1);
    let status_filter = q.status.unwrap_or_default();
    let search = q.search.unwrap_or_default();
    let offset = ((page - 1) * QUEUE_PER_PAGE) as i64;
    let db = state.shared_storage.sqlite_pool();

    let result = tokio::task::spawn_blocking(move || -> Result<PaginatedResponse<ApiQueueTask>, rusqlite::Error> {
        let conn = get_connection(&db).map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut conditions = Vec::new();
        let search_param = if !search.is_empty() {
            conditions.push(
                "(t.url LIKE ?1 ESCAPE '\\' OR COALESCE(u.username,'') LIKE ?1 ESCAPE '\\' \
                 OR CAST(t.user_id AS TEXT) LIKE ?1 ESCAPE '\\' OR t.id LIKE ?1 ESCAPE '\\')"
                    .to_string(),
            );
            Some(like_param(&search))
        } else {
            None
        };
        match status_filter.as_str() {
            "pending" | "leased" | "processing" | "uploading" | "completed" | "dead_letter" => {
                conditions.push(format!("t.status = '{}'", status_filter));
            }
            "active" => {
                conditions.push("t.status IN ('pending','leased','processing','uploading')".to_string());
            }
            _ => {}
        };
        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        let count_sql = format!(
            "SELECT COUNT(*) FROM task_queue t LEFT JOIN users u ON u.telegram_id = t.user_id {}",
            where_clause
        );
        let total: i64 = if let Some(ref sp) = search_param {
            conn.query_row(&count_sql, rusqlite::params![sp], |r| r.get(0))
                .unwrap_or(0)
        } else {
            conn.query_row(&count_sql, [], |r| r.get(0)).unwrap_or(0)
        };
        let total_pages = ((total as f64) / QUEUE_PER_PAGE as f64).ceil() as u32;

        let sql = format!(
            "SELECT t.id, t.user_id, COALESCE(u.username, ''), COALESCE(t.url, ''), \
                    COALESCE(t.format, ''), COALESCE(t.status, ''), COALESCE(t.error_message, ''), \
                    COALESCE(t.retry_count, 0), COALESCE(t.worker_id, ''), \
                    COALESCE(t.created_at, ''), COALESCE(t.started_at, ''), COALESCE(t.finished_at, '') \
             FROM task_queue t LEFT JOIN users u ON u.telegram_id = t.user_id \
             {} ORDER BY t.created_at DESC LIMIT {} OFFSET {}",
            where_clause, QUEUE_PER_PAGE, offset
        );

        let map_q = |r: &rusqlite::Row<'_>| -> rusqlite::Result<ApiQueueTask> {
            Ok(ApiQueueTask {
                id: r.get(0)?,
                user_id: r.get(1)?,
                username: r.get(2)?,
                url: r.get(3)?,
                format: r.get(4)?,
                status: r.get(5)?,
                error_message: r.get(6)?,
                retry_count: r.get(7)?,
                worker_id: r.get(8)?,
                created_at: r.get(9)?,
                started_at: r.get(10)?,
                finished_at: r.get(11)?,
            })
        };
        let tasks: Vec<ApiQueueTask> = if let Some(ref sp) = search_param {
            conn.prepare(&sql)
                .and_then(|mut s| {
                    let rows = s.query_map(rusqlite::params![sp], map_q)?;
                    Ok(rows.filter_map(|r| r.ok()).collect())
                })
                .unwrap_or_default()
        } else {
            conn.prepare(&sql)
                .and_then(|mut s| {
                    let rows = s.query_map([], map_q)?;
                    Ok(rows.filter_map(|r| r.ok()).collect())
                })
                .unwrap_or_default()
        };

        Ok(PaginatedResponse {
            items: tasks,
            total,
            page,
            per_page: QUEUE_PER_PAGE,
            total_pages,
        })
    })
    .await;

    match result {
        Ok(Ok(data)) => Json(data).into_response(),
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "DB error").into_response(),
    }
}

/// POST /admin/api/queue/:id/retry — retry a dead/failed task.
pub(super) async fn admin_api_queue_retry(
    RequireAdminPost(admin_id): RequireAdminPost,
    State(state): State<WebState>,
    Path(task_id): Path<String>,
) -> Response {
    let db = state.shared_storage.sqlite_pool();
    let tid = task_id.clone();
    let tid2 = task_id.clone();
    let result = tokio::task::spawn_blocking(move || {
        let conn = get_connection(&db).map_err(|_| rusqlite::Error::InvalidQuery)?;
        let n = conn.execute(
            "UPDATE task_queue SET status = 'pending', error_message = NULL, retry_count = 0, \
             worker_id = NULL, leased_at = NULL, lease_expires_at = NULL \
             WHERE id = ?1 AND status IN ('dead_letter', 'failed')",
            rusqlite::params![tid],
        )?;
        if n > 0 {
            log_audit(&conn, admin_id, "retry_task", "task", &tid, None);
        }
        Ok::<_, rusqlite::Error>(n)
    })
    .await;

    match result {
        Ok(Ok(0)) => (StatusCode::NOT_FOUND, "Task not found or not retryable").into_response(),
        Ok(Ok(_)) => {
            log::info!("Admin {} retried task {}", admin_id, tid2);
            Json(OkResponse::ok()).into_response()
        }
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "DB error").into_response(),
    }
}

/// POST /admin/api/queue/:id/cancel — cancel a pending/leased task.
pub(super) async fn admin_api_queue_cancel(
    RequireAdminPost(admin_id): RequireAdminPost,
    State(state): State<WebState>,
    Path(task_id): Path<String>,
) -> Response {
    let db = state.shared_storage.sqlite_pool();
    let tid = task_id.clone();
    let result = tokio::task::spawn_blocking(move || {
        let conn = get_connection(&db).map_err(|_| rusqlite::Error::InvalidQuery)?;
        let n = conn.execute(
            "UPDATE task_queue SET status = 'dead_letter', error_message = 'Cancelled by admin' \
             WHERE id = ?1 AND status IN ('pending', 'leased')",
            rusqlite::params![tid],
        )?;
        if n > 0 {
            log_audit(&conn, admin_id, "cancel_task", "task", &tid, None);
        }
        Ok::<_, rusqlite::Error>(n)
    })
    .await;

    match result {
        Ok(Ok(0)) => (StatusCode::NOT_FOUND, "Task not found or not cancellable").into_response(),
        Ok(Ok(_)) => {
            log::info!("Admin {} cancelled task {}", admin_id, task_id);
            Json(OkResponse::ok()).into_response()
        }
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "DB error").into_response(),
    }
}

/// POST /admin/api/queue/bulk-cancel — cancel all pending/leased tasks.
pub(super) async fn admin_api_queue_bulk_cancel(
    RequireAdminPost(admin_id): RequireAdminPost,
    State(state): State<WebState>,
    Json(body): Json<BulkCancelReq>,
) -> Response {
    let db = state.shared_storage.sqlite_pool();
    let status_filter = body.status.unwrap_or_else(|| "pending".to_string());
    let result = tokio::task::spawn_blocking(move || {
        let conn = get_connection(&db).map_err(|_| rusqlite::Error::InvalidQuery)?;
        let valid = ["pending", "leased"];
        if !valid.contains(&status_filter.as_str()) {
            return Ok(0);
        }
        let n = conn.execute(
            "UPDATE task_queue SET status = 'dead_letter', error_message = 'Bulk cancelled by admin' \
             WHERE status = ?1",
            rusqlite::params![&status_filter],
        )?;
        log_audit(
            &conn,
            admin_id,
            "bulk_cancel",
            "task",
            &status_filter,
            Some(&format!("count={}", n)),
        );
        Ok::<_, rusqlite::Error>(n)
    })
    .await;

    match result {
        Ok(Ok(n)) => {
            log::info!("Admin {} bulk-cancelled {} tasks", admin_id, n);
            Json(BulkCountOk::new("cancelled", n as i64)).into_response()
        }
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "DB error").into_response(),
    }
}
