# Fix: Port 9090 Conflict

## Problem

```
http://0.0.0.0:9090/metrics
Forbidden
```

**Cause:** Port 9090 is already in use by another application (e.g., an Electron app).

Bot log shows:
```
[ERROR] Metrics server error: Address already in use (os error 48)
```

## Solution (Applied)

### 1. Changed metrics server port to **9094**

Updated `.env`:
```bash
METRICS_ENABLED=true
METRICS_PORT=9094  # Instead of 9090
```

### 2. Updated Prometheus configuration

File `prometheus.yml`:
```yaml
- job_name: 'doradura-bot'
  static_configs:
    - targets: ['host.docker.internal:9094']  # Instead of 9090
```

### 3. Updated documentation

- `QUICKSTART_MONITORING.md` - new URL
- `.env.example` - new default port

## How to Apply

### Step 1: Restart the bot

```bash
# Stop the current process (Ctrl+C)
cargo run --release
```

Check logs - should show:
```
[INFO] Starting metrics server on port 9094
[INFO] Metrics available at http://0.0.0.0:9094/metrics
```

### Step 2: Check the metrics endpoint

```bash
curl http://localhost:9094/metrics
curl http://localhost:9094/health
```

You should see metrics in Prometheus format.

### Step 3: Start monitoring

```bash
./scripts/start-monitoring.sh
```

Prometheus will automatically connect to the bot on port 9094.

## New URLs

| Service | Old URL | New URL |
|---------|---------|---------|
| Bot Metrics | ~~http://localhost:9090/metrics~~ | **http://localhost:9094/metrics** |
| Prometheus | http://localhost:9091 | http://localhost:9091 |
| Grafana | http://localhost:3000 | http://localhost:3000 |

## Verification

After restarting the bot and monitoring:

```bash
# Check health
./scripts/check-metrics.sh

# Check that port 9094 is being listened on
lsof -i :9094

# Check targets in Prometheus
curl http://localhost:9091/api/v1/targets | jq '.data.activeTargets[] | select(.labels.job=="doradura-bot")'
```

Should show `"health": "up"`.

## Alternative Solutions

### Option 1: Free up port 9090

Find the process on port 9090:
```bash
lsof -i :9090
```

Stop it (if not needed):
```bash
kill -9 <PID>
```

### Option 2: Use any other available port

```bash
# Check available ports
netstat -an | grep LISTEN | grep 909

# Pick an available one (e.g., 9095)
METRICS_PORT=9095
```

Do not forget to update `prometheus.yml`.

## Checklist

After applying the fix:

- [x] `.env` updated with `METRICS_PORT=9094`
- [x] `prometheus.yml` updated to port 9094
- [x] `.env.example` updated for documentation
- [x] `QUICKSTART_MONITORING.md` updated
- [ ] Bot restarted
- [ ] Metrics endpoint accessible at :9094
- [ ] Monitoring started
- [ ] Prometheus collecting metrics

## Summary

**Port changed: 9090 -> 9094**

Everything is configured and ready to use after restarting the bot.
