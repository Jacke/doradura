//! Authentication and authorization for the admin panel.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use axum::{
    body::Body,
    extract::{FromRequestParts, Query, State},
    http::{header, request::Parts, HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

use crate::core::config;
use crate::core::copyright::get_bot_username;

use super::helpers::constant_time_eq;
use super::types::{TelegramAuth, WebState, AUTH_MAX_ATTEMPTS, AUTH_RATE_LIMIT, AUTH_WINDOW_SECS};

// --- CSRF ---

static CSRF_SECRET: LazyLock<String> = LazyLock::new(|| {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let bytes: [u8; 32] = rng.gen();
    hex::encode(bytes)
});

pub(super) fn generate_csrf_token(session_token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(format!("csrf:{}:{}", session_token, &*CSRF_SECRET));
    hex::encode(hasher.finalize())
}

pub(super) fn verify_csrf(header_map: &HeaderMap, _bot_token: &str) -> bool {
    let csrf_header = header_map
        .get("x-csrf-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if csrf_header.is_empty() {
        return false;
    }
    // Regenerate expected token from admin cookie
    let cookie_str = header_map
        .get(header::COOKIE)
        .and_then(|c| c.to_str().ok())
        .unwrap_or("");
    if let Some(token) = cookie_str.split(';').find(|s| s.trim().starts_with("admin_token=")) {
        let token_val = token.trim().strip_prefix("admin_token=").unwrap();
        constant_time_eq(&generate_csrf_token(token_val), csrf_header)
    } else {
        false
    }
}

// --- Rate limiting helpers ---

/// Extract best-effort IP string from request headers.
///
/// Uses `TRUSTED_PROXY_HOPS=N` to pick the *Nth-from-right* entry of
/// `X-Forwarded-For`. The rightmost entries are added by trusted reverse
/// proxies; anything further left is attacker-controlled and must not be
/// trusted. If `TRUSTED_PROXY_HOPS` is unset or zero, the function returns
/// `"unknown"` — callers should rely on the socket peer address instead.
///
/// The previous version took `.next()` (i.e. the FIRST entry), which
/// trusted attacker-supplied values and allowed trivial rate-limit bypass.
pub(super) fn extract_ip(header_map: &HeaderMap) -> String {
    let hops: usize = std::env::var("TRUSTED_PROXY_HOPS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    if hops == 0 {
        return "unknown".to_string();
    }
    header_map
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| {
            let parts: Vec<&str> = s.split(',').map(str::trim).collect();
            if parts.len() >= hops {
                parts.get(parts.len() - hops).copied()
            } else {
                None
            }
        })
        .unwrap_or("unknown")
        .to_string()
}

/// Check and increment a rate-limit bucket. Returns `true` if the request is allowed.
pub(super) async fn check_rate_limit(
    limiter: &tokio::sync::RwLock<std::collections::HashMap<String, (u32, std::time::Instant)>>,
    ip: &str,
    max: u32,
    window_secs: u64,
) -> bool {
    {
        let rates = limiter.read().await;
        if let Some((count, since)) = rates.get(ip) {
            if since.elapsed().as_secs() < window_secs && *count >= max {
                return false;
            }
        }
    }
    {
        let mut rates = limiter.write().await;
        let entry = rates.entry(ip.to_string()).or_insert((0, std::time::Instant::now()));
        if entry.1.elapsed().as_secs() >= window_secs {
            *entry = (1, std::time::Instant::now());
        } else {
            entry.0 += 1;
        }
        rates.retain(|_, (_, since)| since.elapsed().as_secs() < window_secs * 2);
    }
    true
}

// --- Telegram hash verification ---

/// Verify Telegram auth hash.
pub(super) fn verify_telegram_hash(auth: &TelegramAuth, bot_token: &str) -> bool {
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
    constant_time_eq(&hex::encode(result), &auth.hash)
}

// --- Session tokens (DB-backed) ---
//
// The previous implementation generated a deterministic sha256(user_id:bot_token)
// cookie that was:
//   * identical for every login (no nonce),
//   * not stored server-side (no expiry enforcement),
//   * not revocable on logout (Set-Cookie Max-Age=0 only hints the browser).
// This meant BOT_TOKEN leak = permanent global admin access with no recourse.
//
// The new design stores only SHA-256(raw_token) in a DB table with an
// `expires_at` timestamp. verify_admin looks up the hash; logout deletes
// the row. The raw token is the cookie value; it is never persisted.

/// Generate a cryptographically random 32-byte session token (hex-encoded, 64 chars).
pub(super) fn new_session_token() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

/// SHA-256 the raw token for DB storage. We never store the raw token.
fn hash_session_token(token: &str) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hasher.finalize().to_vec()
}

/// Create and persist a new admin session. Returns the raw token to set as cookie.
pub(super) fn create_admin_session(
    conn: &rusqlite::Connection,
    admin_id: i64,
    user_agent: Option<&str>,
    ip: Option<&str>,
) -> Result<String, rusqlite::Error> {
    let token = new_session_token();
    let hash = hash_session_token(&token);
    conn.execute(
        "INSERT INTO admin_sessions (token_hash, admin_id, expires_at, user_agent, ip) \
         VALUES (?1, ?2, datetime('now', '+24 hours'), ?3, ?4)",
        rusqlite::params![hash, admin_id, user_agent, ip],
    )?;
    Ok(token)
}

/// Look up an admin session by raw token. Returns `admin_id` if valid + not expired.
pub(super) fn lookup_admin_session(conn: &rusqlite::Connection, raw_token: &str) -> Option<i64> {
    let hash = hash_session_token(raw_token);
    let result = conn
        .query_row(
            "SELECT admin_id FROM admin_sessions \
             WHERE token_hash = ?1 AND expires_at > datetime('now')",
            rusqlite::params![hash],
            |row| row.get::<_, i64>(0),
        )
        .ok();

    // Update last_seen on successful lookup (best-effort; ignore errors).
    if result.is_some() {
        let _ = conn.execute(
            "UPDATE admin_sessions SET last_seen = datetime('now') WHERE token_hash = ?1",
            rusqlite::params![hash_session_token(raw_token)],
        );
    }
    result
}

/// Revoke a specific admin session (logout).
pub(super) fn revoke_admin_session(conn: &rusqlite::Connection, raw_token: &str) -> Result<(), rusqlite::Error> {
    let hash = hash_session_token(raw_token);
    conn.execute(
        "DELETE FROM admin_sessions WHERE token_hash = ?1",
        rusqlite::params![hash],
    )?;
    Ok(())
}

/// Best-effort periodic cleanup of expired sessions (called from verify_admin).
fn cleanup_expired_sessions(conn: &rusqlite::Connection) {
    let _ = conn.execute("DELETE FROM admin_sessions WHERE expires_at <= datetime('now')", []);
}

/// Extract the `admin_token` cookie value from a header map.
fn extract_admin_cookie(header_map: &HeaderMap) -> Option<String> {
    let cookie_str = header_map.get(header::COOKIE).and_then(|c| c.to_str().ok())?;
    cookie_str
        .split(';')
        .find_map(|s| s.trim().strip_prefix("admin_token=").map(|v| v.to_string()))
}

// --- Admin auth helpers ---

/// Verify admin cookie + CSRF token for POST requests.
#[allow(clippy::result_large_err)]
pub(super) fn verify_admin_post(header_map: &HeaderMap, state: &WebState) -> Result<i64, Response> {
    let admin_id = verify_admin(header_map, state)?;
    if !verify_csrf(header_map, "") {
        return Err((StatusCode::FORBIDDEN, "Invalid CSRF token").into_response());
    }
    Ok(admin_id)
}

/// Verify admin cookie against the DB session store. Returns admin_id on success.
#[allow(clippy::result_large_err)]
pub(super) fn verify_admin(header_map: &HeaderMap, state: &WebState) -> Result<i64, Response> {
    let raw_token = extract_admin_cookie(header_map)
        .ok_or_else(|| (StatusCode::UNAUTHORIZED, "Not authenticated").into_response())?;

    let pool = state.shared_storage.sqlite_pool();
    let conn = crate::storage::get_connection(&pool)
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "DB error").into_response())?;

    if let Some(admin_id) = lookup_admin_session(&conn, &raw_token) {
        // Defense in depth: verify admin_id is still in the allowlist
        // (in case they were removed from ADMIN_IDS after issuance).
        if config::admin::ADMIN_IDS.contains(&admin_id) || *config::admin::ADMIN_USER_ID == admin_id {
            // Periodically clean up expired rows (every verification is fine —
            // cheap query with an index; happens ~once per admin request).
            cleanup_expired_sessions(&conn);
            return Ok(admin_id);
        }
    }
    Err((StatusCode::UNAUTHORIZED, "Not authenticated").into_response())
}

// --- Admin auth extractors ---
//
// These turn the repeated `if let Err(resp) = verify_admin(&header_map, &state) { return resp; }`
// prologue at the top of ~20 admin handlers into a single function-parameter extractor.
// Presence of `RequireAdmin` / `RequireAdminPost` in a handler signature means the
// auth check is statically guaranteed to run — you cannot forget it without the
// compiler refusing to build the route.

/// GET-style admin auth: verifies the `admin_token` cookie against the session
/// store and checks the user is still in the admin allowlist. Returns the
/// resolved admin user id via `.0` — not every handler needs the id, so the
/// field is intentionally allowed to be unread.
#[allow(dead_code)]
pub struct RequireAdmin(pub i64);

impl FromRequestParts<WebState> for RequireAdmin {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &WebState) -> Result<Self, Self::Rejection> {
        verify_admin(&parts.headers, state).map(RequireAdmin)
    }
}

/// POST-style admin auth: everything `RequireAdmin` does, plus validates the
/// `x-csrf-token` header. Use this on any state-mutating endpoint.
pub struct RequireAdminPost(pub i64);

impl FromRequestParts<WebState> for RequireAdminPost {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &WebState) -> Result<Self, Self::Rejection> {
        verify_admin_post(&parts.headers, state).map(RequireAdminPost)
    }
}

// --- Auth route handlers ---

/// GET /admin/login -- Login page with Telegram Widget.
pub(super) async fn admin_login_handler(State(_state): State<WebState>) -> Response {
    let bot_username = match get_bot_username() {
        Some(u) => u.to_string(),
        None => {
            // Fallback to ADMIN_USERNAME env var if bot hasn't started yet
            let fallback = config::admin::ADMIN_USERNAME.clone();
            if fallback.is_empty() {
                return (StatusCode::INTERNAL_SERVER_ERROR, "Bot username not available yet").into_response();
            }
            fallback
        }
    };

    let html = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <title>Admin Login — Doradura</title>
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <style>
        *, *::before, *::after {{ box-sizing: border-box; margin: 0; padding: 0; }}
        body {{
            background: #0d0d0d;
            color: #fff;
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', system-ui, sans-serif;
            display: flex;
            justify-content: center;
            align-items: center;
            min-height: 100vh;
        }}
        .login-wrap {{
            display: flex;
            flex-direction: column;
            align-items: center;
            gap: 32px;
        }}
        .logo {{
            font-size: 2rem;
            font-weight: 800;
            letter-spacing: -0.5px;
            color: #fff;
        }}
        .logo span {{ color: #7c6aff; }}
        .card {{
            background: #1a1a1a;
            border: 1px solid #2a2a2a;
            border-radius: 20px;
            padding: 40px 48px;
            text-align: center;
            box-shadow: 0 24px 64px rgba(0,0,0,0.5);
            min-width: 320px;
        }}
        .card h1 {{
            font-size: 1.3rem;
            font-weight: 600;
            margin-bottom: 8px;
            color: #fff;
        }}
        .card p {{
            color: #666;
            font-size: 0.88rem;
            margin-bottom: 28px;
            line-height: 1.5;
        }}
        .tg-wrap {{
            display: flex;
            justify-content: center;
        }}
        .footer {{
            color: #444;
            font-size: 0.78rem;
        }}
    </style>
</head>
<body>
    <div class="login-wrap">
        <div class="logo">dora<span>dura</span></div>
        <div class="card">
            <h1>Admin Access</h1>
            <p>Sign in with your Telegram account<br>to access the dashboard.</p>
            <div class="tg-wrap">
                <script async src="https://telegram.org/js/telegram-widget.js?22"
                        data-telegram-login="{bot_username}"
                        data-size="large"
                        data-auth-url="/admin/auth"
                        data-request-access="write"></script>
            </div>
        </div>
        <div class="footer">Only authorised admins can log in.</div>
    </div>
</body>
</html>"#,
        bot_username = bot_username
    );

    Html(html).into_response()
}

/// GET /admin/auth -- Telegram authentication callback.
pub(super) async fn admin_auth_handler(
    State(state): State<WebState>,
    header_map: HeaderMap,
    Query(auth): Query<TelegramAuth>,
) -> Response {
    // 0. Rate-limit by IP
    let ip = extract_ip(&header_map);
    if !check_rate_limit(&AUTH_RATE_LIMIT, &ip, AUTH_MAX_ATTEMPTS, AUTH_WINDOW_SECS).await {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            "Too many login attempts. Try again later.",
        )
            .into_response();
    }

    // 1. Verify Telegram hash
    if !verify_telegram_hash(&auth, &state.bot_token) {
        return (StatusCode::UNAUTHORIZED, "Invalid hash").into_response();
    }

    // 2. Reject stale auth data (must be within 5 minutes)
    let now_unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    if now_unix - auth.auth_date > 300 {
        return (StatusCode::UNAUTHORIZED, "Auth data expired. Please log in again.").into_response();
    }

    // 3. Check if user is admin
    let is_admin = config::admin::ADMIN_IDS.contains(&auth.id) || *config::admin::ADMIN_USER_ID == auth.id;
    if !is_admin {
        return (StatusCode::FORBIDDEN, "Not an admin").into_response();
    }

    // 4. Create a random session token, persist its hash in the DB, return raw to client.
    let user_agent = header_map.get(header::USER_AGENT).and_then(|v| v.to_str().ok());
    let ip_for_session = if ip != "unknown" { Some(ip.as_str()) } else { None };

    let pool = state.shared_storage.sqlite_pool();
    let conn = match crate::storage::get_connection(&pool) {
        Ok(c) => c,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "DB error").into_response(),
    };

    let raw_token = match create_admin_session(&conn, auth.id, user_agent, ip_for_session) {
        Ok(t) => t,
        Err(e) => {
            log::error!("Failed to create admin session: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "Session store error").into_response();
        }
    };

    let cookie = format!(
        "admin_token={}; Path=/admin; HttpOnly; Secure; SameSite=Lax; Max-Age=86400",
        raw_token
    );

    Response::builder()
        .status(StatusCode::SEE_OTHER)
        .header(header::SET_COOKIE, cookie)
        .header(header::LOCATION, "/admin")
        .body(axum::body::Body::empty())
        .unwrap()
}

/// GET /admin/logout -- Revoke the session server-side and clear the cookie.
pub(super) async fn admin_logout_handler(State(state): State<WebState>, header_map: HeaderMap) -> Response {
    // Server-side revocation: delete the session row so the cookie stops working
    // even if the browser keeps it.
    if let Some(raw_token) = extract_admin_cookie(&header_map) {
        let pool = state.shared_storage.sqlite_pool();
        if let Ok(conn) = crate::storage::get_connection(&pool) {
            let _ = revoke_admin_session(&conn, &raw_token);
        }
    }

    let cookie = "admin_token=; Path=/admin; HttpOnly; Secure; SameSite=Lax; Max-Age=0";
    Response::builder()
        .status(StatusCode::FOUND)
        .header("Location", "/admin/login")
        .header("Set-Cookie", cookie)
        .body(Body::empty())
        .unwrap()
        .into_response()
}
