use crate::storage::db::{get_connection, DbPool, DownloadHistoryEntry};
use anyhow::{Context, Result};
use std::process::Command;
use std::sync::Arc;

/// Refresh missing metadata for download history entries
pub async fn refresh_missing_metadata(
    db_pool: Arc<DbPool>,
    bot_token: String,
    limit: Option<usize>,
    dry_run: bool,
    verbose: bool,
) -> Result<()> {
    let conn = get_connection(&db_pool).context("Failed to get database connection")?;

    // Query for entries with file_id but missing metadata
    let query = r#"
        SELECT id, url, title, format, downloaded_at, file_id, author, file_size,
               duration, video_quality, audio_bitrate, bot_api_url, bot_api_is_local, source_id, part_index
        FROM download_history
        WHERE file_id IS NOT NULL
        AND (format = 'mp3' OR format = 'mp4')
        AND (file_size IS NULL OR duration IS NULL
             OR (format = 'mp4' AND video_quality IS NULL)
             OR (format = 'mp3' AND audio_bitrate IS NULL))
        ORDER BY downloaded_at DESC
    "#;

    let mut stmt = conn.prepare(query)?;
    let entries_iter = stmt.query_map([], |row| {
        Ok(DownloadHistoryEntry {
            id: row.get(0)?,
            url: row.get(1)?,
            title: row.get(2)?,
            format: row.get(3)?,
            downloaded_at: row.get(4)?,
            file_id: row.get(5)?,
            author: row.get(6)?,
            file_size: row.get(7)?,
            duration: row.get(8)?,
            video_quality: row.get(9)?,
            audio_bitrate: row.get(10)?,
            bot_api_url: row.get(11).ok(),
            bot_api_is_local: row.get(12).ok(),
            source_id: row.get(13).ok(),
            part_index: row.get(14).ok(),
        })
    })?;

    let mut entries: Vec<DownloadHistoryEntry> = entries_iter.collect::<Result<Vec<_>, _>>()?;

    if let Some(lim) = limit {
        entries.truncate(lim);
    }

    if entries.is_empty() {
        println!("✅ No entries with missing metadata found!");
        return Ok(());
    }

    println!("📊 Found {} entries with missing metadata", entries.len());

    if dry_run {
        println!("\n🔍 DRY RUN MODE - no changes will be made\n");
    }

    let mut updated_count = 0;
    let mut failed_count = 0;

    for (idx, entry) in entries.iter().enumerate() {
        if verbose || dry_run {
            println!(
                "\n[{}/{}] Processing: {} (format: {}, file_id: {})",
                idx + 1,
                entries.len(),
                entry.title,
                entry.format,
                entry.file_id.as_deref().unwrap_or("N/A")
            );
        }

        let file_id = match &entry.file_id {
            Some(fid) => fid,
            None => {
                if verbose {
                    println!("  ⚠️  Skipping: no file_id");
                }
                continue;
            }
        };

        // Determine what's missing
        let missing = get_missing_fields(entry);
        if missing.is_empty() {
            if verbose {
                println!("  ✓ No missing fields");
            }
            continue;
        }

        if verbose || dry_run {
            println!("  Missing: {}", missing.join(", "));
        }

        if dry_run {
            continue;
        }

        // Download file from Telegram to temporary location
        match download_telegram_file(&bot_token, file_id).await {
            Ok(file_path) => {
                // Extract metadata using ffprobe
                match extract_metadata(&file_path, &entry.format).await {
                    Ok(metadata) => {
                        // Update database
                        match update_metadata(&conn, entry.id, &metadata) {
                            Ok(_) => {
                                updated_count += 1;
                                if verbose {
                                    println!("  ✅ Updated: {:?}", metadata);
                                }
                            }
                            Err(e) => {
                                failed_count += 1;
                                println!("  ❌ Failed to update database: {}", e);
                            }
                        }
                    }
                    Err(e) => {
                        failed_count += 1;
                        println!("  ❌ Failed to extract metadata: {}", e);
                    }
                }

                // Clean up temporary file
                let _ = std::fs::remove_file(&file_path);
            }
            Err(e) => {
                failed_count += 1;
                println!("  ❌ Failed to download file: {}", e);
            }
        }

        // Add a small delay to avoid rate limiting
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    println!("\n{}", "═".repeat(60));
    println!("📊 Metadata Refresh Summary:");
    println!("   • Total entries found: {}", entries.len());
    println!("   • Successfully updated: {}", updated_count);
    println!("   • Failed: {}", failed_count);
    println!("{}", "═".repeat(60));

    Ok(())
}

fn get_missing_fields(entry: &DownloadHistoryEntry) -> Vec<&str> {
    let mut missing = Vec::new();

    if entry.file_size.is_none() {
        missing.push("file_size");
    }
    if entry.duration.is_none() {
        missing.push("duration");
    }
    if entry.format == "mp4" && entry.video_quality.is_none() {
        missing.push("video_quality");
    }
    if entry.format == "mp3" && entry.audio_bitrate.is_none() {
        missing.push("audio_bitrate");
    }

    missing
}

async fn download_telegram_file(bot_token: &str, file_id: &str) -> Result<String> {
    use reqwest;

    // Get file path from Telegram
    let get_file_url = format!("https://api.telegram.org/bot{}/getFile?file_id={}", bot_token, file_id);
    let client = reqwest::Client::new();
    let response = client.get(&get_file_url).send().await?;
    let json: serde_json::Value = response.json().await?;

    let file_path = json["result"]["file_path"]
        .as_str()
        .context("Failed to get file_path from Telegram response")?;

    // Download the file
    let download_url = format!("https://api.telegram.org/file/bot{}/{}", bot_token, file_path);
    let file_response = client.get(&download_url).send().await?;
    let bytes = file_response.bytes().await?;

    // Save to temporary file
    let temp_dir = std::env::temp_dir();
    let temp_file_name = format!("telegram_download_{}.tmp", uuid::Uuid::new_v4());
    let temp_file_path = temp_dir.join(temp_file_name);

    std::fs::write(&temp_file_path, bytes)?;

    Ok(temp_file_path.to_string_lossy().to_string())
}

#[derive(Debug)]
struct Metadata {
    file_size: Option<i64>,
    duration: Option<i64>,
    video_quality: Option<String>,
    audio_bitrate: Option<String>,
}

async fn extract_metadata(file_path: &str, format: &str) -> Result<Metadata> {
    // Get file size
    let file_size = std::fs::metadata(file_path)?.len() as i64;

    // Use ffprobe to get duration and quality/bitrate
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration:stream=height,bit_rate,codec_name",
            "-of",
            "json",
            file_path,
        ])
        .output()
        .context("Failed to run ffprobe")?;

    if !output.status.success() {
        anyhow::bail!("ffprobe failed: {}", String::from_utf8_lossy(&output.stderr));
    }

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).context("Failed to parse ffprobe output")?;

    // Extract duration
    let duration = json["format"]["duration"]
        .as_str()
        .and_then(|d| d.parse::<f64>().ok())
        .map(|d| d as i64);

    let mut video_quality = None;
    let mut audio_bitrate = None;

    if format == "mp4" {
        // Find video stream and extract height
        if let Some(streams) = json["streams"].as_array() {
            for stream in streams {
                if stream["codec_name"].as_str() == Some("h264") || stream["codec_name"].as_str() == Some("hevc") {
                    if let Some(height) = stream["height"].as_u64() {
                        video_quality = Some(format!("{}p", height));
                        break;
                    }
                }
            }
        }
    } else if format == "mp3" {
        // Find audio stream and extract bitrate
        if let Some(streams) = json["streams"].as_array() {
            for stream in streams {
                if stream["codec_name"].as_str() == Some("mp3") {
                    if let Some(bit_rate) = stream["bit_rate"].as_str().and_then(|br| br.parse::<u64>().ok()) {
                        let kbps = bit_rate / 1000;
                        audio_bitrate = Some(format!("{}k", kbps));
                        break;
                    }
                }
            }
        }
    }

    Ok(Metadata {
        file_size: Some(file_size),
        duration,
        video_quality,
        audio_bitrate,
    })
}

fn update_metadata(conn: &rusqlite::Connection, entry_id: i64, metadata: &Metadata) -> Result<()> {
    let mut updates = Vec::new();
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

    if let Some(fs) = metadata.file_size {
        updates.push("file_size = ?");
        params.push(Box::new(fs));
    }
    if let Some(dur) = metadata.duration {
        updates.push("duration = ?");
        params.push(Box::new(dur));
    }
    if let Some(ref vq) = metadata.video_quality {
        updates.push("video_quality = ?");
        params.push(Box::new(vq.clone()));
    }
    if let Some(ref ab) = metadata.audio_bitrate {
        updates.push("audio_bitrate = ?");
        params.push(Box::new(ab.clone()));
    }

    if updates.is_empty() {
        return Ok(());
    }

    let query = format!("UPDATE download_history SET {} WHERE id = ?", updates.join(", "));
    params.push(Box::new(entry_id));

    let params_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    conn.execute(&query, params_refs.as_slice())?;

    Ok(())
}
