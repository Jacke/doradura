//! Background download runner for dora TUI.
//!
//! Spawns yt-dlp as a child process, streams progress events back to the
//! main loop via an mpsc channel. Handles MP3 and MP4 downloads.
//!
//! Uses `doradura-core` for metadata resolution and progress parsing,
//! but calls yt-dlp directly (no Telegram/proxy overhead).

use std::collections::VecDeque;
use std::path::PathBuf;

use anyhow::Context;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;

use crate::app::DownloadFormat;
use crate::settings::DoraSettings;

/// Options for burning subtitles into a video download.
#[derive(Debug, Clone)]
pub struct SubtitleOptions {
    pub lang: String,
}

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
    /// Subtitle burn in progress.
    BurningSubtitles,
    /// Download failed.
    Failed { reason: String },
}

/// Spawn a background download for the given slot ID and return immediately.
///
/// Progress events are sent on `tx` as `(slot_id, SlotEvent)`.
/// Returns an [`AbortHandle`] that can be used to cancel the download task.
#[allow(clippy::too_many_arguments)]
pub fn spawn_download(
    slot_id: usize,
    url: String,
    format: DownloadFormat,
    settings: DoraSettings,
    tx: mpsc::Sender<(usize, SlotEvent)>,
    subtitle_opts: Option<SubtitleOptions>,
) -> tokio::task::AbortHandle {
    tokio::spawn(async move {
        run_download(slot_id, url, format, settings, tx, subtitle_opts).await;
    })
    .abort_handle()
}

// ── Internal implementation ───────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
async fn run_download(
    slot_id: usize,
    url: String,
    format: DownloadFormat,
    settings: DoraSettings,
    tx: mpsc::Sender<(usize, SlotEvent)>,
    subtitle_opts: Option<SubtitleOptions>,
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
        let _ = fs_err::tokio::create_dir_all(&out_dir).await;
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

    // Modern yt-dlp args for YouTube age-restriction / bot detection bypass
    args.push("--extractor-args".to_string());
    args.push("youtube:player_client=android_vr,web_safari;formats=missing_pot".to_string());
    args.push("--js-runtimes".to_string());
    args.push("deno".to_string());

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
        let mut path_str = path.unwrap_or_else(|| out_dir.to_string_lossy().to_string());

        // Burn subtitles if requested and format is MP4
        if let (Some(ref sub_opts), DownloadFormat::Mp4) = (&subtitle_opts, format) {
            match burn_subtitles(slot_id, &url, &path_str, sub_opts, &settings, &tx).await {
                Ok(burned_path) => {
                    path_str = burned_path;
                }
                Err(e) => {
                    log::warn!("[slot {}] subtitle burn failed: {}", slot_id, e);
                    // Continue with the original video (non-fatal)
                }
            }
        }

        let size_mb = fs_err::metadata(&path_str)
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

// ── Subtitle burn pipeline ────────────────────────────────────────────────────

/// Download SRT subtitles via yt-dlp and burn them into the video with ffmpeg.
/// Returns the final output path on success.
#[allow(clippy::too_many_arguments)]
async fn burn_subtitles(
    slot_id: usize,
    url: &str,
    video_path: &str,
    sub_opts: &SubtitleOptions,
    settings: &DoraSettings,
    tx: &mpsc::Sender<(usize, SlotEvent)>,
) -> anyhow::Result<String> {
    // 0. Pre-flight: check that ffmpeg is available
    if tokio::process::Command::new("ffmpeg")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .is_err()
    {
        anyhow::bail!("ffmpeg not found — install it to burn subtitles");
    }

    let _ = tx.send((slot_id, SlotEvent::BurningSubtitles)).await;

    let ytdlp = if settings.ytdlp_bin.trim().is_empty() {
        ytdlp_bin()
    } else {
        settings.ytdlp_bin.clone()
    };

    // Use the video file's stem as the output template for subtitles
    let video = PathBuf::from(video_path);
    let stem = video.file_stem().and_then(|s| s.to_str()).unwrap_or("video");
    let parent = video.parent().unwrap_or_else(|| std::path::Path::new("."));
    let sub_template = parent.join(stem).to_string_lossy().to_string();

    // 1. Download subtitles
    let mut sub_args = vec![
        "--write-subs".to_string(),
        "--write-auto-subs".to_string(),
        "--sub-lang".to_string(),
        sub_opts.lang.clone(),
        "--sub-format".to_string(),
        "srt".to_string(),
        "--convert-subs".to_string(),
        "srt".to_string(),
        "--skip-download".to_string(),
        "--no-check-certificate".to_string(),
        "-o".to_string(),
        sub_template.clone(),
    ];
    if let Some(cf) = settings.cookies_opt() {
        sub_args.push("--cookies".to_string());
        sub_args.push(cf);
    }
    sub_args.push("--extractor-args".to_string());
    sub_args.push("youtube:player_client=android_vr,web_safari;formats=missing_pot".to_string());
    sub_args.push("--js-runtimes".to_string());
    sub_args.push("deno".to_string());
    sub_args.push(url.to_string());

    log::info!(
        "[slot {}] downloading subtitles: {} {}",
        slot_id,
        ytdlp,
        sub_args.join(" ")
    );

    let sub_output = tokio::process::Command::new(&ytdlp)
        .args(&sub_args)
        .output()
        .await
        .context("Cannot run yt-dlp for subtitles")?;

    if !sub_output.status.success() {
        let stderr = String::from_utf8_lossy(&sub_output.stderr);
        log::warn!("[slot {}] subtitle download failed: {}", slot_id, stderr);
        // Extract last meaningful error line for the user
        let reason = stderr
            .lines()
            .rev()
            .find(|l| {
                let t = l.trim();
                !t.is_empty() && !t.starts_with("WARNING")
            })
            .unwrap_or("Subtitle download failed");
        let reason = reason.trim_start_matches("ERROR: ").trim();
        // Cleanup any partial SRT files
        cleanup_srt_files(parent, stem);
        anyhow::bail!("Subtitles: {reason}");
    }

    // 2. Find the SRT file (yt-dlp adds language suffix like `stem.en.srt`)
    let srt_path = match find_srt_file(parent, stem, &sub_opts.lang) {
        Ok(p) => p,
        Err(e) => {
            cleanup_srt_files(parent, stem);
            return Err(anyhow::anyhow!(e));
        }
    };
    log::info!("[slot {}] found SRT: {}", slot_id, srt_path.display());

    // 3. Burn subtitles via ffmpeg
    let burned_ext = video.extension().and_then(|e| e.to_str()).unwrap_or("mp4");
    let burned_path = parent.join(format!("{}_subs.{}", stem, burned_ext));

    // Escape the SRT path for ffmpeg's subtitles filter.
    // ffmpeg filter syntax requires escaping: \ → \\\\ , : → \\: , ' → '\\''
    let srt_escaped = srt_path
        .to_string_lossy()
        .replace('\\', "\\\\\\\\")
        .replace(':', "\\\\:")
        .replace('\'', "'\\\\\\''");

    let ffmpeg_args = vec![
        "-i".to_string(),
        video_path.to_string(),
        "-vf".to_string(),
        format!("subtitles='{}'", srt_escaped),
        "-c:v".to_string(),
        "libx264".to_string(),
        "-c:a".to_string(),
        "copy".to_string(),
        "-preset".to_string(),
        "fast".to_string(),
        "-y".to_string(),
        burned_path.to_string_lossy().to_string(),
    ];

    log::info!("[slot {}] burning subtitles: ffmpeg {}", slot_id, ffmpeg_args.join(" "));

    let ffmpeg_output = tokio::process::Command::new("ffmpeg")
        .args(&ffmpeg_args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .context("Cannot run ffmpeg")?;

    // Always cleanup temp SRT
    let _ = fs_err::tokio::remove_file(&srt_path).await;

    if !ffmpeg_output.status.success() {
        let stderr = String::from_utf8_lossy(&ffmpeg_output.stderr);
        log::warn!("[slot {}] ffmpeg burn failed: {}", slot_id, stderr);
        let _ = fs_err::tokio::remove_file(&burned_path).await;
        anyhow::bail!("ffmpeg subtitle burn failed");
    }

    // 4. Replace original with burned version — SAFE order:
    //    rename burned → original (atomic on same FS), only then no original to lose.
    let final_path = video.to_string_lossy().to_string();
    if let Err(e) = fs_err::tokio::rename(&burned_path, &final_path).await {
        // rename failed — keep both files, user still has the burned version
        log::warn!("[slot {}] rename failed, keeping _subs file: {}", slot_id, e);
        return Ok(burned_path.to_string_lossy().to_string());
    }

    log::info!("[slot {}] subtitle burn complete: {}", slot_id, final_path);
    Ok(final_path)
}

/// Remove any orphaned .srt files matching the stem (best-effort cleanup).
fn cleanup_srt_files(dir: &std::path::Path, stem: &str) {
    if let Ok(entries) = fs_err::read_dir(dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with(stem) && name.ends_with(".srt") {
                let _ = fs_err::remove_file(entry.path());
            }
        }
    }
}

/// Find the SRT file that yt-dlp created.
/// Tries exact lang match first, then lang prefix (e.g. "en" matches "en-US"),
/// then any .srt matching the stem as last resort.
fn find_srt_file(dir: &std::path::Path, stem: &str, lang: &str) -> anyhow::Result<PathBuf> {
    // Try exact match: stem.lang.srt
    let exact = dir.join(format!("{}.{}.srt", stem, lang));
    if exact.exists() {
        return Ok(exact);
    }

    // Collect all SRT files matching the stem
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = fs_err::read_dir(dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with(stem) && name_str.ends_with(".srt") {
                candidates.push(entry.path());
            }
        }
    }

    if candidates.is_empty() {
        anyhow::bail!("No SRT file found for {}.{}.srt", stem, lang);
    }

    // Prefer files whose lang part starts with the requested lang
    // e.g. requesting "en" should prefer "stem.en-US.srt" over "stem.fr.srt"
    let prefix = format!("{}.{}", stem, lang);
    if let Some(best) = candidates.iter().find(|p| {
        p.file_name()
            .map(|n| n.to_string_lossy().starts_with(&prefix))
            .unwrap_or(false)
    }) {
        return Ok(best.clone());
    }

    // Last resort: first available SRT. `candidates` is non-empty here —
    // the `if candidates.is_empty()` bail above guarantees it.
    Ok(candidates
        .into_iter()
        .next()
        .expect("candidates non-empty — checked above"))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_destination tests ──────────────────────────────────────────────

    #[test]
    fn parse_destination_download() {
        let line = "[download] Destination: /tmp/video.mp4";
        assert_eq!(parse_destination(line), Some("/tmp/video.mp4".to_string()));
    }

    #[test]
    fn parse_destination_extract_audio() {
        let line = "[ExtractAudio] Destination: /home/user/song.mp3";
        assert_eq!(parse_destination(line), Some("/home/user/song.mp3".to_string()));
    }

    #[test]
    fn parse_destination_merger() {
        let line = "[Merger] Merging formats into \"/tmp/video.mp4\"";
        assert_eq!(parse_destination(line), Some("/tmp/video.mp4".to_string()));
    }

    #[test]
    fn parse_destination_unrelated_line() {
        assert_eq!(parse_destination("[download] 50.0% of 10MiB"), None);
        assert_eq!(parse_destination("WARNING: something"), None);
        assert_eq!(parse_destination(""), None);
    }

    // ── find_srt_file tests ─────────────────────────────────────────────────

    #[test]
    fn find_srt_exact_match() {
        let dir = tempfile::tempdir().unwrap();
        let srt = dir.path().join("video.en.srt");
        fs_err::write(&srt, "1\n00:00:00,000 --> 00:00:01,000\nHello").unwrap();

        let result = find_srt_file(dir.path(), "video", "en");
        assert_eq!(result.unwrap(), srt);
    }

    #[test]
    fn find_srt_fallback_any_lang() {
        let dir = tempfile::tempdir().unwrap();
        // yt-dlp sometimes uses different lang suffix (e.g. en-US instead of en)
        let srt = dir.path().join("video.en-US.srt");
        fs_err::write(&srt, "1\n00:00:00,000 --> 00:00:01,000\nHello").unwrap();

        let result = find_srt_file(dir.path(), "video", "en");
        assert_eq!(result.unwrap(), srt);
    }

    #[test]
    fn find_srt_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let result = find_srt_file(dir.path(), "video", "en");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("No SRT file found"));
    }

    #[test]
    fn find_srt_ignores_non_matching_stem() {
        let dir = tempfile::tempdir().unwrap();
        // SRT for a different video
        let srt = dir.path().join("other_video.en.srt");
        fs_err::write(&srt, "subtitle content").unwrap();

        let result = find_srt_file(dir.path(), "video", "en");
        assert!(result.is_err());
    }

    #[test]
    fn find_srt_prefers_lang_prefix_over_random() {
        let dir = tempfile::tempdir().unwrap();
        // Two SRT files: one for "en", one for "fr"
        let en_srt = dir.path().join("video.en-orig.srt");
        let fr_srt = dir.path().join("video.fr.srt");
        fs_err::write(&en_srt, "english").unwrap();
        fs_err::write(&fr_srt, "french").unwrap();

        // Requesting "en" should pick en-orig, not fr
        let result = find_srt_file(dir.path(), "video", "en").unwrap();
        assert!(
            result.to_string_lossy().contains(".en"),
            "expected en SRT, got: {}",
            result.display()
        );
    }

    #[test]
    fn find_srt_with_spaces_in_name() {
        let dir = tempfile::tempdir().unwrap();
        let srt = dir.path().join("my video title.ru.srt");
        fs_err::write(&srt, "subtitles").unwrap();

        let result = find_srt_file(dir.path(), "my video title", "ru");
        assert_eq!(result.unwrap(), srt);
    }

    // ── cleanup_srt_files tests ──────────────────────────────────────────────

    #[test]
    fn cleanup_srt_removes_matching_files() {
        let dir = tempfile::tempdir().unwrap();
        let srt1 = dir.path().join("video.en.srt");
        let srt2 = dir.path().join("video.ru.srt");
        let keep = dir.path().join("other.en.srt");
        fs_err::write(&srt1, "a").unwrap();
        fs_err::write(&srt2, "b").unwrap();
        fs_err::write(&keep, "c").unwrap();

        cleanup_srt_files(dir.path(), "video");

        assert!(!srt1.exists());
        assert!(!srt2.exists());
        assert!(keep.exists(), "should not remove SRT for a different stem");
    }

    // ── SubtitleOptions tests ────────────────────────────────────────────────

    #[test]
    fn subtitle_options_clone() {
        let opts = SubtitleOptions { lang: "ru".to_string() };
        let cloned = opts.clone();
        assert_eq!(cloned.lang, "ru");
    }
}
