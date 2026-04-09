# Security Review: Doradura

**Date:** 2026-04-09
**Scope:** Top rating-5 files in `crates/doracore/src/download/source/`, `crates/doracore/src/core/web/`, `crates/dorabot/src/core/`, `crates/doracore/src/download/cookies.rs`
**Reviewer:** Claude-assisted security audit (Phase 1 prioritisation → Phase 2 deep audit → Phase 3 skeptical validation → Phase 4 fix)

---

## Executive Summary

A four-phase security audit was performed on the 10 highest-priority files of the Doradura Telegram bot + admin panel — the Rust workspace powering `@DoraDuraDoraDuraBot`. Five parallel deep-audit agents produced 73 raw findings (19 CRITICAL, 22 HIGH, 21 MEDIUM, 11 LOW); three validation agents then played skeptical second-reviewer and filtered out false positives, leaving **11 confirmed CRITICAL issues** worth immediate action.

The most serious findings cluster around **(1) the HTTP downloader's SSRF guard**, which had six bypass vectors ranging from DNS rebinding to incomplete IPv4/IPv6 deny-lists; **(2) the payment pipeline**, where `save_charge` and subscription updates were not atomic, Postgres writes lacked transactions, and there is no handler for Telegram's `RefundedPayment` event; and **(3) the admin web panel**, whose session cookie is a deterministic `sha256(user_id:bot_token)` with no server-side state and which mounts admin routes on the public listener with no IP allowlist. A single `format!()`-based SQL injection was found in the audit endpoint — the only unsanitised query-builder in the admin module.

Three critical issues were fixed in-session (SSRF guard, admin SQL injection, `INSERT OR REPLACE` data-corruption); seven remain open and are documented here with concrete remediation plans.

---

## Findings

### 1. SSRF: DNS rebinding bypass — **Critical**

**CWE:** CWE-918 (Server-Side Request Forgery), CWE-367 (TOCTOU)
**Location:** `crates/doracore/src/download/source/http.rs:51-79`, `:222`, `:243`
**Confidence:** High
**Status:** ✅ **Fixed** in commit `6de099b85051`

**Description:**
`check_ssrf` called `tokio::net::lookup_host` to validate the target host, then handed the original URL back to `reqwest::Client::get()`, which performed its **own independent DNS resolution** when actually connecting. An attacker controlling the authoritative resolver for their domain could return a public IP for the first lookup (passes the guard) and `127.0.0.1`/`169.254.169.254` for the second lookup (the real connection).

**Impact:**
Full read access to the container's loopback services (Bot API on :8081, metrics on :9090, bgutil on :4416) and the AWS/GCP/Azure instance metadata service (IAM credentials, instance-identity documents).

**Proof of Concept:**
```python
# Attacker's authoritative DNS for rebind.attacker.tld with TTL=0
# Response 1: A 93.184.216.34    (passes check_ssrf)
# Response 2: A 169.254.169.254  (reqwest connects here)

# User sends: https://rebind.attacker.tld/file.mp3
# Bot downloads IAM creds from AWS IMDSv1 into /data/...
```

**Remediation (applied):**
Build a per-request `reqwest::Client` via `ClientBuilder::resolve_to_addrs(host, &validated_addrs)`. reqwest now connects only to the exact IPs returned by the initial lookup — the second DNS query is bypassed entirely.

---

### 2. SSRF: Redirect chain fetched before validation — **Critical**

**CWE:** CWE-918
**Location:** `crates/doracore/src/download/source/http.rs:237-243`
**Confidence:** High
**Status:** ✅ **Fixed** in commit `6de099b85051`

**Description:**
`reqwest::Client` default redirect policy is `Policy::limited(10)`. Code called `req.send().await` (which follows all redirects internally) and only called `check_ssrf(response.url())` on the **final** URL. Intermediate hops to internal hosts already completed, headers were already sent, and cloud metadata endpoints were already hit.

**Impact:**
Attacker can probe the internal network (`10.0.0.0/8`, `169.254.169.254`, any service behind the Railway private DNS) via redirect chains that end at a public URL to satisfy the post-hoc check.

**Proof of Concept:**
```
https://attacker.tld/file.mp3
  → 302 Location: http://169.254.169.254/latest/meta-data/iam/security-credentials/role
  → (attacker's server captures the request then) 302 Location: https://public.example.com/decoy.mp3
# Final URL passes check_ssrf; intermediate metadata GET already happened.
```

**Remediation (applied):**
`Policy::none()` on the pinned client + manual redirect loop (max 5 hops), each hop re-resolves DNS and re-runs `check_ssrf` BEFORE the next connection. Cross-host redirects rebuild the pinned client for the new host.

---

### 3. SSRF: Incomplete IPv4/IPv6 deny-list — **Critical**

**CWE:** CWE-918, CWE-1287
**Location:** `crates/doracore/src/download/source/http.rs:30-43`
**Confidence:** High
**Status:** ✅ **Fixed** in commit `6de099b85051`

**Description:**
`is_private_ip` used `Ipv4Addr::is_private()` which only covers RFC1918. Missed ranges exploitable on Railway:

| Range | Why it matters |
|---|---|
| `0.0.0.0/8` | Linux routes to localhost (RFC 1122 §3.2.1.3a) |
| `100.64.0.0/10` | CGNAT — **Railway/Tailscale internal mesh** |
| `100.100.100.200` | Alibaba Cloud ECS metadata |
| `192.0.0.192` | Oracle OCI metadata |
| `224.0.0.0/4` | Multicast |
| `240.0.0.0/4` | Reserved (class E) |
| `fc00::/7` | IPv6 ULA (RFC 1918 equivalent) |
| `fe80::/10` | IPv6 link-local |
| `::ffff:127.0.0.1` | IPv4-mapped loopback — **classic bypass** |
| `::ffff:169.254.169.254` | IPv4-mapped metadata — **classic bypass** |

**Impact:**
Depending on the cloud provider and Railway internal networking, attacker reaches the metadata service, Tailscale peers, other tenants on CGNAT, or the container's own localhost services.

**Remediation (applied):**
Hand-rolled comprehensive `is_private_ip` with explicit matches on every range above. For IPv6-mapped addresses, unwrap to `Ipv4Addr` and recurse through the IPv4 check. Scheme whitelist (`http`/`https` only) added as defence in depth.

**Tests added:** 11 new test cases (CGNAT, zero-network, Alibaba, Oracle, class E, IPv6 ULA, link-local, IPv4-mapped loopback, IPv4-mapped metadata, documentation, non-http schemes).

---

### 4. SQL Injection in `admin_api_audit` — **Critical**

**CWE:** CWE-89
**Location:** `crates/doracore/src/core/web/admin_misc.rs:878-899`
**Confidence:** High
**Status:** ✅ **Fixed** in commit `68748d58db20`

**Description:**
The `admin_api_audit` handler read the `action` query parameter as `String` and interpolated it directly into SQL:
```rust
let where_clause = if !action_filter.is_empty() {
    format!("WHERE action = '{}'", action_filter)  // <-- unsafe
} else { String::new() };
```
Unlike every other `format!()` query-builder in `admin_misc.rs` (which match-on an allowlist of literal values), the audit handler had no validation. Any authenticated admin could run arbitrary SELECTs via `UNION`.

**Impact:**
Full-read SQL injection over the SQLite DB: `users`, `charges`, `subscriptions`, `sessions`, `content_subscriptions`, `error_log`. Combined with Finding #5 (deterministic admin cookie) and Finding #6 (no IP allowlist), this is exploitable from the public internet by anyone with `BOT_TOKEN`.

**Proof of Concept:**
```http
GET /admin/api/audit?action=x'%20UNION%20SELECT%20telegram_id,username,'','','','',''%20FROM%20users--
Cookie: admin_token=<forged-via-sha256(id:token)>
```

**Remediation (applied):**
Allowlist `action` against the 15 known audit action types, then use `rusqlite::params!()` parameterised binding for the WHERE clause. LIMIT/OFFSET remain `format!()`-interpolated but are `u32`-typed and safe.

---

### 5. Deterministic admin session cookie — **Critical**

**CWE:** CWE-330 (Insufficiently Random Values), CWE-613 (Insufficient Session Expiration), CWE-384 (Session Fixation)
**Location:** `crates/doracore/src/core/web/auth.rs:137-141`, `:155-180`, `:320-325`, `:336-344`
**Confidence:** High
**Status:** 🔴 **Open** — large refactor required

**Description:**
```rust
pub(super) fn generate_admin_token(user_id: i64, bot_token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(format!("{}:{}", user_id, bot_token));
    hex::encode(hasher.finalize())
}
```

The admin cookie is a pure function of two static inputs:
- **No randomness** — every admin always gets the same cookie value across all sessions.
- **No server-side state** — `verify_admin` recomputes `sha256(user_id:bot_token)` for each known `ADMIN_IDS` and compares. There is no session table.
- **No expiry** — `Max-Age=86400` is a browser hint only; server accepts the cookie forever, until `BOT_TOKEN` is rotated.
- **No revocation** — `admin_logout_handler` only sends `Max-Age=0` to the browser. Captured cookies remain valid.
- **Offline forgery** — anyone with `BOT_TOKEN` + an admin Telegram ID computes the cookie without touching the server. `BOT_TOKEN` lives in Railway env, CI artifacts, historical logs.

**Impact:**
A single leak of `BOT_TOKEN` is equivalent to permanent admin compromise that cannot be remediated without rotating the bot.

**Remediation (planned):**
1. Create migration `V45__admin_sessions.sql` with schema:
   ```sql
   CREATE TABLE admin_sessions (
       token_hash BLOB PRIMARY KEY,     -- sha256(raw_token)
       admin_id   INTEGER NOT NULL,
       created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
       expires_at TEXT NOT NULL,
       last_seen  TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
       user_agent TEXT,
       ip         TEXT
   );
   ```
2. Replace `generate_admin_token` with `new_session_token()` using `rand::thread_rng().fill_bytes(&mut [u8; 32])`, store `sha256(token)` in the DB.
3. `verify_admin` looks up `token_hash` with `expires_at > datetime('now')`.
4. `admin_logout_handler` calls `DELETE FROM admin_sessions WHERE token_hash = ?`.
5. Add a "Log out everywhere" endpoint that wipes all sessions for an `admin_id`.

---

### 6. No IP allowlist on admin routes — **Critical**

**CWE:** CWE-284 (Improper Access Control)
**Location:** `crates/doracore/src/core/web/mod.rs:65-112`
**Confidence:** High
**Status:** 🔴 **Open**

**Description:**
The task description states the threat model includes an IP allowlist, but `grep -rn "ADMIN_IP|IP_ALLOWLIST" crates/` returns nothing. All `/admin/*` routes are mounted on the same public listener as `/s/{id}` and `/health`. Combined with Finding #5, anyone with `BOT_TOKEN` has global admin access from the public internet.

Additionally, `auth.rs:60-68` trusts `X-Forwarded-For` blindly without a trusted-proxy count — any IP allowlist added would be trivially bypassable by spoofing the header.

**Remediation (planned):**
1. Add a tower middleware that runs before `/admin/*`:
   ```rust
   async fn admin_ip_guard(
       ConnectInfo(addr): ConnectInfo<SocketAddr>,
       req: Request,
       next: Next,
   ) -> Response {
       if !req.uri().path().starts_with("/admin") { return next.run(req).await; }
       let allowlist = env::var("ADMIN_IP_ALLOWLIST").unwrap_or_default();
       if allowlist.is_empty() {
           return (StatusCode::NOT_FOUND, "Not Found").into_response();  // fail closed
       }
       let peer = trusted_client_ip(&req, &addr);  // honours TRUSTED_PROXY_HOPS
       if allowlist.split(',').any(|s| s.trim().parse::<IpAddr>().ok() == Some(peer)) {
           next.run(req).await
       } else {
           (StatusCode::NOT_FOUND, "Not Found").into_response()
       }
   }
   ```
2. Derive client IP from `ConnectInfo<SocketAddr>` unless `TRUSTED_PROXY_HOPS > 0`, then take the Nth-from-right entry of `X-Forwarded-For`.
3. Serve with `app.into_make_service_with_connect_info::<SocketAddr>()`.
4. Document `ADMIN_IP_ALLOWLIST` and `TRUSTED_PROXY_HOPS` env vars.

---

### 7. `save_charge` + `update_subscription_data` not atomic — **Critical**

**CWE:** CWE-662 (Improper Synchronization), CWE-362 (Race Condition)
**Location:** `crates/dorabot/src/core/subscription.rs:735-811`
**Confidence:** High
**Status:** 🔴 **Open**

**Description:**
`handle_successful_payment` runs two sequential storage calls:
1. `shared_storage.save_charge(...)` — persists the charge row.
2. `shared_storage.update_subscription_data(...)` — upserts the subscription row + updates `users.plan`.

If step 2 fails (DB error, network blip, process kill), the charge is persisted but the subscription is not activated. Because `charges` has `UNIQUE (telegram_charge_id)`, any retry (manual or from Telegram redelivery) is blocked. The user paid but gets nothing, and recovery requires manual admin SQL intervention.

**Impact:**
Silent payment loss under transient DB failure. No automatic recovery path.

**Remediation (planned):**
Add `SharedStorage::record_successful_payment()` that runs all three writes (charges insert, subscriptions upsert, users update) in a single transaction. For SQLite use `conn.transaction()`; for Postgres use `pg_pool.begin().await?` + commit. The new method becomes the only payment path; existing `save_charge` / `update_subscription_data` remain for admin flows.

---

### 8. Postgres `update_subscription_data` not transactional — **Critical**

**CWE:** CWE-662
**Location:** `crates/doracore/src/storage/shared/subscriptions.rs:243-298`
**Confidence:** High
**Status:** 🔴 **Open**

**Description:**
The SQLite path of `update_subscription_data` wraps INSERT + UPDATE in `BEGIN IMMEDIATE` / `COMMIT`. The Postgres path runs two independent `sqlx::query(...).execute(pg_pool)` calls, each its own auto-committed transaction. Same issue in `cancel_subscription`.

If the second UPDATE fails after the first INSERT succeeds, `subscriptions` says premium while `users.plan` stays stale. Partially mitigated for reads by `get_user`'s `COALESCE(s.plan, u.plan)` but the admin listing screens read `users.plan` directly.

**Remediation (planned):**
Wrap both operations in `let mut tx = pg_pool.begin().await?;` and commit at the end. Apply the same fix to `cancel_subscription`.

---

### 9. Admin `INSERT OR REPLACE` wipes `telegram_charge_id` — **Critical**

**CWE:** CWE-471 (Modification of Assumed-Immutable Data), CWE-840 (Business Logic Errors)
**Location:** `crates/doracore/src/core/web/admin_users.rs:165-191`, `:646-667`
**Confidence:** High
**Status:** ✅ **Fixed** in commit `fbd987ecad38`

**Description:**
SQLite's `INSERT OR REPLACE` performs DELETE + INSERT on PK conflict, wiping every column not listed. The admin handler wrote:
```sql
INSERT OR REPLACE INTO subscriptions (user_id, plan, expires_at) VALUES (...)
```
which silently wiped `telegram_charge_id`, `is_recurring`, and `created_at`. After an admin extends a recurring premium user's expiry, the bot loses its handle to the Telegram subscription. User cannot cancel auto-renewal via the bot; Telegram keeps billing indefinitely.

**Impact:**
Real money loss with no user recourse. Admin helpfulness = permanent billing lockout.

**Remediation (applied):**
Replaced with `INSERT ... ON CONFLICT(user_id) DO UPDATE SET plan = excluded.plan, expires_at = excluded.expires_at, updated_at = datetime('now')`. Also fixed a related SQLi pattern: the previous code interpolated `format!("datetime('now', '+{} days')", clamped)` — the integer is bounded but the pattern is dangerous; replaced with `datetime('now', '+' || ?3 || ' days')` parameterised.

---

### 10. No `RefundedPayment` handler — **Critical**

**CWE:** CWE-840 (Business Logic Errors)
**Location:** Absent (no handler in `crates/dorabot/src/telegram/handlers/schema.rs`)
**Confidence:** High
**Status:** 🔴 **Open**

**Description:**
`grep -rn "refunded_payment\|RefundedPayment" crates/` finds only outbound `bot.refund_star_payment(...)` calls for admin-initiated refunds, not a handler for incoming refund updates. When Telegram delivers a `Message::refunded_payment` event (after a user disputes or admin issues a refund), the bot ignores it. Subscription row stays active, `users.plan` stays paid, and the user retains premium access.

**Impact:**
Free premium for up to 30 days per refund cycle, repeatable.

**Remediation (planned):**
1. Add an `Update::filter_message().filter_map(|m| m.refunded_payment().cloned())` branch to the dispatcher.
2. Implement `handle_refunded_payment` that looks up `user_id` by `telegram_charge_id`, marks the charge as refunded (add `refunded_at` column), downgrades the subscription to free in a single transaction, notifies the user and admin.
3. Add `SharedStorage::cancel_subscription_by_refund(user_id, charge_id)`.

---

### 11. `is_paid()` ignores subscription expiry — **High** (downgraded from CRIT)

**CWE:** CWE-287 (Improper Authentication)
**Location:** `crates/doracore/src/core/types.rs:22-24`
**Confidence:** High
**Status:** 🔴 **Open**

**Description:**
`Plan::is_paid()` is `matches!(self, Plan::Premium | Plan::Vip)` — does not check `subscription_expires_at`. Users retain paid feature access between real expiry and the next hourly reaper tick (`background_tasks.rs:85`, interval `60 * 60`). Subscription-access checks in `core/subscription.rs:141` correctly check expiry; the bug is specifically in hot paths that consult `Plan::is_paid()` directly (`menu/audio_effects.rs:37,340`, `storage/db/sessions.rs:14`).

**Impact:**
Up to ~60 minutes of unauthorised feature access per user per expiry.

**Remediation (planned):**
Introduce `is_subscription_active(user: &User) -> bool` that checks both plan AND `expires_at > now()`. Replace every `plan.is_paid()` call in hot paths with `is_subscription_active(user)`.

---

## Additional Confirmed Findings (High/Medium)

| # | Title | Severity | Location | Status |
|---|-------|---------|----------|--------|
| 12 | `pre_checkout` doesn't check `from.id == payload_user_id` | High | `handlers/schema.rs:767-807` | Open |
| 13 | `X-Forwarded-For` trusted without proxy count | High | `web/auth.rs:60-68` | Open |
| 14 | CSP allows `'unsafe-inline'` scripts | High | `web/mod.rs:42-46` | Open |
| 15 | Latent SQL footguns (`format!()` with allowlisted strings) | High | multiple web handlers | Open |
| 16 | Timing side-channel in admin ID loop | High | `web/auth.rs:163-178` | Open |
| 17 | Logout is cosmetic (no server invalidation) | High | `web/auth.rs:336-344` | Open (fixed by #5) |
| 18 | stdout-blocks-timeout allows yt-dlp DoS | High | `download/source/ytdlp.rs:975-1018` | Open |
| 19 | Proxy credentials leak in debug log | Medium | `download/source/ytdlp.rs:472-477` | Open |
| 20 | Full `telegram_charge_id` logged at INFO | High | `subscription.rs:624-928` | Open |
| 21 | `ParsedCookie` derives Debug with secret field | Medium | `download/cookies.rs:183-190` | Open (latent) |
| 22 | Cookie files world-readable (0644) | Medium | `download/cookies.rs:993` | Open (mitigated by single-tenant) |
| 23 | Broadcast handler leaks Telegram errors into logs | Medium | `web/admin_misc.rs:510-522` | Open |
| 24 | Broadcast uses default-timeout reqwest client | Medium | `web/admin_misc.rs:561-588` | Open |

---

## False Positives (explicitly ruled out)

- **`http.rs` scheme smuggling** — `supports_url()` blocks non-http(s) schemes at the registry level before `HttpSource::download` is reached.
- **Command injection via yt-dlp args** — `std::process::Command::new().args()` passes argv, no shell expansion. All user-controlled values (URL, title, time range) are either passed as the final arg or parsed to numerics before interpolation.
- **Unbounded HTTP download size** — all production callers set `max_file_size` from `format.max_file_size()` (49/100/200 MB by plan). The `Option<u64>` type issue is a type-safety hardening concern, not an exploit.
- **`update_cookies_from_base64` OOM** — function is `#[cfg(test)]`-only. Production upload path goes through `update_cookies_from_content` which is bounded by Telegram's Bot API file size limit.
- **Cookie temp-file symlink attack** — Railway containers are single-tenant; `/data` is owned by `botuser:shareddata` + `telegram-bot-api:shareddata`, both trusted.
- **`pre_checkout` trusting payload (as claimed)** — amount, currency, plan ARE validated in `handle_successful_payment`. The reframed issue is Finding #12 (missing `from.id` cross-check).

---

## Out of Scope / Not Reviewed

- Instagram source (`crates/doracore/src/download/source/instagram.rs`) — 1600 lines, deferred.
- MTProto downloader — cursory look only, fixed OOM bounded-Vec earlier in this session.
- TUI client (`crates/doratui/`) — local-only, no network trust boundary.
- Test files — not reviewed for security.
- Dependency chain — `cargo audit` not run in this session (last run: 2026-04-08, 1 medium `actix-web-lab` known, dev-only).
- Instagram cookies code path — only audited via `cookies.rs` generic code.

---

## Recommended Next Steps

**Priority 1 — Remaining critical fixes (open):**
1. **Finding #5** — Replace deterministic admin cookie with random session token table. Migration + ~80 lines of code. Half a day.
2. **Finding #6** — Add IP allowlist middleware + `TRUSTED_PROXY_HOPS`. 1 hour.
3. **Finding #7** — `SharedStorage::record_successful_payment` atomic method. 2 hours.
4. **Finding #8** — Wrap Postgres subscription writes in `pg_pool.begin()`. 30 min.
5. **Finding #10** — Add `RefundedPayment` handler + `refunded_at` column + revoke path. 2 hours.
6. **Finding #11** — `is_subscription_active` helper + replace hot-path `is_paid()` calls. 1 hour.

**Priority 2 — High findings (open):**
7. **Finding #12** — Verify `msg.from.id` matches payload user_id in `handle_successful_payment`.
8. **Finding #18** — Fix yt-dlp stdout-blocks-timeout DoS (stderr reader thread pattern).
9. **Finding #20** — Redact `telegram_charge_id` in INFO logs (show only last 6 chars).
10. **Finding #13** — Harden `X-Forwarded-For` parsing.
11. **Finding #14** — Switch CSP to nonce-based scripts.

**Priority 3 — Hardening (medium):**
12. **Finding #15** — Mechanical sweep of `format!()`-into-SQL footguns.
13. **Finding #21** — Custom `Debug` impl on `ParsedCookie` to prevent latent secret leak.
14. **Finding #22** — `OpenOptions::mode(0o600)` for cookie file writes.
15. Add a clippy/grep CI gate: `! rg "format!\(.*(WHERE|SELECT|INSERT|UPDATE|DELETE) .* '" crates/`.

**Priority 4 — Operational:**
16. Rotate `BOT_TOKEN` after Finding #5 is fixed (old admin cookies remain valid until then).
17. Audit Railway logs for historic proxy credential leakage (Finding #19).
18. Add Prometheus alert for anomalous `payment_success_total` rate per user (Finding MED-2).
19. Schedule follow-up audit of `instagram.rs`, `mtproto/downloader.rs`, and the remaining `web/*` handlers.

---

## Fixes Applied in This Session

| Commit | Finding | Files |
|---|---|---|
| `6de099b85051` | #1, #2, #3 (SSRF hardening) | `download/source/http.rs` |
| `68748d58db20` | #4 (SQL injection in admin audit) | `web/admin_misc.rs` |
| `fbd987ecad38` | #9 (INSERT OR REPLACE → ON CONFLICT) | `web/admin_users.rs`, `storage/db/mod.rs` |

All three commits passed `cargo fmt`, `cargo clippy -D warnings`, and the full workspace test suite. 11 new SSRF unit tests added; 25 pass.
