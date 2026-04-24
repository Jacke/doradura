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
    add_cookies_args_with_proxy, add_instagram_cookies_args_with_proxy, add_no_cookies_args, build_highres_format,
    build_telegram_safe_format, default_pot_token, default_youtube_extractor_args, find_actual_downloaded_file,
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

/// Convert `concurrent_fragments` to a static string for yt-dlp's `-N` flag.
/// Returns `""` (no-op) for unsupported values including 1 (the default).
fn concurrent_fragments_str(n: u8) -> &'static str {
    match n {
        2 => "2",
        3 => "3",
        4 => "4",
        8 => "8",
        16 => "16",
        _ => "",
    }
}

/// Push `-N <n>` to the arg list when concurrent fragments are enabled.
fn push_concurrent_fragments_arg<'a>(args: &mut Vec<&'a str>, cf_str: &'a str) {
    if !cf_str.is_empty() {
        args.push("-N");
        args.push(cf_str);
    }
}

/// Push the common "runtime / cert / concurrent-fragments" tail that every
/// tier (audio + video, Tier 1/2/3) shares verbatim.
///
/// ⚠️ Order is load-bearing — see CLAUDE.md. The
/// [`push_js_runtimes_tail_has_expected_shape`] test in the
/// `common_args_tests` submodule pins the exact slice.
fn push_js_runtimes_tail<'a>(args: &mut Vec<&'a str>, cf_str: &'a str) {
    args.push("--js-runtimes");
    args.push("deno");
    args.push("--no-check-certificate");
    push_concurrent_fragments_arg(args, cf_str);
}

/// Push the audio-specific prefix args. `with_thumbnail` controls whether
/// `--embed-thumbnail` is included (true for Tier 1/2, false for Tier 3
/// which also sets `--fixup never` separately).
///
/// All pushed strings are `'static`, so this composes with any caller that
/// holds a `Vec<&str>` for a shorter lifetime.
fn push_audio_format_args(args: &mut Vec<&str>, with_thumbnail: bool) {
    args.push("--extract-audio");
    args.push("--audio-format");
    args.push("mp3");
    args.push("--audio-quality");
    args.push("0");
    args.push("--add-metadata");
    if with_thumbnail {
        args.push("--embed-thumbnail");
    }
}

/// Push the video-specific prefix args (Tier 1 / Tier 2 shape — Tier 3
/// drops the `--postprocessor-args Merger:...` pair).
///
/// `container` is the yt-dlp `--merge-output-format` value — `"mp4"` for
/// H.264/AAC downloads, `"mkv"` for AV1/VP9 (4K/8K where YouTube has no H.264).
/// The `+faststart` Merger postprocessor is mp4-specific and is skipped for mkv.
fn push_video_format_args(args: &mut Vec<&str>, with_merger_postprocessor: bool, container: &'static str) {
    args.push("--format");
    args.push("--merge-output-format");
    args.push(container);
    if with_merger_postprocessor && container == "mp4" {
        args.push("--postprocessor-args");
        args.push("Merger:-movflags +faststart");
    }
}

/// Allowlist of domains that yt-dlp is permitted to handle.
/// Only these domains are accepted — arbitrary URLs are rejected for security.
const YTDLP_DOMAINS: &[&str] = &[
    // YouTube
    "youtube.com",
    "youtu.be",
    "music.youtube.com",
    // Audio platforms
    "soundcloud.com",
    "bandcamp.com",
    "audiomack.com",
    "mixcloud.com",
    // Video platforms
    "vimeo.com",
    "dailymotion.com",
    "rutube.ru",
    "bilibili.com",
    "nicovideo.jp",
    // Social media
    "tiktok.com",
    "twitter.com",
    "x.com",
    "facebook.com",
    "reddit.com",
    "ok.ru",
    "vk.com",
    // Streaming / clips
    "twitch.tv",
    "clips.twitch.tv",
    // Music services
    "music.apple.com",
    "deezer.com",
    "open.spotify.com",
    // Other
    "vlipsy.com",
    "streamable.com",
    "coub.com",
    "rumble.com",
    "odysee.com",
    "lbry.tv",
    "piped.video",
    "invidio.us",
    "yewtu.be",
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
            YTDLP_DOMAINS.iter().any(|d| {
                host_lower == *d
                    || (host_lower.len() > d.len()
                        && host_lower.ends_with(d)
                        && host_lower.as_bytes()[host_lower.len() - d.len() - 1] == b'.')
            })
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
        Self::is_known_domain(url)
    }

    async fn get_metadata(&self, url: &Url) -> Result<crate::download::source::MediaMetadata, AppError> {
        let (title, artist) = get_metadata_from_ytdlp(url, None).await?;
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

        // Experimental features graduated to main workflow
        if request.concurrent_fragments > 1 {
            log::info!(
                "Using {} concurrent fragments for audio download",
                request.concurrent_fragments
            );
        }
        let cf_str = concurrent_fragments_str(request.concurrent_fragments);
        let is_youtube = crate::core::share::is_youtube_url(request.url.as_str());

        let subprocess_timeout = config::download::ytdlp_download_timeout_for_quality(None);
        let handle = tokio::task::spawn_blocking(move || {
            let postprocessor_args = format!("ffmpeg:-acodec libmp3lame -b:a {}", bitrate_str);

            download_with_fallback_chain(
                &ytdl_bin,
                &url_str,
                &download_path,
                &progress_tx,
                "audio",
                subprocess_timeout,
                move |args, proxy_option| {
                    push_audio_format_args(args, true);
                    if is_youtube {
                        add_cookies_args_with_proxy(args, proxy_option, default_pot_token());
                        args.push("--extractor-args");
                        args.push(default_youtube_extractor_args());
                    } else {
                        add_no_cookies_args(args, proxy_option);
                        args.push("--extractor-args");
                        args.push("youtube:player_client=default;formats=missing_pot");
                    }
                    push_js_runtimes_tail(args, cf_str);
                    args.push("--postprocessor-args");
                },
                {
                    let url_for_tier2 = url_str.clone();
                    move |args: &mut Vec<&str>, proxy_option: Option<&crate::download::metadata::ProxyConfig>| {
                        // Tier 2 (cookies): audio-specific args
                        push_audio_format_args(args, true);
                        if is_instagram_url(&url_for_tier2) {
                            add_instagram_cookies_args_with_proxy(args, proxy_option);
                        } else {
                            add_cookies_args_with_proxy(args, proxy_option, default_pot_token());
                            args.push("--extractor-args");
                            args.push(default_youtube_extractor_args());
                        }
                        push_js_runtimes_tail(args, cf_str);
                        args.push("--postprocessor-args");
                    }
                },
                move |args, proxy_option| {
                    // Tier 3 (fixup never): audio-specific args
                    args.push("--fixup");
                    args.push("never");
                    push_audio_format_args(args, false);
                    add_cookies_args_with_proxy(args, proxy_option, default_pot_token());
                    args.push("--extractor-args");
                    args.push(default_youtube_extractor_args());
                    push_js_runtimes_tail(args, cf_str);
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

        let file_size = fs_err::metadata(&actual_path).map(|m| m.len()).unwrap_or(0);

        Ok(DownloadOutput {
            file_path: actual_path,
            duration_secs: duration,
            file_size,
            mime_hint: Some("audio/mpeg".to_string()),
            additional_files: None,
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

        // Experimental features graduated to main workflow
        if request.concurrent_fragments > 1 {
            log::info!(
                "Using {} concurrent fragments for video download",
                request.concurrent_fragments
            );
        }
        let cf_str = concurrent_fragments_str(request.concurrent_fragments);
        let is_youtube = crate::core::share::is_youtube_url(request.url.as_str());

        // High-resolution (4K/8K) requires AV1/VP9 codecs — YouTube has no H.264
        // above 1080p. Switch format-string builder + container accordingly.
        let (format_arg, container): (String, &'static str) = match request.video_quality.as_deref() {
            Some("4320p") => (build_highres_format(4320), "mkv"),
            Some("2160p") => (build_highres_format(2160), "mkv"),
            Some("1440p") => (build_highres_format(1440), "mkv"),
            Some("1080p") => (build_telegram_safe_format(Some(1080)), "mp4"),
            Some("720p") => (build_telegram_safe_format(Some(720)), "mp4"),
            Some("480p") => (build_telegram_safe_format(Some(480)), "mp4"),
            Some("360p") => (build_telegram_safe_format(Some(360)), "mp4"),
            Some("240p") => (build_telegram_safe_format(Some(240)), "mp4"),
            Some("144p") => (build_telegram_safe_format(Some(144)), "mp4"),
            _ => (build_telegram_safe_format(None), "mp4"),
        };
        log::debug!("yt-dlp video format string ({}): {}", container, format_arg);

        let subprocess_timeout = config::download::ytdlp_download_timeout_for_quality(request.video_quality.as_deref());
        let handle = tokio::task::spawn_blocking(move || {
            download_with_fallback_chain(
                &ytdl_bin,
                &url_str,
                &download_path,
                &progress_tx,
                "video",
                subprocess_timeout,
                move |args, proxy_option| {
                    push_video_format_args(args, true, container);
                    if is_youtube {
                        add_cookies_args_with_proxy(args, proxy_option, default_pot_token());
                        args.push("--extractor-args");
                        args.push(default_youtube_extractor_args());
                    } else {
                        add_no_cookies_args(args, proxy_option);
                        args.push("--extractor-args");
                        args.push("youtube:player_client=default;formats=missing_pot");
                    }
                    push_js_runtimes_tail(args, cf_str);
                },
                {
                    let url_for_tier2 = url_str.clone();
                    move |args: &mut Vec<&str>, proxy_option: Option<&crate::download::metadata::ProxyConfig>| {
                        // Tier 2 (cookies + PO token)
                        push_video_format_args(args, true, container);
                        if is_instagram_url(&url_for_tier2) {
                            add_instagram_cookies_args_with_proxy(args, proxy_option);
                        } else {
                            add_cookies_args_with_proxy(args, proxy_option, default_pot_token());
                            args.push("--extractor-args");
                            args.push(default_youtube_extractor_args());
                        }
                        push_js_runtimes_tail(args, cf_str);
                    }
                },
                move |args, proxy_option| {
                    // Tier 3 (fixup never): same client logic as tier 2
                    args.push("--fixup");
                    args.push("never");
                    push_video_format_args(args, false, container);
                    add_cookies_args_with_proxy(args, proxy_option, default_pot_token());
                    args.push("--extractor-args");
                    args.push(default_youtube_extractor_args());
                    push_js_runtimes_tail(args, cf_str);
                },
                &format_arg,
                time_range.as_ref(),
            )
        });

        handle
            .await
            .map_err(|e| AppError::Download(DownloadError::YtDlp(format!("Task join error: {}", e))))??;

        let actual_path = find_actual_downloaded_file(&request.output_path)?;

        let file_size = fs_err::metadata(&actual_path).map(|m| m.len()).unwrap_or(0);

        let duration = probe_duration_seconds(&actual_path).await;

        let mime = if actual_path.ends_with(".mkv") {
            "video/x-matroska"
        } else {
            "video/mp4"
        };

        Ok(DownloadOutput {
            file_path: actual_path,
            duration_secs: duration,
            file_size,
            mime_hint: Some(mime.to_string()),
            additional_files: None,
        })
    }
}

/// Check if a URL string belongs to Instagram.
fn is_instagram_url(url_str: &str) -> bool {
    url_str.contains("instagram.com")
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
    subprocess_timeout: Duration,
) -> Result<(), (YtDlpErrorType, String)>
where
    F: Fn(&mut Vec<&str>, Option<&crate::download::metadata::ProxyConfig>),
{
    // Experimental features graduated to main workflow
    let mut args: Vec<&str> = build_common_args(download_path);
    tier1_args_fn(&mut args, proxy_option);

    if media_type == "audio" {
        args.push(extra_arg);
    } else if let Some(pos) = args.iter().position(|a| *a == "--format") {
        args.insert(pos + 1, extra_arg);
    }
    append_section_args(&mut args, section_spec);

    // Use cached info JSON from preview phase to skip yt-dlp extraction (~3-5s savings)
    let info_json_path: Option<String> =
        crate::core::share::youtube_info_cache_path(url_str).filter(|p| std::path::Path::new(p).exists());
    if let Some(ref path) = info_json_path {
        args.push("--load-info-json");
        args.push(path);
        log::info!("[LOAD_INFO_JSON] Using cached JSON for {}", url_str);
    } else {
        args.push(url_str);
    }

    log::debug!(
        "yt-dlp command for {} download: {} {}",
        media_type,
        ytdl_bin,
        args.join(" ")
    );
    let result = run_ytdlp_with_progress(ytdl_bin, &args, progress_tx, subprocess_timeout);

    if let Some(path) = info_json_path {
        let _ = fs_err::remove_file(&path);
    }

    result
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
    subprocess_timeout: Duration,
) -> Tier2Outcome
where
    F: Fn(&mut Vec<&str>, Option<&crate::download::metadata::ProxyConfig>),
{
    // Experimental features graduated to main workflow
    crate::download::cookies::log_cookie_file_diagnostics(&format!("{}_TIER2_BEFORE", media_type.to_uppercase()));

    let _ = fs_err::remove_file(download_path);
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

    match run_ytdlp_with_progress(ytdl_bin, &cookies_args, progress_tx, subprocess_timeout) {
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
    subprocess_timeout: Duration,
) -> bool
where
    F: Fn(&mut Vec<&str>, Option<&crate::download::metadata::ProxyConfig>),
{
    // Experimental features graduated to main workflow
    log::warn!("🔧 Postprocessing error, retrying with --fixup never...");

    let _ = fs_err::remove_file(download_path);
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

    match run_ytdlp_with_progress(ytdl_bin, &fixup_args, progress_tx, subprocess_timeout) {
        Ok(()) => {
            log::info!("✅ [FIXUP_NEVER] {} download succeeded!", media_type);
            true
        }
        Err((_error_type, stderr_text)) => {
            log::warn!("❌ [FIXUP_NEVER] Failed: {}", &stderr_text);
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
    subprocess_timeout: Duration,
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
    // Experimental features graduated to main workflow
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

        let proxy_label = if proxy_name.contains("WARP") || proxy_name.contains("warp") {
            "warp"
        } else if proxy_name.contains("Geonode") {
            "geonode"
        } else if proxy_name.contains("Direct") {
            "direct"
        } else {
            "custom"
        };

        log::info!(
            "📡 {} download attempt {}/{} using [{}]",
            media_type,
            attempt + 1,
            total_proxies,
            proxy_name
        );

        if attempt > 0 {
            let _ = fs_err::remove_file(download_path);
            cleanup_partial_download(download_path);
        }

        // ── Tier 1 ──
        // For YouTube: tier1 closure already uses cookies (datacenter IPs are flagged).
        // For other sites: tier1 closure tries without cookies first.
        let tier1_start = std::time::Instant::now();
        let tier1_result = try_tier1(
            ytdl_bin,
            download_path,
            url_str,
            media_type,
            extra_arg,
            section_spec.as_deref(),
            proxy_option.as_ref(),
            progress_tx,
            &tier1_args_fn,
            subprocess_timeout,
        );
        log::info!("⏱️ [TIER1] done in {:.1}s", tier1_start.elapsed().as_secs_f64());

        match tier1_result {
            Ok(()) => {
                crate::core::metrics::record_tier_attempt("tier1_no_cookies", true);
                crate::core::metrics::PROXY_REQUESTS_TOTAL
                    .with_label_values(&[proxy_label, "success"])
                    .inc();
                log::info!(
                    "✅ {} download succeeded using [{}] (attempt {}/{})",
                    media_type,
                    proxy_name,
                    attempt + 1,
                    total_proxies
                );
                return Ok(runtime_handle.block_on(probe_duration_seconds(download_path)));
            }
            Err((error_type, stderr_text)) => {
                crate::core::metrics::record_tier_attempt("tier1_no_cookies", false);
                let error_msg = get_error_message(&error_type);
                log::error!(
                    "❌ Download failed with [{}]: {:?} - {}",
                    proxy_name,
                    error_type,
                    &stderr_text
                );

                // Geo-block: cookies/PO-token can't unlock a country restriction,
                // but a different proxy might (e.g. preview succeeded via [Direct]
                // after [Custom Proxy] hit a geo-block). Skip Tier 2/3 immediately
                // and try the next proxy.
                let is_geo_block = matches!(error_type, YtDlpErrorType::VideoUnavailable) && {
                    let s = stderr_text.to_lowercase();
                    s.contains("not available in your country")
                        || s.contains("not made this video available")
                        || s.contains("blocked in your country")
                };

                // Network-only errors: skip Tier 2/3, try next proxy
                let is_network_only = matches!(error_type, YtDlpErrorType::NetworkError)
                    || is_geo_block
                    || (is_proxy_related_error(&stderr_text) && !matches!(error_type, YtDlpErrorType::BotDetection));

                if is_network_only && attempt + 1 < total_proxies {
                    log::warn!(
                        "🔄 {} error, trying next proxy (attempt {}/{})",
                        if is_geo_block { "Geo-block" } else { "Network/proxy" },
                        attempt + 2,
                        total_proxies
                    );
                    crate::core::metrics::PROXY_REQUESTS_TOTAL
                        .with_label_values(&[proxy_label, "failure"])
                        .inc();
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

                    let tier2_start = std::time::Instant::now();
                    let tier2_result = try_tier2(
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
                        subprocess_timeout,
                    );
                    log::info!("⏱️ [TIER2] done in {:.1}s", tier2_start.elapsed().as_secs_f64());
                    match tier2_result {
                        Tier2Outcome::Success => {
                            crate::core::metrics::record_tier_attempt("tier2_cookies", true);
                            crate::core::metrics::PROXY_REQUESTS_TOTAL
                                .with_label_values(&[proxy_label, "success"])
                                .inc();
                            return Ok(runtime_handle.block_on(probe_duration_seconds(download_path)));
                        }
                        Tier2Outcome::CookieRefreshed => {
                            crate::core::metrics::record_tier_attempt("tier2_cookies", false);
                            last_error = Some(AppError::Download(DownloadError::YtDlp(error_msg.clone())));
                            continue;
                        }
                        Tier2Outcome::Failed => {
                            crate::core::metrics::record_tier_attempt("tier2_cookies", false);
                        }
                    }
                }

                // ── Tier 3: Fixup never (postprocessing errors) ──
                if error_type == YtDlpErrorType::PostprocessingError {
                    let tier3_ok = try_tier3(
                        ytdl_bin,
                        download_path,
                        url_str,
                        media_type,
                        extra_arg,
                        section_spec.as_deref(),
                        proxy_option.as_ref(),
                        progress_tx,
                        &tier3_args_fn,
                        subprocess_timeout,
                    );
                    crate::core::metrics::record_tier_attempt("tier3_fixup_never", tier3_ok);
                    if tier3_ok {
                        crate::core::metrics::PROXY_REQUESTS_TOTAL
                            .with_label_values(&[proxy_label, "success"])
                            .inc();
                        return Ok(runtime_handle.block_on(probe_duration_seconds(download_path)));
                    }
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

                crate::core::metrics::PROXY_REQUESTS_TOTAL
                    .with_label_values(&[proxy_label, "failure"])
                    .inc();

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

/// Build minimal common arguments (for Tier 2 and Tier 3 fallbacks).
///
/// ⚠️ Order of these args is load-bearing — per CLAUDE.md, arg-order bugs in
/// yt-dlp have caused production outages before. Do not reorder without a
/// Railway smoke test. The associated unit test
/// `build_common_args_has_expected_shape` asserts the exact slice.
fn build_common_args_minimal(download_path: &str) -> Vec<&str> {
    vec![
        "-o",
        download_path,
        "--newline",
        "--force-overwrites",
        "--no-playlist",
        "--age-limit",
        "99",
        "--fragment-retries",
        "10",
        "--socket-timeout",
        "30",
        "--http-chunk-size",
        "10485760",
    ]
}

/// Build common yt-dlp arguments shared by Tier 1 (full set with rate limiting).
/// Starts from `build_common_args_minimal` and appends the retry/throttle flags
/// so the two functions cannot drift out of sync on the shared prefix.
fn build_common_args(download_path: &str) -> Vec<&str> {
    let mut args = build_common_args_minimal(download_path);

    // No rate limit, auto re-extract on throttle, maximize throughput.
    // Note: -N (concurrent fragments) is added later by push_concurrent_fragments_arg.
    args.extend_from_slice(&[
        "--retries",
        "15",
        "--retry-sleep",
        "http:exp=1:30",
        "--retry-sleep",
        "fragment:exp=1:30",
        "--throttled-rate",
        "100K",
    ]);

    args
}

// Tier 2/3 already use build_common_args_minimal (no rate limiting), which is fine.

#[cfg(test)]
mod common_args_tests {
    //! Byte-identical regression tests for the yt-dlp common-argv builders.
    //!
    //! Any change to these arg lists is a potential production outage (see
    //! CLAUDE.md: `-N` between `--postprocessor-args` and its value once
    //! broke every download). The tests pin the exact slice so a refactor
    //! that silently drops or reorders an arg fails CI.
    use super::{build_common_args, build_common_args_minimal};

    const EXPECTED_MINIMAL: &[&str] = &[
        "-o",
        "/tmp/t.mp3",
        "--newline",
        "--force-overwrites",
        "--no-playlist",
        "--age-limit",
        "99",
        "--fragment-retries",
        "10",
        "--socket-timeout",
        "30",
        "--http-chunk-size",
        "10485760",
    ];

    const EXPECTED_FULL_TAIL: &[&str] = &[
        "--retries",
        "15",
        "--retry-sleep",
        "http:exp=1:30",
        "--retry-sleep",
        "fragment:exp=1:30",
        "--throttled-rate",
        "100K",
    ];

    #[test]
    fn minimal_args_have_expected_shape() {
        let args = build_common_args_minimal("/tmp/t.mp3");
        assert_eq!(args.as_slice(), EXPECTED_MINIMAL);
    }

    #[test]
    fn full_args_are_minimal_plus_retry_tail() {
        let args = build_common_args("/tmp/t.mp3");
        let expected: Vec<&str> = EXPECTED_MINIMAL
            .iter()
            .chain(EXPECTED_FULL_TAIL.iter())
            .copied()
            .collect();
        assert_eq!(args, expected);
    }

    #[test]
    fn output_path_is_the_second_arg() {
        // Guards against any reordering that would break the `-o <path>`
        // positional pair — yt-dlp requires them adjacent.
        let args = build_common_args_minimal("/custom/path.mp4");
        assert_eq!(args[0], "-o");
        assert_eq!(args[1], "/custom/path.mp4");
    }

    // ==== Byte-identical tests for the Tier 1/2/3 helper functions ====

    use super::{push_audio_format_args, push_js_runtimes_tail, push_video_format_args};

    #[test]
    fn js_runtimes_tail_with_cf_enabled() {
        let mut args: Vec<&str> = Vec::new();
        push_js_runtimes_tail(&mut args, "4");
        assert_eq!(args, vec!["--js-runtimes", "deno", "--no-check-certificate", "-N", "4"]);
    }

    #[test]
    fn js_runtimes_tail_without_cf() {
        let mut args: Vec<&str> = Vec::new();
        push_js_runtimes_tail(&mut args, "");
        // Empty cf_str → no -N pair
        assert_eq!(args, vec!["--js-runtimes", "deno", "--no-check-certificate"]);
    }

    #[test]
    fn audio_format_args_with_thumbnail_match_tier1_2() {
        // Pins the exact Tier 1/2 audio prefix: 7 args in this exact order.
        let mut args: Vec<&str> = Vec::new();
        push_audio_format_args(&mut args, true);
        assert_eq!(
            args,
            vec![
                "--extract-audio",
                "--audio-format",
                "mp3",
                "--audio-quality",
                "0",
                "--add-metadata",
                "--embed-thumbnail",
            ]
        );
    }

    #[test]
    fn audio_format_args_without_thumbnail_match_tier3() {
        // Tier 3 drops --embed-thumbnail because it's followed by --fixup never.
        let mut args: Vec<&str> = Vec::new();
        push_audio_format_args(&mut args, false);
        assert_eq!(
            args,
            vec![
                "--extract-audio",
                "--audio-format",
                "mp3",
                "--audio-quality",
                "0",
                "--add-metadata",
            ]
        );
    }

    #[test]
    fn video_format_args_with_merger_match_tier1_2() {
        // Tier 1/2 video: --format followed by the Merger postprocessor pair.
        let mut args: Vec<&str> = Vec::new();
        push_video_format_args(&mut args, true, "mp4");
        assert_eq!(
            args,
            vec![
                "--format",
                "--merge-output-format",
                "mp4",
                "--postprocessor-args",
                "Merger:-movflags +faststart",
            ]
        );
    }

    #[test]
    fn video_format_args_without_merger_match_tier3() {
        // Tier 3 video: no Merger postprocessor.
        let mut args: Vec<&str> = Vec::new();
        push_video_format_args(&mut args, false, "mp4");
        assert_eq!(args, vec!["--format", "--merge-output-format", "mp4"]);
    }

    #[test]
    fn video_format_args_mkv_skips_faststart() {
        // High-res (mkv container) must skip the mp4-specific faststart Merger arg.
        let mut args: Vec<&str> = Vec::new();
        push_video_format_args(&mut args, true, "mkv");
        assert_eq!(args, vec!["--format", "--merge-output-format", "mkv"]);
    }
}

/// Run yt-dlp with stdout/stderr capture and progress reporting.
///
/// Returns Ok(()) on success, or Err((YtDlpErrorType, stderr_text)) on failure.
fn run_ytdlp_with_progress(
    ytdl_bin: &str,
    args: &[&str],
    progress_tx: &mpsc::UnboundedSender<SourceProgress>,
    subprocess_timeout: Duration,
) -> Result<(), (YtDlpErrorType, String)> {
    let ytdlp_start = std::time::Instant::now();
    let child_result = Command::new(ytdl_bin)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();

    let mut child = match child_result {
        Ok(c) => c,
        Err(e) => {
            log::error!("Failed to spawn yt-dlp: {}", e);
            crate::core::metrics::YTDLP_EXECUTION_DURATION_SECONDS
                .with_label_values(&["download"])
                .observe(ytdlp_start.elapsed().as_secs_f64());
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
                    // Log ERROR/WARNING lines at warn level for visibility in production
                    let lower = line_str.to_lowercase();
                    if lower.contains("error") || lower.contains("warning") || lower.contains("failed") {
                        log::warn!("yt-dlp stderr: {}", line_str);
                    } else {
                        log::debug!("yt-dlp stderr: {}", line_str);
                    }
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
    let ytdlp_timeout = subprocess_timeout;
    let deadline = std::time::Instant::now() + ytdlp_timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    log::error!("yt-dlp process timed out after {}s, killing", ytdlp_timeout.as_secs());
                    let _ = child.kill();
                    let _ = child.wait();
                    crate::core::metrics::YTDLP_EXECUTION_DURATION_SECONDS
                        .with_label_values(&["download"])
                        .observe(ytdlp_start.elapsed().as_secs_f64());
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
                crate::core::metrics::YTDLP_EXECUTION_DURATION_SECONDS
                    .with_label_values(&["download"])
                    .observe(ytdlp_start.elapsed().as_secs_f64());
                return Err((YtDlpErrorType::Unknown, format!("downloader process failed: {}", e)));
            }
        }
    };

    if status.success() {
        crate::core::metrics::YTDLP_EXECUTION_DURATION_SECONDS
            .with_label_values(&["download"])
            .observe(ytdlp_start.elapsed().as_secs_f64());
        return Ok(());
    }

    let stderr_text = if let Ok(mut lines) = stderr_lines.lock() {
        lines.make_contiguous().join("\n")
    } else {
        String::new()
    };

    let error_type = analyze_ytdlp_error(&stderr_text);
    crate::core::metrics::YTDLP_EXECUTION_DURATION_SECONDS
        .with_label_values(&["download"])
        .observe(ytdlp_start.elapsed().as_secs_f64());
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
    fn test_supports_url_soundcloud_artist() {
        let source = YtDlpSource::new();
        let url = Url::parse("https://soundcloud.com/artist").unwrap();
        assert!(source.supports_url(&url));
    }

    #[test]
    fn test_supports_url_soundcloud_track() {
        let source = YtDlpSource::new();
        let url = Url::parse("https://soundcloud.com/artist/track-name").unwrap();
        assert!(source.supports_url(&url));
    }

    #[test]
    fn test_supports_url_soundcloud_set() {
        let source = YtDlpSource::new();
        let url = Url::parse("https://soundcloud.com/artist/sets/album").unwrap();
        assert!(source.supports_url(&url));
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
