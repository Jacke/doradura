//! Per-user cancellation flags for active downloads (GH #9).
//!
//! When `download_phase` starts a new download, it calls [`register`] to
//! claim a fresh `Arc<AtomicBool>` keyed by `chat_id`. The same flag is
//! threaded into the `DownloadRequest` so the yt-dlp polling loop can
//! observe it (see `crates/doracore/src/download/source/ytdlp.rs`
//! `run_ytdlp_with_progress`).
//!
//! When the user clicks "❌ Cancel" on the progress message, the
//! `CallbackKind::Cancel` handler calls [`cancel`], which flips the bool.
//! Within ≤200 ms (the yt-dlp poll interval), the subprocess is SIGKILL'd
//! and the download returns `YtDlpErrorType::Cancelled`. The pipeline
//! treats this as a non-error termination — no sticker, no admin
//! notification, just a neutral "Cancelled" status.
//!
//! `unregister` is called on `download_phase` exit (success, failure, or
//! cancellation) to keep the map small. One user = one active download
//! at a time, so we don't need any kind of nested map.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

static REGISTRY: LazyLock<Mutex<HashMap<i64, Arc<AtomicBool>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Allocate a fresh cancel flag for `chat_id`. Replaces any existing flag.
/// Caller (the pipeline) must keep the returned `Arc` alive for the whole
/// download and call [`unregister`] on completion.
pub fn register(chat_id: i64) -> Arc<AtomicBool> {
    let flag = Arc::new(AtomicBool::new(false));
    if let Ok(mut map) = REGISTRY.lock() {
        map.insert(chat_id, Arc::clone(&flag));
    }
    flag
}

/// Set the cancel flag for `chat_id` (if present). Returns `true` if a
/// download was found and signalled, `false` if nothing was active.
pub fn cancel(chat_id: i64) -> bool {
    if let Ok(map) = REGISTRY.lock()
        && let Some(flag) = map.get(&chat_id)
    {
        flag.store(true, Ordering::Relaxed);
        return true;
    }
    false
}

/// Drop the registry entry for `chat_id`. Called by `download_phase` on
/// exit so the map doesn't grow unbounded across long-lived bots.
pub fn unregister(chat_id: i64) {
    if let Ok(mut map) = REGISTRY.lock() {
        map.remove(&chat_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_returns_unset_flag() {
        let chat_id = -42_001;
        unregister(chat_id);
        let flag = register(chat_id);
        assert!(!flag.load(Ordering::Relaxed));
        unregister(chat_id);
    }

    #[test]
    fn cancel_sets_registered_flag() {
        let chat_id = -42_002;
        let flag = register(chat_id);
        assert!(cancel(chat_id));
        assert!(flag.load(Ordering::Relaxed));
        unregister(chat_id);
    }

    #[test]
    fn cancel_returns_false_when_no_active_download() {
        let chat_id = -42_003;
        unregister(chat_id);
        assert!(!cancel(chat_id));
    }

    #[test]
    fn unregister_drops_flag_so_cancel_after_is_noop() {
        let chat_id = -42_004;
        let _flag = register(chat_id);
        unregister(chat_id);
        assert!(!cancel(chat_id));
    }
}
