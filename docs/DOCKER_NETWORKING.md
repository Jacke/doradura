# Docker Networking: Access Between Containers and the Host

## Problem

How do you access services on the host machine from inside a Docker container?

```
localhost:9094      # Does not work from a container (refers to the container itself)
127.0.0.1:9094      # Also does not work
```

## Solutions

### macOS and Windows (Docker Desktop)

Use the special DNS name:

```yaml
host.docker.internal:9094
```

**This automatically resolves to the IP address of the host machine.**

#### Verify from inside a container

```bash
# Start a temporary container
docker run --rm -it alpine sh

# Inside the container:
ping host.docker.internal
curl http://host.docker.internal:9094/health
```

### Linux

On Linux there are 3 options:

#### Option 1: `host.docker.internal` via extra_hosts (Used)

Add to `docker-compose.yml`:

```yaml
services:
  prometheus:
    extra_hosts:
      - "host.docker.internal:host-gateway"
```

**Already configured!** `host.docker.internal` now works on Linux too.

#### Option 2: Docker bridge IP address

```bash
# Find docker0 IP
ip addr show docker0

# Usually:
172.17.0.1
```

In `prometheus.yml`:
```yaml
- targets: ['172.17.0.1:9094']
```

#### Option 3: Host network mode

```yaml
services:
  prometheus:
    network_mode: host  # Use the host network directly
```

**Note**: In this mode, port 9091 may conflict with port 9090 on the host.

---

## Network Access Map

### From Host to Containers

```
localhost:9091  -> Prometheus (port forwarded)
localhost:3000  -> Grafana (port forwarded)
localhost:9093  -> AlertManager (port forwarded)
```

Works via `ports` in docker-compose:
```yaml
ports:
  - "9091:9090"  # host:container
```

### From Container to Host

```
# macOS/Windows:
host.docker.internal:9094  -> Bot metrics server

# Linux (with extra_hosts):
host.docker.internal:9094  -> Bot metrics server

# Linux (without extra_hosts):
172.17.0.1:9094  -> Bot metrics server
```

### Between Containers

Use service names:

```yaml
# Prometheus -> Grafana
prometheus:9090

# Grafana -> Prometheus
prometheus:9090

# Any -> AlertManager
alertmanager:9093
```

Works via Docker DNS inside the `monitoring` network:
```yaml
networks:
  monitoring:
    driver: bridge
```

---

## Your Current Architecture

```
+-------------------------------------------------------------+
|                      HOST MACHINE                            |
|                                                               |
|  Bot (Rust) :9094                                            |
|      ^                                                        |
|      | host.docker.internal:9094                             |
|      |                                                        |
|  +---+------------------------------------------------------+ |
|  |              Docker Network: monitoring                   | |
|  |                                                           | |
|  |  +--------------+  +--------------+  +------------+      | |
|  |  | Prometheus   |  |   Grafana    |  |AlertManager|      | |
|  |  |   :9090      |  |    :3000     |  |   :9093    |      | |
|  |  | (internal)   |  |  (internal)  |  | (internal) |      | |
|  |  +------+-------+  +--------------+  +------------+      | |
|  |         |                                                  | |
|  |         | Scrapes: host.docker.internal:9094              | |
|  |         +-----------------------------------------------+ | |
|  |                                                           | |
|  +-----------------------------------------------------------+ |
|                                                               |
|  Port Mappings (accessible from outside):                    |
|    :9091 -> Prometheus:9090                                  |
|    :3000 -> Grafana:3000                                     |
|    :9093 -> AlertManager:9093                                |
+-------------------------------------------------------------+

Browser:
  http://localhost:9091 -> Prometheus UI
  http://localhost:3000 -> Grafana UI
  http://localhost:9093 -> AlertManager UI
```

---

## Diagnostics

### Check that the bot is listening on the correct interface

```bash
# The bot should listen on 0.0.0.0, not 127.0.0.1
lsof -i :9094

# Should show:
# *:9094 (LISTEN)  <- good, listening on all interfaces
#
# Should NOT show:
# 127.0.0.1:9094 (LISTEN)  <- bad, localhost only
```

Check in the metrics_server code:
```rust
// Correct
let addr = SocketAddr::from(([0, 0, 0, 0], port));

// Incorrect
let addr = SocketAddr::from(([127, 0, 0, 1], port));
```

### Check accessibility from inside a container

```bash
# Start a shell in the Prometheus container
docker exec -it doradura-prometheus sh

# Inside the container:
# Check that host.docker.internal resolves
getent hosts host.docker.internal

# Check metrics accessibility
wget -O- http://host.docker.internal:9094/metrics
# or
curl http://host.docker.internal:9094/metrics
```

### Check targets in Prometheus

```bash
# From the host
curl http://localhost:9091/api/v1/targets | jq '.data.activeTargets[] | select(.labels.job=="doradura-bot")'

# Should show:
{
  "health": "up",
  "labels": {
    "instance": "doradura-bot",
    "job": "doradura-bot"
  },
  "lastScrape": "2025-12-14T10:00:00Z",
  "scrapeUrl": "http://host.docker.internal:9094/metrics"
}
```

### Check Prometheus logs

```bash
docker logs doradura-prometheus

# If there are connection errors:
# "context deadline exceeded" -> bot is unreachable
# "connection refused" -> port is closed or wrong
# "no such host" -> DNS is not resolving
```

---

## Common Issues

### 1. "Connection refused" from a container

**Cause**: Bot is listening only on 127.0.0.1

**Solution**: Make sure the bot listens on `0.0.0.0:9094`

```rust
// src/core/metrics_server.rs
let addr = SocketAddr::from(([0, 0, 0, 0], port));
```

### 2. "No such host: host.docker.internal" on Linux

**Cause**: On Linux this name does not work out of the box

**Solution**: Use `extra_hosts` (already added to docker-compose.yml):
```yaml
extra_hosts:
  - "host.docker.internal:host-gateway"
```

### 3. Firewall blocking

**macOS/Linux**: Check firewall rules

```bash
# macOS
sudo /usr/libexec/ApplicationFirewall/socketfilterfw --listapps

# Linux (ufw)
sudo ufw status
```

**Solution**: Allow incoming connections on port 9094

### 4. Incorrect prometheus.yml configuration

```yaml
# Incorrect
- targets: ['localhost:9094']

# Correct
- targets: ['host.docker.internal:9094']
```

---

## Production: Railway

On Railway, services communicate over the internal network:

### Internal Domains

```yaml
# prometheus.yml for Railway
scrape_configs:
  - job_name: 'doradura-bot'
    static_configs:
      - targets: ['doradura-bot.railway.internal:9094']
      # Or if in the same project:
      - targets: ['doradura-bot:9094']
```

Railway automatically creates DNS records for services.

### Verification on Railway

```bash
# In the service terminal
railway run bash

# Inside:
curl http://doradura-bot.railway.internal:9094/metrics
```

---

## Checklist

### Development (Local)

- [x] `extra_hosts` added to docker-compose.yml
- [x] `prometheus.yml` uses `host.docker.internal:9094`
- [ ] Bot listens on `0.0.0.0:9094` (not `127.0.0.1`)
- [ ] Port 9094 is not blocked by firewall
- [ ] `curl http://localhost:9094/metrics` works from the host
- [ ] Targets in Prometheus show "up"

### Production (Railway)

- [ ] Use internal domain: `doradura-bot.railway.internal`
- [ ] Or service name: `doradura-bot`
- [ ] Do not use `host.docker.internal` in production

---

## Best Practices

1. **Development**: Use `host.docker.internal` with `extra_hosts`
2. **Production**: Use internal service names
3. **Metrics Server**: Always listen on `0.0.0.0`, not on `127.0.0.1`
4. **Docker Networks**: Use bridge network for isolation
5. **Port Mapping**: Forward only necessary ports

---

## Useful Links

- [Docker Networking Docs](https://docs.docker.com/network/)
- [Docker Desktop Networking](https://docs.docker.com/desktop/networking/)
- [Railway Private Networking](https://docs.railway.app/reference/private-networking)

---

## Summary

**The current configuration works on:**
- macOS (Docker Desktop)
- Windows (Docker Desktop)
- Linux (via `extra_hosts`)

**Setup:**
- Prometheus scrapes: `host.docker.internal:9094`
- Works cross-platform
- No manual IP address configuration required
