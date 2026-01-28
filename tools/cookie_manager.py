#!/usr/bin/env python3
"""
YouTube Cookie Manager — aiohttp server + undetected-chromedriver automation.

Manages YouTube cookies automatically:
1. First login via noVNC (admin logs in visually)
2. Cookie refresh every 30 minutes (headless)
3. Exports cookies in Netscape format for yt-dlp

API endpoints:
  POST /api/login_start    — Start login session (Xvfb + Chromium + noVNC)
  POST /api/login_stop     — Stop login, export cookies
  GET  /api/status         — Cookie manager status
  POST /api/export_cookies — Force cookie re-export
"""

import asyncio
import logging
import os
import shutil
import subprocess
import tempfile
import time
from datetime import datetime, timezone

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
NOVNC_PORT = int(os.environ.get("NOVNC_PORT", "6080"))
VNC_PORT = 5900
VNC_PASSWORD = os.environ.get("VNC_PASSWORD", "")
DISPLAY = ":99"
CHROMIUM_PATH = os.environ.get("CHROMIUM_PATH", "/usr/bin/chromium-browser")
CHROMEDRIVER_PATH = os.environ.get("CHROMEDRIVER_PATH", "/usr/bin/chromedriver")

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
    level=logging.DEBUG,  # Enable DEBUG for detailed cookie tracking
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
}

login_state = {
    "driver": None,
    "xvfb_proc": None,
    "vnc_proc": None,
    "novnc_proc": None,
}

# Lock to prevent concurrent browser sessions (login vs refresh).
# Initialized in on_startup() since asyncio.Lock() needs a running event loop.
_browser_lock = None


def _init_browser_lock():
    global _browser_lock
    _browser_lock = asyncio.Lock()


# ---------------------------------------------------------------------------
# Selenium helpers
# ---------------------------------------------------------------------------

def _kill_chrome_on_profile(profile_dir: str):
    """Kill all Chrome/chromedriver processes using the given profile directory.

    When cookie_manager is restarted by supervisor, orphaned Chrome child
    processes from the previous instance keep running and hold the profile
    lock. We must kill them before creating a new session.
    """
    killed = 0
    try:
        # Read /proc to find Chrome processes with matching user-data-dir.
        # This works on any Linux, unlike ps which varies between busybox/procps.
        needle = f"--user-data-dir={profile_dir}"
        my_pid = os.getpid()
        for entry in os.listdir("/proc"):
            if not entry.isdigit():
                continue
            pid = int(entry)
            if pid == my_pid:
                continue
            try:
                cmdline_path = f"/proc/{pid}/cmdline"
                with open(cmdline_path, "rb") as f:
                    cmdline = f.read().decode("utf-8", errors="replace")
                if needle in cmdline:
                    os.kill(pid, 9)  # SIGKILL
                    killed += 1
            except (OSError, PermissionError):
                pass

        if killed:
            log.info("Killed %d orphaned Chrome processes on %s", killed, profile_dir)
            time.sleep(1)
    except Exception as e:
        log.warning("Failed to kill orphaned Chrome processes: %s", e)

    # Also kill any stale chromedriver processes
    try:
        subprocess.run(["killall", "-9", "chromedriver"],
                       capture_output=True, timeout=5)
    except Exception:
        pass


def _cleanup_profile_locks(profile_dir: str):
    """Remove stale Chrome lock files (symlinks) from a profile directory."""
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


def _make_chrome_options(*, headless: bool, profile_dir: str) -> uc.ChromeOptions:
    """Create ChromeOptions with common flags."""
    opts = uc.ChromeOptions()
    opts.binary_location = CHROMIUM_PATH
    opts.add_argument(f"--user-data-dir={profile_dir}")
    opts.add_argument("--no-sandbox")
    opts.add_argument("--disable-gpu")
    opts.add_argument("--disable-dev-shm-usage")

    # Common anti-detection flags
    opts.add_argument("--disable-blink-features=AutomationControlled")
    opts.add_argument("--disable-infobars")
    opts.add_argument("--disable-extensions")

    if headless:
        # Use new headless mode which is less detectable
        opts.add_argument("--headless=new")
        # Set window size even in headless
        opts.add_argument("--window-size=1920,1080")
        # Fake user agent to not reveal HeadlessChrome
        opts.add_argument(
            "--user-agent=Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 "
            "(KHTML, like Gecko) Chrome/136.0.0.0 Safari/537.36"
        )
    else:
        opts.add_argument("--start-maximized")
        opts.add_argument("--window-size=1920,1080")

    return opts


def _create_driver(*, headless: bool, profile_dir: str) -> uc.Chrome:
    """Create an undetected Chrome driver with the given profile directory."""
    _kill_chrome_on_profile(profile_dir)
    _cleanup_profile_locks(profile_dir)
    opts = _make_chrome_options(headless=headless, profile_dir=profile_dir)

    # Ensure HOME is writable (undetected-chromedriver writes to ~/.local/)
    home = os.environ.get("HOME", "/tmp")
    local_dir = os.path.join(home, ".local", "share", "undetected_chromedriver")
    os.makedirs(local_dir, exist_ok=True)

    return uc.Chrome(
        options=opts,
        browser_executable_path=CHROMIUM_PATH,
        driver_executable_path=CHROMEDRIVER_PATH,
        headless=headless,
        use_subprocess=True,
    )


# ---------------------------------------------------------------------------
# Cookie export
# ---------------------------------------------------------------------------

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

    # Group by domain
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

            # Only show detailed info for important cookies
            if name in TRACKED_COOKIES:
                # Calculate expiry info
                if expiry:
                    expires_in = expiry - now
                    if expires_in > 0:
                        days = expires_in / 86400
                        expiry_str = f"expires in {days:.1f} days"
                    else:
                        expiry_str = f"EXPIRED {abs(expires_in)/3600:.1f}h ago!"
                else:
                    expiry_str = "session cookie (no expiry)"

                # Truncate value for security but show enough to compare
                value_preview = value[:20] + "..." if len(value) > 20 else value

                # Check if changed from previous
                prev_value = _previous_cookies.get(f"{domain}:{name}")
                changed = ""
                if prev_value is not None:
                    if prev_value != value:
                        changed = " [CHANGED!]"
                    else:
                        changed = " [same]"

                log.info("  📌 %s = %s | %s | secure=%s httpOnly=%s%s",
                         name, value_preview, expiry_str, secure, http_only, changed)

                # Store for next comparison
                _previous_cookies[f"{domain}:{name}"] = value

    # Summary of required cookies
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

        # Current URL
        log.info("Current URL: %s", driver.current_url)

        # Page title
        log.info("Page title: %s", driver.title)

        # Check for common elements
        checks = [
            ("Avatar/Account button", "#avatar-btn, button[aria-label*='Account']"),
            ("Sign In button", "[aria-label='Sign in'], a[href*='accounts.google.com/ServiceLogin']"),
            ("Sign Out option", "[aria-label='Sign out']"),
            ("YouTube logo", "#logo"),
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

        # Look for error messages or unusual content
        try:
            page_source = driver.page_source
            error_indicators = [
                "Sign in to confirm you're not a bot",
                "unusual traffic",
                "captcha",
                "verify it's you",
                "session expired",
                "signed out",
            ]
            for indicator in error_indicators:
                if indicator.lower() in page_source.lower():
                    log.warning("  ⚠️ FOUND ERROR INDICATOR: '%s'", indicator)
        except Exception:
            pass

        log.info("=" * 60)

    except Exception as e:
        log.error("Error logging page state: %s", e)


def _export_cookies_from_driver(driver: uc.Chrome) -> int:
    """Extract cookies from an open driver and write to file."""
    log.info("Starting cookie export from driver...")

    # Log initial state before navigation
    initial_cookies = driver.get_cookies()
    log.info("Cookies BEFORE YouTube navigation: %d", len(initial_cookies))
    _log_cookie_details(initial_cookies, "BEFORE navigation")

    # Navigate to YouTube to ensure cookies are fresh
    log.info("Navigating to YouTube...")
    driver.get("https://www.youtube.com")

    # Wait for page to fully load and cookie rotation JavaScript to execute
    # YouTube calls RotateCookiesPage periodically to refresh session cookies
    log.info("Waiting 5s for page load and JS execution...")
    time.sleep(5)

    # Log page state
    _log_page_state(driver, "after YouTube load")

    # Simulate some interaction to trigger cookie refresh
    try:
        log.info("Simulating user interaction (scroll)...")
        driver.execute_script("window.scrollTo(0, 500);")
        time.sleep(2)
        driver.execute_script("window.scrollTo(0, 0);")
    except Exception as e:
        log.warning("Could not simulate scroll: %s", e)

    # Collect cookies from YouTube
    youtube_cookies = driver.get_cookies()
    log.info("Cookies AFTER YouTube interaction: %d", len(youtube_cookies))
    _log_cookie_details(youtube_cookies, "AFTER YouTube")

    # Also grab Google cookies by visiting accounts.google.com
    try:
        log.info("Navigating to accounts.google.com...")
        driver.get("https://accounts.google.com")
        time.sleep(3)  # Wait for cookie sync

        _log_page_state(driver, "after Google accounts load")

        google_cookies = driver.get_cookies()
        log.info("Cookies from Google: %d", len(google_cookies))
        _log_cookie_details(google_cookies, "Google domain")

        youtube_cookies.extend(google_cookies)
    except Exception as e:
        log.warning("Could not get Google cookies: %s", e)

    # Deduplicate by (domain, name)
    seen = set()
    unique = []
    for c in youtube_cookies:
        key = (c.get("domain", ""), c.get("name", ""))
        if key not in seen:
            seen.add(key)
            unique.append(c)

    # Filter relevant cookies
    relevant = [
        c
        for c in unique
        if any(
            d in c.get("domain", "")
            for d in ["youtube.com", "google.com", "googleapis.com"]
        )
    ]

    log.info("=" * 60)
    log.info("FINAL EXPORT: %d relevant cookies (from %d total)", len(relevant), len(unique))
    _log_cookie_details(relevant, "FINAL EXPORT")

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

    cookie_count = len(relevant)
    cookie_names = {c["name"] for c in relevant}
    has_session = bool(REQUIRED_COOKIES & cookie_names)

    log.info(
        "✅ Exported %d cookies (%s session cookies)",
        cookie_count,
        "has" if has_session else "NO",
    )

    return cookie_count


async def export_cookies_from_profile() -> int:
    """Launch headless browser with saved profile, extract and write cookies."""
    if not os.path.exists(BROWSER_PROFILE_DIR):
        raise FileNotFoundError(f"Browser profile not found: {BROWSER_PROFILE_DIR}")

    if _browser_lock is None:
        raise RuntimeError("Browser lock not initialized")

    async with _browser_lock:
        # Run Selenium in a thread to avoid blocking the event loop
        loop = asyncio.get_running_loop()
        return await loop.run_in_executor(None, _export_cookies_headless)


def _export_cookies_headless() -> int:
    """Headless export — runs in a thread.

    IMPORTANT: Uses the original profile directly (not a copy) to avoid
    Google detecting a "new device" and invalidating the session.

    The previous approach of copying to temp directory caused Google to
    see the browser as a new device, triggering session invalidation.
    """
    log.info("=" * 80)
    log.info("HEADLESS COOKIE REFRESH STARTED")
    log.info("=" * 80)

    # Kill any orphaned Chrome processes and clean up locks first
    _kill_chrome_on_profile(BROWSER_PROFILE_DIR)
    _cleanup_profile_locks(BROWSER_PROFILE_DIR)

    log.info("Using original profile: %s", BROWSER_PROFILE_DIR)

    # Check profile directory contents and cookie database
    if os.path.exists(BROWSER_PROFILE_DIR):
        try:
            items = os.listdir(BROWSER_PROFILE_DIR)
            log.info("Profile directory contains %d items: %s", len(items), items[:10])

            # Check Default profile directory
            default_dir = os.path.join(BROWSER_PROFILE_DIR, "Default")
            if os.path.exists(default_dir):
                log.info("  ✅ Default directory exists")
                default_items = os.listdir(default_dir)
                log.info("  Default contains %d items", len(default_items))

                # Check for cookie database files (Chrome stores cookies here)
                cookie_paths = [
                    os.path.join(default_dir, "Cookies"),
                    os.path.join(default_dir, "Network", "Cookies"),
                ]
                for cookie_path in cookie_paths:
                    if os.path.exists(cookie_path):
                        size = os.path.getsize(cookie_path)
                        log.info("  ✅ Cookie DB found: %s (%d bytes)", cookie_path, size)

                        # Try to read cookie count from SQLite
                        try:
                            import sqlite3
                            conn = sqlite3.connect(cookie_path)
                            cursor = conn.cursor()
                            cursor.execute("SELECT COUNT(*) FROM cookies")
                            count = cursor.fetchone()[0]
                            cursor.execute("SELECT host_key, name, expires_utc FROM cookies LIMIT 10")
                            sample = cursor.fetchall()
                            conn.close()
                            log.info("  Cookie DB contains %d cookies", count)
                            for host, name, expires in sample:
                                log.info("    - %s: %s (expires: %s)", host, name, expires)
                        except Exception as e:
                            log.warning("  Could not read cookie DB: %s", e)
                    else:
                        log.warning("  ❌ Cookie DB not found: %s", cookie_path)
            else:
                log.warning("  ❌ Default directory NOT FOUND")

            # Check Local State
            local_state = os.path.join(BROWSER_PROFILE_DIR, "Local State")
            if os.path.exists(local_state):
                log.info("  ✅ Local State exists (%d bytes)", os.path.getsize(local_state))
            else:
                log.warning("  ❌ Local State NOT FOUND")

        except Exception as e:
            log.warning("Could not analyze profile directory: %s", e)

    driver = _create_driver(headless=True, profile_dir=BROWSER_PROFILE_DIR)
    try:
        # Get cookies BEFORE any navigation
        initial_cookies = driver.get_cookies()
        log.info("Cookies in browser BEFORE navigation: %d", len(initial_cookies))
        if initial_cookies:
            _log_cookie_details(initial_cookies, "INITIAL (before navigation)")
        else:
            log.warning("⚠️ NO COOKIES in browser before navigation!")

        # Navigate to YouTube and wait for cookie rotation to happen
        log.info("Navigating to YouTube for session refresh...")
        driver.get("https://www.youtube.com")

        # Wait for page to fully load and JavaScript to execute
        log.info("Waiting 5s for page load...")
        time.sleep(5)

        # Log page state
        _log_page_state(driver, "after YouTube navigation")

        # Check cookies after YouTube load
        after_yt_cookies = driver.get_cookies()
        log.info("Cookies AFTER YouTube load: %d", len(after_yt_cookies))
        _log_cookie_details(after_yt_cookies, "AFTER YouTube load")

        # Simulate human-like behavior to keep session "warm"
        # Scroll down a bit
        try:
            log.info("Simulating user interaction...")
            driver.execute_script("window.scrollTo(0, 300);")
            time.sleep(2)
            driver.execute_script("window.scrollTo(0, 0);")
            time.sleep(2)
        except Exception as e:
            log.warning("Could not simulate scrolling: %s", e)

        # Check if we're still logged in by looking for sign-in button
        is_logged_in = _check_youtube_logged_in(driver)

        if not is_logged_in:
            log.error("=" * 80)
            log.error("SESSION IS NOT LOGGED IN! SIGN-OUT DETECTED!")
            log.error("=" * 80)

            # Take screenshot for debugging (save to /data)
            try:
                screenshot_path = "/data/signout_debug.png"
                driver.save_screenshot(screenshot_path)
                log.info("Screenshot saved to %s", screenshot_path)
            except Exception as e:
                log.warning("Could not save screenshot: %s", e)

            # Log current page source (truncated)
            try:
                source = driver.page_source[:2000]
                log.info("Page source (first 2000 chars):\n%s", source)
            except Exception:
                pass

            return 0

        log.info("✅ Session is still logged in, proceeding with cookie export")

        # Wait a bit more to ensure cookie rotation completes
        time.sleep(3)

        return _export_cookies_from_driver(driver)
    except Exception as e:
        log.error("Error during headless refresh: %s", e, exc_info=True)
        raise
    finally:
        try:
            driver.quit()
        except Exception:
            pass
        # Clean up any locks that might have been created
        _cleanup_profile_locks(BROWSER_PROFILE_DIR)
        log.info("=" * 80)
        log.info("HEADLESS COOKIE REFRESH COMPLETED")
        log.info("=" * 80)


def _check_youtube_logged_in(driver: uc.Chrome) -> bool:
    """Check if YouTube session is logged in by examining page elements."""
    try:
        # Method 1: Check for avatar button (logged in users have this)
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
                    log.info("Found avatar element with selector: %s", selector)
                    return True
            except Exception:
                pass

        # Method 2: Check for Sign In button (means NOT logged in)
        sign_in_selectors = [
            "a[href*='accounts.google.com/ServiceLogin']",
            "ytd-button-renderer a[href*='accounts.google']",
            "tp-yt-paper-button[aria-label='Sign in']",
            "[aria-label='Sign in']",
        ]
        for selector in sign_in_selectors:
            try:
                elements = driver.find_elements("css selector", selector)
                if elements:
                    log.warning("Found Sign In button with selector: %s - NOT logged in!", selector)
                    return False
            except Exception:
                pass

        # Method 3: Check page source for login indicators
        page_source = driver.page_source.lower()
        if "sign in" in page_source and "sign out" not in page_source:
            log.warning("Page contains 'Sign in' without 'Sign out' - likely NOT logged in")
            return False

        # Method 4: Check cookies for session indicators
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
        # If we can't determine, assume logged in and let cookie export decide
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


async def start_login() -> dict:
    """Start visual login session: Xvfb + Chromium + x11vnc + noVNC."""
    if status["login_active"]:
        return {"error": "Login session already active"}

    if _browser_lock is None:
        return {"error": "Browser lock not initialized"}

    if _browser_lock.locked():
        return {"error": "Browser is busy (refresh in progress), try again in a moment"}

    log.info("Starting login session...")

    # Ensure profile dir exists
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

    # 2. Launch Chromium via Selenium in headed mode on Xvfb display
    os.environ["DISPLAY"] = DISPLAY

    try:
        loop = asyncio.get_running_loop()
        driver = await loop.run_in_executor(
            None, lambda: _create_driver(headless=False, profile_dir=BROWSER_PROFILE_DIR)
        )
        login_state["driver"] = driver

        # Navigate to Google login
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

    vnc_proc = subprocess.Popen(
        vnc_cmd,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    login_state["vnc_proc"] = vnc_proc
    await asyncio.sleep(0.5)

    # 4. Start noVNC websockify
    novnc_proc = subprocess.Popen(
        [
            "websockify",
            "--web=/opt/novnc",
            str(NOVNC_PORT),
            f"localhost:{VNC_PORT}",
        ],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    login_state["novnc_proc"] = novnc_proc
    await asyncio.sleep(0.5)

    # Update status
    status["login_active"] = True
    status["login_started_at"] = datetime.now(timezone.utc).isoformat()

    # Build noVNC URL
    host = NOVNC_HOST or "localhost"
    novnc_url = f"http://{host}:{NOVNC_PORT}/vnc.html?autoconnect=true"

    log.info("Login session started. noVNC URL: %s", novnc_url)

    # Schedule auto-timeout
    asyncio.get_running_loop().call_later(LOGIN_TIMEOUT, _auto_stop_login)

    return {"status": "ok", "novnc_url": novnc_url}


def _auto_stop_login():
    """Auto-stop login session after timeout."""
    if status["login_active"]:
        log.warning("Login session timed out after %d seconds", LOGIN_TIMEOUT)
        asyncio.ensure_future(stop_login())


async def stop_login() -> dict:
    """Stop login session, export cookies, cleanup."""
    if not status["login_active"]:
        return {"error": "No active login session"}

    log.info("=" * 80)
    log.info("STOPPING LOGIN SESSION - SAVING COOKIES")
    log.info("=" * 80)

    result = {"status": "ok", "cookies_exported": False, "cookie_count": 0}

    # Export cookies before closing
    driver = login_state.get("driver")
    if driver:
        try:
            # First, log current browser cookies
            browser_cookies = driver.get_cookies()
            log.info("Browser has %d cookies before export", len(browser_cookies))
            _log_cookie_details(browser_cookies, "BROWSER STATE at login stop")

            loop = asyncio.get_running_loop()
            count = await loop.run_in_executor(None, _export_cookies_from_driver, driver)

            result["cookies_exported"] = True
            result["cookie_count"] = count

            status["cookie_count"] = count
            status["last_refresh"] = datetime.now(timezone.utc).isoformat()
            status["last_refresh_success"] = True
            status["needs_relogin"] = not check_session_valid(count)
            status["last_error"] = None

            log.info("Exported %d cookies after login", count)

            # Check if cookies were saved to profile
            log.info("Checking if cookies persisted to profile...")
            default_dir = os.path.join(BROWSER_PROFILE_DIR, "Default")
            for cookie_path in [
                os.path.join(default_dir, "Cookies"),
                os.path.join(default_dir, "Network", "Cookies"),
            ]:
                if os.path.exists(cookie_path):
                    try:
                        import sqlite3
                        conn = sqlite3.connect(cookie_path)
                        cursor = conn.cursor()
                        cursor.execute("SELECT COUNT(*) FROM cookies")
                        db_count = cursor.fetchone()[0]
                        conn.close()
                        log.info("  ✅ Cookie DB %s has %d cookies", cookie_path, db_count)
                    except Exception as e:
                        log.warning("  Could not read cookie DB: %s", e)

        except Exception as e:
            log.error("Failed to export cookies after login: %s", e)
            result["error"] = str(e)
            status["last_error"] = str(e)

        # Quit browser
        try:
            driver.quit()
        except Exception:
            pass

    # Kill any remaining Chrome processes on the profile
    _kill_chrome_on_profile(BROWSER_PROFILE_DIR)

    # Cleanup display stack
    _kill_proc(login_state.get("novnc_proc"))
    _kill_proc(login_state.get("vnc_proc"))
    _kill_proc(login_state.get("xvfb_proc"))

    # Reset state
    login_state.update({
        "driver": None,
        "xvfb_proc": None,
        "vnc_proc": None,
        "novnc_proc": None,
    })

    status["login_active"] = False
    status["login_started_at"] = None

    log.info("Login session stopped")

    return result


# ---------------------------------------------------------------------------
# Refresh loop
# ---------------------------------------------------------------------------

async def refresh_loop():
    """Background task: refresh cookies every REFRESH_INTERVAL seconds."""
    log.info("Cookie refresh loop started (interval: %ds)", REFRESH_INTERVAL)

    # Initial delay — give bot time to start
    await asyncio.sleep(30)

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

        log.info("Running scheduled cookie refresh...")

        try:
            count = await export_cookies_from_profile()

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

    # Add detailed cookie analysis
    analysis = get_cookie_analysis()
    status["cookie_analysis"] = analysis
    status["required_found"] = analysis["required_found"]
    status["required_missing"] = analysis["required_missing"]
    status["invalid_reason"] = analysis["reason"]

    return web.json_response(status)


async def handle_export_cookies(request):
    try:
        count = await export_cookies_from_profile()
        status["last_refresh"] = datetime.now(timezone.utc).isoformat()
        status["last_refresh_success"] = True
        status["cookie_count"] = count
        status["last_error"] = None
        status["needs_relogin"] = not check_session_valid(count)
        return web.json_response({"success": True, "cookie_count": count})
    except Exception as e:
        status["last_error"] = str(e)
        return web.json_response({"success": False, "error": str(e)}, status=500)


async def handle_cookie_debug(request):
    """Return detailed cookie analysis for debugging."""
    result = {
        "cookies_file_exists": os.path.exists(COOKIES_FILE),
        "profile_exists": os.path.exists(BROWSER_PROFILE_DIR),
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

                    # Mark important cookies
                    cookie_info["is_required"] = name in REQUIRED_COOKIES
                    cookie_info["is_tracked"] = name in TRACKED_COOKIES

                    cookies.append(cookie_info)

            result["cookies"] = cookies
            result["total_cookies"] = len(cookies)

            # Analysis
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
    """Start refresh loop on server startup."""
    _init_browser_lock()
    app["refresh_task"] = asyncio.ensure_future(refresh_loop())


async def on_cleanup(app):
    """Cleanup on server shutdown."""
    task = app.get("refresh_task")
    if task:
        task.cancel()
        try:
            await task
        except asyncio.CancelledError:
            pass

    # Stop login if active
    if status["login_active"]:
        await stop_login()


def main():
    log.info("Cookie Manager starting on %s:%d", API_HOST, API_PORT)
    log.info("  Cookies file: %s", COOKIES_FILE)
    log.info("  Browser profile: %s", BROWSER_PROFILE_DIR)
    log.info("  Refresh interval: %ds", REFRESH_INTERVAL)
    log.info("  Chromium path: %s", CHROMIUM_PATH)
    log.info("  ChromeDriver path: %s", CHROMEDRIVER_PATH)

    # Kill orphaned Chrome processes and clean up stale lock files
    _kill_chrome_on_profile(BROWSER_PROFILE_DIR)
    _cleanup_profile_locks(BROWSER_PROFILE_DIR)

    app = web.Application()
    app.router.add_post("/api/login_start", handle_login_start)
    app.router.add_post("/api/login_stop", handle_login_stop)
    app.router.add_get("/api/status", handle_status)
    app.router.add_post("/api/export_cookies", handle_export_cookies)
    app.router.add_get("/api/cookie_debug", handle_cookie_debug)

    app.on_startup.append(on_startup)
    app.on_cleanup.append(on_cleanup)

    web.run_app(app, host=API_HOST, port=API_PORT, print=None)


if __name__ == "__main__":
    main()
