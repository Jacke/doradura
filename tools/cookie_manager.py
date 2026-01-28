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
DISPLAY = ":99"
CHROMIUM_PATH = os.environ.get("CHROMIUM_PATH", "/usr/bin/chromium-browser")
CHROMEDRIVER_PATH = os.environ.get("CHROMEDRIVER_PATH", "/usr/bin/chromedriver")

# Required YouTube/Google cookies that indicate a valid session
REQUIRED_COOKIES = {"SID", "HSID", "SSID", "APISID", "SAPISID"}

# ---------------------------------------------------------------------------
# Logging
# ---------------------------------------------------------------------------

logging.basicConfig(
    level=logging.INFO,
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
    if headless:
        opts.add_argument("--headless=new")
    else:
        opts.add_argument("--start-maximized")
        opts.add_argument("--window-size=1280,720")
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


def _export_cookies_from_driver(driver: uc.Chrome) -> int:
    """Extract cookies from an open driver and write to file."""
    # Navigate to YouTube to ensure cookies are fresh
    driver.get("https://www.youtube.com")

    # Collect cookies from both YouTube and Google domains
    cookies = driver.get_cookies()

    # Also grab Google cookies by visiting accounts.google.com
    try:
        driver.get("https://accounts.google.com")
        cookies.extend(driver.get_cookies())
    except Exception as e:
        log.warning("Could not get Google cookies: %s", e)

    # Deduplicate by (domain, name)
    seen = set()
    unique = []
    for c in cookies:
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
        "Exported %d cookies (%s session cookies)",
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

    Copies the persistent browser profile to a temporary directory to avoid
    Chrome's "user data directory already in use" lock conflict with any
    concurrent headed session or previous crash.
    """
    tmp_dir = tempfile.mkdtemp(prefix="cookie_refresh_")
    try:
        # Copy profile to temp dir (ignore errors for lock/socket files)
        tmp_profile = os.path.join(tmp_dir, "profile")
        shutil.copytree(
            BROWSER_PROFILE_DIR,
            tmp_profile,
            ignore=shutil.ignore_patterns(
                "SingletonLock", "SingletonCookie", "SingletonSocket",
            ),
        )
        log.info("Copied profile to temp dir: %s", tmp_profile)

        driver = _create_driver(headless=True, profile_dir=tmp_profile)
        try:
            return _export_cookies_from_driver(driver)
        finally:
            driver.quit()
    finally:
        shutil.rmtree(tmp_dir, ignore_errors=True)


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
        ["Xvfb", DISPLAY, "-screen", "0", "1280x720x24"],
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
    vnc_proc = subprocess.Popen(
        [
            "x11vnc",
            "-display", DISPLAY,
            "-nopw",
            "-forever",
            "-rfbport", str(VNC_PORT),
            "-shared",
        ],
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

    log.info("Stopping login session...")

    result = {"status": "ok", "cookies_exported": False, "cookie_count": 0}

    # Export cookies before closing
    driver = login_state.get("driver")
    if driver:
        try:
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

    app.on_startup.append(on_startup)
    app.on_cleanup.append(on_cleanup)

    web.run_app(app, host=API_HOST, port=API_PORT, print=None)


if __name__ == "__main__":
    main()
