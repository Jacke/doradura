//! Public-facing web handlers (no admin auth required).
//!
//! Includes: metrics, privacy policy, share pages, health check.

use std::collections::BTreeMap;

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Json, Response};
use indoc::formatdoc;
use serde_json::json;

use crate::i18n;
use crate::storage::SharePageRecord;

use super::auth::{check_rate_limit, extract_ip};
use super::helpers::{constant_time_eq, html_escape, is_safe_url};
use super::types::{ErrorResponse, WebState, SHARE_MAX_PER_MIN, SHARE_RATE_LIMIT, SHARE_WINDOW_SECS};

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /metrics — Prometheus metrics endpoint, protected by Bearer token.
///
/// Returns 404 if METRICS_AUTH_TOKEN is not set (disabled).
/// Returns 401 if the Authorization header doesn't match.
pub(super) async fn metrics_handler(headers: HeaderMap) -> Response {
    let token = match std::env::var("METRICS_AUTH_TOKEN") {
        Ok(t) if !t.is_empty() => t,
        _ => return (StatusCode::NOT_FOUND, "Not Found").into_response(),
    };

    let auth = headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("");
    let expected = format!("Bearer {}", token);
    if !constant_time_eq(auth, &expected) {
        return (StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
    }

    let encoder = prometheus::TextEncoder::new();
    let metric_families = prometheus::gather();
    let output = encoder.encode_to_string(&metric_families).unwrap_or_default();
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/plain; version=0.0.4; charset=utf-8")],
        output,
    )
        .into_response()
}

/// GET /privacy — renders the privacy policy HTML.
pub(super) async fn privacy_handler(headers: HeaderMap, Query(params): Query<BTreeMap<String, String>>) -> Response {
    // 1. Detect language (Query param > Accept-Language > Fallback RU)
    let lang_code = params
        .get("lang")
        .cloned()
        .or_else(|| {
            headers
                .get(header::ACCEPT_LANGUAGE)
                .and_then(|h| h.to_str().ok())
                .and_then(|s| s.split(',').next())
                .and_then(|s| s.split(';').next())
                .map(|s| s.trim().split('-').next().unwrap_or("ru").to_lowercase())
        })
        .unwrap_or_else(|| "ru".to_string());

    let supported = i18n::is_language_supported(&lang_code).unwrap_or("ru");

    let html = render_privacy_page(supported);
    Html(html).into_response()
}

/// GET /s/:id — renders the share page HTML.
pub(super) async fn share_page_handler(
    Path(id): Path<String>,
    State(state): State<WebState>,
    header_map: HeaderMap,
) -> Response {
    let ip = extract_ip(&header_map);
    if !check_rate_limit(&SHARE_RATE_LIMIT, &ip, SHARE_MAX_PER_MIN, SHARE_WINDOW_SECS).await {
        return (StatusCode::TOO_MANY_REQUESTS, "Too many requests. Try again later.").into_response();
    }

    let Some(row) = state.shared_storage.get_share_page_record(&id).await.ok().flatten() else {
        return (StatusCode::NOT_FOUND, Html("<h1>Not found</h1>".to_string())).into_response();
    };

    let html = render_share_page(&row);
    Html(html).into_response()
}

/// GET /api/s/:id — returns share page data as JSON.
pub(super) async fn share_api_handler(
    Path(id): Path<String>,
    State(state): State<WebState>,
    header_map: HeaderMap,
) -> Response {
    let ip = extract_ip(&header_map);
    if !check_rate_limit(&SHARE_RATE_LIMIT, &ip, SHARE_MAX_PER_MIN, SHARE_WINDOW_SECS).await {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(ErrorResponse {
                error: "Too many requests",
            }),
        )
            .into_response();
    }

    let Some(row) = state.shared_storage.get_share_page_record(&id).await.ok().flatten() else {
        return (StatusCode::NOT_FOUND, Json(ErrorResponse { error: "Not found" })).into_response();
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
pub(super) async fn health_handler() -> impl IntoResponse {
    (StatusCode::OK, "ok")
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Renderers
// ---------------------------------------------------------------------------

fn render_privacy_page(lang: &str) -> String {
    let title = match lang {
        "en" => "Privacy Policy — Doradura",
        "ru" => "Политика конфиденциальности — Doradura",
        "fr" => "Politique de confidentialité — Doradura",
        "de" => "Datenschutzerklärung — Doradura",
        _ => "Privacy Policy",
    };

    let content = match lang {
        "ru" => include_str!("html/privacy_content_ru.html"),
        "fr" => include_str!("html/privacy_content_fr.html"),
        "de" => include_str!("html/privacy_content_de.html"),
        _ => include_str!("html/privacy_content_en.html"),
    };

    let lang_switcher = formatdoc! {r#"
        <div class="lang-switcher">
            <a href="/privacy?lang=en" class="{en_active}">EN</a>
            <a href="/privacy?lang=ru" class="{ru_active}">RU</a>
            <a href="/privacy?lang=fr" class="{fr_active}">FR</a>
            <a href="/privacy?lang=de" class="{de_active}">DE</a>
        </div>"#,
        en_active = if lang == "en" { "active" } else { "" },
        ru_active = if lang == "ru" { "active" } else { "" },
        fr_active = if lang == "fr" { "active" } else { "" },
        de_active = if lang == "de" { "active" } else { "" },
    };

    const LAYOUT: &str = include_str!("html/privacy_layout.html");
    LAYOUT
        .replace("{LANG}", lang)
        .replace("{TITLE}", title)
        .replace("{LANG_SWITCHER}", &lang_switcher)
        .replace("{CONTENT}", content)
}

/// Render the share page HTML with ambilight UI.
fn render_share_page(row: &SharePageRecord) -> String {
    let title = html_escape(&row.title);
    let artist = row.artist.as_deref().map(html_escape).unwrap_or_default();
    let raw_thumbnail = row.thumbnail_url.as_deref().unwrap_or("");
    let thumbnail_url = if is_safe_url(raw_thumbnail) { raw_thumbnail } else { "" };
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

    let og_image = if thumbnail_url.is_empty() {
        String::new()
    } else {
        format!(r#"<meta property="og:image" content="{}">"#, html_escape(thumbnail_url))
    };

    const TEMPLATE: &str = include_str!("html/share_page.html");
    TEMPLATE
        .replace("{TITLE}", &title)
        .replace("{OG_IMAGE}", &og_image)
        .replace("{AMBILIGHT_BG}", &ambilight_bg)
        .replace("{THUMB_HTML}", &thumb_html)
        .replace("{ARTIST_HTML}", &artist_html)
        .replace("{DURATION_HTML}", &duration_html)
        .replace("{STREAMING_BTNS}", &streaming_btns)
}

fn render_streaming_buttons(links: &serde_json::Value, youtube_url: &str) -> String {
    use std::fmt::Write as _;

    /// Append a single button to `btns` if the URL is present and passes the
    /// safe-scheme check. Uses `write!` against the existing `String` to avoid
    /// the intermediate `format!` allocation.
    fn append_btn(btns: &mut String, url: &str, css: &str, label: &str) {
        if !is_safe_url(url) {
            return;
        }
        // write! into a String can only fail on alloc OOM — propagating here
        // would only hide the panic, so swallow the Result.
        let _ = write!(
            btns,
            r#"<a href="{}" class="btn {}" target="_blank" rel="noopener">{}</a>"#,
            html_escape(url),
            css,
            label
        );
    }

    let mut btns = String::new();
    let services: &[(&str, &str, &str)] = &[
        ("spotify", "spotify", "Spotify"),
        ("appleMusic", "apple", "Apple Music"),
        ("youtubeMusic", "yt", "YouTube Music"),
        ("deezer", "deezer", "Deezer"),
        ("tidal", "tidal", "Tidal"),
        ("amazonMusic", "amazon", "Amazon Music"),
    ];

    for (key, css, label) in services {
        if let Some(url) = links.get(key).and_then(|v| v.as_str()) {
            append_btn(&mut btns, url, css, label);
        }
    }

    // Always show the original YouTube link if it has a safe scheme
    append_btn(&mut btns, youtube_url, "youtube-src", "YouTube");

    btns
}
