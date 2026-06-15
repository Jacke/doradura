//! Recommendation **ranking** — source/platform-neutral, pure, unit-testable.
//!
//! The actual fetch of candidate "radio"/related items lives in the bot (it
//! needs yt-dlp + proxy/cookies; see `dorabot::download::recommend`). This
//! module holds only the scoring/dedup/blend so it can be tested without I/O.
//!
//! Strategy: gather YouTube **Mix/Radio** (`RD<id>`) lists for several seeds
//! (the user's recent downloads). A candidate that shows up across *multiple*
//! seed mixes is a stronger taste match → score = number of mixes it appears in.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

/// A candidate recommendation (a video/track the user might want).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawRec {
    pub url: String,
    pub title: String,
    pub uploader: Option<String>,
}

/// Rank candidates gathered from one or more radio mixes.
///
/// - Score = how many mixes surfaced a url (cross-seed agreement wins).
/// - `exclude` (already-owned urls + the seeds themselves) and duplicates dropped.
/// - Ties keep first-seen order (stable). Returns at most `limit`.
pub fn rank(mixes: &[Vec<RawRec>], exclude: &HashSet<String>, limit: usize) -> Vec<RawRec> {
    let mut score: HashMap<&str, usize> = HashMap::new();
    let mut first: Vec<&RawRec> = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();

    for mix in mixes {
        for r in mix {
            if exclude.contains(&r.url) {
                continue;
            }
            *score.entry(r.url.as_str()).or_insert(0) += 1;
            if seen.insert(r.url.as_str()) {
                first.push(r);
            }
        }
    }

    // Stable sort by score desc; equal scores keep insertion (first-seen) order.
    let mut out = first;
    out.sort_by(|a, b| score[b.url.as_str()].cmp(&score[a.url.as_str()]));
    out.into_iter().take(limit).cloned().collect()
}

/// Top up `base` with `extra` candidates (e.g. global trending) that aren't
/// already present or excluded, up to `limit`. Used for cold-start / diversity.
pub fn blend_fill(mut base: Vec<RawRec>, extra: Vec<RawRec>, exclude: &HashSet<String>, limit: usize) -> Vec<RawRec> {
    let mut have: HashSet<String> = base.iter().map(|r| r.url.clone()).collect();
    for e in extra {
        if base.len() >= limit {
            break;
        }
        if exclude.contains(&e.url) || have.contains(&e.url) {
            continue;
        }
        have.insert(e.url.clone());
        base.push(e);
    }
    base.truncate(limit);
    base
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(u: &str) -> RawRec {
        RawRec {
            url: u.into(),
            title: u.into(),
            uploader: None,
        }
    }

    #[test]
    fn ranks_by_cross_mix_frequency() {
        let mixes = vec![
            vec![rec("a"), rec("b"), rec("c")],
            vec![rec("b"), rec("d")], // b appears in 2 mixes → top
            vec![rec("b"), rec("c")], // c in 2 mixes → second (first-seen before d)
        ];
        let out = rank(&mixes, &HashSet::new(), 10);
        assert_eq!(out[0].url, "b"); // score 3
        assert_eq!(out[1].url, "c"); // score 2, seen before d
        assert!(out.iter().any(|r| r.url == "a"));
    }

    #[test]
    fn excludes_owned_and_seeds_and_dups() {
        let mixes = vec![vec![rec("a"), rec("b"), rec("a")]];
        let excl: HashSet<String> = ["b".to_string()].into_iter().collect();
        let out = rank(&mixes, &excl, 10);
        assert_eq!(out.len(), 1); // b excluded, a de-duped
        assert_eq!(out[0].url, "a");
    }

    #[test]
    fn respects_limit() {
        let mixes = vec![vec![rec("a"), rec("b"), rec("c"), rec("d")]];
        assert_eq!(rank(&mixes, &HashSet::new(), 2).len(), 2);
    }

    #[test]
    fn blend_fill_tops_up_without_dups_or_excluded() {
        let base = vec![rec("a"), rec("b")];
        let extra = vec![rec("b"), rec("c"), rec("x"), rec("d")];
        let excl: HashSet<String> = ["x".to_string()].into_iter().collect();
        let out = blend_fill(base, extra, &excl, 3);
        assert_eq!(
            out.iter().map(|r| r.url.as_str()).collect::<Vec<_>>(),
            vec!["a", "b", "c"]
        );
    }
}
