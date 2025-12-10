# Optimization Opportunities

A high-level backlog of potential performance improvements. Apply based on profiling and observed load.

## Queue and concurrency
- Tune `MAX_CONCURRENT_DOWNLOADS` for the host (separate knobs for audio/video if needed).
- Prioritize tasks by plan (VIP/Premium) and drop/age out stale tasks.
- Add queue metrics: wait time, in-flight tasks, failures.

## yt-dlp and downloads
- Prefer `android` client without cookies; `web/ios/tv_embedded` with cookies.
- Set sensible timeouts; retry transient network errors with backoff.
- Cache metadata with TTL; invalidate on error; avoid caching empty titles.
- Consider limiting fragment concurrency for slow disks/networks.

## Database
- Add indexes for frequent lookups (tasks by id, user tasks, expirations).
- Vacuum/clean old history in a background job.
- Wrap multi-step updates in transactions to reduce lock contention.

## WebApp / API
- Rate-limit Mini App endpoints; validate payloads strictly.
- Cache user settings per request to reduce DB hits.
- Return precise error messages for invalid URLs/expired sessions.

## Telemetry
- Expose Prometheus metrics: queue depth, download duration, send duration, errors, cache hit rate.
- Add tracing spans around command handling, queue pops, yt-dlp runs, and Telegram sends.

## Resilience
- Automatic recovery of stuck/failed tasks with capped retries.
- Alerting when failure rate spikes or when queue backlog grows.

## Storage
- Store temp files on fast disk; ensure cleanup jobs delete old artifacts.
- Validate free space before downloads; fail early with a clear message.

## Not priorities right now
- Micro-optimizing URL parsing/regex.
- Premature caching of uncommon DB queries without evidence of latency.

Start with measurement (metrics + short load test), then iterate on the highest-impact areas.
