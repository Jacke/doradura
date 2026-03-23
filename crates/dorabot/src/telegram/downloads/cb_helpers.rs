use teloxide::prelude::*;
use teloxide::types::InlineKeyboardMarkup;

use crate::telegram::Bot;

/// Convert any Display error to teloxide::RequestError.
pub(super) fn to_req_err(e: impl std::fmt::Display) -> teloxide::RequestError {
    teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string())))
}

/// Download audio, extract segment, convert to OGG Opus mono, send as voice.
pub(super) async fn send_as_voice_segment(
    bot: &Bot,
    chat_id: ChatId,
    telegram_file_id: &str,
    start_secs: i64,
    duration_secs: i64,
) -> ResponseResult<teloxide::types::Message> {
    // Use /tmp directly -- /data gets cleaned by init-data script (removes subdirs with binlogs)
    let tmp_dir = std::path::PathBuf::from(format!("/tmp/voice_{}", uuid::Uuid::new_v4()));
    tokio::fs::create_dir_all(&tmp_dir).await.map_err(to_req_err)?;

    let input_path = tmp_dir.join("input.mp3");
    let output_path = tmp_dir.join("output.ogg");

    log::info!("Voice: downloading file {} to {:?}", telegram_file_id, input_path);
    crate::telegram::download_file_from_telegram(bot, telegram_file_id, Some(input_path.clone()))
        .await
        .map_err(|e| {
            log::error!("Voice: download failed: {}", e);
            teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string())))
        })?;
    log::info!(
        "Voice: download complete, converting segment start={}s dur={}s",
        start_secs,
        duration_secs
    );

    // Extract segment + convert to OGG Opus mono
    let in_str = input_path.to_string_lossy().to_string();
    let out_str = output_path.to_string_lossy().to_string();
    let seg_start = start_secs;
    let seg_dur = duration_secs;
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<Option<u32>> {
        let mut cmd = std::process::Command::new("ffmpeg");
        cmd.arg("-i").arg(&in_str);
        if seg_start > 0 {
            cmd.arg("-ss").arg(seg_start.to_string());
        }
        if seg_dur > 0 {
            cmd.arg("-t").arg(seg_dur.to_string());
        }
        cmd.args(["-vn", "-ac", "1", "-ar", "48000", "-c:a", "libopus", "-y"])
            .arg(&out_str);
        let output = cmd.output()?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow::anyhow!("ffmpeg failed: {}", stderr));
        }
        // Probe duration for waveform
        let probe = std::process::Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-show_entries",
                "format=duration",
                "-of",
                "default=noprint_wrappers=1:nokey=1",
            ])
            .arg(&out_str)
            .output()?;
        let dur = if probe.status.success() {
            String::from_utf8_lossy(&probe.stdout)
                .trim()
                .parse::<f64>()
                .ok()
                .map(|d| d as u32)
        } else {
            None
        };
        Ok(dur)
    })
    .await
    .map_err(to_req_err)?
    .map_err(to_req_err)?;

    // Always set duration -- required for waveform. Fall back to segment length.
    let dur = result.unwrap_or(duration_secs.max(1) as u32);
    let file_size = tokio::fs::metadata(&output_path).await.map(|m| m.len()).unwrap_or(0);
    log::info!(
        "Voice: sending OGG file {:?} (duration={}s, size={}B / {}KB)",
        output_path,
        dur,
        file_size,
        file_size / 1024
    );

    // Send directly via official Telegram API -- Local Bot API strips waveform metadata.
    // Guard: ensure temp dir cleanup even on error.
    let send_result: Result<teloxide::types::Message, teloxide::RequestError> = async {
        const MAX_VOICE_SIZE: u64 = 10 * 1024 * 1024; // 10 MB safety limit
        if file_size > MAX_VOICE_SIZE {
            return Err(to_req_err(format!("Voice file too large: {} bytes", file_size)));
        }
        let file_bytes = tokio::fs::read(&output_path).await.map_err(to_req_err)?;
        let voice_part = reqwest::multipart::Part::bytes(file_bytes)
            .file_name("voice.ogg")
            .mime_str("audio/ogg")
            .unwrap();
        let form = reqwest::multipart::Form::new()
            .text("chat_id", chat_id.0.to_string())
            .text("duration", dur.to_string())
            .part("voice", voice_part);
        // Use official API (not BOT_API_URL) -- local Bot API strips OGG waveform metadata
        let url = format!("https://api.telegram.org/bot{}/sendVoice", bot.token());
        let resp: serde_json::Value = bot
            .client()
            .post(&url)
            .multipart(form)
            .send()
            .await
            .map_err(to_req_err)?
            .json()
            .await
            .map_err(to_req_err)?;
        log::info!(
            "Voice: official API response ok={}",
            resp.get("ok").and_then(|v| v.as_bool()).unwrap_or(false)
        );
        if resp.get("ok").and_then(|v| v.as_bool()) == Some(true) {
            let result_val = resp.get("result").cloned().unwrap_or(serde_json::Value::Null);
            serde_json::from_value(result_val).map_err(to_req_err)
        } else {
            let desc = resp.get("description").and_then(|v| v.as_str()).unwrap_or("unknown");
            Err(teloxide::RequestError::Api(teloxide::ApiError::Unknown(
                desc.to_string(),
            )))
        }
    }
    .await;

    // Always cleanup temp files, even on error
    let _ = tokio::fs::remove_dir_all(tmp_dir).await;
    send_result
}

/// Build section picker keyboard for lyrics + audio re-send.
/// Callbacks: `downloads:lyrics_send:{download_id}:{session_id}:{idx_or_all}`
pub(super) fn build_lyrics_audio_keyboard(
    download_id: i64,
    session_id: &str,
    sections: &[crate::lyrics::LyricsSection],
) -> InlineKeyboardMarkup {
    use std::collections::HashMap;

    let mut total: HashMap<String, usize> = HashMap::new();
    for s in sections {
        *total.entry(s.name.clone()).or_insert(0) += 1;
    }
    let mut seen: HashMap<String, usize> = HashMap::new();

    let buttons: Vec<teloxide::types::InlineKeyboardButton> = sections
        .iter()
        .enumerate()
        .map(|(idx, s)| {
            let occ = seen.entry(s.name.clone()).or_insert(0);
            *occ += 1;
            let label = if total.get(&s.name).copied().unwrap_or(1) > 1 {
                format!("{} ({})", s.name, occ)
            } else {
                s.name.clone()
            };
            crate::telegram::cb(
                label,
                format!("downloads:lyrics_send:{}:{}:{}", download_id, session_id, idx),
            )
        })
        .collect();

    let mut rows: Vec<Vec<teloxide::types::InlineKeyboardButton>> = buttons.chunks(3).map(|c| c.to_vec()).collect();

    rows.push(vec![crate::telegram::cb(
        "All Lyrics".to_string(),
        format!("downloads:lyrics_send:{}:{}:all", download_id, session_id),
    )]);

    rows.push(vec![crate::telegram::cb(
        "Cancel".to_string(),
        "downloads:cancel".to_string(),
    )]);

    InlineKeyboardMarkup::new(rows)
}
