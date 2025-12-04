use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, patch, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;

use crate::core::rate_limiter::RateLimiter;
use crate::download::{queue::DownloadTask, DownloadQueue};
use crate::storage::db::{self, DbPool};
use crate::telegram::preview;
use crate::telegram::webapp_auth;

// ============================================================================
// СТРУКТУРЫ ДАННЫХ ДЛЯ API
// ============================================================================

/// Данные, отправляемые из Mini App (legacy, для обратной совместимости)
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WebAppData {
    pub url: String,
    pub format: String,
    #[serde(rename = "videoQuality")]
    pub video_quality: Option<String>,
    #[serde(rename = "audioBitrate")]
    pub audio_bitrate: Option<String>,
    #[serde(rename = "userId")]
    pub user_id: Option<i64>,
    pub username: Option<String>,
}

/// Новая структура для обработки действий из Mini App
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WebAppAction {
    pub action: String,
    pub plan: Option<String>,
    // Для обратной совместимости с WebAppData
    pub url: Option<String>,
    pub format: Option<String>,
    #[serde(rename = "videoQuality")]
    pub video_quality: Option<String>,
    #[serde(rename = "audioBitrate")]
    pub audio_bitrate: Option<String>,
}

/// Запрос на получение preview (метаданные видео/аудио)
#[derive(Debug, Deserialize)]
pub struct PreviewRequest {
    pub url: String,
    pub format: Option<String>,        // "mp3", "mp4", "srt", "txt"
    pub video_quality: Option<String>, // "1080p", "720p", "480p", "360p", "best"
}

/// Информация о формате видео
#[derive(Debug, Serialize, Clone)]
pub struct VideoFormatInfo {
    pub quality: String,                // "1080p", "720p", etc
    pub size_bytes: Option<u64>,        // размер файла
    pub size_formatted: Option<String>, // "45.2 MB"
    pub resolution: Option<String>,     // "1920x1080"
}

/// Ответ с метаданными для preview
#[derive(Debug, Serialize)]
pub struct PreviewResponse {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub thumbnail_url: Option<String>,
    pub duration: Option<u32>,              // секунды
    pub duration_formatted: Option<String>, // "3:45"
    pub filesize: Option<u64>,              // байты
    pub filesize_formatted: Option<String>, // "45.2 MB"
    pub description: Option<String>,
    pub video_formats: Option<Vec<VideoFormatInfo>>,
    pub available_formats: Vec<String>, // ["mp3", "mp4", "srt"]
}

/// Запрос на начало загрузки
#[derive(Debug, Deserialize)]
pub struct DownloadRequest {
    pub url: String,
    pub format: String,                 // "mp3", "mp4", "mp4+mp3", "srt", "txt"
    pub video_quality: Option<String>,  // для видео
    pub audio_bitrate: Option<String>,  // для аудио
    pub send_as_document: Option<bool>, // true = document, false = media
}

/// Ответ на запрос загрузки
#[derive(Debug, Serialize)]
pub struct DownloadResponse {
    pub task_id: String,
    pub queue_position: usize,
    pub estimated_time: Option<u64>, // секунды (опционально)
}

/// Статус задачи загрузки
#[derive(Debug, Serialize)]
pub struct TaskStatusResponse {
    pub status: String,       // "pending", "processing", "completed", "failed"
    pub progress: Option<u8>, // 0-100
    pub error: Option<String>,
    pub created_at: Option<String>,
    pub completed_at: Option<String>,
}

/// Настройки пользователя
#[derive(Debug, Serialize, Deserialize)]
pub struct UserSettings {
    pub download_format: String,      // "mp3", "mp4", etc
    pub video_quality: String,        // "best", "1080p", etc
    pub audio_bitrate: String,        // "128k", "192k", "256k", "320k"
    pub send_as_document: bool,       // для видео
    pub send_audio_as_document: bool, // для аудио
    pub plan: String,                 // "free", "premium", "vip"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_admin: Option<bool>, // true если пользователь админ
}

/// Обновление настроек пользователя
#[derive(Debug, Deserialize)]
pub struct UpdateSettingsRequest {
    pub download_format: Option<String>,
    pub video_quality: Option<String>,
    pub audio_bitrate: Option<String>,
    pub send_as_document: Option<bool>,
    pub send_audio_as_document: Option<bool>,
    pub plan: Option<String>,
}

/// Элемент истории загрузок
#[derive(Debug, Serialize)]
pub struct HistoryItem {
    pub id: i64,
    pub url: String,
    pub title: Option<String>,
    pub format: String,
    pub status: String, // "completed", "failed"
    pub created_at: String,
    pub completed_at: Option<String>,
    pub error: Option<String>,
}

/// Ответ со статистикой пользователя
#[derive(Debug, Serialize)]
pub struct UserStatsResponse {
    pub total_downloads: i64,
    pub successful_downloads: i64,
    pub failed_downloads: i64,
    pub total_size_bytes: Option<i64>,
}

/// Элемент активной очереди пользователя
#[derive(Debug, Serialize)]
pub struct QueueItem {
    pub id: String,
    pub url: String,
    pub format: String,
    pub status: String,
    pub created_at: String,
    pub queue_position: usize,
}

/// Административная статистика
#[derive(Debug, Serialize)]
pub struct AdminStatsResponse {
    pub total_users: i64,
    pub total_downloads: i64,
    pub active_queue: usize,
    pub total_size: u64,
    pub plans: PlanDistribution,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queue: Option<Vec<AdminQueueItem>>,
}

/// Распределение пользователей по планам
#[derive(Debug, Serialize)]
pub struct PlanDistribution {
    pub free: i64,
    pub premium: i64,
    pub vip: i64,
}

/// Элемент очереди для админа
#[derive(Debug, Serialize)]
pub struct AdminQueueItem {
    pub user_id: i64,
    pub url: String,
    pub format: String,
    pub status: String,
    pub created_at: String,
}

// ============================================================================
// СОСТОЯНИЕ ПРИЛОЖЕНИЯ
// ============================================================================

/// Shared state для всех endpoints
#[derive(Clone)]
pub struct WebAppState {
    pub db_pool: Arc<DbPool>,
    pub download_queue: Arc<DownloadQueue>,
    pub rate_limiter: Arc<RateLimiter>,
    pub bot_token: String,
}

// ============================================================================
// ВСПОМОГАТЕЛЬНЫЕ ФУНКЦИИ
// ============================================================================

/// Извлечение user_id из headers (Telegram init data)
async fn extract_user_id(headers: &HeaderMap, bot_token: &str) -> Result<i64, AppError> {
    let init_data = headers
        .get("X-Telegram-Init-Data")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| AppError::Unauthorized("Missing Telegram init data".to_string()))?;

    // Валидация подписи Telegram
    let user_id = webapp_auth::validate_telegram_webapp_data(init_data, bot_token)
        .map_err(|e| AppError::Unauthorized(format!("Invalid init data: {}", e)))?;

    Ok(user_id)
}

/// Форматирование размера файла
fn format_filesize(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

/// Форматирование длительности
fn format_duration(seconds: u32) -> String {
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let secs = seconds % 60;

    if hours > 0 {
        format!("{}:{:02}:{:02}", hours, minutes, secs)
    } else {
        format!("{}:{:02}", minutes, secs)
    }
}

// ============================================================================
// ERROR HANDLING
// ============================================================================

#[derive(Debug)]
pub enum AppError {
    Unauthorized(String),
    BadRequest(String),
    NotFound(String),
    RateLimited(String),
    Internal(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            AppError::Unauthorized(msg) => (StatusCode::UNAUTHORIZED, msg),
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            AppError::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
            AppError::RateLimited(msg) => (StatusCode::TOO_MANY_REQUESTS, msg),
            AppError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
        };

        let body = Json(serde_json::json!({
            "error": message
        }));

        (status, body).into_response()
    }
}

// ============================================================================
// РОУТЕР
// ============================================================================

/// Создает роутер для Mini App
pub fn create_webapp_router(
    db_pool: Arc<DbPool>,
    download_queue: Arc<DownloadQueue>,
    rate_limiter: Arc<RateLimiter>,
    bot_token: String,
) -> Router {
    let state = WebAppState {
        db_pool,
        download_queue,
        rate_limiter,
        bot_token,
    };

    // CORS для Mini App
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Router::new()
        // Статические файлы (HTML, CSS, JS) - root path
        .nest_service("/", ServeDir::new("webapp/static"))
        // API endpoints
        .route("/api/health", get(health_check))
        .route("/api/preview", post(handle_preview))
        .route("/api/download", post(handle_download))
        .route("/api/task/:id/status", get(handle_task_status))
        .route("/api/user/:id/settings", get(handle_get_settings))
        .route("/api/user/:id/settings", patch(handle_update_settings))
        .route("/api/user/:id/history", get(handle_get_history))
        .route("/api/user/:id/queue", get(handle_get_queue))
        .route("/api/user/:id/stats", get(handle_get_stats))
        .route("/api/services", get(handle_get_services))
        .route("/api/admin/stats", get(handle_get_admin_stats))
        .layer(cors)
        .with_state(Arc::new(state))
}

/// Запускает веб-сервер для Mini App
pub async fn run_webapp_server(
    port: u16,
    db_pool: Arc<DbPool>,
    download_queue: Arc<DownloadQueue>,
    rate_limiter: Arc<RateLimiter>,
    bot_token: String,
) -> anyhow::Result<()> {
    let app = create_webapp_router(db_pool, download_queue, rate_limiter, bot_token);

    let addr = format!("0.0.0.0:{}", port);
    log::info!("🌐 Starting Mini App web server on http://{}", addr);
    log::info!("📱 Mini App URL: http://{}/ ", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

// ============================================================================
// API HANDLERS
// ============================================================================

/// Health check endpoint
async fn health_check() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "ok",
        "service": "doradura-webapp"
    }))
}

/// POST /api/preview - Получить метаданные для preview
async fn handle_preview(
    State(state): State<Arc<WebAppState>>,
    headers: HeaderMap,
    Json(req): Json<PreviewRequest>,
) -> Result<Json<PreviewResponse>, AppError> {
    // Валидация user_id
    let user_id = extract_user_id(&headers, &state.bot_token).await?;

    log::info!("Preview request from user {}: {}", user_id, req.url);

    // Парсинг URL
    let url = url::Url::parse(&req.url)
        .map_err(|e| AppError::BadRequest(format!("Invalid URL: {}", e)))?;

    // Получение метаданных через существующую функцию
    let metadata =
        preview::get_preview_metadata(&url, req.format.as_deref(), req.video_quality.as_deref())
            .await
            .map_err(|e| AppError::Internal(format!("Failed to get metadata: {}", e)))?;

    // Форматирование ответа
    let response = PreviewResponse {
        title: Some(metadata.title.clone()),
        artist: Some(metadata.artist.clone()),
        thumbnail_url: metadata.thumbnail_url.clone(),
        duration: metadata.duration,
        duration_formatted: metadata.duration.map(format_duration),
        filesize: metadata.filesize,
        filesize_formatted: metadata.filesize.map(format_filesize),
        description: metadata.description.clone(),
        video_formats: metadata.video_formats.map(|formats| {
            formats
                .into_iter()
                .map(|f| VideoFormatInfo {
                    quality: f.quality.clone(),
                    size_bytes: f.size_bytes,
                    size_formatted: f.size_bytes.map(format_filesize),
                    resolution: f.resolution.clone(),
                })
                .collect()
        }),
        available_formats: vec!["mp3".to_string(), "mp4".to_string(), "srt".to_string()],
    };

    Ok(Json(response))
}

/// POST /api/download - Начать загрузку
async fn handle_download(
    State(state): State<Arc<WebAppState>>,
    headers: HeaderMap,
    Json(req): Json<DownloadRequest>,
) -> Result<Json<DownloadResponse>, AppError> {
    // Валидация user_id
    let user_id = extract_user_id(&headers, &state.bot_token).await?;

    log::info!(
        "Download request from user {}: {} (format: {})",
        user_id,
        req.url,
        req.format
    );

    // Получение настроек пользователя для rate limiting
    let conn = db::get_connection(&state.db_pool)
        .map_err(|e| AppError::Internal(format!("DB error: {}", e)))?;

    let user = match db::get_user(&conn, user_id)
        .map_err(|e| AppError::Internal(format!("Failed to get user: {}", e)))?
    {
        Some(u) => u,
        None => {
            // Создать пользователя если не существует
            db::create_user(&conn, user_id, None)
                .map_err(|e| AppError::Internal(format!("Failed to create user: {}", e)))?;

            // Retry
            db::get_user(&conn, user_id)
                .map_err(|e| {
                    AppError::Internal(format!("Failed to get user after creation: {}", e))
                })?
                .ok_or_else(|| AppError::Internal("User not found after creation".to_string()))?
        }
    };

    let plan = user.plan.as_str();

    // Rate limiting
    if state
        .rate_limiter
        .is_rate_limited(teloxide::types::ChatId(user_id), plan)
        .await
    {
        let remaining = state
            .rate_limiter
            .get_remaining_time(teloxide::types::ChatId(user_id))
            .await
            .map(|d| d.as_secs())
            .unwrap_or(30);

        return Err(AppError::RateLimited(format!(
            "Too many requests. Wait {} seconds",
            remaining
        )));
    }

    // Update rate limit
    state
        .rate_limiter
        .update_rate_limit(teloxide::types::ChatId(user_id), plan)
        .await;

    // Парсинг URL для валидации
    let _ = url::Url::parse(&req.url)
        .map_err(|e| AppError::BadRequest(format!("Invalid URL: {}", e)))?;

    // Создание задачи
    let is_video = req.format == "mp4" || req.format == "mp4+mp3";
    let chat_id = teloxide::types::ChatId(user_id);

    let task = DownloadTask::new(
        req.url.clone(),
        chat_id,
        None, // Web app requests don't have original message
        is_video,
        req.format.clone(),
        req.video_quality.clone(),
        req.audio_bitrate.clone(),
    );

    let task_id = task.id.clone();

    // Добавление в очередь
    state
        .download_queue
        .add_task(task, Some(Arc::clone(&state.db_pool)))
        .await;

    // Получение позиции в очереди (приблизительно)
    let queue_len = state.download_queue.queue.lock().await.len();

    Ok(Json(DownloadResponse {
        task_id,
        queue_position: queue_len,
        estimated_time: None, // TODO: calculate based on queue
    }))
}

/// GET /api/task/:id/status - Получить статус задачи
async fn handle_task_status(
    State(state): State<Arc<WebAppState>>,
    headers: HeaderMap,
    Path(task_id): Path<String>,
) -> Result<Json<TaskStatusResponse>, AppError> {
    // Валидация user_id
    let _user_id = extract_user_id(&headers, &state.bot_token).await?;

    // Получение задачи из БД
    let conn = db::get_connection(&state.db_pool)
        .map_err(|e| AppError::Internal(format!("DB error: {}", e)))?;

    let task = db::get_task_by_id(&conn, &task_id)
        .map_err(|e| AppError::Internal(format!("Failed to get task: {}", e)))?
        .ok_or_else(|| AppError::NotFound("Task not found".to_string()))?;

    Ok(Json(TaskStatusResponse {
        status: task.status.clone(),
        progress: None, // TODO: add progress tracking
        error: task.error_message.clone(),
        created_at: Some(task.created_at.clone()),
        completed_at: None, // TODO: add completed_at to TaskQueueEntry
    }))
}

/// GET /api/user/:id/settings - Получить настройки пользователя
async fn handle_get_settings(
    State(state): State<Arc<WebAppState>>,
    headers: HeaderMap,
    Path(user_id): Path<i64>,
) -> Result<Json<UserSettings>, AppError> {
    // Валидация что запрос от того же пользователя
    let authenticated_user_id = extract_user_id(&headers, &state.bot_token).await?;
    if authenticated_user_id != user_id {
        return Err(AppError::Unauthorized("Access denied".to_string()));
    }

    let conn = db::get_connection(&state.db_pool)
        .map_err(|e| AppError::Internal(format!("DB error: {}", e)))?;

    let user = db::get_user(&conn, user_id)
        .map_err(|e| AppError::Internal(format!("Failed to get user: {}", e)))?
        .ok_or_else(|| AppError::NotFound("User not found".to_string()))?;

    // Проверяем, является ли пользователь админом
    let is_admin = user.username.as_deref() == Some(crate::core::config::admin::ADMIN_USERNAME);

    Ok(Json(UserSettings {
        download_format: user.download_format().to_string(),
        video_quality: user.video_quality.clone(),
        audio_bitrate: user.audio_bitrate.clone(),
        send_as_document: user.send_as_document == 1,
        send_audio_as_document: user.send_audio_as_document == 1,
        plan: user.plan.clone(),
        is_admin: if is_admin { Some(true) } else { None },
    }))
}

/// PATCH /api/user/:id/settings - Обновить настройки пользователя
async fn handle_update_settings(
    State(state): State<Arc<WebAppState>>,
    headers: HeaderMap,
    Path(user_id): Path<i64>,
    Json(req): Json<UpdateSettingsRequest>,
) -> Result<Json<UserSettings>, AppError> {
    // Валидация
    let authenticated_user_id = extract_user_id(&headers, &state.bot_token).await?;
    if authenticated_user_id != user_id {
        return Err(AppError::Unauthorized("Access denied".to_string()));
    }

    let conn = db::get_connection(&state.db_pool)
        .map_err(|e| AppError::Internal(format!("DB error: {}", e)))?;

    // Обновление настроек в БД
    if let Some(format) = req.download_format {
        db::set_user_download_format(&conn, user_id, &format)
            .map_err(|e| AppError::Internal(format!("Failed to update format: {}", e)))?;
    }

    if let Some(quality) = req.video_quality {
        db::set_user_video_quality(&conn, user_id, &quality)
            .map_err(|e| AppError::Internal(format!("Failed to update quality: {}", e)))?;
    }

    if let Some(bitrate) = req.audio_bitrate {
        db::set_user_audio_bitrate(&conn, user_id, &bitrate)
            .map_err(|e| AppError::Internal(format!("Failed to update bitrate: {}", e)))?;
    }

    if let Some(send_as_doc) = req.send_as_document {
        db::set_user_send_as_document(&conn, user_id, if send_as_doc { 1 } else { 0 })
            .map_err(|e| AppError::Internal(format!("Failed to update send mode: {}", e)))?;
    }

    if let Some(send_audio_as_doc) = req.send_audio_as_document {
        db::set_user_send_audio_as_document(&conn, user_id, if send_audio_as_doc { 1 } else { 0 })
            .map_err(|e| AppError::Internal(format!("Failed to update audio send mode: {}", e)))?;
    }

    if let Some(plan) = req.plan {
        let normalized = plan.to_lowercase();
        match normalized.as_str() {
            "free" | "premium" | "vip" => {
                db::update_user_plan(&conn, user_id, &normalized)
                    .map_err(|e| AppError::Internal(format!("Failed to update plan: {}", e)))?;
            }
            _ => {
                return Err(AppError::BadRequest("Unsupported plan".to_string()));
            }
        }
    }

    // Возвращаем обновленные настройки
    handle_get_settings(State(state), headers, Path(user_id)).await
}

/// GET /api/user/:id/history - Получить историю загрузок
async fn handle_get_history(
    State(state): State<Arc<WebAppState>>,
    headers: HeaderMap,
    Path(user_id): Path<i64>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<Vec<HistoryItem>>, AppError> {
    // Валидация
    let authenticated_user_id = extract_user_id(&headers, &state.bot_token).await?;
    if authenticated_user_id != user_id {
        return Err(AppError::Unauthorized("Access denied".to_string()));
    }

    let conn = db::get_connection(&state.db_pool)
        .map_err(|e| AppError::Internal(format!("DB error: {}", e)))?;

    let limit = params
        .get("limit")
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(50);

    // Получение истории из task_queue
    let mut stmt = conn
        .prepare(
            "SELECT
             id,
             url,
             format,
             status,
             created_at,
             CASE WHEN status = 'completed' THEN updated_at ELSE NULL END AS completed_at,
             error_message
         FROM task_queue
         WHERE user_id = ?1
         ORDER BY created_at DESC
         LIMIT ?2",
        )
        .map_err(|e| AppError::Internal(format!("Failed to prepare query: {}", e)))?;

    let history_iter = stmt
        .query_map([user_id, limit], |row| {
            Ok(HistoryItem {
                id: row.get(0)?,
                url: row.get(1)?,
                title: None, // TODO: parse from metadata
                format: row.get(2)?,
                status: row.get(3)?,
                created_at: row.get(4)?,
                completed_at: row.get(5)?,
                error: row.get(6)?,
            })
        })
        .map_err(|e| AppError::Internal(format!("Failed to query history: {}", e)))?;

    let items: Vec<HistoryItem> = history_iter.filter_map(|item| item.ok()).collect();

    Ok(Json(items))
}

/// GET /api/user/:id/queue - Активная очередь пользователя
async fn handle_get_queue(
    State(state): State<Arc<WebAppState>>,
    headers: HeaderMap,
    Path(user_id): Path<i64>,
) -> Result<Json<Vec<QueueItem>>, AppError> {
    let authenticated_user_id = extract_user_id(&headers, &state.bot_token).await?;
    if authenticated_user_id != user_id {
        return Err(AppError::Unauthorized("Access denied".to_string()));
    }

    let conn = db::get_connection(&state.db_pool)
        .map_err(|e| AppError::Internal(format!("DB error: {}", e)))?;

    let mut stmt = conn
        .prepare(
            "SELECT id, user_id, url, format, status, created_at
         FROM task_queue
         WHERE status IN ('pending', 'processing')
         ORDER BY CASE status WHEN 'processing' THEN 0 ELSE 1 END,
                  created_at ASC",
        )
        .map_err(|e| AppError::Internal(format!("Failed to prepare queue query: {}", e)))?;

    let mut rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        })
        .map_err(|e| AppError::Internal(format!("Failed to fetch queue: {}", e)))?;

    let mut items = Vec::new();
    let mut pending_counter: usize = 0;

    while let Some(row) = rows.next() {
        let (task_id, task_user_id, url, format, status, created_at) =
            row.map_err(|e| AppError::Internal(format!("Queue row error: {}", e)))?;

        let mut position = 0usize;
        if status == "pending" {
            pending_counter += 1;
            position = pending_counter;
        }

        if task_user_id == user_id {
            items.push(QueueItem {
                id: task_id,
                url,
                format,
                status: status.clone(),
                created_at,
                queue_position: if status == "pending" { position } else { 0 },
            });
        }
    }

    Ok(Json(items))
}

/// GET /api/user/:id/stats - Получить статистику пользователя
async fn handle_get_stats(
    State(state): State<Arc<WebAppState>>,
    headers: HeaderMap,
    Path(user_id): Path<i64>,
) -> Result<Json<UserStatsResponse>, AppError> {
    // Валидация
    let authenticated_user_id = extract_user_id(&headers, &state.bot_token).await?;
    if authenticated_user_id != user_id {
        return Err(AppError::Unauthorized("Access denied".to_string()));
    }

    let conn = db::get_connection(&state.db_pool)
        .map_err(|e| AppError::Internal(format!("DB error: {}", e)))?;

    let stats = db::get_user_stats(&conn, user_id)
        .map_err(|e| AppError::Internal(format!("Failed to get stats: {}", e)))?;

    Ok(Json(UserStatsResponse {
        total_downloads: stats.total_downloads,
        successful_downloads: stats.total_downloads, // TODO: add proper successful count
        failed_downloads: 0,                         // TODO: add failed count
        total_size_bytes: Some(stats.total_size),
    }))
}

/// GET /api/admin/stats - Получить административную статистику
async fn handle_get_admin_stats(
    State(state): State<Arc<WebAppState>>,
    headers: HeaderMap,
) -> Result<Json<AdminStatsResponse>, AppError> {
    // Аутентификация
    let user_id = extract_user_id(&headers, &state.bot_token).await?;

    let conn = db::get_connection(&state.db_pool)
        .map_err(|e| AppError::Internal(format!("DB error: {}", e)))?;

    // Проверяем, является ли пользователь админом
    let user = db::get_user(&conn, user_id)
        .map_err(|e| AppError::Internal(format!("Failed to get user: {}", e)))?
        .ok_or_else(|| AppError::NotFound("User not found".to_string()))?;

    let is_admin = user.username.as_deref() == Some(crate::core::config::admin::ADMIN_USERNAME);
    if !is_admin {
        return Err(AppError::Unauthorized("Admin access required".to_string()));
    }

    // Получаем всех пользователей для подсчета
    let all_users = db::get_all_users(&conn)
        .map_err(|e| AppError::Internal(format!("Failed to get users: {}", e)))?;

    // Подсчитываем распределение по планам
    let mut plan_free = 0i64;
    let mut plan_premium = 0i64;
    let mut plan_vip = 0i64;

    for user in &all_users {
        match user.plan.as_str() {
            "free" => plan_free += 1,
            "premium" => plan_premium += 1,
            "vip" => plan_vip += 1,
            _ => plan_free += 1, // по умолчанию free
        }
    }

    // Получаем общую статистику загрузок по всем пользователям
    let mut total_downloads = 0i64;

    for user in &all_users {
        if let Ok(history) = db::get_all_download_history(&conn, user.telegram_id) {
            total_downloads += history.len() as i64;
        }
    }

    // Для общего размера данных используем статистику пользователей
    let total_size: u64 = all_users
        .iter()
        .filter_map(|u| {
            db::get_user_stats(&conn, u.telegram_id)
                .ok()
                .map(|stats| stats.total_size as u64)
        })
        .sum();

    // Получаем текущую очередь из БД
    let mut stmt = conn
        .prepare(
            "SELECT id, user_id, url, format, status, created_at
         FROM task_queue
         WHERE status IN ('pending', 'processing')
         ORDER BY CASE status WHEN 'processing' THEN 0 ELSE 1 END,
                  created_at ASC
         LIMIT 20",
        )
        .map_err(|e| AppError::Internal(format!("Failed to prepare queue query: {}", e)))?;

    let queue_iter = stmt
        .query_map([], |row| {
            Ok(AdminQueueItem {
                user_id: row.get(1)?,
                url: row.get(2)?,
                format: row.get(3)?,
                status: row.get(4)?,
                created_at: row.get(5)?,
            })
        })
        .map_err(|e| AppError::Internal(format!("Failed to query queue: {}", e)))?;

    let admin_queue: Vec<AdminQueueItem> = queue_iter.filter_map(|item| item.ok()).collect();

    // Подсчитываем активную очередь
    let mut count_stmt = conn
        .prepare("SELECT COUNT(*) FROM task_queue WHERE status IN ('pending', 'processing')")
        .map_err(|e| AppError::Internal(format!("Failed to prepare count query: {}", e)))?;

    let active_queue: usize = count_stmt.query_row([], |row| row.get(0)).unwrap_or(0);

    Ok(Json(AdminStatsResponse {
        total_users: all_users.len() as i64,
        total_downloads,
        active_queue,
        total_size,
        plans: PlanDistribution {
            free: plan_free,
            premium: plan_premium,
            vip: plan_vip,
        },
        queue: if admin_queue.is_empty() {
            None
        } else {
            Some(admin_queue)
        },
    }))
}

/// GET /api/services - Получить список поддерживаемых сервисов
async fn handle_get_services() -> impl IntoResponse {
    Json(serde_json::json!({
        "services": [
            { "name": "YouTube", "icon": "🎬", "supported": true },
            { "name": "SoundCloud", "icon": "🎵", "supported": true },
            { "name": "Vimeo", "icon": "🎥", "supported": true },
            { "name": "Twitch", "icon": "🟣", "supported": true },
            { "name": "Twitter/X", "icon": "🐦", "supported": true },
            { "name": "Reddit", "icon": "🤖", "supported": true },
            { "name": "Instagram", "icon": "📷", "supported": true },
            { "name": "TikTok", "icon": "🎭", "supported": true },
            { "name": "Dailymotion", "icon": "🎬", "supported": true },
            { "name": "Bandcamp", "icon": "🎵", "supported": true },
        ]
    }))
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_filesize() {
        assert_eq!(format_filesize(500), "500 B");
        assert_eq!(format_filesize(1536), "1.5 KB");
        assert_eq!(format_filesize(1_572_864), "1.5 MB");
        assert_eq!(format_filesize(1_610_612_736), "1.5 GB");
    }

    #[test]
    fn test_format_duration() {
        assert_eq!(format_duration(45), "0:45");
        assert_eq!(format_duration(185), "3:05");
        assert_eq!(format_duration(3665), "1:01:05");
    }
}
