//! Background download runner for dora TUI.
//!
//! Spawns yt-dlp as a child process, streams progress events back to the
//! main loop via an mpsc channel. Handles MP3 and MP4 downloads.
//!
//! Uses `doradura-core` for metadata resolution and progress parsing,
//! but calls yt-dlp directly (no Telegram/proxy overhead).

use std::collections::VecDeque;
use std::path::PathBuf;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;

use crate::app::DownloadFormat;
use crate::settings::DoraSettings;

/// Events emitted by a background download task → main event channel.
/// Each event is tagged with the slot's stable ID.
#[derive(Debug)]
pub enum SlotEvent {
    /// yt-dlp is fetching metadata.
    Fetching,
    /// Metadata resolved — update slot title/artist.
    Metadata {
        title: Option<String>,
        artist: Option<String>,
    },
    /// Progress update.
    Progress { percent: u8, speed_mbs: f64, eta_secs: u64 },
    /// Download finished successfully.
    Done { path: String, size_mb: f64 },
    /// Download failed.
    Failed { reason: String },
}

/// Spawn a background download for the given slot ID and return immediately.
///
/// Progress events are sent on `tx` as `(slot_id, SlotEvent)`.
/// Returns an [`AbortHandle`] that can be used to cancel the download task.
pub fn spawn_download(
    slot_id: usize,
    url: String,
    format: DownloadFormat,
    settings: DoraSettings,
    tx: mpsc::Sender<(usize, SlotEvent)>,
) -> tokio::task::AbortHandle {
    tokio::spawn(async move {
        run_download(slot_id, url, format, settings, tx).await;
    })
    .abort_handle()
}

// ── Internal implementation ───────────────────────────────────────────────────

async fn run_download(
    slot_id: usize,
    url: String,
    format: DownloadFormat,
    settings: DoraSettings,
    tx: mpsc::Sender<(usize, SlotEvent)>,
) {
    log::info!("[slot {}] start download: {} ({:?})", slot_id, url, format);

    // Signal that we are fetching metadata
    let _ = tx.send((slot_id, SlotEvent::Fetching)).await;

    // 1. Resolve title + artist via core's SourceRegistry (best-effort)
    let (title, artist) = fetch_metadata(&url).await;
    log::debug!("[slot {}] metadata: title={:?} artist={:?}", slot_id, title, artist);
    let _ = tx
        .send((
            slot_id,
            SlotEvent::Metadata {
                title: title.clone(),
                artist: artist.clone(),
            },
        ))
        .await;

    // 2. Prepare output directory (from settings, or ~/Downloads fallback)
    let out_dir = PathBuf::from(settings.output_dir());
    if !out_dir.exists() {
        let _ = tokio::fs::create_dir_all(&out_dir).await;
    }
    let template = format!("{}/%(title)s.%(ext)s", out_dir.to_string_lossy());

    // 3. Build yt-dlp command args
    let ytdlp = if settings.ytdlp_bin.trim().is_empty() {
        ytdlp_bin()
    } else {
        settings.ytdlp_bin.clone()
    };
    let mut args: Vec<String> = vec![
        "-o".to_string(),
        template,
        "--newline".to_string(),
        "--no-playlist".to_string(),
        "--no-check-certificate".to_string(),
    ];

    // Rate limit
    if let Some(rate) = settings.rate_limit_arg() {
        args.push("--limit-rate".to_string());
        args.push(rate.to_string());
    }

    // Cookies file
    if let Some(cf) = settings.cookies_opt() {
        args.push("--cookies".to_string());
        args.push(cf);
    }

    match format {
        DownloadFormat::Mp3 => {
            let bitrate = if settings.audio_bitrate.is_empty() {
                "320k".to_string()
            } else {
                settings.audio_bitrate.clone()
            };
            args.extend([
                "--extract-audio".to_string(),
                "--audio-format".to_string(),
                "mp3".to_string(),
                "--audio-quality".to_string(),
                bitrate,
                "--add-metadata".to_string(),
            ]);
        }
        DownloadFormat::Mp4 => {
            let fmt_filter = match settings.video_quality.as_str() {
                "1080p" => "bestvideo[height<=1080][ext=mp4]+bestaudio[ext=m4a]/best[height<=1080][ext=mp4]/best",
                "720p" => "bestvideo[height<=720][ext=mp4]+bestaudio[ext=m4a]/best[height<=720][ext=mp4]/best",
                "480p" => "bestvideo[height<=480][ext=mp4]+bestaudio[ext=m4a]/best[height<=480][ext=mp4]/best",
                "360p" => "bestvideo[height<=360][ext=mp4]+bestaudio[ext=m4a]/best[height<=360][ext=mp4]/best",
                _ => "bestvideo[ext=mp4]+bestaudio[ext=m4a]/best[ext=mp4]/best",
            };
            args.extend([
                "--format".to_string(),
                fmt_filter.to_string(),
                "--merge-output-format".to_string(),
                "mp4".to_string(),
            ]);
        }
    }

    args.push(url.clone());

    log::info!("[slot {}] yt-dlp cmd: {} {}", slot_id, ytdlp, args.join(" "));

    // 4. Spawn yt-dlp
    let mut child = match tokio::process::Command::new(&ytdlp)
        .args(&args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            let _ = tx
                .send((
                    slot_id,
                    SlotEvent::Failed {
                        reason: format!("Cannot start yt-dlp: {}", e),
                    },
                ))
                .await;
            return;
        }
    };

    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");

    // Track the final file path as yt-dlp announces destinations
    let captured_path = std::sync::Arc::new(tokio::sync::Mutex::new(None::<String>));

    // Stream stdout: progress events + destination capture
    let tx_out = tx.clone();
    let path_out = captured_path.clone();
    let stdout_task = tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(dest) = parse_destination(&line) {
                *path_out.lock().await = Some(dest);
            }
            if let Some(ev) = parse_yt_progress(&line) {
                let _ = tx_out.send((slot_id, ev)).await;
            }
        }
    });

    // Stream stderr: progress events + destination capture + error collection
    let tx_err = tx.clone();
    let path_err = captured_path.clone();
    let stderr_task = tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        let mut tail: VecDeque<String> = VecDeque::new();
        while let Ok(Some(line)) = lines.next_line().await {
            log::debug!("yt-dlp stderr: {}", line);
            if let Some(dest) = parse_destination(&line) {
                *path_err.lock().await = Some(dest);
            }
            if let Some(ev) = parse_yt_progress(&line) {
                let _ = tx_err.send((slot_id, ev)).await;
            }
            tail.push_back(line);
            if tail.len() > 60 {
                tail.pop_front();
            }
        }
        tail
    });

    let status = match child.wait().await {
        Ok(s) => s,
        Err(e) => {
            let _ = tx.send((slot_id, SlotEvent::Failed { reason: e.to_string() })).await;
            return;
        }
    };

    let _ = stdout_task.await;
    let tail = stderr_task.await.unwrap_or_default();

    if status.success() {
        let path = captured_path.lock().await.clone();
        let path_str = path.unwrap_or_else(|| out_dir.to_string_lossy().to_string());
        let size_mb = std::fs::metadata(&path_str)
            .map(|m| m.len() as f64 / 1_048_576.0)
            .unwrap_or(0.0);
        log::info!("[slot {}] done: {} ({:.1} MB)", slot_id, path_str, size_mb);
        let _ = tx
            .send((
                slot_id,
                SlotEvent::Done {
                    path: path_str,
                    size_mb,
                },
            ))
            .await;
    } else {
        // Find the last meaningful error line (skip warnings and empty lines)
        let reason = tail
            .iter()
            .rev()
            .find(|l| {
                let t = l.trim();
                !t.is_empty() && !t.starts_with("WARNING") && !t.starts_with("[download]")
            })
            .cloned()
            .unwrap_or_else(|| "Download failed".to_string());
        let reason = reason.trim_start_matches("ERROR: ").trim().to_string();
        log::warn!("[slot {}] failed: {}", slot_id, reason);
        let _ = tx.send((slot_id, SlotEvent::Failed { reason })).await;
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Fetch title + artist using doradura-core's SourceRegistry (best-effort).
async fn fetch_metadata(url: &str) -> (Option<String>, Option<String>) {
    let parsed = match url.parse::<url::Url>() {
        Ok(u) => u,
        Err(_) => return (None, None),
    };
    let registry = doracore::download::source::SourceRegistry::global();
    match registry.resolve(&parsed) {
        Some(source) => match source.get_metadata(&parsed).await {
            Ok(meta) => (
                Some(meta.title).filter(|s| !s.is_empty()),
                Some(meta.artist).filter(|s| !s.is_empty()),
            ),
            Err(_) => (None, None),
        },
        None => (None, None),
    }
}

/// Return the yt-dlp binary name, honouring the `YTDL_BIN` env var (same as doradura-core).
fn ytdlp_bin() -> String {
    doracore::config::YTDL_BIN.clone()
}

/// Parse a yt-dlp stdout/stderr line for a progress update.
/// Delegates to doradura-core's `parse_progress` for consistency.
fn parse_yt_progress(line: &str) -> Option<SlotEvent> {
    let sp = doracore::download::parse_progress(line)?;
    Some(SlotEvent::Progress {
        percent: sp.percent,
        speed_mbs: sp.speed_bytes_sec.unwrap_or(0.0) / 1_048_576.0,
        eta_secs: sp.eta_seconds.unwrap_or(0),
    })
}

/// Extract a file path from known yt-dlp destination announcement lines:
///
/// - `[download] Destination: /path/to/file.ext`
/// - `[ExtractAudio] Destination: /path/to/file.mp3`
/// - `[Merger] Merging formats into "/path/to/file.mp4"`
fn parse_destination(line: &str) -> Option<String> {
    for prefix in &["[download] Destination: ", "[ExtractAudio] Destination: "] {
        if let Some(rest) = line.strip_prefix(prefix) {
            let path = rest.trim();
            if !path.is_empty() {
                return Some(path.to_string());
            }
        }
    }
    // [Merger] Merging formats into "/path/to/file.mp4"
    if let Some(rest) = line.strip_prefix("[Merger] Merging formats into \"") {
        let path = rest.trim_end_matches('"').trim();
        if !path.is_empty() {
            return Some(path.to_string());
        }
    }
    None
}
