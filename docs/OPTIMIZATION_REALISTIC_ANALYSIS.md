# Performance Analysis (Realistic Expectations)

## Current state
- Download pipeline is dominated by yt-dlp and network I/O; CPU-bound work is minimal.
- Queue uses a semaphore to cap concurrent downloads; defaults are safe for typical hardware.
- SQLite handles current load; most operations are short-lived.

## Observations
- Main latency sources: remote video fetch, file upload to Telegram, and metadata retrieval.
- Progress updates can add API chatter; keep debounced.
- Cache TTLs affect repeated downloads; shorter TTLs reduce stale metadata but increase yt-dlp calls.

## Practical optimizations
1. **Concurrency tuning**
   - Adjust `MAX_CONCURRENT_DOWNLOADS` based on CPU/network capacity.
   - Use separate limits for video vs audio if needed.
2. **Retry strategy**
   - Exponential backoff for Telegram send/yt-dlp transient errors.
   - Cap retries to avoid queue clogging.
3. **Caching**
   - Metadata cache with TTL; invalidate on errors.
   - Optional in-memory queue position cache for status endpoints.
4. **I/O considerations**
   - Keep temp files on fast disk; ensure cleanup jobs run.
   - Stream uploads where possible.
5. **Metrics for tuning**
   - Track download duration, upload duration, failures, queue wait time, and cache hit rate.

## Not recommended (low ROI)
- Premature micro-optimizations of URL parsing or small allocations.
- Aggressive DB caching without evidence of contention.

## Next steps
- Add basic metrics to measure current throughput.
- Run a short load test to choose sensible concurrency values.
- Revisit settings after observing production traffic patterns.
