# Multi-Instance Rollout Plan

## Goal

Roll out `doradura` as a `multi-instance monolith` with:

- `PostgreSQL` as canonical shared runtime state
- `Redis` for distributed rate limiting
- `Telegram webhook` behind a single public endpoint
- `SQLite` preserved for local and single-instance fallback

This plan assumes PR `#16` and PR `#17` are merged.

## Preconditions

Before rollout, verify:

1. `cargo test --workspace --quiet` is green on the merge commit.
2. PostgreSQL is reachable from all instances.
3. Redis is reachable from all instances.
4. `setWebhook` is managed centrally, not per-instance at runtime.
5. The public webhook URL is stable and served behind TLS.

## Required Environment

Multi-instance production must set:

- `DATABASE_DRIVER=postgres`
- `DATABASE_URL=postgres://...`
- `REDIS_URL=redis://...`
- `WEBHOOK_URL=https://bot.example.com`
- `WEBHOOK_SECRET_TOKEN=...`
- `WEBHOOK_PATH=/telegram/webhook`
- `WEBHOOK_LISTEN_ADDR=0.0.0.0:8080`
- `BOT_TOKEN=...`

Recommended:

- `WEBHOOK_MAX_CONNECTIONS=40`
- `QUEUE_MAX_CONCURRENT=2`
- `TEMP_FILES_DIR=/tmp`
- `DOWNLOAD_FOLDER=/tmp/dora-files`

## Topology

Target topology:

```text
Telegram
  -> HTTPS webhook
  -> Load Balancer / Reverse Proxy
  -> N identical doradura instances
  -> PostgreSQL
  -> Redis
```

Rules:

- Do not use long polling in multi-instance production.
- Do not expose multiple webhook URLs for one bot.
- Do not let instances call `deleteWebhook` on shutdown.

## Rollout Sequence

### Phase 1: Data Plane Bring-Up

1. Provision PostgreSQL.
2. Provision Redis.
3. Apply application startup once against PostgreSQL to bootstrap shared tables.
4. Verify these tables exist:
   - `processed_updates`
   - `task_queue`
   - `users`
   - `subscriptions`
   - `url_cache`
   - `alert_history`
   - `error_log`

### Phase 2: Single Instance on Postgres

1. Deploy exactly one instance with:
   - `DATABASE_DRIVER=postgres`
   - `REDIS_URL` set
   - webhook mode enabled
2. Set the Telegram webhook once:
   - `WEBHOOK_URL`
   - `WEBHOOK_SECRET_TOKEN`
   - `WEBHOOK_MAX_CONNECTIONS`
3. Run smoke checks:
   - `/start`
   - regular download
   - preview -> callback -> download
   - playlist flow
   - admin user panel
   - content subscription notification

Success criteria:

- webhook requests return `200`
- `processed_updates` fills
- `task_queue` leases advance correctly
- Redis rate limiting works

### Phase 3: Add Second Instance

1. Scale from `1 -> 2` instances.
2. Keep the same webhook URL and same secret token.
3. Verify:
   - only one instance holds singleton jobs
   - content watcher runs on one instance
   - cookies checker runs on one instance
   - stats reporter runs on one instance
   - alert monitor runs on one instance
4. Force callback-heavy flows across instances:
   - preview callbacks
   - voice effects
   - history resend
   - playlist/player flows

Success criteria:

- no duplicate watcher notifications
- no duplicate admin cookie warnings
- no callback expiry caused by instance-local state

### Phase 4: Scale Out

1. Increase to the target instance count.
2. Watch:
   - webhook non-2xx rate
   - queue depth
   - lease reclaim count
   - Redis connectivity
   - Telegram 429s
   - disk pressure

## Verification Checklist

Operational checks after rollout:

1. `processed_updates` increases during traffic.
2. `task_queue` has no permanently stuck `leased` rows.
3. `lease_expires_at` is moving for active work.
4. `url_cache` is shared and expiring normally.
5. Only one instance logs singleton-job startup messages.
6. Redis keys for rate limiting appear and expire normally.
7. Duplicate webhook deliveries do not create duplicate side effects.

## Rollback

If the multi-instance deployment misbehaves:

1. Scale down to one instance.
2. Keep PostgreSQL and Redis in place.
3. Keep webhook mode enabled.
4. If needed, switch runtime back to SQLite single-instance only in a separate rollback release.

Do not:

- run polling and webhook together
- point Telegram to a new webhook URL during partial rollback unless necessary
- run multiple production instances with `DATABASE_DRIVER=sqlite`

## Post-Rollout Hardening

After production is stable:

1. Add Postgres + Redis integration jobs to CI.
2. Add deployment automation for webhook management.
3. Add dashboards for:
   - webhook latency
   - queue lease churn
   - singleton lock ownership
   - Redis rate-limit errors
