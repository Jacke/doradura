# Proxy System for yt-dlp

## Overview

The Doradura bot now supports proxy downloads through yt-dlp. This system provides:

- **Multiple proxy protocols**: HTTP, HTTPS, SOCKS5
- **Smart proxy selection**: Round-robin, random, weighted, or fixed strategies
- **Proxy rotation**: Use different proxies for each download
- **Health tracking**: Monitor proxy success rates and auto-skip bad proxies
- **Dynamic proxy lists**: Load proxies from files, environment, or URLs
- **Thread-safe management**: Async-compatible proxy handling

## Why Use Proxies?

Proxies help with:
- **Bypass regional restrictions**: Access geo-blocked content
- **Avoid rate limiting**: Distribute requests across multiple IPs
- **IP rotation**: Prevent detection as bot
- **Load balancing**: Spread traffic across multiple servers
- **Privacy**: Hide your real IP address

## Quick Start

### 1. Set Up Proxies via Environment

**Simple proxy list:**
```bash
export PROXY_LIST="http://127.0.0.1:8080,http://127.0.0.1:8081,socks5://127.0.0.1:1080"
cargo run -- run
```

**With authentication:**
```bash
export PROXY_LIST="http://user:pass@proxy1.com:8080,http://user:pass@proxy2.com:8080"
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
| `PROXY_LIST` | Comma-separated proxy list | empty | `http://127.0.0.1:8080,socks5://127.0.0.1:1080` |
| `PROXY_FILE` | File path with proxies (one per line) | empty | `/etc/proxies.txt` |
| `PROXY_STRATEGY` | Selection strategy | `round_robin` | `random`, `weighted`, `fixed` |
| `PROXY_ROTATION_ENABLED` | Enable proxy rotation per download | `true` | `true` / `false` |
| `PROXY_MIN_HEALTH` | Minimum proxy health score (0-1) | `0.5` | `0.7` (70% success rate) |
| `PROXY_UPDATE_URL` | URL to fetch proxy list from | empty | `https://proxy-api.example.com/list` |
| `PROXY_UPDATE_INTERVAL` | Update interval in seconds | `3600` | `1800` (30 minutes) |

### Priority Order

Proxies are loaded in this order:
1. `PROXY_LIST` environment variable (highest priority)
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

```
Proxy 1 → Proxy 2 → Proxy 3 → Proxy 1 → ...
```

### Random
- Randomly picks proxy each time
- Best for: Unpredictable behavior, avoiding patterns
- Good for evading detection

```
Select: Random from available proxies
```

### Weighted
- Proxies with higher weight are used more often
- Best for: Some proxies are better than others
- Can set weight when adding proxies

```
Proxy1 (weight=2) has 2x chance vs Proxy2 (weight=1)
```

### Fixed
- Always uses first proxy
- Best for: Testing, single proxy setup
- Minimal overhead

```
Always use Proxy 1
```

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

**View proxy statistics:**
```bash
# Admin command (in bot) - Coming Soon
/proxy_stats
```

## Usage Examples

### Example 1: Simple Single Proxy

```bash
export PROXY_LIST="http://127.0.0.1:8080"
cargo run -- run
```

All downloads go through `127.0.0.1:8080`.

### Example 2: Multiple Proxies with Rotation

```bash
export PROXY_LIST="http://proxy1.example.com:8080,http://proxy2.example.com:8080,socks5://proxy3.example.com:1080"
export PROXY_STRATEGY=round_robin
export PROXY_ROTATION_ENABLED=true
cargo run -- run
```

Downloads rotate: proxy1 → proxy2 → proxy3 → proxy1...

### Example 3: Proxies from File

Create `~/proxies.txt`:
```
http://user1:pass1@proxy1.example.com:8080
http://user2:pass2@proxy2.example.com:8080
http://user3:pass3@proxy3.example.com:8080
socks5://proxy4.example.com:1080
```

Setup:
```bash
export PROXY_FILE=~/proxies.txt
export PROXY_STRATEGY=random
cargo run -- run
```

Random proxy selected for each download.

### Example 4: Weighted Distribution

```bash
# Fast proxies get more traffic
export PROXY_LIST="http://fast-proxy.example.com:8080#weight:3,http://slow-proxy.example.com:8080#weight:1"
export PROXY_STRATEGY=weighted
cargo run -- run
```

Fast proxy is used 3x more often.

### Example 5: Dynamic Proxy List

```bash
export PROXY_UPDATE_URL="https://my-proxy-api.example.com/list.txt"
export PROXY_UPDATE_INTERVAL=1800  # Update every 30 minutes
export PROXY_STRATEGY=random
cargo run -- run
```

Fetches and updates proxy list every 30 minutes.

### Example 6: High-Reliability Setup

```bash
export PROXY_FILE=/etc/proxies.txt
export PROXY_STRATEGY=random
export PROXY_ROTATION_ENABLED=true
export PROXY_MIN_HEALTH=0.8  # Skip bad proxies
cargo run -- run
```

Only uses proxies with >80% success rate.

## Docker Setup

### Dockerfile with Proxy List

```dockerfile
FROM rust:latest

WORKDIR /app
COPY . .

ENV PROXY_LIST="http://proxy1:8080,http://proxy2:8080"
ENV PROXY_STRATEGY=round_robin
ENV PROXY_ROTATION_ENABLED=true

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
      PROXY_FILE: /etc/proxies.txt
      PROXY_STRATEGY: random
      PROXY_ROTATION_ENABLED: "true"
      PROXY_MIN_HEALTH: 0.7
    volumes:
      - ./proxies.txt:/etc/proxies.txt:ro
    ports:
      - "8080:8080"
```

### Using Proxy Provider

Example with ProxyMesh:
```dockerfile
ENV PROXY_LIST="http://user-key:@proxymesh.com:31280,http://user-key:@proxymesh.com:31281"
ENV PROXY_STRATEGY=round_robin
```

## Production Best Practices

### 1. High Availability

```bash
# Use multiple proxies with health checking
export PROXY_FILE=/etc/proxies-production.txt
export PROXY_MIN_HEALTH=0.75  # 75% uptime minimum
export PROXY_STRATEGY=weighted  # Use best proxies more
```

### 2. Rotation for Stealth

```bash
# Rotate proxy for each download
export PROXY_STRATEGY=random
export PROXY_ROTATION_ENABLED=true
```

### 3. Cost Optimization

```bash
# Use cheaper proxies more, expensive ones less
# Weight them accordingly
export PROXY_STRATEGY=weighted
```

### 4. Failover Setup

```bash
# Have backup proxies in reserve
export PROXY_FILE=/etc/proxies.txt  # Contains primary + backups
export PROXY_MIN_HEALTH=0.5  # Use if primary fails
```

## Proxy Sources

### Free Proxies
- http://proxy-list.net
- http://free-proxy-list.net
- http://www.sslproxies.org

⚠️ **Warning**: Free proxies are often slow, unreliable, and unsafe.

### Paid Services (Recommended)
- **ProxyMesh**: Residential + datacenter proxies
- **Bright Data**: Premium residential proxy network
- **Smartproxy**: Affordable rotating proxies
- **Oxylabs**: Enterprise-grade proxies
- **ScraperAPI**: Proxy service with built-in JS rendering

### Self-Hosted
- Squid proxy server
- TinyProxy
- Tach proxy
- 3proxy

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
export PROXY_MIN_HEALTH=0.3  # Allow weaker proxies
```

**Disable proxies temporarily:**
```bash
unset PROXY_LIST
unset PROXY_FILE
cargo run -- run  # Run without proxies
```

### Issue: Slow downloads

**Switch to faster proxies:**
```bash
export PROXY_STRATEGY=weighted
# Assign higher weight to faster proxies
```

**Use fewer concurrent downloads:**
```bash
# Edit src/core/config.rs
pub const MAX_CONCURRENT_DOWNLOADS: usize = 2;  // Reduce from 5
```

### Issue: Memory usage high with many proxies

**Limit tracked statistics:**
- Reset statistics periodically
- Use `PROXY_MIN_HEALTH` to remove dead proxies

## API Reference

### Core Types

```rust
// Proxy protocol
pub enum ProxyProtocol {
    Http,
    Https,
    Socks5,
}

// Single proxy
pub struct Proxy {
    pub protocol: ProxyProtocol,
    pub host: String,
    pub port: u16,
    pub auth: Option<String>,
    pub weight: u32,
}

// Selection strategy
pub enum ProxySelectionStrategy {
    RoundRobin,
    Random,
    Weighted,
    Fixed,
}

// Proxy list manager
pub struct ProxyListManager {
    // Thread-safe proxy management
}
```

### Usage in Code

```rust
use doradura::download::{ProxyListManager, ProxySelectionStrategy, Proxy, ProxyProtocol};

// Create manager
let manager = ProxyListManager::new(ProxySelectionStrategy::RoundRobin);

// Add proxy
manager.add_proxy_string("http://127.0.0.1:8080").await?;

// Select proxy
if let Some(proxy) = manager.select().await {
    println!("Using proxy: {}", proxy);

    // Pass to yt-dlp as: --proxy http://127.0.0.1:8080
}

// Record results
manager.record_success(&proxy).await;

// View stats
let stats = manager.all_stats().await;
for (proxy_url, stat) in stats {
    println!("{}: {}", proxy_url, stat);
}
```

## Performance Impact

### Bandwidth
- **No change**: Proxies don't affect bandwidth, only routing

### Speed
- **Slight overhead**: ~50-200ms added per connection
- **Depends on**: Proxy location, network, server load

### Reliability
- **Improved**: With good proxies and health checking
- **Better**: Than single point of failure

## Security Notes

### ⚠️ Important

1. **Don't trust free proxies**: May log traffic, inject ads, or steal data
2. **HTTPS with authentication**: Use when available
3. **Rotate IPs**: Avoid detection by changing IPs
4. **Monitor usage**: Track proxy success rates
5. **Update regularly**: Keep proxy list fresh

### Best Practices

- Use proxies from reputable providers
- Rotate IPs frequently
- Monitor for unusual patterns
- Use HTTPS when possible
- Don't use same proxy for sensitive operations

## FAQ

**Q: Can I use proxies for all downloads?**
A: Yes, when properly configured. System works with all content types.

**Q: What's the overhead per download?**
A: ~50-200ms depending on proxy distance and quality.

**Q: Can I mix HTTP and SOCKS5 proxies?**
A: Yes, list them together. System handles both transparently.

**Q: How often should I update proxy list?**
A: Depends on provider. Hourly to daily is typical. Use `PROXY_UPDATE_INTERVAL`.

**Q: What if all proxies fail?**
A: Falls back to direct connection (no proxy) if all proxies unhealthy.

**Q: How to test proxy before using?**
A: Manual test: `curl -x [proxy] http://ipinfo.io`

**Q: Can I change proxy strategy at runtime?**
A: Not yet - need bot command. Restart to change strategy.

**Q: How to monitor proxy health?**
A: Admin command `/proxy_stats` (coming soon) or check logs.

## Related Documentation

- [yt-dlp Update System](YTDLP_UPDATE_GUIDE.md) - Keep yt-dlp updated
- [YouTube Error Handling](FIX_YOUTUBE_ERRORS.md) - Error recovery
- [Cookies Management](YOUTUBE_COOKIES.md) - Authentication
- [Troubleshooting](TROUBLESHOOTING.md) - General issues
