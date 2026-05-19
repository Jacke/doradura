//! Anti-spam for guest_message: caps per (chat_id, user_id) tuple at 5 reqs
//! per minute. In-memory DashMap — bot restart resets the counter, which is
//! fine for this use case (the goal is to deter accidental hammering, not
//! enforce a billable quota).

use std::sync::LazyLock;

use dashmap::DashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_PER_MINUTE: u32 = 5;

/// (chat_id, user_id, minute_bucket) → request count
static COUNTERS: LazyLock<DashMap<(i64, i64, u64), AtomicU32>> = LazyLock::new(DashMap::new);

/// Returns `true` if this request is allowed, `false` if the caller is throttled.
pub fn check(chat_id: i64, user_id: i64) -> bool {
    let bucket = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() / 60)
        .unwrap_or(0);
    // Light cleanup: drop entries from buckets older than 2 minutes to keep
    // the map bounded under churn. Done lazily on each check rather than via
    // a background task so we don't add a tokio dependency edge here.
    COUNTERS.retain(|(_, _, b), _| *b + 2 > bucket);

    let key = (chat_id, user_id, bucket);
    let counter = COUNTERS.entry(key).or_insert_with(|| AtomicU32::new(0));
    let prev = counter.fetch_add(1, Ordering::Relaxed);
    prev < MAX_PER_MINUTE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn within_limit_returns_true() {
        // Different chat per test to avoid bleed across runs.
        let chat = -1_000_000_001_i64;
        let user = 1_001_i64;
        for _ in 0..MAX_PER_MINUTE {
            assert!(check(chat, user));
        }
    }

    #[test]
    fn over_limit_returns_false() {
        let chat = -1_000_000_002_i64;
        let user = 1_002_i64;
        for _ in 0..MAX_PER_MINUTE {
            assert!(check(chat, user));
        }
        // 6th call in the same minute → throttled.
        assert!(!check(chat, user));
    }

    #[test]
    fn separate_users_have_separate_limits() {
        let chat = -1_000_000_003_i64;
        for _ in 0..MAX_PER_MINUTE {
            assert!(check(chat, 100));
        }
        // Different user_id → independent counter.
        assert!(check(chat, 200));
    }
}
