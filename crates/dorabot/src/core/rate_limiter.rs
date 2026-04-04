use anyhow::{anyhow, Context, Result};
use doracore::core::config::{self, DatabaseDriver};
use redis::AsyncCommands;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use teloxide::types::ChatId;
use tokio::sync::Mutex;
use tokio::time::{Duration, Instant};

const REDIS_KEY_PREFIX: &str = "doradura:rate_limit";

#[derive(Clone)]
enum RateLimiterBackend {
    Memory(Arc<Mutex<HashMap<ChatId, Instant>>>),
    Redis(redis::Client),
}

/// Rate limiter that throttles how often users can send requests.
///
/// Applies different limits per subscription plan.
#[derive(Clone)]
pub struct RateLimiter {
    backend: RateLimiterBackend,
    /// Base delay between requests for the free plan
    free_duration: Duration,
    /// Delay between requests for the premium plan
    premium_duration: Duration,
    /// Delay between requests for the VIP plan
    vip_duration: Duration,
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

impl RateLimiter {
    /// Creates an in-memory rate limiter with default limits per plan.
    ///
    /// Returns a `RateLimiter` configured as:
    /// - Free: 30 seconds between requests
    /// - Premium: 10 seconds between requests
    /// - VIP: 5 seconds between requests
    pub fn new() -> Self {
        Self {
            backend: RateLimiterBackend::Memory(Arc::new(Mutex::new(HashMap::new()))),
            free_duration: Duration::from_secs(30),
            premium_duration: Duration::from_secs(10),
            vip_duration: Duration::from_secs(5),
        }
    }

    /// Creates a rate limiter from the runtime configuration.
    ///
    /// - SQLite mode uses in-memory cooldown tracking.
    /// - PostgreSQL mode requires Redis for distributed cooldown tracking.
    pub fn from_config() -> Result<Self> {
        match *config::DATABASE_DRIVER {
            DatabaseDriver::Sqlite => Ok(Self::new()),
            DatabaseDriver::Postgres => {
                let redis_url = config::REDIS_URL
                    .as_ref()
                    .filter(|value| !value.trim().is_empty())
                    .ok_or_else(|| anyhow!("REDIS_URL must be set when DATABASE_DRIVER=postgres"))?;
                let client = redis::Client::open(redis_url.as_str()).context("create Redis client for rate limiter")?;
                Ok(Self {
                    backend: RateLimiterBackend::Redis(client),
                    ..Self::new()
                })
            }
        }
    }

    /// Creates an in-memory rate limiter with custom limits.
    ///
    /// * `free_duration` - Delay between requests for the free plan
    /// * `premium_duration` - Delay between requests for the premium plan
    /// * `vip_duration` - Delay between requests for the VIP plan
    pub fn with_durations(free_duration: Duration, premium_duration: Duration, vip_duration: Duration) -> Self {
        Self {
            backend: RateLimiterBackend::Memory(Arc::new(Mutex::new(HashMap::new()))),
            free_duration,
            premium_duration,
            vip_duration,
        }
    }

    /// Gets the throttle duration for the given plan.
    fn get_duration_for_plan(&self, plan: &str) -> Duration {
        match plan {
            "free" | "" => self.free_duration,
            "premium" => self.premium_duration,
            "vip" => self.vip_duration,
            other => {
                log::warn!("Unknown plan '{}' in rate limiter, defaulting to free", other);
                self.free_duration
            }
        }
    }

    fn redis_key(chat_id: ChatId) -> String {
        format!("{REDIS_KEY_PREFIX}:{}", chat_id.0)
    }

    fn now_millis() -> i64 {
        match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(duration) => duration.as_millis().min(i64::MAX as u128) as i64,
            Err(_) => 0,
        }
    }

    fn remaining_from_expiry(expires_at_ms: i64) -> Option<Duration> {
        let now_ms = Self::now_millis();
        if expires_at_ms <= now_ms {
            return None;
        }
        Some(Duration::from_millis((expires_at_ms - now_ms) as u64))
    }

    /// Atomically checks whether a user is limited and starts a new cooldown if not.
    ///
    /// Returns `Ok(None)` if the request is allowed and the cooldown was reserved.
    /// Returns `Ok(Some(duration))` if the user is still cooling down.
    pub async fn check_and_update(&self, chat_id: ChatId, plan: &str) -> Result<Option<Duration>> {
        let duration = self.get_duration_for_plan(plan);
        match &self.backend {
            RateLimiterBackend::Memory(limits) => {
                let mut limits = limits.lock().await;
                let now = Instant::now();
                if let Some(&instant) = limits.get(&chat_id) {
                    if now < instant {
                        return Ok(Some(instant - now));
                    }
                }
                limits.insert(chat_id, now + duration);
                Ok(None)
            }
            RateLimiterBackend::Redis(client) => {
                let script = redis::Script::new(
                    r#"
                    local current = redis.call('GET', KEYS[1])
                    if current then
                        return tonumber(current)
                    end
                    redis.call('SET', KEYS[1], ARGV[2], 'PX', ARGV[1], 'NX')
                    return 0
                    "#,
                );
                let ttl_ms = duration.as_millis().min(i64::MAX as u128) as i64;
                let expires_at_ms = Self::now_millis() + ttl_ms;
                let mut conn = client
                    .get_multiplexed_async_connection()
                    .await
                    .context("open Redis rate limiter connection")?;
                let existing_expires_at_ms: i64 = script
                    .key(Self::redis_key(chat_id))
                    .arg(ttl_ms)
                    .arg(expires_at_ms)
                    .invoke_async(&mut conn)
                    .await
                    .context("run Redis rate limiter check-and-set")?;
                if existing_expires_at_ms == 0 {
                    Ok(None)
                } else {
                    Ok(Self::remaining_from_expiry(existing_expires_at_ms).or(Some(Duration::from_millis(1))))
                }
            }
        }
    }

    /// Checks whether a user is currently rate-limited.
    pub async fn is_rate_limited(&self, chat_id: ChatId, _plan: &str) -> bool {
        match self.get_remaining_time(chat_id).await {
            Ok(Some(_)) => true,
            Ok(None) => false,
            Err(e) => {
                log::error!("Rate limiter check failed for {}: {}", chat_id.0, e);
                true
            }
        }
    }

    /// Returns remaining time until the user is unlocked.
    pub async fn get_remaining_time(&self, chat_id: ChatId) -> Result<Option<Duration>> {
        match &self.backend {
            RateLimiterBackend::Memory(limits) => {
                let limits = limits.lock().await;
                if let Some(&instant) = limits.get(&chat_id) {
                    let now = Instant::now();
                    if now < instant {
                        return Ok(Some(instant - now));
                    }
                }
                Ok(None)
            }
            RateLimiterBackend::Redis(client) => {
                let mut conn = client
                    .get_multiplexed_async_connection()
                    .await
                    .context("open Redis rate limiter connection")?;
                let expires_at_ms: Option<i64> = conn
                    .get(Self::redis_key(chat_id))
                    .await
                    .context("fetch Redis rate limiter TTL value")?;
                Ok(expires_at_ms.and_then(Self::remaining_from_expiry))
            }
        }
    }

    /// Force-refreshes the cooldown for a user.
    pub async fn update_rate_limit(&self, chat_id: ChatId, plan: &str) -> Result<()> {
        let duration = self.get_duration_for_plan(plan);
        match &self.backend {
            RateLimiterBackend::Memory(limits) => {
                let mut limits = limits.lock().await;
                limits.insert(chat_id, Instant::now() + duration);
                Ok(())
            }
            RateLimiterBackend::Redis(client) => {
                let ttl_ms = duration.as_millis().min(i64::MAX as u128) as u64;
                let expires_at_ms = Self::now_millis() + ttl_ms as i64;
                let mut conn = client
                    .get_multiplexed_async_connection()
                    .await
                    .context("open Redis rate limiter connection")?;
                let _: () = redis::cmd("SET")
                    .arg(Self::redis_key(chat_id))
                    .arg(expires_at_ms)
                    .arg("PX")
                    .arg(ttl_ms)
                    .query_async(&mut conn)
                    .await
                    .context("update Redis rate limiter value")?;
                Ok(())
            }
        }
    }

    /// Removes the limit for the given user.
    pub async fn remove_rate_limit(&self, chat_id: ChatId) -> Result<()> {
        match &self.backend {
            RateLimiterBackend::Memory(limits) => {
                let mut limits = limits.lock().await;
                limits.remove(&chat_id);
                Ok(())
            }
            RateLimiterBackend::Redis(client) => {
                let mut conn = client
                    .get_multiplexed_async_connection()
                    .await
                    .context("open Redis rate limiter connection")?;
                let _: usize = conn
                    .del(Self::redis_key(chat_id))
                    .await
                    .context("delete Redis rate limiter key")?;
                Ok(())
            }
        }
    }

    /// Removes expired in-memory entries from the rate limiter HashMap.
    ///
    /// Redis mode relies on key TTLs, so cleanup is a no-op there.
    pub async fn cleanup_expired(&self) -> usize {
        match &self.backend {
            RateLimiterBackend::Memory(limits) => {
                let mut limits = limits.lock().await;
                let now = Instant::now();
                let initial_len = limits.len();
                limits.retain(|_, instant| now < *instant);
                let removed = initial_len - limits.len();
                if removed > 0 {
                    log::debug!(
                        "Rate limiter cleanup: removed {} expired entries, {} remaining",
                        removed,
                        limits.len()
                    );
                }
                removed
            }
            RateLimiterBackend::Redis(_) => 0,
        }
    }

    /// Returns the current number of tracked users.
    ///
    /// Redis mode intentionally returns 0; the distributed limiter is TTL-backed and
    /// this count is only used for local diagnostics in tests.
    pub async fn len(&self) -> usize {
        match &self.backend {
            RateLimiterBackend::Memory(limits) => limits.lock().await.len(),
            RateLimiterBackend::Redis(_) => 0,
        }
    }

    /// Returns true if no users are currently tracked.
    pub async fn is_empty(&self) -> bool {
        self.len().await == 0
    }

    /// Spawns a background task that periodically cleans up expired entries.
    pub fn spawn_cleanup_task(self: Arc<Self>, interval: Duration) {
        if matches!(self.backend, RateLimiterBackend::Redis(_)) {
            return;
        }
        tokio::spawn(async move {
            let mut interval_timer = tokio::time::interval(interval);
            loop {
                interval_timer.tick().await;
                self.cleanup_expired().await;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_rate_limiter_new() {
        let limiter = RateLimiter::new();

        assert_eq!(limiter.free_duration, Duration::from_secs(30));
        assert_eq!(limiter.premium_duration, Duration::from_secs(10));
        assert_eq!(limiter.vip_duration, Duration::from_secs(5));
    }

    #[tokio::test]
    async fn test_rate_limiter_default() {
        let limiter = RateLimiter::default();

        assert_eq!(limiter.free_duration, Duration::from_secs(30));
        assert_eq!(limiter.premium_duration, Duration::from_secs(10));
        assert_eq!(limiter.vip_duration, Duration::from_secs(5));
    }

    #[tokio::test]
    async fn test_rate_limiter_with_custom_durations() {
        let limiter = RateLimiter::with_durations(
            Duration::from_secs(60),
            Duration::from_secs(20),
            Duration::from_secs(10),
        );

        assert_eq!(limiter.free_duration, Duration::from_secs(60));
        assert_eq!(limiter.premium_duration, Duration::from_secs(20));
        assert_eq!(limiter.vip_duration, Duration::from_secs(10));
    }

    #[tokio::test]
    async fn test_get_duration_for_plan() {
        let limiter = RateLimiter::new();

        assert_eq!(limiter.get_duration_for_plan("free"), Duration::from_secs(30));
        assert_eq!(limiter.get_duration_for_plan("premium"), Duration::from_secs(10));
        assert_eq!(limiter.get_duration_for_plan("vip"), Duration::from_secs(5));
        assert_eq!(limiter.get_duration_for_plan("unknown"), Duration::from_secs(30));
        assert_eq!(limiter.get_duration_for_plan(""), Duration::from_secs(30));
    }

    #[tokio::test]
    async fn test_not_rate_limited_initially() {
        let limiter = RateLimiter::new();
        let chat_id = ChatId(12345);

        let is_limited = limiter.is_rate_limited(chat_id, "free").await;
        assert!(!is_limited);
    }

    #[tokio::test]
    async fn test_check_and_update_is_atomic_for_memory_backend() {
        let limiter = RateLimiter::with_durations(
            Duration::from_millis(100),
            Duration::from_millis(50),
            Duration::from_millis(25),
        );
        let chat_id = ChatId(12346);

        assert!(limiter.check_and_update(chat_id, "free").await.unwrap().is_none());

        let remaining = limiter.check_and_update(chat_id, "free").await.unwrap();
        assert!(remaining.is_some());

        tokio::time::sleep(Duration::from_millis(150)).await;

        assert!(limiter.check_and_update(chat_id, "free").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_get_remaining_time() {
        let limiter = RateLimiter::with_durations(
            Duration::from_millis(200),
            Duration::from_millis(100),
            Duration::from_millis(50),
        );
        let chat_id = ChatId(12347);

        let remaining = limiter.get_remaining_time(chat_id).await.unwrap();
        assert!(remaining.is_none());

        limiter.update_rate_limit(chat_id, "free").await.unwrap();

        let remaining = limiter.get_remaining_time(chat_id).await.unwrap();
        assert!(remaining.is_some());
        let remaining = remaining.unwrap();
        assert!(remaining.as_millis() > 0);
        assert!(remaining.as_millis() <= 200);

        tokio::time::sleep(Duration::from_millis(250)).await;

        let remaining = limiter.get_remaining_time(chat_id).await.unwrap();
        assert!(remaining.is_none());
    }

    #[tokio::test]
    async fn test_remove_rate_limit() {
        let limiter = RateLimiter::with_durations(
            Duration::from_secs(60),
            Duration::from_secs(30),
            Duration::from_secs(15),
        );
        let chat_id = ChatId(12348);

        limiter.update_rate_limit(chat_id, "free").await.unwrap();
        assert!(limiter.is_rate_limited(chat_id, "free").await);

        limiter.remove_rate_limit(chat_id).await.unwrap();
        assert!(!limiter.is_rate_limited(chat_id, "free").await);
    }

    #[tokio::test]
    async fn test_different_plans_have_different_durations() {
        let limiter = RateLimiter::with_durations(
            Duration::from_millis(300),
            Duration::from_millis(200),
            Duration::from_millis(100),
        );

        let free_user = ChatId(1001);
        let premium_user = ChatId(1002);
        let vip_user = ChatId(1003);

        limiter.update_rate_limit(free_user, "free").await.unwrap();
        limiter.update_rate_limit(premium_user, "premium").await.unwrap();
        limiter.update_rate_limit(vip_user, "vip").await.unwrap();

        assert!(limiter.is_rate_limited(free_user, "free").await);
        assert!(limiter.is_rate_limited(premium_user, "premium").await);
        assert!(limiter.is_rate_limited(vip_user, "vip").await);

        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(!limiter.is_rate_limited(vip_user, "vip").await);
        assert!(limiter.is_rate_limited(premium_user, "premium").await);
        assert!(limiter.is_rate_limited(free_user, "free").await);

        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!limiter.is_rate_limited(premium_user, "premium").await);
        assert!(limiter.is_rate_limited(free_user, "free").await);

        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!limiter.is_rate_limited(free_user, "free").await);
    }

    #[tokio::test]
    async fn test_multiple_users_independent() {
        let limiter = RateLimiter::with_durations(
            Duration::from_millis(100),
            Duration::from_millis(50),
            Duration::from_millis(25),
        );

        let user1 = ChatId(2001);
        let user2 = ChatId(2002);

        limiter.update_rate_limit(user1, "free").await.unwrap();

        assert!(limiter.is_rate_limited(user1, "free").await);
        assert!(!limiter.is_rate_limited(user2, "free").await);
    }

    #[tokio::test]
    async fn test_clone_shares_state() {
        let limiter = RateLimiter::with_durations(
            Duration::from_secs(60),
            Duration::from_secs(30),
            Duration::from_secs(15),
        );
        let cloned = limiter.clone();
        let chat_id = ChatId(3001);

        limiter.update_rate_limit(chat_id, "free").await.unwrap();
        assert!(cloned.is_rate_limited(chat_id, "free").await);

        cloned.remove_rate_limit(chat_id).await.unwrap();
        assert!(!limiter.is_rate_limited(chat_id, "free").await);
    }

    #[tokio::test]
    async fn test_rate_limit_refresh() {
        let limiter = RateLimiter::with_durations(
            Duration::from_millis(100),
            Duration::from_millis(50),
            Duration::from_millis(25),
        );
        let chat_id = ChatId(4001);

        limiter.update_rate_limit(chat_id, "free").await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        limiter.update_rate_limit(chat_id, "free").await.unwrap();
        tokio::time::sleep(Duration::from_millis(60)).await;

        assert!(limiter.is_rate_limited(chat_id, "free").await);

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!limiter.is_rate_limited(chat_id, "free").await);
    }
}
