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
    constant_time_eq(&hex::encode(result), &auth.hash)
}

/// Generate a secure token for the admin cookie.
fn generate_admin_token(user_id: i64, bot_token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(format!("{}:{}", user_id, bot_token));
    hex::encode(hasher.finalize())
}

// --- Admin auth helper ---

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
    let admin_id = match verify_admin(&header_map, &state.bot_token) {
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
        conn.execute(
            "UPDATE users SET plan = ?1 WHERE telegram_id = ?2",
            rusqlite::params![plan, user_id],
        )
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
    let admin_id = match verify_admin(&header_map, &state.bot_token) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let db = state.shared_storage.sqlite_pool();
    let blocked = body.blocked;
    let result = tokio::task::spawn_blocking(move || {
        let conn = get_connection(&db).map_err(|_| rusqlite::Error::InvalidQuery)?;
        let blocked_val: i64 = if blocked { 1 } else { 0 };
        conn.execute(
            "UPDATE users SET is_blocked = ?1 WHERE telegram_id = ?2",
            rusqlite::params![blocked_val, user_id],
        )
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
    let admin_id = match verify_admin(&header_map, &state.bot_token) {
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
    let admin_id = match verify_admin(&header_map, &state.bot_token) {
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
    let admin_id = match verify_admin(&header_map, &state.bot_token) {
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
    let admin_id = match verify_admin(&header_map, &state.bot_token) {
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
    let admin_id = match verify_admin(&header_map, &state.bot_token) {
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

        json!({
            "ytdlp_version": ytdlp_version,
            "queue": queue,
            "errors_24h": errors_24h,
            "error_types": error_types,
            "unacked_alerts": unacked_alerts,
            "unread_feedback": unread_feedback,
            "db_size": db_size,
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
    let admin_id = match verify_admin(&header_map, &state.bot_token) {
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
    let admin_id = match verify_admin(&header_map, &state.bot_token) {
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

fn render_admin_dashboard(stats: &AdminStats) -> String {
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
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <title>Dashboard — Doradura Admin</title>
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <style>
        *, *::before, *::after {{ box-sizing: border-box; margin: 0; padding: 0; }}

        :root {{
            --bg:       #0d0d0d;
            --surface:  #141414;
            --card:     #1a1a1a;
            --border:   #252525;
            --border2:  #333;
            --text:     #e8e8e8;
            --muted:    #666;
            --accent:   #7c6aff;
            --green:    #22c55e;
            --red:      #ef4444;
            --yellow:   #f59e0b;
            --blue:     #3b82f6;
        }}

        body {{
            background: var(--bg);
            color: var(--text);
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', system-ui, sans-serif;
            font-size: 14px;
            line-height: 1.5;
            min-height: 100vh;
        }}

        /* ── Layout ── */
        .topbar {{
            position: sticky; top: 0; z-index: 100;
            background: rgba(13,13,13,0.85);
            backdrop-filter: blur(12px);
            border-bottom: 1px solid var(--border);
            padding: 0 28px;
            height: 56px;
            display: flex; align-items: center; justify-content: space-between;
        }}
        .topbar-brand {{ font-weight: 700; font-size: 1.05rem; letter-spacing: -0.3px; }}
        .topbar-brand span {{ color: var(--accent); }}
        .topbar-right {{ display: flex; align-items: center; gap: 16px; }}
        .logout {{
            color: var(--muted); text-decoration: none; font-size: 0.82rem;
            padding: 5px 12px; border: 1px solid var(--border2); border-radius: 8px;
            transition: color .15s, border-color .15s;
        }}
        .logout:hover {{ color: var(--text); border-color: #555; }}

        .page {{ max-width: 1200px; margin: 0 auto; padding: 28px 24px 60px; }}

        /* ── Tabs (CSS-only) ── */
        .tabs-wrap {{ margin-bottom: 28px; }}
        .tab-radio {{ display: none; }}

        .tab-labels {{
            display: flex; gap: 4px;
            background: var(--surface);
            border: 1px solid var(--border);
            border-radius: 12px;
            padding: 4px;
            width: fit-content;
        }}
        .tab-label {{
            padding: 7px 20px;
            border-radius: 8px;
            cursor: pointer;
            font-size: 0.85rem;
            font-weight: 500;
            color: var(--muted);
            transition: background .15s, color .15s;
            user-select: none;
        }}
        .tab-label:hover {{ color: var(--text); }}

        #tab-overview:checked ~ .tab-labels label[for="tab-overview"],
        #tab-users:checked   ~ .tab-labels label[for="tab-users"],
        #tab-dl:checked      ~ .tab-labels label[for="tab-dl"],
        #tab-errors:checked  ~ .tab-labels label[for="tab-errors"],
        #tab-queue:checked   ~ .tab-labels label[for="tab-queue"],
        #tab-health:checked  ~ .tab-labels label[for="tab-health"],
        #tab-feedback:checked ~ .tab-labels label[for="tab-feedback"],
        #tab-alerts:checked  ~ .tab-labels label[for="tab-alerts"],
        #tab-revenue:checked ~ .tab-labels label[for="tab-revenue"] {{
            background: var(--card);
            color: var(--text);
            border: 1px solid var(--border2);
        }}

        .tab-content {{ display: none; }}
        #tab-overview:checked ~ .tab-contents #pane-overview,
        #tab-users:checked   ~ .tab-contents #pane-users,
        #tab-dl:checked      ~ .tab-contents #pane-dl,
        #tab-errors:checked  ~ .tab-contents #pane-errors,
        #tab-queue:checked   ~ .tab-contents #pane-queue,
        #tab-health:checked  ~ .tab-contents #pane-health,
        #tab-feedback:checked ~ .tab-contents #pane-feedback,
        #tab-alerts:checked  ~ .tab-contents #pane-alerts,
        #tab-revenue:checked ~ .tab-contents #pane-revenue {{
            display: block;
        }}

        /* ── Stat Cards ── */
        .stats-grid {{
            display: grid;
            grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
            gap: 16px;
            margin-bottom: 32px;
        }}
        .stat-card {{
            background: var(--card);
            border: 1px solid var(--border);
            border-radius: 14px;
            padding: 18px 20px;
            display: flex; align-items: flex-start; gap: 14px;
            transition: border-color .15s;
        }}
        .stat-card:hover {{ border-color: var(--border2); }}
        .stat-icon {{ font-size: 1.4rem; opacity: 0.7; flex-shrink: 0; }}
        .stat-value {{
            font-size: 1.7rem; font-weight: 700;
            line-height: 1.1; margin-bottom: 4px;
            font-variant-numeric: tabular-nums;
        }}
        .stat-label {{ color: var(--muted); font-size: 0.8rem; }}
        .active-val {{ color: var(--green); }}
        .err-val    {{ color: var(--red); }}

        /* ── Section headers ── */
        .section-title {{
            font-size: 0.78rem; font-weight: 600;
            text-transform: uppercase; letter-spacing: 0.08em;
            color: var(--muted); margin-bottom: 14px;
        }}

        /* ── Cards / panels ── */
        .panel {{
            background: var(--card);
            border: 1px solid var(--border);
            border-radius: 14px;
            overflow: hidden;
            margin-bottom: 28px;
        }}
        .panel-head {{
            padding: 14px 20px;
            border-bottom: 1px solid var(--border);
            font-size: 0.85rem; font-weight: 600; color: var(--muted);
            text-transform: uppercase; letter-spacing: 0.06em;
        }}

        /* ── Tables ── */
        .tbl-wrap {{ overflow-x: auto; }}
        table {{ width: 100%; border-collapse: collapse; }}
        th, td {{ padding: 11px 18px; text-align: left; border-bottom: 1px solid var(--border); }}
        th {{ background: var(--surface); color: var(--muted); font-weight: 500; font-size: 0.78rem; text-transform: uppercase; letter-spacing: 0.06em; }}
        tr:last-child td {{ border-bottom: none; }}
        tr:hover td {{ background: rgba(255,255,255,0.02); }}

        .mono  {{ font-family: 'SF Mono', 'Fira Code', ui-monospace, monospace; font-size: 0.82rem; }}
        .small {{ font-size: 0.78rem; }}
        .dim   {{ color: var(--muted); }}
        .title-cell {{ max-width: 300px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }}
        .msg-cell   {{ max-width: 320px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }}
        .empty-state {{ padding: 32px; text-align: center; color: var(--muted); font-size: 0.9rem; }}

        /* ── Pills / badges ── */
        .pill {{
            display: inline-block; padding: 2px 9px; border-radius: 20px;
            font-size: 0.75rem; font-weight: 600;
        }}
        .plan-free    {{ background: #232323; color: #888; }}
        .plan-premium {{ background: rgba(124,106,255,0.18); color: #a799ff; }}
        .plan-vip     {{ background: rgba(245,158,11,0.15); color: #f59e0b; }}

        .fmt-badge {{
            display: inline-block; padding: 1px 8px; border-radius: 6px;
            font-size: 0.75rem; font-weight: 600;
            background: rgba(255,255,255,0.07); color: #ccc;
            font-family: 'SF Mono', ui-monospace, monospace;
        }}

        .err-badge {{
            display: inline-block; padding: 2px 9px; border-radius: 6px;
            font-size: 0.75rem; font-weight: 600;
            font-family: 'SF Mono', ui-monospace, monospace;
        }}
        .err-network  {{ background: rgba(59,130,246,0.15); color: #60a5fa; }}
        .err-auth     {{ background: rgba(245,158,11,0.15); color: #fbbf24; }}
        .err-download {{ background: rgba(239,68,68,0.15);  color: #f87171; }}
        .err-db       {{ background: rgba(168,85,247,0.15); color: #c084fc; }}
        .err-other    {{ background: rgba(255,255,255,0.07); color: #999; }}

        /* ── Rank ── */
        .rank {{ display: inline-block; font-weight: 700; font-size: 0.82rem; width: 28px; text-align: center; }}
        .rank-gold   {{ color: #f59e0b; }}
        .rank-silver {{ color: #94a3b8; }}
        .rank-bronze {{ color: #a16207; }}

        /* ── Bar chart ── */
        .chart-wrap {{
            padding: 20px 16px 0;
            height: 180px;
            display: flex; align-items: flex-end; gap: 3px;
            overflow-x: auto;
        }}
        .bar-col {{
            display: flex; flex-direction: column; align-items: center;
            flex: 1 1 0; min-width: 18px; max-width: 44px;
            height: 100%;
            position: relative;
            justify-content: flex-end;
        }}
        .bar-tip {{
            font-size: 0.6rem; color: var(--muted);
            margin-bottom: 2px;
            white-space: nowrap;
        }}
        .bar {{
            width: 100%; background: var(--accent);
            border-radius: 4px 4px 0 0;
            min-height: 2px;
            opacity: 0.75;
            transition: opacity .15s;
        }}
        .bar:hover {{ opacity: 1; }}
        .bar-label {{
            font-size: 0.58rem; color: var(--muted);
            margin-top: 4px; writing-mode: vertical-rl;
            transform: rotate(180deg);
            max-height: 40px; overflow: hidden;
        }}
        .chart-footer {{
            padding: 8px 16px 16px;
            font-size: 0.75rem; color: var(--muted);
            text-align: right;
        }}

        /* ── Format bars ── */
        .fmt-rows {{ padding: 16px 20px; display: flex; flex-direction: column; gap: 12px; }}
        .fmt-row {{ display: flex; align-items: center; gap: 12px; }}
        .fmt-name {{ width: 56px; font-family: 'SF Mono', ui-monospace, monospace; font-size: 0.8rem; color: var(--muted); flex-shrink: 0; }}
        .fmt-bar-track {{
            flex: 1; height: 8px; background: var(--border); border-radius: 99px; overflow: hidden;
        }}
        .fmt-bar {{ height: 100%; border-radius: 99px; transition: width .4s; }}
        .fmt-mp3      {{ background: var(--accent); }}
        .fmt-video    {{ background: var(--blue); }}
        .fmt-aac      {{ background: var(--green); }}
        .fmt-lossless {{ background: var(--yellow); }}
        .fmt-other    {{ background: #555; }}
        .fmt-count {{ font-size: 0.78rem; color: var(--muted); white-space: nowrap; width: 110px; text-align: right; }}

        /* ── Toolbar / search / filters ── */
        .toolbar {{
            display: flex; align-items: center; gap: 12px;
            margin-bottom: 16px; flex-wrap: wrap;
        }}
        .filter-group {{ display: flex; gap: 4px; }}
        .filter-btn {{
            padding: 6px 14px; border-radius: 8px; border: 1px solid var(--border2);
            background: transparent; color: var(--muted); font-size: 0.8rem; font-weight: 500;
            cursor: pointer; transition: all .15s;
        }}
        .filter-btn:hover {{ color: var(--text); border-color: #555; }}
        .filter-btn.active {{ background: var(--card); color: var(--text); border-color: var(--accent); }}
        .search-input {{
            padding: 7px 14px; border-radius: 8px; border: 1px solid var(--border2);
            background: var(--surface); color: var(--text); font-size: 0.85rem;
            outline: none; min-width: 220px; transition: border-color .15s;
        }}
        .search-input:focus {{ border-color: var(--accent); }}
        .search-input::placeholder {{ color: #555; }}

        /* ── Pagination ── */
        .pagination {{
            display: flex; align-items: center; justify-content: center;
            gap: 8px; margin-top: 16px;
        }}
        .page-btn {{
            padding: 6px 12px; border-radius: 6px; border: 1px solid var(--border2);
            background: transparent; color: var(--muted); font-size: 0.8rem;
            cursor: pointer; transition: all .15s;
        }}
        .page-btn:hover {{ color: var(--text); border-color: #555; }}
        .page-btn.active {{ background: var(--accent); color: #fff; border-color: var(--accent); }}
        .page-btn:disabled {{ opacity: 0.3; cursor: default; }}
        .page-info {{ color: var(--muted); font-size: 0.8rem; }}

        /* ── Action buttons ── */
        .action-group {{ display: flex; gap: 4px; }}
        .act-btn {{
            padding: 3px 8px; border-radius: 5px; border: 1px solid var(--border2);
            background: transparent; color: var(--muted); font-size: 0.72rem; font-weight: 500;
            cursor: pointer; transition: all .15s; white-space: nowrap;
        }}
        .act-btn:hover {{ color: var(--text); border-color: #555; }}
        .act-btn.danger {{ border-color: rgba(239,68,68,0.3); color: #f87171; }}
        .act-btn.danger:hover {{ background: rgba(239,68,68,0.1); }}
        .act-btn.success {{ border-color: rgba(34,197,94,0.3); color: #22c55e; }}
        .act-btn.success:hover {{ background: rgba(34,197,94,0.1); }}

        /* ── Plan select dropdown ── */
        .plan-select {{
            padding: 3px 6px; border-radius: 5px; border: 1px solid var(--border2);
            background: var(--surface); color: var(--text); font-size: 0.75rem;
            cursor: pointer; outline: none;
        }}

        /* ── Modal ── */
        .modal-overlay {{
            display: none; position: fixed; inset: 0; background: rgba(0,0,0,0.6);
            z-index: 200; justify-content: center; align-items: center;
        }}
        .modal-overlay.open {{ display: flex; }}
        .modal {{
            background: var(--card); border: 1px solid var(--border2);
            border-radius: 16px; padding: 28px; min-width: 320px; max-width: 480px;
        }}
        .modal h3 {{ margin-bottom: 16px; font-size: 1rem; }}
        .modal-actions {{ display: flex; gap: 8px; justify-content: flex-end; margin-top: 20px; }}
        .modal-btn {{
            padding: 8px 18px; border-radius: 8px; border: none;
            font-size: 0.85rem; font-weight: 500; cursor: pointer;
        }}
        .modal-btn.cancel {{ background: var(--surface); color: var(--muted); }}
        .modal-btn.confirm {{ background: var(--accent); color: #fff; }}
        .modal-btn.confirm.danger {{ background: var(--red); }}

        /* ── Severity / status badges ── */
        .sev-critical {{ background: rgba(239,68,68,0.15); color: #f87171; }}
        .sev-warning  {{ background: rgba(245,158,11,0.15); color: #fbbf24; }}
        .sev-info     {{ background: rgba(59,130,246,0.15); color: #60a5fa; }}
        .status-new      {{ background: rgba(59,130,246,0.15); color: #60a5fa; }}
        .status-reviewed {{ background: rgba(245,158,11,0.15); color: #fbbf24; }}
        .status-replied  {{ background: rgba(34,197,94,0.12); color: #22c55e; }}
        .status-pending    {{ background: rgba(245,158,11,0.15); color: #fbbf24; }}
        .status-processing {{ background: rgba(59,130,246,0.15); color: #60a5fa; }}
        .status-uploading  {{ background: rgba(124,106,255,0.18); color: #a799ff; }}
        .status-completed  {{ background: rgba(34,197,94,0.12); color: #22c55e; }}
        .status-dead_letter {{ background: rgba(239,68,68,0.15); color: #f87171; }}
        .status-leased     {{ background: rgba(168,85,247,0.15); color: #c084fc; }}
        .resolved-yes {{ opacity: 0.5; }}

        /* ── Health grid ── */
        .health-grid {{
            display: grid;
            grid-template-columns: repeat(auto-fill, minmax(220px, 1fr));
            gap: 16px; margin-bottom: 28px;
        }}
        .health-card {{
            background: var(--card); border: 1px solid var(--border); border-radius: 14px;
            padding: 20px; display: flex; flex-direction: column; gap: 8px;
        }}
        .health-card .hc-label {{ color: var(--muted); font-size: 0.78rem; text-transform: uppercase; letter-spacing: 0.06em; }}
        .health-card .hc-value {{ font-size: 1.3rem; font-weight: 700; font-variant-numeric: tabular-nums; }}

        /* ── Detail drawer (right slide-in) ── */
        .detail-overlay {{
            display: none; position: fixed; inset: 0; background: rgba(0,0,0,0.5);
            z-index: 250; justify-content: flex-end;
        }}
        .detail-overlay.open {{ display: flex; }}
        .detail-panel {{
            background: var(--bg); width: min(560px, 90vw); height: 100vh;
            overflow-y: auto; border-left: 1px solid var(--border2); padding: 28px;
        }}
        .detail-panel h2 {{ margin-bottom: 20px; font-size: 1.1rem; display: flex; justify-content: space-between; align-items: center; }}
        .detail-panel .close-btn {{
            background: none; border: 1px solid var(--border2); border-radius: 8px;
            color: var(--muted); padding: 4px 12px; cursor: pointer; font-size: 0.82rem;
        }}
        .detail-section {{ margin-bottom: 24px; }}
        .detail-section h3 {{ font-size: 0.82rem; color: var(--muted); text-transform: uppercase; letter-spacing: 0.06em; margin-bottom: 10px; }}
        .detail-row {{ display: flex; justify-content: space-between; padding: 6px 0; border-bottom: 1px solid var(--border); font-size: 0.88rem; }}
        .detail-row span:first-child {{ color: var(--muted); }}

        /* ── Broadcast form ── */
        .broadcast-area {{
            display: flex; flex-direction: column; gap: 12px; padding: 20px;
        }}
        .broadcast-area textarea {{
            background: var(--surface); color: var(--text); border: 1px solid var(--border2);
            border-radius: 8px; padding: 12px; font-size: 0.88rem; min-height: 100px;
            resize: vertical; outline: none; font-family: inherit;
        }}
        .broadcast-area textarea:focus {{ border-color: var(--accent); }}
        .broadcast-area .send-btn {{
            align-self: flex-end; padding: 8px 20px; border-radius: 8px;
            border: none; background: var(--accent); color: #fff;
            font-size: 0.85rem; font-weight: 500; cursor: pointer;
        }}
        .broadcast-area .send-btn:disabled {{ opacity: 0.4; cursor: default; }}
        .tab-labels {{ flex-wrap: wrap; }}

        /* ── Two-column layout ── */
        .two-col {{ display: grid; grid-template-columns: 1fr 1fr; gap: 20px; }}
        @media (max-width: 768px) {{
            .two-col {{ grid-template-columns: 1fr; }}
            .stats-grid {{ grid-template-columns: repeat(2, 1fr); }}
            .tab-labels {{ overflow-x: auto; flex-wrap: nowrap; -webkit-overflow-scrolling: touch; scrollbar-width: none; }}
            .tab-labels::-webkit-scrollbar {{ display: none; }}
            .tab-label {{ flex-shrink: 0; }}
            .detail-panel {{ width: 100vw !important; }}
            .topbar {{ padding: 0 12px; }}
            .page {{ padding: 16px 12px 40px; }}
            .toolbar {{ flex-direction: column; align-items: stretch; }}
            .search-input {{ min-width: 0; width: 100%; }}
        }}
        @media (max-width: 480px) {{
            .stats-grid {{ grid-template-columns: 1fr; }}
            .tab-label {{ padding: 7px 12px; font-size: 0.78rem; }}
            .health-grid {{ grid-template-columns: repeat(2, 1fr); }}
        }}
    </style>
</head>
<body>

<div class="topbar">
    <div class="topbar-brand">dora<span>dura</span></div>
    <div class="topbar-right">
        <span style="color:var(--muted);font-size:0.8rem;">Admin Dashboard</span>
        <button class="logout" style="cursor:pointer;background:none;" onclick="openBroadcastFor('')">Broadcast</button>
        <button id="auto-refresh-btn" class="logout" style="cursor:pointer;background:none;" onclick="toggleAutoRefresh()">▶ Auto</button>
        <a href="/admin/logout" class="logout">Logout</a>
    </div>
</div>

<div class="page">

    <!-- CSS-only tab switcher -->
    <input type="radio" name="tab" id="tab-overview" class="tab-radio" checked>
    <input type="radio" name="tab" id="tab-users"    class="tab-radio">
    <input type="radio" name="tab" id="tab-dl"       class="tab-radio">
    <input type="radio" name="tab" id="tab-queue"    class="tab-radio">
    <input type="radio" name="tab" id="tab-errors"   class="tab-radio">
    <input type="radio" name="tab" id="tab-health"   class="tab-radio">
    <input type="radio" name="tab" id="tab-feedback" class="tab-radio">
    <input type="radio" name="tab" id="tab-alerts"   class="tab-radio">
    <input type="radio" name="tab" id="tab-revenue"  class="tab-radio">

    <div class="tabs-wrap">
        <div class="tab-labels">
            <label for="tab-overview" class="tab-label">Overview</label>
            <label for="tab-users"    class="tab-label">Users</label>
            <label for="tab-dl"       class="tab-label">Downloads</label>
            <label for="tab-queue"    class="tab-label">Queue</label>
            <label for="tab-errors"   class="tab-label">Errors</label>
            <label for="tab-health"   class="tab-label">Health</label>
            <label for="tab-feedback" class="tab-label">Feedback</label>
            <label for="tab-alerts"   class="tab-label">Alerts</label>
            <label for="tab-revenue"  class="tab-label">Revenue</label>
        </div>
    </div>

    <div class="tab-contents">

        <!-- ══════════════════════════════════════════
             Pane: Overview
        ══════════════════════════════════════════ -->
        <div class="tab-content" id="pane-overview">

            <div class="stats-grid">
                {cards}
            </div>

            <!-- Downloads chart -->
            <div class="panel">
                <div class="panel-head">Downloads — last 30 days</div>
                <div class="chart-wrap">
                    {chart}
                </div>
                <div class="chart-footer">Each bar = one calendar day</div>
            </div>

            <!-- Two-col: format dist + system -->
            <div class="two-col">
                <div class="panel">
                    <div class="panel-head">Format Distribution</div>
                    <div class="fmt-rows">{fmt}</div>
                </div>
                <div class="panel">
                    <div class="panel-head">System</div>
                    <div style="padding:20px; display:flex; flex-direction:column; gap:14px;">
                        <div style="display:flex;justify-content:space-between;">
                            <span style="color:var(--muted);">Queue size</span>
                            <strong>{active_tasks}</strong>
                        </div>
                        <div style="display:flex;justify-content:space-between;">
                            <span style="color:var(--muted);">Total users</span>
                            <strong>{total_users}</strong>
                        </div>
                        <div style="display:flex;justify-content:space-between;">
                            <span style="color:var(--muted);">Total downloads</span>
                            <strong>{total_dl}</strong>
                        </div>
                        <div style="display:flex;justify-content:space-between;">
                            <span style="color:var(--muted);">Downloads today</span>
                            <strong>{dl_today}</strong>
                        </div>
                        <div style="display:flex;justify-content:space-between;">
                            <span style="color:var(--muted);">Errors today</span>
                            <strong style="color:{err_color};">{errors_today}</strong>
                        </div>
                    </div>
                </div>
            </div>

        </div><!-- /pane-overview -->

        <!-- ══════════════════════════════════════════
             Pane: Users (dynamic via JS)
        ══════════════════════════════════════════ -->
        <div class="tab-content" id="pane-users">
            <div class="toolbar">
                <div class="filter-group">
                    <button class="filter-btn active" data-filter="all">All</button>
                    <button class="filter-btn" data-filter="free">Free</button>
                    <button class="filter-btn" data-filter="premium">Premium</button>
                    <button class="filter-btn" data-filter="vip">VIP</button>
                    <button class="filter-btn" data-filter="blocked">Blocked</button>
                </div>
                <input type="text" id="user-search" class="search-input" placeholder="Search by username or ID...">
            </div>
            <div class="panel">
                <div class="tbl-wrap">
                    <table>
                        <thead>
                            <tr>
                                <th>ID</th>
                                <th>Username</th>
                                <th>Plan</th>
                                <th>Downloads</th>
                                <th>Lang</th>
                                <th>Status</th>
                                <th>Actions</th>
                            </tr>
                        </thead>
                        <tbody id="users-tbody"><tr><td colspan="7" class="empty-state">Loading...</td></tr></tbody>
                    </table>
                </div>
            </div>
            <div id="users-pagination" class="pagination"></div>
        </div><!-- /pane-users -->

        <!-- ══════════════════════════════════════════
             Pane: Downloads (dynamic via JS)
        ══════════════════════════════════════════ -->
        <div class="tab-content" id="pane-dl">
            <div class="toolbar">
                <input type="text" id="dl-search" class="search-input" placeholder="Search by title, author, or user...">
            </div>
            <div class="panel">
                <div class="tbl-wrap">
                    <table>
                        <thead>
                            <tr>
                                <th>Title / Author</th>
                                <th>User</th>
                                <th>Format</th>
                                <th>Quality</th>
                                <th>Size</th>
                                <th>Duration</th>
                                <th>Time</th>
                            </tr>
                        </thead>
                        <tbody id="dl-tbody"><tr><td colspan="7" class="empty-state">Loading...</td></tr></tbody>
                    </table>
                </div>
            </div>
            <div id="dl-pagination" class="pagination"></div>
        </div><!-- /pane-dl -->

        <!-- ══════════════════════════════════════════
             Pane: Queue (dynamic via JS)
        ══════════════════════════════════════════ -->
        <div class="tab-content" id="pane-queue">
            <div class="toolbar">
                <div class="filter-group">
                    <button class="filter-btn active" data-qfilter="all">All</button>
                    <button class="filter-btn" data-qfilter="active">Active</button>
                    <button class="filter-btn" data-qfilter="pending">Pending</button>
                    <button class="filter-btn" data-qfilter="processing">Processing</button>
                    <button class="filter-btn" data-qfilter="completed">Completed</button>
                    <button class="filter-btn" data-qfilter="dead_letter">Dead</button>
                </div>
                <input type="text" id="queue-search" class="search-input" placeholder="Search by URL, user, task ID...">
            </div>
            <div class="panel">
                <div class="tbl-wrap">
                    <table>
                        <thead>
                            <tr>
                                <th>ID</th><th>User</th><th>URL</th><th>Format</th>
                                <th>Status</th><th>Retries</th><th>Worker</th>
                                <th>Created</th><th>Actions</th>
                            </tr>
                        </thead>
                        <tbody id="queue-tbody"><tr><td colspan="9" class="empty-state">Loading...</td></tr></tbody>
                    </table>
                </div>
            </div>
            <div id="queue-pagination" class="pagination"></div>
        </div><!-- /pane-queue -->

        <!-- ══════════════════════════════════════════
             Pane: Errors (dynamic via JS)
        ══════════════════════════════════════════ -->
        <div class="tab-content" id="pane-errors">
            <div class="toolbar">
                <div class="filter-group">
                    <button class="filter-btn active" data-efilter="all">All</button>
                    <button class="filter-btn" data-efilter="no">Unresolved</button>
                    <button class="filter-btn" data-efilter="yes">Resolved</button>
                </div>
                <input type="text" id="errors-search" class="search-input" placeholder="Search errors...">
            </div>
            <div class="panel">
                <div class="tbl-wrap">
                    <table>
                        <thead>
                            <tr>
                                <th>Time</th><th>Type</th><th>Message</th>
                                <th>URL</th><th>User</th><th>Actions</th>
                            </tr>
                        </thead>
                        <tbody id="errors-tbody"><tr><td colspan="6" class="empty-state">Loading...</td></tr></tbody>
                    </table>
                </div>
            </div>
            <div id="errors-pagination" class="pagination"></div>
        </div><!-- /pane-errors -->

        <!-- ══════════════════════════════════════════
             Pane: Health (dynamic via JS)
        ══════════════════════════════════════════ -->
        <div class="tab-content" id="pane-health">
            <div id="health-content"><div class="empty-state">Loading...</div></div>
        </div><!-- /pane-health -->

        <!-- ══════════════════════════════════════════
             Pane: Feedback (dynamic via JS)
        ══════════════════════════════════════════ -->
        <div class="tab-content" id="pane-feedback">
            <div class="toolbar">
                <div class="filter-group">
                    <button class="filter-btn active" data-ffilter="all">All</button>
                    <button class="filter-btn" data-ffilter="new">New</button>
                    <button class="filter-btn" data-ffilter="reviewed">Reviewed</button>
                    <button class="filter-btn" data-ffilter="replied">Replied</button>
                </div>
                <input type="text" id="feedback-search" class="search-input" placeholder="Search feedback...">
            </div>
            <div class="panel">
                <div class="tbl-wrap">
                    <table>
                        <thead>
                            <tr>
                                <th>User</th><th>Name</th><th>Message</th>
                                <th>Status</th><th>Time</th><th>Actions</th>
                            </tr>
                        </thead>
                        <tbody id="feedback-tbody"><tr><td colspan="6" class="empty-state">Loading...</td></tr></tbody>
                    </table>
                </div>
            </div>
            <div id="feedback-pagination" class="pagination"></div>
        </div><!-- /pane-feedback -->

        <!-- ══════════════════════════════════════════
             Pane: Alerts (dynamic via JS)
        ══════════════════════════════════════════ -->
        <div class="tab-content" id="pane-alerts">
            <div class="toolbar">
                <div class="filter-group">
                    <button class="filter-btn active" data-afilter="all">All</button>
                    <button class="filter-btn" data-afilter="critical">Critical</button>
                    <button class="filter-btn" data-afilter="warning">Warning</button>
                    <button class="filter-btn" data-afilter="unacked">Unacked</button>
                </div>
                <input type="text" id="alerts-search" class="search-input" placeholder="Search alerts...">
            </div>
            <div class="panel">
                <div class="tbl-wrap">
                    <table>
                        <thead>
                            <tr>
                                <th>Time</th><th>Type</th><th>Severity</th>
                                <th>Message</th><th>Status</th><th>Actions</th>
                            </tr>
                        </thead>
                        <tbody id="alerts-tbody"><tr><td colspan="6" class="empty-state">Loading...</td></tr></tbody>
                    </table>
                </div>
            </div>
            <div id="alerts-pagination" class="pagination"></div>
        </div><!-- /pane-alerts -->

        <!-- ══════════════════════════════════════════
             Pane: Revenue (dynamic via JS)
        ══════════════════════════════════════════ -->
        <div class="tab-content" id="pane-revenue">
            <div id="revenue-content"><div class="empty-state">Loading...</div></div>
            <div class="toolbar" style="margin-top:16px;">
                <div class="filter-group">
                    <button class="filter-btn active" data-rfilter="all">All</button>
                    <button class="filter-btn" data-rfilter="premium">Premium</button>
                    <button class="filter-btn" data-rfilter="vip">VIP</button>
                    <button class="filter-btn" data-rfilter="recurring">Recurring</button>
                </div>
            </div>
            <div class="panel">
                <div class="tbl-wrap">
                    <table>
                        <thead>
                            <tr><th>User</th><th>Plan</th><th>Amount</th><th>Currency</th><th>Recurring</th><th>Date</th></tr>
                        </thead>
                        <tbody id="revenue-tbody"><tr><td colspan="6" class="empty-state">Loading...</td></tr></tbody>
                    </table>
                </div>
            </div>
            <div id="revenue-pagination" class="pagination"></div>
        </div><!-- /pane-revenue -->

    </div><!-- /tab-contents -->
</div><!-- /page -->

<!-- User detail drawer -->
<div class="detail-overlay" id="detail-drawer">
    <div class="detail-panel" id="detail-content">
        <h2><span id="detail-title">User Details</span> <button class="close-btn" onclick="closeDetail()">Close</button></h2>
        <div id="detail-body"><div class="empty-state">Loading...</div></div>
    </div>
</div>

<!-- Broadcast modal -->
<div class="modal-overlay" id="broadcast-modal">
    <div class="modal" style="min-width:400px;">
        <h3 id="bc-title">Send Message</h3>
        <div class="broadcast-area">
            <input type="text" id="bc-target" class="search-input" placeholder="User ID or 'all' for broadcast" style="min-width:100%;">
            <textarea id="bc-message" placeholder="Message text..."></textarea>
            <div style="display:flex;gap:8px;justify-content:flex-end;">
                <button class="modal-btn cancel" onclick="closeBroadcast()">Cancel</button>
                <button class="send-btn" id="bc-send" onclick="sendBroadcast()">Send</button>
            </div>
        </div>
    </div>
</div>

<!-- User action modal -->
<div class="modal-overlay" id="modal">
    <div class="modal">
        <h3 id="modal-title">Confirm</h3>
        <p id="modal-body" style="color:var(--muted);font-size:0.9rem;"></p>
        <div class="modal-actions">
            <button class="modal-btn cancel" onclick="closeModal()">Cancel</button>
            <button class="modal-btn confirm" id="modal-confirm">Confirm</button>
        </div>
    </div>
</div>

<script>
(function() {{
    // --- State ---
    let usersPage=1, usersFilter='all', usersSearch='';
    let dlPage=1, dlSearch='';
    let queuePage=1, queueFilter='all', queueSearch='';
    let errorsPage=1, errorsResolved='all', errorsSearch='';
    let feedbackPage=1, feedbackFilter='all', feedbackSearch='';
    let alertsPage=1, alertsFilter='all', alertsSearch='';
    let revPage=1, revFilter='all';
    const loaded = {{}};
    let debounces = {{}};

    // --- Helpers ---
    const esc = s => String(s||'').replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;');
    const fmtNum = n => Number(n||0).toLocaleString();
    const fmtSize = b => {{
        if (!b) return '';
        if (b < 1024) return b + ' B';
        if (b < 1048576) return (b/1024).toFixed(1) + ' KB';
        return (b/1048576).toFixed(1) + ' MB';
    }};
    const fmtDur = s => {{
        if (!s) return '';
        const h = Math.floor(s/3600), m = Math.floor((s%3600)/60), sec = s%60;
        return h > 0 ? h+':'+(m<10?'0':'')+m+':'+(sec<10?'0':'')+sec : m+':'+(sec<10?'0':'')+sec;
    }};
    const fmtTime = ts => ts ? ts.substring(0, 16).replace('T',' ') : '';
    const errClass = t => {{
        t = t.toLowerCase();
        if (t.includes('network')||t.includes('timeout')) return 'err-network';
        if (t.includes('auth')||t.includes('forbidden')) return 'err-auth';
        if (t.includes('download')||t.includes('yt')) return 'err-download';
        if (t.includes('db')||t.includes('sql')) return 'err-db';
        return 'err-other';
    }};

    // --- Auto-refresh ---
    let autoRefresh = localStorage.getItem('adminAutoRefresh')==='1';
    let autoTimer = null;
    let lastUpdated = {{}};
    function startAutoRefresh() {{
        stopAutoRefresh();
        if (!autoRefresh) return;
        autoTimer = setInterval(() => {{
            const active = document.querySelector('.tab-radio:checked');
            if (active) active.dispatchEvent(new Event('change'));
        }}, 30000);
    }}
    function stopAutoRefresh() {{ if (autoTimer) {{ clearInterval(autoTimer); autoTimer=null; }} }}
    function toggleAutoRefresh() {{
        autoRefresh = !autoRefresh;
        localStorage.setItem('adminAutoRefresh', autoRefresh?'1':'0');
        const btn = document.getElementById('auto-refresh-btn');
        if (btn) btn.textContent = autoRefresh ? '⏸ Auto' : '▶ Auto';
        autoRefresh ? startAutoRefresh() : stopAutoRefresh();
    }}
    window.toggleAutoRefresh = toggleAutoRefresh;
    if (autoRefresh) startAutoRefresh();

    async function api(url, opts) {{
        const resp = await fetch(url, opts);
        if (resp.status === 401) {{ window.location = '/admin/login'; return null; }}
        if (!resp.ok) {{ const t = await resp.text(); alert('Error: ' + t); return null; }}
        return resp.json();
    }}
    const postJson = (url, body) => api(url, {{ method:'POST', headers:{{'Content-Type':'application/json'}}, body:JSON.stringify(body) }});
    function debounce(key, fn, ms) {{ clearTimeout(debounces[key]); debounces[key]=setTimeout(fn, ms||300); }}

    // --- Pagination ---
    function renderPagination(elId, data, onPage) {{
        const el = document.getElementById(elId);
        if (data.total_pages <= 1) {{ el.innerHTML = ''; return; }}
        let html = `<span class="page-info">${{fmtNum(data.total)}} total — page ${{data.page}} of ${{data.total_pages}}</span>`;
        html += `<button class="page-btn" ${{data.page<=1?'disabled':''}} onclick="void(0)">‹ Prev</button>`;
        for (let p = Math.max(1,data.page-2); p <= Math.min(data.total_pages,data.page+2); p++)
            html += `<button class="page-btn ${{p===data.page?'active':''}}" onclick="void(0)">${{p}}</button>`;
        html += `<button class="page-btn" ${{data.page>=data.total_pages?'disabled':''}} onclick="void(0)">Next ›</button>`;
        el.innerHTML = html;
        el.querySelectorAll('.page-btn').forEach(btn => btn.addEventListener('click', () => {{
            const t = btn.textContent.trim();
            if (t === '‹ Prev' && data.page > 1) onPage(data.page - 1);
            else if (t === 'Next ›' && data.page < data.total_pages) onPage(data.page + 1);
            else {{ const n = parseInt(t); if (!isNaN(n)) onPage(n); }}
        }}));
    }}

    // ══════ Users ══════
    async function loadUsers() {{
        const p = new URLSearchParams({{page:usersPage, filter:usersFilter}});
        if (usersSearch) p.set('search', usersSearch);
        const data = await api('/admin/api/users?'+p);
        if (!data) return;
        const tb = document.getElementById('users-tbody');
        if (!data.items.length) {{ tb.innerHTML='<tr><td colspan="7" class="empty-state">No users found.</td></tr>'; document.getElementById('users-pagination').innerHTML=''; return; }}
        tb.innerHTML = data.items.map(u => {{
            const st = u.is_blocked ? '<span class="pill" style="background:rgba(239,68,68,0.15);color:#f87171;">Blocked</span>' : '<span class="pill" style="background:rgba(34,197,94,0.12);color:#22c55e;">Active</span>';
            return `<tr style="cursor:pointer" onclick="openUserDetail(${{u.telegram_id}})">
                <td class="mono">${{u.telegram_id}}</td>
                <td class="mono">${{u.username?'@'+esc(u.username):'<span class="dim">—</span>'}}</td>
                <td><select class="plan-select" onclick="event.stopPropagation()" onchange="changePlan(${{u.telegram_id}},this.value)">
                    <option value="free" ${{u.plan==='free'?'selected':''}}>free</option>
                    <option value="premium" ${{u.plan==='premium'?'selected':''}}>premium</option>
                    <option value="vip" ${{u.plan==='vip'?'selected':''}}>vip</option>
                </select></td>
                <td>${{fmtNum(u.download_count)}}</td>
                <td class="dim">${{esc(u.language)}}</td>
                <td>${{st}}</td>
                <td><div class="action-group" onclick="event.stopPropagation()">
                    ${{u.is_blocked
                        ? `<button class="act-btn success" onclick="toggleBlock(${{u.telegram_id}},false)">Unblock</button>`
                        : `<button class="act-btn danger" onclick="toggleBlock(${{u.telegram_id}},true)">Block</button>`}}
                    <button class="act-btn" onclick="openBroadcastFor(${{u.telegram_id}})">Msg</button>
                </div></td></tr>`;
        }}).join('');
        renderPagination('users-pagination', data, p => {{ usersPage=p; loadUsers(); }});
    }}

    // ══════ Downloads ══════
    async function loadDownloads() {{
        const p = new URLSearchParams({{page:dlPage}});
        if (dlSearch) p.set('search', dlSearch);
        const data = await api('/admin/api/downloads?'+p);
        if (!data) return;
        const tb = document.getElementById('dl-tbody');
        if (!data.items.length) {{ tb.innerHTML='<tr><td colspan="7" class="empty-state">No downloads.</td></tr>'; document.getElementById('dl-pagination').innerHTML=''; return; }}
        tb.innerHTML = data.items.map(d => {{
            const tl = esc(d.title.length>45?d.title.slice(0,45)+'…':d.title);
            const al = d.author?`<div class="dim small">${{esc(d.author)}}</div>`:'';
            const q = d.video_quality||d.audio_bitrate||'';
            return `<tr><td class="title-cell">${{tl}}${{al}}</td><td class="mono">@${{esc(d.user)}}</td>
                <td><span class="fmt-badge">${{esc(d.format)}}</span></td><td class="dim">${{esc(q)}}</td>
                <td class="dim mono small">${{fmtSize(d.file_size)}}</td><td class="dim mono small">${{fmtDur(d.duration)}}</td>
                <td class="dim small">${{fmtTime(d.downloaded_at)}}</td></tr>`;
        }}).join('');
        renderPagination('dl-pagination', data, p => {{ dlPage=p; loadDownloads(); }});
    }}

    // ══════ Queue ══════
    async function loadQueue() {{
        const p = new URLSearchParams({{page:queuePage}});
        if (queueFilter!=='all') p.set('status', queueFilter);
        if (queueSearch) p.set('search', queueSearch);
        const data = await api('/admin/api/queue?'+p);
        if (!data) return;
        const tb = document.getElementById('queue-tbody');
        if (!data.items.length) {{ tb.innerHTML='<tr><td colspan="9" class="empty-state">Queue empty.</td></tr>'; document.getElementById('queue-pagination').innerHTML=''; return; }}
        tb.innerHTML = data.items.map(t => {{
            const shortUrl = t.url.length>40 ? t.url.slice(0,40)+'…' : t.url;
            const shortId = t.id.length>8 ? t.id.slice(0,8)+'…' : t.id;
            const canRetry = t.status==='dead_letter';
            const canCancel = t.status==='pending'||t.status==='leased';
            return `<tr>
                <td class="mono small" title="${{esc(t.id)}}">${{esc(shortId)}}</td>
                <td class="mono">${{t.username?'@'+esc(t.username):t.user_id}}</td>
                <td class="small" title="${{esc(t.url)}}">${{esc(shortUrl)}}</td>
                <td><span class="fmt-badge">${{esc(t.format)}}</span></td>
                <td><span class="pill status-${{t.status}}">${{esc(t.status)}}</span></td>
                <td class="dim">${{t.retry_count}}</td>
                <td class="dim mono small">${{esc(t.worker_id||'—')}}</td>
                <td class="dim small">${{fmtTime(t.created_at)}}</td>
                <td><div class="action-group">
                    ${{canRetry?`<button class="act-btn success" onclick="retryTask('${{t.id}}')">Retry</button>`:''}}
                    ${{canCancel?`<button class="act-btn danger" onclick="cancelTask('${{t.id}}')">Cancel</button>`:''}}
                </div></td></tr>`;
        }}).join('');
        renderPagination('queue-pagination', data, p => {{ queuePage=p; loadQueue(); }});
    }}
    window.retryTask = async id => {{ if (await postJson(`/admin/api/queue/${{id}}/retry`, {{}})) loadQueue(); }};
    window.cancelTask = async id => {{ if (await postJson(`/admin/api/queue/${{id}}/cancel`, {{}})) loadQueue(); }};

    // ══════ Errors ══════
    async function loadErrors() {{
        const p = new URLSearchParams({{page:errorsPage}});
        if (errorsResolved!=='all') p.set('resolved', errorsResolved);
        if (errorsSearch) p.set('search', errorsSearch);
        const data = await api('/admin/api/errors?'+p);
        if (!data) return;
        const tb = document.getElementById('errors-tbody');
        if (!data.items.length) {{ tb.innerHTML='<tr><td colspan="6" class="empty-state">No errors. 🎉</td></tr>'; document.getElementById('errors-pagination').innerHTML=''; return; }}
        tb.innerHTML = data.items.map(e => {{
            const cls = e.resolved?'resolved-yes':'';
            const shortUrl = e.url.length>40?e.url.slice(0,40)+'…':e.url;
            const ctx = e.context ? `<tr class="err-ctx" style="display:none" data-eid="${{e.id}}"><td colspan="6" class="mono small dim" style="padding:8px 18px;white-space:pre-wrap;background:var(--surface);">${{esc(e.context)}}</td></tr>` : '';
            return `<tr class="${{cls}}" style="cursor:pointer" onclick="toggleErrCtx(${{e.id}})">
                <td class="dim mono small">${{fmtTime(e.timestamp)}}</td>
                <td><span class="err-badge ${{errClass(e.error_type)}}">${{esc(e.error_type)}}</span></td>
                <td class="msg-cell" title="${{esc(e.error_message)}}">${{esc(e.error_message.length>60?e.error_message.slice(0,60)+'…':e.error_message)}}</td>
                <td class="dim mono small" title="${{esc(e.url)}}">${{esc(shortUrl)}}</td>
                <td class="dim mono">${{e.user_id||''}}</td>
                <td onclick="event.stopPropagation()">${{e.resolved?'<span class="dim">Resolved</span>':`<button class="act-btn success" onclick="resolveError(${{e.id}})">Resolve</button>`}}</td>
            </tr>${{ctx}}`;
        }}).join('');
        renderPagination('errors-pagination', data, p => {{ errorsPage=p; loadErrors(); }});
    }}
    window.resolveError = async id => {{ if (await postJson(`/admin/api/errors/${{id}}/resolve`, {{}})) loadErrors(); }};

    // ══════ Health ══════
    async function loadHealth() {{
        const data = await api('/admin/api/health');
        if (!data) return;
        const el = document.getElementById('health-content');
        const q = data.queue || {{}};
        const qTotal = Object.values(q).reduce((a,b) => a+b, 0);
        const et = data.error_types || {{}};
        let queueHtml = Object.entries(q).map(([s,c]) => `<div class="detail-row"><span>${{esc(s)}}</span><strong>${{c}}</strong></div>`).join('');
        let errHtml = Object.entries(et).map(([t,c]) => `<div class="detail-row"><span>${{esc(t)}}</span><strong>${{c}}</strong></div>`).join('');
        el.innerHTML = `
            <div class="health-grid">
                <div class="health-card"><div class="hc-label">yt-dlp version</div><div class="hc-value">${{esc(data.ytdlp_version)}}</div></div>
                <div class="health-card"><div class="hc-label">Queue Total</div><div class="hc-value">${{qTotal}}</div></div>
                <div class="health-card"><div class="hc-label">Errors (24h)</div><div class="hc-value" style="color:${{data.errors_24h>0?'var(--red)':'inherit'}}">${{data.errors_24h}}</div></div>
                <div class="health-card"><div class="hc-label">Unacked Alerts</div><div class="hc-value" style="color:${{data.unacked_alerts>0?'var(--yellow)':'inherit'}}">${{data.unacked_alerts}}</div></div>
                <div class="health-card"><div class="hc-label">Unread Feedback</div><div class="hc-value">${{data.unread_feedback}}</div></div>
                <div class="health-card"><div class="hc-label">DB Size</div><div class="hc-value">${{fmtSize(data.db_size)}}</div></div>
            </div>
            <div class="two-col">
                <div class="panel"><div class="panel-head">Queue by Status</div><div style="padding:16px 20px;">${{queueHtml||'<div class="empty-state">Empty</div>'}}</div></div>
                <div class="panel"><div class="panel-head">Errors by Type (24h)</div><div style="padding:16px 20px;">${{errHtml||'<div class="empty-state">No errors</div>'}}</div></div>
            </div>`;
    }}

    // ══════ Feedback ══════
    async function loadFeedback() {{
        const p = new URLSearchParams({{page:feedbackPage}});
        if (feedbackFilter!=='all') p.set('status', feedbackFilter);
        if (feedbackSearch) p.set('search', feedbackSearch);
        const data = await api('/admin/api/feedback?'+p);
        if (!data) return;
        const tb = document.getElementById('feedback-tbody');
        if (!data.items.length) {{ tb.innerHTML='<tr><td colspan="6" class="empty-state">No feedback yet.</td></tr>'; document.getElementById('feedback-pagination').innerHTML=''; return; }}
        tb.innerHTML = data.items.map(f => `<tr>
            <td class="mono">${{f.username?'@'+esc(f.username):f.user_id}}</td>
            <td>${{esc(f.first_name)}}</td>
            <td class="msg-cell" title="${{esc(f.message)}}">${{esc(f.message.length>60?f.message.slice(0,60)+'…':f.message)}}</td>
            <td><span class="pill status-${{f.status}}">${{esc(f.status)}}</span></td>
            <td class="dim small">${{fmtTime(f.created_at)}}</td>
            <td><div class="action-group">
                ${{f.status==='new'?`<button class="act-btn" onclick="markFeedback(${{f.id}},'reviewed')">Mark Read</button>`:''}}
                <button class="act-btn" onclick="replyToFeedback(${{f.user_id}},${{f.id}},'')">Reply</button>
            </div></td></tr>`).join('');
        renderPagination('feedback-pagination', data, p => {{ feedbackPage=p; loadFeedback(); }});
    }}
    window.markFeedback = async (id,status) => {{ if (await postJson(`/admin/api/feedback/${{id}}/status`, {{status}})) loadFeedback(); }};

    // ══════ Alerts ══════
    async function loadAlerts() {{
        const p = new URLSearchParams({{page:alertsPage}});
        if (alertsFilter!=='all') p.set('severity', alertsFilter);
        if (alertsSearch) p.set('search', alertsSearch);
        const data = await api('/admin/api/alerts?'+p);
        if (!data) return;
        const tb = document.getElementById('alerts-tbody');
        if (!data.items.length) {{ tb.innerHTML='<tr><td colspan="6" class="empty-state">No alerts.</td></tr>'; document.getElementById('alerts-pagination').innerHTML=''; return; }}
        tb.innerHTML = data.items.map(a => {{
            const resolved = a.resolved_at ? `<span class="dim">Resolved ${{fmtTime(a.resolved_at)}}</span>` : '<span style="color:var(--yellow)">Active</span>';
            return `<tr>
                <td class="dim small">${{fmtTime(a.triggered_at)}}</td>
                <td class="mono small">${{esc(a.alert_type)}}</td>
                <td><span class="pill sev-${{a.severity}}">${{esc(a.severity)}}</span></td>
                <td class="msg-cell" title="${{esc(a.message)}}">${{esc(a.message.length>60?a.message.slice(0,60)+'…':a.message)}}</td>
                <td>${{resolved}}</td>
                <td>${{a.acknowledged?'<span class="dim">Acked</span>':`<button class="act-btn" onclick="ackAlert(${{a.id}})">Ack</button>`}}</td>
            </tr>`;
        }}).join('');
        renderPagination('alerts-pagination', data, p => {{ alertsPage=p; loadAlerts(); }});
    }}
    window.ackAlert = async id => {{ if (await postJson(`/admin/api/alerts/${{id}}/acknowledge`, {{}})) loadAlerts(); }};

    // ══════ Detail/Broadcast/Modal setup ══════
    window.closeDetail = () => document.getElementById('detail-drawer').classList.remove('open');
    document.getElementById('detail-drawer').addEventListener('click', e => {{ if (e.target.id==='detail-drawer') closeDetail(); }});
    window.openBroadcastFor = function(uid) {{
        document.getElementById('bc-target').value = uid || '';
        document.getElementById('bc-message').value = '';
        document.getElementById('bc-title').textContent = 'Send Message';
        document.getElementById('broadcast-modal').classList.add('open');
    }};
    window.closeBroadcast = () => document.getElementById('broadcast-modal').classList.remove('open');
    document.getElementById('broadcast-modal').addEventListener('click', e => {{ if (e.target.id==='broadcast-modal') closeBroadcast(); }});

    // ══════ Modal (block/unblock) ══════
    let modalCallback = null;
    function openModal(title, body, btnText, isDanger, cb) {{
        document.getElementById('modal-title').textContent = title;
        document.getElementById('modal-body').textContent = body;
        const btn = document.getElementById('modal-confirm');
        btn.textContent = btnText;
        btn.className = 'modal-btn confirm' + (isDanger ? ' danger' : '');
        modalCallback = cb;
        document.getElementById('modal').classList.add('open');
    }}
    window.closeModal = () => {{ document.getElementById('modal').classList.remove('open'); modalCallback=null; }};
    document.getElementById('modal-confirm').addEventListener('click', async () => {{ if (modalCallback) await modalCallback(); closeModal(); }});

    window.changePlan = async (uid,plan) => {{ if (await postJson(`/admin/api/users/${{uid}}/plan`, {{plan}})) loadUsers(); }};
    window.toggleBlock = (uid,block) => {{
        const a = block?'Block':'Unblock';
        openModal(a+' User', `Are you sure you want to ${{a.toLowerCase()}} user ${{uid}}?`, a, block,
            async () => {{ if (await postJson(`/admin/api/users/${{uid}}/block`, {{blocked:block}})) loadUsers(); }});
    }};

    // ══════ Error context toggle ══════
    window.toggleErrCtx = id => {{
        const row = document.querySelector(`.err-ctx[data-eid="${{id}}"]`);
        if (row) row.style.display = row.style.display==='none'?'table-row':'none';
    }};

    // ══════ Revenue ══════
    async function loadRevenue() {{
        const p = new URLSearchParams({{page:revPage}});
        if (revFilter!=='all') p.set('plan', revFilter);
        const data = await api('/admin/api/revenue?'+p);
        if (!data) return;
        const s = data.stats||{{}};
        const el = document.getElementById('revenue-content');
        const rpd = data.revenue_per_day||[];
        const maxR = Math.max(1,...rpd.map(d=>d[1]));
        let chartH = rpd.map(d => {{
            const pct = (d[1]/maxR*100)|0;
            const dt = d[0].substring(5);
            return `<div class="bar-col"><div class="bar-tip">${{d[1]}}</div><div class="bar" style="height:${{pct}}%;background:var(--green)"></div><div class="bar-label">${{dt}}</div></div>`;
        }}).join('');
        el.innerHTML = `
            <div class="health-grid">
                <div class="health-card"><div class="hc-label">Total Revenue</div><div class="hc-value">${{fmtNum(s.total_amount)}} ⭐</div></div>
                <div class="health-card"><div class="hc-label">Total Charges</div><div class="hc-value">${{fmtNum(s.total_charges)}}</div></div>
                <div class="health-card"><div class="hc-label">Premium</div><div class="hc-value" style="color:var(--accent)">${{s.premium_count}}</div></div>
                <div class="health-card"><div class="hc-label">VIP</div><div class="hc-value" style="color:var(--yellow)">${{s.vip_count}}</div></div>
                <div class="health-card"><div class="hc-label">Recurring</div><div class="hc-value">${{s.recurring_count}}</div></div>
                <div class="health-card"><div class="hc-label">Avg Check</div><div class="hc-value">${{s.total_charges?Math.round(s.total_amount/s.total_charges):0}} ⭐</div></div>
            </div>
            <div class="panel"><div class="panel-head">Revenue — last 30 days</div>
                <div class="chart-wrap">${{chartH||'<div class="empty-state">No data</div>'}}</div>
                <div class="chart-footer">Stars per day</div>
            </div>`;
        // Render charges table
        const ch = data.charges||{{}};
        const tb = document.getElementById('revenue-tbody');
        if (!ch.items||!ch.items.length) {{ tb.innerHTML='<tr><td colspan="6" class="empty-state">No charges.</td></tr>'; document.getElementById('revenue-pagination').innerHTML=''; return; }}
        tb.innerHTML = ch.items.map(c => `<tr>
            <td class="mono">${{c.username?'@'+esc(c.username):c.user_id}}</td>
            <td><span class="pill plan-${{c.plan}}">${{esc(c.plan)}}</span></td>
            <td><strong>${{c.amount}}</strong></td>
            <td class="dim">${{esc(c.currency)}}</td>
            <td>${{c.is_recurring?'🔄':'—'}}</td>
            <td class="dim small">${{fmtTime(c.payment_date)}}</td>
        </tr>`).join('');
        renderPagination('revenue-pagination', ch, p => {{ revPage=p; loadRevenue(); }});
    }}

    // ══════ Enhanced User Detail ══════
    // (extended openUserDetail to show preferences and editable fields)
    const _origOpenDetail = window.openUserDetail;
    window.openUserDetail = async function(uid) {{
        const drawer = document.getElementById('detail-drawer');
        const body = document.getElementById('detail-body');
        drawer.classList.add('open');
        body.innerHTML = '<div class="empty-state">Loading...</div>';
        const data = await api(`/admin/api/users/${{uid}}/details`);
        if (!data || data.error) {{ body.innerHTML = '<div class="empty-state">User not found.</div>'; return; }}
        const u = data.user, s = data.stats, sub = data.subscription;
        document.getElementById('detail-title').textContent = u.username ? '@'+u.username : 'User '+u.telegram_id;
        let html = `<div class="detail-section"><h3>Profile</h3>
            <div class="detail-row"><span>Telegram ID</span><span class="mono">${{u.telegram_id}}</span></div>
            <div class="detail-row"><span>Plan</span><span>
                <select class="plan-select" onchange="updateUserSetting(${{u.telegram_id}},'plan',this.value)">
                    <option value="free" ${{u.plan==='free'?'selected':''}}>free</option>
                    <option value="premium" ${{u.plan==='premium'?'selected':''}}>premium</option>
                    <option value="vip" ${{u.plan==='vip'?'selected':''}}>vip</option>
                </select></span></div>
            <div class="detail-row"><span>Language</span><span>
                <select class="plan-select" onchange="updateUserSetting(${{u.telegram_id}},'language',this.value)">
                    <option value="en" ${{u.language==='en'?'selected':''}}>en</option>
                    <option value="ru" ${{u.language==='ru'?'selected':''}}>ru</option>
                    <option value="fr" ${{u.language==='fr'?'selected':''}}>fr</option>
                    <option value="de" ${{u.language==='de'?'selected':''}}>de</option>
                </select></span></div>
            <div class="detail-row"><span>Status</span><span>${{u.is_blocked?'<span style="color:var(--red)">Blocked</span>':'<span style="color:var(--green)">Active</span>'}}</span></div>
            ${{sub?`<div class="detail-row"><span>Subscription</span><span>${{sub.plan}}${{sub.expires_at?' until '+fmtTime(sub.expires_at):''}}</span></div>
            <div class="detail-row"><span>Recurring</span><span>${{sub.is_recurring?'Yes':'No'}}</span></div>`:''}}
        </div>`;
        html += `<div class="detail-section"><h3>Preferences</h3>
            <div class="detail-row"><span>Format</span><span>${{esc(u.download_format||'mp3')}}</span></div>
            <div class="detail-row"><span>Video Quality</span><span>${{esc(u.video_quality||'best')}}</span></div>
            <div class="detail-row"><span>Audio Bitrate</span><span>${{esc(u.audio_bitrate||'320k')}}</span></div>
            <div class="detail-row"><span>Send as Doc</span><span>${{u.send_as_document?'Yes':'No'}}</span></div>
            <div class="detail-row"><span>Burn Subtitles</span><span>${{u.burn_subtitles?'Yes':'No'}}</span></div>
            <div class="detail-row"><span>Progress Style</span><span>${{esc(u.progress_bar_style||'classic')}}</span></div>
        </div>`;
        html += `<div class="detail-section"><h3>Stats</h3>
            <div class="detail-row"><span>Total Downloads</span><strong>${{fmtNum(s.total_downloads)}}</strong></div>
            <div class="detail-row"><span>Total Size</span><strong>${{fmtSize(s.total_size)}}</strong></div>
            <div class="detail-row"><span>Active Days</span><strong>${{s.active_days}}</strong></div>
        </div>`;
        if (s.top_artists && s.top_artists.length)
            html += `<div class="detail-section"><h3>Top Artists</h3>${{s.top_artists.map(a => `<div class="detail-row"><span>${{esc(a[0])}}</span><span>${{a[1]}}</span></div>`).join('')}}</div>`;
        if (data.charges && data.charges.length)
            html += `<div class="detail-section"><h3>Payments</h3>${{data.charges.map(c => `<div class="detail-row"><span>${{esc(c.plan)}} ${{c.is_recurring?'🔄':''}}</span><span>${{c.amount}} ${{esc(c.currency)}} — ${{fmtTime(c.payment_date)}}</span></div>`).join('')}}</div>`;
        if (data.recent_downloads && data.recent_downloads.length)
            html += `<div class="detail-section"><h3>Recent Downloads</h3>${{data.recent_downloads.slice(0,10).map(d => `<div class="detail-row"><span>${{esc(d.title.length>30?d.title.slice(0,30)+'…':d.title)}}</span><span class="dim small">${{esc(d.format)}} · ${{fmtTime(d.downloaded_at)}}</span></div>`).join('')}}</div>`;
        if (data.errors && data.errors.length)
            html += `<div class="detail-section"><h3>Recent Errors</h3>${{data.errors.map(e => `<div class="detail-row"><span class="err-badge ${{errClass(e.error_type)}}">${{esc(e.error_type)}}</span><span class="dim small">${{esc(e.error_message.length>40?e.error_message.slice(0,40)+'…':e.error_message)}}</span></div>`).join('')}}</div>`;
        html += `<div style="margin-top:20px;display:flex;gap:8px;">
            <button class="act-btn" onclick="openBroadcastFor(${{u.telegram_id}})">Send Message</button>
            ${{u.is_blocked
                ?`<button class="act-btn success" onclick="updateUserSetting(${{u.telegram_id}},'is_blocked',false);closeDetail()">Unblock</button>`
                :`<button class="act-btn danger" onclick="updateUserSetting(${{u.telegram_id}},'is_blocked',true);closeDetail()">Block</button>`}}
        </div>`;
        body.innerHTML = html;
    }};
    window.updateUserSetting = async function(uid, field, value) {{
        const body = {{}};
        if (field==='plan') body.plan = value;
        else if (field==='language') body.language = value;
        else if (field==='is_blocked') body.is_blocked = value;
        const data = await postJson(`/admin/api/users/${{uid}}/settings`, body);
        if (data && data.ok) {{ if (loaded.users) loadUsers(); }}
    }};

    // ══════ Broadcast confirmation ══════
    const _origSendBroadcast = window.sendBroadcast;
    window.sendBroadcast = async function() {{
        const target = document.getElementById('bc-target').value.trim();
        const message = document.getElementById('bc-message').value.trim();
        if (!target || !message) {{ alert('Fill in target and message'); return; }}
        if (target === 'all') {{
            if (!confirm('This will broadcast to ALL users. Are you sure?')) return;
        }}
        const btn = document.getElementById('bc-send');
        btn.disabled = true; btn.textContent = 'Sending...';
        const data = await postJson('/admin/api/broadcast', {{ target, message }});
        btn.disabled = false; btn.textContent = 'Send';
        if (data && data.ok) {{
            if (data.status === 'broadcasting') alert(`Broadcasting to ${{data.total}} users in background.`);
            else if (data.blocked > 0) alert('User has blocked the bot.');
            else alert('Message sent!');
            closeBroadcast();
        }}
    }};

    // ══════ Reply to feedback ══════
    window.replyToFeedback = function(userId, feedbackId, originalMsg) {{
        document.getElementById('bc-target').value = userId;
        document.getElementById('bc-message').value = '';
        document.getElementById('bc-title').textContent = 'Reply to Feedback';
        document.getElementById('broadcast-modal').classList.add('open');
        // After send, mark as replied
        const origSend = document.getElementById('bc-send').onclick;
        document.getElementById('bc-send').onclick = async function() {{
            const msg = document.getElementById('bc-message').value.trim();
            if (!msg) {{ alert('Enter a message'); return; }}
            const btn = document.getElementById('bc-send');
            btn.disabled = true; btn.textContent = 'Sending...';
            const data = await postJson('/admin/api/broadcast', {{ target: String(userId), message: msg }});
            btn.disabled = false; btn.textContent = 'Send';
            if (data && data.ok) {{
                await postJson(`/admin/api/feedback/${{feedbackId}}/status`, {{ status: 'replied' }});
                alert('Reply sent!');
                closeBroadcast();
                if (loaded.feedback) loadFeedback();
            }}
            document.getElementById('bc-send').onclick = origSend;
        }};
    }};

    // ══════ Tab switching ══════
    const tabLoaders = {{
        'tab-users': loadUsers, 'tab-dl': loadDownloads, 'tab-queue': loadQueue,
        'tab-errors': loadErrors, 'tab-health': loadHealth, 'tab-feedback': loadFeedback,
        'tab-alerts': loadAlerts, 'tab-revenue': loadRevenue,
    }};
    document.querySelectorAll('.tab-radio').forEach(r => r.addEventListener('change', () => {{
        const loader = tabLoaders[r.id];
        if (loader) {{
            if (!loaded[r.id]) {{ loaded[r.id]=1; loader(); }}
            else if (autoRefresh) loader(); // refresh on auto-refresh re-trigger
        }}
    }}));

    // ══════ Filter buttons ══════
    function bindFilters(attr, setter, loader) {{
        document.querySelectorAll(`[${{attr}}]`).forEach(btn => btn.addEventListener('click', () => {{
            btn.closest('.filter-group').querySelectorAll('.filter-btn').forEach(b => b.classList.remove('active'));
            btn.classList.add('active');
            setter(btn.getAttribute(attr));
            loader();
        }}));
    }}
    bindFilters('data-filter', v => {{ usersFilter=v; usersPage=1; }}, loadUsers);
    bindFilters('data-qfilter', v => {{ queueFilter=v; queuePage=1; }}, loadQueue);
    bindFilters('data-efilter', v => {{ errorsResolved=v; errorsPage=1; }}, loadErrors);
    bindFilters('data-ffilter', v => {{ feedbackFilter=v; feedbackPage=1; }}, loadFeedback);
    bindFilters('data-afilter', v => {{ alertsFilter=v; alertsPage=1; }}, loadAlerts);
    bindFilters('data-rfilter', v => {{ revFilter=v; revPage=1; }}, loadRevenue);

    // ══════ Search ══════
    const userSearchEl = document.getElementById('user-search');
    if (userSearchEl) userSearchEl.addEventListener('input', () => {{
        clearTimeout(usersDebounce);
        usersDebounce = setTimeout(() => {{ usersSearch=userSearchEl.value.trim(); usersPage=1; loadUsers(); }}, 300);
    }});
    const dlSearchEl = document.getElementById('dl-search');
    if (dlSearchEl) dlSearchEl.addEventListener('input', () => debounce('dl', () => {{ dlSearch=dlSearchEl.value.trim(); dlPage=1; loadDownloads(); }}));

    // Search on new tabs
    const qsEl = document.getElementById('queue-search');
    if (qsEl) qsEl.addEventListener('input', () => debounce('queue', () => {{ queueSearch=qsEl.value.trim(); queuePage=1; loadQueue(); }}));
    const esEl = document.getElementById('errors-search');
    if (esEl) esEl.addEventListener('input', () => debounce('errors', () => {{ errorsSearch=esEl.value.trim(); errorsPage=1; loadErrors(); }}));
    const fsEl = document.getElementById('feedback-search');
    if (fsEl) fsEl.addEventListener('input', () => debounce('feedback', () => {{ feedbackSearch=fsEl.value.trim(); feedbackPage=1; loadFeedback(); }}));
    const asEl = document.getElementById('alerts-search');
    if (asEl) asEl.addEventListener('input', () => debounce('alerts', () => {{ alertsSearch=asEl.value.trim(); alertsPage=1; loadAlerts(); }}));
}})();
</script>

</body>
</html>"#,
        cards = cards_html,
        chart = chart_html,
        fmt = fmt_html,
        active_tasks = fmt_num(stats.active_tasks),
        total_users = fmt_num(stats.total_users),
        total_dl = fmt_num(stats.total_downloads),
        dl_today = fmt_num(stats.downloads_today),
        errors_today = fmt_num(stats.errors_today),
        err_color = if stats.errors_today > 0 { "#ef4444" } else { "inherit" },
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
