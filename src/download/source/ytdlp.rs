//! YtDlpSource — one of several download backends, powered by yt-dlp.
//!
//! Handles 1000+ sites (YouTube, SoundCloud, Vimeo, TikTok, Instagram, etc.)
//! via the yt-dlp extractor ecosystem. Implements the full v5.0 fallback chain:
//!   Tier 1 (no cookies) → Tier 2 (cookies + PO token) → Tier 3 (fixup never)
//! with proxy chain failover at each tier.
//!
//! This is a pluggable backend — see `DownloadSource` trait in `source/mod.rs`
//! for the interface that all backends implement.

use crate::core::config;
use crate::core::error::AppError;
use crate::download::cookies::report_and_wait_for_refresh;
use crate::download::downloader::{cleanup_partial_download, parse_progress};
use crate::download::error::DownloadError;
use crate::download::metadata::{
    add_cookies_args_with_proxy, add_no_cookies_args, build_telegram_safe_format, find_actual_downloaded_file,
    get_estimated_filesize, get_metadata_from_ytdlp, get_proxy_chain, is_proxy_related_error, probe_duration_seconds,
};
use crate::download::source::{DownloadOutput, DownloadRequest, DownloadSource, SourceProgress};
use crate::download::ytdlp_errors::{analyze_ytdlp_error, get_error_message, YtDlpErrorType};
use async_trait::async_trait;
use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use url::Url;

/// Known domains handled by yt-dlp (non-exhaustive, used for `supports_url`).
const YTDLP_DOMAINS: &[&str] = &[
    "youtube.com",
    "youtu.be",
    "music.youtube.com",
    "soundcloud.com",
    "vimeo.com",
    "tiktok.com",
    "instagram.com",
    "twitter.com",
    "x.com",
    "facebook.com",
    "twitch.tv",
    "dailymotion.com",
    "bandcamp.com",
    "reddit.com",
    "bilibili.com",
    "nicovideo.jp",
    "rutube.ru",
    "ok.ru",
    "vk.com",
    "clips.twitch.tv",
];

/// Download source powered by yt-dlp for extracting media from supported sites.
pub struct YtDlpSource;

impl Default for YtDlpSource {
    fn default() -> Self {
        Self::new()
    }
}

impl YtDlpSource {
    pub fn new() -> Self {
        Self
    }

    /// Check if a domain matches any known yt-dlp domain.
    fn is_known_domain(url: &Url) -> bool {
        if let Some(host) = url.host_str() {
            let host_lower = host.to_lowercase();
            YTDLP_DOMAINS
                .iter()
                .any(|d| host_lower == *d || host_lower.ends_with(&format!(".{}", d)))
        } else {
            false
        }
    }
}

#[async_trait]
impl DownloadSource for YtDlpSource {
    fn name(&self) -> &str {
        "yt-dlp"
    }

    fn supports_url(&self, url: &Url) -> bool {
        // Support known domains, plus any non-direct-file URL as a fallback
        // (yt-dlp supports 1000+ sites)
        if Self::is_known_domain(url) {
            return true;
        }
        // If it's http(s) but doesn't look like a direct file link, yt-dlp might handle it
        let scheme = url.scheme();
        if scheme != "http" && scheme != "https" {
            return false;
        }
        let path = url.path().to_lowercase();
        // Reject obvious direct file URLs (handled by HttpSource)
        !matches!(
            path.rsplit('.').next(),
            Some("mp3" | "mp4" | "wav" | "flac" | "ogg" | "m4a" | "webm" | "avi" | "mkv" | "zip" | "rar" | "pdf")
        )
    }

    async fn get_metadata(&self, url: &Url) -> Result<crate::download::source::MediaMetadata, AppError> {
        let (title, artist) = get_metadata_from_ytdlp(None, None, url).await?;
        Ok(crate::download::source::MediaMetadata { title, artist })
    }

    async fn estimate_size(&self, url: &Url) -> Option<u64> {
        get_estimated_filesize(url).await
    }

    async fn is_livestream(&self, url: &Url) -> bool {
        crate::download::metadata::is_livestream(url).await
    }

    async fn download(
        &self,
        request: &DownloadRequest,
        progress_tx: mpsc::UnboundedSender<SourceProgress>,
    ) -> Result<DownloadOutput, AppError> {
        let is_audio = request.format == "mp3";

        if is_audio {
            self.download_audio(request, progress_tx).await
        } else {
            self.download_video(request, progress_tx).await
        }
    }
}

impl YtDlpSource {
    /// Download audio using yt-dlp with the full fallback chain.
    async fn download_audio(
        &self,
        request: &DownloadRequest,
        progress_tx: mpsc::UnboundedSender<SourceProgress>,
    ) -> Result<DownloadOutput, AppError> {
        let ytdl_bin = config::YTDL_BIN.clone();
        let url_str = request.url.to_string();
        let download_path = request.output_path.clone();
        let bitrate_str = request.audio_bitrate.clone().unwrap_or_else(|| "320k".to_string());
        let time_range = request.time_range.clone();

        let handle = tokio::task::spawn_blocking(move || {
            let postprocessor_args = format!("ffmpeg:-acodec libmp3lame -b:a {}", bitrate_str);

            download_with_fallback_chain(
                &ytdl_bin,
                &url_str,
                &download_path,
                &progress_tx,
                "audio",
                |args, proxy_option| {
                    // Audio-specific yt-dlp args
                    args.extend_from_slice(&[
                        "--extract-audio",
                        "--audio-format",
                        "mp3",
                        "--audio-quality",
                        "0",
                        "--add-metadata",
                        "--embed-thumbnail",
                    ]);
                    // Tier 1: no cookies
                    add_no_cookies_args(args, proxy_option);
                    // Extractor + runtime
                    args.push("--extractor-args");
                    args.push("youtube:player_client=default;formats=missing_pot");
                    args.push("--js-runtimes");
                    args.push("deno");
                    args.extend_from_slice(&["--no-check-certificate", "--postprocessor-args"]);
                    // NOTE: postprocessor_args is borrowed from the outer closure
                },
                |args, proxy_option| {
                    // Tier 2 (cookies): audio-specific args
                    args.extend_from_slice(&[
                        "--extract-audio",
                        "--audio-format",
                        "mp3",
                        "--audio-quality",
                        "0",
                        "--add-metadata",
                        "--embed-thumbnail",
                    ]);
                    add_cookies_args_with_proxy(args, proxy_option);
                    args.push("--extractor-args");
                    args.push("youtube:player_client=default");
                    args.push("--js-runtimes");
                    args.push("deno");
                    args.push("--no-check-certificate");
                    args.push("--postprocessor-args");
                },
                |args, proxy_option| {
                    // Tier 3 (fixup never): audio-specific args
                    args.extend_from_slice(&[
                        "--fixup",
                        "never",
                        "--extract-audio",
                        "--audio-format",
                        "mp3",
                        "--audio-quality",
                        "0",
                        "--add-metadata",
                    ]);
                    add_cookies_args_with_proxy(args, proxy_option);
                    args.push("--extractor-args");
                    args.push("youtube:player_client=default");
                    args.push("--js-runtimes");
                    args.push("deno");
                    args.push("--no-check-certificate");
                },
                &postprocessor_args,
                time_range.as_ref(),
            )
        });

        let duration = handle
            .await
            .map_err(|e| AppError::Download(DownloadError::YtDlp(format!("Task join error: {}", e))))??;

        let actual_path =
            find_actual_downloaded_file(&request.output_path).unwrap_or_else(|_| request.output_path.clone());

        let file_size = std::fs::metadata(&actual_path).map(|m| m.len()).unwrap_or(0);

        Ok(DownloadOutput {
            file_path: actual_path,
            duration_secs: duration,
            file_size,
            mime_hint: Some("audio/mpeg".to_string()),
        })
    }

    /// Download video using yt-dlp with the full fallback chain.
    async fn download_video(
        &self,
        request: &DownloadRequest,
        progress_tx: mpsc::UnboundedSender<SourceProgress>,
    ) -> Result<DownloadOutput, AppError> {
        let ytdl_bin = config::YTDL_BIN.clone();
        let url_str = request.url.to_string();
        let download_path = request.output_path.clone();
        let time_range = request.time_range.clone();

        let format_arg = match request.video_quality.as_deref() {
            Some("1080p") => build_telegram_safe_format(Some(1080)),
            Some("720p") => build_telegram_safe_format(Some(720)),
            Some("480p") => build_telegram_safe_format(Some(480)),
            Some("360p") => build_telegram_safe_format(Some(360)),
            _ => build_telegram_safe_format(None),
        };

        let handle = tokio::task::spawn_blocking(move || {
            download_with_fallback_chain(
                &ytdl_bin,
                &url_str,
                &download_path,
                &progress_tx,
                "video",
                |args, proxy_option| {
                    // Video-specific yt-dlp args (Tier 1: no cookies)
                    args.push("--format");
                    // SAFETY: format_arg is captured by the closure, we push a &str pointing into it
                    // This is valid because format_arg lives for the entire closure scope.
                    args.push("--merge-output-format");
                    args.push("mp4");
                    args.push("--postprocessor-args");
                    args.push("Merger:-movflags +faststart");
                    add_no_cookies_args(args, proxy_option);
                    args.push("--extractor-args");
                    args.push("youtube:player_client=default;formats=missing_pot");
                    args.push("--js-runtimes");
                    args.push("deno");
                    args.push("--no-check-certificate");
                },
                |args, proxy_option| {
                    // Tier 2 (cookies): video-specific args
                    args.push("--format");
                    args.push("--merge-output-format");
                    args.push("mp4");
                    args.push("--postprocessor-args");
                    args.push("Merger:-movflags +faststart");
                    add_cookies_args_with_proxy(args, proxy_option);
                    args.push("--extractor-args");
                    args.push("youtube:player_client=default");
                    args.push("--js-runtimes");
                    args.push("deno");
                    args.push("--no-check-certificate");
                },
                |args, proxy_option| {
                    // Tier 3 (fixup never): video-specific args
                    args.push("--fixup");
                    args.push("never");
                    args.push("--format");
                    args.push("--merge-output-format");
                    args.push("mp4");
                    add_cookies_args_with_proxy(args, proxy_option);
                    args.push("--extractor-args");
                    args.push("youtube:player_client=default");
                    args.push("--js-runtimes");
                    args.push("deno");
                    args.push("--no-check-certificate");
                },
                &format_arg,
                time_range.as_ref(),
            )
        });

        handle
            .await
            .map_err(|e| AppError::Download(DownloadError::YtDlp(format!("Task join error: {}", e))))??;

        let actual_path = find_actual_downloaded_file(&request.output_path)?;

        let file_size = std::fs::metadata(&actual_path).map(|m| m.len()).unwrap_or(0);

        let duration = probe_duration_seconds(&actual_path);

        Ok(DownloadOutput {
            file_path: actual_path,
            duration_secs: duration,
            file_size,
            mime_hint: Some("video/mp4".to_string()),
        })
    }
}

/// Result from Tier 2 (cookies) attempt, signaling whether the outer proxy loop should retry.
enum Tier2Outcome {
    /// Download succeeded
    Success,
    /// Cookies were refreshed; the outer loop should `continue` to retry from Tier 1
    CookieRefreshed,
    /// Tier 2 failed (non-recoverable at this proxy level)
    Failed,
}

/// Try Tier 1 (no cookies) download with progress reporting.
///
/// Builds args via `tier1_args_fn`, runs yt-dlp with live progress, returns
/// `Ok(())` on success or `Err((error_type, stderr))` on failure.
fn try_tier1<F>(
    ytdl_bin: &str,
    download_path: &str,
    url_str: &str,
    media_type: &str,
    extra_arg: &str,
    section_spec: Option<&str>,
    proxy_option: Option<&crate::download::metadata::ProxyConfig>,
    progress_tx: &mpsc::UnboundedSender<SourceProgress>,
    tier1_args_fn: &F,
) -> Result<(), (YtDlpErrorType, String)>
where
    F: Fn(&mut Vec<&str>, Option<&crate::download::metadata::ProxyConfig>),
{
    let mut args: Vec<&str> = build_common_args(download_path);
    tier1_args_fn(&mut args, proxy_option);

    if media_type == "audio" {
        args.push(extra_arg);
    } else if let Some(pos) = args.iter().position(|a| *a == "--format") {
        args.insert(pos + 1, extra_arg);
    }
    append_section_args(&mut args, section_spec);
    args.push(url_str);

    log::debug!(
        "yt-dlp command for {} download: {} {}",
        media_type,
        ytdl_bin,
        args.join(" ")
    );
    run_ytdlp_with_progress(ytdl_bin, &args, progress_tx)
}

/// Try Tier 2 (cookies + PO token) download with progress reporting.
///
/// Returns `Tier2Outcome` to signal the outer loop:
/// - `Success` — download completed
/// - `CookieRefreshed` — cookies were refreshed, caller should retry from Tier 1
/// - `Failed` — Tier 2 failed, continue to next tier/proxy
fn try_tier2<F>(
    ytdl_bin: &str,
    download_path: &str,
    url_str: &str,
    media_type: &str,
    extra_arg: &str,
    section_spec: Option<&str>,
    proxy_option: Option<&crate::download::metadata::ProxyConfig>,
    progress_tx: &mpsc::UnboundedSender<SourceProgress>,
    tier2_args_fn: &F,
    runtime_handle: &tokio::runtime::Handle,
) -> Tier2Outcome
where
    F: Fn(&mut Vec<&str>, Option<&crate::download::metadata::ProxyConfig>),
{
    crate::download::cookies::log_cookie_file_diagnostics(&format!("{}_TIER2_BEFORE", media_type.to_uppercase()));

    let _ = std::fs::remove_file(download_path);
    cleanup_partial_download(download_path);

    let mut cookies_args: Vec<&str> = build_common_args_minimal(download_path);
    tier2_args_fn(&mut cookies_args, proxy_option);
    if media_type == "audio" {
        cookies_args.push(extra_arg);
    } else if let Some(pos) = cookies_args.iter().position(|a| *a == "--format") {
        cookies_args.insert(pos + 1, extra_arg);
    }
    append_section_args(&mut cookies_args, section_spec);
    cookies_args.push(url_str);

    log::info!(
        "🔑 [WITH_COOKIES] Attempting {} download WITH cookies + PO Token...",
        media_type
    );

    match run_ytdlp_with_progress(ytdl_bin, &cookies_args, progress_tx) {
        Ok(()) => {
            log::info!("✅ [WITH_COOKIES] {} download succeeded!", media_type);
            return Tier2Outcome::Success;
        }
        Err((cookies_error_type, _cookies_stderr)) => {
            log::error!(
                "❌ [TIER2_FAILED] {} with-cookies failed: error={:?}",
                media_type,
                cookies_error_type
            );

            crate::download::cookies::log_cookie_file_diagnostics(&format!(
                "{}_TIER2_AFTER_FAIL",
                media_type.to_uppercase()
            ));

            if matches!(cookies_error_type, YtDlpErrorType::InvalidCookies) {
                log::warn!("🍪 [COOKIE_INVALID] Requesting async cookie refresh...");
                let url_for_report = url_str.to_string();
                let (tx, rx) = std::sync::mpsc::channel();
                runtime_handle.spawn(async move {
                    let result = report_and_wait_for_refresh("InvalidCookies", &url_for_report).await;
                    let _ = tx.send(result);
                });
                let should_retry = rx.recv_timeout(std::time::Duration::from_secs(20)).unwrap_or(false);
                if should_retry {
                    log::info!("🔄 Cookie refresh successful, will retry");
                    std::thread::sleep(std::time::Duration::from_secs(3));
                    return Tier2Outcome::CookieRefreshed;
                }
            } else if matches!(cookies_error_type, YtDlpErrorType::BotDetection) {
                log::error!("🤖 [BOT_DETECTED] Tier 2 bot detection WITH cookies.");
                crate::download::cookies::log_cookie_file_diagnostics("BOT_DETECTED_WITH_COOKIES");
            }
        }
    }

    log::error!(
        "💀 [BOTH_TIERS_FAILED] Both Tier 1 and Tier 2 failed for {}",
        media_type
    );
    Tier2Outcome::Failed
}

/// Try Tier 3 (--fixup never) download with progress reporting.
///
/// Returns `true` if the download succeeded.
fn try_tier3<F>(
    ytdl_bin: &str,
    download_path: &str,
    url_str: &str,
    media_type: &str,
    extra_arg: &str,
    section_spec: Option<&str>,
    proxy_option: Option<&crate::download::metadata::ProxyConfig>,
    progress_tx: &mpsc::UnboundedSender<SourceProgress>,
    tier3_args_fn: &F,
) -> bool
where
    F: Fn(&mut Vec<&str>, Option<&crate::download::metadata::ProxyConfig>),
{
    log::warn!("🔧 Postprocessing error, retrying with --fixup never...");

    let _ = std::fs::remove_file(download_path);
    cleanup_partial_download(download_path);

    let mut fixup_args: Vec<&str> = build_common_args_minimal(download_path);
    tier3_args_fn(&mut fixup_args, proxy_option);
    if media_type == "video" {
        if let Some(pos) = fixup_args.iter().position(|a| *a == "--format") {
            fixup_args.insert(pos + 1, extra_arg);
        }
    }
    append_section_args(&mut fixup_args, section_spec);
    fixup_args.push(url_str);

    log::info!(
        "🔧 [FIXUP_NEVER] Attempting {} download without postprocessing...",
        media_type
    );

    match run_ytdlp_with_progress(ytdl_bin, &fixup_args, progress_tx) {
        Ok(()) => {
            log::info!("✅ [FIXUP_NEVER] {} download succeeded!", media_type);
            true
        }
        Err((_error_type, stderr_text)) => {
            log::warn!(
                "❌ [FIXUP_NEVER] Failed: {}",
                &stderr_text[..std::cmp::min(500, stderr_text.len())]
            );
            false
        }
    }
}

/// Core download logic with the v5.0 three-tier fallback chain and proxy failover.
///
/// Orchestrates three tiers per proxy:
///   - Tier 1: No cookies (yt-dlp 2026+ modern mode) — with progress
///   - Tier 2: With cookies + PO token (full authentication) — with progress
///   - Tier 3: --fixup never (skip postprocessing on ffmpeg errors) — with progress
///
/// The `tier1_args_fn`, `tier2_args_fn`, and `tier3_args_fn` closures add
/// format-specific arguments (audio vs video) for each tier.
fn download_with_fallback_chain<F1, F2, F3>(
    ytdl_bin: &str,
    url_str: &str,
    download_path: &str,
    progress_tx: &mpsc::UnboundedSender<SourceProgress>,
    media_type: &str,
    tier1_args_fn: F1,
    tier2_args_fn: F2,
    tier3_args_fn: F3,
    extra_arg: &str,
    time_range: Option<&(String, String)>,
) -> Result<Option<u32>, AppError>
where
    F1: Fn(&mut Vec<&str>, Option<&crate::download::metadata::ProxyConfig>),
    F2: Fn(&mut Vec<&str>, Option<&crate::download::metadata::ProxyConfig>),
    F3: Fn(&mut Vec<&str>, Option<&crate::download::metadata::ProxyConfig>),
{
    let runtime_handle = tokio::runtime::Handle::current();
    let proxy_chain = get_proxy_chain();
    let total_proxies = proxy_chain.len();
    let mut last_error: Option<AppError> = None;
    let section_spec = time_range.map(|(start, end)| format!("*{}-{}", start, end));

    for (attempt, proxy_option) in proxy_chain.into_iter().enumerate() {
        let proxy_name = proxy_option
            .as_ref()
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "Direct (no proxy)".to_string());

        log::info!(
            "📡 {} download attempt {}/{} using [{}]",
            media_type,
            attempt + 1,
            total_proxies,
            proxy_name
        );

        if attempt > 0 {
            let _ = std::fs::remove_file(download_path);
            cleanup_partial_download(download_path);
        }

        // ── Tier 1: No cookies ──
        match try_tier1(
            ytdl_bin,
            download_path,
            url_str,
            media_type,
            extra_arg,
            section_spec.as_deref(),
            proxy_option.as_ref(),
            progress_tx,
            &tier1_args_fn,
        ) {
            Ok(()) => {
                log::info!(
                    "✅ {} download succeeded using [{}] (attempt {}/{})",
                    media_type,
                    proxy_name,
                    attempt + 1,
                    total_proxies
                );
                return Ok(probe_duration_seconds(download_path));
            }
            Err((error_type, stderr_text)) => {
                let error_msg = get_error_message(&error_type);
                log::error!(
                    "❌ Download failed with [{}]: {:?} - {}",
                    proxy_name,
                    error_type,
                    &stderr_text[..std::cmp::min(500, stderr_text.len())]
                );

                // Network-only errors: skip Tier 2/3, try next proxy
                let is_network_only = matches!(error_type, YtDlpErrorType::NetworkError)
                    || (is_proxy_related_error(&stderr_text) && !matches!(error_type, YtDlpErrorType::BotDetection));

                if is_network_only && attempt + 1 < total_proxies {
                    log::warn!(
                        "🔄 Network/proxy error, trying next proxy (attempt {}/{})",
                        attempt + 2,
                        total_proxies
                    );
                    last_error = Some(AppError::Download(DownloadError::YtDlp(error_msg)));
                    continue;
                }

                // ── Tier 2: With cookies + PO Token ──
                let should_try_tier2 = matches!(
                    error_type,
                    YtDlpErrorType::InvalidCookies | YtDlpErrorType::BotDetection | YtDlpErrorType::NetworkError
                );
                if should_try_tier2 {
                    log::warn!(
                        "🍪 [TIER1→TIER2] No-cookies mode failed (error={:?}), trying WITH cookies...",
                        error_type
                    );

                    match try_tier2(
                        ytdl_bin,
                        download_path,
                        url_str,
                        media_type,
                        extra_arg,
                        section_spec.as_deref(),
                        proxy_option.as_ref(),
                        progress_tx,
                        &tier2_args_fn,
                        &runtime_handle,
                    ) {
                        Tier2Outcome::Success => return Ok(probe_duration_seconds(download_path)),
                        Tier2Outcome::CookieRefreshed => {
                            last_error = Some(AppError::Download(DownloadError::YtDlp(error_msg.clone())));
                            continue;
                        }
                        Tier2Outcome::Failed => {}
                    }
                }

                // ── Tier 3: Fixup never (postprocessing errors) ──
                if error_type == YtDlpErrorType::PostprocessingError
                    && try_tier3(
                        ytdl_bin,
                        download_path,
                        url_str,
                        media_type,
                        extra_arg,
                        section_spec.as_deref(),
                        proxy_option.as_ref(),
                        progress_tx,
                        &tier3_args_fn,
                    )
                {
                    return Ok(probe_duration_seconds(download_path));
                }

                // Record metrics
                let error_category = match error_type {
                    YtDlpErrorType::InvalidCookies => "invalid_cookies",
                    YtDlpErrorType::BotDetection => "bot_detection",
                    YtDlpErrorType::VideoUnavailable => "video_unavailable",
                    YtDlpErrorType::NetworkError => "network",
                    YtDlpErrorType::FragmentError => "fragment_error",
                    YtDlpErrorType::PostprocessingError => "postprocessing_error",
                    YtDlpErrorType::DiskSpaceError => "disk_space_error",
                    YtDlpErrorType::Unknown => "ytdlp_unknown",
                };
                crate::core::metrics::record_error("download", &format!("{}_download:{}", media_type, error_category));

                if attempt + 1 < total_proxies {
                    log::warn!(
                        "🔄 All tiers failed with [{}], trying next proxy (attempt {}/{})",
                        proxy_name,
                        attempt + 2,
                        total_proxies
                    );
                    last_error = Some(AppError::Download(DownloadError::YtDlp(error_msg)));
                    continue;
                }

                return Err(AppError::Download(DownloadError::YtDlp(error_msg)));
            }
        }
    }

    log::error!("❌ All {} proxies failed for {} download", total_proxies, media_type);
    Err(last_error.unwrap_or_else(|| AppError::Download(DownloadError::YtDlp("All proxies failed".to_string()))))
}

/// Append `--download-sections` and `--force-keyframes-at-cuts` when a time range is set.
fn append_section_args<'a>(args: &mut Vec<&'a str>, section_spec: Option<&'a str>) {
    if let Some(spec) = section_spec {
        args.push("--download-sections");
        args.push(spec);
        args.push("--force-keyframes-at-cuts");
    }
}

/// Build common yt-dlp arguments shared by all tiers (full set with rate limiting).
fn build_common_args(download_path: &str) -> Vec<&str> {
    vec![
        "-o",
        download_path,
        "--newline",
        "--force-overwrites",
        "--no-playlist",
        "--concurrent-fragments",
        "1",
        "--fragment-retries",
        "10",
        "--socket-timeout",
        "30",
        "--http-chunk-size",
        "2097152",
        "--sleep-requests",
        "2",
        "--sleep-interval",
        "3",
        "--max-sleep-interval",
        "10",
        "--limit-rate",
        "5M",
        "--retry-sleep",
        "http:exp=1:30",
        "--retry-sleep",
        "fragment:exp=1:30",
        "--retries",
        "15",
    ]
}

/// Build minimal common arguments (for Tier 2 and Tier 3 fallbacks).
fn build_common_args_minimal(download_path: &str) -> Vec<&str> {
    vec![
        "-o",
        download_path,
        "--newline",
        "--force-overwrites",
        "--no-playlist",
        "--concurrent-fragments",
        "1",
        "--fragment-retries",
        "10",
        "--socket-timeout",
        "30",
        "--http-chunk-size",
        "2097152",
    ]
}

/// Run yt-dlp with stdout/stderr capture and progress reporting.
///
/// Returns Ok(()) on success, or Err((YtDlpErrorType, stderr_text)) on failure.
fn run_ytdlp_with_progress(
    ytdl_bin: &str,
    args: &[&str],
    progress_tx: &mpsc::UnboundedSender<SourceProgress>,
) -> Result<(), (YtDlpErrorType, String)> {
    let child_result = Command::new(ytdl_bin)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();

    let mut child = match child_result {
        Ok(c) => c,
        Err(e) => {
            log::error!("Failed to spawn yt-dlp: {}", e);
            return Err((YtDlpErrorType::Unknown, format!("Failed to spawn yt-dlp: {}", e)));
        }
    };

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let stderr_lines = Arc::new(std::sync::Mutex::new(VecDeque::<String>::new()));
    let stderr_lines_clone = Arc::clone(&stderr_lines);

    let tx_clone = progress_tx.clone();

    // Read stderr in a separate thread
    if let Some(stderr_stream) = stderr {
        std::thread::spawn(move || {
            let reader = BufReader::new(stderr_stream);
            for line in reader.lines() {
                if let Ok(line_str) = line {
                    log::debug!("yt-dlp stderr: {}", line_str);
                    if let Ok(mut lines) = stderr_lines_clone.lock() {
                        lines.push_back(line_str.clone());
                        if lines.len() > 200 {
                            lines.pop_front();
                        }
                    }
                    if let Some(sp) = parse_progress(&line_str) {
                        let _ = tx_clone.send(sp);
                    }
                }
            }
        });
    }

    // Read stdout on the current thread
    if let Some(stdout_stream) = stdout {
        let reader = BufReader::new(stdout_stream);
        for line in reader.lines() {
            if let Ok(line_str) = line {
                log::debug!("yt-dlp stdout: {}", line_str);
                if let Some(sp) = parse_progress(&line_str) {
                    let _ = progress_tx.send(sp);
                }
            }
        }
    }

    // Wait for the process with a timeout
    let ytdlp_timeout = config::download::ytdlp_timeout();
    let deadline = std::time::Instant::now() + ytdlp_timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    log::error!("yt-dlp process timed out after {}s, killing", ytdlp_timeout.as_secs());
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err((
                        YtDlpErrorType::Unknown,
                        format!("yt-dlp process timed out after {}s", ytdlp_timeout.as_secs()),
                    ));
                }
                // Poll interval inside spawn_blocking — std::thread::sleep is correct here
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(e) => {
                log::error!("Downloader process failed: {}", e);
                return Err((YtDlpErrorType::Unknown, format!("downloader process failed: {}", e)));
            }
        }
    };

    if status.success() {
        return Ok(());
    }

    let stderr_text = if let Ok(mut lines) = stderr_lines.lock() {
        lines.make_contiguous().join("\n")
    } else {
        String::new()
    };

    let error_type = analyze_ytdlp_error(&stderr_text);
    Err((error_type, stderr_text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_supports_url_youtube() {
        let source = YtDlpSource::new();
        let url = Url::parse("https://www.youtube.com/watch?v=abc123").unwrap();
        assert!(source.supports_url(&url));
    }

    #[test]
    fn test_supports_url_youtu_be() {
        let source = YtDlpSource::new();
        let url = Url::parse("https://youtu.be/abc123").unwrap();
        assert!(source.supports_url(&url));
    }

    #[test]
    fn test_supports_url_soundcloud() {
        let source = YtDlpSource::new();
        let url = Url::parse("https://soundcloud.com/artist/track").unwrap();
        assert!(source.supports_url(&url));
    }

    #[test]
    fn test_supports_url_direct_file_rejected() {
        let source = YtDlpSource::new();
        let url = Url::parse("https://example.com/file.mp3").unwrap();
        assert!(!source.supports_url(&url));
    }

    #[test]
    fn test_supports_url_tiktok() {
        let source = YtDlpSource::new();
        let url = Url::parse("https://www.tiktok.com/@user/video/123").unwrap();
        assert!(source.supports_url(&url));
    }

    #[test]
    fn test_is_known_domain() {
        assert!(YtDlpSource::is_known_domain(
            &Url::parse("https://www.youtube.com/watch?v=x").unwrap()
        ));
        assert!(YtDlpSource::is_known_domain(
            &Url::parse("https://music.youtube.com/watch?v=x").unwrap()
        ));
        assert!(!YtDlpSource::is_known_domain(
            &Url::parse("https://example.com/page").unwrap()
        ));
    }

    #[test]
    fn test_append_section_args_with_range() {
        let mut args = vec!["-o", "/tmp/test.mp4"];
        append_section_args(&mut args, Some("*00:01:00-00:02:30"));
        assert_eq!(
            args,
            vec![
                "-o",
                "/tmp/test.mp4",
                "--download-sections",
                "*00:01:00-00:02:30",
                "--force-keyframes-at-cuts"
            ]
        );
    }

    #[test]
    fn test_append_section_args_without_range() {
        let mut args = vec!["-o", "/tmp/test.mp4"];
        append_section_args(&mut args, None);
        assert_eq!(args, vec!["-o", "/tmp/test.mp4"]);
    }
}
