#!/usr/bin/env python3
"""
YouTube Cookie Manager — aiohttp server + Persistent Browser Architecture.

Manages YouTube cookies automatically with a PERSISTENT headless browser:
1. Browser starts once at startup and stays running
2. Cookie refresh via navigation (not browser restart) every 30 minutes
3. Session cookies are preserved because browser never closes

Key difference from previous approach:
- OLD: Start Chrome → navigate → export → quit Chrome (session cookies lost!)
- NEW: Start Chrome once → navigate periodically → export → browser stays running

API endpoints:
  POST /api/login_start     — Start visual login session (Xvfb + noVNC)
  POST /api/login_stop      — Stop login, export cookies
  GET  /api/status          — Cookie manager status
  POST /api/export_cookies  — Force cookie re-export
  GET  /api/browser_health  — Check persistent browser health
  POST /api/restart_browser — Force restart persistent browser
  GET  /api/cookie_debug    — Detailed cookie analysis
"""

import asyncio
import logging
import os
import signal
import subprocess
import threading
import time
from datetime import datetime, timezone
from typing import Optional

from aiohttp import web
import undetected_chromedriver as uc

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------

_raw_cookies_file = os.environ.get("YTDL_COOKIES_FILE", "/data/youtube_cookies.txt")
COOKIES_FILE = _raw_cookies_file if os.path.isabs(_raw_cookies_file) else os.path.join("/data", _raw_cookies_file)
BROWSER_PROFILE_DIR = os.environ.get("BROWSER_PROFILE_DIR", "/data/browser_profile")
REFRESH_INTERVAL = int(os.environ.get("COOKIE_REFRESH_INTERVAL", "1800"))  # 30 min
LOGIN_TIMEOUT = int(os.environ.get("COOKIE_LOGIN_TIMEOUT", "900"))  # 15 min
API_PORT = int(os.environ.get("COOKIE_MANAGER_PORT", "9876"))
API_HOST = os.environ.get("COOKIE_MANAGER_HOST", "127.0.0.1")
NOVNC_HOST = os.environ.get("NOVNC_HOST", "")
NOVNC_PORT = int(os.environ.get("NOVNC_PORT", "6080"))  # Internal port websockify listens on
NOVNC_EXTERNAL_PORT = int(os.environ.get("NOVNC_EXTERNAL_PORT", "0"))  # External port for URL (0 = same as NOVNC_PORT)
VNC_PORT = 5900
VNC_PASSWORD = os.environ.get("VNC_PASSWORD", "")
DISPLAY = ":99"
CHROMIUM_PATH = os.environ.get("CHROMIUM_PATH", "/usr/bin/chromium-browser")
CHROMEDRIVER_PATH = os.environ.get("CHROMEDRIVER_PATH", "/usr/bin/chromedriver")

# Watchdog configuration
HEALTH_CHECK_INTERVAL = int(os.environ.get("BROWSER_HEALTH_CHECK_INTERVAL", "300"))  # 5 min
BROWSER_MAX_MEMORY_MB = int(os.environ.get("BROWSER_MAX_MEMORY_MB", "1024"))  # 1 GB
BROWSER_RESTART_INTERVAL = int(os.environ.get("BROWSER_RESTART_INTERVAL", "21600"))  # 6 hours

# Required YouTube/Google cookies that indicate a valid session
REQUIRED_COOKIES = {"SID", "HSID", "SSID", "APISID", "SAPISID"}

# All important Google/YouTube cookies to track
TRACKED_COOKIES = {
    "SID", "HSID", "SSID", "APISID", "SAPISID",  # Core session
    "__Secure-1PSID", "__Secure-3PSID",  # Secure session variants
    "__Secure-1PAPISID", "__Secure-3PAPISID",  # Secure API variants
    "LOGIN_INFO",  # YouTube login state
    "PREF", "YSC", "VISITOR_INFO1_LIVE",  # YouTube preferences
    "CONSENT", "SOCS",  # Cookie consent
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
    # Persistent browser status
    "browser_running": False,
    "browser_started_at": None,
    "browser_restarts": 0,
    "last_health_check": None,
    "browser_memory_mb": None,
}

login_state = {
    "driver": None,
    "xvfb_proc": None,
    "vnc_proc": None,
    "novnc_proc": None,
}

# Lock to prevent concurrent browser operations
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

    Key features:
    - Browser starts once and stays alive
    - Session cookies are preserved (not deleted on quit)
    - Periodic navigation refreshes the session
    - Watchdog monitors health and restarts if needed
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
                # Try to get window handles - fails if browser crashed
                _ = self.driver.window_handles
                return True
            except Exception:
                return False

    def start(self) -> bool:
        """Start the persistent browser. Returns True if successful."""
        with self._lock:
            if self.driver is not None:
                log.warning("Browser already running, not starting new one")
                return True

            log.info("=" * 60)
            log.info("STARTING PERSISTENT BROWSER")
            log.info("=" * 60)

            try:
                self._cleanup_before_start()
                self.driver = self._create_browser()
                self.started_at = datetime.now(timezone.utc)
                self._last_activity = time.time()

                # Navigate to YouTube to establish session
                log.info("Initial navigation to YouTube...")
                self.driver.get("https://www.youtube.com")
                time.sleep(3)

                log.info("✅ Persistent browser started successfully")
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
                log.info("Browser not running, nothing to stop")
                return 0

            log.info("=" * 60)
            log.info("STOPPING PERSISTENT BROWSER")
            log.info("=" * 60)

            cookie_count = 0

            if export_cookies:
                try:
                    log.info("Exporting cookies before shutdown...")
                    cookie_count = self._export_cookies_internal()
                    log.info("Exported %d cookies", cookie_count)
                except Exception as e:
                    log.error("Failed to export cookies on shutdown: %s", e)

            try:
                self.driver.quit()
            except Exception as e:
                log.warning("Error quitting browser: %s", e)

            self.driver = None
            status["browser_running"] = False

            # Kill any orphaned processes
            _kill_chrome_on_profile(BROWSER_PROFILE_DIR)
            _cleanup_profile_locks(BROWSER_PROFILE_DIR)

            log.info("Persistent browser stopped")
            return cookie_count

    def restart(self) -> bool:
        """Restart the browser (stop + start). Returns True if successful."""
        log.info("Restarting persistent browser...")
        self.stop(export_cookies=True)
        time.sleep(2)
        success = self.start()
        if success:
            self.restarts += 1
            status["browser_restarts"] = self.restarts
        return success

    def refresh_and_export(self) -> int:
        """
        Navigate to YouTube to refresh session, then export cookies.
        This is the key operation that keeps session alive WITHOUT restarting browser.
        Returns cookie count.
        """
        with self._lock:
            if self.driver is None:
                raise RuntimeError("Browser not running")

            log.info("=" * 60)
            log.info("REFRESHING SESSION (in-browser navigation)")
            log.info("=" * 60)

            try:
                # Navigate to YouTube
                log.info("Navigating to YouTube...")
                self.driver.get("https://www.youtube.com")
                time.sleep(5)

                # Log page state
                _log_page_state(self.driver, "after YouTube navigation")

                # Simulate user interaction to keep session warm
                log.info("Simulating user interaction...")
                self.driver.execute_script("window.scrollTo(0, 300);")
                time.sleep(2)
                self.driver.execute_script("window.scrollTo(0, 0);")
                time.sleep(2)

                # Check if still logged in
                if not _check_youtube_logged_in(self.driver):
                    log.warning("SESSION LOGGED OUT! Attempting auto-relogin...")
                    self._take_screenshot("signout_detected")

                    # Try auto-relogin using saved browser profile
                    if self._attempt_auto_relogin():
                        log.info("✅ Auto-relogin successful!")
                    else:
                        log.error("❌ Auto-relogin failed, manual login required via /browser_login")
                        status["needs_relogin"] = True
                        return 0

                # Export cookies
                cookie_count = self._export_cookies_internal()
                self._last_activity = time.time()

                log.info("✅ Session refresh complete: %d cookies exported", cookie_count)
                return cookie_count

            except Exception as e:
                log.error("Error during refresh: %s", e)
                self._take_screenshot("refresh_error")
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

        # Check memory usage
        if self.driver:
            try:
                # Get Chrome PID and check memory
                chrome_pid = self._get_chrome_pid()
                if chrome_pid:
                    mem_mb = self._get_process_memory(chrome_pid)
                    health["memory_mb"] = mem_mb
                    status["browser_memory_mb"] = mem_mb

                    if mem_mb and mem_mb > BROWSER_MAX_MEMORY_MB:
                        health["needs_restart"] = True
                        health["restart_reason"] = f"Memory usage {mem_mb}MB > {BROWSER_MAX_MEMORY_MB}MB"
            except Exception as e:
                log.debug("Could not check memory: %s", e)

        # Check scheduled restart
        if health["uptime_seconds"] and health["uptime_seconds"] > BROWSER_RESTART_INTERVAL:
            health["needs_restart"] = True
            health["restart_reason"] = f"Scheduled restart after {BROWSER_RESTART_INTERVAL}s"

        status["last_health_check"] = datetime.now(timezone.utc).isoformat()
        return health

    def start_watchdog(self):
        """Start watchdog thread that monitors browser health."""
        if self._watchdog_thread and self._watchdog_thread.is_alive():
            log.warning("Watchdog already running")
            return

        self._shutdown_event.clear()
        self._watchdog_thread = threading.Thread(target=self._watchdog_loop, daemon=True)
        self._watchdog_thread.start()
        log.info("Watchdog thread started (check interval: %ds)", HEALTH_CHECK_INTERVAL)

    def stop_watchdog(self):
        """Stop the watchdog thread."""
        self._shutdown_event.set()
        if self._watchdog_thread:
            self._watchdog_thread.join(timeout=10)
            self._watchdog_thread = None
        log.info("Watchdog thread stopped")

    def _watchdog_loop(self):
        """Watchdog loop that runs in a separate thread."""
        while not self._shutdown_event.is_set():
            try:
                # Skip watchdog checks when login session is active
                # Login uses a separate headed browser, not the persistent one
                if status.get("login_active"):
                    self._shutdown_event.wait(timeout=30)
                    continue

                health = self.get_health()

                if not health["running"]:
                    log.warning("⚠️ Watchdog: Browser not running, attempting restart...")
                    self.restart()

                elif health["needs_restart"]:
                    log.info("🔄 Watchdog: %s", health["restart_reason"])
                    self.restart()

            except Exception as e:
                log.error("Watchdog error: %s", e)

            # Wait for next check (but can be interrupted by shutdown)
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

        # Ensure HOME is writable
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

    def _export_cookies_internal(self, force: bool = False) -> int:
        """
        Export cookies from the running browser. Called with lock held.

        PROTECTION: Will NOT overwrite existing valid cookies if browser
        doesn't have a valid session (missing required cookies like SID, HSID, etc.)
        This prevents destroying manually uploaded cookies when browser is not logged in.

        Args:
            force: If True, skip protection and always write (used after manual login)
        """
        if self.driver is None:
            return 0

        # Get cookies from YouTube domain
        log.info("Collecting cookies from YouTube...")
        youtube_cookies = self.driver.get_cookies()
        _log_cookie_details(youtube_cookies, "YouTube cookies")

        # Also get cookies from Google domain
        try:
            log.info("Navigating to Google to collect additional cookies...")
            self.driver.get("https://accounts.google.com")
            time.sleep(3)
            google_cookies = self.driver.get_cookies()
            _log_cookie_details(google_cookies, "Google cookies")
            youtube_cookies.extend(google_cookies)
        except Exception as e:
            log.warning("Could not get Google cookies: %s", e)

        # Navigate back to YouTube
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

        # Filter relevant
        relevant = [
            c for c in unique
            if any(d in c.get("domain", "") for d in ["youtube.com", "google.com", "googleapis.com"])
        ]

        # === PROTECTION: Check if browser has valid session cookies ===
        browser_cookie_names = {c.get("name", "") for c in relevant}
        browser_has_session = bool(REQUIRED_COOKIES & browser_cookie_names)
        browser_required_count = len(REQUIRED_COOKIES & browser_cookie_names)

        if not force and not browser_has_session:
            # Browser doesn't have valid session - check if existing file has better cookies
            existing_required_count = 0
            if os.path.exists(COOKIES_FILE):
                try:
                    with open(COOKIES_FILE, "r") as f:
                        existing_content = f.read()
                    for name in REQUIRED_COOKIES:
                        if f"\t{name}\t" in existing_content:
                            existing_required_count += 1
                except Exception as e:
                    log.warning("Could not read existing cookies file: %s", e)

            if existing_required_count > browser_required_count:
                log.warning("=" * 60)
                log.warning("⚠️  PROTECTION: NOT overwriting cookies!")
                log.warning("Browser has %d/%d required cookies (missing: %s)",
                           browser_required_count, len(REQUIRED_COOKIES),
                           REQUIRED_COOKIES - browser_cookie_names)
                log.warning("Existing file has %d/%d required cookies",
                           existing_required_count, len(REQUIRED_COOKIES))
                log.warning("To update cookies, use /browser_login or /update_cookies")
                log.warning("=" * 60)
                status["needs_relogin"] = True
                return 0

        # Write Netscape format
        lines = [
            "# Netscape HTTP Cookie File",
            f"# Generated by cookie_manager.py at {datetime.now(timezone.utc).isoformat()}",
            "",
        ]
        for c in relevant:
            lines.append(format_netscape_cookie(c))

        content = "\n".join(lines) + "\n"

        # Atomic write
        tmp_path = COOKIES_FILE + ".tmp"
        with open(tmp_path, "w") as f:
            f.write(content)
        os.rename(tmp_path, COOKIES_FILE)
        os.chmod(COOKIES_FILE, 0o644)

        log.info("✅ Exported %d cookies to %s (required: %d/%d)",
                len(relevant), COOKIES_FILE, browser_required_count, len(REQUIRED_COOKIES))
        return len(relevant)

    def _take_screenshot(self, name: str):
        """Take a screenshot for debugging."""
        try:
            if self.driver:
                path = f"/data/{name}_{int(time.time())}.png"
                self.driver.save_screenshot(path)
                log.info("Screenshot saved: %s", path)
        except Exception as e:
            log.warning("Could not save screenshot: %s", e)

    def _attempt_auto_relogin(self) -> bool:
        """
        Attempt automatic re-login using saved browser profile.

        When a user has previously logged in, Google remembers the device.
        Navigating to accounts.google.com may automatically restore the session
        without requiring password or 2FA (device is trusted).

        Returns True if re-login was successful.
        """
        if self.driver is None:
            return False

        log.info("=" * 60)
        log.info("ATTEMPTING AUTO-RELOGIN (using saved profile)")
        log.info("=" * 60)

        try:
            # Step 1: Navigate to Google accounts
            log.info("Step 1: Navigating to accounts.google.com...")
            self.driver.get("https://accounts.google.com/")
            time.sleep(5)
            self._take_screenshot("auto_relogin_step1_accounts")

            # Step 2: Navigate to YouTube (may trigger automatic login)
            log.info("Step 2: Navigating to YouTube...")
            self.driver.get("https://www.youtube.com/")
            time.sleep(5)
            self._take_screenshot("auto_relogin_step2_youtube")

            # Step 3: Check if logged in now
            if _check_youtube_logged_in(self.driver):
                log.info("✅ Auto-relogin SUCCEEDED! Session restored from profile.")
                status["needs_relogin"] = False
                return True

            # Step 4: Try clicking on account avatar or sign-in prompt
            log.info("Step 3: Checking for sign-in prompt...")
            try:
                # Look for "Sign in" button and click it
                sign_in_selectors = [
                    "a[href*='accounts.google.com/ServiceLogin']",
                    "ytd-button-renderer a[href*='accounts.google']",
                    "[aria-label='Sign in']",
                ]
                for selector in sign_in_selectors:
                    elements = self.driver.find_elements("css selector", selector)
                    if elements:
                        log.info("Found sign-in element: %s, clicking...", selector)
                        elements[0].click()
                        time.sleep(5)
                        break
            except Exception as e:
                log.warning("Could not click sign-in: %s", e)

            # Step 5: Navigate back to YouTube and check again
            self.driver.get("https://www.youtube.com/")
            time.sleep(5)
            self._take_screenshot("auto_relogin_step3_final")

            if _check_youtube_logged_in(self.driver):
                log.info("✅ Auto-relogin SUCCEEDED after sign-in click!")
                status["needs_relogin"] = False
                return True

            log.warning("❌ Auto-relogin FAILED. Manual login required.")
            return False

        except Exception as e:
            log.error("Error during auto-relogin: %s", e)
            self._take_screenshot("auto_relogin_error")
            return False

    def _get_chrome_pid(self) -> Optional[int]:
        """Get the PID of the Chrome browser process."""
        try:
            if self.driver and hasattr(self.driver, 'service'):
                # Chrome spawns from chromedriver, so we need to find Chrome process
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
        """Get memory usage of a process in MB."""
        try:
            with open(f"/proc/{pid}/status", "r") as f:
                for line in f:
                    if line.startswith("VmRSS:"):
                        # VmRSS: 123456 kB
                        parts = line.split()
                        if len(parts) >= 2:
                            return int(parts[1]) // 1024  # kB to MB
        except Exception:
            pass
        return None


# Global persistent browser manager
browser_manager = PersistentBrowserManager()


# ---------------------------------------------------------------------------
# Helper functions (extracted from old code)
# ---------------------------------------------------------------------------

def _kill_chrome_on_profile(profile_dir: str):
    """Kill all Chrome/chromedriver processes using the given profile directory."""
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
            log.info("Killed %d orphaned Chrome processes on %s", killed, profile_dir)
            time.sleep(1)
    except Exception as e:
        log.warning("Failed to kill orphaned Chrome processes: %s", e)

    try:
        subprocess.run(["killall", "-9", "chromedriver"], capture_output=True, timeout=5)
    except Exception:
        pass


def _cleanup_profile_locks(profile_dir: str):
    """Remove stale Chrome lock files from a profile directory."""
    if not os.path.exists(profile_dir):
        return

    for name in ("SingletonLock", "SingletonCookie", "SingletonSocket"):
        lock_path = os.path.join(profile_dir, name)
        if os.path.exists(lock_path) or os.path.islink(lock_path):
            try:
                os.remove(lock_path)
                log.info("Removed stale lock file: %s", lock_path)
            except OSError as e:
                log.warning("Failed to remove lock file %s: %s", lock_path, e)


def format_netscape_cookie(cookie: dict) -> str:
    """Convert a Selenium cookie dict to a Netscape cookie file line."""
    domain = cookie.get("domain", "")
    subdomain = "TRUE" if domain.startswith(".") else "FALSE"
    path = cookie.get("path", "/")
    secure = "TRUE" if cookie.get("secure", False) else "FALSE"
    expiry = cookie.get("expiry", 0)
    expires = str(int(expiry)) if expiry else "0"
    name = cookie.get("name", "")
    value = cookie.get("value", "")
    return f"{domain}\t{subdomain}\t{path}\t{secure}\t{expires}\t{name}\t{value}"


def _log_cookie_details(cookies: list, context: str = ""):
    """Log detailed information about cookies for debugging."""
    global _previous_cookies

    now = time.time()
    log.info("=" * 60)
    log.info("COOKIE ANALYSIS %s", f"({context})" if context else "")
    log.info("=" * 60)
    log.info("Total cookies: %d", len(cookies))

    by_domain = {}
    for c in cookies:
        domain = c.get("domain", "unknown")
        if domain not in by_domain:
            by_domain[domain] = []
        by_domain[domain].append(c)

    for domain in sorted(by_domain.keys()):
        log.info("-" * 40)
        log.info("Domain: %s (%d cookies)", domain, len(by_domain[domain]))

        for c in sorted(by_domain[domain], key=lambda x: x.get("name", "")):
            name = c.get("name", "")
            value = c.get("value", "")
            expiry = c.get("expiry", 0)
            secure = c.get("secure", False)
            http_only = c.get("httpOnly", False)

            if name in TRACKED_COOKIES:
                if expiry:
                    expires_in = expiry - now
                    if expires_in > 0:
                        days = expires_in / 86400
                        expiry_str = f"expires in {days:.1f} days"
                    else:
                        expiry_str = f"EXPIRED {abs(expires_in)/3600:.1f}h ago!"
                else:
                    expiry_str = "session cookie (no expiry)"

                value_preview = value[:20] + "..." if len(value) > 20 else value

                prev_value = _previous_cookies.get(f"{domain}:{name}")
                changed = ""
                if prev_value is not None:
                    if prev_value != value:
                        changed = " [CHANGED!]"
                    else:
                        changed = " [same]"

                log.info("  📌 %s = %s | %s | secure=%s httpOnly=%s%s",
                         name, value_preview, expiry_str, secure, http_only, changed)

                _previous_cookies[f"{domain}:{name}"] = value

    cookie_names = {c.get("name", "") for c in cookies}
    found_required = REQUIRED_COOKIES & cookie_names
    missing_required = REQUIRED_COOKIES - cookie_names

    log.info("-" * 40)
    log.info("REQUIRED COOKIES STATUS:")
    log.info("  ✅ Found: %s", found_required if found_required else "NONE!")
    log.info("  ❌ Missing: %s", missing_required if missing_required else "none")
    log.info("=" * 60)

    return bool(found_required)


def _log_page_state(driver: uc.Chrome, context: str = ""):
    """Log page state for debugging sign-out issues."""
    try:
        log.info("=" * 60)
        log.info("PAGE STATE ANALYSIS %s", f"({context})" if context else "")
        log.info("=" * 60)
        log.info("Current URL: %s", driver.current_url)
        log.info("Page title: %s", driver.title)

        checks = [
            ("Avatar/Account button", "#avatar-btn, button[aria-label*='Account']"),
            ("Sign In button", "[aria-label='Sign in'], a[href*='accounts.google.com/ServiceLogin']"),
        ]

        for name, selector in checks:
            try:
                elements = driver.find_elements("css selector", selector)
                if elements:
                    log.info("  ✅ %s: FOUND (%d elements)", name, len(elements))
                else:
                    log.info("  ❌ %s: not found", name)
            except Exception as e:
                log.info("  ⚠️ %s: error checking (%s)", name, e)

        log.info("=" * 60)

    except Exception as e:
        log.error("Error logging page state: %s", e)


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
                    log.info("Found avatar element: %s", selector)
                    return True
            except Exception:
                pass

        sign_in_selectors = [
            "a[href*='accounts.google.com/ServiceLogin']",
            "ytd-button-renderer a[href*='accounts.google']",
            "[aria-label='Sign in']",
        ]
        for selector in sign_in_selectors:
            try:
                elements = driver.find_elements("css selector", selector)
                if elements:
                    log.warning("Found Sign In button: %s - NOT logged in!", selector)
                    return False
            except Exception:
                pass

        cookies = driver.get_cookies()
        cookie_names = {c.get("name", "") for c in cookies}
        has_session_cookies = bool(REQUIRED_COOKIES & cookie_names)

        if has_session_cookies:
            log.info("Session cookies present: %s", REQUIRED_COOKIES & cookie_names)
            return True
        else:
            log.warning("No session cookies found!")
            return False

    except Exception as e:
        log.error("Error checking login status: %s", e)
        return True


def check_session_valid(cookie_count: int) -> bool:
    """Check if the exported cookies contain valid session data."""
    if cookie_count < 5:
        return False
    try:
        with open(COOKIES_FILE, "r") as f:
            content = f.read()
        for name in REQUIRED_COOKIES:
            if f"\t{name}\t" in content:
                return True
    except OSError:
        pass
    return False


def get_cookie_analysis() -> dict:
    """Analyze cookies file and return detailed status."""
    result = {
        "required_found": [],
        "required_missing": list(REQUIRED_COOKIES),
        "session_valid": False,
        "reason": "No cookies file",
    }

    if not os.path.exists(COOKIES_FILE):
        return result

    try:
        with open(COOKIES_FILE, "r") as f:
            content = f.read()

        found = set()
        for name in REQUIRED_COOKIES:
            if f"\t{name}\t" in content:
                found.add(name)

        result["required_found"] = list(found)
        result["required_missing"] = list(REQUIRED_COOKIES - found)

        if found:
            result["session_valid"] = True
            result["reason"] = None
        else:
            result["reason"] = f"Missing all required cookies: {', '.join(REQUIRED_COOKIES)}"

    except OSError as e:
        result["reason"] = f"Could not read cookies file: {e}"

    return result


# ---------------------------------------------------------------------------
# Login flow (visual via noVNC) — kept for initial login
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
    """Create Chrome driver for login (headed mode on Xvfb)."""
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
    """Start visual login session: Xvfb + Chromium + x11vnc + noVNC."""
    if status["login_active"]:
        return {"error": "Login session already active"}

    # Stop persistent browser before login
    if browser_manager.is_running():
        log.info("Stopping persistent browser for login session...")
        browser_manager.stop(export_cookies=True)

    if _browser_lock is None:
        return {"error": "Browser lock not initialized"}

    if _browser_lock.locked():
        return {"error": "Browser is busy, try again in a moment"}

    log.info("Starting login session...")

    os.makedirs(BROWSER_PROFILE_DIR, exist_ok=True)

    # 1. Start Xvfb
    xvfb_proc = subprocess.Popen(
        ["Xvfb", DISPLAY, "-screen", "0", "1920x1080x24"],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    login_state["xvfb_proc"] = xvfb_proc
    await asyncio.sleep(1)

    if xvfb_proc.poll() is not None:
        return {"error": "Failed to start Xvfb"}

    # 2. Launch Chromium
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
        log.error("Failed to start Chromium: %s", e)
        _kill_proc(xvfb_proc)
        login_state["xvfb_proc"] = None
        return {"error": f"Failed to start Chromium: {e}"}

    # 3. Start x11vnc
    vnc_cmd = [
        "x11vnc",
        "-display", DISPLAY,
        "-forever",
        "-rfbport", str(VNC_PORT),
        "-shared",
    ]
    if VNC_PASSWORD:
        vnc_cmd += ["-passwd", VNC_PASSWORD]
    else:
        vnc_cmd += ["-nopw"]

    vnc_proc = subprocess.Popen(vnc_cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    login_state["vnc_proc"] = vnc_proc
    await asyncio.sleep(0.5)

    # 4. Start noVNC websockify
    novnc_proc = subprocess.Popen(
        ["websockify", "--web=/opt/novnc", str(NOVNC_PORT), f"localhost:{VNC_PORT}"],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    login_state["novnc_proc"] = novnc_proc
    await asyncio.sleep(0.5)

    status["login_active"] = True
    status["login_started_at"] = datetime.now(timezone.utc).isoformat()

    host = NOVNC_HOST or "localhost"
    # Use external port for URL if set, otherwise use internal port
    url_port = NOVNC_EXTERNAL_PORT if NOVNC_EXTERNAL_PORT else NOVNC_PORT
    novnc_url = f"http://{host}:{url_port}/vnc.html?autoconnect=true"

    log.info("Login session started. noVNC URL: %s", novnc_url)

    asyncio.get_running_loop().call_later(LOGIN_TIMEOUT, _auto_stop_login)

    return {"status": "ok", "novnc_url": novnc_url}


def _auto_stop_login():
    """Auto-stop login session after timeout."""
    if status["login_active"]:
        log.warning("Login session timed out after %d seconds", LOGIN_TIMEOUT)
        asyncio.ensure_future(stop_login())


async def stop_login() -> dict:
    """Stop login session, export cookies, start persistent browser."""
    if not status["login_active"]:
        return {"error": "No active login session"}

    log.info("=" * 80)
    log.info("STOPPING LOGIN SESSION - SAVING COOKIES")
    log.info("=" * 80)

    result = {"status": "ok", "cookies_exported": False, "cookie_count": 0}

    driver = login_state.get("driver")
    if driver:
        try:
            browser_cookies = driver.get_cookies()
            log.info("Browser has %d cookies before export", len(browser_cookies))
            _log_cookie_details(browser_cookies, "BROWSER STATE at login stop")

            loop = asyncio.get_running_loop()
            count = await loop.run_in_executor(None, _export_cookies_from_login_driver, driver)

            result["cookies_exported"] = True
            result["cookie_count"] = count

            status["cookie_count"] = count
            status["last_refresh"] = datetime.now(timezone.utc).isoformat()
            status["last_refresh_success"] = True
            status["needs_relogin"] = not check_session_valid(count)
            status["last_error"] = None

            log.info("Exported %d cookies after login", count)

        except Exception as e:
            log.error("Failed to export cookies after login: %s", e)
            result["error"] = str(e)
            status["last_error"] = str(e)

        try:
            driver.quit()
        except Exception:
            pass

    _kill_chrome_on_profile(BROWSER_PROFILE_DIR)

    # Cleanup display stack
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

    log.info("Login session stopped")

    # Start persistent browser after login
    log.info("Starting persistent browser after login...")
    await asyncio.sleep(2)
    loop = asyncio.get_running_loop()
    await loop.run_in_executor(None, browser_manager.start)

    return result


def _export_cookies_from_login_driver(driver: uc.Chrome) -> int:
    """Export cookies from login browser."""
    log.info("Navigating to YouTube...")
    driver.get("https://www.youtube.com")
    time.sleep(5)

    youtube_cookies = driver.get_cookies()

    try:
        log.info("Navigating to Google...")
        driver.get("https://accounts.google.com")
        time.sleep(3)
        google_cookies = driver.get_cookies()
        youtube_cookies.extend(google_cookies)
    except Exception as e:
        log.warning("Could not get Google cookies: %s", e)

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

    lines = [
        "# Netscape HTTP Cookie File",
        f"# Generated by cookie_manager.py at {datetime.now(timezone.utc).isoformat()}",
        "",
    ]
    for c in relevant:
        lines.append(format_netscape_cookie(c))

    content = "\n".join(lines) + "\n"

    tmp_path = COOKIES_FILE + ".tmp"
    with open(tmp_path, "w") as f:
        f.write(content)
    os.rename(tmp_path, COOKIES_FILE)
    os.chmod(COOKIES_FILE, 0o644)

    log.info("✅ Exported %d cookies", len(relevant))
    return len(relevant)


# ---------------------------------------------------------------------------
# Refresh loop (uses persistent browser)
# ---------------------------------------------------------------------------

async def refresh_loop():
    """Background task: refresh session every REFRESH_INTERVAL seconds."""
    log.info("Cookie refresh loop started (interval: %ds)", REFRESH_INTERVAL)

    # Initial delay
    await asyncio.sleep(30)

    # Start persistent browser if not running and profile exists
    if os.path.exists(BROWSER_PROFILE_DIR) and not browser_manager.is_running():
        log.info("Starting persistent browser on startup...")
        loop = asyncio.get_running_loop()
        await loop.run_in_executor(None, browser_manager.start)

    while True:
        status["profile_exists"] = os.path.exists(BROWSER_PROFILE_DIR)

        if status["login_active"]:
            log.debug("Skipping refresh — login session active")
            await asyncio.sleep(60)
            continue

        if not status["profile_exists"]:
            status["needs_relogin"] = True
            log.warning("No browser profile found — need initial login")
            await asyncio.sleep(REFRESH_INTERVAL)
            continue

        # Ensure browser is running
        if not browser_manager.is_running():
            log.info("Browser not running, starting...")
            loop = asyncio.get_running_loop()
            success = await loop.run_in_executor(None, browser_manager.start)
            if not success:
                log.error("Failed to start browser, will retry later")
                await asyncio.sleep(60)
                continue

        log.info("Running scheduled cookie refresh...")

        try:
            loop = asyncio.get_running_loop()
            count = await loop.run_in_executor(None, browser_manager.refresh_and_export)

            status["last_refresh"] = datetime.now(timezone.utc).isoformat()
            status["last_refresh_success"] = True
            status["cookie_count"] = count
            status["last_error"] = None

            if not check_session_valid(count):
                status["needs_relogin"] = True
                log.warning("Session appears expired — needs re-login")
            else:
                status["needs_relogin"] = False
                log.info("Cookie refresh successful: %d cookies", count)

        except Exception as e:
            status["last_refresh"] = datetime.now(timezone.utc).isoformat()
            status["last_refresh_success"] = False
            status["last_error"] = str(e)
            status["needs_relogin"] = True
            log.error("Cookie refresh failed: %s", e)

        await asyncio.sleep(REFRESH_INTERVAL)


# ---------------------------------------------------------------------------
# HTTP API
# ---------------------------------------------------------------------------

async def handle_login_start(request):
    result = await start_login()
    return web.json_response(result)


async def handle_login_stop(request):
    result = await stop_login()
    return web.json_response(result)


async def handle_status(request):
    status["profile_exists"] = os.path.exists(BROWSER_PROFILE_DIR)
    status["cookies_exist"] = os.path.exists(COOKIES_FILE)
    status["browser_running"] = browser_manager.is_running()

    analysis = get_cookie_analysis()
    status["cookie_analysis"] = analysis
    status["required_found"] = analysis["required_found"]
    status["required_missing"] = analysis["required_missing"]
    status["invalid_reason"] = analysis["reason"]

    return web.json_response(status)


async def handle_export_cookies(request):
    """Force cookie export from persistent browser."""
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
    """Get browser health information."""
    health = browser_manager.get_health()
    return web.json_response(health)


async def handle_restart_browser(request):
    """Force restart the persistent browser."""
    try:
        loop = asyncio.get_running_loop()
        success = await loop.run_in_executor(None, browser_manager.restart)
        if success:
            return web.json_response({"success": True, "message": "Browser restarted"})
        else:
            return web.json_response({"success": False, "error": "Failed to restart browser"}, status=500)
    except Exception as e:
        return web.json_response({"success": False, "error": str(e)}, status=500)


async def handle_cookie_debug(request):
    """Return detailed cookie analysis for debugging."""
    result = {
        "cookies_file_exists": os.path.exists(COOKIES_FILE),
        "profile_exists": os.path.exists(BROWSER_PROFILE_DIR),
        "browser_running": browser_manager.is_running(),
        "cookies": [],
        "analysis": {},
    }

    if os.path.exists(COOKIES_FILE):
        try:
            with open(COOKIES_FILE, "r") as f:
                content = f.read()

            result["file_size"] = len(content)
            result["file_lines"] = len(content.splitlines())

            now = time.time()
            cookies = []
            for line in content.splitlines():
                if line.startswith("#") or not line.strip():
                    continue
                parts = line.split("\t")
                if len(parts) >= 7:
                    domain, _, path, secure, expiry, name, value = parts[:7]
                    expiry_int = int(expiry) if expiry.isdigit() else 0

                    cookie_info = {
                        "domain": domain,
                        "name": name,
                        "value_preview": value[:30] + "..." if len(value) > 30 else value,
                        "secure": secure == "TRUE",
                        "expiry": expiry_int,
                    }

                    if expiry_int > 0:
                        expires_in = expiry_int - now
                        cookie_info["expires_in_hours"] = round(expires_in / 3600, 1)
                        cookie_info["expired"] = expires_in < 0
                    else:
                        cookie_info["expires_in_hours"] = None
                        cookie_info["expired"] = False

                    cookie_info["is_required"] = name in REQUIRED_COOKIES
                    cookie_info["is_tracked"] = name in TRACKED_COOKIES

                    cookies.append(cookie_info)

            result["cookies"] = cookies
            result["total_cookies"] = len(cookies)

            required_found = [c["name"] for c in cookies if c["is_required"]]
            required_missing = list(REQUIRED_COOKIES - set(required_found))
            expired = [c["name"] for c in cookies if c.get("expired")]

            result["analysis"] = {
                "required_cookies_found": required_found,
                "required_cookies_missing": required_missing,
                "expired_cookies": expired,
                "session_valid": len(required_found) > 0 and len(expired) == 0,
            }

        except Exception as e:
            result["error"] = str(e)

    return web.json_response(result)


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

async def on_startup(app):
    """Start refresh loop and watchdog on server startup."""
    _init_browser_lock()
    app["refresh_task"] = asyncio.ensure_future(refresh_loop())

    # Start watchdog in background
    browser_manager.start_watchdog()


async def on_cleanup(app):
    """Cleanup on server shutdown."""
    log.info("Server shutting down...")

    task = app.get("refresh_task")
    if task:
        task.cancel()
        try:
            await task
        except asyncio.CancelledError:
            pass

    # Stop watchdog
    browser_manager.stop_watchdog()

    # Stop login if active
    if status["login_active"]:
        await stop_login()

    # Stop persistent browser (exports cookies)
    if browser_manager.is_running():
        log.info("Exporting cookies before shutdown...")
        loop = asyncio.get_running_loop()
        await loop.run_in_executor(None, browser_manager.stop, True)

    log.info("Cleanup complete")


def _handle_signal(signum, frame):
    """Handle shutdown signals gracefully."""
    log.info("Received signal %d, initiating graceful shutdown...", signum)
    if browser_manager.is_running():
        browser_manager.stop(export_cookies=True)
    raise SystemExit(0)


def main():
    log.info("=" * 60)
    log.info("Cookie Manager starting (PERSISTENT BROWSER ARCHITECTURE)")
    log.info("=" * 60)
    log.info("  API: %s:%d", API_HOST, API_PORT)
    log.info("  Cookies file: %s", COOKIES_FILE)
    log.info("  Browser profile: %s", BROWSER_PROFILE_DIR)
    log.info("  Refresh interval: %ds", REFRESH_INTERVAL)
    log.info("  Health check interval: %ds", HEALTH_CHECK_INTERVAL)
    log.info("  Browser restart interval: %ds", BROWSER_RESTART_INTERVAL)
    log.info("  Chromium path: %s", CHROMIUM_PATH)
    log.info("  ChromeDriver path: %s", CHROMEDRIVER_PATH)
    log.info("=" * 60)

    # Register signal handlers
    signal.signal(signal.SIGTERM, _handle_signal)
    signal.signal(signal.SIGINT, _handle_signal)

    # Clean up before start
    _kill_chrome_on_profile(BROWSER_PROFILE_DIR)
    _cleanup_profile_locks(BROWSER_PROFILE_DIR)

    app = web.Application()
    app.router.add_post("/api/login_start", handle_login_start)
    app.router.add_post("/api/login_stop", handle_login_stop)
    app.router.add_get("/api/status", handle_status)
    app.router.add_post("/api/export_cookies", handle_export_cookies)
    app.router.add_get("/api/cookie_debug", handle_cookie_debug)
    app.router.add_get("/api/browser_health", handle_browser_health)
    app.router.add_post("/api/restart_browser", handle_restart_browser)

    app.on_startup.append(on_startup)
    app.on_cleanup.append(on_cleanup)

    web.run_app(app, host=API_HOST, port=API_PORT, print=None)


if __name__ == "__main__":
    main()
