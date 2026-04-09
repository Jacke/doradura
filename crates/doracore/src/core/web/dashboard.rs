//! Admin dashboard handler and supporting types/renderers.

use std::sync::Arc;

use axum::extract::State;
use axum::http::header;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};

use crate::storage::db::DbPool;
use crate::storage::get_connection;

use super::auth::{generate_csrf_token, verify_admin};
use super::helpers::{fmt_num, html_escape};
use super::types::WebState;

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

/// GET /admin — Admin Dashboard.
pub(super) async fn admin_dashboard_handler(State(state): State<WebState>, header_map: header::HeaderMap) -> Response {
    // 1. Check admin cookie
    if let Err(resp) = verify_admin(&header_map, &state) {
        return resp;
    }

    // 2. Fetch stats (sync SQLite — offload to blocking thread pool)
    let db = state.shared_storage.sqlite_pool();
    let stats = match tokio::task::spawn_blocking(move || fetch_admin_stats(&db)).await {
        Ok(s) => s,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "DB error").into_response(),
    };

    // 3. Generate CSRF token from admin cookie
    let admin_token = header_map
        .get(header::COOKIE)
        .and_then(|c| c.to_str().ok())
        .unwrap_or("")
        .split(';')
        .find(|s| s.trim().starts_with("admin_token="))
        .and_then(|t| t.trim().strip_prefix("admin_token="))
        .unwrap_or("");
    let csrf_token = generate_csrf_token(admin_token);

    // 4. Render Dashboard
    let html = render_admin_dashboard(&stats, &csrf_token);
    Html(html).into_response()
}

// ---------------------------------------------------------------------------
// Stats
// ---------------------------------------------------------------------------

struct AdminStats {
    // Overview
    total_users: i64,
    total_downloads: i64,
    active_tasks: i64,
    errors_today: i64,
    downloads_today: i64,
    new_users_today: i64,
    // Downloads per day — (date_str, count), last 30 days, chronological order
    downloads_per_day: Vec<(String, i64)>,
    // Format distribution — (format, count)
    format_dist: Vec<(String, i64)>,
}

fn fetch_admin_stats(db: &Arc<DbPool>) -> AdminStats {
    let conn = match get_connection(db) {
        Ok(c) => c,
        Err(_) => {
            return AdminStats {
                total_users: 0,
                total_downloads: 0,
                active_tasks: 0,
                errors_today: 0,
                downloads_today: 0,
                new_users_today: 0,
                downloads_per_day: vec![],
                format_dist: vec![],
            };
        }
    };

    let total_users: i64 = conn
        .query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))
        .unwrap_or(0);

    let total_downloads: i64 = conn
        .query_row("SELECT COUNT(*) FROM download_history", [], |r| r.get(0))
        .unwrap_or(0);

    let active_tasks: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM task_queue WHERE status IN ('pending','processing')",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    let errors_today: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM error_log WHERE date(timestamp) = date('now')",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    let downloads_today: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM download_history WHERE date(downloaded_at) = date('now')",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    // users table has no created_at column; approximate via first download
    let new_users_today: i64 = conn
        .query_row(
            "SELECT COUNT(DISTINCT user_id) FROM download_history \
             WHERE date(downloaded_at) = date('now') \
             AND user_id NOT IN (SELECT user_id FROM download_history WHERE date(downloaded_at) < date('now'))",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    // Downloads per day for last 30 days
    let downloads_per_day = conn
        .prepare(
            "SELECT date(downloaded_at) AS day, COUNT(*) AS cnt \
             FROM download_history \
             WHERE downloaded_at >= date('now','-29 days') \
             GROUP BY day \
             ORDER BY day ASC",
        )
        .and_then(|mut s| {
            let rows = s.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
            Ok(rows.filter_map(|r| r.ok()).collect::<Vec<_>>())
        })
        .unwrap_or_default();

    // Format distribution
    let format_dist = conn
        .prepare(
            "SELECT COALESCE(format, 'unknown') AS fmt, COUNT(*) AS cnt \
             FROM download_history \
             GROUP BY fmt \
             ORDER BY cnt DESC",
        )
        .and_then(|mut s| {
            let rows = s.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
            Ok(rows.filter_map(|r| r.ok()).collect::<Vec<_>>())
        })
        .unwrap_or_default();

    AdminStats {
        total_users,
        total_downloads,
        active_tasks,
        errors_today,
        downloads_today,
        new_users_today,
        downloads_per_day,
        format_dist,
    }
}

// ---------------------------------------------------------------------------
// Renderer
// ---------------------------------------------------------------------------

fn render_admin_dashboard(stats: &AdminStats, csrf_token: &str) -> String {
    // --- Overview cards ---
    let cards_html = format!(
        r#"
        <div class="stat-card">
            <div class="stat-icon">👤</div>
            <div class="stat-body">
                <div class="stat-value">{total_users}</div>
                <div class="stat-label">Total Users</div>
            </div>
        </div>
        <div class="stat-card">
            <div class="stat-icon">⬇</div>
            <div class="stat-body">
                <div class="stat-value">{total_dl}</div>
                <div class="stat-label">Total Downloads</div>
            </div>
        </div>
        <div class="stat-card">
            <div class="stat-icon">⚙</div>
            <div class="stat-body">
                <div class="stat-value active-val">{active_tasks}</div>
                <div class="stat-label">Active Tasks</div>
            </div>
        </div>
        <div class="stat-card">
            <div class="stat-icon">⚡</div>
            <div class="stat-body">
                <div class="stat-value">{dl_today}</div>
                <div class="stat-label">Downloads Today</div>
            </div>
        </div>
        <div class="stat-card">
            <div class="stat-icon">🆕</div>
            <div class="stat-body">
                <div class="stat-value">{new_users}</div>
                <div class="stat-label">New Users Today</div>
            </div>
        </div>
        <div class="stat-card">
            <div class="stat-icon">🔴</div>
            <div class="stat-body">
                <div class="stat-value {err_class}">{errors_today}</div>
                <div class="stat-label">Errors Today</div>
            </div>
        </div>"#,
        total_users = fmt_num(stats.total_users),
        total_dl = fmt_num(stats.total_downloads),
        active_tasks = fmt_num(stats.active_tasks),
        dl_today = fmt_num(stats.downloads_today),
        new_users = fmt_num(stats.new_users_today),
        errors_today = fmt_num(stats.errors_today),
        err_class = if stats.errors_today > 0 { "err-val" } else { "" },
    );

    // --- Downloads per day bar chart ---
    let max_day_count = stats
        .downloads_per_day
        .iter()
        .map(|(_, c)| *c)
        .max()
        .unwrap_or(1)
        .max(1);
    let mut chart_html = String::new();
    // Fill gaps: build a map for quick lookup then iterate last 30 days
    // We render whatever the DB returns (already ordered)
    for (date, count) in &stats.downloads_per_day {
        let pct = (*count as f64 / max_day_count as f64 * 100.0) as u64;
        let short_date = date.get(5..).unwrap_or(date); // MM-DD
        chart_html.push_str(&format!(
            r#"<div class="bar-col">
                <div class="bar-tooltip">{count} downloads</div>
                <div class="bar" style="height:{pct}%"></div>
                <div class="bar-label">{date}</div>
            </div>"#,
            count = count,
            pct = pct,
            date = short_date,
        ));
    }
    if chart_html.is_empty() {
        chart_html = r#"<div class="empty-state">No download data yet.</div>"#.to_owned();
    }

    // --- Format distribution ---
    let total_fmt: i64 = stats.format_dist.iter().map(|(_, c)| c).sum::<i64>().max(1);
    let mut fmt_html = String::new();
    for (fmt, count) in &stats.format_dist {
        let pct = (*count as f64 / total_fmt as f64 * 100.0) as u64;
        let bar_class = match fmt.as_str() {
            "mp3" => "fmt-mp3",
            "mp4" | "mkv" | "webm" => "fmt-video",
            "m4a" | "aac" => "fmt-aac",
            "flac" | "wav" => "fmt-lossless",
            _ => "fmt-other",
        };
        fmt_html.push_str(&format!(
            r#"<div class="fmt-row">
                <span class="fmt-name">{fmt}</span>
                <div class="fmt-bar-track">
                    <div class="fmt-bar {bar_class}" style="width:{pct}%"></div>
                </div>
                <span class="fmt-count">{count} ({pct}%)</span>
            </div>"#,
            fmt = html_escape(fmt),
            bar_class = bar_class,
            pct = pct,
            count = fmt_num(*count),
        ));
    }
    if fmt_html.is_empty() {
        fmt_html = r#"<div class="empty-state">No data yet.</div>"#.to_owned();
    }

    format!(
        include_str!("../admin_dashboard.html"),
        cards = cards_html,
        chart = chart_html,
        fmt = fmt_html,
        active_tasks = fmt_num(stats.active_tasks),
        total_users = fmt_num(stats.total_users),
        total_dl = fmt_num(stats.total_downloads),
        dl_today = fmt_num(stats.downloads_today),
        errors_today = fmt_num(stats.errors_today),
        err_color = if stats.errors_today > 0 { "#ef4444" } else { "inherit" },
        csrf_token = csrf_token,
    )
}
