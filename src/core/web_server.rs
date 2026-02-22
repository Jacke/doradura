//! Public-facing web server for share pages.
//!
//! Serves beautiful ambilight share pages with streaming links at /s/{id}.
//! Runs on WEB_PORT (default 3000) alongside the internal metrics server.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{Html, IntoResponse, Json, Response},
    routing::get,
    Router,
};
use serde_json::json;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;

use crate::storage::db::DbPool;
use crate::storage::get_connection;

/// Shared state for the web server.
#[derive(Clone)]
struct WebState {
    db: Arc<DbPool>,
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

/// Start the public web server.
pub async fn start_web_server(port: u16, db: Arc<DbPool>) -> Result<(), Box<dyn std::error::Error>> {
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let state = WebState { db };

    let app = Router::new()
        .route("/s/:id", get(share_page_handler))
        .route("/api/s/:id", get(share_api_handler))
        .route("/health", get(health_handler))
        .with_state(state);

    log::info!("Starting web server on http://{}", addr);
    log::info!("  /s/:id      - Share page (HTML)");
    log::info!("  /api/s/:id  - Share page (JSON)");
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
