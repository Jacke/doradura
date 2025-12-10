# 📋 Remaining Tasks

## ✅ Quick wins (minimal effort)

### 1. `panic!` in test ⚠️ very low priority
**File:** `src/downloader.rs:957`  
**Status:** Acceptable in tests but can improve the message.  
**Time:** ~1 minute
```rust
// Current (fine for tests):
Err(e) => {
    let _ = fs::remove_file(&dest_str);
    panic!("Download test failed: {:?}", e);
}

// Optional improvement:
Err(e) => {
    let _ = fs::remove_file(&dest_str);
    panic!("Download test failed for URL {}: {:?}", url, e);
}
```

---

## 🔧 Optional improvements (low priority)

### 2. URL parsing cache ❌ low priority
**Files:** `src/commands.rs:89`, `src/main.rs:307`  
- URLs are parsed on each request, but parsing is microseconds.  
- Each URL is unique, so cache has little value.  
- Validation already runs before enqueueing.  
**Recommendation:** leave as-is unless performance issues arise.

### 3. Metrics and monitoring ❌ low priority
Add production metrics if load grows. Libraries: `prometheus`, `metrics`, or `tracing` exporters. Useful metrics: request rate, queue depth, download durations, errors.

### 4. Performance tuning ❌ low priority
Profile before optimizing. Possible ideas: reuse HTTP client, tune semaphore limits, cache metadata if it proves slow.

---

## 🧭 Notes
- None of these are blockers.
- Tackle only if/when performance or observability needs increase.
