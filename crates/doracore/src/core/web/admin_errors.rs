//! Error management admin handlers.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use serde_json::json;

use crate::storage::get_connection;

use super::auth::{verify_admin, verify_admin_post};
use super::helpers::{like_param, log_audit};
use super::types::*;

const ERRORS_PER_PAGE: u32 = 50;

/// GET /admin/api/errors — paginated, filterable error log.
pub(super) async fn admin_api_errors(
    State(state): State<WebState>,
    header_map: HeaderMap,
    Query(q): Query<ErrorQuery>,
) -> Response {
    if let Err(resp) = verify_admin(&header_map, &state) {
        return resp;
    }
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
    State(state): State<WebState>,
    header_map: HeaderMap,
    Path(error_id): Path<i64>,
) -> Response {
    let admin_id = match verify_admin_post(&header_map, &state) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
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
            Json(json!({"ok": true})).into_response()
        }
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "DB error").into_response(),
    }
}

/// POST /admin/api/errors/bulk-resolve — resolve all unresolved errors, optionally by type.
pub(super) async fn admin_api_errors_bulk_resolve(
    State(state): State<WebState>,
    header_map: HeaderMap,
    Json(body): Json<BulkResolveReq>,
) -> Response {
    let admin_id = match verify_admin_post(&header_map, &state) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
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
            Json(json!({"ok": true, "resolved": n})).into_response()
        }
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "DB error").into_response(),
    }
}
