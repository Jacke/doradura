//! HttpSource — direct HTTP download source with chunked transfer and resume support.
//!
//! Handles direct file URLs (e.g., `https://example.com/file.mp3`).
//! Features:
//! - Chunked download with progress tracking via reqwest
//! - Resume via HTTP Range headers (if server supports it)
//! - Content-Disposition parsing for filename
//! - HEAD request for size estimation
//! - Fallback source for any http/https URL not handled by YtDlpSource

use crate::core::error::AppError;
use crate::download::error::DownloadError;
use crate::download::source::{DownloadOutput, DownloadRequest, DownloadSource, SourceProgress};
use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::Client;
use std::net::SocketAddr;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;
use url::Url;

/// Known direct-file extensions this source handles.
const DIRECT_FILE_EXTENSIONS: &[&str] = &[
    "mp3", "mp4", "wav", "flac", "ogg", "m4a", "webm", "avi", "mkv", "aac", "opus",
];

/// Returns `true` when the IP address belongs to a private, loopback, link-local,
/// or otherwise non-routable range that should never be reachable from the internet.
///
/// Used by the SSRF guard to prevent downloads from being redirected to internal
/// infrastructure (e.g. cloud metadata endpoints, LAN hosts).
fn is_private_ip(ip: &std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_unspecified()
                || v4.is_multicast()
                || o[0] == 0                                       // 0.0.0.0/8
                || (o[0] == 100 && (o[1] & 0xc0) == 64)            // 100.64/10 CGNAT
                || o[0] == 127                                     // loopback (belt+braces)
                || (o[0] == 169 && o[1] == 254)                    // link-local + AWS/GCP/Azure metadata
                || (o[0] == 192 && o[1] == 0 && o[2] == 0)         // 192.0.0/24 IETF protocol assignments
                || (o[0] == 192 && o[1] == 0 && o[2] == 2)         // TEST-NET-1
                || (o[0] == 198 && (o[1] == 18 || o[1] == 19))     // benchmark 198.18/15
                || (o[0] == 198 && o[1] == 51 && o[2] == 100)      // TEST-NET-2
                || (o[0] == 203 && o[1] == 0 && o[2] == 113)       // TEST-NET-3
                || o[0] >= 240                                     // class E (240/4) + 255.255.255.255
                // Cloud metadata endpoints on public ranges:
                || o == [100, 100, 100, 200]                       // Alibaba Cloud ECS metadata
                || o == [192, 0, 0, 192] // Oracle OCI metadata
        }
        std::net::IpAddr::V6(v6) => {
            if v6.is_loopback() || v6.is_unspecified() || v6.is_multicast() {
                return true;
            }
            let seg = v6.segments();
            // fc00::/7 Unique Local Addresses
            if (seg[0] & 0xfe00) == 0xfc00 {
                return true;
            }
            // fe80::/10 link-local
            if (seg[0] & 0xffc0) == 0xfe80 {
                return true;
            }
            // ::ffff:0:0/96 IPv4-mapped — unwrap and recheck as v4.
            // Manual check for broader rustc compat: first 5 segments are zero, 6th is 0xffff.
            if seg[0] == 0 && seg[1] == 0 && seg[2] == 0 && seg[3] == 0 && seg[4] == 0 && seg[5] == 0xffff {
                let v4 = std::net::Ipv4Addr::new(
                    (seg[6] >> 8) as u8,
                    (seg[6] & 0xff) as u8,
                    (seg[7] >> 8) as u8,
                    (seg[7] & 0xff) as u8,
                );
                return is_private_ip(&std::net::IpAddr::V4(v4));
            }
            // 2001:db8::/32 documentation range
            if seg[0] == 0x2001 && seg[1] == 0x0db8 {
                return true;
            }
            false
        }
    }
}

/// Result of `check_ssrf`: the validated host string and the pinned list of
/// resolved `SocketAddr`s that passed the private-IP filter.
///
/// The caller should pass these addresses to `reqwest::ClientBuilder::resolve_to_addrs`
/// so that reqwest uses the **exact** IPs we validated — defeating DNS rebinding
/// attacks where an attacker's DNS server returns a public IP on the first lookup
/// and a private IP on the second.
struct SsrfCheck {
    host: String,
    addrs: Vec<SocketAddr>,
}

/// Resolve the hostname in `url`, reject it if any resolved address is private,
/// and return the pinned (host, addrs) pair so the caller can force reqwest to
/// connect to those exact IPs.
///
/// Blocks SSRF attacks that use internal hostnames (e.g. `http://169.254.169.254/latest/meta-data/`)
/// AND DNS rebinding attacks (where DNS is re-resolved between the check and the actual
/// connection and returns a different IP the second time).
///
/// Also enforces a scheme whitelist — only `http` and `https` are allowed. Schemes like
/// `file://`, `gopher://`, `ftp://` are rejected outright.
async fn check_ssrf(url: &Url) -> Result<SsrfCheck, AppError> {
    // Scheme whitelist — reject anything exotic (file, gopher, ftp, data, ...).
    let scheme = url.scheme();
    if scheme != "http" && scheme != "https" {
        log::warn!("SSRF blocked: non-http(s) scheme '{}' in URL {}", scheme, url);
        return Err(AppError::Validation(format!(
            "Unsupported URL scheme '{}': only http/https allowed",
            scheme
        )));
    }

    let host = url.host_str().ok_or_else(|| {
        log::warn!("SSRF blocked: URL has no host: {}", url);
        AppError::Validation("URL has no host component".to_string())
    })?;

    // Use the URL's port if present, otherwise the scheme default. This matters
    // because `resolve_to_addrs` needs the same port the client will connect to.
    let port = url
        .port_or_known_default()
        .unwrap_or(if scheme == "https" { 443 } else { 80 });
    let lookup_addr = format!("{}:{}", host, port);

    let resolved = tokio::net::lookup_host(&lookup_addr).await.map_err(|e| {
        log::warn!("SSRF check: hostname resolution failed for {}: {}", host, e);
        AppError::Validation(format!("Could not resolve hostname '{}': {}", host, e))
    })?;

    let addrs: Vec<SocketAddr> = resolved.collect();
    if addrs.is_empty() {
        return Err(AppError::Validation(format!(
            "Hostname '{}' resolved to no addresses",
            host
        )));
    }

    for addr in &addrs {
        if is_private_ip(&addr.ip()) {
            log::warn!("SSRF blocked: URL {} resolves to private IP {}", url, addr.ip());
            return Err(AppError::Validation(format!(
                "URL resolves to a private/internal IP address ({}): blocked for security",
                addr.ip()
            )));
        }
    }

    Ok(SsrfCheck {
        host: host.to_string(),
        addrs,
    })
}

/// Build an ad-hoc `reqwest::Client` that is pinned to the exact resolved
/// addresses returned by `check_ssrf`, with redirects **disabled** so the
/// caller can validate each hop manually.
///
/// This is the cornerstone of our SSRF defense — by pinning `resolve_to_addrs`,
/// reqwest never performs its own DNS lookup for this host, so a malicious
/// DNS server cannot flip the IP between our check and the actual connection.
fn build_pinned_client(check: &SsrfCheck) -> Result<Client, AppError> {
    Client::builder()
        .user_agent("Mozilla/5.0 (compatible; doradura/0.2)")
        .timeout(std::time::Duration::from_secs(600))
        .connect_timeout(std::time::Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .resolve_to_addrs(&check.host, &check.addrs)
        .build()
        .map_err(|e| AppError::Download(DownloadError::Other(format!("pinned client build failed: {}", e))))
}

/// Maximum number of redirect hops we'll follow manually. Each hop gets its own
/// SSRF re-validation so redirects cannot escape into the private network.
const MAX_REDIRECT_HOPS: u8 = 5;

/// Download source for direct HTTP file downloads.
///
/// Each request builds its own pinned `reqwest::Client` via `build_pinned_client`
/// so the resolved IPs cannot be changed by a DNS server between our SSRF check
/// and the actual connection (DNS rebinding defense).
pub struct HttpSource;

impl Default for HttpSource {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpSource {
    pub fn new() -> Self {
        Self
    }

    /// Extract filename from Content-Disposition header or URL path.
    fn extract_filename(response: &reqwest::Response, url: &Url) -> String {
        // Try Content-Disposition header first
        if let Some(cd) = response.headers().get("content-disposition") {
            if let Ok(cd_str) = cd.to_str() {
                // Parse: attachment; filename="file.mp3" or filename*=UTF-8''file.mp3
                if let Some(start) = cd_str.find("filename=") {
                    let value = &cd_str[start + 9..];
                    let filename = value.trim_start_matches('"').split('"').next().unwrap_or("download");
                    // Sanitize: strip path components to prevent traversal
                    let safe_name = std::path::Path::new(filename)
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("download");
                    if !safe_name.is_empty() && !safe_name.contains('\0') {
                        return safe_name.to_string();
                    }
                }
            }
        }

        // Fallback: extract from URL path
        url.path_segments()
            .and_then(|mut segments| segments.next_back())
            .filter(|s| !s.is_empty())
            .map(|s| {
                // URL-decode the filename
                urlencoding::decode(s).unwrap_or_else(|_| s.into()).to_string()
            })
            .unwrap_or_else(|| "download".to_string())
    }

    /// Guess MIME type from file extension.
    fn mime_from_extension(path: &str) -> Option<String> {
        let ext = path.rsplit('.').next()?.to_lowercase();
        match ext.as_str() {
            "mp3" => Some("audio/mpeg".to_string()),
            "mp4" => Some("video/mp4".to_string()),
            "wav" => Some("audio/wav".to_string()),
            "flac" => Some("audio/flac".to_string()),
            "ogg" => Some("audio/ogg".to_string()),
            "m4a" => Some("audio/mp4".to_string()),
            "webm" => Some("video/webm".to_string()),
            "avi" => Some("video/x-msvideo".to_string()),
            "mkv" => Some("video/x-matroska".to_string()),
            "aac" => Some("audio/aac".to_string()),
            "opus" => Some("audio/opus".to_string()),
            _ => None,
        }
    }
}

#[async_trait]
impl DownloadSource for HttpSource {
    fn name(&self) -> &str {
        "http"
    }

    fn supports_url(&self, url: &Url) -> bool {
        let scheme = url.scheme();
        if scheme != "http" && scheme != "https" {
            return false;
        }

        // Check if URL path ends with a known file extension
        let path = url.path().to_lowercase();
        DIRECT_FILE_EXTENSIONS
            .iter()
            .any(|ext| path.ends_with(&format!(".{}", ext)))
    }

    async fn get_metadata(&self, url: &Url) -> Result<crate::download::source::MediaMetadata, AppError> {
        // For direct HTTP files, title is the filename and artist is empty
        let filename = url
            .path_segments()
            .and_then(|mut segments| segments.next_back())
            .filter(|s| !s.is_empty())
            .map(|s| urlencoding::decode(s).unwrap_or_else(|_| s.into()).to_string())
            .unwrap_or_else(|| "Download".to_string());

        // Strip extension from title
        let title = if let Some(dot_pos) = filename.rfind('.') {
            filename[..dot_pos].to_string()
        } else {
            filename
        };

        Ok(crate::download::source::MediaMetadata {
            title,
            artist: String::new(),
        })
    }

    async fn estimate_size(&self, url: &Url) -> Option<u64> {
        // SSRF guard — resolve and validate, then build a pinned client that connects
        // only to the validated IPs (defeats DNS rebinding).
        let check = check_ssrf(url).await.ok()?;
        let pinned = build_pinned_client(&check).ok()?;

        // Manual redirect loop (up to 3 hops for HEAD, each re-validated)
        let mut current = url.clone();
        for _ in 0..3 {
            let response =
                tokio::time::timeout(std::time::Duration::from_secs(10), pinned.head(current.as_str()).send())
                    .await
                    .ok()?
                    .ok()?;

            if response.status().is_redirection() {
                let loc = response.headers().get(reqwest::header::LOCATION)?.to_str().ok()?;
                current = current.join(loc).ok()?;
                // Re-validate the redirect target before following
                let _ = check_ssrf(&current).await.ok()?;
                // Note: different host means the pinned client's resolve map no longer
                // applies — we'd need a new client. For safety, abort cross-host redirects.
                if current.host_str() != url.host_str() {
                    log::warn!("estimate_size: cross-host redirect rejected");
                    return None;
                }
                continue;
            }
            return response.content_length();
        }
        None
    }

    async fn is_livestream(&self, _url: &Url) -> bool {
        false // Direct HTTP files are never livestreams
    }

    async fn download(
        &self,
        request: &DownloadRequest,
        progress_tx: mpsc::UnboundedSender<SourceProgress>,
    ) -> Result<DownloadOutput, AppError> {
        log::info!("📥 HTTP direct download: {}", request.url);

        // SSRF guard — resolve and validate, pin reqwest to the validated IPs so
        // a second DNS lookup cannot return a different (private) address.
        let ssrf_check = check_ssrf(&request.url).await?;
        let pinned_client = build_pinned_client(&ssrf_check)?;

        // Check if we can resume (file already partially downloaded)
        let existing_size = fs_err::tokio::metadata(&request.output_path)
            .await
            .map(|m| m.len())
            .unwrap_or(0);

        // Manual redirect loop: each hop gets SSRF re-validated before the connection.
        // Cross-host redirects rebuild a new pinned client with fresh DNS validation.
        let mut current = request.url.clone();
        let mut current_client = pinned_client;
        let mut current_host = ssrf_check.host.clone();
        let response: reqwest::Response;

        let mut hops: u8 = 0;
        loop {
            if hops >= MAX_REDIRECT_HOPS {
                return Err(AppError::Download(DownloadError::Other(
                    "too many redirects".to_string(),
                )));
            }
            hops += 1;

            let mut req = current_client.get(current.as_str());
            if existing_size > 0 && hops == 1 {
                log::info!("Resuming download from byte {}: {}", existing_size, request.output_path);
                req = req.header("Range", format!("bytes={}-", existing_size));
            }

            let resp = req
                .send()
                .await
                .map_err(|e| AppError::Download(DownloadError::Other(format!("HTTP request failed: {}", e))))?;

            if resp.status().is_redirection() {
                let loc = resp
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .ok_or_else(|| AppError::Download(DownloadError::Other("redirect missing Location".into())))?
                    .to_str()
                    .map_err(|_| AppError::Download(DownloadError::Other("invalid Location header".into())))?
                    .to_string();

                drop(resp);
                let next = current
                    .join(&loc)
                    .map_err(|e| AppError::Download(DownloadError::Other(format!("invalid redirect URL: {}", e))))?;

                // Re-validate EVERY redirect hop BEFORE connecting.
                let next_check = check_ssrf(&next).await?;

                // Cross-host redirect → rebuild pinned client for the new host.
                if next_check.host != current_host {
                    log::info!("cross-host redirect: {} -> {}", current_host, next_check.host);
                    current_client = build_pinned_client(&next_check)?;
                    current_host = next_check.host.clone();
                }
                current = next;
                continue;
            }

            response = resp;
            break;
        }

        if !response.status().is_success() && response.status().as_u16() != 206 {
            return Err(AppError::Download(DownloadError::Other(format!(
                "HTTP {} for {}",
                response.status(),
                request.url
            ))));
        }

        let is_partial = response.status().as_u16() == 206;
        let total_size = if is_partial {
            // Parse Content-Range header for total size
            response
                .headers()
                .get("content-range")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.rsplit('/').next())
                .and_then(|s| s.parse::<u64>().ok())
        } else {
            response.content_length()
        };

        let _filename = Self::extract_filename(&response, &request.url);
        let mime_hint = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
            .or_else(|| Self::mime_from_extension(&request.output_path));

        // Open file for writing (append if resuming) — async to avoid blocking the runtime
        let mut file = if is_partial && existing_size > 0 {
            tokio::fs::OpenOptions::new()
                .append(true)
                .open(&request.output_path)
                .await
                .map_err(|e| {
                    AppError::Download(DownloadError::Other(format!("Failed to open file for resume: {}", e)))
                })?
        } else {
            tokio::fs::File::create(&request.output_path)
                .await
                .map_err(|e| AppError::Download(DownloadError::Other(format!("Failed to create file: {}", e))))?
        };

        let mut downloaded: u64 = if is_partial { existing_size } else { 0 };
        let mut last_progress_percent = 0u8;

        // Stream response body in chunks
        let mut stream = response.bytes_stream();

        while let Some(chunk_result) = stream.next().await {
            let chunk = chunk_result
                .map_err(|e| AppError::Download(DownloadError::Other(format!("Error reading chunk: {}", e))))?;

            file.write_all(&chunk)
                .await
                .map_err(|e| AppError::Download(DownloadError::Other(format!("Error writing to file: {}", e))))?;

            downloaded += chunk.len() as u64;

            // Check max file size
            if let Some(max_size) = request.max_file_size {
                if downloaded > max_size {
                    crate::core::utils::try_remove_file(&request.output_path).await;
                    return Err(AppError::Validation(format!(
                        "File exceeds maximum size: {} bytes > {} bytes",
                        downloaded, max_size
                    )));
                }
            }

            // Send progress
            let percent = total_size
                .map(|total| {
                    if total > 0 {
                        ((downloaded as f64 / total as f64) * 100.0) as u8
                    } else {
                        0
                    }
                })
                .unwrap_or(0);

            if percent >= last_progress_percent + 5 || percent == 100 {
                last_progress_percent = percent;
                let _ = progress_tx.send(SourceProgress {
                    percent,
                    speed_bytes_sec: None, // Could add rate calculation
                    eta_seconds: None,
                    downloaded_bytes: Some(downloaded),
                    total_bytes: total_size,
                });
            }
        }

        file.flush()
            .await
            .map_err(|e| AppError::Download(DownloadError::Other(format!("Failed to flush file: {}", e))))?;

        let file_size = fs_err::tokio::metadata(&request.output_path)
            .await
            .map(|m| m.len())
            .unwrap_or(downloaded);

        log::info!(
            "✅ HTTP download complete: {} ({:.2} MB)",
            request.output_path,
            file_size as f64 / (1024.0 * 1024.0)
        );

        // Probe duration if it's a media file
        let duration_secs = crate::download::metadata::probe_duration_seconds(&request.output_path).await;

        Ok(DownloadOutput {
            file_path: request.output_path.clone(),
            duration_secs,
            file_size,
            mime_hint,
            additional_files: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_supports_url_mp3() {
        let source = HttpSource::new();
        let url = Url::parse("https://example.com/music/file.mp3").unwrap();
        assert!(source.supports_url(&url));
    }

    #[test]
    fn test_supports_url_mp4() {
        let source = HttpSource::new();
        let url = Url::parse("https://cdn.example.com/video.mp4").unwrap();
        assert!(source.supports_url(&url));
    }

    #[test]
    fn test_rejects_html_page() {
        let source = HttpSource::new();
        let url = Url::parse("https://example.com/page").unwrap();
        assert!(!source.supports_url(&url));
    }

    #[test]
    fn test_rejects_youtube() {
        let source = HttpSource::new();
        let url = Url::parse("https://www.youtube.com/watch?v=abc").unwrap();
        assert!(!source.supports_url(&url));
    }

    #[test]
    fn test_supports_flac() {
        let source = HttpSource::new();
        let url = Url::parse("https://example.com/audio.flac").unwrap();
        assert!(source.supports_url(&url));
    }

    #[test]
    fn test_mime_from_extension() {
        assert_eq!(
            HttpSource::mime_from_extension("file.mp3"),
            Some("audio/mpeg".to_string())
        );
        assert_eq!(
            HttpSource::mime_from_extension("video.mp4"),
            Some("video/mp4".to_string())
        );
        assert_eq!(HttpSource::mime_from_extension("file.xyz"), None);
    }

    // ── SSRF guard unit tests ──

    #[test]
    fn test_is_private_ip_loopback_v4() {
        use std::net::IpAddr;
        assert!(is_private_ip(&"127.0.0.1".parse::<IpAddr>().unwrap()));
    }

    #[test]
    fn test_is_private_ip_loopback_v6() {
        use std::net::IpAddr;
        assert!(is_private_ip(&"::1".parse::<IpAddr>().unwrap()));
    }

    #[test]
    fn test_is_private_ip_rfc1918() {
        use std::net::IpAddr;
        assert!(is_private_ip(&"10.0.0.1".parse::<IpAddr>().unwrap()));
        assert!(is_private_ip(&"172.16.0.1".parse::<IpAddr>().unwrap()));
        assert!(is_private_ip(&"192.168.1.1".parse::<IpAddr>().unwrap()));
    }

    #[test]
    fn test_is_private_ip_metadata_service() {
        use std::net::IpAddr;
        // AWS/GCP/Azure instance metadata endpoint
        assert!(is_private_ip(&"169.254.169.254".parse::<IpAddr>().unwrap()));
    }

    #[test]
    fn test_is_private_ip_public_not_blocked() {
        use std::net::IpAddr;
        assert!(!is_private_ip(&"8.8.8.8".parse::<IpAddr>().unwrap()));
        assert!(!is_private_ip(&"1.1.1.1".parse::<IpAddr>().unwrap()));
        assert!(!is_private_ip(&"93.184.216.34".parse::<IpAddr>().unwrap()));
    }

    #[tokio::test]
    async fn test_check_ssrf_rejects_localhost() {
        let url = Url::parse("http://localhost/secret.mp3").unwrap();
        assert!(check_ssrf(&url).await.is_err());
    }

    #[tokio::test]
    async fn test_check_ssrf_rejects_127_0_0_1() {
        let url = Url::parse("http://127.0.0.1/file.mp3").unwrap();
        assert!(check_ssrf(&url).await.is_err());
    }

    #[tokio::test]
    async fn test_check_ssrf_rejects_metadata_ip() {
        let url = Url::parse("http://169.254.169.254/latest/meta-data/file.mp3").unwrap();
        assert!(check_ssrf(&url).await.is_err());
    }

    // ── Extended deny-list tests ──

    #[test]
    fn test_is_private_ip_cgnat() {
        use std::net::IpAddr;
        // Railway/Tailscale CGNAT 100.64.0.0/10
        assert!(is_private_ip(&"100.64.0.1".parse::<IpAddr>().unwrap()));
        assert!(is_private_ip(&"100.127.255.255".parse::<IpAddr>().unwrap()));
    }

    #[test]
    fn test_is_private_ip_zero_network() {
        use std::net::IpAddr;
        // 0.0.0.0/8 routes to localhost on Linux
        assert!(is_private_ip(&"0.0.0.0".parse::<IpAddr>().unwrap()));
        assert!(is_private_ip(&"0.1.2.3".parse::<IpAddr>().unwrap()));
    }

    #[test]
    fn test_is_private_ip_alibaba_metadata() {
        use std::net::IpAddr;
        assert!(is_private_ip(&"100.100.100.200".parse::<IpAddr>().unwrap()));
    }

    #[test]
    fn test_is_private_ip_oracle_metadata() {
        use std::net::IpAddr;
        assert!(is_private_ip(&"192.0.0.192".parse::<IpAddr>().unwrap()));
    }

    #[test]
    fn test_is_private_ip_class_e() {
        use std::net::IpAddr;
        assert!(is_private_ip(&"240.0.0.1".parse::<IpAddr>().unwrap()));
        assert!(is_private_ip(&"255.255.255.255".parse::<IpAddr>().unwrap()));
    }

    #[test]
    fn test_is_private_ip_v6_ula() {
        use std::net::IpAddr;
        assert!(is_private_ip(&"fc00::1".parse::<IpAddr>().unwrap()));
        assert!(is_private_ip(&"fd12:3456::1".parse::<IpAddr>().unwrap()));
    }

    #[test]
    fn test_is_private_ip_v6_link_local() {
        use std::net::IpAddr;
        assert!(is_private_ip(&"fe80::1".parse::<IpAddr>().unwrap()));
    }

    #[test]
    fn test_is_private_ip_v6_ipv4_mapped_loopback() {
        use std::net::IpAddr;
        // Classic SSRF bypass: ::ffff:127.0.0.1
        assert!(is_private_ip(&"::ffff:127.0.0.1".parse::<IpAddr>().unwrap()));
    }

    #[test]
    fn test_is_private_ip_v6_ipv4_mapped_metadata() {
        use std::net::IpAddr;
        // SSRF bypass: ::ffff:169.254.169.254
        assert!(is_private_ip(&"::ffff:169.254.169.254".parse::<IpAddr>().unwrap()));
    }

    #[test]
    fn test_is_private_ip_v6_documentation() {
        use std::net::IpAddr;
        assert!(is_private_ip(&"2001:db8::1".parse::<IpAddr>().unwrap()));
    }

    #[tokio::test]
    async fn test_check_ssrf_rejects_non_http_scheme() {
        // file:// and gopher:// must be rejected by the scheme whitelist
        let url = Url::parse("file:///etc/passwd").unwrap();
        assert!(check_ssrf(&url).await.is_err());
    }
}
