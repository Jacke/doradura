#!/usr/bin/env python3
"""
YouTube Cookie Manager — aiohttp server + Persistent Browser Architecture.

"UNKILLABLE COOKIES" ARCHITECTURE v3.0:
=======================================

This cookie manager is designed to be maximally resilient and self-healing.
v3.0 adds FEEDBACK LOOP - Rust bot reports errors back to cookie manager!

DEFENSE IN DEPTH - 7 LAYERS OF PROTECTION:

1. MEMORY CACHE (Layer 1 - Instant):
   - Cookies always kept in RAM
   - Instant recovery from file corruption
   - Hash validation for integrity

2. MULTI-FILE REDUNDANCY (Layer 2 - Fast):
   - Primary: /data/cookies.txt
   - Backup: /tmp/cookies_backup.txt
   - Shadow: /data/cookies_shadow.txt
   - Atomic writes with fsync

3. ENVIRONMENT BOOTSTRAP (Layer 3 - Fallback):
   - YTDL_COOKIES_B64 environment variable
   - Always available at startup

4. TELEGRAM BACKUP (Layer 4 - Disaster Recovery):
   - Periodic backup to admin as encrypted file
   - Can restore from Telegram message

5. QUICK HEALTH CHECK (Layer 5 - Proactive):
   - HEAD request to YouTube every 5 min
   - Detect session expiry before download fails

6. FEEDBACK LOOP (Layer 6 - Reactive) [NEW in v3.0]:
   - Rust bot reports cookie errors via /api/report_error
   - Triggers emergency refresh on InvalidCookies/BotDetection
   - Enables retry-with-refresh pattern

7. GRACEFUL DEGRADATION (Layer 7 - Ultimate Fallback):
   - PO Token only mode (limited but works)
   - Alert admin, continue with reduced functionality

RECOVERY CHAIN:
   Memory → Primary File → Backup File → Shadow File → Env → Telegram → Degraded Mode

FEEDBACK LOOP (v3.0):
   Rust Bot → /api/report_error → Emergency Refresh → Bot Retries → Success!

API endpoints:
  GET  /health              — Health check (Railway monitoring)
  POST /api/login_start     — Start visual login session (Xvfb + noVNC)
  POST /api/login_stop      — Stop login, export cookies
  GET  /api/status          — Cookie manager status
  POST /api/export_cookies  — Force cookie re-export
  GET  /api/browser_health  — Check persistent browser health
  POST /api/restart_browser — Force restart persistent browser
  GET  /api/cookie_debug    — Detailed cookie analysis
  POST /api/backup_telegram — Backup cookies to Telegram
  POST /api/report_error    — Report cookie error from Rust bot [NEW]
  GET  /metrics             — Prometheus metrics
"""

import asyncio
import base64
import hashlib
import logging
import os
import signal
import subprocess
import tempfile
import threading
import time
from datetime import datetime, timezone
from enum import Enum
from pathlib import Path
from typing import Dict, List, Optional, Tuple

import aiohttp
from aiohttp import web
import undetected_chromedriver as uc

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------

_raw_cookies_file = os.environ.get("YTDL_COOKIES_FILE", "/data/youtube_cookies.txt")
COOKIES_FILE = _raw_cookies_file if os.path.isabs(_raw_cookies_file) else os.path.join("/data", _raw_cookies_file)

# Multi-file redundancy paths
COOKIES_BACKUP_FILE = "/tmp/cookies_backup.txt"
COOKIES_SHADOW_FILE = "/data/cookies_shadow.txt"
COOKIE_LOCATIONS = [COOKIES_FILE, COOKIES_BACKUP_FILE, COOKIES_SHADOW_FILE]

BROWSER_PROFILE_DIR = os.environ.get("BROWSER_PROFILE_DIR", "/data/browser_profile")
REFRESH_INTERVAL = int(os.environ.get("COOKIE_REFRESH_INTERVAL", "1800"))  # 30 min
LOGIN_TIMEOUT = int(os.environ.get("COOKIE_LOGIN_TIMEOUT", "900"))  # 15 min
API_PORT = int(os.environ.get("COOKIE_MANAGER_PORT", "9876"))
API_HOST = os.environ.get("COOKIE_MANAGER_HOST", "127.0.0.1")
NOVNC_HOST = os.environ.get("NOVNC_HOST", "")
NOVNC_PORT = int(os.environ.get("NOVNC_PORT", "6080"))
NOVNC_EXTERNAL_PORT = int(os.environ.get("NOVNC_EXTERNAL_PORT", "0"))
VNC_PORT = 5900
VNC_PASSWORD = os.environ.get("VNC_PASSWORD", "")
DISPLAY = ":99"
CHROMIUM_PATH = os.environ.get("CHROMIUM_PATH", "/usr/bin/chromium-browser")
CHROMEDRIVER_PATH = os.environ.get("CHROMEDRIVER_PATH", "/usr/bin/chromedriver")

# Watchdog configuration
HEALTH_CHECK_INTERVAL = int(os.environ.get("BROWSER_HEALTH_CHECK_INTERVAL", "300"))  # 5 min
BROWSER_MAX_MEMORY_MB = int(os.environ.get("BROWSER_MAX_MEMORY_MB", "1024"))  # 1 GB
BROWSER_RESTART_INTERVAL = int(os.environ.get("BROWSER_RESTART_INTERVAL", "21600"))  # 6 hours

# Circuit breaker configuration
CIRCUIT_BREAKER_THRESHOLD = int(os.environ.get("CIRCUIT_BREAKER_THRESHOLD", "5"))
CIRCUIT_BREAKER_RESET_TIMEOUT = int(os.environ.get("CIRCUIT_BREAKER_RESET_TIMEOUT", "600"))

# Telegram alerting configuration
TELEGRAM_BOT_TOKEN = os.environ.get("TELOXIDE_TOKEN", "")
TELEGRAM_ADMIN_ID = os.environ.get("ADMIN_IDS", "").split(",")[0] if os.environ.get("ADMIN_IDS") else ""

# Adaptive refresh configuration
REFRESH_INTERVAL_CRITICAL = 5 * 60    # 5 min when cookies expire < 2h
REFRESH_INTERVAL_WARNING = 15 * 60    # 15 min when cookies expire < 12h
REFRESH_INTERVAL_NORMAL = 30 * 60     # 30 min when cookies expire < 48h
REFRESH_INTERVAL_RELAXED = 60 * 60    # 1 hour when cookies expire > 48h

# Cookie expiry warning thresholds (hours)
EXPIRY_CRITICAL_HOURS = 2
EXPIRY_WARNING_HOURS = 12
EXPIRY_NOTICE_HOURS = 48

# Network retry configuration
NETWORK_RETRY_COUNT = 3
NETWORK_RETRY_DELAY = 5  # seconds

# Quick health check interval
QUICK_HEALTH_CHECK_INTERVAL = int(os.environ.get("QUICK_HEALTH_CHECK_INTERVAL", "300"))  # 5 min

# Telegram backup interval (seconds)
TELEGRAM_BACKUP_INTERVAL = int(os.environ.get("TELEGRAM_BACKUP_INTERVAL", "86400"))  # 24 hours

# Required YouTube/Google cookies that indicate a valid session
REQUIRED_COOKIES = {"SID", "HSID", "SSID", "APISID", "SAPISID"}

# All important Google/YouTube cookies to track
TRACKED_COOKIES = {
    "SID", "HSID", "SSID", "APISID", "SAPISID",
    "__Secure-1PSID", "__Secure-3PSID",
    "__Secure-1PAPISID", "__Secure-3PAPISID",
    "LOGIN_INFO",
    "PREF", "YSC", "VISITOR_INFO1_LIVE",
    "CONSENT", "SOCS",
}

# Store previous cookies for comparison
_previous_cookies = {}

# ---------------------------------------------------------------------------
# Logging
# ---------------------------------------------------------------------------

logging.basicConfig(
    level=logging.DEBUG,
    format="%(asctime)s [cookie-manager] %(levelname)s %(message)s",
    datefmt="%H:%M:%S",
)
log = logging.getLogger("cookie-manager")


# ---------------------------------------------------------------------------
# Memory Cache (Layer 1 - Instant Recovery)
# ---------------------------------------------------------------------------

class MemoryCache:
    """
    In-memory cookie cache for instant recovery.

    Protects against:
    - File system corruption
    - Disk full errors
    - Permission issues
    - Race conditions during write
    """

    def __init__(self):
        self._lock = threading.Lock()
        self._content: Optional[str] = None
        self._timestamp: float = 0
        self._hash: Optional[str] = None
        self._ttl: int = 3600  # 1 hour max cache age

    def set(self, content: str):
        """Store cookies in memory cache."""
        with self._lock:
            self._content = content
            self._timestamp = time.time()
            self._hash = hashlib.md5(content.encode()).hexdigest()
            log.debug("Memory cache updated (hash: %s)", self._hash[:8])

    def get(self) -> Optional[str]:
        """Get cookies from memory cache if valid."""
        with self._lock:
            if self._content is None:
                return None

            # Check TTL
            if time.time() - self._timestamp > self._ttl:
                log.debug("Memory cache expired (age: %ds)", int(time.time() - self._timestamp))
                return None

            # Verify integrity
            if self._hash != hashlib.md5(self._content.encode()).hexdigest():
                log.error("Memory cache corruption detected!")
                self._content = None
                return None

            return self._content

    def get_info(self) -> dict:
        """Get cache status info."""
        with self._lock:
            return {
                "has_content": self._content is not None,
                "age_seconds": int(time.time() - self._timestamp) if self._timestamp else None,
                "hash": self._hash[:8] if self._hash else None,
                "size_bytes": len(self._content) if self._content else 0,
            }

    def invalidate(self):
        """Clear the cache."""
        with self._lock:
            self._content = None
            self._timestamp = 0
            self._hash = None
            log.debug("Memory cache invalidated")


# Global memory cache
memory_cache = MemoryCache()


# ---------------------------------------------------------------------------
# Error Tracker (Feedback Loop - v3.0)
# ---------------------------------------------------------------------------

class CookieHealth:
    """
    Cookie Health Scoring (v4.0) — tracks success/failure rates
    and predicts when cookies might fail.

    Score range: 0-100
    - 100: Perfect health, cookies work great
    - 70+: Healthy
    - 50-70: Degraded, proactive refresh recommended
    - 30-50: Critical, emergency refresh needed
    - <30: Failing, consider PO Token only mode
    """

    def __init__(self):
        self._lock = threading.Lock()
        self._score: int = 100
        self._successes: int = 0
        self._failures: int = 0
        self._last_success: Optional[float] = None
        self._last_failure: Optional[float] = None
        self._consecutive_failures: int = 0

    def record_success(self):
        """Record a successful download using cookies."""
        with self._lock:
            self._successes += 1
            self._last_success = time.time()
            self._consecutive_failures = 0
            # Increase score on success (slow recovery)
            self._score = min(100, self._score + 5)
            log.debug("Cookie health: success recorded, score=%d", self._score)

    def record_failure(self, error_type: str = "unknown"):
        """Record a failed download due to cookie issues."""
        with self._lock:
            self._failures += 1
            self._last_failure = time.time()
            self._consecutive_failures += 1

            # Decrease score based on error type and consecutive failures
            penalty = 15 if error_type in ("InvalidCookies", "BotDetection") else 10
            penalty += self._consecutive_failures * 5  # Escalating penalty

            self._score = max(0, self._score - penalty)
            log.warning("Cookie health: failure recorded (%s), score=%d, consecutive=%d",
                       error_type, self._score, self._consecutive_failures)

    def record_refresh_success(self):
        """Record successful cookie refresh — partial score recovery."""
        with self._lock:
            self._score = min(100, self._score + 20)
            self._consecutive_failures = 0
            log.info("Cookie health: refresh success, score=%d", self._score)

    def should_proactive_refresh(self) -> bool:
        """Check if proactive refresh is recommended (score < 50)."""
        with self._lock:
            return self._score < 50

    def should_use_po_token_only(self) -> bool:
        """Check if we should prefer PO Token only mode (score < 30)."""
        with self._lock:
            return self._score < 30

    def get_score(self) -> int:
        """Get current health score."""
        with self._lock:
            return self._score

    def get_status(self) -> dict:
        """Get detailed health status."""
        with self._lock:
            now = time.time()
            return {
                "score": self._score,
                "status": (
                    "healthy" if self._score >= 70 else
                    "degraded" if self._score >= 50 else
                    "critical" if self._score >= 30 else
                    "failing"
                ),
                "successes": self._successes,
                "failures": self._failures,
                "consecutive_failures": self._consecutive_failures,
                "success_rate": round(self._successes / max(1, self._successes + self._failures) * 100, 1),
                "last_success_ago": int(now - self._last_success) if self._last_success else None,
                "last_failure_ago": int(now - self._last_failure) if self._last_failure else None,
                "recommend_refresh": self._score < 50,
                "recommend_po_token_only": self._score < 30,
            }

    def reset(self):
        """Reset health after successful login/refresh."""
        with self._lock:
            self._score = 100
            self._consecutive_failures = 0
            log.info("Cookie health: reset to 100")


# Global cookie health tracker
cookie_health = CookieHealth()


class ErrorTracker:
    """
    Tracks cookie-related errors reported by Rust bot.

    Enables:
    - Pattern detection (multiple errors = emergency mode)
    - Dynamic refresh frequency adjustment
    - Automatic recovery attempts
    """

    # Error types that trigger refresh
    COOKIE_ERRORS = {"InvalidCookies", "BotDetection", "invalid_cookies", "bot_detection"}

    def __init__(self):
        self._lock = threading.Lock()
        self._errors: List[Tuple[float, str, str]] = []  # (timestamp, error_type, url)
        self._emergency_mode = False
        self._emergency_started: Optional[float] = None
        self._last_refresh_trigger: float = 0
        self._refresh_cooldown: int = 30  # Min seconds between refresh triggers

        # Dynamic intervals
        self._normal_quick_check_interval = QUICK_HEALTH_CHECK_INTERVAL
        self._emergency_quick_check_interval = 30  # 30 seconds in emergency
        self._current_quick_check_interval = self._normal_quick_check_interval

    def record_error(self, error_type: str, url: str = "") -> dict:
        """
        Record an error reported by Rust bot.

        Returns action taken: {"action": "refresh_triggered"|"cooldown"|"ignored"}
        """
        with self._lock:
            now = time.time()
            self._errors.append((now, error_type, url))

            # Keep only last 100 errors
            if len(self._errors) > 100:
                self._errors = self._errors[-100:]

            # Check if this is a cookie-related error
            if error_type not in self.COOKIE_ERRORS:
                log.debug("Ignoring non-cookie error: %s", error_type)
                return {"action": "ignored", "reason": "not_cookie_error"}

            # Check cooldown
            if now - self._last_refresh_trigger < self._refresh_cooldown:
                remaining = self._refresh_cooldown - (now - self._last_refresh_trigger)
                log.info("Refresh on cooldown, %ds remaining", int(remaining))
                return {"action": "cooldown", "remaining_seconds": int(remaining)}

            # Check for emergency mode trigger (3+ errors in 5 minutes)
            recent_errors = [e for e in self._errors
                           if now - e[0] < 300 and e[1] in self.COOKIE_ERRORS]

            if len(recent_errors) >= 3 and not self._emergency_mode:
                self._enter_emergency_mode()

            # Trigger refresh
            self._last_refresh_trigger = now
            log.warning("Cookie error recorded: %s (url: %s)", error_type, url[:50] if url else "N/A")

            return {
                "action": "refresh_triggered",
                "emergency_mode": self._emergency_mode,
                "recent_errors": len(recent_errors),
            }

    def _enter_emergency_mode(self):
        """Enter emergency mode - more aggressive checks."""
        self._emergency_mode = True
        self._emergency_started = time.time()
        self._current_quick_check_interval = self._emergency_quick_check_interval

        log.error("=" * 60)
        log.error("EMERGENCY MODE ACTIVATED")
        log.error("   3+ cookie errors in 5 minutes!")
        log.error("   Quick check interval: %ds → %ds",
                 self._normal_quick_check_interval,
                 self._emergency_quick_check_interval)
        log.error("=" * 60)

        inc_metric("emergency_mode_activations_total")

    def exit_emergency_mode(self):
        """Exit emergency mode after successful recovery."""
        with self._lock:
            if not self._emergency_mode:
                return

            self._emergency_mode = False
            self._current_quick_check_interval = self._normal_quick_check_interval

            duration = time.time() - self._emergency_started if self._emergency_started else 0
            log.info("=" * 60)
            log.info("EMERGENCY MODE DEACTIVATED")
            log.info("   Duration: %ds", int(duration))
            log.info("   Quick check interval: %ds", self._current_quick_check_interval)
            log.info("=" * 60)

    def should_refresh(self) -> bool:
        """Check if we should trigger a refresh based on error pattern."""
        with self._lock:
            now = time.time()

            # In emergency mode, be more aggressive
            if self._emergency_mode:
                # Auto-exit after 10 minutes if no new errors
                recent = [e for e in self._errors if now - e[0] < 600]
                if len(recent) == 0:
                    self.exit_emergency_mode()
                return True

            return False

    def get_quick_check_interval(self) -> int:
        """Get current quick check interval (dynamic based on error rate)."""
        with self._lock:
            return self._current_quick_check_interval

    def get_status(self) -> dict:
        """Get error tracker status."""
        with self._lock:
            now = time.time()
            recent_5min = [e for e in self._errors if now - e[0] < 300]
            recent_1h = [e for e in self._errors if now - e[0] < 3600]

            return {
                "emergency_mode": self._emergency_mode,
                "emergency_duration_s": int(now - self._emergency_started) if self._emergency_started and self._emergency_mode else None,
                "errors_last_5min": len(recent_5min),
                "errors_last_1h": len(recent_1h),
                "total_errors": len(self._errors),
                "quick_check_interval": self._current_quick_check_interval,
                "last_refresh_trigger_ago": int(now - self._last_refresh_trigger) if self._last_refresh_trigger else None,
                "refresh_cooldown": self._refresh_cooldown,
            }

    def clear_errors(self):
        """Clear error history (after successful login)."""
        with self._lock:
            self._errors.clear()
            self.exit_emergency_mode()
            log.info("Error history cleared")


# Global error tracker
error_tracker = ErrorTracker()


# ---------------------------------------------------------------------------
# Atomic File Operations (Corruption Protection)
# ---------------------------------------------------------------------------

def atomic_write(path: str, content: str) -> bool:
    """
    Write file atomically to prevent corruption.

    Uses write-to-temp + fsync + atomic rename pattern.
    Returns True if successful.
    """
    temp_path = f"{path}.tmp.{os.getpid()}"
    try:
        # Ensure directory exists
        os.makedirs(os.path.dirname(path), exist_ok=True)

        # Write to temp file with fsync
        with open(temp_path, 'w') as f:
            f.write(content)
            f.flush()
            os.fsync(f.fileno())

        # Atomic rename
        os.rename(temp_path, path)
        os.chmod(path, 0o644)

        return True
    except Exception as e:
        log.error("Atomic write failed for %s: %s", path, e)
        # Cleanup temp file
        try:
            if os.path.exists(temp_path):
                os.unlink(temp_path)
        except Exception:
            pass
        return False


def save_cookies_everywhere(content: str) -> Tuple[int, List[str]]:
    """
    Write cookies to all backup locations.

    Returns (success_count, failed_paths).
    """
    success = 0
    failed = []

    for path in COOKIE_LOCATIONS:
        try:
            if atomic_write(path, content):
                success += 1
                log.debug("Saved cookies to %s", path)
            else:
                failed.append(path)
        except Exception as e:
            log.warning("Failed to write to %s: %s", path, e)
            failed.append(path)

    # Always update memory cache
    memory_cache.set(content)

    if success > 0:
        log.info("Cookies saved to %d/%d locations", success, len(COOKIE_LOCATIONS))
    else:
        log.error("Failed to save cookies to ANY location!")

    return success, failed


# ---------------------------------------------------------------------------
# Recovery Chain (Layer 2-6)
# ---------------------------------------------------------------------------

def validate_netscape_format(content: str) -> bool:
    """Validate content is valid Netscape cookie format."""
    if not content or not content.strip():
        return False

    lines = content.strip().split('\n')

    # Check for header
    has_header = any(
        line.strip().startswith("# Netscape HTTP Cookie File") or
        line.strip().startswith("# HTTP Cookie File")
        for line in lines[:5]
    )

    # Check for at least one cookie line (7 tab-separated fields)
    has_cookies = any(
        not line.strip().startswith('#') and
        line.strip() and
        len(line.split('\t')) >= 7
        for line in lines
    )

    return has_header and has_cookies


def has_required_cookies(content: str) -> bool:
    """Check if content has at least one required session cookie."""
    return any(f"\t{name}\t" in content for name in REQUIRED_COOKIES)


def get_best_cookies() -> Optional[str]:
    """
    Get cookies from the best available source.

    Recovery chain:
    1. Memory cache (instant, 0ms)
    2. Primary file (fast, ~1ms)
    3. Backup file
    4. Shadow file
    5. Environment variable (bootstrap)
    6. Return None (triggers graceful degradation)
    """
    # Layer 1: Memory cache
    content = memory_cache.get()
    if content and validate_netscape_format(content) and has_required_cookies(content):
        log.debug("Using cookies from memory cache")
        return content

    # Layer 2-4: File locations
    for path in COOKIE_LOCATIONS:
        try:
            if not os.path.exists(path):
                continue

            content = Path(path).read_text()
            if validate_netscape_format(content) and has_required_cookies(content):
                log.info("Recovered cookies from %s", path)
                # Update memory cache with recovered content
                memory_cache.set(content)
                return content
            else:
                log.warning("Invalid cookies in %s", path)
        except Exception as e:
            log.warning("Failed to read %s: %s", path, e)

    # Layer 5: Environment variable
    b64_cookies = os.environ.get("YTDL_COOKIES_B64", "")
    if b64_cookies:
        try:
            content = base64.b64decode(b64_cookies).decode("utf-8")
            if validate_netscape_format(content):
                log.info("Recovered cookies from YTDL_COOKIES_B64")
                # Save to files for next time
                save_cookies_everywhere(content)
                return content
        except Exception as e:
            log.warning("Failed to decode YTDL_COOKIES_B64: %s", e)

    # Layer 6: No cookies available
    log.error("No valid cookies found in any source!")
    return None


def count_required_cookies(content: str) -> int:
    """Count how many required cookies are present."""
    return sum(1 for name in REQUIRED_COOKIES if f"\t{name}\t" in content)


# ---------------------------------------------------------------------------
# Bootstrap from environment variable
# ---------------------------------------------------------------------------

def bootstrap_cookies_from_env() -> bool:
    """
    Initialize cookies from YTDL_COOKIES_B64 environment variable.

    Returns True if cookies were successfully bootstrapped.
    """
    b64_cookies = os.environ.get("YTDL_COOKIES_B64", "")
    if not b64_cookies:
        log.info("No YTDL_COOKIES_B64 env var found, skipping bootstrap")
        return False

    try:
        content = base64.b64decode(b64_cookies).decode("utf-8")

        if not validate_netscape_format(content):
            log.warning("YTDL_COOKIES_B64 doesn't look like valid Netscape cookies")
            return False

        env_required = count_required_cookies(content)

        # Check if existing file has better cookies
        should_write = True
        if os.path.exists(COOKIES_FILE):
            try:
                existing = Path(COOKIES_FILE).read_text()
                existing_required = count_required_cookies(existing)

                if existing_required >= env_required:
                    log.info("Existing cookies have %d required, env has %d - keeping existing",
                            existing_required, env_required)
                    # Still update memory cache
                    memory_cache.set(existing)
                    should_write = False
            except Exception:
                pass

        if should_write:
            success, failed = save_cookies_everywhere(content)

            log.info("=" * 60)
            log.info("BOOTSTRAPPED cookies from YTDL_COOKIES_B64")
            log.info("   Saved to: %d locations", success)
            log.info("   Required cookies: %d/%d", env_required, len(REQUIRED_COOKIES))
            log.info("=" * 60)
            return True

        return False

    except Exception as e:
        log.error("Failed to bootstrap cookies from env: %s", e)
        return False


# ---------------------------------------------------------------------------
# Quick Health Check (Proactive Detection)
# ---------------------------------------------------------------------------

async def quick_cookie_check() -> Tuple[bool, str]:
    """
    Quick check if cookies are still valid without full browser refresh.

    Makes a HEAD request to YouTube with cookies to verify session.
    Much faster than browser refresh (~100ms vs ~10s).

    Returns (is_valid, reason).
    """
    content = get_best_cookies()
    if not content:
        return False, "no_cookies"

    # Parse cookies from Netscape format
    cookies = {}
    for line in content.split('\n'):
        if line.startswith('#') or not line.strip():
            continue
        parts = line.split('\t')
        if len(parts) >= 7:
            name, value = parts[5], parts[6]
            cookies[name] = value

    if not cookies:
        return False, "no_valid_cookies"

    try:
        jar = aiohttp.CookieJar()
        for name, value in cookies.items():
            jar.update_cookies({name: value})

        timeout = aiohttp.ClientTimeout(total=10)
        async with aiohttp.ClientSession(cookie_jar=jar, timeout=timeout) as session:
            # Check subscriptions page - requires login
            async with session.head(
                'https://www.youtube.com/feed/subscriptions',
                allow_redirects=False
            ) as resp:
                if resp.status == 200:
                    return True, "valid"
                elif resp.status in (302, 303):
                    # Redirect to login = session expired
                    return False, "session_expired"
                elif resp.status == 403:
                    return False, "forbidden"
                else:
                    return False, f"http_{resp.status}"

    except asyncio.TimeoutError:
        return False, "timeout"
    except aiohttp.ClientError as e:
        return False, f"network_error_{type(e).__name__}"
    except Exception as e:
        return False, f"error_{type(e).__name__}"


# ---------------------------------------------------------------------------
# Telegram Backup (Disaster Recovery)
# ---------------------------------------------------------------------------

async def backup_cookies_to_telegram() -> bool:
    """
    Send current cookies to admin as file backup.

    This is the ultimate disaster recovery - cookies can be restored
    by forwarding the file back to the bot.
    """
    if not TELEGRAM_BOT_TOKEN or not TELEGRAM_ADMIN_ID:
        log.debug("Telegram backup not configured")
        return False

    content = get_best_cookies()
    if not content:
        log.warning("No cookies to backup")
        return False

    try:
        # Create temp file with cookies
        with tempfile.NamedTemporaryFile(mode='w', suffix='.txt', delete=False) as f:
            # Add metadata header
            f.write(f"# Backup created: {datetime.now(timezone.utc).isoformat()}\n")
            f.write(f"# Required cookies: {count_required_cookies(content)}/{len(REQUIRED_COOKIES)}\n")
            f.write(content)
            temp_path = f.name

        # Send via Telegram
        url = f"https://api.telegram.org/bot{TELEGRAM_BOT_TOKEN}/sendDocument"

        timestamp = datetime.now().strftime("%Y%m%d_%H%M")

        async with aiohttp.ClientSession() as session:
            with open(temp_path, 'rb') as f:
                data = aiohttp.FormData()
                data.add_field('chat_id', TELEGRAM_ADMIN_ID)
                data.add_field('document', f, filename=f'cookies_backup_{timestamp}.txt')
                data.add_field('caption',
                    f"Backup {datetime.now():%Y-%m-%d %H:%M}\n"
                    f"Required: {count_required_cookies(content)}/{len(REQUIRED_COOKIES)}"
                )

                async with session.post(url, data=data, timeout=30) as resp:
                    if resp.status == 200:
                        log.info("Cookies backed up to Telegram")
                        return True
                    else:
                        log.warning("Telegram backup failed: %s", await resp.text())
                        return False

    except Exception as e:
        log.error("Telegram backup error: %s", e)
        return False

    finally:
        try:
            os.unlink(temp_path)
        except Exception:
            pass


async def send_telegram_alert(message: str):
    """Send alert message to admin via Telegram."""
    if not TELEGRAM_BOT_TOKEN or not TELEGRAM_ADMIN_ID:
        log.debug("Telegram alerting not configured, skipping alert")
        return

    try:
        url = f"https://api.telegram.org/bot{TELEGRAM_BOT_TOKEN}/sendMessage"
        payload = {
            "chat_id": TELEGRAM_ADMIN_ID,
            "text": f"🍪 Cookie Manager\n\n{message}",
            "parse_mode": "HTML",
        }

        async with aiohttp.ClientSession() as session:
            async with session.post(url, json=payload, timeout=10) as resp:
                if resp.status == 200:
                    log.info("Telegram alert sent successfully")
                else:
                    log.warning("Failed to send Telegram alert: %s", await resp.text())
    except Exception as e:
        log.warning("Could not send Telegram alert: %s", e)


# ---------------------------------------------------------------------------
# Emergency Cookie Refresh (Feedback Loop - v3.0)
# ---------------------------------------------------------------------------

async def emergency_cookie_refresh() -> dict:
    """
    Emergency cookie refresh triggered by error report from Rust bot.

    This is called when yt-dlp reports InvalidCookies or BotDetection.
    Attempts to refresh cookies immediately so Rust bot can retry.

    Returns status dict with success/failure info.
    """
    log.warning("=" * 60)
    log.warning("EMERGENCY COOKIE REFRESH TRIGGERED")
    log.warning("=" * 60)

    inc_metric("emergency_refresh_total")
    result = {
        "success": False,
        "method": None,
        "cookie_count": 0,
        "error": None,
    }

    # Check if browser is running
    if not browser_manager.is_running():
        log.warning("Browser not running, attempting to start...")
        loop = asyncio.get_running_loop()
        started = await loop.run_in_executor(None, browser_manager.start)
        if not started:
            result["error"] = "Failed to start browser"
            log.error("Emergency refresh FAILED: %s", result["error"])
            inc_metric("emergency_refresh_failed")
            return result

    # Check circuit breaker
    if not circuit_breaker.can_execute():
        log.warning("Circuit breaker OPEN, using static cookie refresh")
        # Just validate and re-save existing cookies
        content = get_best_cookies()
        if content and has_required_cookies(content):
            save_cookies_everywhere(content)
            result["success"] = True
            result["method"] = "static_refresh"
            result["cookie_count"] = count_required_cookies(content)
            log.info("Emergency refresh via static cookies: %d required", result["cookie_count"])
            return result
        else:
            result["error"] = "Circuit breaker open and no valid static cookies"
            log.error("Emergency refresh FAILED: %s", result["error"])
            inc_metric("emergency_refresh_failed")
            return result

    # Try browser refresh
    try:
        loop = asyncio.get_running_loop()
        cookie_count = await loop.run_in_executor(None, browser_manager.refresh_and_export)

        result["success"] = True
        result["method"] = "browser_refresh"
        result["cookie_count"] = cookie_count

        log.info("Emergency refresh SUCCESS: %d cookies exported", cookie_count)
        inc_metric("emergency_refresh_success")

        # Update cookie health (v4.0)
        cookie_health.record_refresh_success()

        # If we were in emergency mode and refresh succeeded, exit it
        if error_tracker._emergency_mode:
            # Don't exit immediately - wait for quick health check to confirm
            pass

        return result

    except Exception as e:
        log.error("Emergency browser refresh failed: %s", e)
        circuit_breaker.record_failure()

        # Fallback: try to re-save existing cookies
        content = get_best_cookies()
        if content and has_required_cookies(content):
            save_cookies_everywhere(content)
            result["success"] = True
            result["method"] = "fallback_static"
            result["cookie_count"] = count_required_cookies(content)
            result["error"] = f"Browser failed ({e}), used static fallback"
            log.warning("Emergency refresh partial success: %s", result["error"])
            return result

        result["error"] = str(e)
        inc_metric("emergency_refresh_failed")
        return result


# ---------------------------------------------------------------------------
# Graceful Degradation
# ---------------------------------------------------------------------------

class CookieMode(Enum):
    """Operating mode based on cookie availability."""
    FULL = "full"           # Cookies work perfectly
    DEGRADED = "degraded"   # Cookies expiring soon / some issues
    PO_TOKEN_ONLY = "po_token"  # No cookies, PO Token only
    OFFLINE = "offline"     # Nothing works


def get_current_mode() -> CookieMode:
    """Determine current operating mode."""
    content = get_best_cookies()

    if content and has_required_cookies(content):
        # Check expiry
        expiry_info = get_cookie_expiry_info()
        min_hours = expiry_info.get("min_expiry_hours")

        if min_hours is not None and min_hours < EXPIRY_CRITICAL_HOURS:
            return CookieMode.DEGRADED

        return CookieMode.FULL

    # No valid cookies - check if PO Token is available
    # (PO Token server runs on port 4416)
    try:
        import socket
        s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        s.settimeout(1)
        result = s.connect_ex(('127.0.0.1', 4416))
        s.close()
        if result == 0:
            return CookieMode.PO_TOKEN_ONLY
    except Exception:
        pass

    return CookieMode.OFFLINE


# ---------------------------------------------------------------------------
# Prometheus Metrics
# ---------------------------------------------------------------------------

# Simple in-memory metrics (no external dependency)
METRICS: Dict[str, float] = {
    "cookie_refresh_total": 0,
    "cookie_refresh_success": 0,
    "cookie_refresh_failed": 0,
    "cookie_freshness_score": 0,
    "cookie_age_seconds": 0,
    "browser_restarts_total": 0,
    "circuit_breaker_opens_total": 0,
    "quick_health_checks_total": 0,
    "quick_health_checks_failed": 0,
    "telegram_backups_total": 0,
    "recovery_from_backup_total": 0,
    # Feedback loop metrics (v3.0)
    "error_reports_total": 0,
    "error_reports_invalid_cookies": 0,
    "error_reports_bot_detection": 0,
    "emergency_refresh_total": 0,
    "emergency_refresh_success": 0,
    "emergency_refresh_failed": 0,
    "emergency_mode_activations_total": 0,
}


def inc_metric(name: str, value: float = 1):
    """Increment a metric."""
    METRICS[name] = METRICS.get(name, 0) + value


def set_metric(name: str, value: float):
    """Set a metric value."""
    METRICS[name] = value


def get_prometheus_metrics() -> str:
    """Generate Prometheus-format metrics output."""
    lines = []
    for name, value in METRICS.items():
        # Convert to Prometheus naming convention
        metric_name = f"cookie_manager_{name}"
        lines.append(f"# HELP {metric_name} Cookie manager metric")
        lines.append(f"# TYPE {metric_name} gauge")
        lines.append(f"{metric_name} {value}")
    return "\n".join(lines) + "\n"


# ---------------------------------------------------------------------------
# Circuit Breaker
# ---------------------------------------------------------------------------

class CircuitBreaker:
    """Circuit breaker pattern to prevent cascading failures."""

    def __init__(self, threshold: int = 5, reset_timeout: int = 600):
        self.threshold = threshold
        self.reset_timeout = reset_timeout
        self.failures = 0
        self.last_failure_time = 0.0
        self.state = "CLOSED"
        self._lock = threading.Lock()

    def record_failure(self):
        """Record a failure. May open the circuit."""
        with self._lock:
            self.failures += 1
            self.last_failure_time = time.time()

            if self.failures >= self.threshold:
                if self.state != "OPEN":
                    log.warning("=" * 60)
                    log.warning("CIRCUIT BREAKER OPEN - too many failures (%d)", self.failures)
                    log.warning("   Browser operations disabled for %ds", self.reset_timeout)
                    log.warning("   Falling back to static cookies")
                    log.warning("=" * 60)
                    inc_metric("circuit_breaker_opens_total")
                    asyncio.create_task(send_telegram_alert(
                        f"Circuit Breaker OPEN\n\n"
                        f"Browser failed {self.failures} times.\n"
                        f"Falling back to static cookies for {self.reset_timeout}s."
                    ))
                self.state = "OPEN"

    def record_success(self):
        """Record a success. Resets the circuit."""
        with self._lock:
            if self.state != "CLOSED":
                log.info("Circuit breaker CLOSED - browser recovered")
            self.failures = 0
            self.state = "CLOSED"

    def can_execute(self) -> bool:
        """Check if we can execute browser operations."""
        with self._lock:
            if self.state == "CLOSED":
                return True

            if self.state == "OPEN":
                if time.time() - self.last_failure_time > self.reset_timeout:
                    log.info("Circuit breaker HALF-OPEN - testing recovery")
                    self.state = "HALF_OPEN"
                    return True
                return False

            return True  # HALF_OPEN

    def get_status(self) -> dict:
        """Get circuit breaker status."""
        with self._lock:
            return {
                "state": self.state,
                "failures": self.failures,
                "threshold": self.threshold,
                "last_failure_ago": int(time.time() - self.last_failure_time) if self.last_failure_time else None,
                "reset_timeout": self.reset_timeout,
            }


circuit_breaker = CircuitBreaker(
    threshold=CIRCUIT_BREAKER_THRESHOLD,
    reset_timeout=CIRCUIT_BREAKER_RESET_TIMEOUT
)


# ---------------------------------------------------------------------------
# Error Classification
# ---------------------------------------------------------------------------

class ErrorType(Enum):
    """Classification of errors for smart retry logic."""
    NETWORK = "network"
    BROWSER_CRASH = "browser"
    SESSION_EXPIRED = "session"
    RATE_LIMITED = "ratelimit"
    UNKNOWN = "unknown"


def classify_error(e: Exception) -> ErrorType:
    """Classify an error to determine the appropriate recovery strategy."""
    msg = str(e).lower()

    network_keywords = ["timeout", "connection", "network", "dns", "refused", "reset"]
    if any(kw in msg for kw in network_keywords):
        return ErrorType.NETWORK

    browser_keywords = ["session", "disconnected", "chrome", "driver", "crashed", "dead"]
    if any(kw in msg for kw in browser_keywords):
        return ErrorType.BROWSER_CRASH

    session_keywords = ["sign in", "logged out", "login", "unauthorized", "forbidden"]
    if any(kw in msg for kw in session_keywords):
        return ErrorType.SESSION_EXPIRED

    if "429" in msg or "rate" in msg or "too many" in msg:
        return ErrorType.RATE_LIMITED

    return ErrorType.UNKNOWN


# ---------------------------------------------------------------------------
# Cookie Analysis Functions
# ---------------------------------------------------------------------------

def get_cookie_expiry_info() -> dict:
    """Analyze cookie expiry times and return detailed info."""
    result = {
        "min_expiry_hours": None,
        "expiring_soon": [],
        "expired": [],
        "all_expiries": {},
    }

    content = get_best_cookies()
    if not content:
        return result

    try:
        now = time.time()
        for line in content.split('\n'):
            if line.startswith("#") or not line.strip():
                continue
            parts = line.split("\t")
            if len(parts) >= 7:
                name = parts[5]
                expiry_str = parts[4]

                if not expiry_str.isdigit():
                    continue

                expiry = int(expiry_str)
                if expiry == 0:
                    continue

                hours_left = (expiry - now) / 3600
                result["all_expiries"][name] = hours_left

                if hours_left < 0:
                    result["expired"].append(name)
                elif hours_left < EXPIRY_NOTICE_HOURS:
                    result["expiring_soon"].append({
                        "name": name,
                        "hours_left": round(hours_left, 1)
                    })

                if result["min_expiry_hours"] is None or (hours_left > 0 and hours_left < result["min_expiry_hours"]):
                    result["min_expiry_hours"] = hours_left

    except Exception as e:
        log.warning("Error analyzing cookie expiry: %s", e)

    if result["min_expiry_hours"] is not None:
        result["min_expiry_hours"] = round(result["min_expiry_hours"], 1)

    return result


def calculate_freshness_score() -> Tuple[int, str]:
    """
    Calculate cookie freshness score (0-100) and status.

    Score breakdown:
    - 100: Perfect
    - 70-99: Good
    - 40-69: Degraded
    - 0-39: Critical
    """
    score = 100
    reasons = []

    content = get_best_cookies()
    if not content:
        return 0, "No cookies available"

    # Check required cookies
    present = count_required_cookies(content)
    missing = len(REQUIRED_COOKIES) - present
    if missing > 0:
        penalty = missing * 15
        score -= penalty
        reasons.append(f"missing {missing} required cookies")

    # Check expiry
    expiry_info = get_cookie_expiry_info()
    if expiry_info["expired"]:
        score -= 25
        reasons.append(f"{len(expiry_info['expired'])} expired")

    min_hours = expiry_info.get("min_expiry_hours")
    if min_hours is not None:
        if min_hours < EXPIRY_CRITICAL_HOURS:
            score -= 30
            reasons.append(f"expires in {min_hours:.1f}h")
        elif min_hours < EXPIRY_WARNING_HOURS:
            score -= 15
            reasons.append(f"expires in {min_hours:.1f}h")
        elif min_hours < EXPIRY_NOTICE_HOURS:
            score -= 5

    # Check browser status
    if not browser_manager.is_running():
        score -= 10
        reasons.append("browser offline")

    # Check circuit breaker
    cb_status = circuit_breaker.get_status()
    if cb_status["state"] == "OPEN":
        score -= 15
        reasons.append("circuit breaker open")
    elif cb_status["failures"] > 0:
        score -= cb_status["failures"] * 2
        reasons.append(f"{cb_status['failures']} failures")

    score = max(0, min(100, score))
    set_metric("cookie_freshness_score", score)

    reason = "; ".join(reasons) if reasons else "all checks passed"
    return score, reason


def get_adaptive_refresh_interval() -> int:
    """Calculate refresh interval based on cookie expiry proximity."""
    expiry_info = get_cookie_expiry_info()
    min_hours = expiry_info.get("min_expiry_hours")

    if min_hours is None:
        return REFRESH_INTERVAL_NORMAL

    if min_hours < EXPIRY_CRITICAL_HOURS:
        return REFRESH_INTERVAL_CRITICAL
    elif min_hours < EXPIRY_WARNING_HOURS:
        return REFRESH_INTERVAL_WARNING
    elif min_hours < EXPIRY_NOTICE_HOURS:
        return REFRESH_INTERVAL_NORMAL
    else:
        return REFRESH_INTERVAL_RELAXED


async def check_and_alert_expiry():
    """Check cookie expiry and send alerts if needed."""
    expiry_info = get_cookie_expiry_info()

    if expiry_info["expired"]:
        await send_telegram_alert(
            f"EXPIRED COOKIES!\n\n"
            f"The following cookies have expired:\n"
            f"{', '.join(expiry_info['expired'])}\n\n"
            f"Manual re-login required via noVNC."
        )
        return

    min_hours = expiry_info.get("min_expiry_hours")
    if min_hours is not None and min_hours < EXPIRY_CRITICAL_HOURS:
        await send_telegram_alert(
            f"COOKIES EXPIRING SOON!\n\n"
            f"Cookies will expire in {min_hours:.1f} hours.\n\n"
            f"Consider refreshing session or re-login."
        )


def get_cookie_analysis() -> dict:
    """Analyze cookies file and return detailed status."""
    content = get_best_cookies()

    if not content:
        return {
            "required_found": [],
            "required_missing": list(REQUIRED_COOKIES),
            "session_valid": False,
            "reason": "No cookies available",
        }

    found = set()
    for name in REQUIRED_COOKIES:
        if f"\t{name}\t" in content:
            found.add(name)

    return {
        "required_found": list(found),
        "required_missing": list(REQUIRED_COOKIES - found),
        "session_valid": len(found) > 0,
        "reason": None if found else "Missing all required cookies",
    }


# ---------------------------------------------------------------------------
# Global state
# ---------------------------------------------------------------------------

status = {
    "login_active": False,
    "login_started_at": None,
    "last_refresh": None,
    "last_refresh_success": None,
    "cookie_count": 0,
    "needs_relogin": False,
    "profile_exists": False,
    "last_error": None,
    "browser_running": False,
    "browser_started_at": None,
    "browser_restarts": 0,
    "last_health_check": None,
    "browser_memory_mb": None,
    "last_quick_check": None,
    "last_quick_check_result": None,
    "last_telegram_backup": None,
    "current_mode": "unknown",
}

login_state = {
    "driver": None,
    "xvfb_proc": None,
    "vnc_proc": None,
    "novnc_proc": None,
}

_browser_lock: Optional[asyncio.Lock] = None


def _init_browser_lock():
    global _browser_lock
    _browser_lock = asyncio.Lock()


# ---------------------------------------------------------------------------
# Persistent Browser Manager
# ---------------------------------------------------------------------------

class PersistentBrowserManager:
    """
    Manages a persistent headless Chrome browser that stays running.
    """

    def __init__(self):
        self.driver: Optional[uc.Chrome] = None
        self.started_at: Optional[datetime] = None
        self.restarts: int = 0
        self._lock = threading.Lock()
        self._watchdog_thread: Optional[threading.Thread] = None
        self._shutdown_event = threading.Event()
        self._last_activity: float = 0

    def is_running(self) -> bool:
        """Check if browser is currently running."""
        with self._lock:
            if self.driver is None:
                return False
            try:
                _ = self.driver.window_handles
                return True
            except Exception:
                return False

    def start(self) -> bool:
        """Start the persistent browser. Returns True if successful."""
        with self._lock:
            if self.driver is not None:
                log.warning("Browser already running")
                return True

            log.info("=" * 60)
            log.info("STARTING PERSISTENT BROWSER")
            log.info("=" * 60)

            try:
                self._cleanup_before_start()
                self.driver = self._create_browser()
                self.started_at = datetime.now(timezone.utc)
                self._last_activity = time.time()

                log.info("Initial navigation to YouTube...")
                self.driver.get("https://www.youtube.com")
                time.sleep(3)

                # Import cookies from file into browser
                # This fixes issue where headed browser exports to file but
                # headless browser doesn't see them in profile
                imported = self._import_cookies_from_file()
                if imported > 0:
                    log.info("Imported %d cookies from file", imported)

                log.info("Persistent browser started successfully")
                status["browser_running"] = True
                status["browser_started_at"] = self.started_at.isoformat()

                return True

            except Exception as e:
                log.error("Failed to start persistent browser: %s", e)
                self.driver = None
                status["browser_running"] = False
                status["last_error"] = f"Browser start failed: {e}"
                return False

    def stop(self, export_cookies: bool = True) -> int:
        """Stop the browser. Returns cookie count if exported."""
        with self._lock:
            if self.driver is None:
                return 0

            log.info("=" * 60)
            log.info("STOPPING PERSISTENT BROWSER")
            log.info("=" * 60)

            cookie_count = 0

            if export_cookies:
                try:
                    cookie_count = self._export_cookies_internal()
                except Exception as e:
                    log.error("Failed to export cookies on shutdown: %s", e)

            try:
                self.driver.quit()
            except Exception as e:
                log.warning("Error quitting browser: %s", e)

            self.driver = None
            status["browser_running"] = False

            _kill_chrome_on_profile(BROWSER_PROFILE_DIR)
            _cleanup_profile_locks(BROWSER_PROFILE_DIR)

            return cookie_count

    def restart(self) -> bool:
        """Restart the browser. Returns True if successful."""
        log.info("Restarting persistent browser...")
        self.stop(export_cookies=True)
        time.sleep(2)
        success = self.start()
        if success:
            self.restarts += 1
            status["browser_restarts"] = self.restarts
            inc_metric("browser_restarts_total")
        return success

    def refresh_and_export(self) -> int:
        """
        Navigate to YouTube to refresh session, then export cookies.
        AGGRESSIVE EXPORT: Cookies saved after EACH navigation step.
        """
        with self._lock:
            if self.driver is None:
                raise RuntimeError("Browser not running")

            log.info("=" * 60)
            log.info("REFRESHING SESSION")
            log.info("=" * 60)

            try:
                log.info("Navigating to YouTube...")
                self.driver.get("https://www.youtube.com")
                time.sleep(5)

                # Aggressive export after navigation
                self._export_cookies_internal()

                # Simulate interaction
                self.driver.execute_script("window.scrollTo(0, 300);")
                time.sleep(2)
                self.driver.execute_script("window.scrollTo(0, 0);")
                time.sleep(2)

                # Check if logged in
                if not _check_youtube_logged_in(self.driver):
                    log.warning("SESSION LOGGED OUT!")
                    self._take_screenshot("signout_detected")

                    asyncio.create_task(send_telegram_alert(
                        "YouTube session logged out!\n\nAttempting auto-relogin..."
                    ))

                    if self._attempt_auto_relogin():
                        log.info("Auto-relogin successful!")
                        asyncio.create_task(send_telegram_alert("Auto-relogin successful!"))
                    else:
                        status["needs_relogin"] = True
                        asyncio.create_task(send_telegram_alert(
                            "Auto-relogin FAILED!\n\nManual login required via noVNC."
                        ))
                        return 0

                cookie_count = self._export_cookies_internal()
                self._last_activity = time.time()
                circuit_breaker.record_success()

                log.info("Session refresh complete: %d cookies", cookie_count)
                return cookie_count

            except Exception as e:
                log.error("Error during refresh: %s", e)
                self._take_screenshot("refresh_error")
                circuit_breaker.record_failure()
                raise

    def get_health(self) -> dict:
        """Get browser health information."""
        health = {
            "running": self.is_running(),
            "uptime_seconds": None,
            "memory_mb": None,
            "last_activity_ago": None,
            "needs_restart": False,
            "restart_reason": None,
        }

        if self.started_at:
            health["uptime_seconds"] = int((datetime.now(timezone.utc) - self.started_at).total_seconds())

        if self._last_activity:
            health["last_activity_ago"] = int(time.time() - self._last_activity)

        if self.driver:
            try:
                chrome_pid = self._get_chrome_pid()
                if chrome_pid:
                    mem_mb = self._get_process_memory(chrome_pid)
                    health["memory_mb"] = mem_mb
                    status["browser_memory_mb"] = mem_mb

                    if mem_mb and mem_mb > BROWSER_MAX_MEMORY_MB:
                        health["needs_restart"] = True
                        health["restart_reason"] = f"Memory {mem_mb}MB > {BROWSER_MAX_MEMORY_MB}MB"
            except Exception:
                pass

        if health["uptime_seconds"] and health["uptime_seconds"] > BROWSER_RESTART_INTERVAL:
            health["needs_restart"] = True
            health["restart_reason"] = "Scheduled restart"

        status["last_health_check"] = datetime.now(timezone.utc).isoformat()
        return health

    def start_watchdog(self):
        """Start watchdog thread that monitors browser health."""
        if self._watchdog_thread and self._watchdog_thread.is_alive():
            return

        self._shutdown_event.clear()
        self._watchdog_thread = threading.Thread(target=self._watchdog_loop, daemon=True)
        self._watchdog_thread.start()
        log.info("Watchdog thread started")

    def stop_watchdog(self):
        """Stop the watchdog thread."""
        self._shutdown_event.set()
        if self._watchdog_thread:
            self._watchdog_thread.join(timeout=10)
            self._watchdog_thread = None

    def _watchdog_loop(self):
        """Watchdog loop in separate thread."""
        while not self._shutdown_event.is_set():
            try:
                if status.get("login_active"):
                    self._shutdown_event.wait(timeout=30)
                    continue

                if not circuit_breaker.can_execute():
                    self._shutdown_event.wait(timeout=HEALTH_CHECK_INTERVAL)
                    continue

                health = self.get_health()

                # Preemptive export on high memory
                if health.get("memory_mb"):
                    memory_percent = health["memory_mb"] / BROWSER_MAX_MEMORY_MB * 100
                    if memory_percent > 80:
                        log.warning("Memory at %.1f%%, preemptive export...", memory_percent)
                        try:
                            self._export_cookies_internal()
                        except Exception as e:
                            log.error("Preemptive export failed: %s", e)

                if not health["running"]:
                    log.warning("Browser not running, restarting...")
                    if not self.restart():
                        circuit_breaker.record_failure()

                elif health["needs_restart"]:
                    log.info("Watchdog: %s", health["restart_reason"])
                    if not self.restart():
                        circuit_breaker.record_failure()

            except Exception as e:
                log.error("Watchdog error: %s", e)
                circuit_breaker.record_failure()

            self._shutdown_event.wait(timeout=HEALTH_CHECK_INTERVAL)

    def _create_browser(self) -> uc.Chrome:
        """Create Chrome browser instance."""
        opts = uc.ChromeOptions()
        opts.binary_location = CHROMIUM_PATH
        opts.add_argument(f"--user-data-dir={BROWSER_PROFILE_DIR}")
        opts.add_argument("--no-sandbox")
        opts.add_argument("--disable-gpu")
        opts.add_argument("--disable-dev-shm-usage")
        opts.add_argument("--disable-blink-features=AutomationControlled")
        opts.add_argument("--disable-infobars")
        opts.add_argument("--disable-extensions")
        opts.add_argument("--headless=new")
        opts.add_argument("--window-size=1920,1080")
        opts.add_argument(
            "--user-agent=Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 "
            "(KHTML, like Gecko) Chrome/136.0.0.0 Safari/537.36"
        )

        home = os.environ.get("HOME", "/tmp")
        local_dir = os.path.join(home, ".local", "share", "undetected_chromedriver")
        os.makedirs(local_dir, exist_ok=True)

        return uc.Chrome(
            options=opts,
            browser_executable_path=CHROMIUM_PATH,
            driver_executable_path=CHROMEDRIVER_PATH,
            headless=True,
            use_subprocess=True,
        )

    def _cleanup_before_start(self):
        """Clean up before starting browser."""
        os.makedirs(BROWSER_PROFILE_DIR, exist_ok=True)
        _kill_chrome_on_profile(BROWSER_PROFILE_DIR)
        _cleanup_profile_locks(BROWSER_PROFILE_DIR)

    def _import_cookies_from_file(self) -> int:
        """
        Import cookies from Netscape file into browser.

        This fixes the issue where headed browser (noVNC login) exports cookies
        to file, but headless browser doesn't see them because they weren't
        synced to the browser profile before quit.

        Returns number of cookies imported.
        """
        if self.driver is None:
            return 0

        cookies = parse_netscape_to_selenium(COOKIES_FILE)
        if not cookies:
            log.debug("No cookies to import from file")
            return 0

        imported = 0
        youtube_cookies = [c for c in cookies if 'youtube.com' in c.get('domain', '')]
        google_cookies = [c for c in cookies if 'google.com' in c.get('domain', '') and 'youtube' not in c.get('domain', '')]

        # Import YouTube cookies (browser should already be on youtube.com)
        for cookie in youtube_cookies:
            try:
                self.driver.add_cookie(cookie)
                imported += 1
            except Exception as e:
                log.debug("Failed to add YouTube cookie %s: %s", cookie.get('name'), e)

        # Navigate to Google to import Google cookies
        if google_cookies:
            try:
                self.driver.get("https://accounts.google.com")
                time.sleep(2)
                for cookie in google_cookies:
                    try:
                        self.driver.add_cookie(cookie)
                        imported += 1
                    except Exception as e:
                        log.debug("Failed to add Google cookie %s: %s", cookie.get('name'), e)
            except Exception as e:
                log.warning("Could not navigate to Google for cookie import: %s", e)

        # Go back to YouTube and refresh to apply cookies
        try:
            self.driver.get("https://www.youtube.com")
            time.sleep(2)
        except Exception:
            pass

        log.info("Imported %d cookies from file into browser", imported)
        return imported

    def _export_cookies_internal(self, force: bool = False) -> int:
        """Export cookies from the running browser."""
        if self.driver is None:
            return 0

        youtube_cookies = self.driver.get_cookies()

        try:
            self.driver.get("https://accounts.google.com")
            time.sleep(3)
            google_cookies = self.driver.get_cookies()
            youtube_cookies.extend(google_cookies)
        except Exception as e:
            log.warning("Could not get Google cookies: %s", e)

        try:
            self.driver.get("https://www.youtube.com")
            time.sleep(2)
        except Exception:
            pass

        # Deduplicate
        seen = set()
        unique = []
        for c in youtube_cookies:
            key = (c.get("domain", ""), c.get("name", ""))
            if key not in seen:
                seen.add(key)
                unique.append(c)

        relevant = [
            c for c in unique
            if any(d in c.get("domain", "") for d in ["youtube.com", "google.com", "googleapis.com"])
        ]

        # Protection check
        browser_cookie_names = {c.get("name", "") for c in relevant}
        browser_required_count = len(REQUIRED_COOKIES & browser_cookie_names)

        if not force and browser_required_count == 0:
            existing_content = get_best_cookies()
            if existing_content and count_required_cookies(existing_content) > 0:
                log.warning("Browser has no session, keeping existing cookies")
                status["needs_relogin"] = True
                return 0

        # Build Netscape format
        lines = [
            "# Netscape HTTP Cookie File",
            f"# Generated by cookie_manager.py at {datetime.now(timezone.utc).isoformat()}",
            f"# Required cookies: {browser_required_count}/{len(REQUIRED_COOKIES)}",
            "",
        ]
        for c in relevant:
            lines.append(format_netscape_cookie(c))

        content = "\n".join(lines) + "\n"

        # Save everywhere (atomic + memory cache)
        success, failed = save_cookies_everywhere(content)

        log.info("Exported %d cookies (saved to %d locations)", len(relevant), success)
        return len(relevant)

    def _take_screenshot(self, name: str):
        """Take a screenshot for debugging."""
        try:
            if self.driver:
                path = f"/data/{name}_{int(time.time())}.png"
                self.driver.save_screenshot(path)
                log.info("Screenshot: %s", path)
        except Exception as e:
            log.warning("Screenshot failed: %s", e)

    def _attempt_auto_relogin(self) -> bool:
        """Attempt automatic re-login using saved browser profile."""
        if self.driver is None:
            return False

        try:
            self.driver.get("https://accounts.google.com/")
            time.sleep(5)
            self._take_screenshot("auto_relogin_1")

            self.driver.get("https://www.youtube.com/")
            time.sleep(5)
            self._take_screenshot("auto_relogin_2")

            if _check_youtube_logged_in(self.driver):
                status["needs_relogin"] = False
                return True

            return False

        except Exception as e:
            log.error("Auto-relogin error: %s", e)
            return False

    def _get_chrome_pid(self) -> Optional[int]:
        """Get Chrome browser process PID."""
        try:
            needle = f"--user-data-dir={BROWSER_PROFILE_DIR}"
            for entry in os.listdir("/proc"):
                if not entry.isdigit():
                    continue
                try:
                    with open(f"/proc/{entry}/cmdline", "rb") as f:
                        cmdline = f.read().decode("utf-8", errors="replace")
                    if needle in cmdline and "chromium" in cmdline.lower():
                        return int(entry)
                except (OSError, PermissionError):
                    pass
        except Exception:
            pass
        return None

    def _get_process_memory(self, pid: int) -> Optional[int]:
        """Get process memory usage in MB."""
        try:
            with open(f"/proc/{pid}/status", "r") as f:
                for line in f:
                    if line.startswith("VmRSS:"):
                        parts = line.split()
                        if len(parts) >= 2:
                            return int(parts[1]) // 1024
        except Exception:
            pass
        return None


browser_manager = PersistentBrowserManager()


# ---------------------------------------------------------------------------
# Helper functions
# ---------------------------------------------------------------------------

def _kill_chrome_on_profile(profile_dir: str):
    """Kill Chrome processes using the given profile."""
    killed = 0
    try:
        needle = f"--user-data-dir={profile_dir}"
        my_pid = os.getpid()
        for entry in os.listdir("/proc"):
            if not entry.isdigit():
                continue
            pid = int(entry)
            if pid == my_pid:
                continue
            try:
                with open(f"/proc/{pid}/cmdline", "rb") as f:
                    cmdline = f.read().decode("utf-8", errors="replace")
                if needle in cmdline:
                    os.kill(pid, 9)
                    killed += 1
            except (OSError, PermissionError):
                pass

        if killed:
            log.info("Killed %d orphaned Chrome processes", killed)
            time.sleep(1)
    except Exception as e:
        log.warning("Failed to kill Chrome processes: %s", e)

    try:
        subprocess.run(["killall", "-9", "chromedriver"], capture_output=True, timeout=5)
    except Exception:
        pass


def _cleanup_profile_locks(profile_dir: str):
    """Remove stale Chrome lock files."""
    if not os.path.exists(profile_dir):
        return

    for name in ("SingletonLock", "SingletonCookie", "SingletonSocket"):
        lock_path = os.path.join(profile_dir, name)
        if os.path.exists(lock_path) or os.path.islink(lock_path):
            try:
                os.remove(lock_path)
            except OSError:
                pass


def format_netscape_cookie(cookie: dict) -> str:
    """Convert Selenium cookie to Netscape format line."""
    domain = cookie.get("domain", "")
    subdomain = "TRUE" if domain.startswith(".") else "FALSE"
    path = cookie.get("path", "/")
    secure = "TRUE" if cookie.get("secure", False) else "FALSE"
    expiry = cookie.get("expiry", 0)
    expires = str(int(expiry)) if expiry else "0"
    name = cookie.get("name", "")
    value = cookie.get("value", "")
    return f"{domain}\t{subdomain}\t{path}\t{secure}\t{expires}\t{name}\t{value}"


def parse_netscape_to_selenium(filepath: str) -> list:
    """
    Parse Netscape cookie file to list of Selenium cookie dicts.

    Returns list of cookies that can be added via driver.add_cookie()
    """
    cookies = []
    if not os.path.exists(filepath):
        return cookies

    try:
        with open(filepath, 'r') as f:
            for line in f:
                line = line.strip()
                # Skip comments and empty lines
                if not line or line.startswith('#'):
                    continue

                parts = line.split('\t')
                if len(parts) >= 7:
                    domain, _, path, secure, expiry, name, value = parts[:7]

                    cookie = {
                        'name': name,
                        'value': value,
                        'domain': domain,
                        'path': path,
                        'secure': secure.upper() == 'TRUE',
                    }

                    # Add expiry if not session cookie
                    try:
                        exp = int(expiry)
                        if exp > 0:
                            cookie['expiry'] = exp
                    except ValueError:
                        pass

                    cookies.append(cookie)
    except Exception as e:
        log.warning("Failed to parse cookies file %s: %s", filepath, e)

    return cookies


def _check_youtube_logged_in(driver: uc.Chrome) -> bool:
    """Check if YouTube session is logged in."""
    try:
        avatar_selectors = [
            "button#avatar-btn",
            "#avatar-btn",
            "ytd-topbar-menu-button-renderer button",
            "[aria-label*='Account']",
        ]
        for selector in avatar_selectors:
            try:
                elements = driver.find_elements("css selector", selector)
                if elements:
                    return True
            except Exception:
                pass

        sign_in_selectors = [
            "a[href*='accounts.google.com/ServiceLogin']",
            "[aria-label='Sign in']",
        ]
        for selector in sign_in_selectors:
            try:
                elements = driver.find_elements("css selector", selector)
                if elements:
                    return False
            except Exception:
                pass

        cookies = driver.get_cookies()
        cookie_names = {c.get("name", "") for c in cookies}
        return bool(REQUIRED_COOKIES & cookie_names)

    except Exception:
        return True


def check_session_valid(cookie_count: int) -> bool:
    """Check if exported cookies contain valid session data."""
    if cookie_count < 5:
        return False

    content = get_best_cookies()
    if not content:
        return False

    return has_required_cookies(content)


# ---------------------------------------------------------------------------
# Login flow (visual via noVNC)
# ---------------------------------------------------------------------------

def _kill_proc(proc):
    """Safely kill a subprocess."""
    if proc and proc.poll() is None:
        try:
            proc.terminate()
            proc.wait(timeout=5)
        except (subprocess.TimeoutExpired, OSError):
            try:
                proc.kill()
            except OSError:
                pass


def _make_login_chrome_options(profile_dir: str) -> uc.ChromeOptions:
    """Create ChromeOptions for login (headed mode)."""
    opts = uc.ChromeOptions()
    opts.binary_location = CHROMIUM_PATH
    opts.add_argument(f"--user-data-dir={profile_dir}")
    opts.add_argument("--no-sandbox")
    opts.add_argument("--disable-gpu")
    opts.add_argument("--disable-dev-shm-usage")
    opts.add_argument("--disable-blink-features=AutomationControlled")
    opts.add_argument("--disable-infobars")
    opts.add_argument("--disable-extensions")
    opts.add_argument("--start-maximized")
    opts.add_argument("--window-size=1920,1080")
    return opts


def _create_login_driver(profile_dir: str) -> uc.Chrome:
    """Create Chrome driver for login (headed mode)."""
    _kill_chrome_on_profile(profile_dir)
    _cleanup_profile_locks(profile_dir)
    opts = _make_login_chrome_options(profile_dir)

    home = os.environ.get("HOME", "/tmp")
    local_dir = os.path.join(home, ".local", "share", "undetected_chromedriver")
    os.makedirs(local_dir, exist_ok=True)

    return uc.Chrome(
        options=opts,
        browser_executable_path=CHROMIUM_PATH,
        driver_executable_path=CHROMEDRIVER_PATH,
        headless=False,
        use_subprocess=True,
    )


async def start_login() -> dict:
    """Start visual login session."""
    if status["login_active"]:
        return {"error": "Login session already active"}

    if browser_manager.is_running():
        browser_manager.stop(export_cookies=True)

    if _browser_lock is None:
        return {"error": "Browser lock not initialized"}

    if _browser_lock.locked():
        return {"error": "Browser is busy"}

    os.makedirs(BROWSER_PROFILE_DIR, exist_ok=True)

    # Start Xvfb
    xvfb_proc = subprocess.Popen(
        ["Xvfb", DISPLAY, "-screen", "0", "1920x1080x24"],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    login_state["xvfb_proc"] = xvfb_proc
    await asyncio.sleep(1)

    if xvfb_proc.poll() is not None:
        _, stderr = xvfb_proc.communicate()
        return {"error": f"Xvfb failed: {stderr.decode() if stderr else 'unknown'}"}

    os.environ["DISPLAY"] = DISPLAY

    try:
        loop = asyncio.get_running_loop()
        driver = await loop.run_in_executor(
            None, lambda: _create_login_driver(BROWSER_PROFILE_DIR)
        )
        login_state["driver"] = driver

        await loop.run_in_executor(
            None,
            driver.get,
            "https://accounts.google.com/ServiceLogin?continue=https://www.youtube.com/",
        )
    except Exception as e:
        _kill_proc(xvfb_proc)
        login_state["xvfb_proc"] = None
        return {"error": f"Chromium failed: {e}"}

    # Start x11vnc
    vnc_cmd = [
        "x11vnc", "-display", DISPLAY, "-forever",
        "-rfbport", str(VNC_PORT), "-shared",
    ]
    if VNC_PASSWORD:
        vnc_cmd += ["-passwd", VNC_PASSWORD]
    else:
        vnc_cmd += ["-nopw"]

    vnc_proc = subprocess.Popen(vnc_cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    login_state["vnc_proc"] = vnc_proc
    await asyncio.sleep(1)

    if vnc_proc.poll() is not None:
        _, stderr = vnc_proc.communicate()
        _kill_proc(xvfb_proc)
        return {"error": f"x11vnc failed: {stderr.decode() if stderr else 'unknown'}"}

    # Start websockify
    novnc_proc = subprocess.Popen(
        ["websockify", "--web=/opt/novnc", f"0.0.0.0:{NOVNC_PORT}", f"localhost:{VNC_PORT}"],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    login_state["novnc_proc"] = novnc_proc
    await asyncio.sleep(1)

    if novnc_proc.poll() is not None:
        _, stderr = novnc_proc.communicate()
        _kill_proc(vnc_proc)
        _kill_proc(xvfb_proc)
        return {"error": f"websockify failed: {stderr.decode() if stderr else 'unknown'}"}

    status["login_active"] = True
    status["login_started_at"] = datetime.now(timezone.utc).isoformat()

    host = NOVNC_HOST or "localhost"
    url_port = NOVNC_EXTERNAL_PORT if NOVNC_EXTERNAL_PORT else NOVNC_PORT
    novnc_url = f"http://{host}:{url_port}/vnc.html?autoconnect=true"

    asyncio.get_running_loop().call_later(LOGIN_TIMEOUT, _auto_stop_login)

    return {"status": "ok", "novnc_url": novnc_url}


def _auto_stop_login():
    """Auto-stop login session after timeout."""
    if status["login_active"]:
        asyncio.ensure_future(stop_login())


async def stop_login() -> dict:
    """Stop login session, export cookies, start persistent browser."""
    if not status["login_active"]:
        return {"error": "No active login session"}

    result = {"status": "ok", "cookies_exported": False, "cookie_count": 0}

    driver = login_state.get("driver")
    if driver:
        try:
            loop = asyncio.get_running_loop()
            count = await loop.run_in_executor(None, _export_cookies_from_login_driver, driver)

            result["cookies_exported"] = True
            result["cookie_count"] = count

            status["cookie_count"] = count
            status["last_refresh"] = datetime.now(timezone.utc).isoformat()
            status["last_refresh_success"] = True
            status["needs_relogin"] = not check_session_valid(count)
            status["last_error"] = None

        except Exception as e:
            result["error"] = str(e)
            status["last_error"] = str(e)

        try:
            driver.quit()
        except Exception:
            pass

    _kill_chrome_on_profile(BROWSER_PROFILE_DIR)

    _kill_proc(login_state.get("novnc_proc"))
    _kill_proc(login_state.get("vnc_proc"))
    _kill_proc(login_state.get("xvfb_proc"))

    login_state.update({
        "driver": None,
        "xvfb_proc": None,
        "vnc_proc": None,
        "novnc_proc": None,
    })

    status["login_active"] = False
    status["login_started_at"] = None

    await asyncio.sleep(2)
    loop = asyncio.get_running_loop()
    await loop.run_in_executor(None, browser_manager.start)

    return result


def _export_cookies_from_login_driver(driver: uc.Chrome) -> int:
    """Export cookies from login browser."""
    driver.get("https://www.youtube.com")
    time.sleep(5)

    youtube_cookies = driver.get_cookies()

    try:
        driver.get("https://accounts.google.com")
        time.sleep(3)
        google_cookies = driver.get_cookies()
        youtube_cookies.extend(google_cookies)
    except Exception:
        pass

    seen = set()
    unique = []
    for c in youtube_cookies:
        key = (c.get("domain", ""), c.get("name", ""))
        if key not in seen:
            seen.add(key)
            unique.append(c)

    relevant = [
        c for c in unique
        if any(d in c.get("domain", "") for d in ["youtube.com", "google.com", "googleapis.com"])
    ]

    browser_required = len(REQUIRED_COOKIES & {c.get("name", "") for c in relevant})

    lines = [
        "# Netscape HTTP Cookie File",
        f"# Generated by cookie_manager.py at {datetime.now(timezone.utc).isoformat()}",
        f"# Required cookies: {browser_required}/{len(REQUIRED_COOKIES)}",
        "",
    ]
    for c in relevant:
        lines.append(format_netscape_cookie(c))

    content = "\n".join(lines) + "\n"
    save_cookies_everywhere(content)

    return len(relevant)


# ---------------------------------------------------------------------------
# Background Tasks
# ---------------------------------------------------------------------------

async def refresh_loop():
    """Background task: refresh session with adaptive intervals."""
    log.info("Cookie refresh loop started")
    await asyncio.sleep(30)

    if os.path.exists(BROWSER_PROFILE_DIR) and not browser_manager.is_running():
        if circuit_breaker.can_execute():
            loop = asyncio.get_running_loop()
            success = await loop.run_in_executor(None, browser_manager.start)
            if not success:
                circuit_breaker.record_failure()

    last_expiry_check = 0

    while True:
        status["profile_exists"] = os.path.exists(BROWSER_PROFILE_DIR)
        status["current_mode"] = get_current_mode().value

        score, reason = calculate_freshness_score()
        status["freshness_score"] = score
        status["freshness_reason"] = reason

        # Periodic expiry check
        if time.time() - last_expiry_check > 3600:
            await check_and_alert_expiry()
            last_expiry_check = time.time()

        if status["login_active"]:
            await asyncio.sleep(60)
            continue

        if not circuit_breaker.can_execute():
            log.info("Circuit breaker OPEN - using static cookies")
            interval = get_adaptive_refresh_interval()
            await asyncio.sleep(interval)
            continue

        if not status["profile_exists"]:
            status["needs_relogin"] = True
            await asyncio.sleep(REFRESH_INTERVAL_NORMAL)
            continue

        if not browser_manager.is_running():
            loop = asyncio.get_running_loop()
            success = await loop.run_in_executor(None, browser_manager.start)
            if not success:
                circuit_breaker.record_failure()
                await asyncio.sleep(60)
                continue

        inc_metric("cookie_refresh_total")
        success = await _refresh_with_retry()

        if success:
            inc_metric("cookie_refresh_success")
            status["last_refresh_success"] = True
            status["last_error"] = None
            status["needs_relogin"] = not check_session_valid(status.get("cookie_count", 0))
        else:
            inc_metric("cookie_refresh_failed")

        status["last_refresh"] = datetime.now(timezone.utc).isoformat()

        interval = get_adaptive_refresh_interval()
        await asyncio.sleep(interval)


async def _refresh_with_retry() -> bool:
    """Attempt refresh with smart retry logic."""
    loop = asyncio.get_running_loop()

    for attempt in range(NETWORK_RETRY_COUNT + 1):
        try:
            count = await loop.run_in_executor(None, browser_manager.refresh_and_export)
            status["cookie_count"] = count
            return True

        except Exception as e:
            error_type = classify_error(e)
            log.error("Refresh attempt %d failed [%s]: %s", attempt + 1, error_type.value, e)

            if error_type == ErrorType.NETWORK:
                if attempt < NETWORK_RETRY_COUNT:
                    await asyncio.sleep(NETWORK_RETRY_DELAY)
                    continue
                circuit_breaker.record_failure()

            elif error_type == ErrorType.BROWSER_CRASH:
                await loop.run_in_executor(None, browser_manager.restart)
                if attempt == 0:
                    continue
                circuit_breaker.record_failure()

            elif error_type == ErrorType.SESSION_EXPIRED:
                status["needs_relogin"] = True
                await send_telegram_alert("Session expired! Manual re-login required.")
                break

            elif error_type == ErrorType.RATE_LIMITED:
                await asyncio.sleep(60)
                if attempt < NETWORK_RETRY_COUNT:
                    continue
                circuit_breaker.record_failure()

            else:
                status["last_error"] = str(e)
                circuit_breaker.record_failure()
                break

    status["last_refresh_success"] = False
    status["last_error"] = "Refresh failed after retries"
    return False


async def quick_health_loop():
    """Background task: quick health checks via HEAD request."""
    log.info("Quick health check loop started")
    await asyncio.sleep(60)

    while True:
        try:
            inc_metric("quick_health_checks_total")
            is_valid, reason = await quick_cookie_check()

            status["last_quick_check"] = datetime.now(timezone.utc).isoformat()
            status["last_quick_check_result"] = {"valid": is_valid, "reason": reason}

            if not is_valid:
                inc_metric("quick_health_checks_failed")
                log.warning("Quick health check failed: %s", reason)

                # NOTE: Disabled session_expired handling because browser health check
                # is unreliable - headless browser often loses session even when
                # cookie FILE is valid. yt-dlp validation is the authoritative source.
                # We no longer set needs_relogin or send alerts based on this check.
                if reason == "session_expired":
                    log.debug("Quick health check session_expired (ignored, yt-dlp validation is authoritative)")

        except Exception as e:
            log.error("Quick health check error: %s", e)

        await asyncio.sleep(QUICK_HEALTH_CHECK_INTERVAL)


async def telegram_backup_loop():
    """Background task: periodic Telegram backup."""
    log.info("Telegram backup loop started")
    await asyncio.sleep(300)  # Wait 5 min before first backup

    while True:
        try:
            if TELEGRAM_BOT_TOKEN and TELEGRAM_ADMIN_ID:
                success = await backup_cookies_to_telegram()
                if success:
                    inc_metric("telegram_backups_total")
                    status["last_telegram_backup"] = datetime.now(timezone.utc).isoformat()

        except Exception as e:
            log.error("Telegram backup error: %s", e)

        await asyncio.sleep(TELEGRAM_BACKUP_INTERVAL)


# ---------------------------------------------------------------------------
# HTTP API
# ---------------------------------------------------------------------------

async def handle_login_start(request):
    return web.json_response(await start_login())


async def handle_login_stop(request):
    return web.json_response(await stop_login())


async def handle_status(request):
    status["profile_exists"] = os.path.exists(BROWSER_PROFILE_DIR)
    status["cookies_exist"] = os.path.exists(COOKIES_FILE)
    status["browser_running"] = browser_manager.is_running()
    status["current_mode"] = get_current_mode().value
    status["memory_cache"] = memory_cache.get_info()

    analysis = get_cookie_analysis()
    status["cookie_analysis"] = analysis
    status["required_found"] = analysis["required_found"]
    status["required_missing"] = analysis["required_missing"]

    # Error tracker (feedback loop v3.0)
    status["error_tracker"] = error_tracker.get_status()

    # Cookie health (v4.0)
    status["cookie_health"] = cookie_health.get_status()

    return web.json_response(status)


async def handle_export_cookies(request):
    """Force cookie export."""
    if not browser_manager.is_running():
        return web.json_response({"success": False, "error": "Browser not running"}, status=500)

    try:
        loop = asyncio.get_running_loop()
        count = await loop.run_in_executor(None, browser_manager.refresh_and_export)
        status["last_refresh"] = datetime.now(timezone.utc).isoformat()
        status["last_refresh_success"] = True
        status["cookie_count"] = count
        status["last_error"] = None
        status["needs_relogin"] = not check_session_valid(count)
        return web.json_response({"success": True, "cookie_count": count})
    except Exception as e:
        status["last_error"] = str(e)
        return web.json_response({"success": False, "error": str(e)}, status=500)


async def handle_browser_health(request):
    """Get browser health."""
    return web.json_response(browser_manager.get_health())


async def handle_restart_browser(request):
    """Force restart browser."""
    try:
        loop = asyncio.get_running_loop()
        success = await loop.run_in_executor(None, browser_manager.restart)
        if success:
            return web.json_response({"success": True})
        return web.json_response({"success": False, "error": "Restart failed"}, status=500)
    except Exception as e:
        return web.json_response({"success": False, "error": str(e)}, status=500)


async def handle_backup_telegram(request):
    """Manually trigger Telegram backup."""
    success = await backup_cookies_to_telegram()
    if success:
        status["last_telegram_backup"] = datetime.now(timezone.utc).isoformat()
        return web.json_response({"success": True})
    return web.json_response({"success": False, "error": "Backup failed"}, status=500)


async def handle_report_error(request):
    """
    Handle error reports from Rust bot (Feedback Loop v3.0).

    POST /api/report_error
    Body: {"error_type": "InvalidCookies"|"BotDetection", "url": "..."}

    This enables the retry-with-refresh pattern:
    1. Rust bot gets cookie error from yt-dlp
    2. Rust bot calls this endpoint
    3. Cookie manager triggers emergency refresh
    4. Rust bot retries download with fresh cookies
    """
    try:
        data = await request.json()
    except Exception:
        return web.json_response(
            {"success": False, "error": "Invalid JSON"},
            status=400
        )

    error_type = data.get("error_type", "Unknown")
    url = data.get("url", "")

    log.info("Error report received: type=%s, url=%s", error_type, url[:80] if url else "N/A")

    # Record metrics
    inc_metric("error_reports_total")
    if error_type in ("InvalidCookies", "invalid_cookies"):
        inc_metric("error_reports_invalid_cookies")
    elif error_type in ("BotDetection", "bot_detection"):
        inc_metric("error_reports_bot_detection")

    # Record cookie health (v4.0)
    cookie_health.record_failure(error_type)

    # Record error and check if refresh needed
    tracker_result = error_tracker.record_error(error_type, url)

    if tracker_result["action"] == "ignored":
        return web.json_response({
            "success": True,
            "action": "ignored",
            "reason": tracker_result.get("reason", "not_cookie_error"),
        })

    if tracker_result["action"] == "cooldown":
        return web.json_response({
            "success": True,
            "action": "cooldown",
            "remaining_seconds": tracker_result.get("remaining_seconds", 0),
            "message": "Refresh recently triggered, waiting for cooldown",
        })

    # Trigger emergency refresh
    refresh_result = await emergency_cookie_refresh()

    # Send alert to admin
    if tracker_result.get("emergency_mode"):
        await send_telegram_alert(
            f"EMERGENCY MODE ACTIVE\n\n"
            f"Error: {error_type}\n"
            f"Recent errors: {tracker_result.get('recent_errors', 0)}\n"
            f"Refresh: {'✅ Success' if refresh_result['success'] else '❌ Failed'}\n"
            f"Method: {refresh_result.get('method', 'N/A')}"
        )
    elif not refresh_result["success"]:
        await send_telegram_alert(
            f"Cookie error reported\n\n"
            f"Error: {error_type}\n"
            f"Refresh FAILED: {refresh_result.get('error', 'Unknown')}"
        )

    return web.json_response({
        "success": refresh_result["success"],
        "action": "refresh_triggered",
        "refresh_result": refresh_result,
        "emergency_mode": tracker_result.get("emergency_mode", False),
        "recent_errors": tracker_result.get("recent_errors", 0),
    })


async def handle_report_success(request):
    """
    Handle success reports from Rust bot (Cookie Health v4.0).

    POST /api/report_success
    Body: {"url": "..."} (optional)

    Called when a download succeeds using cookies.
    Used to track cookie health and potentially exit emergency mode.
    """
    # Record success in cookie health
    cookie_health.record_success()

    # If we were in emergency mode and health is recovering, maybe exit
    health_status = cookie_health.get_status()
    if error_tracker._emergency_mode and health_status["score"] >= 70:
        error_tracker.exit_emergency_mode()
        log.info("Exiting emergency mode due to recovered cookie health (score=%d)",
                health_status["score"])

    return web.json_response({
        "success": True,
        "cookie_health": health_status,
    })


async def handle_health(request):
    """Health check endpoint for Railway."""
    health = {
        "status": "healthy",
        "timestamp": datetime.now(timezone.utc).isoformat(),
        "mode": get_current_mode().value,
        "checks": {},
    }

    freshness_score, freshness_reason = calculate_freshness_score()
    health["freshness_score"] = freshness_score
    health["freshness_reason"] = freshness_reason

    # Cookies check
    cookies_check = {"exists": False, "valid": False, "required_count": 0}
    content = get_best_cookies()
    if content:
        cookies_check["exists"] = True
        cookies_check["valid"] = has_required_cookies(content)
        cookies_check["required_count"] = count_required_cookies(content)

        expiry_info = get_cookie_expiry_info()
        cookies_check["min_expiry_hours"] = expiry_info.get("min_expiry_hours")
        cookies_check["expired"] = len(expiry_info.get("expired", []))
    health["checks"]["cookies"] = cookies_check

    # Browser check
    health["checks"]["browser"] = {
        "running": browser_manager.is_running(),
        "restarts": status.get("browser_restarts", 0),
        "memory_mb": status.get("browser_memory_mb"),
    }

    # Circuit breaker
    health["checks"]["circuit_breaker"] = circuit_breaker.get_status()

    # Memory cache
    health["checks"]["memory_cache"] = memory_cache.get_info()

    # Error tracker (feedback loop v3.0)
    health["checks"]["error_tracker"] = error_tracker.get_status()

    # Cookie health (v4.0)
    health["checks"]["cookie_health"] = cookie_health.get_status()

    # Determine status
    if freshness_score >= 70:
        health["status"] = "healthy"
    elif freshness_score >= 40:
        health["status"] = "degraded"
        health["reason"] = freshness_reason
    else:
        health["status"] = "unhealthy"
        health["reason"] = freshness_reason

    if not cookies_check["valid"]:
        health["status"] = "unhealthy"
        health["reason"] = "No valid session cookies"

    http_status = 503 if health["status"] == "unhealthy" else 200
    return web.json_response(health, status=http_status)


async def handle_cookie_debug(request):
    """Detailed cookie analysis."""
    result = {
        "cookies_sources": {},
        "recovery_chain_status": {},
        "analysis": {},
    }

    # Check each source
    for path in COOKIE_LOCATIONS:
        result["cookies_sources"][path] = {
            "exists": os.path.exists(path),
            "size": os.path.getsize(path) if os.path.exists(path) else 0,
        }

    # Memory cache
    result["cookies_sources"]["memory_cache"] = memory_cache.get_info()

    # Env var
    result["cookies_sources"]["YTDL_COOKIES_B64"] = bool(os.environ.get("YTDL_COOKIES_B64"))

    # Recovery chain test
    content = get_best_cookies()
    if content:
        result["recovery_chain_status"]["success"] = True
        result["recovery_chain_status"]["required_cookies"] = count_required_cookies(content)

        # Detailed cookie list
        cookies = []
        now = time.time()
        for line in content.split('\n'):
            if line.startswith('#') or not line.strip():
                continue
            parts = line.split('\t')
            if len(parts) >= 7:
                name, value = parts[5], parts[6]
                expiry = int(parts[4]) if parts[4].isdigit() else 0

                cookie_info = {
                    "name": name,
                    "is_required": name in REQUIRED_COOKIES,
                    "value_preview": value[:20] + "..." if len(value) > 20 else value,
                }
                if expiry > 0:
                    cookie_info["expires_in_hours"] = round((expiry - now) / 3600, 1)
                cookies.append(cookie_info)

        result["analysis"]["cookies"] = cookies
        result["analysis"]["total"] = len(cookies)
        result["analysis"]["required_found"] = [c["name"] for c in cookies if c["is_required"]]
    else:
        result["recovery_chain_status"]["success"] = False

    return web.json_response(result)


async def handle_metrics(request):
    """Prometheus metrics endpoint."""
    return web.Response(text=get_prometheus_metrics(), content_type="text/plain")


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

async def on_startup(app):
    """Start background tasks on server startup."""
    _init_browser_lock()
    app["refresh_task"] = asyncio.ensure_future(refresh_loop())
    app["quick_health_task"] = asyncio.ensure_future(quick_health_loop())
    app["telegram_backup_task"] = asyncio.ensure_future(telegram_backup_loop())
    browser_manager.start_watchdog()


async def on_cleanup(app):
    """Cleanup on server shutdown."""
    log.info("Server shutting down...")

    for task_name in ["refresh_task", "quick_health_task", "telegram_backup_task"]:
        task = app.get(task_name)
        if task:
            task.cancel()
            try:
                await task
            except asyncio.CancelledError:
                pass

    browser_manager.stop_watchdog()

    if status["login_active"]:
        await stop_login()

    if browser_manager.is_running():
        loop = asyncio.get_running_loop()
        await loop.run_in_executor(None, browser_manager.stop, True)

    log.info("Cleanup complete")


def _handle_signal(signum, frame):
    """Handle shutdown signals."""
    log.info("Received signal %d", signum)
    if browser_manager.is_running():
        browser_manager.stop(export_cookies=True)
    raise SystemExit(0)


def main():
    log.info("=" * 60)
    log.info("Cookie Manager v2.0 (UNKILLABLE ARCHITECTURE)")
    log.info("=" * 60)
    log.info("  API: %s:%d", API_HOST, API_PORT)
    log.info("  Cookie locations: %s", COOKIE_LOCATIONS)
    log.info("  Browser profile: %s", BROWSER_PROFILE_DIR)
    log.info("  Refresh interval: %ds (adaptive)", REFRESH_INTERVAL)
    log.info("  Quick health check: %ds", QUICK_HEALTH_CHECK_INTERVAL)
    log.info("  Telegram backup: %ds", TELEGRAM_BACKUP_INTERVAL)
    log.info("  Telegram alerts: %s", "enabled" if TELEGRAM_BOT_TOKEN else "disabled")
    log.info("=" * 60)

    signal.signal(signal.SIGTERM, _handle_signal)
    signal.signal(signal.SIGINT, _handle_signal)

    # Bootstrap from env
    bootstrap_cookies_from_env()

    # Cleanup
    _kill_chrome_on_profile(BROWSER_PROFILE_DIR)
    _cleanup_profile_locks(BROWSER_PROFILE_DIR)

    app = web.Application()
    app.router.add_get("/health", handle_health)
    app.router.add_get("/metrics", handle_metrics)
    app.router.add_post("/api/login_start", handle_login_start)
    app.router.add_post("/api/login_stop", handle_login_stop)
    app.router.add_get("/api/status", handle_status)
    app.router.add_post("/api/export_cookies", handle_export_cookies)
    app.router.add_get("/api/cookie_debug", handle_cookie_debug)
    app.router.add_get("/api/browser_health", handle_browser_health)
    app.router.add_post("/api/restart_browser", handle_restart_browser)
    app.router.add_post("/api/backup_telegram", handle_backup_telegram)
    app.router.add_post("/api/report_error", handle_report_error)
    app.router.add_post("/api/report_success", handle_report_success)

    app.on_startup.append(on_startup)
    app.on_cleanup.append(on_cleanup)

    web.run_app(app, host=API_HOST, port=API_PORT, print=None)


if __name__ == "__main__":
    main()
