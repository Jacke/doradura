//! Recommendation **fetch** — gathers candidate recs via YouTube Mix/Radio
//! (`RD<id>`, YouTube's own "up-next" algorithm) for a set of seeds, then ranks
//! them with the pure logic in [`doracore::recommend`].
//!
//! Seeds = the user's recent downloads → strong taste signal. Cross-seed
//! agreement (a candidate appearing in several seed mixes) ranks highest.
//! Falls back to global trending (`popular_files`) for cold-start / to fill.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use doracore::recommend::{self, RawRec};
use tokio::process::Command as TokioCommand;
use tokio::time::timeout;

use crate::core::config;
use crate::download::search::{YtdlpFlatEntry, append_proxy_args};
use crate::storage::SharedStorage;

const RADIO_TIMEOUT_SECS: u64 = 45;
/// How many recent downloads to seed the radio from.
const SEED_COUNT: usize = 3;
/// How many candidates to pull per seed mix.
const PER_SEED: usize = 12;

/// Extract a YouTube video id from a watch / youtu.be / shorts URL. `None` for
/// non-YouTube URLs (they get no radio).
pub fn youtube_id(url: &str) -> Option<String> {
    let take = |s: &str| {
        s.split(['?', '&', '/', '#'])
            .next()
            .map(str::to_string)
            .filter(|x| !x.is_empty())
    };
    if let Some(rest) = url.split("youtu.be/").nth(1) {
        return take(rest);
    }
    if let Some(rest) = url.split("shorts/").nth(1) {
        return take(rest);
    }
    if url.contains("youtube.com")
        && let Some(rest) = url.split("v=").nth(1)
    {
        return rest.split('&').next().map(str::to_string).filter(|x| !x.is_empty());
    }
    None
}

/// Normalize a flat-playlist entry to a full watchable URL (yt-dlp often returns
/// the bare video id in `url` for `--flat-playlist`).
fn entry_url(e: &YtdlpFlatEntry) -> Option<String> {
    let u = e.webpage_url.as_deref().or(e.url.as_deref())?;
    if u.starts_with("http") {
        Some(u.to_string())
    } else {
        Some(format!("https://www.youtube.com/watch?v={u}"))
    }
}

/// Fetch the YouTube Mix/Radio for one seed → candidate recs (best-effort; empty
/// on any failure so one bad seed never sinks the batch).
async fn radio_for(seed_url: &str, limit: usize) -> Vec<RawRec> {
    let Some(id) = youtube_id(seed_url) else {
        return Vec::new();
    };
    let radio_url = format!("https://www.youtube.com/watch?v={id}&list=RD{id}");

    let mut args: Vec<String> = vec![
        "--flat-playlist".into(),
        "--dump-json".into(),
        "--no-warnings".into(),
        "--no-check-certificate".into(),
        "--playlist-end".into(),
        limit.to_string(),
    ];
    if let Some(cookies) = config::YTDL_COOKIES_FILE.as_deref() {
        args.push("--cookies".into());
        args.push(cookies.to_string());
    }
    append_proxy_args(&mut args);
    args.push(radio_url);

    let ytdl_bin = &*config::YTDL_BIN;
    let output = match timeout(
        Duration::from_secs(RADIO_TIMEOUT_SECS),
        TokioCommand::new(ytdl_bin).args(&args).output(),
    )
    .await
    {
        Ok(Ok(o)) if o.status.success() => o,
        Ok(Ok(o)) => {
            log::warn!(
                "radio_for {}: yt-dlp failed: {}",
                seed_url,
                String::from_utf8_lossy(&o.stderr).lines().next().unwrap_or("")
            );
            return Vec::new();
        }
        Ok(Err(e)) => {
            log::warn!("radio_for {}: spawn failed: {}", seed_url, e);
            return Vec::new();
        }
        Err(_) => {
            log::warn!("radio_for {}: timed out", seed_url);
            return Vec::new();
        }
    };

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| {
            let e: YtdlpFlatEntry = serde_json::from_str(l).ok()?;
            let url = entry_url(&e)?;
            Some(RawRec {
                url,
                title: e.title.clone().unwrap_or_default(),
                uploader: e.artist().map(String::from),
            })
        })
        .collect()
}

/// Build personalized recommendations for a user: radio from their recent
/// downloads, ranked by cross-seed agreement, excluding what they already have,
/// topped up with global trending. Returns at most `limit`.
pub async fn recommend_for_user(storage: &Arc<SharedStorage>, user_id: i64, limit: usize) -> Vec<RawRec> {
    let history = storage
        .get_download_history(user_id, Some(40))
        .await
        .unwrap_or_default();

    // Seeds: recent distinct YouTube URLs.
    let mut seeds: Vec<String> = Vec::new();
    let mut seed_seen: HashSet<String> = HashSet::new();
    for h in &history {
        if youtube_id(&h.url).is_some() && seed_seen.insert(h.url.clone()) {
            seeds.push(h.url.clone());
            if seeds.len() >= SEED_COUNT {
                break;
            }
        }
    }

    // Exclude everything already downloaded + the seeds themselves.
    let mut exclude: HashSet<String> = history.iter().map(|h| h.url.clone()).collect();
    exclude.extend(seeds.iter().cloned());

    // Fetch radios (sequential — keeps deps simple; ~a few seeds).
    let mut mixes: Vec<Vec<RawRec>> = Vec::with_capacity(seeds.len());
    for s in &seeds {
        mixes.push(radio_for(s, PER_SEED).await);
    }
    let mut ranked = recommend::rank(&mixes, &exclude, limit);

    // Cold-start / fill from global trending (popular_files).
    if ranked.len() < limit {
        let pop = storage.top_popular_files((limit * 2) as u32).await.unwrap_or_default();
        let extra: Vec<RawRec> = pop
            .into_iter()
            .map(|p| RawRec {
                url: p.url,
                title: p.title.unwrap_or_default(),
                uploader: p.author,
            })
            .collect();
        ranked = recommend::blend_fill(ranked, extra, &exclude, limit);
    }

    ranked
}

/// Recommendations seeded from a **single** video (the "🎧 More like this"
/// button). Returns the YouTube Mix for `seed_url`, minus the seed itself.
pub async fn similar_to(seed_url: &str, limit: usize) -> Vec<RawRec> {
    let seed_id = youtube_id(seed_url);
    let mut recs = radio_for(seed_url, limit + 1).await;
    recs.retain(|r| youtube_id(&r.url) != seed_id);
    recs.truncate(limit);
    recs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_youtube_id_from_forms() {
        assert_eq!(
            youtube_id("https://www.youtube.com/watch?v=9TGhIqmpdgo").as_deref(),
            Some("9TGhIqmpdgo")
        );
        assert_eq!(
            youtube_id("https://youtu.be/9TGhIqmpdgo?si=x").as_deref(),
            Some("9TGhIqmpdgo")
        );
        assert_eq!(
            youtube_id("https://www.youtube.com/shorts/abc123/").as_deref(),
            Some("abc123")
        );
        assert_eq!(
            youtube_id("https://www.youtube.com/watch?list=RD&v=xyz").as_deref(),
            Some("xyz")
        );
        assert_eq!(youtube_id("https://soundcloud.com/x/y"), None);
    }

    #[test]
    fn entry_url_normalizes_bare_id() {
        let e = YtdlpFlatEntry {
            title: None,
            uploader: None,
            channel: None,
            webpage_url: None,
            url: Some("abc123".into()),
            duration: None,
            thumbnail: None,
        };
        assert_eq!(entry_url(&e).as_deref(), Some("https://www.youtube.com/watch?v=abc123"));
    }
}
