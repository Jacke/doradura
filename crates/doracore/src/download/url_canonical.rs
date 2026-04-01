//! URL canonicalization for aggressive cross-user file_id cache.
//!
//! Normalizes URL variants so the same content with different URL formats
//! shares the same cache entry. Applied before cache lookup and before save.

use url::Url;

/// Universal tracking parameters stripped from ALL URLs.
const UNIVERSAL_TRACKING_PARAMS: &[&str] = &[
    "utm_source",
    "utm_medium",
    "utm_campaign",
    "utm_term",
    "utm_content",
    "fbclid",
    "gclid",
    "dclid",
    "msclkid",
    "twclid",
    "igshid",
    "igsh",
    "si",
    "ref",
    "_ga",
    "_gl",
];

/// Canonicalize a URL for cache deduplication.
///
/// Returns a normalized URL string where different variants of the same
/// content produce identical output. If parsing fails, returns the input as-is.
pub fn canonicalize_url(raw: &str) -> String {
    let mut url = match Url::parse(raw) {
        Ok(u) => u,
        Err(_) => return raw.to_string(),
    };

    // 1. Remove fragment
    url.set_fragment(None);

    // 2. Platform-specific normalization (may change host, path, params)
    let host = url.host_str().unwrap_or("").to_lowercase();
    if host.contains("youtube.com") || host.contains("youtu.be") || host.contains("youtube-nocookie.com") {
        canonicalize_youtube(&mut url);
    } else if host.contains("instagram.com") {
        canonicalize_instagram(&mut url);
    } else if host.contains("tiktok.com") && !host.starts_with("vm.") && !host.starts_with("vt.") {
        canonicalize_tiktok(&mut url);
    } else if host.contains("twitter.com") || host.contains("x.com") {
        canonicalize_twitter(&mut url);
    } else if host.contains("spotify.com") {
        canonicalize_spotify(&mut url);
    } else if host.contains("soundcloud.com") && !host.starts_with("on.") {
        canonicalize_soundcloud(&mut url);
    } else if host.contains("vimeo.com") {
        canonicalize_vimeo(&mut url);
    } else if host == "vk.com" || host == "m.vk.com" || host == "vk.ru" || host == "www.vk.com" {
        canonicalize_vk(&mut url);
    } else if host.contains("reddit.com") {
        canonicalize_reddit(&mut url);
    } else if host.contains("facebook.com") {
        canonicalize_facebook(&mut url);
    } else if host.contains("twitch.tv") {
        canonicalize_twitch(&mut url);
    }

    // 3. Strip universal tracking params from ALL URLs
    strip_params(&mut url, UNIVERSAL_TRACKING_PARAMS);

    // 4. Remove trailing slash (but not bare "/")
    let path = url.path().to_string();
    if path.len() > 1 && path.ends_with('/') {
        url.set_path(&path[..path.len() - 1]);
    }

    // 5. Remove empty query string
    if url.query() == Some("") {
        url.set_query(None);
    }

    // 6. Sort remaining query params for deterministic output
    if url.query().is_some() {
        let mut pairs: Vec<(String, String)> = url
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        pairs.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        let sorted: String = pairs
            .iter()
            .map(|(k, v)| {
                if v.is_empty() {
                    k.clone()
                } else {
                    format!("{}={}", k, v)
                }
            })
            .collect::<Vec<_>>()
            .join("&");
        if sorted.is_empty() {
            url.set_query(None);
        } else {
            url.set_query(Some(&sorted));
        }
    }

    url.to_string()
}

// ── Helpers ──────────────────────────────────────────────────────────────

fn strip_params(url: &mut Url, params: &[&str]) {
    let pairs: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(k, _)| !params.contains(&k.as_ref()))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    if pairs.is_empty() {
        url.set_query(None);
    } else {
        let q: String = pairs
            .iter()
            .map(|(k, v)| {
                if v.is_empty() {
                    k.clone()
                } else {
                    format!("{}={}", k, v)
                }
            })
            .collect::<Vec<_>>()
            .join("&");
        url.set_query(Some(&q));
    }
}

fn keep_only_params(url: &mut Url, params: &[&str]) {
    let pairs: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(k, _)| params.contains(&k.as_ref()))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    if pairs.is_empty() {
        url.set_query(None);
    } else {
        let q: String = pairs
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect::<Vec<_>>()
            .join("&");
        url.set_query(Some(&q));
    }
}

fn set_host_preserve_path(url: &mut Url, new_host: &str) {
    let _ = url.set_host(Some(new_host));
}

/// Extract path segment at given index (0-based).
fn path_segment(url: &Url, idx: usize) -> Option<String> {
    url.path_segments()?.nth(idx).map(|s| s.to_string())
}

// ── YouTube ─────────────────────────────────────────────────────────────

fn canonicalize_youtube(url: &mut Url) {
    let host = url.host_str().unwrap_or("").to_lowercase();

    // Extract video ID from various URL formats
    let video_id = extract_youtube_video_id(url, &host);

    if let Some(vid) = video_id {
        // Rebuild as canonical: https://www.youtube.com/watch?v=ID
        set_host_preserve_path(url, "www.youtube.com");
        url.set_path("/watch");
        url.set_query(Some(&format!("v={}", vid)));
    } else {
        // Not a video URL (channel, playlist, etc.) — just normalize host
        if host.starts_with("m.") || host == "music.youtube.com" {
            set_host_preserve_path(url, "www.youtube.com");
        }
    }
}

fn extract_youtube_video_id(url: &Url, host: &str) -> Option<String> {
    // youtu.be/ID
    if host == "youtu.be" {
        return path_segment(url, 0).filter(|s| !s.is_empty());
    }

    let path = url.path();

    // /watch?v=ID
    if path.starts_with("/watch") {
        return url.query_pairs().find(|(k, _)| k == "v").map(|(_, v)| v.into_owned());
    }

    // /shorts/ID, /live/ID, /embed/ID
    for prefix in &["/shorts/", "/live/", "/embed/"] {
        if let Some(rest) = path.strip_prefix(prefix) {
            let id = rest.split('/').next().unwrap_or("");
            if !id.is_empty() {
                return Some(id.to_string());
            }
        }
    }

    None
}

// ── Instagram ───────────────────────────────────────────────────────────

fn canonicalize_instagram(url: &mut Url) {
    set_host_preserve_path(url, "www.instagram.com");

    // /reels/CODE/ → /reel/CODE/
    let path = url.path().to_string();
    if path.starts_with("/reels/") {
        url.set_path(&path.replacen("/reels/", "/reel/", 1));
    }

    strip_params(url, &["igsh", "igshid", "img_index", "hl", "taken-by"]);
}

// ── TikTok ──────────────────────────────────────────────────────────────

fn canonicalize_tiktok(url: &mut Url) {
    let host = url.host_str().unwrap_or("").to_lowercase();
    if host.starts_with("m.") || host == "lite.tiktok.com" {
        set_host_preserve_path(url, "www.tiktok.com");
    }

    strip_params(
        url,
        &[
            "is_from_webapp",
            "sender_device",
            "is_copy_url",
            "_t",
            "_r",
            "sec_uid",
            "tt_from",
            "source",
            "checksum",
            "u_code",
        ],
    );

    // Strip share_* params
    let pairs: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(k, _)| !k.starts_with("share_"))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    if pairs.is_empty() {
        url.set_query(None);
    } else {
        let q: String = pairs
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect::<Vec<_>>()
            .join("&");
        url.set_query(Some(&q));
    }
}

// ── Twitter / X ─────────────────────────────────────────────────────────

fn canonicalize_twitter(url: &mut Url) {
    set_host_preserve_path(url, "x.com");
    strip_params(url, &["s", "t", "ref_src", "ref_url", "cxt"]);
}

// ── Spotify ─────────────────────────────────────────────────────────────

fn canonicalize_spotify(url: &mut Url) {
    let host = url.host_str().unwrap_or("").to_lowercase();
    if host == "play.spotify.com" {
        set_host_preserve_path(url, "open.spotify.com");
    }

    // Strip /intl-{locale}/ prefix
    let path = url.path().to_string();
    if let Some(rest) = path.strip_prefix("/intl-").and_then(|p| p.find('/').map(|i| &p[i..])) {
        url.set_path(rest);
    }

    // Strip legacy /user/{id}/ prefix from playlist URLs
    let path = url.path().to_string();
    if let Some(idx) = path.find("/playlist/") {
        url.set_path(&path[idx..]);
    }

    strip_params(url, &["context", "dl_branch", "nd"]);

    // Strip _branch_* params
    let pairs: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(k, _)| !k.starts_with("_branch_"))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    if pairs.is_empty() {
        url.set_query(None);
    } else {
        let q: String = pairs
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect::<Vec<_>>()
            .join("&");
        url.set_query(Some(&q));
    }
}

// ── SoundCloud ──────────────────────────────────────────────────────────

fn canonicalize_soundcloud(url: &mut Url) {
    let host = url.host_str().unwrap_or("").to_lowercase();
    if host.starts_with("m.") || host.starts_with("www.") {
        set_host_preserve_path(url, "soundcloud.com");
    }
    strip_params(url, &["in"]);
}

// ── Vimeo ───────────────────────────────────────────────────────────────

fn canonicalize_vimeo(url: &mut Url) {
    let host = url.host_str().unwrap_or("").to_lowercase();

    // player.vimeo.com/video/ID → vimeo.com/ID
    if host == "player.vimeo.com" {
        set_host_preserve_path(url, "vimeo.com");
        let path = url.path().to_string();
        if let Some(rest) = path.strip_prefix("/video") {
            url.set_path(rest);
        }
        return;
    }

    let path = url.path().to_string();
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

    // /channels/NAME/ID → /ID
    if segments.len() >= 3 && segments[0] == "channels" && segments[2].parse::<u64>().is_ok() {
        url.set_path(&format!("/{}", segments[2]));
    }
    // /groups/NAME/videos/ID → /ID
    else if segments.len() >= 4
        && segments[0] == "groups"
        && segments[2] == "videos"
        && segments[3].parse::<u64>().is_ok()
    {
        url.set_path(&format!("/{}", segments[3]));
    }

    // Keep h= param (privacy hash), strip tracking via universal pass
    keep_only_params(url, &["h"]);
}

// ── VK ──────────────────────────────────────────────────────────────────

fn canonicalize_vk(url: &mut Url) {
    set_host_preserve_path(url, "vk.com");
    strip_params(url, &["from", "source"]);
}

// ── Reddit ──────────────────────────────────────────────────────────────

fn canonicalize_reddit(url: &mut Url) {
    let host = url.host_str().unwrap_or("").to_lowercase();
    if host == "old.reddit.com"
        || host == "new.reddit.com"
        || host == "np.reddit.com"
        || host == "i.reddit.com"
        || host == "m.reddit.com"
    {
        set_host_preserve_path(url, "www.reddit.com");
    }

    // Strip slug: /r/sub/comments/ID/slug → /r/sub/comments/ID
    let path = url.path().to_string();
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if segments.len() >= 5 && segments[0] == "r" && segments[2] == "comments" {
        // Keep: /r/{sub}/comments/{id}
        url.set_path(&format!("/r/{}/comments/{}", segments[1], segments[3]));
    }

    strip_params(
        url,
        &["context", "sort", "limit", "depth", "share_id", "ref_source", "rdt"],
    );
}

// ── Facebook ────────────────────────────────────────────────────────────

fn canonicalize_facebook(url: &mut Url) {
    let host = url.host_str().unwrap_or("").to_lowercase();
    if host.starts_with("m.")
        || host.starts_with("web.")
        || host.starts_with("touch.")
        || host.starts_with("business.")
        || host.starts_with("l.")
    {
        set_host_preserve_path(url, "www.facebook.com");
    }

    strip_params(
        url,
        &["mibextid", "__tn__", "__xts__", "_rdr", "locale", "paipv", "eav"],
    );

    // Strip __cft__* params
    let pairs: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(k, _)| !k.starts_with("__cft__"))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    if pairs.is_empty() {
        url.set_query(None);
    } else {
        let q: String = pairs
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect::<Vec<_>>()
            .join("&");
        url.set_query(Some(&q));
    }
}

// ── Twitch ──────────────────────────────────────────────────────────────

fn canonicalize_twitch(url: &mut Url) {
    let host = url.host_str().unwrap_or("").to_lowercase();

    // clips.twitch.tv/SLUG → www.twitch.tv/clip/SLUG
    if host == "clips.twitch.tv" {
        let slug = path_segment(url, 0).unwrap_or_default();
        if !slug.is_empty() {
            set_host_preserve_path(url, "www.twitch.tv");
            url.set_path(&format!("/clip/{}", slug));
        }
        return;
    }

    if host.starts_with("m.") {
        set_host_preserve_path(url, "www.twitch.tv");
    }

    strip_params(url, &["tt_medium", "tt_content", "filter", "sort"]);
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn c(url: &str) -> String {
        canonicalize_url(url)
    }

    // ── YouTube ─────────────────────────────────────────────────────

    #[test]
    fn youtube_short_link() {
        assert_eq!(
            c("https://youtu.be/dQw4w9WgXcQ"),
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ"
        );
    }

    #[test]
    fn youtube_short_link_with_tracking() {
        assert_eq!(
            c("https://youtu.be/dQw4w9WgXcQ?si=xyz&t=42"),
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ"
        );
    }

    #[test]
    fn youtube_mobile() {
        assert_eq!(
            c("https://m.youtube.com/watch?v=dQw4w9WgXcQ"),
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ"
        );
    }

    #[test]
    fn youtube_music() {
        assert_eq!(
            c("https://music.youtube.com/watch?v=dQw4w9WgXcQ&si=abc"),
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ"
        );
    }

    #[test]
    fn youtube_shorts() {
        assert_eq!(
            c("https://www.youtube.com/shorts/dQw4w9WgXcQ"),
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ"
        );
    }

    #[test]
    fn youtube_live() {
        assert_eq!(
            c("https://www.youtube.com/live/dQw4w9WgXcQ"),
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ"
        );
    }

    #[test]
    fn youtube_embed() {
        assert_eq!(
            c("https://www.youtube.com/embed/dQw4w9WgXcQ"),
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ"
        );
    }

    #[test]
    fn youtube_full_tracking_strip() {
        assert_eq!(
            c("https://www.youtube.com/watch?v=dQw4w9WgXcQ&list=PLxxx&index=2&si=abc&feature=share&pp=1"),
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ"
        );
    }

    #[test]
    fn youtube_already_canonical() {
        assert_eq!(
            c("https://www.youtube.com/watch?v=dQw4w9WgXcQ"),
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ"
        );
    }

    // ── Instagram ───────────────────────────────────────────────────

    #[test]
    fn instagram_reel() {
        assert_eq!(
            c("https://instagram.com/reel/ABC123/?igsh=xyz"),
            "https://www.instagram.com/reel/ABC123"
        );
    }

    #[test]
    fn instagram_reels_to_reel() {
        assert_eq!(
            c("https://www.instagram.com/reels/ABC123/?igshid=abc"),
            "https://www.instagram.com/reel/ABC123"
        );
    }

    // ── TikTok ──────────────────────────────────────────────────────

    #[test]
    fn tiktok_mobile_with_tracking() {
        assert_eq!(
            c("https://m.tiktok.com/@user/video/123?is_from_webapp=1&sender_device=pc"),
            "https://www.tiktok.com/@user/video/123"
        );
    }

    #[test]
    fn tiktok_short_link_passthrough() {
        let short = "https://vm.tiktok.com/abc123/";
        // Short links should NOT be modified (require HTTP redirect)
        let result = c(short);
        assert!(result.starts_with("https://vm.tiktok.com/"));
    }

    // ── Twitter / X ─────────────────────────────────────────────────

    #[test]
    fn twitter_to_x() {
        assert_eq!(
            c("https://twitter.com/user/status/123?s=20&t=xyz"),
            "https://x.com/user/status/123"
        );
    }

    #[test]
    fn twitter_mobile_to_x() {
        assert_eq!(
            c("https://mobile.twitter.com/user/status/123"),
            "https://x.com/user/status/123"
        );
    }

    #[test]
    fn x_already_canonical() {
        assert_eq!(
            c("https://x.com/user/status/123?ref_src=twsrc"),
            "https://x.com/user/status/123"
        );
    }

    // ── Spotify ─────────────────────────────────────────────────────

    #[test]
    fn spotify_intl_strip() {
        assert_eq!(
            c("https://open.spotify.com/intl-en/track/abc?si=xyz&context=123"),
            "https://open.spotify.com/track/abc"
        );
    }

    #[test]
    fn spotify_play_to_open() {
        assert_eq!(
            c("https://play.spotify.com/track/abc"),
            "https://open.spotify.com/track/abc"
        );
    }

    #[test]
    fn spotify_legacy_user_playlist() {
        assert_eq!(
            c("https://open.spotify.com/user/123/playlist/abc?si=xyz"),
            "https://open.spotify.com/playlist/abc"
        );
    }

    // ── SoundCloud ──────────────────────────────────────────────────

    #[test]
    fn soundcloud_mobile() {
        assert_eq!(
            c("https://m.soundcloud.com/artist/track?si=abc&ref=clipboard"),
            "https://soundcloud.com/artist/track"
        );
    }

    // ── Vimeo ───────────────────────────────────────────────────────

    #[test]
    fn vimeo_player_embed() {
        assert_eq!(
            c("https://player.vimeo.com/video/123456?h=abc"),
            "https://vimeo.com/123456?h=abc"
        );
    }

    #[test]
    fn vimeo_channel() {
        assert_eq!(
            c("https://vimeo.com/channels/staffpicks/123456"),
            "https://vimeo.com/123456"
        );
    }

    #[test]
    fn vimeo_group() {
        assert_eq!(
            c("https://vimeo.com/groups/cool/videos/123456"),
            "https://vimeo.com/123456"
        );
    }

    // ── Reddit ──────────────────────────────────────────────────────

    #[test]
    fn reddit_old_with_slug() {
        assert_eq!(
            c("https://old.reddit.com/r/rust/comments/abc123/my_cool_title/?utm_source=share"),
            "https://www.reddit.com/r/rust/comments/abc123"
        );
    }

    // ── Facebook ────────────────────────────────────────────────────

    #[test]
    fn facebook_mobile_with_tracking() {
        assert_eq!(
            c("https://m.facebook.com/watch?v=123&fbclid=abc&__tn__=xyz"),
            "https://www.facebook.com/watch?v=123"
        );
    }

    // ── Twitch ──────────────────────────────────────────────────────

    #[test]
    fn twitch_clips_domain() {
        assert_eq!(
            c("https://clips.twitch.tv/SomeClipSlug"),
            "https://www.twitch.tv/clip/SomeClipSlug"
        );
    }

    #[test]
    fn twitch_mobile() {
        assert_eq!(c("https://m.twitch.tv/streamer"), "https://www.twitch.tv/streamer");
    }

    // ── VK ──────────────────────────────────────────────────────────

    #[test]
    fn vk_mobile() {
        assert_eq!(c("https://m.vk.com/video123_456"), "https://vk.com/video123_456");
    }

    #[test]
    fn vk_ru() {
        assert_eq!(c("https://vk.ru/video123_456"), "https://vk.com/video123_456");
    }

    // ── General / Edge cases ────────────────────────────────────────

    #[test]
    fn universal_tracking_strip() {
        assert_eq!(
            c("https://example.com/file.mp3?utm_source=twitter&utm_medium=social&fbclid=abc"),
            "https://example.com/file.mp3"
        );
    }

    #[test]
    fn unparseable_passthrough() {
        assert_eq!(c("not-a-url"), "not-a-url");
    }

    #[test]
    fn bandcamp_unchanged() {
        assert_eq!(
            c("https://artist.bandcamp.com/track/cool-song"),
            "https://artist.bandcamp.com/track/cool-song"
        );
    }

    #[test]
    fn idempotent() {
        let urls = [
            "https://youtu.be/dQw4w9WgXcQ?si=abc",
            "https://m.youtube.com/shorts/test123",
            "https://twitter.com/u/status/1?s=20",
            "https://m.tiktok.com/@u/video/1?is_from_webapp=1",
            "https://open.spotify.com/intl-de/track/abc?si=xyz",
        ];
        for url in urls {
            let first = canonicalize_url(url);
            let second = canonicalize_url(&first);
            assert_eq!(first, second, "Not idempotent for: {}", url);
        }
    }
}
