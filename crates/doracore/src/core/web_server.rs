//! Public-facing web server for share pages.
//!
//! Serves beautiful ambilight share pages with streaming links at /s/{id}.
//! Runs on WEB_PORT (default 3000) alongside the internal metrics server.

use axum::{
    body::Body,
    extract::{DefaultBodyLimit, Path, Query, Request, State},
    http::{header, HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{Html, IntoResponse, Json, Redirect, Response},
    routing::{get, post},
    Router,
};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::sync::{Arc, LazyLock};
use tokio::net::TcpListener;
use tokio::sync::RwLock;

use crate::core::config;
use crate::core::copyright::get_bot_username;
use crate::i18n;
use crate::storage::db::DbPool;
use crate::storage::{get_connection, SharePageRecord, SharedStorage};

// --- Rate limiters ---

static AUTH_RATE_LIMIT: LazyLock<RwLock<HashMap<String, (u32, std::time::Instant)>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

static SHARE_RATE_LIMIT: LazyLock<RwLock<HashMap<String, (u32, std::time::Instant)>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

const AUTH_MAX_ATTEMPTS: u32 = 10;
const AUTH_WINDOW_SECS: u64 = 300;
const SHARE_MAX_PER_MIN: u32 = 60;
const SHARE_WINDOW_SECS: u64 = 60;

/// Shared state for the web server.
#[derive(Clone)]
struct WebState {
    shared_storage: Arc<SharedStorage>,
    bot_token: String,
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

// --- Admin API types ---

#[derive(Deserialize)]
struct UserQuery {
    page: Option<u32>,
    filter: Option<String>,
    search: Option<String>,
}

#[derive(Deserialize)]
struct DownloadQuery {
    page: Option<u32>,
    search: Option<String>,
}

#[derive(Deserialize)]
struct PlanUpdateReq {
    plan: String,
}

#[derive(Deserialize)]
struct BlockUpdateReq {
    blocked: bool,
}

#[derive(Serialize)]
struct ApiUser {
    telegram_id: i64,
    username: String,
    plan: String,
    is_blocked: bool,
    download_count: i64,
    language: String,
}

#[derive(Serialize)]
struct ApiDownload {
    id: i64,
    title: String,
    author: String,
    user: String,
    user_id: i64,
    format: String,
    file_size: Option<i64>,
    duration: Option<i64>,
    video_quality: String,
    audio_bitrate: String,
    downloaded_at: String,
    url: String,
}

#[derive(Serialize)]
struct PaginatedResponse<T: Serialize> {
    items: Vec<T>,
    total: i64,
    page: u32,
    per_page: u32,
    total_pages: u32,
}

// --- Queue API types ---

#[derive(Deserialize)]
struct QueueQuery {
    page: Option<u32>,
    status: Option<String>,
    search: Option<String>,
}

#[derive(Serialize)]
struct ApiQueueTask {
    id: String,
    user_id: i64,
    username: String,
    url: String,
    format: String,
    status: String,
    error_message: String,
    retry_count: i32,
    worker_id: String,
    created_at: String,
    started_at: String,
    finished_at: String,
}

// --- Error API types ---

#[derive(Deserialize)]
struct ErrorQuery {
    page: Option<u32>,
    error_type: Option<String>,
    resolved: Option<String>,
    search: Option<String>,
}

#[derive(Serialize)]
struct ApiError {
    id: i64,
    timestamp: String,
    user_id: Option<i64>,
    username: String,
    error_type: String,
    error_message: String,
    url: String,
    context: String,
    resolved: bool,
}

// --- Feedback API types ---

#[derive(Deserialize)]
struct FeedbackQuery {
    page: Option<u32>,
    status: Option<String>,
    search: Option<String>,
}

#[derive(Serialize)]
struct ApiFeedback {
    id: i64,
    user_id: i64,
    username: String,
    first_name: String,
    message: String,
    status: String,
    admin_reply: String,
    created_at: String,
}

#[derive(Deserialize)]
struct FeedbackStatusReq {
    status: String,
}

// --- Alert API types ---

#[derive(Deserialize)]
struct AlertQuery {
    page: Option<u32>,
    severity: Option<String>,
    search: Option<String>,
}

#[derive(Serialize)]
struct ApiAlert {
    id: i64,
    alert_type: String,
    severity: String,
    message: String,
    metadata: String,
    triggered_at: String,
    resolved_at: String,
    acknowledged: bool,
}

// --- Revenue API types ---

#[derive(Deserialize)]
struct RevenueQuery {
    page: Option<u32>,
    plan: Option<String>,
}

#[derive(Serialize)]
struct ApiChargeEntry {
    id: i64,
    user_id: i64,
    username: String,
    plan: String,
    amount: i64,
    currency: String,
    is_recurring: bool,
    payment_date: String,
}

// --- Analytics API types ---

#[derive(Deserialize)]
struct AnalyticsQuery {
    days: Option<u32>,
}

// --- User settings API types ---

#[derive(Deserialize)]
struct UserSettingsReq {
    language: Option<String>,
    plan: Option<String>,
    plan_days: Option<i32>,
    is_blocked: Option<bool>,
}

// --- Broadcast API types ---

#[derive(Deserialize)]
struct BroadcastReq {
    target: String,
    message: String,
}

// --- Audit log API types ---

#[derive(Deserialize)]
struct AuditQuery {
    page: Option<u32>,
    action: Option<String>,
}

#[derive(Serialize)]
struct ApiAuditEntry {
    id: i64,
    admin_id: i64,
    action: String,
    target_type: String,
    target_id: String,
    details: String,
    created_at: String,
}

// --- CSRF ---

static CSRF_SECRET: LazyLock<String> = LazyLock::new(|| {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let bytes: [u8; 32] = rng.gen();
    hex::encode(bytes)
});

fn generate_csrf_token(session_token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(format!("csrf:{}:{}", session_token, &*CSRF_SECRET));
    hex::encode(hasher.finalize())
}

fn verify_csrf(header_map: &HeaderMap, _bot_token: &str) -> bool {
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

// --- Audit log helper ---

fn log_audit(
    conn: &rusqlite::Connection,
    admin_id: i64,
    action: &str,
    target_type: &str,
    target_id: &str,
    details: Option<&str>,
) {
    let _ = conn.execute(
        "INSERT INTO admin_audit_log (admin_id, action, target_type, target_id, details) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![admin_id, action, target_type, target_id, details],
    );
}

// --- Bulk action types ---

#[derive(Deserialize)]
struct BulkResolveReq {
    error_type: Option<String>,
}

#[derive(Deserialize)]
struct BulkCancelReq {
    status: Option<String>,
}

/// Constant-time byte-level string comparison to prevent timing side-channels.
fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Return true only for http(s) URLs; rejects javascript:, data:, etc.
fn is_safe_url(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://")
}

/// Middleware that injects standard security headers into every response.
async fn security_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert("X-Frame-Options", "DENY".parse().unwrap());
    headers.insert("X-Content-Type-Options", "nosniff".parse().unwrap());
    headers.insert("Referrer-Policy", "strict-origin-when-cross-origin".parse().unwrap());
    headers.insert(
        "Content-Security-Policy",
        "default-src 'self'; script-src 'self' https://telegram.org 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' https: data:; connect-src 'self'; frame-src https://oauth.telegram.org; frame-ancestors 'none'"
            .parse()
            .unwrap(),
    );
    headers.insert("Access-Control-Allow-Origin", "null".parse().unwrap());
    response
}

/// Start the public web server.
pub async fn start_web_server(port: u16, shared_storage: Arc<SharedStorage>) -> Result<(), Box<dyn std::error::Error>> {
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let bot_token = config::BOT_TOKEN.clone();
    let state = WebState {
        shared_storage,
        bot_token,
    };

    let app = Router::new()
        .route("/s/{id}", get(share_page_handler))
        .route("/api/s/{id}", get(share_api_handler))
        .route("/health", get(health_handler))
        .route("/privacy", get(privacy_handler))
        // Admin routes
        .route("/admin", get(admin_dashboard_handler))
        .route("/admin/login", get(admin_login_handler))
        .route("/admin/auth", get(admin_auth_handler))
        .route("/admin/logout", get(admin_logout_handler))
        // Admin API
        .route("/admin/api/users", get(admin_api_users))
        .route("/admin/api/users/{id}/plan", post(admin_api_user_plan))
        .route("/admin/api/users/{id}/block", post(admin_api_user_block))
        .route("/admin/api/downloads", get(admin_api_downloads))
        // Queue API
        .route("/admin/api/queue", get(admin_api_queue))
        .route("/admin/api/queue/{id}/retry", post(admin_api_queue_retry))
        .route("/admin/api/queue/{id}/cancel", post(admin_api_queue_cancel))
        // Errors API (paginated)
        .route("/admin/api/errors", get(admin_api_errors))
        .route("/admin/api/errors/{id}/resolve", post(admin_api_error_resolve))
        // Feedback API
        .route("/admin/api/feedback", get(admin_api_feedback))
        .route("/admin/api/feedback/{id}/status", post(admin_api_feedback_status))
        // Alerts API
        .route("/admin/api/alerts", get(admin_api_alerts))
        .route("/admin/api/alerts/{id}/acknowledge", post(admin_api_alert_acknowledge))
        // User details + Health + Broadcast + Revenue + Analytics
        .route("/admin/api/users/{id}/details", get(admin_api_user_details))
        .route("/admin/api/users/{id}/settings", post(admin_api_user_settings))
        .route("/admin/api/health", get(admin_api_health))
        .route("/admin/api/broadcast", post(admin_api_broadcast))
        .route("/admin/api/revenue", get(admin_api_revenue))
        .route("/admin/api/analytics", get(admin_api_analytics))
        .route("/admin/api/audit", get(admin_api_audit))
        // Bulk actions
        .route("/admin/api/errors/bulk-resolve", post(admin_api_errors_bulk_resolve))
        .route("/admin/api/queue/bulk-cancel", post(admin_api_queue_bulk_cancel))
        // Lightweight polling for tab badges
        .route("/admin/api/counts", get(admin_api_counts))
        .route("/metrics", get(metrics_handler))
        .with_state(state)
        .layer(DefaultBodyLimit::max(1024 * 1024)) // 1 MB
        .layer(middleware::from_fn(security_headers));

    log::info!("Starting web server on http://{}", addr);
    log::info!("  /s/:id      - Share page (HTML)");
    log::info!("  /api/s/:id  - Share page (JSON)");
    log::info!("  /privacy    - Privacy Policy");
    log::info!("  /admin      - Admin Dashboard");
    log::info!("  /health     - Health check");
    log::info!("  /metrics    - Prometheus metrics (Bearer auth)");

    let listener = TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

/// GET /metrics — Prometheus metrics endpoint, protected by Bearer token.
///
/// Returns 404 if METRICS_AUTH_TOKEN is not set (disabled).
/// Returns 401 if the Authorization header doesn't match.
async fn metrics_handler(headers: HeaderMap) -> Response {
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

/// GET /privacy — renders the privacy policy HTML.
async fn privacy_handler(headers: HeaderMap, Query(params): Query<BTreeMap<String, String>>) -> Response {
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

fn render_privacy_page(lang: &str) -> String {
    let title = match lang {
        "en" => "Privacy Policy — Doradura",
        "ru" => "Политика конфиденциальности — Doradura",
        "fr" => "Politique de confidentialité — Doradura",
        "de" => "Datenschutzerklärung — Doradura",
        _ => "Privacy Policy",
    };

    let content = match lang {
        "ru" => {
            r#"
            <h1>Политика конфиденциальности</h1>
            <p class="lead">Мы уважаем вашу конфиденциальность и стремимся защищать ваши персональные данные.</p>
            
            <section>
                <h2>1. Общие положения</h2>
                <p>Настоящая политика объясняет, как сервис Doradura обрабатывает данные пользователей. Мы минимизируем сбор данных и храним только то, что необходимо для работы сервиса.</p>
            </section>

            <section>
                <h2>2. Какие данные мы собираем</h2>
                <ul>
                    <li><strong>Telegram ID и юзернейм:</strong> Для идентификации вашего аккаунта и предоставления доступа к функциям бота.</li>
                    <li><strong>История загрузок:</strong> Метаданные (название, ссылка, дата) для статистики и повторного доступа к файлам.</li>
                    <li><strong>Настройки:</strong> Выбранный язык, предпочтительное качество видео и аудио.</li>
                </ul>
            </section>

            <section>
                <h2>3. Как мы используем данные</h2>
                <p>Ваши данные используются исключительно для:</p>
                <ul>
                    <li>Обеспечения работы функций загрузки и конвертации.</li>
                    <li>Улучшения качества сервиса на основе анонимной статистики.</li>
                    <li>Предоставления персональных настроек и доступа к подпискам.</li>
                </ul>
            </section>

            <section>
                <h2>4. Безопасность и хранение</h2>
                <p>Мы используем современные методы шифрования для защиты базы данных. Ваши файлы не хранятся на наших серверах долго — они удаляются автоматически через 10 минут после отправки.</p>
            </section>

            <section>
                <h2>5. Права пользователей</h2>
                <p>Вы имеете право запросить удаление всех ваших данных из нашей системы через команду /settings или обратившись к администратору.</p>
            </section>
        "#
        }
        "fr" => {
            r#"
            <h1>Politique de confidentialité</h1>
            <p class="lead">Nous respectons votre vie privée et nous nous engageons à protéger vos données personnelles.</p>
            
            <section>
                <h2>1. Dispositions générales</h2>
                <p>Cette politique explique comment Doradura traite les données des utilisateurs. Nous minimisons la collecte de données et ne conservons que ce qui est nécessaire au fonctionnement du service.</p>
            </section>

            <section>
                <h2>2. Données collectées</h2>
                <ul>
                    <li><strong>ID Telegram et nom d'utilisateur :</strong> Pour identifier votre compte et fournir l'accès aux fonctions du bot.</li>
                    <li><strong>Historique des téléchargements :</strong> Métadonnées (titre, lien, date) pour les statistiques et l'accès répété aux fichiers.</li>
                    <li><strong>Paramètres :</strong> Langue choisie, qualité vidéo et audio préférée.</li>
                </ul>
            </section>

            <section>
                <h2>3. Utilisation des données</h2>
                <p>Vos données sont utilisées exclusivement pour :</p>
                <ul>
                    <li>Assurer le fonctionnement des fonctions de téléchargement et de conversion.</li>
                    <li>Améliorer la qualité du service sur la base de statistiques anonymes.</li>
                    <li>Fournir des paramètres personnels et l'accès aux abonnements.</li>
                </ul>
            </section>

            <section>
                <h2>4. Sécurité et stockage</h2>
                <p>Nous utilisons des méthodes de cryptage modernes pour protéger la base de données. Vos fichiers ne sont pas stockés longtemps sur nos serveurs — ils sont supprimés automatiquement 10 minutes après l'envoi.</p>
            </section>
        "#
        }
        "de" => {
            r#"
            <h1>Datenschutzerklärung</h1>
            <p class="lead">Wir respektieren Ihre Privatsphäre und setzen uns für den Schutz Ihrer personenbezogenen Daten ein.</p>
            
            <section>
                <h2>1. Allgemeine Bestimmungen</h2>
                <p>Diese Richtlinie erklärt, wie Doradura Benutzerdaten verarbeitet. Wir minimieren die Datenerhebung und speichern nur das, was für den Betrieb des Dienstes notwendig ist.</p>
            </section>

            <section>
                <h2>2. Welche Daten wir sammeln</h2>
                <ul>
                    <li><strong>Telegram ID und Benutzername:</strong> Um Ihr Konto zu identifizieren und Zugriff auf die Bot-Funktionen zu gewähren.</li>
                    <li><strong>Download-Verlauf:</strong> Metadaten (Titel, Link, Datum) für Statistiken und wiederholten Zugriff auf Dateien.</li>
                    <li><strong>Einstellungen:</strong> Gewählte Sprache, bevorzugte Video- und Audioqualität.</li>
                </ul>
            </section>

            <section>
                <h2>3. Verwendung der Daten</h2>
                <p>Ihre Daten werden ausschließlich verwendet für:</p>
                <ul>
                    <li>Bereitstellung von Download- und Konvertierungsfunktionen.</li>
                    <li>Verbesserung der Servicequalität auf Basis anonymer Statistiken.</li>
                    <li>Bereitstellung persönlicher Einstellungen und Zugriff auf Abonnements.</li>
                </ul>
            </section>

            <section>
                <h2>4. Sicherheit und Speicherung</h2>
                <p>Wir verwenden moderne Verschlüsselungsmethoden, um die Datenbank zu schützen. Ihre Dateien werden nicht lange auf unseren Servern gespeichert — sie werden 10 Minuten nach dem Senden automatisch gelöscht.</p>
            </section>
        "#
        }
        _ => {
            r#"
            <h1>Privacy Policy</h1>
            <p class="lead">We respect your privacy and are committed to protecting your personal data.</p>
            
            <section>
                <h2>1. General Provisions</h2>
                <p>This policy explains how Doradura processes user data. We minimize data collection and only store what is necessary for the service to function.</p>
            </section>

            <section>
                <h2>2. Data We Collect</h2>
                <ul>
                    <li><strong>Telegram ID and Username:</strong> To identify your account and provide access to the bot's features.</li>
                    <li><strong>Download History:</strong> Metadata (title, link, date) for statistics and repeat access to files.</li>
                    <li><strong>Settings:</strong> Chosen language, preferred video and audio quality.</li>
                </ul>
            </section>

            <section>
                <h2>3. How We Use Data</h2>
                <p>Your data is used exclusively to:</p>
                <ul>
                    <li>Ensure the operation of download and conversion functions.</li>
                    <li>Improve service quality based on anonymous statistics.</li>
                    <li>Provide personal settings and access to subscriptions.</li>
                </ul>
            </section>

            <section>
                <h2>4. Security and Storage</h2>
                <p>We use modern encryption methods to protect the database. Your files are not stored on our servers for long — they are deleted automatically 10 minutes after sending.</p>
            </section>

            <section>
                <h2>5. User Rights</h2>
                <p>You have the right to request the deletion of all your data from our system via the /settings command or by contacting the administrator.</p>
            </section>
        "#
        }
    };

    let lang_switcher = format!(
        r#"<div class="lang-switcher">
            <a href="/privacy?lang=en" class="{en_active}">EN</a>
            <a href="/privacy?lang=ru" class="{ru_active}">RU</a>
            <a href="/privacy?lang=fr" class="{fr_active}">FR</a>
            <a href="/privacy?lang=de" class="{de_active}">DE</a>
        </div>"#,
        en_active = if lang == "en" { "active" } else { "" },
        ru_active = if lang == "ru" { "active" } else { "" },
        fr_active = if lang == "fr" { "active" } else { "" },
        de_active = if lang == "de" { "active" } else { "" },
    );

    format!(
        r#"<!DOCTYPE html>
<html lang="{lang}">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>{title}</title>
    <style>
        :root {{
            --bg: #0d0d0d;
            --surface: #141414;
            --text: #e0e0e0;
            --muted: #888;
            --accent: #7c6aff;
            --border: #252525;
        }}
        body {{
            background: var(--bg);
            color: var(--text);
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', system-ui, sans-serif;
            line-height: 1.6;
            margin: 0;
            padding: 40px 20px;
            display: flex;
            justify-content: center;
        }}
        .container {{
            max-width: 700px;
            width: 100%;
        }}
        .logo {{
            font-size: 1.5rem;
            font-weight: 800;
            margin-bottom: 40px;
            text-align: center;
        }}
        .logo span {{ color: var(--accent); }}
        h1 {{ font-size: 2rem; font-weight: 700; margin-bottom: 16px; color: #fff; }}
        h2 {{ font-size: 1.25rem; font-weight: 600; margin-top: 32px; margin-bottom: 12px; color: #fff; }}
        p {{ margin-bottom: 16px; color: var(--text); }}
        .lead {{ font-size: 1.15rem; color: var(--muted); margin-bottom: 32px; }}
        ul {{ margin-bottom: 24px; padding-left: 20px; }}
        li {{ margin-bottom: 8px; }}
        strong {{ color: #fff; }}
        section {{ border-top: 1px solid var(--border); padding-top: 8px; margin-top: 32px; }}
        .lang-switcher {{
            display: flex;
            justify-content: center;
            gap: 12px;
            margin-bottom: 40px;
        }}
        .lang-switcher a {{
            color: var(--muted);
            text-decoration: none;
            font-size: 0.85rem;
            font-weight: 600;
            padding: 4px 12px;
            border-radius: 6px;
            border: 1px solid var(--border);
            transition: all 0.2s;
        }}
        .lang-switcher a:hover {{ border-color: var(--accent); color: #fff; }}
        .lang-switcher a.active {{ background: var(--accent); border-color: var(--accent); color: #fff; }}
        footer {{
            margin-top: 60px;
            padding-top: 20px;
            border-top: 1px solid var(--border);
            text-align: center;
            color: var(--muted);
            font-size: 0.85rem;
        }}
    </style>
</head>
<body>
    <div class="container">
        <div class="logo">dora<span>dura</span></div>
        {lang_switcher}
        <article>
            {content}
        </article>
        <footer>
            &copy; 2026 Doradura. All rights reserved.
        </footer>
    </div>
</body>
</html>"#,
        lang = lang,
        title = title,
        lang_switcher = lang_switcher,
        content = content
    )
}

/// Extract best-effort IP string from request headers.
fn extract_ip(header_map: &HeaderMap) -> String {
    header_map
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .unwrap_or("unknown")
        .trim()
        .to_string()
}

/// Check and increment a rate-limit bucket. Returns `true` if the request is allowed.
async fn check_rate_limit(
    limiter: &RwLock<HashMap<String, (u32, std::time::Instant)>>,
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

/// GET /s/:id — renders the share page HTML.
async fn share_page_handler(Path(id): Path<String>, State(state): State<WebState>, header_map: HeaderMap) -> Response {
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
async fn share_api_handler(Path(id): Path<String>, State(state): State<WebState>, header_map: HeaderMap) -> Response {
    let ip = extract_ip(&header_map);
    if !check_rate_limit(&SHARE_RATE_LIMIT, &ip, SHARE_MAX_PER_MIN, SHARE_WINDOW_SECS).await {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({"error": "Too many requests"})),
        )
            .into_response();
    }

    let Some(row) = state.shared_storage.get_share_page_record(&id).await.ok().flatten() else {
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

/// GET /admin/auth — Telegram authentication callback.
async fn admin_auth_handler(
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

/// GET /admin/logout — Clear admin cookie and redirect to login.
async fn admin_logout_handler() -> Response {
    let cookie = "admin_token=; Path=/admin; HttpOnly; Secure; SameSite=Lax; Max-Age=0";
    Response::builder()
        .status(StatusCode::FOUND)
        .header("Location", "/admin/login")
        .header("Set-Cookie", cookie)
        .body(Body::empty())
        .unwrap()
        .into_response()
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
            if constant_time_eq(&generate_admin_token(admin_id, &state.bot_token), token_val) {
                authed_user_id = Some(admin_id);
                break;
            }
        }
        if authed_user_id.is_none()
            && *config::admin::ADMIN_USER_ID != 0
            && constant_time_eq(
                &generate_admin_token(*config::admin::ADMIN_USER_ID, &state.bot_token),
                token_val,
            )
        {
            authed_user_id = Some(*config::admin::ADMIN_USER_ID);
        }
    }

    if authed_user_id.is_none() {
        return Redirect::to("/admin/login").into_response();
    }

    // 2. Fetch stats (sync SQLite — offload to blocking thread pool)
    let db = state.shared_storage.sqlite_pool();
    let stats = match tokio::task::spawn_blocking(move || fetch_admin_stats(&db)).await {
        Ok(s) => s,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "DB error").into_response(),
    };

    // 3. Generate CSRF token from admin cookie
    let admin_token = cookie_str
        .split(';')
        .find(|s| s.trim().starts_with("admin_token="))
        .and_then(|t| t.trim().strip_prefix("admin_token="))
        .unwrap_or("");
    let csrf_token = generate_csrf_token(admin_token);

    // 4. Render Dashboard
    let html = render_admin_dashboard(&stats, &csrf_token);
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
    constant_time_eq(&hex::encode(result), &auth.hash)
}

/// Generate a secure token for the admin cookie.
fn generate_admin_token(user_id: i64, bot_token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(format!("{}:{}", user_id, bot_token));
    hex::encode(hasher.finalize())
}

// --- Admin auth helper ---

/// Verify admin cookie + CSRF token for POST requests.
#[allow(clippy::result_large_err)]
fn verify_admin_post(header_map: &HeaderMap, bot_token: &str) -> Result<i64, Response> {
    let admin_id = verify_admin(header_map, bot_token)?;
    if !verify_csrf(header_map, bot_token) {
        return Err((StatusCode::FORBIDDEN, "Invalid CSRF token").into_response());
    }
    Ok(admin_id)
}

/// Verify admin cookie and return admin user ID, or an error response.
#[allow(clippy::result_large_err)]
fn verify_admin(header_map: &HeaderMap, bot_token: &str) -> Result<i64, Response> {
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

// --- Admin API handlers ---

const USERS_PER_PAGE: u32 = 50;
const DOWNLOADS_PER_PAGE: u32 = 50;

/// GET /admin/api/users — paginated, filterable user list.
async fn admin_api_users(State(state): State<WebState>, header_map: HeaderMap, Query(q): Query<UserQuery>) -> Response {
    if let Err(resp) = verify_admin(&header_map, &state.bot_token) {
        return resp;
    }
    let page = q.page.unwrap_or(1).max(1);
    let filter = q.filter.unwrap_or_else(|| "all".to_string());
    let search = q.search.unwrap_or_default();
    let offset = ((page - 1) * USERS_PER_PAGE) as i64;
    let db = state.shared_storage.sqlite_pool();

    let result = tokio::task::spawn_blocking(move || -> Result<PaginatedResponse<ApiUser>, rusqlite::Error> {
        let conn = get_connection(&db).map_err(|_| rusqlite::Error::InvalidQuery)?;

        // Build WHERE clause with owned string parameters
        let mut conditions = Vec::new();

        match filter.as_str() {
            "free" => conditions.push("COALESCE(u.plan, 'free') = 'free'".to_string()),
            "premium" => conditions.push("u.plan = 'premium'".to_string()),
            "vip" => conditions.push("u.plan = 'vip'".to_string()),
            "blocked" => conditions.push("u.is_blocked = 1".to_string()),
            _ => {}
        }

        let search_param = if !search.is_empty() {
            conditions.push("(u.username LIKE ?1 OR CAST(u.telegram_id AS TEXT) LIKE ?1)".to_string());
            Some(format!("%{}%", search))
        } else {
            None
        };

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        let count_sql = format!("SELECT COUNT(*) FROM users u {}", where_clause);
        let total: i64 = if let Some(ref sp) = search_param {
            conn.query_row(&count_sql, rusqlite::params![sp], |r| r.get(0))
                .unwrap_or(0)
        } else {
            conn.query_row(&count_sql, [], |r| r.get(0)).unwrap_or(0)
        };

        let total_pages = ((total as f64) / USERS_PER_PAGE as f64).ceil() as u32;

        let query_sql = format!(
            "SELECT u.telegram_id, COALESCE(u.username, ''), COALESCE(u.plan, 'free'), \
                    COALESCE(u.is_blocked, 0), COALESCE(u.language, 'ru'), \
                    COUNT(d.id) AS dl_count \
             FROM users u \
             LEFT JOIN download_history d ON d.user_id = u.telegram_id \
             {} \
             GROUP BY u.telegram_id \
             ORDER BY dl_count DESC \
             LIMIT {} OFFSET {}",
            where_clause, USERS_PER_PAGE, offset
        );

        let users: Vec<ApiUser> = if let Some(ref sp) = search_param {
            conn.prepare(&query_sql)
                .and_then(|mut s| {
                    let rows = s.query_map(rusqlite::params![sp], |r| {
                        Ok(ApiUser {
                            telegram_id: r.get(0)?,
                            username: r.get(1)?,
                            plan: r.get(2)?,
                            is_blocked: r.get::<_, i64>(3)? != 0,
                            language: r.get(4)?,
                            download_count: r.get(5)?,
                        })
                    })?;
                    Ok(rows.filter_map(|r| r.ok()).collect())
                })
                .unwrap_or_default()
        } else {
            conn.prepare(&query_sql)
                .and_then(|mut s| {
                    let rows = s.query_map([], |r| {
                        Ok(ApiUser {
                            telegram_id: r.get(0)?,
                            username: r.get(1)?,
                            plan: r.get(2)?,
                            is_blocked: r.get::<_, i64>(3)? != 0,
                            language: r.get(4)?,
                            download_count: r.get(5)?,
                        })
                    })?;
                    Ok(rows.filter_map(|r| r.ok()).collect())
                })
                .unwrap_or_default()
        };

        Ok(PaginatedResponse {
            items: users,
            total,
            page,
            per_page: USERS_PER_PAGE,
            total_pages,
        })
    })
    .await;

    match result {
        Ok(Ok(data)) => Json(data).into_response(),
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "DB error").into_response(),
    }
}

/// POST /admin/api/users/:id/plan — change user plan.
async fn admin_api_user_plan(
    State(state): State<WebState>,
    header_map: HeaderMap,
    Path(user_id): Path<i64>,
    Json(body): Json<PlanUpdateReq>,
) -> Response {
    let admin_id = match verify_admin_post(&header_map, &state.bot_token) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let valid_plans = ["free", "premium", "vip"];
    if !valid_plans.contains(&body.plan.as_str()) {
        return (StatusCode::BAD_REQUEST, "Invalid plan").into_response();
    }
    let db = state.shared_storage.sqlite_pool();
    let plan = body.plan.clone();
    let result = tokio::task::spawn_blocking(move || {
        let conn = get_connection(&db).map_err(|_| rusqlite::Error::InvalidQuery)?;
        let n = conn.execute(
            "UPDATE users SET plan = ?1 WHERE telegram_id = ?2",
            rusqlite::params![plan, user_id],
        )?;
        if n > 0 {
            log_audit(
                &conn,
                admin_id,
                "plan_change",
                "user",
                &user_id.to_string(),
                Some(&format!("plan={}", plan)),
            );
        }
        Ok::<_, rusqlite::Error>(n)
    })
    .await;

    match result {
        Ok(Ok(0)) => (StatusCode::NOT_FOUND, "User not found").into_response(),
        Ok(Ok(_)) => {
            log::info!("Admin {} changed plan for user {} to {}", admin_id, user_id, body.plan);
            Json(json!({"ok": true, "plan": body.plan})).into_response()
        }
        Ok(Err(e)) => {
            log::error!("Failed to update plan: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "DB error").into_response()
        }
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "DB error").into_response(),
    }
}

/// POST /admin/api/users/:id/block — block/unblock user.
async fn admin_api_user_block(
    State(state): State<WebState>,
    header_map: HeaderMap,
    Path(user_id): Path<i64>,
    Json(body): Json<BlockUpdateReq>,
) -> Response {
    let admin_id = match verify_admin_post(&header_map, &state.bot_token) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let db = state.shared_storage.sqlite_pool();
    let blocked = body.blocked;
    let result = tokio::task::spawn_blocking(move || {
        let conn = get_connection(&db).map_err(|_| rusqlite::Error::InvalidQuery)?;
        let blocked_val: i64 = if blocked { 1 } else { 0 };
        let n = conn.execute(
            "UPDATE users SET is_blocked = ?1 WHERE telegram_id = ?2",
            rusqlite::params![blocked_val, user_id],
        )?;
        if n > 0 {
            let action = if blocked { "block" } else { "unblock" };
            log_audit(&conn, admin_id, action, "user", &user_id.to_string(), None);
        }
        Ok::<_, rusqlite::Error>(n)
    })
    .await;

    match result {
        Ok(Ok(0)) => (StatusCode::NOT_FOUND, "User not found").into_response(),
        Ok(Ok(_)) => {
            log::info!(
                "Admin {} {} user {}",
                admin_id,
                if body.blocked { "blocked" } else { "unblocked" },
                user_id
            );
            Json(json!({"ok": true, "blocked": body.blocked})).into_response()
        }
        Ok(Err(e)) => {
            log::error!("Failed to update block status: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "DB error").into_response()
        }
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "DB error").into_response(),
    }
}

/// GET /admin/api/downloads — paginated download history with full details.
async fn admin_api_downloads(
    State(state): State<WebState>,
    header_map: HeaderMap,
    Query(q): Query<DownloadQuery>,
) -> Response {
    if let Err(resp) = verify_admin(&header_map, &state.bot_token) {
        return resp;
    }
    let page = q.page.unwrap_or(1).max(1);
    let search = q.search.unwrap_or_default();
    let offset = ((page - 1) * DOWNLOADS_PER_PAGE) as i64;
    let db = state.shared_storage.sqlite_pool();

    let result = tokio::task::spawn_blocking(move || -> Result<PaginatedResponse<ApiDownload>, rusqlite::Error> {
        let conn = get_connection(&db).map_err(|_| rusqlite::Error::InvalidQuery)?;

        let search_param: Option<String> = if search.is_empty() {
            None
        } else {
            Some(format!("%{}%", search))
        };

        let where_clause = if search_param.is_some() {
            "WHERE d.title LIKE ?1 OR COALESCE(d.author,'') LIKE ?1 OR COALESCE(u.username,'') LIKE ?1 OR CAST(d.user_id AS TEXT) LIKE ?1"
        } else {
            ""
        };

        let count_sql = format!(
            "SELECT COUNT(*) FROM download_history d LEFT JOIN users u ON u.telegram_id = d.user_id {}",
            where_clause
        );
        let total: i64 = if let Some(ref sp) = search_param {
            conn.query_row(&count_sql, rusqlite::params![sp], |r| r.get(0)).unwrap_or(0)
        } else {
            conn.query_row(&count_sql, [], |r| r.get(0)).unwrap_or(0)
        };

        let total_pages = ((total as f64) / DOWNLOADS_PER_PAGE as f64).ceil() as u32;

        let query_sql = format!(
            "SELECT d.id, COALESCE(d.title, ''), COALESCE(d.author, ''), \
                    COALESCE(u.username, CAST(d.user_id AS TEXT)), d.user_id, \
                    COALESCE(d.format, '?'), d.file_size, d.duration, \
                    COALESCE(d.video_quality, ''), COALESCE(d.audio_bitrate, ''), \
                    d.downloaded_at, COALESCE(d.url, '') \
             FROM download_history d \
             LEFT JOIN users u ON u.telegram_id = d.user_id \
             {} \
             ORDER BY d.downloaded_at DESC \
             LIMIT {} OFFSET {}",
            where_clause, DOWNLOADS_PER_PAGE, offset
        );

        let map_row = |r: &rusqlite::Row<'_>| -> rusqlite::Result<ApiDownload> {
            Ok(ApiDownload {
                id: r.get(0)?,
                title: r.get(1)?,
                author: r.get(2)?,
                user: r.get(3)?,
                user_id: r.get(4)?,
                format: r.get(5)?,
                file_size: r.get(6)?,
                duration: r.get(7)?,
                video_quality: r.get(8)?,
                audio_bitrate: r.get(9)?,
                downloaded_at: r.get(10)?,
                url: r.get(11)?,
            })
        };

        let downloads: Vec<ApiDownload> = if let Some(ref sp) = search_param {
            conn.prepare(&query_sql)
                .and_then(|mut s| {
                    let rows = s.query_map(rusqlite::params![sp], map_row)?;
                    Ok(rows.filter_map(|r| r.ok()).collect())
                })
                .unwrap_or_default()
        } else {
            conn.prepare(&query_sql)
                .and_then(|mut s| {
                    let rows = s.query_map([], map_row)?;
                    Ok(rows.filter_map(|r| r.ok()).collect())
                })
                .unwrap_or_default()
        };

        Ok(PaginatedResponse { items: downloads, total, page, per_page: DOWNLOADS_PER_PAGE, total_pages })
    })
    .await;

    match result {
        Ok(Ok(data)) => Json(data).into_response(),
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "DB error").into_response(),
    }
}

// --- Queue API ---

const QUEUE_PER_PAGE: u32 = 50;

/// GET /admin/api/queue — paginated task queue with status filter.
async fn admin_api_queue(
    State(state): State<WebState>,
    header_map: HeaderMap,
    Query(q): Query<QueueQuery>,
) -> Response {
    if let Err(resp) = verify_admin(&header_map, &state.bot_token) {
        return resp;
    }
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
                "(t.url LIKE ?1 OR COALESCE(u.username,'') LIKE ?1 \
                 OR CAST(t.user_id AS TEXT) LIKE ?1 OR t.id LIKE ?1)"
                    .to_string(),
            );
            Some(format!("%{}%", search))
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
async fn admin_api_queue_retry(
    State(state): State<WebState>,
    header_map: HeaderMap,
    Path(task_id): Path<String>,
) -> Response {
    let admin_id = match verify_admin_post(&header_map, &state.bot_token) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let db = state.shared_storage.sqlite_pool();
    let tid = task_id.clone();
    let result = tokio::task::spawn_blocking(move || {
        let conn = get_connection(&db).map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "UPDATE task_queue SET status = 'pending', error_message = NULL, retry_count = 0, \
             worker_id = NULL, leased_at = NULL, lease_expires_at = NULL \
             WHERE id = ?1 AND status IN ('dead_letter', 'failed')",
            rusqlite::params![tid],
        )
    })
    .await;

    match result {
        Ok(Ok(0)) => (StatusCode::NOT_FOUND, "Task not found or not retryable").into_response(),
        Ok(Ok(_)) => {
            log::info!("Admin {} retried task {}", admin_id, task_id);
            Json(json!({"ok": true})).into_response()
        }
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "DB error").into_response(),
    }
}

/// POST /admin/api/queue/:id/cancel — cancel a pending/leased task.
async fn admin_api_queue_cancel(
    State(state): State<WebState>,
    header_map: HeaderMap,
    Path(task_id): Path<String>,
) -> Response {
    let admin_id = match verify_admin_post(&header_map, &state.bot_token) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let db = state.shared_storage.sqlite_pool();
    let tid = task_id.clone();
    let result = tokio::task::spawn_blocking(move || {
        let conn = get_connection(&db).map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "UPDATE task_queue SET status = 'dead_letter', error_message = 'Cancelled by admin' \
             WHERE id = ?1 AND status IN ('pending', 'leased')",
            rusqlite::params![tid],
        )
    })
    .await;

    match result {
        Ok(Ok(0)) => (StatusCode::NOT_FOUND, "Task not found or not cancellable").into_response(),
        Ok(Ok(_)) => {
            log::info!("Admin {} cancelled task {}", admin_id, task_id);
            Json(json!({"ok": true})).into_response()
        }
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "DB error").into_response(),
    }
}

// --- Errors API (paginated) ---

const ERRORS_PER_PAGE: u32 = 50;

/// GET /admin/api/errors — paginated, filterable error log.
async fn admin_api_errors(
    State(state): State<WebState>,
    header_map: HeaderMap,
    Query(q): Query<ErrorQuery>,
) -> Response {
    if let Err(resp) = verify_admin(&header_map, &state.bot_token) {
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
            conditions.push("error_type LIKE ?1".to_string());
            Some(format!("%{}%", type_filter))
        } else if !search_filter.is_empty() {
            conditions.push(
                "(error_message LIKE ?1 OR COALESCE(error_type,'') LIKE ?1 \
                 OR COALESCE(url,'') LIKE ?1 OR CAST(COALESCE(user_id,0) AS TEXT) LIKE ?1)"
                    .to_string(),
            );
            Some(format!("%{}%", search_filter))
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
async fn admin_api_error_resolve(
    State(state): State<WebState>,
    header_map: HeaderMap,
    Path(error_id): Path<i64>,
) -> Response {
    let admin_id = match verify_admin_post(&header_map, &state.bot_token) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let db = state.shared_storage.sqlite_pool();
    let result = tokio::task::spawn_blocking(move || {
        let conn = get_connection(&db).map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "UPDATE error_log SET resolved = 1 WHERE id = ?1",
            rusqlite::params![error_id],
        )
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

// --- Feedback API ---

const FEEDBACK_PER_PAGE: u32 = 50;

/// GET /admin/api/feedback — paginated feedback messages.
async fn admin_api_feedback(
    State(state): State<WebState>,
    header_map: HeaderMap,
    Query(q): Query<FeedbackQuery>,
) -> Response {
    if let Err(resp) = verify_admin(&header_map, &state.bot_token) {
        return resp;
    }
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
                "(message LIKE ?1 OR COALESCE(username,'') LIKE ?1 OR COALESCE(first_name,'') LIKE ?1)".to_string(),
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
async fn admin_api_feedback_status(
    State(state): State<WebState>,
    header_map: HeaderMap,
    Path(feedback_id): Path<i64>,
    Json(body): Json<FeedbackStatusReq>,
) -> Response {
    let admin_id = match verify_admin_post(&header_map, &state.bot_token) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let valid = ["new", "reviewed", "replied"];
    if !valid.contains(&body.status.as_str()) {
        return (StatusCode::BAD_REQUEST, "Invalid status").into_response();
    }
    let db = state.shared_storage.sqlite_pool();
    let status = body.status.clone();
    let result = tokio::task::spawn_blocking(move || {
        let conn = get_connection(&db).map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "UPDATE feedback_messages SET status = ?1 WHERE id = ?2",
            rusqlite::params![status, feedback_id],
        )
    })
    .await;

    match result {
        Ok(Ok(0)) => (StatusCode::NOT_FOUND, "Feedback not found").into_response(),
        Ok(Ok(_)) => {
            log::info!("Admin {} updated feedback {} to {}", admin_id, feedback_id, body.status);
            Json(json!({"ok": true, "status": body.status})).into_response()
        }
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "DB error").into_response(),
    }
}

// --- Alerts API ---

const ALERTS_PER_PAGE: u32 = 50;

/// GET /admin/api/alerts — paginated alert history.
async fn admin_api_alerts(
    State(state): State<WebState>,
    header_map: HeaderMap,
    Query(q): Query<AlertQuery>,
) -> Response {
    if let Err(resp) = verify_admin(&header_map, &state.bot_token) {
        return resp;
    }
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
            conditions.push("(COALESCE(alert_type,'') LIKE ?1 OR message LIKE ?1)".to_string());
            Some(format!("%{}%", search))
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
async fn admin_api_alert_acknowledge(
    State(state): State<WebState>,
    header_map: HeaderMap,
    Path(alert_id): Path<i64>,
) -> Response {
    let admin_id = match verify_admin_post(&header_map, &state.bot_token) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let db = state.shared_storage.sqlite_pool();
    let result = tokio::task::spawn_blocking(move || {
        let conn = get_connection(&db).map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "UPDATE alert_history SET acknowledged = 1, acknowledged_at = datetime('now') WHERE id = ?1",
            rusqlite::params![alert_id],
        )
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

// --- User Details API ---

/// GET /admin/api/users/:id/details — full user profile with stats, downloads, charges.
async fn admin_api_user_details(
    State(state): State<WebState>,
    header_map: HeaderMap,
    Path(user_id): Path<i64>,
) -> Response {
    if let Err(resp) = verify_admin(&header_map, &state.bot_token) {
        return resp;
    }
    let db = state.shared_storage.sqlite_pool();

    let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, rusqlite::Error> {
        let conn = get_connection(&db).map_err(|_| rusqlite::Error::InvalidQuery)?;

        // User info (extended with preferences)
        let user = conn.query_row(
            "SELECT u.telegram_id, COALESCE(u.username, ''), COALESCE(u.plan, 'free'), \
                    COALESCE(u.is_blocked, 0), COALESCE(u.language, 'ru'), \
                    (SELECT COUNT(*) FROM download_history WHERE user_id = u.telegram_id), \
                    COALESCE(u.download_format, 'mp3'), COALESCE(u.video_quality, 'best'), \
                    COALESCE(u.audio_bitrate, '320k'), COALESCE(u.send_as_document, 0), \
                    COALESCE(u.burn_subtitles, 0), COALESCE(u.progress_bar_style, 'classic') \
             FROM users u WHERE u.telegram_id = ?1",
            rusqlite::params![user_id],
            |r| {
                Ok(json!({
                    "telegram_id": r.get::<_, i64>(0)?,
                    "username": r.get::<_, String>(1)?,
                    "plan": r.get::<_, String>(2)?,
                    "is_blocked": r.get::<_, i64>(3)? != 0,
                    "language": r.get::<_, String>(4)?,
                    "download_count": r.get::<_, i64>(5)?,
                    "download_format": r.get::<_, String>(6)?,
                    "video_quality": r.get::<_, String>(7)?,
                    "audio_bitrate": r.get::<_, String>(8)?,
                    "send_as_document": r.get::<_, i64>(9)? != 0,
                    "burn_subtitles": r.get::<_, i64>(10)? != 0,
                    "progress_bar_style": r.get::<_, String>(11)?,
                }))
            },
        );
        let user = match user {
            Ok(u) => u,
            Err(_) => return Ok(json!({"error": "not_found"})),
        };

        // Subscription
        let sub = conn
            .query_row(
                "SELECT COALESCE(plan, 'free'), COALESCE(expires_at, ''), \
                    COALESCE(telegram_charge_id, ''), COALESCE(is_recurring, 0) \
             FROM subscriptions WHERE user_id = ?1",
                rusqlite::params![user_id],
                |r| {
                    Ok(json!({
                        "plan": r.get::<_, String>(0)?,
                        "expires_at": r.get::<_, String>(1)?,
                        "charge_id": r.get::<_, String>(2)?,
                        "is_recurring": r.get::<_, i64>(3)? != 0,
                    }))
                },
            )
            .ok();

        // Stats
        let total_downloads: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM download_history WHERE user_id = ?1",
                rusqlite::params![user_id],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let total_size: i64 = conn
            .query_row(
                "SELECT COALESCE(SUM(COALESCE(file_size, 0)), 0) FROM download_history WHERE user_id = ?1",
                rusqlite::params![user_id],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let active_days: i64 = conn
            .query_row(
                "SELECT COUNT(DISTINCT date(downloaded_at)) FROM download_history WHERE user_id = ?1",
                rusqlite::params![user_id],
                |r| r.get(0),
            )
            .unwrap_or(0);

        let top_artists: Vec<serde_json::Value> = conn
            .prepare(
                "SELECT COALESCE(author, 'Unknown'), COUNT(*) FROM download_history \
             WHERE user_id = ?1 AND author IS NOT NULL AND author != '' \
             GROUP BY author ORDER BY COUNT(*) DESC LIMIT 5",
            )
            .and_then(|mut s| {
                let rows = s.query_map(rusqlite::params![user_id], |r| {
                    Ok(json!([r.get::<_, String>(0)?, r.get::<_, i64>(1)?]))
                })?;
                Ok(rows.filter_map(|r| r.ok()).collect())
            })
            .unwrap_or_default();

        let top_formats: Vec<serde_json::Value> = conn
            .prepare(
                "SELECT COALESCE(format, 'unknown'), COUNT(*) FROM download_history \
             WHERE user_id = ?1 GROUP BY format ORDER BY COUNT(*) DESC",
            )
            .and_then(|mut s| {
                let rows = s.query_map(rusqlite::params![user_id], |r| {
                    Ok(json!([r.get::<_, String>(0)?, r.get::<_, i64>(1)?]))
                })?;
                Ok(rows.filter_map(|r| r.ok()).collect())
            })
            .unwrap_or_default();

        // Recent downloads (last 20)
        let downloads: Vec<serde_json::Value> = conn
            .prepare(
                "SELECT COALESCE(title, ''), COALESCE(author, ''), COALESCE(format, ''), \
                    file_size, duration, downloaded_at \
             FROM download_history WHERE user_id = ?1 ORDER BY downloaded_at DESC LIMIT 20",
            )
            .and_then(|mut s| {
                let rows = s.query_map(rusqlite::params![user_id], |r| {
                    Ok(json!({
                        "title": r.get::<_, String>(0)?,
                        "author": r.get::<_, String>(1)?,
                        "format": r.get::<_, String>(2)?,
                        "file_size": r.get::<_, Option<i64>>(3)?,
                        "duration": r.get::<_, Option<i64>>(4)?,
                        "downloaded_at": r.get::<_, String>(5)?,
                    }))
                })?;
                Ok(rows.filter_map(|r| r.ok()).collect())
            })
            .unwrap_or_default();

        // Charges
        let charges: Vec<serde_json::Value> = conn
            .prepare(
                "SELECT COALESCE(plan, ''), total_amount, COALESCE(currency, 'XTR'), \
                    COALESCE(is_recurring, 0), COALESCE(payment_date, '') \
             FROM charges WHERE user_id = ?1 ORDER BY payment_date DESC LIMIT 20",
            )
            .and_then(|mut s| {
                let rows = s.query_map(rusqlite::params![user_id], |r| {
                    Ok(json!({
                        "plan": r.get::<_, String>(0)?,
                        "amount": r.get::<_, i64>(1)?,
                        "currency": r.get::<_, String>(2)?,
                        "is_recurring": r.get::<_, i64>(3)? != 0,
                        "payment_date": r.get::<_, String>(4)?,
                    }))
                })?;
                Ok(rows.filter_map(|r| r.ok()).collect())
            })
            .unwrap_or_default();

        // Recent errors
        let errors: Vec<serde_json::Value> = conn
            .prepare(
                "SELECT COALESCE(timestamp, ''), COALESCE(error_type, ''), COALESCE(error_message, '') \
             FROM error_log WHERE user_id = ?1 ORDER BY timestamp DESC LIMIT 10",
            )
            .and_then(|mut s| {
                let rows = s.query_map(rusqlite::params![user_id], |r| {
                    Ok(json!({
                        "timestamp": r.get::<_, String>(0)?,
                        "error_type": r.get::<_, String>(1)?,
                        "error_message": r.get::<_, String>(2)?,
                    }))
                })?;
                Ok(rows.filter_map(|r| r.ok()).collect())
            })
            .unwrap_or_default();

        Ok(json!({
            "user": user,
            "subscription": sub,
            "stats": {
                "total_downloads": total_downloads,
                "total_size": total_size,
                "active_days": active_days,
                "top_artists": top_artists,
                "top_formats": top_formats,
            },
            "recent_downloads": downloads,
            "charges": charges,
            "errors": errors,
        }))
    })
    .await;

    match result {
        Ok(Ok(ref data)) if data.get("error").is_some() => (StatusCode::NOT_FOUND, "User not found").into_response(),
        Ok(Ok(data)) => Json(data).into_response(),
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "DB error").into_response(),
    }
}

// --- System Health API ---

/// GET /admin/api/health — system health overview.
async fn admin_api_health(State(state): State<WebState>, header_map: HeaderMap) -> Response {
    if let Err(resp) = verify_admin(&header_map, &state.bot_token) {
        return resp;
    }
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

        // Cookies check
        let cookies_path = std::env::var("COOKIES_FILE").unwrap_or_else(|_| "/data/cookies.txt".to_string());
        let cookies_exist = std::path::Path::new(&cookies_path).exists();
        let cookies_count = if cookies_exist {
            std::fs::read_to_string(&cookies_path)
                .map(|c| {
                    c.lines()
                        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
                        .count()
                })
                .unwrap_or(0)
        } else {
            0
        };
        let required_cookies = ["APISID", "SAPISID", "HSID", "SID", "SSID"];
        let cookies_content = if cookies_exist {
            std::fs::read_to_string(&cookies_path).unwrap_or_default()
        } else {
            String::new()
        };
        let mut cookies_found = serde_json::Map::new();
        for name in &required_cookies {
            cookies_found.insert(name.to_string(), json!(cookies_content.contains(name)));
        }

        // WARP proxy check
        let warp_proxy = std::env::var("WARP_PROXY").unwrap_or_default();
        let warp_ok = if !warp_proxy.is_empty() {
            std::net::TcpStream::connect_timeout(
                &warp_proxy.parse().unwrap_or_else(|_| "127.0.0.1:1080".parse().unwrap()),
                std::time::Duration::from_secs(2),
            )
            .is_ok()
        } else {
            false
        };

        // PO Token server check (port 4416)
        let pot_ok =
            std::net::TcpStream::connect_timeout(&"127.0.0.1:4416".parse().unwrap(), std::time::Duration::from_secs(2))
                .is_ok();

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

// --- Broadcast API ---

/// POST /admin/api/broadcast — send message to one user or broadcast to all.
async fn admin_api_broadcast(
    State(state): State<WebState>,
    header_map: HeaderMap,
    Json(body): Json<BroadcastReq>,
) -> Response {
    let admin_id = match verify_admin_post(&header_map, &state.bot_token) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
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

// --- Revenue API ---

const REVENUE_PER_PAGE: u32 = 50;

/// GET /admin/api/revenue — paginated charges with aggregate stats.
async fn admin_api_revenue(
    State(state): State<WebState>,
    header_map: HeaderMap,
    Query(q): Query<RevenueQuery>,
) -> Response {
    if let Err(resp) = verify_admin(&header_map, &state.bot_token) {
        return resp;
    }
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

// --- Analytics API ---

/// GET /admin/api/analytics — DAU/MAU trends, download trends.
async fn admin_api_analytics(
    State(state): State<WebState>,
    header_map: HeaderMap,
    Query(q): Query<AnalyticsQuery>,
) -> Response {
    if let Err(resp) = verify_admin(&header_map, &state.bot_token) {
        return resp;
    }
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

// --- User Settings API ---

/// POST /admin/api/users/:id/settings — update user settings from detail drawer.
async fn admin_api_user_settings(
    State(state): State<WebState>,
    header_map: HeaderMap,
    Path(user_id): Path<i64>,
    Json(body): Json<UserSettingsReq>,
) -> Response {
    let admin_id = match verify_admin_post(&header_map, &state.bot_token) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let db = state.shared_storage.sqlite_pool();
    let result = tokio::task::spawn_blocking(move || {
        let conn = get_connection(&db).map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut updated = Vec::new();

        if let Some(ref lang) = body.language {
            let valid = ["en", "ru", "fr", "de"];
            if valid.contains(&lang.as_str()) {
                conn.execute(
                    "UPDATE users SET language = ?1 WHERE telegram_id = ?2",
                    rusqlite::params![lang, user_id],
                )?;
                updated.push("language");
            }
        }
        if let Some(ref plan) = body.plan {
            let valid = ["free", "premium", "vip"];
            if valid.contains(&plan.as_str()) {
                if let Some(days) = body.plan_days {
                    let expires = format!("datetime('now', '+{} days')", days.clamp(1, 3650));
                    conn.execute(
                        &format!(
                            "INSERT OR REPLACE INTO subscriptions (user_id, plan, expires_at) \
                             VALUES (?1, ?2, {})",
                            expires
                        ),
                        rusqlite::params![user_id, plan],
                    )?;
                }
                conn.execute(
                    "UPDATE users SET plan = ?1 WHERE telegram_id = ?2",
                    rusqlite::params![plan, user_id],
                )?;
                updated.push("plan");
            }
        }
        if let Some(blocked) = body.is_blocked {
            let v: i64 = if blocked { 1 } else { 0 };
            conn.execute(
                "UPDATE users SET is_blocked = ?1 WHERE telegram_id = ?2",
                rusqlite::params![v, user_id],
            )?;
            updated.push("is_blocked");
        }

        Ok::<_, rusqlite::Error>(updated)
    })
    .await;

    match result {
        Ok(Ok(updated)) if updated.is_empty() => (StatusCode::BAD_REQUEST, "No valid fields to update").into_response(),
        Ok(Ok(updated)) => {
            log::info!("Admin {} updated user {} settings: {:?}", admin_id, user_id, updated);
            Json(json!({"ok": true, "updated": updated})).into_response()
        }
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "DB error").into_response(),
    }
}

// --- Audit Log API ---

const AUDIT_PER_PAGE: u32 = 50;

/// GET /admin/api/audit — paginated admin audit log.
async fn admin_api_audit(
    State(state): State<WebState>,
    header_map: HeaderMap,
    Query(q): Query<AuditQuery>,
) -> Response {
    if let Err(resp) = verify_admin(&header_map, &state.bot_token) {
        return resp;
    }
    let page = q.page.unwrap_or(1).max(1);
    let action_filter = q.action.unwrap_or_default();
    let offset = ((page - 1) * AUDIT_PER_PAGE) as i64;
    let db = state.shared_storage.sqlite_pool();

    let result = tokio::task::spawn_blocking(move || -> Result<PaginatedResponse<ApiAuditEntry>, rusqlite::Error> {
        let conn = get_connection(&db).map_err(|_| rusqlite::Error::InvalidQuery)?;
        let where_clause = if !action_filter.is_empty() {
            format!("WHERE action = '{}'", action_filter)
        } else {
            String::new()
        };

        let total: i64 = conn
            .query_row(
                &format!("SELECT COUNT(*) FROM admin_audit_log {}", where_clause),
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let total_pages = ((total as f64) / AUDIT_PER_PAGE as f64).ceil() as u32;

        let sql = format!(
            "SELECT id, admin_id, action, target_type, target_id, \
                    COALESCE(details, ''), created_at \
             FROM admin_audit_log {} ORDER BY created_at DESC LIMIT {} OFFSET {}",
            where_clause, AUDIT_PER_PAGE, offset
        );

        let entries: Vec<ApiAuditEntry> = conn
            .prepare(&sql)
            .and_then(|mut s| {
                let rows = s.query_map([], |r| {
                    Ok(ApiAuditEntry {
                        id: r.get(0)?,
                        admin_id: r.get(1)?,
                        action: r.get(2)?,
                        target_type: r.get(3)?,
                        target_id: r.get(4)?,
                        details: r.get(5)?,
                        created_at: r.get(6)?,
                    })
                })?;
                Ok(rows.filter_map(|r| r.ok()).collect())
            })
            .unwrap_or_default();

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

// --- Counts (lightweight polling for badges) ---

/// GET /admin/api/counts — quick counts for tab badges.
async fn admin_api_counts(State(state): State<WebState>, header_map: HeaderMap) -> Response {
    if let Err(resp) = verify_admin(&header_map, &state.bot_token) {
        return resp;
    }
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

// --- Bulk Actions ---

/// POST /admin/api/errors/bulk-resolve — resolve all errors matching type.
async fn admin_api_errors_bulk_resolve(
    State(state): State<WebState>,
    header_map: HeaderMap,
    Json(body): Json<BulkResolveReq>,
) -> Response {
    let admin_id = match verify_admin_post(&header_map, &state.bot_token) {
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

/// POST /admin/api/queue/bulk-cancel — cancel all pending/leased tasks.
async fn admin_api_queue_bulk_cancel(
    State(state): State<WebState>,
    header_map: HeaderMap,
    Json(body): Json<BulkCancelReq>,
) -> Response {
    let admin_id = match verify_admin_post(&header_map, &state.bot_token) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let db = state.shared_storage.sqlite_pool();
    let status_filter = body.status.unwrap_or_else(|| "pending".to_string());
    let result = tokio::task::spawn_blocking(move || {
        let conn = get_connection(&db).map_err(|_| rusqlite::Error::InvalidQuery)?;
        let valid = ["pending", "leased"];
        if !valid.contains(&status_filter.as_str()) {
            return Ok(0);
        }
        let n = conn.execute(
            &format!(
                "UPDATE task_queue SET status = 'dead_letter', error_message = 'Bulk cancelled by admin' \
                 WHERE status = '{}'",
                status_filter
            ),
            [],
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
            Json(json!({"ok": true, "cancelled": n})).into_response()
        }
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "DB error").into_response(),
    }
}

// --- Admin Stats ---

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

/// Format an integer with thousands separators, e.g. 1234567 -> "1,234,567".
fn fmt_num(n: i64) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    let offset = bytes.len() % 3;
    for (i, &b) in bytes.iter().enumerate() {
        if i != 0 && (i % 3 == offset) {
            out.push(',');
        }
        out.push(b as char);
    }
    out
}

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
                <div class="bar-tip">{count}</div>
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
        include_str!("admin_dashboard.html"),
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
        if is_safe_url(url) {
            btns.push_str(&format!(
                r#"<a href="{}" class="btn spotify" target="_blank" rel="noopener">Spotify</a>"#,
                html_escape(url)
            ));
        }
    }
    if let Some(url) = links.get("appleMusic").and_then(|v| v.as_str()) {
        if is_safe_url(url) {
            btns.push_str(&format!(
                r#"<a href="{}" class="btn apple" target="_blank" rel="noopener">Apple Music</a>"#,
                html_escape(url)
            ));
        }
    }
    if let Some(url) = links.get("youtubeMusic").and_then(|v| v.as_str()) {
        if is_safe_url(url) {
            btns.push_str(&format!(
                r#"<a href="{}" class="btn yt" target="_blank" rel="noopener">YouTube Music</a>"#,
                html_escape(url)
            ));
        }
    }
    if let Some(url) = links.get("deezer").and_then(|v| v.as_str()) {
        if is_safe_url(url) {
            btns.push_str(&format!(
                r#"<a href="{}" class="btn deezer" target="_blank" rel="noopener">Deezer</a>"#,
                html_escape(url)
            ));
        }
    }
    if let Some(url) = links.get("tidal").and_then(|v| v.as_str()) {
        if is_safe_url(url) {
            btns.push_str(&format!(
                r#"<a href="{}" class="btn tidal" target="_blank" rel="noopener">Tidal</a>"#,
                html_escape(url)
            ));
        }
    }
    if let Some(url) = links.get("amazonMusic").and_then(|v| v.as_str()) {
        if is_safe_url(url) {
            btns.push_str(&format!(
                r#"<a href="{}" class="btn amazon" target="_blank" rel="noopener">Amazon Music</a>"#,
                html_escape(url)
            ));
        }
    }

    // Always show the original YouTube link if it has a safe scheme
    if is_safe_url(youtube_url) {
        btns.push_str(&format!(
            r#"<a href="{}" class="btn youtube-src" target="_blank" rel="noopener">YouTube</a>"#,
            html_escape(youtube_url)
        ));
    }

    btns
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
