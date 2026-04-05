use crate::telegram::Bot;
use anyhow::Result;
use std::path::PathBuf;
use teloxide::prelude::*;
use url::Url;

/// Downloads a file from Telegram by file_id and saves it locally
///
/// # Arguments
/// * `bot` - Telegram bot instance
/// * `file_id` - Telegram file_id to download
/// * `destination_path` - Optional custom path to save the file. If None, saves to ./downloads/
///
/// # Returns
/// * `Ok(PathBuf)` - Path to the downloaded file
/// * `Err(anyhow::Error)` - If download fails
///
/// # Example
/// ```ignore
/// # use doradura::telegram::{Bot, download_file_from_telegram};
/// # async fn run() -> anyhow::Result<()> {
/// let teloxide_bot = teloxide::Bot::new("BOT_TOKEN");
/// let bot = Bot::new(teloxide_bot);
/// let path = download_file_from_telegram(&bot, "BQACAgIAAxkBAAIBCGXxxx...", None).await?;
/// println!("File saved to: {:?}", path);
/// # Ok(())
/// # }
/// ```
pub async fn download_file_from_telegram(
    bot: &Bot,
    file_id: &str,
    destination_path: Option<PathBuf>,
) -> Result<PathBuf> {
    log::info!("📥 Starting download for file_id: {}", file_id);

    // Get file info from Telegram
    use teloxide::types::FileId;
    let file = bot.get_file(FileId(file_id.to_string())).await?;
    log::info!(
        "✅ File info retrieved: path = '{}', size = {} bytes",
        file.path,
        file.size
    );
    log::debug!("📋 Raw file.path from Bot API: {:?}", file.path);

    // Determine destination path
    let dest_path = if let Some(custom_path) = destination_path {
        custom_path
    } else {
        // Create downloads directory if it doesn't exist
        let downloads_dir = PathBuf::from("./downloads");
        tokio::fs::create_dir_all(&downloads_dir).await?;

        // Generate filename from file_id or use original filename from Telegram path
        // Telegram path format: "documents/file_123.pdf" or "photos/file_456.jpg"
        let filename = PathBuf::from(&file.path)
            .file_name()
            .and_then(|n| n.to_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("file_{}.bin", &file_id[..20.min(file_id.len())]));

        downloads_dir.join(filename)
    };

    log::info!("📂 Destination path: {:?}", dest_path);

    let (bot_api_url, bot_api_is_local) = std::env::var("BOT_API_URL")
        .ok()
        .map(|u| {
            let is_local = !u.contains("api.telegram.org");
            (Some(u), is_local)
        })
        .unwrap_or((None, false));

    let base_url_str = bot_api_url.as_deref().unwrap_or("https://api.telegram.org");

    // For local Bot API with BOT_API_DATA_DIR, copy file directly from mounted volume
    if bot_api_is_local {
        if let Ok(data_dir) = std::env::var("BOT_API_DATA_DIR") {
            log::info!("🔍 BOT_API_DATA_DIR: {}", data_dir);
            log::info!("🔍 file.path from Bot API: {}", file.path);

            // file.path can be:
            // - Old format: /telegram-bot-api/8224275354:.../videos/file_1.mp4
            // - New format: /data/8224275354:.../videos/file_1.mp4
            // Try both prefixes
            let prefixes = ["/data/", "/telegram-bot-api/"];
            let mut matched_prefix = None;
            let mut relative_path_str = String::new();

            for prefix in &prefixes {
                if let Some(rel_path) = file.path.strip_prefix(prefix) {
                    matched_prefix = Some(*prefix);
                    relative_path_str = rel_path.to_string();
                    log::info!("✅ Matched prefix '{}', relative path: {}", prefix, rel_path);
                    break;
                }
            }

            if let Some(_prefix) = matched_prefix {
                let source_path = std::path::Path::new(&data_dir).join(&relative_path_str);
                log::info!("📂 Local Bot API: attempting direct file copy from {:?}", source_path);

                let max_attempts = 6;
                let mut last_size: Option<u64> = None;
                let mut stable_count = 0;

                for attempt in 1..=max_attempts {
                    log::info!(
                        "🔍 Checking source file (attempt {}/{}): {:?}",
                        attempt,
                        max_attempts,
                        source_path
                    );
                    if let Ok(metadata) = tokio::fs::metadata(&source_path).await {
                        let size = metadata.len();
                        log::info!("📏 File size: {} bytes", size);
                        if size > 0 {
                            if Some(size) == last_size {
                                stable_count += 1;
                                log::info!("✅ File size stable (count={})", stable_count);
                            } else {
                                stable_count = 0;
                                log::info!(
                                    "⏳ File size changed: {} -> {} bytes (still writing...)",
                                    last_size.unwrap_or(0),
                                    size
                                );
                            }
                            last_size = Some(size);

                            if stable_count >= 1 {
                                log::info!(
                                    "✅ File exists locally and stable (size={} bytes), copying directly...",
                                    size
                                );
                                // Ensure parent directory exists
                                if let Some(parent) = dest_path.parent() {
                                    log::info!("📁 Creating parent directory: {:?}", parent);
                                    if let Err(e) = tokio::fs::create_dir_all(parent).await {
                                        log::error!("❌ Failed to create parent directory {:?}: {}", parent, e);
                                        return Err(anyhow::anyhow!("Failed to create directory: {}", e));
                                    }
                                    log::info!("✅ Parent directory ready");
                                }

                                // Check source file permissions
                                match tokio::fs::metadata(&source_path).await {
                                    Ok(meta) => {
                                        log::info!(
                                            "📋 Source file permissions: readonly={}, len={}",
                                            meta.permissions().readonly(),
                                            meta.len()
                                        );
                                    }
                                    Err(e) => {
                                        log::error!("❌ Cannot read source file metadata: {}", e);
                                    }
                                }

                                // Remove destination if it exists (might be from failed previous attempt)
                                if dest_path.exists() {
                                    log::warn!("⚠️ Destination file already exists, removing: {:?}", dest_path);
                                    if let Err(e) = tokio::fs::remove_file(&dest_path).await {
                                        log::error!("❌ Failed to remove existing destination: {}", e);
                                    }
                                }

                                log::info!("📥 Copying {} -> {}", source_path.display(), dest_path.display());
                                if let Err(e) = tokio::fs::copy(&source_path, &dest_path).await {
                                    log::error!(
                                        "❌ Copy failed: {} (source={:?}, dest={:?})",
                                        e,
                                        source_path,
                                        dest_path
                                    );
                                    return Err(anyhow::anyhow!("Copy failed: {}", e));
                                }
                                log::info!("✅ File copied successfully to: {:?}", dest_path);
                                log::info!(
                                    "📊 Final file size: {} bytes ({:.2} MB)",
                                    file.size,
                                    file.size as f64 / (1024.0 * 1024.0)
                                );
                                return Ok(dest_path);
                            }
                        } else {
                            log::warn!("⚠️ File exists but size is 0 bytes");
                        }
                    } else {
                        log::warn!("⚠️ File not found yet at {:?}", source_path);
                        // Try to check if parent directory exists and is accessible
                        if let Some(parent) = source_path.parent() {
                            match tokio::fs::metadata(parent).await {
                                Ok(_) => log::info!("✅ Parent directory exists: {:?}", parent),
                                Err(e) => log::error!("❌ Cannot access parent directory {:?}: {}", parent, e),
                            }
                        }
                    }

                    if attempt < max_attempts {
                        log::info!(
                            "⏳ Waiting for local file to finish writing (attempt {}/{})",
                            attempt,
                            max_attempts
                        );
                        tokio::time::sleep(std::time::Duration::from_millis(800)).await;
                    }
                }

                if let Some(size) = last_size {
                    log::warn!(
                        "⚠️ Local file not ready for copy after {} attempts (last size={} bytes)",
                        max_attempts,
                        size
                    );
                } else {
                    log::warn!("⚠️ Local file not found at {:?}", source_path);
                }
            } else {
                log::warn!(
                    "⚠️ File path doesn't start with any expected prefix (['/data/', '/telegram-bot-api/']), got: {}",
                    file.path
                );
                log::info!("🔄 Trying to use file.path directly as relative path");

                // Try treating file.path as already relative or absolute path that needs BOT_API_DATA_DIR
                let source_path = if file.path.starts_with('/') {
                    // It's absolute path, maybe already pointing to /data
                    std::path::PathBuf::from(&file.path)
                } else {
                    // It's relative path
                    std::path::Path::new(&data_dir).join(&file.path)
                };

                log::info!("📂 Trying direct path: {:?}", source_path);

                if let Ok(metadata) = tokio::fs::metadata(&source_path).await {
                    let size = metadata.len();
                    log::info!("✅ Found file at direct path! Size: {} bytes", size);
                    if size > 0 {
                        tokio::fs::copy(&source_path, &dest_path).await?;
                        log::info!("✅ File copied successfully to: {:?}", dest_path);
                        return Ok(dest_path);
                    }
                } else {
                    log::warn!("❌ File not found at direct path either: {:?}", source_path);
                }
            }
        } else {
            log::warn!("⚠️ BOT_API_DATA_DIR not set, will try HTTP fallback (will likely fail)");
        }
    }

    // Skip pre-check for local Bot API - just try to download directly
    // If it fails with 404, we'll fallback to api.telegram.org in the download logic below
    log::info!("📥 Attempting to download from: {}", base_url_str);

    let base_url =
        Url::parse(base_url_str).map_err(|e| anyhow::anyhow!("Invalid Bot API base URL for file download: {}", e))?;

    let file_url = build_file_url(&base_url, bot.token(), &file.path)?;
    log::info!("🌐 Telegram Bot API GET request URL: {}", file_url);
    log::debug!(
        "🔍 URL building details: base={}, token_len={}, file_path={}",
        base_url_str,
        bot.token().len(),
        file.path
    );

    // Download via HTTP (teloxide::Bot::download_file uses api.telegram.org internally)
    use tokio::io::AsyncWriteExt;
    let client = reqwest::Client::builder()
        .timeout(crate::config::network::timeout())
        .build()?;

    let tmp_path = dest_path.with_file_name(format!(
        "{}.part",
        dest_path.file_name().and_then(|n| n.to_str()).unwrap_or("download")
    ));

    let mut resp = client.get(file_url.clone()).send().await?;
    let status = resp.status();

    // If local Bot API returns 404, retry with official api.telegram.org
    if status == reqwest::StatusCode::NOT_FOUND && bot_api_is_local && base_url_str != "https://api.telegram.org" {
        log::warn!(
            "⚠️ File not found on local Bot API ({}), retrying with api.telegram.org",
            file.path
        );

        // Create a temporary bot instance pointed at official API to get correct file path
        // IMPORTANT: Must explicitly set API URL to avoid using BOT_API_URL env var
        let official_bot = teloxide::Bot::new(bot.token().to_string())
            .set_api_url(reqwest::Url::parse("https://api.telegram.org").expect("Failed to parse official API URL"));

        // Re-fetch file info from official API to get the correct path
        use teloxide::types::FileId;
        let official_file = official_bot
            .get_file(FileId(file_id.to_string()))
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get file info from official API: {}", e))?;

        log::info!("📥 Official API file path: {}", official_file.path);

        let fallback_base =
            Url::parse("https://api.telegram.org").map_err(|e| anyhow::anyhow!("Invalid fallback URL: {}", e))?;
        let fallback_url = build_file_url(&fallback_base, bot.token(), &official_file.path)?;

        resp = client.get(fallback_url).send().await?;
        let fallback_status = resp.status();

        if !fallback_status.is_success() && fallback_status != reqwest::StatusCode::PARTIAL_CONTENT {
            let body = resp.text().await.unwrap_or_default();
            tokio::fs::remove_file(&tmp_path).await.ok();
            return Err(anyhow::anyhow!(
                "Telegram file download failed on both local Bot API and api.telegram.org (path={}, local_status={}, fallback_status={}): {}",
                file.path,
                status,
                fallback_status,
                body
            ));
        }

        log::info!("✅ File downloaded successfully from api.telegram.org (fallback)");
    } else if !status.is_success() && status != reqwest::StatusCode::PARTIAL_CONTENT {
        let body = resp.text().await.unwrap_or_default();
        tokio::fs::remove_file(&tmp_path).await.ok();
        return Err(anyhow::anyhow!(
            "Telegram file download failed (base={}, path={}, status={}): {}",
            base_url_str,
            file.path,
            status,
            body
        ));
    }

    let mut dst = tokio::fs::File::create(&tmp_path).await?;
    while let Some(chunk) = resp.chunk().await? {
        dst.write_all(&chunk).await?;
    }
    dst.flush().await.ok();
    tokio::fs::rename(&tmp_path, &dest_path).await?;

    log::info!("✅ File downloaded successfully to: {:?}", dest_path);
    log::info!(
        "📊 File size: {} bytes ({:.2} MB)",
        file.size,
        file.size as f64 / (1024.0 * 1024.0)
    );

    Ok(dest_path)
}

/// Downloads a file from Telegram with fallback chain:
/// 1. Local Bot API server (if BOT_API_DATA_DIR is set)
/// 2. Bot API HTTP download (local or official)
/// 3. MTProto direct download (using message_id to get fresh file_reference)
///
/// # Arguments
/// * `bot` - Telegram bot instance
/// * `file_id` - Telegram file_id
/// * `message_id` - Optional message_id for MTProto fallback
/// * `chat_id` - Optional chat_id for MTProto fallback
/// * `destination_path` - Where to save the file
///
/// # Returns
/// Path to the downloaded file or an error
pub async fn download_file_with_fallback(
    bot: &Bot,
    file_id: &str,
    message_id: Option<i32>,
    chat_id: Option<i64>,
    destination_path: Option<PathBuf>,
) -> Result<PathBuf> {
    log::info!(
        "📥 Starting download with fallback chain: file_id={}, message_id={:?}",
        &file_id[..20.min(file_id.len())],
        message_id
    );

    // Try standard Bot API download first
    match download_file_from_telegram(bot, file_id, destination_path.clone()).await {
        Ok(path) => {
            log::info!("✅ Downloaded via Bot API: {:?}", path);
            Ok(path)
        }
        Err(e) => {
            log::warn!("⚠️ Bot API download failed: {}", e);

            // Check if we have message_id for MTProto fallback
            if let (Some(msg_id), Some(_chat)) = (message_id, chat_id) {
                log::info!("🔄 Attempting MTProto fallback with message_id={}", msg_id);

                match download_via_mtproto(msg_id, destination_path).await {
                    Ok(path) => {
                        log::info!("✅ Downloaded via MTProto: {:?}", path);
                        Ok(path)
                    }
                    Err(mtproto_err) => {
                        log::error!("❌ MTProto fallback also failed: {}", mtproto_err);
                        Err(anyhow::anyhow!(
                            "File download failed. Bot API error: {}. MTProto error: {}",
                            e,
                            mtproto_err
                        ))
                    }
                }
            } else {
                log::warn!("⚠️ No message_id available for MTProto fallback");
                Err(anyhow::anyhow!(
                    "File download failed and no message_id available for MTProto fallback: {}",
                    e
                ))
            }
        }
    }
}

/// Downloads a file via MTProto using message_id to get fresh file_reference
async fn download_via_mtproto(message_id: i32, destination_path: Option<PathBuf>) -> Result<PathBuf> {
    use crate::mtproto::{MtProtoClient, MtProtoDownloader};

    // Load MTProto credentials from environment
    let api_id: i32 = std::env::var("TELEGRAM_API_ID")
        .map_err(|_| anyhow::anyhow!("TELEGRAM_API_ID not set"))?
        .parse()
        .map_err(|e| anyhow::anyhow!("Invalid TELEGRAM_API_ID: {}", e))?;

    let api_hash = std::env::var("TELEGRAM_API_HASH").map_err(|_| anyhow::anyhow!("TELEGRAM_API_HASH not set"))?;

    let bot_token = std::env::var("BOT_TOKEN")
        .or_else(|_| std::env::var("TELOXIDE_TOKEN"))
        .map_err(|_| anyhow::anyhow!("BOT_TOKEN or TELOXIDE_TOKEN not set"))?;

    let session_path = std::env::var("MTPROTO_SESSION_PATH").unwrap_or_else(|_| "mtproto_session.bin".to_string());

    log::info!("🔌 Initializing MTProto client for fallback download...");

    let client = MtProtoClient::new_bot(api_id, &api_hash, &bot_token, std::path::Path::new(&session_path))
        .await
        .map_err(|e| anyhow::anyhow!("Failed to initialize MTProto client: {}", e))?;

    let downloader = MtProtoDownloader::with_bot_token(client, bot_token);

    // Get message with fresh media info
    log::info!("📨 Fetching message {} for fresh file_reference...", message_id);
    let messages = downloader
        .get_bot_messages(&[message_id])
        .await
        .map_err(|e| anyhow::anyhow!("Failed to get message via MTProto: {}", e))?;

    let message = messages
        .first()
        .ok_or_else(|| anyhow::anyhow!("Message {} not found via MTProto", message_id))?;

    let media = message
        .media
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Message {} has no media", message_id))?;

    // Determine output path
    let output_path = destination_path.unwrap_or_else(|| {
        let filename = media
            .filename
            .clone()
            .unwrap_or_else(|| format!("mtproto_download_{}.bin", message_id));
        PathBuf::from("./downloads").join(filename)
    });

    // Ensure parent directory exists
    if let Some(parent) = output_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    log::info!("📥 Downloading via MTProto: {} bytes to {:?}", media.size, output_path);

    downloader
        .download_media(media, &output_path)
        .await
        .map_err(|e| anyhow::anyhow!("MTProto download failed: {}", e))?;

    Ok(output_path)
}

fn build_file_url(base: &Url, token: &str, file_path: &str) -> Result<Url> {
    let mut url = base.clone();

    // For local Bot API in --local mode, file_path is absolute filesystem path like:
    // Old format: "/telegram-bot-api/<token>/videos/file_1.mp4"
    // New format: "/data/<token>/videos/file_1.mp4"
    // We need to extract only the relative part after the token directory
    let normalized_path = if !base.as_str().contains("api.telegram.org") {
        let mut stripped = file_path.trim_start_matches('/');

        // Remove "data/" prefix if present (new format)
        if let Some(rest) = stripped.strip_prefix("data/") {
            stripped = rest;
        }

        // Remove "telegram-bot-api/" prefix if present (old format)
        if let Some(rest) = stripped.strip_prefix("telegram-bot-api/") {
            stripped = rest;
        }

        // Remove token directory (e.g., "6310079371:AAH5...wpUw/")
        // The token in path doesn't have "bot" prefix
        if let Some(rest) = stripped.strip_prefix(token) {
            stripped = rest.trim_start_matches('/');
        }

        log::debug!(
            "🔧 Normalized path for local Bot API: '{}' -> '{}'",
            file_path,
            stripped
        );
        stripped
    } else {
        // Official API: use file_path as-is
        file_path
    };

    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|_| anyhow::anyhow!("BOT_API_URL cannot be a base URL"))?;
        segments.push("file");
        segments.push(&format!("bot{token}"));
        for seg in normalized_path.split('/') {
            if !seg.is_empty() {
                segments.push(seg);
            }
        }
    }
    Ok(url)
}
