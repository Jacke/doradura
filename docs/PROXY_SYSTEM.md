# Proxy System for yt-dlp

## Overview

The Doradura bot supports proxy downloads through yt-dlp. This system provides:

- **Multiple proxy protocols**: HTTP, HTTPS, SOCKS5
- **Smart proxy selection**: Round-robin, random, weighted, or fixed strategies
- **Proxy rotation**: Use different proxies for each download
- **Health tracking**: Monitor proxy success rates and auto-skip bad proxies
- **Dynamic proxy lists**: Load proxies from files or URLs
- **Thread-safe management**: Async-compatible proxy handling

## Why Use Proxies?

Proxies help with:
- **Bypass regional restrictions**: Access geo-blocked content
- **Avoid rate limiting**: Distribute requests across multiple IPs
- **IP rotation**: Prevent detection as bot
- **Load balancing**: Spread traffic across multiple servers
- **Privacy**: Hide your real IP address

## Quick Start

### 1. WARP Proxy (Recommended)

The primary proxy method uses Cloudflare WARP:

```bash
export WARP_PROXY="socks5://127.0.0.1:40000"
cargo run -- run
```

### 2. Proxy File

Create `proxies.txt`:
```
http://127.0.0.1:8080
http://127.0.0.1:8081
socks5://127.0.0.1:1080
http://user:pass@proxy.example.com:3128
https://proxy.example.com:8443
```

Use it:
```bash
export PROXY_FILE=/path/to/proxies.txt
cargo run -- run
```

### 3. Configure Selection Strategy

```bash
# Round-robin (default): rotate through proxies in order
export PROXY_STRATEGY=round_robin

# Random: pick random proxy each time
export PROXY_STRATEGY=random

# Weighted: proxies with higher weight are used more
export PROXY_STRATEGY=weighted

# Fixed: always use first proxy
export PROXY_STRATEGY=fixed

cargo run -- run
```

## Configuration

### Environment Variables

| Variable | Description | Default | Example |
|----------|-------------|---------|---------|
| `WARP_PROXY` | Cloudflare WARP proxy URL | empty | `socks5://127.0.0.1:40000` |
| `PROXY_FILE` | File path with proxies (one per line) | empty | `/etc/proxies.txt` |
| `PROXY_STRATEGY` | Selection strategy | `round_robin` | `random`, `weighted`, `fixed` |
| `PROXY_ROTATION_ENABLED` | Enable proxy rotation per download | `true` | `true` / `false` |
| `PROXY_MIN_HEALTH` | Minimum proxy health score (0-1) | `0.5` | `0.7` (70% success rate) |
| `PROXY_UPDATE_URL` | URL to fetch proxy list from | empty | `https://proxy-api.example.com/list` |
| `PROXY_UPDATE_INTERVAL` | Update interval in seconds | `3600` | `1800` (30 minutes) |

### Priority Order

Proxies are loaded in this order:
1. `WARP_PROXY` environment variable (highest priority)
2. `PROXY_FILE` file path
3. Dynamic URL from `PROXY_UPDATE_URL` (periodically)

## Proxy Format

Supported formats:

**Without authentication:**
```
http://127.0.0.1:8080
https://proxy.example.com:8443
socks5://proxy.example.com:1080
```

**With authentication:**
```
http://username:password@proxy.example.com:8080
socks5://user:pass@proxy.example.com:1080
https://apikey:@proxy.example.com:3128
```

## Selection Strategies

### Round-Robin (Default)
- Cycles through proxies in order
- Best for: Load balancing across similar proxies
- Deterministic and fair

### Random
- Randomly picks proxy each time
- Best for: Unpredictable behavior, avoiding patterns
- Good for evading detection

### Weighted
- Proxies with higher weight are used more often
- Best for: Some proxies are better than others

### Fixed
- Always uses first proxy
- Best for: Testing, single proxy setup

## Health Tracking

The system monitors proxy success rates:

**Automatic tracking:**
- Counts successful downloads per proxy
- Counts failed downloads per proxy
- Tracks total bytes downloaded per proxy

**Auto-skip unhealthy proxies:**
```bash
# Skip proxies with <50% success rate (default)
export PROXY_MIN_HEALTH=0.5

# Only use proxies with >80% success rate
export PROXY_MIN_HEALTH=0.8
```

## Usage Examples

### Example 1: WARP Proxy (Recommended)

```bash
export WARP_PROXY="socks5://127.0.0.1:40000"
cargo run -- run
```

All downloads go through Cloudflare WARP.

### Example 2: Proxies from File

Create `~/proxies.txt`:
```
http://user1:pass1@proxy1.example.com:8080
http://user2:pass2@proxy2.example.com:8080
socks5://proxy3.example.com:1080
```

Setup:
```bash
export PROXY_FILE=~/proxies.txt
export PROXY_STRATEGY=random
cargo run -- run
```

### Example 3: High-Reliability Setup

```bash
export WARP_PROXY="socks5://127.0.0.1:40000"
export PROXY_FILE=/etc/proxies.txt  # Backup proxies
export PROXY_STRATEGY=random
export PROXY_MIN_HEALTH=0.8
cargo run -- run
```

## Docker Setup

### Dockerfile with WARP Proxy

```dockerfile
FROM rust:latest

WORKDIR /app
COPY . .

ENV WARP_PROXY="socks5://warp:40000"
ENV PROXY_STRATEGY=fixed

RUN cargo build --release

CMD ["./target/release/doradura", "run"]
```

### Docker Compose

```yaml
version: '3'

services:
  doradura:
    build: .
    environment:
      WARP_PROXY: "socks5://warp:40000"
      PROXY_STRATEGY: fixed
    ports:
      - "8080:8080"

  warp:
    image: caomingjun/warp
    restart: always
```

## Troubleshooting

### Issue: Proxies not working

**Check proxy format:**
```bash
# Valid formats
http://127.0.0.1:8080
http://user:pass@127.0.0.1:8080
socks5://127.0.0.1:1080
```

**Test proxy manually:**
```bash
curl -x http://127.0.0.1:8080 http://ipinfo.io
```

### Issue: All downloads failing

**Check proxy health:**
```bash
# Lower the minimum health threshold
export PROXY_MIN_HEALTH=0.3
```

**Disable proxies temporarily:**
```bash
unset WARP_PROXY
unset PROXY_FILE
cargo run -- run  # Run without proxies
```

### Issue: Slow downloads

**Use WARP proxy:**
```bash
export WARP_PROXY="socks5://127.0.0.1:40000"
```

## FAQ

**Q: What's the recommended proxy setup?**
A: Use Cloudflare WARP via `WARP_PROXY`. It's free and reliable.

**Q: Can I mix HTTP and SOCKS5 proxies?**
A: Yes, list them together in PROXY_FILE. System handles both transparently.

**Q: What if all proxies fail?**
A: Falls back to direct connection (no proxy) if all proxies unhealthy.

**Q: How to test proxy before using?**
A: Manual test: `curl -x [proxy] http://ipinfo.io`

## Related Documentation

- [yt-dlp Update System](YTDLP_UPDATE_GUIDE.md) - Keep yt-dlp updated
- [YouTube Error Handling](FIX_YOUTUBE_ERRORS.md) - Error recovery
- [Cookies Management](YOUTUBE_COOKIES.md) - Authentication
- [Troubleshooting](TROUBLESHOOTING.md) - General issues
