//! Public-facing web server for share pages.
//!
//! Serves beautiful ambilight share pages with streaming links at /s/{id}.
//! Runs on WEB_PORT (default 3000) alongside the internal metrics server.

use axum::{
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{Html, IntoResponse, Json, Redirect, Response},
    routing::get,
    Router,
};
use hmac::{Hmac, Mac};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;

use crate::core::config;
use crate::storage::db::DbPool;
use crate::storage::get_connection;

/// Shared state for the web server.
#[derive(Clone)]
struct WebState {
    db: Arc<DbPool>,
    bot_token: String,
}

/// Row fetched from the share_pages table.
struct ShareRow {
    id: String,
    youtube_url: String,
    title: String,
    artist: Option<String>,
    thumbnail_url: Option<String>,
    duration_secs: Option<i64>,
    streaming_links_json: Option<String>,
    created_at: String,
}

/// Query parameters from Telegram Login Widget
#[derive(Deserialize, Debug)]
struct TelegramAuth {
    id: i64,
    first_name: Option<String>,
    last_name: Option<String>,
    username: Option<String>,
    photo_url: Option<String>,
    auth_date: i64,
    hash: String,
}

/// Start the public web server.
pub async fn start_web_server(port: u16, db: Arc<DbPool>) -> Result<(), Box<dyn std::error::Error>> {
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let bot_token = config::BOT_TOKEN.clone();
    let state = WebState { db, bot_token };

    let app = Router::new()
        .route("/s/:id", get(share_page_handler))
        .route("/api/s/:id", get(share_api_handler))
        .route("/health", get(health_handler))
        // Admin routes
        .route("/admin", get(admin_dashboard_handler))
        .route("/admin/login", get(admin_login_handler))
        .route("/admin/auth", get(admin_auth_handler))
        .with_state(state);

    log::info!("Starting web server on http://{}", addr);
    log::info!("  /s/:id      - Share page (HTML)");
    log::info!("  /api/s/:id  - Share page (JSON)");
    log::info!("  /admin      - Admin Dashboard");
    log::info!("  /health     - Health check");

    let listener = TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

/// Fetch a share page row from DB by ID.
fn fetch_share_row(db: &Arc<DbPool>, id: &str) -> Option<ShareRow> {
    let conn = get_connection(db).ok()?;
    conn.query_row(
        "SELECT id, youtube_url, title, artist, thumbnail_url, duration_secs, streaming_links, created_at FROM share_pages WHERE id = ?1",
        rusqlite::params![id],
        |row| {
            Ok(ShareRow {
                id: row.get(0)?,
                youtube_url: row.get(1)?,
                title: row.get(2)?,
                artist: row.get(3)?,
                thumbnail_url: row.get(4)?,
                duration_secs: row.get(5)?,
                streaming_links_json: row.get(6)?,
                created_at: row.get(7)?,
            })
        },
    )
    .ok()
}

/// GET /s/:id — renders the share page HTML.
async fn share_page_handler(Path(id): Path<String>, State(state): State<WebState>) -> Response {
    let Some(row) = fetch_share_row(&state.db, &id) else {
        return (StatusCode::NOT_FOUND, Html("<h1>Not found</h1>".to_string())).into_response();
    };

    let html = render_share_page(&row);
    Html(html).into_response()
}

/// GET /api/s/:id — returns share page data as JSON.
async fn share_api_handler(Path(id): Path<String>, State(state): State<WebState>) -> Response {
    let Some(row) = fetch_share_row(&state.db, &id) else {
        return (StatusCode::NOT_FOUND, Json(json!({"error": "Not found"}))).into_response();
    };

    let streaming_links = row
        .streaming_links_json
        .as_deref()
        .map(parse_streaming_links)
        .unwrap_or_default();

    let data = json!({
        "id": row.id,
        "title": row.title,
        "artist": row.artist,
        "thumbnail_url": row.thumbnail_url,
        "duration_secs": row.duration_secs,
        "youtube_url": row.youtube_url,
        "streaming_links": streaming_links,
        "created_at": row.created_at,
    });

    Json(data).into_response()
}

/// GET /health — simple health check.
async fn health_handler() -> impl IntoResponse {
    (StatusCode::OK, "ok")
}

// --- Admin Handlers ---

/// GET /admin/login — Login page with Telegram Widget.
async fn admin_login_handler(State(_state): State<WebState>) -> Response {
    let bot_username = config::admin::ADMIN_USERNAME.as_str();
    if bot_username.is_empty() {
        return (StatusCode::INTERNAL_SERVER_ERROR, "ADMIN_USERNAME not set").into_response();
    }

    let html = format!(
        r#"<!DOCTYPE html>
<html>
<head>
    <title>Admin Login — Doradura</title>
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <style>
        body {{ background: #0d0d0d; color: #fff; font-family: sans-serif; display: flex; justify-content: center; align-items: center; height: 100vh; margin: 0; }}
        .card {{ background: rgba(255,255,255,0.05); padding: 40px; border-radius: 20px; text-align: center; border: 1px solid rgba(255,255,255,0.1); }}
        h1 {{ margin-bottom: 20px; font-size: 1.5rem; }}
    </style>
</head>
<body>
    <div class="card">
        <h1>Admin Login</h1>
        <script async src="https://telegram.org/js/telegram-widget.js?22" 
                data-telegram-login="{}" 
                data-size="large" 
                data-auth-url="/admin/auth" 
                data-request-access="write"></script>
    </div>
</body>
</html>"#,
        bot_username
    );

    Html(html).into_response()
}

/// GET /admin/auth — Telegram authentication callback.
async fn admin_auth_handler(State(state): State<WebState>, Query(auth): Query<TelegramAuth>) -> Response {
    // 1. Verify Telegram hash
    if !verify_telegram_hash(&auth, &state.bot_token) {
        return (StatusCode::UNAUTHORIZED, "Invalid hash").into_response();
    }

    // 2. Check if user is admin
    let is_admin = config::admin::ADMIN_IDS.contains(&auth.id) || *config::admin::ADMIN_USER_ID == auth.id;
    if !is_admin {
        return (StatusCode::FORBIDDEN, "Not an admin").into_response();
    }

    // 3. Set admin cookie
    let admin_token = generate_admin_token(auth.id, &state.bot_token);
    let cookie = format!(
        "admin_token={}; Path=/; HttpOnly; SameSite=Lax; Max-Age=86400",
        admin_token
    );

    Response::builder()
        .status(StatusCode::SEE_OTHER)
        .header(header::SET_COOKIE, cookie)
        .header(header::LOCATION, "/admin")
        .body(axum::body::Body::empty())
        .unwrap()
}

/// GET /admin — Admin Dashboard.
async fn admin_dashboard_handler(State(state): State<WebState>, header_map: header::HeaderMap) -> Response {
    // 1. Check admin cookie
    let cookie_str = header_map
        .get(header::COOKIE)
        .and_then(|c| c.to_str().ok())
        .unwrap_or("");
    let mut authed_user_id = None;

    if let Some(token) = cookie_str.split(';').find(|s| s.trim().starts_with("admin_token=")) {
        let token_val = token.trim().strip_prefix("admin_token=").unwrap();

        // Verify token (brute force check against admin IDs since it's a small list)
        for &admin_id in config::admin::ADMIN_IDS.iter() {
            if generate_admin_token(admin_id, &state.bot_token) == token_val {
                authed_user_id = Some(admin_id);
                break;
            }
        }
        if authed_user_id.is_none()
            && *config::admin::ADMIN_USER_ID != 0
            && generate_admin_token(*config::admin::ADMIN_USER_ID, &state.bot_token) == token_val
        {
            authed_user_id = Some(*config::admin::ADMIN_USER_ID);
        }
    }

    if authed_user_id.is_none() {
        return Redirect::to("/admin/login").into_response();
    }

    // 2. Fetch stats
    let stats = fetch_admin_stats(&state.db);

    // 3. Render Dashboard
    let html = render_admin_dashboard(&stats);
    Html(html).into_response()
}

/// Verify Telegram auth hash.
fn verify_telegram_hash(auth: &TelegramAuth, bot_token: &str) -> bool {
    let mut params = BTreeMap::new();
    params.insert("id", auth.id.to_string());
    if let Some(ref s) = auth.first_name {
        params.insert("first_name", s.clone());
    }
    if let Some(ref s) = auth.last_name {
        params.insert("last_name", s.clone());
    }
    if let Some(ref s) = auth.username {
        params.insert("username", s.clone());
    }
    if let Some(ref s) = auth.photo_url {
        params.insert("photo_url", s.clone());
    }
    params.insert("auth_date", auth.auth_date.to_string());

    let data_check_string = params
        .iter()
        .map(|(k, v)| format!("{}={}", k, v))
        .collect::<Vec<_>>()
        .join("\n");

    let mut hasher = Sha256::new();
    hasher.update(bot_token);
    let secret_key = hasher.finalize();

    type HmacSha256 = Hmac<Sha256>;
    let mut mac = HmacSha256::new_from_slice(&secret_key).expect("HMAC can take key of any size");
    mac.update(data_check_string.as_bytes());

    let result = mac.finalize().into_bytes();
    hex::encode(result) == auth.hash
}

/// Generate a secure token for the admin cookie.
fn generate_admin_token(user_id: i64, bot_token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(format!("{}:{}", user_id, bot_token));
    hex::encode(hasher.finalize())
}

struct AdminStats {
    total_users: i64,
    total_downloads: i64,
    active_tasks: i64,
    recent_errors: Vec<(String, String, String)>, // (timestamp, category, message)
}

fn fetch_admin_stats(db: &Arc<DbPool>) -> AdminStats {
    let conn = get_connection(db).unwrap();

    let total_users: i64 = conn
        .query_row("SELECT COUNT(*) FROM users", [], |row| row.get(0))
        .unwrap_or(0);
    let total_downloads: i64 = conn
        .query_row("SELECT COUNT(*) FROM download_history", [], |row| row.get(0))
        .unwrap_or(0);
    let active_tasks: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM task_queue WHERE status IN ('pending', 'processing')",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);

    let mut stmt = conn
        .prepare("SELECT timestamp, category, message FROM error_log ORDER BY timestamp DESC LIMIT 10")
        .unwrap();
    let recent_errors = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();

    AdminStats {
        total_users,
        total_downloads,
        active_tasks,
        recent_errors,
    }
}

fn render_admin_dashboard(stats: &AdminStats) -> String {
    let mut errors_html = String::new();
    for (ts, cat, msg) in &stats.recent_errors {
        errors_html.push_str(&format!(
            r#"<tr><td>{}</td><td><span class="badge">{}</span></td><td>{}</td></tr>"#,
            ts,
            cat,
            html_escape(msg)
        ));
    }

    format!(
        r#"<!DOCTYPE html>
<html>
<head>
    <title>Dashboard — Doradura Admin</title>
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <style>
        body {{ background: #0d0d0d; color: #fff; font-family: -apple-system, system-ui, sans-serif; margin: 0; padding: 20px; }}
        .container {{ max-width: 900px; margin: 0 auto; }}
        .header {{ display: flex; justify-content: space-between; align-items: center; margin-bottom: 30px; border-bottom: 1px solid #333; padding-bottom: 10px; }}
        h1 {{ margin: 0; font-size: 1.5rem; }}
        .stats-grid {{ display: grid; grid-template-columns: repeat(auto-fit, minmax(200px, 1fr)); gap: 20px; margin-bottom: 40px; }}
        .stat-card {{ background: #1a1a1a; padding: 20px; border-radius: 12px; border: 1px solid #333; }}
        .stat-card .label {{ color: #888; font-size: 0.9rem; margin-bottom: 10px; }}
        .stat-card .value {{ font-size: 1.8rem; font-weight: bold; }}
        table {{ width: 100%; border-collapse: collapse; background: #1a1a1a; border-radius: 12px; overflow: hidden; border: 1px solid #333; }}
        th, td {{ padding: 12px 15px; text-align: left; border-bottom: 1px solid #333; }}
        th {{ background: #222; color: #888; font-weight: normal; font-size: 0.85rem; text-transform: uppercase; }}
        .badge {{ background: #444; padding: 2px 8px; border-radius: 4px; font-size: 0.8rem; }}
        tr:last-child td {{ border-bottom: none; }}
        @media (max-width: 600px) {{ .stats-grid {{ grid-template-columns: 1fr; }} }}
    </style>
</head>
<body>
    <div class="container">
        <div class="header">
            <h1>Doradura Admin</h1>
            <a href="/admin/login" style="color:#888; text-decoration:none; font-size:0.9rem;">Logout</a>
        </div>
        
        <div class="stats-grid">
            <div class="stat-card">
                <div class="label">Total Users</div>
                <div class="value">{}</div>
            </div>
            <div class="stat-card">
                <div class="label">Total Downloads</div>
                <div class="value">{}</div>
            </div>
            <div class="stat-card">
                <div class="label">Active Tasks</div>
                <div class="value">{}</div>
            </div>
        </div>

        <h2>Recent Errors</h2>
        <table>
            <thead>
                <tr>
                    <th>Time</th>
                    <th>Category</th>
                    <th>Message</th>
                </tr>
            </thead>
            <tbody>
                {}
            </tbody>
        </table>
    </div>
</body>
</html>"#,
        stats.total_users, stats.total_downloads, stats.active_tasks, errors_html
    )
}

/// Format seconds as MM:SS or H:MM:SS.
fn format_duration(secs: i64) -> String {
    if secs < 0 {
        return String::new();
    }
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{}:{:02}:{:02}", h, m, s)
    } else {
        format!("{}:{:02}", m, s)
    }
}

/// Parse streaming links JSON into individual URLs.
fn parse_streaming_links(json_str: &str) -> serde_json::Value {
    serde_json::from_str(json_str).unwrap_or_default()
}

/// Render the share page HTML with ambilight UI.
fn render_share_page(row: &ShareRow) -> String {
    let title = html_escape(&row.title);
    let artist = row.artist.as_deref().map(html_escape).unwrap_or_default();
    let thumbnail_url = row.thumbnail_url.as_deref().unwrap_or("");
    let duration = row.duration_secs.map(format_duration).unwrap_or_default();

    let streaming_links = row
        .streaming_links_json
        .as_deref()
        .map(parse_streaming_links)
        .unwrap_or_default();

    let ambilight_bg = if thumbnail_url.is_empty() {
        String::new()
    } else {
        format!(
            r#"<div class="ambilight-bg" style="background-image:url('{}')"></div>"#,
            html_escape(thumbnail_url)
        )
    };

    let thumb_html = if thumbnail_url.is_empty() {
        String::new()
    } else {
        format!(
            r#"<img class="thumb" src="{}" alt="{}" loading="lazy">"#,
            html_escape(thumbnail_url),
            html_escape(&row.title)
        )
    };

    let artist_html = if artist.is_empty() {
        String::new()
    } else {
        format!(r#"<p class="artist">{}</p>"#, artist)
    };

    let duration_html = if duration.is_empty() {
        String::new()
    } else {
        format!(r#"<p class="duration">{}</p>"#, duration)
    };

    let streaming_btns = render_streaming_buttons(&streaming_links, &row.youtube_url);

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>{title} — Listen</title>
<meta property="og:title" content="{title}">
<meta property="og:description" content="Listen on your favourite streaming service">
{og_image}
<style>
*{{box-sizing:border-box;margin:0;padding:0}}
body{{background:#0d0d0d;min-height:100vh;display:flex;justify-content:center;align-items:center;font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',sans-serif;padding:20px}}
.ambilight-bg{{position:fixed;inset:-60px;background-size:cover;background-position:center;filter:blur(60px) saturate(160%) brightness(50%);z-index:0;opacity:.85}}
.card{{position:relative;z-index:1;backdrop-filter:blur(20px);-webkit-backdrop-filter:blur(20px);background:rgba(255,255,255,.08);border:1px solid rgba(255,255,255,.12);border-radius:24px;padding:32px;max-width:480px;width:100%;text-align:center;color:#fff}}
.thumb{{width:100%;border-radius:16px;box-shadow:0 8px 40px rgba(0,0,0,.6);margin-bottom:20px;display:block}}
h1{{font-size:1.4rem;font-weight:700;line-height:1.3;margin-bottom:8px}}
.artist{{color:rgba(255,255,255,.7);font-size:.95rem;margin-bottom:4px}}
.duration{{color:rgba(255,255,255,.5);font-size:.85rem;margin-bottom:24px}}
.streaming-links{{display:flex;flex-wrap:wrap;gap:8px;justify-content:center;margin-bottom:20px}}
.btn{{display:inline-block;padding:10px 20px;border-radius:50px;text-decoration:none;font-weight:600;font-size:.9rem;transition:opacity .15s}}
.btn:hover{{opacity:.85}}
.btn.spotify{{background:#1DB954;color:#000}}
.btn.apple{{background:#fc3c44;color:#fff}}
.btn.yt{{background:#ff0000;color:#fff}}
.btn.deezer{{background:#a238ff;color:#fff}}
.btn.tidal{{background:#000;color:#fff;border:1px solid #444}}
.btn.amazon{{background:#00a8e1;color:#fff}}
.btn.youtube-src{{background:rgba(255,255,255,.12);color:#fff;border:1px solid rgba(255,255,255,.2)}}
.disclaimer{{color:rgba(255,255,255,.35);font-size:.75rem;line-height:1.4}}
</style>
</head>
<body>
{ambilight_bg}
<div class="card">
{thumb_html}
<h1>{title}</h1>
{artist_html}
{duration_html}
<div class="streaming-links">
{streaming_btns}
</div>
<p class="disclaimer">Content belongs to respective rights holders.<br>Links provided for legal streaming only.</p>
</div>
</body>
</html>"#,
        title = title,
        og_image = if thumbnail_url.is_empty() {
            String::new()
        } else {
            format!(r#"<meta property="og:image" content="{}">"#, html_escape(thumbnail_url))
        },
        ambilight_bg = ambilight_bg,
        thumb_html = thumb_html,
        artist_html = artist_html,
        duration_html = duration_html,
        streaming_btns = streaming_btns,
    )
}

fn render_streaming_buttons(links: &serde_json::Value, youtube_url: &str) -> String {
    let mut btns = String::new();

    if let Some(url) = links.get("spotify").and_then(|v| v.as_str()) {
        btns.push_str(&format!(
            r#"<a href="{}" class="btn spotify" target="_blank" rel="noopener">Spotify</a>"#,
            html_escape(url)
        ));
    }
    if let Some(url) = links.get("appleMusic").and_then(|v| v.as_str()) {
        btns.push_str(&format!(
            r#"<a href="{}" class="btn apple" target="_blank" rel="noopener">Apple Music</a>"#,
            html_escape(url)
        ));
    }
    if let Some(url) = links.get("youtubeMusic").and_then(|v| v.as_str()) {
        btns.push_str(&format!(
            r#"<a href="{}" class="btn yt" target="_blank" rel="noopener">YouTube Music</a>"#,
            html_escape(url)
        ));
    }
    if let Some(url) = links.get("deezer").and_then(|v| v.as_str()) {
        btns.push_str(&format!(
            r#"<a href="{}" class="btn deezer" target="_blank" rel="noopener">Deezer</a>"#,
            html_escape(url)
        ));
    }
    if let Some(url) = links.get("tidal").and_then(|v| v.as_str()) {
        btns.push_str(&format!(
            r#"<a href="{}" class="btn tidal" target="_blank" rel="noopener">Tidal</a>"#,
            html_escape(url)
        ));
    }
    if let Some(url) = links.get("amazonMusic").and_then(|v| v.as_str()) {
        btns.push_str(&format!(
            r#"<a href="{}" class="btn amazon" target="_blank" rel="noopener">Amazon Music</a>"#,
            html_escape(url)
        ));
    }

    // Always show the original YouTube link
    btns.push_str(&format!(
        r#"<a href="{}" class="btn youtube-src" target="_blank" rel="noopener">YouTube</a>"#,
        html_escape(youtube_url)
    ));

    btns
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
