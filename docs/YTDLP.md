# yt-dlp — Setup, Updates & Troubleshooting

## Quick Reference

### Commands

| Command | Purpose |
|---------|---------|
| `cargo run -- update-ytdlp --check` | Show current version |
| `cargo run -- update-ytdlp` | Update if newer version available |
| `cargo run -- update-ytdlp --force` | Force update (ignores version check) |
| `cargo run -- run` | Start bot (auto-checks for updates on startup) |

### When to Update

| Scenario | Action |
|----------|--------|
| Bot startup | Automatic check (updates if needed) |
| 403 errors increasing | Force update immediately |
| Weekly maintenance | `update-ytdlp` |
| Before deploying | `update-ytdlp --force` |
| Unknown extraction errors | `update-ytdlp --check` then update |

### Error Table

| Error | Cause | Fix |
|-------|-------|-----|
| `HTTP Error 403: Forbidden` | Outdated yt-dlp or blocked requests | Update yt-dlp, refresh cookies |
| `Unable to extract video id` | Signature extraction failed | Force update yt-dlp |
| `[youtube] Skipping fragment` | Fragment download failed | Already handled (10 auto-retries) |
| `pip command not found` | Python/pip not installed | Install `python3-pip` |
| `Permission denied` | Insufficient permissions | Use `--user` flag or `sudo` |

### Environment Variables

```bash
YTDL_BIN=/usr/local/bin/yt-dlp          # Custom yt-dlp binary path
YTDL_COOKIES_BROWSER=chrome             # Browser cookie extraction
YOUTUBE_COOKIES_PATH=/path/cookies.txt  # File-based cookies
YTDLP_UPDATE_TIMEOUT=300               # Update timeout in seconds
```

---

## Why Updates Matter

YouTube actively works against video downloaders by:
- Changing player algorithms and JavaScript signatures
- Updating authentication mechanisms
- Blocking outdated user agents
- Rotating API endpoints and rate limits

Without regular yt-dlp updates you'll see: `HTTP Error 403: Forbidden`, `Unable to extract video id`, `Sign in to confirm you're not a bot`, `Video unavailable`.

---

## Setup & Installation

### Installation Methods

**System package (recommended for servers):**
```bash
brew install yt-dlp          # macOS
sudo apt-get install yt-dlp  # Ubuntu/Debian
sudo dnf install yt-dlp      # Fedora
sudo pacman -S yt-dlp        # Arch
```
Updates via: `yt-dlp -U`

**Python pip:**
```bash
pip install yt-dlp
pip3 install yt-dlp
```
Updates via: `pip install --upgrade yt-dlp`

The bot supports both `pip` and `pip3` and tries them in order.

**Standalone binary:** Set `YTDL_BIN=/path/to/yt-dlp` in `.env`.

### Verify Installation

```bash
yt-dlp --version
yt-dlp --list-extractors | wc -l   # Should be 700+
```

---

## Fragment Error Handling

Fragment download failures are handled automatically with these yt-dlp parameters:

```
--concurrent-fragments 3    Reduced from 5 (less aggressive, reduces rate-limiting)
--fragment-retries 10       Auto-retry failed fragments 10 times
--socket-timeout 30         Prevent hanging connections
--http-chunk-size 10485760  10 MB chunks for granular retry
--sleep-requests 1          1 ms delay between requests
```

**Result:** ~95% of transient 403 fragment errors are auto-recovered. Users see a friendly "retry later" message only when all 10 retries fail.

**Error classification:**
- Fragment 403s → Temporary, auto-retry
- Signature extraction failure → Persistent, notify admin
- Private video → User error, explain
- Network timeout → Temporary, suggest retry

---

## Troubleshooting

### 403 Errors Persist

```bash
cargo run -- update-ytdlp --force
cargo run -- update-ytdlp --check
export YOUTUBE_COOKIES_PATH=/path/to/new_cookies.txt
cargo run -- run
```

### `pip` Not Found

```bash
brew install python3          # macOS
sudo apt-get install python3-pip  # Ubuntu/Debian
pip3 install --upgrade yt-dlp
```

### Permission Denied on Update

```bash
pip install --user --upgrade yt-dlp
# or system-wide:
sudo pip install --upgrade yt-dlp
```

### Update Timed Out (> 120 seconds)

```bash
pip install --upgrade yt-dlp --timeout 300
```

### Bot Won't Start

```bash
yt-dlp --version          # Verify yt-dlp is installed
pip install yt-dlp        # Install if missing
cargo run -- update-ytdlp --check
```

### Monitor Fragment Retries

```bash
tail -f logs/bot.log | grep -i "403\|fragment"
# Healthy: "Retrying fragment 5 (3/10)"
# Problem: repeated immediate failures
```

### Test a Download Directly

```bash
yt-dlp "https://www.youtube.com/watch?v=dQw4w9WgXcQ" --dump-json | head
```

---

## Scheduled Updates

### Cron Job (Linux/macOS)

```bash
crontab -e
# Add (runs at 2 AM daily):
0 2 * * * cd /path/to/doradura && cargo run -- update-ytdlp --force >> /tmp/ytdlp-update.log 2>&1
```

With bot restart:
```bash
# /home/user/update_ytdlp.sh
#!/bin/bash
cd /home/user/doradura
cargo run -- update-ytdlp --force
systemctl restart doradura
```
```bash
chmod +x /home/user/update_ytdlp.sh
crontab -e   # Add: 0 2 * * * /home/user/update_ytdlp.sh
```

### Systemd Timer (Linux)

`/etc/systemd/system/ytdlp-update.timer`:
```ini
[Unit]
Description=Update yt-dlp daily

[Timer]
OnCalendar=*-*-* 02:00:00
Persistent=true

[Install]
WantedBy=timers.target
```

`/etc/systemd/system/ytdlp-update.service`:
```ini
[Unit]
Description=Update yt-dlp
After=network-online.target

[Service]
Type=oneshot
WorkingDirectory=/path/to/doradura
ExecStart=/usr/bin/cargo run -- update-ytdlp
User=your_user
```

```bash
sudo systemctl daemon-reload
sudo systemctl enable ytdlp-update.timer
sudo systemctl start ytdlp-update.timer
```

### Docker

```bash
#!/bin/bash
cargo run -- update-ytdlp
cargo run -- run
```

---

## API

```rust
use doradura::download::ytdlp;

ytdlp::check_and_update_ytdlp().await?;  // Update only if needed
ytdlp::force_update_ytdlp().await?;      // Always update
ytdlp::print_ytdlp_version().await?;     // Print current version
```

---

## FAQ

**Q: How often should I update yt-dlp?**
A: At least weekly. Daily is ideal for production.

**Q: Will updating break my bot?**
A: No. Updates are backwards-compatible. The bot continues working even if the update fails.

**Q: Should I use `--force` or normal update?**
A: Normal update for scheduled runs. `--force` when you suspect corruption or need the latest immediately.

**Q: Does the bot need to restart after an update?**
A: Yes. Stop the bot, update, then restart.

**Q: How do I know if yt-dlp is outdated?**
A: `403 Forbidden` or `signature extraction failed` errors are the main signals.

**Q: Can I downgrade yt-dlp?**
A: Yes: `pip install yt-dlp==2024.01.01` (specify exact version).

**Q: What if yt-dlp is installed via pip?**
A: The bot detects this and updates automatically via pip/pip3.

**Q: What if `pip` is not found?**
A: Install Python: `brew install python3` (macOS) or `apt install python3-pip` (Linux).
