//! Authentication and authorization for the admin panel.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use axum::{
    body::Body,
    extract::{Query, State},
    http::{header, HeaderMap, StatusCode},
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

/// Generate a secure token for the admin cookie.
pub(super) fn generate_admin_token(user_id: i64, bot_token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(format!("{}:{}", user_id, bot_token));
    hex::encode(hasher.finalize())
}

// --- Admin auth helpers ---

/// Verify admin cookie + CSRF token for POST requests.
#[allow(clippy::result_large_err)]
pub(super) fn verify_admin_post(header_map: &HeaderMap, bot_token: &str) -> Result<i64, Response> {
    let admin_id = verify_admin(header_map, bot_token)?;
    if !verify_csrf(header_map, bot_token) {
        return Err((StatusCode::FORBIDDEN, "Invalid CSRF token").into_response());
    }
    Ok(admin_id)
}

/// Verify admin cookie and return admin user ID, or an error response.
#[allow(clippy::result_large_err)]
pub(super) fn verify_admin(header_map: &HeaderMap, bot_token: &str) -> Result<i64, Response> {
    let cookie_str = header_map
        .get(header::COOKIE)
        .and_then(|c| c.to_str().ok())
        .unwrap_or("");

    if let Some(token) = cookie_str.split(';').find(|s| s.trim().starts_with("admin_token=")) {
        let token_val = token.trim().strip_prefix("admin_token=").unwrap();
        for &admin_id in config::admin::ADMIN_IDS.iter() {
            if constant_time_eq(&generate_admin_token(admin_id, bot_token), token_val) {
                return Ok(admin_id);
            }
        }
        if *config::admin::ADMIN_USER_ID != 0
            && constant_time_eq(
                &generate_admin_token(*config::admin::ADMIN_USER_ID, bot_token),
                token_val,
            )
        {
            return Ok(*config::admin::ADMIN_USER_ID);
        }
    }
    Err((StatusCode::UNAUTHORIZED, "Not authenticated").into_response())
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

    // 4. Set admin cookie (Path scoped to /admin, Secure flag required)
    let admin_token = generate_admin_token(auth.id, &state.bot_token);
    let cookie = format!(
        "admin_token={}; Path=/admin; HttpOnly; Secure; SameSite=Lax; Max-Age=86400",
        admin_token
    );

    Response::builder()
        .status(StatusCode::SEE_OTHER)
        .header(header::SET_COOKIE, cookie)
        .header(header::LOCATION, "/admin")
        .body(axum::body::Body::empty())
        .unwrap()
}

/// GET /admin/logout -- Clear admin cookie and redirect to login.
pub(super) async fn admin_logout_handler() -> Response {
    let cookie = "admin_token=; Path=/admin; HttpOnly; Secure; SameSite=Lax; Max-Age=0";
    Response::builder()
        .status(StatusCode::FOUND)
        .header("Location", "/admin/login")
        .header("Set-Cookie", cookie)
        .body(Body::empty())
        .unwrap()
        .into_response()
}
