//! Public-facing web handlers (no admin auth required).
//!
//! Includes: metrics, privacy policy, share pages, health check.

use std::collections::BTreeMap;

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Json, Response};
use serde_json::json;

use crate::i18n;
use crate::storage::SharePageRecord;

use super::auth::{check_rate_limit, extract_ip};
use super::helpers::{constant_time_eq, html_escape, is_safe_url};
use super::types::{WebState, SHARE_MAX_PER_MIN, SHARE_RATE_LIMIT, SHARE_WINDOW_SECS};

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
