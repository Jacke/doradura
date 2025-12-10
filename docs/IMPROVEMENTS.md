# ⚙️ Technical Improvements (Backlog)

A consolidated list of potential enhancements for Doradura. Priorities reflect production impact.

## High priority
- **Metrics & monitoring:** expose Prometheus metrics (queue depth, active users, download durations, error rates, CPU/memory). Create Grafana dashboards.
- **Reliability of downloads:** tighten retries/timeouts around yt-dlp; better handling of transient network errors; clear alerts when failures spike.
- **Subscription flows:** ensure renewal/expiry jobs are resilient; add audit logs for plan changes; notify users before expiry.

## Medium priority
- **Performance tuning:** profile queue processing and ffmpeg/yt-dlp calls; consider caching metadata when effective; tune semaphore limits for concurrency.
- **DB resilience:** add more indices where needed; background vacuum/cleanup; graceful handling of SQLite locks.
- **WebApp polish:** tighter validation of WebApp data; clearer error responses; rate limits on Mini App endpoints.
- **Caching strategy:** invalidate cache on errors; consider shorter TTLs for metadata; optional manual cache clear command.

## Low priority / nice-to-have
- **Config from file:** allow loading config via file or env overrides for easier deploys.
- **Telemetry:** structured tracing spans around main flows (commands, queue, yt-dlp) with correlation IDs.
- **UX tweaks:** richer progress UI, better fallback texts when metadata is missing, optional English locale.
- **Admin tooling:** simple admin dashboard for active queue, user plans, and failed tasks.

## Testing
- Expand integration tests for edge cases (long videos, expired cookies, network interruptions).
- Add load/soak tests for the queue and telegram send limits.

## Security
- Periodically rotate cookies/tokens; ensure secrets are not logged.
- Harden webhook mode (TLS, signature checks) if enabled.

These items can be picked incrementally based on time and production needs.
