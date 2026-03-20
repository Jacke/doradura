# Health Check & Avatar Status System

## Architecture Overview

The bot uses a **two-layer** health monitoring approach:

1. **Internal checks** (inside the bot process) — `startup.rs` sets the online avatar on boot; `scheduler.rs` runs periodic smoke tests. On graceful shutdown, the bot sets the offline avatar.

2. **External health monitor** (separate binary) — `health-monitor` runs as an independent s6-overlay service. It periodically pings the bot's `/health` endpoint and switches the avatar on status transitions. This covers **crash scenarios** (OOM, kill -9, panic) where the bot dies without running graceful shutdown.

## How the Health Monitor Works

### State Machine

```
Start
  |
  v
Sleep(startup_delay)        # default 60s — wait for bot to start
  |
  v
is_online = false
failures = fail_threshold   # start assuming bot is DOWN
  |
  v
+---> GET /health (10s timeout)
|       |
|       +-- 200 + "healthy" --> failures = 0
|       |                       if !is_online: set_avatar(ONLINE), is_online = true
|       |
|       +-- error/unhealthy --> failures++
|                               if failures >= threshold && is_online:
|                                 set_avatar(OFFLINE), is_online = false
|       |
+--- sleep(interval) <---------+
```

Key behaviors:
- Starts assuming the bot is **down** — only sets online after first successful check
- Requires `fail_threshold` consecutive failures before switching to offline (avoids flapping)
- Only calls `setMyProfilePhoto` on **transitions**, not every check cycle

### Failure Threshold

Default threshold is **3 consecutive failures**. With a 30s interval, the bot must be unhealthy for ~90 seconds before the avatar switches to offline. This prevents transient hiccups (GC pause, brief overload) from triggering unnecessary avatar changes.

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `TELOXIDE_TOKEN` / `BOT_TOKEN` | *(required)* | Telegram bot token |
| `BOT_API_URL` | `http://localhost:8081` | Local Bot API server URL |
| `HEALTH_MONITOR_HEALTH_URL` | `http://localhost:9090/health` | Bot health endpoint URL |
| `HEALTH_MONITOR_INTERVAL_SECS` | `30` | Seconds between health checks |
| `HEALTH_MONITOR_FAIL_THRESHOLD` | `3` | Consecutive failures before offline |
| `HEALTH_MONITOR_STARTUP_DELAY_SECS` | `60` | Wait before first health check |

## Avatar Switching

The monitor uses the Telegram Bot API method `setMyProfilePhoto` to upload a PNG avatar on status transitions.

- **Online avatar**: `assets/avatar/online.png`
- **Offline avatar**: `assets/avatar/offline.png`

Both images are embedded into the binary at compile time via `include_bytes!`.

### Rate Limits

Telegram imposes rate limits on `setMyProfilePhoto`. In practice this is not an issue because:
- The monitor only calls the API on **transitions** (healthy→unhealthy or vice versa)
- Normal operation: 1 call at startup (→ online), 0 calls during steady state
- Crash scenario: 1 call when bot dies (→ offline), 1 call when bot recovers (→ online)

## Coverage Matrix

| Scenario | Covered By | Avatar Result |
|----------|-----------|---------------|
| Normal startup | Bot (`startup.rs`) + Monitor | Online |
| Graceful shutdown (SIGTERM) | Bot (`startup.rs`) | Offline |
| Crash (OOM, kill -9, panic) | Monitor | Offline (after ~90s) |
| Bot restart after crash | Monitor | Online |
| Periodic health check | Bot (`scheduler.rs`) | N/A (internal) |
| Bot API server down | Neither | Avatar unchanged |

## Race Condition Note

Both the bot and the monitor can call `setMyProfilePhoto`. This is harmless — they set the **same avatar** for the **same state**. The monitor is the fallback for when the bot cannot act. No synchronization is needed.

## Deployment

The health monitor is built as a separate binary (`health-monitor`) and runs as an s6-overlay `longrun` service with a dependency on `doradura-bot`. It starts after the bot service and inherits environment variables via `S6_KEEP_ENV=1`.

## Troubleshooting

### Avatar not changing to offline after crash
- Check `HEALTH_MONITOR_HEALTH_URL` points to the correct endpoint
- Verify the monitor is running: check logs for `[health-monitor]` entries
- The startup delay (default 60s) may not have elapsed yet
- Check if Bot API server is accessible from the monitor

### Avatar not changing to online after recovery
- The bot itself also sets the online avatar on startup, so the monitor is a backup
- Check for Telegram API rate limit errors in logs
- Verify `BOT_API_URL` is correct

### Monitor logs
Filter container logs for `[health-monitor]` prefix to see monitor-specific output.
