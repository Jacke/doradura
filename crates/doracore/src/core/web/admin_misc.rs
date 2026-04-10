//! Miscellaneous admin handlers: feedback, alerts, health, broadcast,
//! revenue, analytics, audit log, content subscriptions, and tab-badge counts.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use serde_json::json;

use crate::storage::get_connection;

use super::auth::{RequireAdmin, RequireAdminPost};
use super::helpers::{like_param, log_audit};
use super::types::*;

// ---------------------------------------------------------------------------
// Feedback API
// ---------------------------------------------------------------------------

const FEEDBACK_PER_PAGE: u32 = 50;

/// GET /admin/api/feedback — paginated feedback messages.
pub(super) async fn admin_api_feedback(
    _admin: RequireAdmin,
    State(state): State<WebState>,
    Query(q): Query<FeedbackQuery>,
) -> Response {
    let page = q.page.unwrap_or(1).max(1);
    let status_filter = q.status.unwrap_or_default();
    let search = q.search.unwrap_or_default();
    let offset = ((page - 1) * FEEDBACK_PER_PAGE) as i64;
    let db = state.shared_storage.sqlite_pool();

    let result = tokio::task::spawn_blocking(move || -> Result<PaginatedResponse<ApiFeedback>, rusqlite::Error> {
        let conn = get_connection(&db).map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut conditions = Vec::new();
        match status_filter.as_str() {
            "new" | "reviewed" | "replied" => {
                conditions.push(format!("status = '{}'", status_filter));
            }
            _ => {}
        }
        let search_param = if !search.is_empty() {
            conditions.push(
                "(message LIKE ?1 ESCAPE '\\' OR COALESCE(username,'') LIKE ?1 ESCAPE '\\' OR COALESCE(first_name,'') LIKE ?1 ESCAPE '\\')".to_string(),
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

        let count_sql = format!("SELECT COUNT(*) FROM feedback_messages {}", where_clause);
        let total: i64 = if let Some(ref sp) = search_param {
            conn.query_row(&count_sql, rusqlite::params![sp], |r| r.get(0))
                .unwrap_or(0)
        } else {
            conn.query_row(&count_sql, [], |r| r.get(0)).unwrap_or(0)
        };
        let total_pages = ((total as f64) / FEEDBACK_PER_PAGE as f64).ceil() as u32;

        let sql = format!(
            "SELECT id, user_id, COALESCE(username, ''), COALESCE(first_name, ''), \
                    message, status, COALESCE(admin_reply, ''), created_at \
             FROM feedback_messages {} ORDER BY created_at DESC LIMIT {} OFFSET {}",
            where_clause, FEEDBACK_PER_PAGE, offset
        );

        let map_f = |r: &rusqlite::Row<'_>| -> rusqlite::Result<ApiFeedback> {
            Ok(ApiFeedback {
                id: r.get(0)?,
                user_id: r.get(1)?,
                username: r.get(2)?,
                first_name: r.get(3)?,
                message: r.get(4)?,
                status: r.get(5)?,
                admin_reply: r.get(6)?,
                created_at: r.get(7)?,
            })
        };
        let feedback: Vec<ApiFeedback> = if let Some(ref sp) = search_param {
            conn.prepare(&sql)
                .and_then(|mut s| {
                    let rows = s.query_map(rusqlite::params![sp], map_f)?;
                    Ok(rows.filter_map(|r| r.ok()).collect())
                })
                .unwrap_or_default()
        } else {
            conn.prepare(&sql)
                .and_then(|mut s| {
                    let rows = s.query_map([], map_f)?;
                    Ok(rows.filter_map(|r| r.ok()).collect())
                })
                .unwrap_or_default()
        };

        Ok(PaginatedResponse {
            items: feedback,
            total,
            page,
            per_page: FEEDBACK_PER_PAGE,
            total_pages,
        })
    })
    .await;

    match result {
        Ok(Ok(data)) => Json(data).into_response(),
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "DB error").into_response(),
    }
}

/// POST /admin/api/feedback/:id/status — update feedback status.
pub(super) async fn admin_api_feedback_status(
    RequireAdminPost(admin_id): RequireAdminPost,
    State(state): State<WebState>,
    Path(feedback_id): Path<i64>,
    Json(body): Json<FeedbackStatusReq>,
) -> Response {
    let valid = ["new", "reviewed", "replied"];
    if !valid.contains(&body.status.as_str()) {
        return (StatusCode::BAD_REQUEST, "Invalid status").into_response();
    }
    let db = state.shared_storage.sqlite_pool();
    let status = body.status.clone();
    let status2 = body.status.clone();
    let result = tokio::task::spawn_blocking(move || {
        let conn = get_connection(&db).map_err(|_| rusqlite::Error::InvalidQuery)?;
        let n = conn.execute(
            "UPDATE feedback_messages SET status = ?1 WHERE id = ?2",
            rusqlite::params![status, feedback_id],
        )?;
        if n > 0 {
            log_audit(
                &conn,
                admin_id,
                "feedback_status",
                "feedback",
                &feedback_id.to_string(),
                Some(&status),
            );
        }
        Ok::<_, rusqlite::Error>(n)
    })
    .await;

    match result {
        Ok(Ok(0)) => (StatusCode::NOT_FOUND, "Feedback not found").into_response(),
        Ok(Ok(_)) => {
            log::info!("Admin {} updated feedback {} to {}", admin_id, feedback_id, status2);
            Json(json!({"ok": true, "status": body.status})).into_response()
        }
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "DB error").into_response(),
    }
}

// ---------------------------------------------------------------------------
// Alerts API
// ---------------------------------------------------------------------------

const ALERTS_PER_PAGE: u32 = 50;

/// GET /admin/api/alerts — paginated alert history.
pub(super) async fn admin_api_alerts(
    _admin: RequireAdmin,
    State(state): State<WebState>,
    Query(q): Query<AlertQuery>,
) -> Response {
    let page = q.page.unwrap_or(1).max(1);
    let severity_filter = q.severity.unwrap_or_default();
    let search = q.search.unwrap_or_default();
    let offset = ((page - 1) * ALERTS_PER_PAGE) as i64;
    let db = state.shared_storage.sqlite_pool();

    let result = tokio::task::spawn_blocking(move || -> Result<PaginatedResponse<ApiAlert>, rusqlite::Error> {
        let conn = get_connection(&db).map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut conditions = Vec::new();
        match severity_filter.as_str() {
            "critical" | "warning" | "info" => {
                conditions.push(format!("severity = '{}'", severity_filter));
            }
            "unresolved" => conditions.push("resolved_at IS NULL".to_string()),
            "unacked" => conditions.push("COALESCE(acknowledged, 0) = 0".to_string()),
            _ => {}
        }
        let search_param = if !search.is_empty() {
            conditions.push("(COALESCE(alert_type,'') LIKE ?1 ESCAPE '\\' OR message LIKE ?1 ESCAPE '\\')".to_string());
            Some(like_param(&search))
        } else {
            None
        };
        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        let count_sql = format!("SELECT COUNT(*) FROM alert_history {}", where_clause);
        let total: i64 = if let Some(ref sp) = search_param {
            conn.query_row(&count_sql, rusqlite::params![sp], |r| r.get(0))
                .unwrap_or(0)
        } else {
            conn.query_row(&count_sql, [], |r| r.get(0)).unwrap_or(0)
        };
        let total_pages = ((total as f64) / ALERTS_PER_PAGE as f64).ceil() as u32;

        let sql = format!(
            "SELECT id, COALESCE(alert_type, ''), COALESCE(severity, ''), \
                    COALESCE(message, ''), COALESCE(metadata, ''), \
                    COALESCE(triggered_at, ''), COALESCE(resolved_at, ''), \
                    COALESCE(acknowledged, 0) \
             FROM alert_history {} ORDER BY triggered_at DESC LIMIT {} OFFSET {}",
            where_clause, ALERTS_PER_PAGE, offset
        );

        let map_a = |r: &rusqlite::Row<'_>| -> rusqlite::Result<ApiAlert> {
            Ok(ApiAlert {
                id: r.get(0)?,
                alert_type: r.get(1)?,
                severity: r.get(2)?,
                message: r.get(3)?,
                metadata: r.get(4)?,
                triggered_at: r.get(5)?,
                resolved_at: r.get(6)?,
                acknowledged: r.get::<_, i64>(7)? != 0,
            })
        };
        let alerts: Vec<ApiAlert> = if let Some(ref sp) = search_param {
            conn.prepare(&sql)
                .and_then(|mut s| {
                    let rows = s.query_map(rusqlite::params![sp], map_a)?;
                    Ok(rows.filter_map(|r| r.ok()).collect())
                })
                .unwrap_or_default()
        } else {
            conn.prepare(&sql)
                .and_then(|mut s| {
                    let rows = s.query_map([], map_a)?;
                    Ok(rows.filter_map(|r| r.ok()).collect())
                })
                .unwrap_or_default()
        };

        Ok(PaginatedResponse {
            items: alerts,
            total,
            page,
            per_page: ALERTS_PER_PAGE,
            total_pages,
        })
    })
    .await;

    match result {
        Ok(Ok(data)) => Json(data).into_response(),
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "DB error").into_response(),
    }
}

/// POST /admin/api/alerts/:id/acknowledge — acknowledge an alert.
pub(super) async fn admin_api_alert_acknowledge(
    RequireAdminPost(admin_id): RequireAdminPost,
    State(state): State<WebState>,
    Path(alert_id): Path<i64>,
) -> Response {
    let db = state.shared_storage.sqlite_pool();
    let result = tokio::task::spawn_blocking(move || {
        let conn = get_connection(&db).map_err(|_| rusqlite::Error::InvalidQuery)?;
        let n = conn.execute(
            "UPDATE alert_history SET acknowledged = 1, acknowledged_at = datetime('now') WHERE id = ?1",
            rusqlite::params![alert_id],
        )?;
        if n > 0 {
            log_audit(&conn, admin_id, "ack_alert", "alert", &alert_id.to_string(), None);
        }
        Ok::<_, rusqlite::Error>(n)
    })
    .await;

    match result {
        Ok(Ok(0)) => (StatusCode::NOT_FOUND, "Alert not found").into_response(),
        Ok(Ok(_)) => {
            log::info!("Admin {} acknowledged alert {}", admin_id, alert_id);
            Json(json!({"ok": true})).into_response()
        }
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "DB error").into_response(),
    }
}

// ---------------------------------------------------------------------------
// Health API
// ---------------------------------------------------------------------------

/// GET /admin/api/health — system health overview.
pub(super) async fn admin_api_health(_admin: RequireAdmin, State(state): State<WebState>) -> Response {
    let db = state.shared_storage.sqlite_pool();

    let result = tokio::task::spawn_blocking(move || -> serde_json::Value {
        let ytdlp_version = std::process::Command::new("yt-dlp")
            .arg("--version")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| "not found".to_string());

        let conn = match get_connection(&db) {
            Ok(c) => c,
            Err(_) => return json!({"ytdlp_version": ytdlp_version, "db_error": true}),
        };

        // Queue breakdown by status
        let mut queue = serde_json::Map::new();
        if let Ok(mut stmt) =
            conn.prepare("SELECT COALESCE(status, 'unknown'), COUNT(*) FROM task_queue GROUP BY status")
        {
            if let Ok(rows) = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))) {
                for row in rows.flatten() {
                    queue.insert(row.0, json!(row.1));
                }
            }
        }

        // Error rate last 24h by type
        let mut error_types = serde_json::Map::new();
        let errors_24h: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM error_log WHERE timestamp >= datetime('now', '-24 hours')",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        if let Ok(mut stmt) = conn.prepare(
            "SELECT COALESCE(error_type, 'unknown'), COUNT(*) FROM error_log \
             WHERE timestamp >= datetime('now', '-24 hours') GROUP BY error_type ORDER BY COUNT(*) DESC LIMIT 10",
        ) {
            if let Ok(rows) = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))) {
                for row in rows.flatten() {
                    error_types.insert(row.0, json!(row.1));
                }
            }
        }

        // Unacked alerts & unread feedback
        let unacked_alerts: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM alert_history WHERE COALESCE(acknowledged, 0) = 0 AND resolved_at IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);

        let unread_feedback: i64 = conn
            .query_row("SELECT COUNT(*) FROM feedback_messages WHERE status = 'new'", [], |r| {
                r.get(0)
            })
            .unwrap_or(0);

        // DB size
        let db_size: i64 = conn
            .query_row(
                "SELECT page_count * page_size FROM pragma_page_count(), pragma_page_size()",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);

        // Error rate per hour (last 24h, for sparkline)
        let error_hourly: Vec<serde_json::Value> = conn
            .prepare(
                "SELECT strftime('%Y-%m-%d %H:00', timestamp) AS hr, COUNT(*) \
                 FROM error_log WHERE timestamp >= datetime('now', '-24 hours') \
                 GROUP BY hr ORDER BY hr ASC",
            )
            .and_then(|mut s| {
                let rows = s.query_map([], |r| Ok(json!([r.get::<_, String>(0)?, r.get::<_, i64>(1)?])))?;
                Ok(rows.filter_map(|r| r.ok()).collect())
            })
            .unwrap_or_default();

        // Cookies check (single read)
        let cookies_path = std::env::var("COOKIES_FILE").unwrap_or_else(|_| "/data/cookies.txt".to_string());
        let cookies_content = std::fs::read_to_string(&cookies_path).unwrap_or_default();
        let cookies_exist = !cookies_content.is_empty();
        let cookies_count = cookies_content
            .lines()
            .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
            .count();
        let required_cookies = ["APISID", "SAPISID", "HSID", "SID", "SSID"];
        let mut cookies_found = serde_json::Map::new();
        for name in &required_cookies {
            // Match cookie name as a tab-delimited field (Netscape format: domain\tpath\t...\tNAME\tvalue)
            let pattern = format!("\t{}\t", name);
            cookies_found.insert(name.to_string(), json!(cookies_content.contains(&pattern)));
        }

        // WARP proxy check
        let warp_proxy = std::env::var("WARP_PROXY").unwrap_or_default();
        let tcp_timeout = std::time::Duration::from_millis(500);
        let warp_ok = if !warp_proxy.is_empty() {
            std::net::TcpStream::connect_timeout(
                &warp_proxy.parse().unwrap_or_else(|_| "127.0.0.1:1080".parse().unwrap()),
                tcp_timeout,
            )
            .is_ok()
        } else {
            false
        };

        let pot_ok = std::net::TcpStream::connect_timeout(&"127.0.0.1:4416".parse().unwrap(), tcp_timeout).is_ok();

        json!({
            "ytdlp_version": ytdlp_version,
            "queue": queue,
            "errors_24h": errors_24h,
            "error_types": error_types,
            "error_hourly": error_hourly,
            "unacked_alerts": unacked_alerts,
            "unread_feedback": unread_feedback,
            "db_size": db_size,
            "cookies": {
                "exists": cookies_exist,
                "count": cookies_count,
                "required": cookies_found,
            },
            "warp": {
                "configured": !warp_proxy.is_empty(),
                "address": warp_proxy,
                "reachable": warp_ok,
            },
            "pot_server": pot_ok,
        })
    })
    .await;

    match result {
        Ok(data) => Json(data).into_response(),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "Error").into_response(),
    }
}

// ---------------------------------------------------------------------------
// Broadcast API
// ---------------------------------------------------------------------------

/// POST /admin/api/broadcast — send message to one user or broadcast to all.
pub(super) async fn admin_api_broadcast(
    RequireAdminPost(admin_id): RequireAdminPost,
    State(state): State<WebState>,
    Json(body): Json<BroadcastReq>,
) -> Response {
    if body.message.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "Empty message").into_response();
    }
    if body.message.len() > 4096 {
        return (StatusCode::BAD_REQUEST, "Message too long (max 4096)").into_response();
    }

    let bot_token = state.bot_token.clone();

    if body.target != "all" {
        // Send to specific user
        let target_id: i64 = match body.target.parse() {
            Ok(id) => id,
            Err(_) => return (StatusCode::BAD_REQUEST, "Invalid target ID").into_response(),
        };

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("https://api.telegram.org/bot{}/sendMessage", bot_token))
            .json(&json!({"chat_id": target_id, "text": body.message}))
            .send()
            .await;

        match resp {
            Ok(r) if r.status().is_success() => {
                log::info!("Admin {} sent message to user {}", admin_id, target_id);
                let db2 = state.shared_storage.sqlite_pool();
                tokio::task::spawn_blocking(move || {
                    if let Ok(conn) = get_connection(&db2) {
                        log_audit(&conn, admin_id, "send_message", "user", &target_id.to_string(), None);
                    }
                });
                Json(json!({"ok": true, "sent": 1, "blocked": 0, "failed": 0})).into_response()
            }
            Ok(r) => {
                let text = r.text().await.unwrap_or_default();
                if text.contains("Forbidden") || text.contains("blocked") {
                    Json(json!({"ok": true, "sent": 0, "blocked": 1, "failed": 0})).into_response()
                } else {
                    log::warn!("Telegram send error to {}: {}", target_id, text);
                    (StatusCode::BAD_REQUEST, "Telegram error").into_response()
                }
            }
            Err(e) => {
                log::error!("Broadcast request error: {}", e);
                (StatusCode::INTERNAL_SERVER_ERROR, "Request failed").into_response()
            }
        }
    } else {
        // Broadcast to all users — runs in background
        let db = state.shared_storage.sqlite_pool();
        let message = body.message.clone();

        let user_ids: Vec<i64> = tokio::task::spawn_blocking(move || {
            let conn = match get_connection(&db) {
                Ok(c) => c,
                Err(_) => return vec![],
            };
            conn.prepare("SELECT telegram_id FROM users WHERE COALESCE(is_blocked, 0) = 0")
                .and_then(|mut s| {
                    let rows = s.query_map([], |r| r.get::<_, i64>(0))?;
                    Ok(rows.filter_map(|r| r.ok()).collect())
                })
                .unwrap_or_default()
        })
        .await
        .unwrap_or_default();

        let total = user_ids.len();
        log::info!("Admin {} started broadcast to {} users", admin_id, total);
        let db3 = state.shared_storage.sqlite_pool();
        tokio::task::spawn_blocking(move || {
            if let Ok(conn) = get_connection(&db3) {
                log_audit(
                    &conn,
                    admin_id,
                    "broadcast",
                    "broadcast",
                    "all",
                    Some(&format!("total={}", total)),
                );
            }
        });

        // Fire-and-forget background task
        tokio::spawn(async move {
            let client = reqwest::Client::new();
            let (mut sent, mut blocked, mut failed) = (0u32, 0u32, 0u32);
            for uid in &user_ids {
                if *uid == admin_id {
                    continue;
                }
                let resp = client
                    .post(format!("https://api.telegram.org/bot{}/sendMessage", bot_token))
                    .json(&json!({"chat_id": uid, "text": message}))
                    .send()
                    .await;
                match resp {
                    Ok(r) if r.status().is_success() => sent += 1,
                    Ok(r) => {
                        let text = r.text().await.unwrap_or_default();
                        if text.contains("Forbidden") || text.contains("blocked") || text.contains("not found") {
                            blocked += 1;
                        } else {
                            failed += 1;
                        }
                    }
                    Err(_) => failed += 1,
                }
                tokio::time::sleep(std::time::Duration::from_millis(35)).await;
            }
            log::info!("Broadcast done: sent={}, blocked={}, failed={}", sent, blocked, failed);
        });

        Json(json!({"ok": true, "total": total, "status": "broadcasting"})).into_response()
    }
}

// ---------------------------------------------------------------------------
// Revenue API
// ---------------------------------------------------------------------------

const REVENUE_PER_PAGE: u32 = 50;

/// GET /admin/api/revenue — paginated charges with aggregate stats.
pub(super) async fn admin_api_revenue(
    _admin: RequireAdmin,
    State(state): State<WebState>,
    Query(q): Query<RevenueQuery>,
) -> Response {
    let page = q.page.unwrap_or(1).max(1);
    let plan_filter = q.plan.unwrap_or_default();
    let offset = ((page - 1) * REVENUE_PER_PAGE) as i64;
    let db = state.shared_storage.sqlite_pool();

    let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, rusqlite::Error> {
        let conn = get_connection(&db).map_err(|_| rusqlite::Error::InvalidQuery)?;

        // Aggregate stats
        let (total_charges, total_amount, premium_count, vip_count, recurring_count): (i64, i64, i64, i64, i64) = conn
            .query_row(
                "SELECT COUNT(*), COALESCE(SUM(total_amount),0), \
                 SUM(CASE WHEN plan='premium' THEN 1 ELSE 0 END), \
                 SUM(CASE WHEN plan='vip' THEN 1 ELSE 0 END), \
                 SUM(CASE WHEN is_recurring=1 THEN 1 ELSE 0 END) \
                 FROM charges",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap_or((0, 0, 0, 0, 0));

        // Revenue per day (last 30 days)
        let revenue_per_day: Vec<serde_json::Value> = conn
            .prepare(
                "SELECT date(payment_date) AS day, SUM(total_amount) AS amt \
                 FROM charges WHERE payment_date >= date('now','-29 days') \
                 GROUP BY day ORDER BY day ASC",
            )
            .and_then(|mut s| {
                let rows = s.query_map([], |r| Ok(json!([r.get::<_, String>(0)?, r.get::<_, i64>(1)?])))?;
                Ok(rows.filter_map(|r| r.ok()).collect())
            })
            .unwrap_or_default();

        // Paginated charges
        let where_clause = match plan_filter.as_str() {
            "premium" | "vip" => format!("WHERE c.plan = '{}'", plan_filter),
            "recurring" => "WHERE c.is_recurring = 1".to_string(),
            _ => String::new(),
        };

        let total: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM charges c {}", where_clause), [], |r| {
                r.get(0)
            })
            .unwrap_or(0);
        let total_pages = ((total as f64) / REVENUE_PER_PAGE as f64).ceil() as u32;

        let sql = format!(
            "SELECT c.id, c.user_id, COALESCE(u.username, ''), COALESCE(c.plan, ''), \
                    c.total_amount, COALESCE(c.currency, 'XTR'), COALESCE(c.is_recurring, 0), \
                    COALESCE(c.payment_date, '') \
             FROM charges c LEFT JOIN users u ON u.telegram_id = c.user_id \
             {} ORDER BY c.payment_date DESC LIMIT {} OFFSET {}",
            where_clause, REVENUE_PER_PAGE, offset
        );

        let charges: Vec<ApiChargeEntry> = conn
            .prepare(&sql)
            .and_then(|mut s| {
                let rows = s.query_map([], |r| {
                    Ok(ApiChargeEntry {
                        id: r.get(0)?,
                        user_id: r.get(1)?,
                        username: r.get(2)?,
                        plan: r.get(3)?,
                        amount: r.get(4)?,
                        currency: r.get(5)?,
                        is_recurring: r.get::<_, i64>(6)? != 0,
                        payment_date: r.get(7)?,
                    })
                })?;
                Ok(rows.filter_map(|r| r.ok()).collect())
            })
            .unwrap_or_default();

        Ok(json!({
            "stats": {
                "total_charges": total_charges,
                "total_amount": total_amount,
                "premium_count": premium_count,
                "vip_count": vip_count,
                "recurring_count": recurring_count,
            },
            "revenue_per_day": revenue_per_day,
            "charges": {
                "items": charges,
                "total": total,
                "page": page,
                "per_page": REVENUE_PER_PAGE,
                "total_pages": total_pages,
            },
        }))
    })
    .await;

    match result {
        Ok(Ok(data)) => Json(data).into_response(),
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "DB error").into_response(),
    }
}

// ---------------------------------------------------------------------------
// Analytics API
// ---------------------------------------------------------------------------

/// GET /admin/api/analytics — DAU/MAU trends, download trends.
pub(super) async fn admin_api_analytics(
    _admin: RequireAdmin,
    State(state): State<WebState>,
    Query(q): Query<AnalyticsQuery>,
) -> Response {
    let days = q.days.unwrap_or(30).min(90) as i64;
    let db = state.shared_storage.sqlite_pool();

    let result = tokio::task::spawn_blocking(move || -> serde_json::Value {
        let conn = match get_connection(&db) {
            Ok(c) => c,
            Err(_) => return json!({"error": "DB unavailable"}),
        };

        // DAU (from request_history)
        let dau: Vec<serde_json::Value> = conn
            .prepare(&format!(
                "SELECT date(timestamp) AS day, COUNT(DISTINCT user_id) AS users \
                 FROM request_history WHERE timestamp >= date('now','-{} days') \
                 GROUP BY day ORDER BY day ASC",
                days
            ))
            .and_then(|mut s| {
                let rows = s.query_map([], |r| Ok(json!([r.get::<_, String>(0)?, r.get::<_, i64>(1)?])))?;
                Ok(rows.filter_map(|r| r.ok()).collect())
            })
            .unwrap_or_default();

        // MAU (last 30 days)
        let mau: i64 = conn
            .query_row(
                "SELECT COUNT(DISTINCT user_id) FROM request_history \
                 WHERE timestamp >= datetime('now', '-30 days')",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);

        // WAU (last 7 days)
        let wau: i64 = conn
            .query_row(
                "SELECT COUNT(DISTINCT user_id) FROM request_history \
                 WHERE timestamp >= datetime('now', '-7 days')",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);

        // Today's DAU
        let dau_today: i64 = conn
            .query_row(
                "SELECT COUNT(DISTINCT user_id) FROM request_history \
                 WHERE date(timestamp) = date('now')",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);

        // Downloads per day
        let downloads_daily: Vec<serde_json::Value> = conn
            .prepare(&format!(
                "SELECT date(downloaded_at) AS day, COUNT(*) AS cnt \
                 FROM download_history WHERE downloaded_at >= date('now','-{} days') \
                 GROUP BY day ORDER BY day ASC",
                days
            ))
            .and_then(|mut s| {
                let rows = s.query_map([], |r| Ok(json!([r.get::<_, String>(0)?, r.get::<_, i64>(1)?])))?;
                Ok(rows.filter_map(|r| r.ok()).collect())
            })
            .unwrap_or_default();

        // New users per day (first download)
        let new_users_daily: Vec<serde_json::Value> = conn
            .prepare(&format!(
                "SELECT day, COUNT(*) FROM ( \
                    SELECT MIN(date(downloaded_at)) AS day FROM download_history GROUP BY user_id \
                 ) WHERE day >= date('now','-{} days') GROUP BY day ORDER BY day ASC",
                days
            ))
            .and_then(|mut s| {
                let rows = s.query_map([], |r| Ok(json!([r.get::<_, String>(0)?, r.get::<_, i64>(1)?])))?;
                Ok(rows.filter_map(|r| r.ok()).collect())
            })
            .unwrap_or_default();

        // Format distribution trend (last 7 days)
        let format_trend: Vec<serde_json::Value> = conn
            .prepare(
                "SELECT COALESCE(format, 'unknown'), COUNT(*) FROM download_history \
                 WHERE downloaded_at >= date('now','-7 days') \
                 GROUP BY format ORDER BY COUNT(*) DESC LIMIT 5",
            )
            .and_then(|mut s| {
                let rows = s.query_map([], |r| Ok(json!([r.get::<_, String>(0)?, r.get::<_, i64>(1)?])))?;
                Ok(rows.filter_map(|r| r.ok()).collect())
            })
            .unwrap_or_default();

        // Top users this week
        let top_users: Vec<serde_json::Value> = conn
            .prepare(
                "SELECT d.user_id, COALESCE(u.username, ''), COUNT(*) AS cnt \
                 FROM download_history d LEFT JOIN users u ON u.telegram_id = d.user_id \
                 WHERE d.downloaded_at >= date('now','-7 days') \
                 GROUP BY d.user_id ORDER BY cnt DESC LIMIT 10",
            )
            .and_then(|mut s| {
                let rows = s.query_map([], |r| {
                    Ok(json!({
                        "user_id": r.get::<_, i64>(0)?,
                        "username": r.get::<_, String>(1)?,
                        "count": r.get::<_, i64>(2)?,
                    }))
                })?;
                Ok(rows.filter_map(|r| r.ok()).collect())
            })
            .unwrap_or_default();

        json!({
            "dau_today": dau_today,
            "wau": wau,
            "mau": mau,
            "dau": dau,
            "downloads_daily": downloads_daily,
            "new_users_daily": new_users_daily,
            "format_trend": format_trend,
            "top_users": top_users,
        })
    })
    .await;

    match result {
        Ok(data) => Json(data).into_response(),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "Error").into_response(),
    }
}

// ---------------------------------------------------------------------------
// Audit log API
// ---------------------------------------------------------------------------

const AUDIT_PER_PAGE: u32 = 50;

/// GET /admin/api/audit — paginated admin audit log.
pub(super) async fn admin_api_audit(
    _admin: RequireAdmin,
    State(state): State<WebState>,
    Query(q): Query<AuditQuery>,
) -> Response {
    let page = q.page.unwrap_or(1).max(1);
    let action_filter = q.action.unwrap_or_default();
    let offset = ((page - 1) * AUDIT_PER_PAGE) as i64;
    let db = state.shared_storage.sqlite_pool();

    let result = tokio::task::spawn_blocking(move || -> Result<PaginatedResponse<ApiAuditEntry>, rusqlite::Error> {
        let conn = get_connection(&db).map_err(|_| rusqlite::Error::InvalidQuery)?;
        // Allowlist the action filter — anything else is treated as "no filter".
        // Previously this was a raw format!() into SQL, which was a SQL injection
        // vector (the only unallowlisted format!() into SQL in this file).
        let action_allowed = matches!(
            action_filter.as_str(),
            "plan_change"
                | "block"
                | "unblock"
                | "feedback_status"
                | "ack_alert"
                | "send_message"
                | "broadcast"
                | "resolve_error"
                | "bulk_resolve"
                | "retry_task"
                | "cancel_task"
                | "bulk_cancel"
                | "reactivate_sub"
                | "deactivate_sub"
                | "user_settings"
        );

        let total: i64 = if action_allowed {
            conn.query_row(
                "SELECT COUNT(*) FROM admin_audit_log WHERE action = ?1",
                rusqlite::params![action_filter.as_str()],
                |r| r.get(0),
            )
            .unwrap_or(0)
        } else {
            conn.query_row("SELECT COUNT(*) FROM admin_audit_log", [], |r| r.get(0))
                .unwrap_or(0)
        };
        let total_pages = ((total as f64) / AUDIT_PER_PAGE as f64).ceil() as u32;

        // LIMIT / OFFSET are integers derived from u32 query params, safe to format!.
        let sql = if action_allowed {
            format!(
                "SELECT id, admin_id, action, target_type, target_id, \
                        COALESCE(details, ''), created_at \
                 FROM admin_audit_log WHERE action = ?1 ORDER BY created_at DESC LIMIT {} OFFSET {}",
                AUDIT_PER_PAGE, offset
            )
        } else {
            format!(
                "SELECT id, admin_id, action, target_type, target_id, \
                        COALESCE(details, ''), created_at \
                 FROM admin_audit_log ORDER BY created_at DESC LIMIT {} OFFSET {}",
                AUDIT_PER_PAGE, offset
            )
        };

        let entries: Vec<ApiAuditEntry> = {
            let map_row = |r: &rusqlite::Row| -> rusqlite::Result<ApiAuditEntry> {
                Ok(ApiAuditEntry {
                    id: r.get(0)?,
                    admin_id: r.get(1)?,
                    action: r.get(2)?,
                    target_type: r.get(3)?,
                    target_id: r.get(4)?,
                    details: r.get(5)?,
                    created_at: r.get(6)?,
                })
            };
            if action_allowed {
                conn.prepare(&sql)
                    .and_then(|mut s| {
                        let rows = s.query_map(rusqlite::params![action_filter.as_str()], map_row)?;
                        Ok(rows.filter_map(|r| r.ok()).collect::<Vec<_>>())
                    })
                    .unwrap_or_default()
            } else {
                conn.prepare(&sql)
                    .and_then(|mut s| {
                        let rows = s.query_map([], map_row)?;
                        Ok(rows.filter_map(|r| r.ok()).collect::<Vec<_>>())
                    })
                    .unwrap_or_default()
            }
        };

        Ok(PaginatedResponse {
            items: entries,
            total,
            page,
            per_page: AUDIT_PER_PAGE,
            total_pages,
        })
    })
    .await;

    match result {
        Ok(Ok(data)) => Json(data).into_response(),
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "DB error").into_response(),
    }
}

// ---------------------------------------------------------------------------
// Content Subscriptions API
// ---------------------------------------------------------------------------

const SUBS_PER_PAGE: u32 = 50;

/// GET /admin/api/subscriptions — paginated content subscriptions (all users).
pub(super) async fn admin_api_subscriptions(
    _admin: RequireAdmin,
    State(state): State<WebState>,
    Query(q): Query<SubsQuery>,
) -> Response {
    let page = q.page.unwrap_or(1).max(1);
    let status_filter = q.status.unwrap_or_default();
    let search = q.search.unwrap_or_default();
    let offset = ((page - 1) * SUBS_PER_PAGE) as i64;
    let db = state.shared_storage.sqlite_pool();

    let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, rusqlite::Error> {
        let conn = get_connection(&db).map_err(|_| rusqlite::Error::InvalidQuery)?;

        // Aggregate stats
        let total_active: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM content_subscriptions WHERE is_active = 1",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let total_inactive: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM content_subscriptions WHERE is_active = 0",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let total_errored: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM content_subscriptions WHERE is_active = 1 AND consecutive_errors > 0",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let unique_sources: i64 = conn
            .query_row(
                "SELECT COUNT(DISTINCT source_id) FROM content_subscriptions WHERE is_active = 1",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);

        // Build WHERE
        let mut conditions = Vec::new();
        match status_filter.as_str() {
            "active" => conditions.push("s.is_active = 1".to_string()),
            "inactive" => conditions.push("s.is_active = 0".to_string()),
            "errored" => conditions.push("s.is_active = 1 AND s.consecutive_errors > 0".to_string()),
            _ => {}
        }
        let search_param = if !search.is_empty() {
            conditions.push(
                "(s.display_name LIKE ?1 OR s.source_id LIKE ?1 \
                     OR COALESCE(u.username,'') LIKE ?1 OR CAST(s.user_id AS TEXT) LIKE ?1)"
                    .to_string(),
            );
            Some(format!("%{}%", search))
        } else {
            None
        };
        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        let count_sql = format!(
            "SELECT COUNT(*) FROM content_subscriptions s \
                 LEFT JOIN users u ON u.telegram_id = s.user_id {}",
            where_clause
        );
        let total: i64 = if let Some(ref sp) = search_param {
            conn.query_row(&count_sql, rusqlite::params![sp], |r| r.get(0))
                .unwrap_or(0)
        } else {
            conn.query_row(&count_sql, [], |r| r.get(0)).unwrap_or(0)
        };
        let total_pages = ((total as f64) / SUBS_PER_PAGE as f64).ceil() as u32;

        let sql = format!(
            "SELECT s.id, s.user_id, COALESCE(u.username, ''), s.source_type, s.source_id, \
                    COALESCE(s.display_name, ''), COALESCE(s.is_active, 0), \
                    COALESCE(s.last_checked_at, ''), COALESCE(s.last_error, ''), \
                    COALESCE(s.consecutive_errors, 0), COALESCE(s.created_at, '') \
                 FROM content_subscriptions s \
                 LEFT JOIN users u ON u.telegram_id = s.user_id \
                 {} ORDER BY s.created_at DESC LIMIT {} OFFSET {}",
            where_clause, SUBS_PER_PAGE, offset
        );

        let map_row = |r: &rusqlite::Row<'_>| -> rusqlite::Result<ApiContentSub> {
            Ok(ApiContentSub {
                id: r.get(0)?,
                user_id: r.get(1)?,
                username: r.get(2)?,
                source_type: r.get(3)?,
                source_id: r.get(4)?,
                display_name: r.get(5)?,
                is_active: r.get::<_, i64>(6)? != 0,
                last_checked_at: r.get(7)?,
                last_error: r.get(8)?,
                consecutive_errors: r.get(9)?,
                created_at: r.get(10)?,
            })
        };

        let items: Vec<ApiContentSub> = if let Some(ref sp) = search_param {
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

        Ok(json!({
            "stats": {
                "active": total_active,
                "inactive": total_inactive,
                "errored": total_errored,
                "unique_sources": unique_sources,
            },
            "items": items,
            "total": total,
            "page": page,
            "per_page": SUBS_PER_PAGE,
            "total_pages": total_pages,
        }))
    })
    .await;

    match result {
        Ok(Ok(data)) => Json(data).into_response(),
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "DB error").into_response(),
    }
}

/// POST /admin/api/subscriptions/:id/toggle — activate/deactivate a subscription.
pub(super) async fn admin_api_sub_toggle(
    RequireAdminPost(admin_id): RequireAdminPost,
    State(state): State<WebState>,
    Path(sub_id): Path<i64>,
    Json(body): Json<SubToggleReq>,
) -> Response {
    let db = state.shared_storage.sqlite_pool();
    let active = body.is_active;
    let result = tokio::task::spawn_blocking(move || {
        let conn = get_connection(&db).map_err(|_| rusqlite::Error::InvalidQuery)?;
        let val: i64 = if active { 1 } else { 0 };
        let n = conn.execute(
            "UPDATE content_subscriptions SET is_active = ?1, updated_at = CURRENT_TIMESTAMP WHERE id = ?2",
            rusqlite::params![val, sub_id],
        )?;
        if n > 0 {
            let action = if active { "reactivate_sub" } else { "deactivate_sub" };
            log_audit(&conn, admin_id, action, "subscription", &sub_id.to_string(), None);
        }
        Ok::<_, rusqlite::Error>(n)
    })
    .await;

    match result {
        Ok(Ok(0)) => (StatusCode::NOT_FOUND, "Subscription not found").into_response(),
        Ok(Ok(_)) => {
            log::info!("Admin {} toggled sub {} to active={}", admin_id, sub_id, active);
            Json(json!({"ok": true, "is_active": active})).into_response()
        }
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "DB error").into_response(),
    }
}

// ---------------------------------------------------------------------------
// Counts (lightweight polling for badges)
// ---------------------------------------------------------------------------

/// GET /admin/api/counts — quick counts for tab badges.
pub(super) async fn admin_api_counts(_admin: RequireAdmin, State(state): State<WebState>) -> Response {
    let db = state.shared_storage.sqlite_pool();
    let result = tokio::task::spawn_blocking(move || -> serde_json::Value {
        let conn = match get_connection(&db) {
            Ok(c) => c,
            Err(_) => return json!({}),
        };
        let q = |sql: &str| -> i64 {
            conn.query_row(sql, [], |r| r.get(0)).unwrap_or(0)
        };
        json!({
            "queue_active": q("SELECT COUNT(*) FROM task_queue WHERE status IN ('pending','leased','processing','uploading')"),
            "errors_unresolved": q("SELECT COUNT(*) FROM error_log WHERE COALESCE(resolved, 0) = 0"),
            "feedback_new": q("SELECT COUNT(*) FROM feedback_messages WHERE status = 'new'"),
            "alerts_unacked": q("SELECT COUNT(*) FROM alert_history WHERE COALESCE(acknowledged, 0) = 0 AND resolved_at IS NULL"),
        })
    })
    .await;

    match result {
        Ok(data) => Json(data).into_response(),
        Err(_) => Json(json!({})).into_response(),
    }
}
