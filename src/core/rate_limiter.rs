use std::collections::HashMap;
use std::sync::Arc;
use teloxide::types::ChatId;
use tokio::sync::Mutex;
use tokio::time::{Duration, Instant};

/// Rate limiter that throttles how often users can send requests.
///
/// Applies different limits per subscription plan.
#[derive(Clone)]
pub struct RateLimiter {
    /// Timestamps of the last request for each user
    limits: Arc<Mutex<HashMap<ChatId, Instant>>>,
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
    /// Creates a rate limiter with default limits per plan.
    ///
    /// Returns a `RateLimiter` configured as:
    /// - Free: 30 seconds between requests
    /// - Premium: 10 seconds between requests
    /// - VIP: 5 seconds between requests
    pub fn new() -> Self {
        Self {
            limits: Arc::new(Mutex::new(HashMap::new())),
            free_duration: Duration::from_secs(30),
            premium_duration: Duration::from_secs(10),
            vip_duration: Duration::from_secs(5),
        }
    }

    /// Creates a rate limiter with custom limits.
    ///
    /// * `free_duration` - Delay between requests for the free plan
    /// * `premium_duration` - Delay between requests for the premium plan
    /// * `vip_duration` - Delay between requests for the VIP plan
    pub fn with_durations(free_duration: Duration, premium_duration: Duration, vip_duration: Duration) -> Self {
        Self {
            limits: Arc::new(Mutex::new(HashMap::new())),
            free_duration,
            premium_duration,
            vip_duration,
        }
    }

    /// Gets the throttle duration for the given plan.
    fn get_duration_for_plan(&self, plan: &str) -> Duration {
        match plan {
            "premium" => self.premium_duration,
            "vip" => self.vip_duration,
            _ => self.free_duration, // default to free
        }
    }

    /// Checks whether a user is currently rate-limited.
    ///
    /// * `chat_id` - User chat ID
    /// * `plan` - User plan ("free", "premium", "vip")
    ///
    /// Returns `true` if limited, otherwise `false`.
    pub async fn is_rate_limited(&self, chat_id: ChatId, _plan: &str) -> bool {
        let limits = self.limits.lock().await;
        if let Some(&instant) = limits.get(&chat_id) {
            if Instant::now() < instant {
                return true;
            }
        }
        false
    }

    /// Returns remaining time until the user is unlocked.
    ///
    /// * `chat_id` - User chat ID
    ///
    /// Returns `Some(Duration)` if limited, otherwise `None`.
    pub async fn get_remaining_time(&self, chat_id: ChatId) -> Option<Duration> {
        let limits = self.limits.lock().await;
        if let Some(&instant) = limits.get(&chat_id) {
            let now = Instant::now();
            if now < instant {
                return Some(instant - now);
            }
        }
        None
    }

    /// Updates the last-request timestamp for a user.
    ///
    /// Call after a successful request to set the new cooldown.
    ///
    /// * `chat_id` - User chat ID
    /// * `plan` - User plan ("free", "premium", "vip")
    pub async fn update_rate_limit(&self, chat_id: ChatId, plan: &str) {
        let mut limits = self.limits.lock().await;
        let duration = self.get_duration_for_plan(plan);
        limits.insert(chat_id, Instant::now() + duration);
    }

    /// Removes the limit for the given user.
    ///
    /// Useful for admin actions or manual resets.
    ///
    /// * `chat_id` - User chat ID
    pub async fn remove_rate_limit(&self, chat_id: ChatId) {
        let mut limits = self.limits.lock().await;
        limits.remove(&chat_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_rate_limiter_new() {
        let limiter = RateLimiter::new();

        // Verify default durations
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
        // Unknown plan defaults to free
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
    async fn test_update_and_check_rate_limit() {
        let limiter = RateLimiter::with_durations(
            Duration::from_millis(100), // Short duration for testing
            Duration::from_millis(50),
            Duration::from_millis(25),
        );
        let chat_id = ChatId(12346);

        // Update rate limit
        limiter.update_rate_limit(chat_id, "free").await;

        // Should be rate limited now
        let is_limited = limiter.is_rate_limited(chat_id, "free").await;
        assert!(is_limited);

        // Wait for limit to expire
        tokio::time::sleep(Duration::from_millis(150)).await;

        // Should not be rate limited anymore
        let is_limited = limiter.is_rate_limited(chat_id, "free").await;
        assert!(!is_limited);
    }

    #[tokio::test]
    async fn test_get_remaining_time() {
        let limiter = RateLimiter::with_durations(
            Duration::from_millis(200),
            Duration::from_millis(100),
            Duration::from_millis(50),
        );
        let chat_id = ChatId(12347);

        // No limit set - should return None
        let remaining = limiter.get_remaining_time(chat_id).await;
        assert!(remaining.is_none());

        // Set limit
        limiter.update_rate_limit(chat_id, "free").await;

        // Should have remaining time
        let remaining = limiter.get_remaining_time(chat_id).await;
        assert!(remaining.is_some());
        let remaining = remaining.unwrap();
        assert!(remaining.as_millis() > 0);
        assert!(remaining.as_millis() <= 200);

        // Wait for limit to expire
        tokio::time::sleep(Duration::from_millis(250)).await;

        // Should return None after expiry
        let remaining = limiter.get_remaining_time(chat_id).await;
        assert!(remaining.is_none());
    }

    #[tokio::test]
    async fn test_remove_rate_limit() {
        let limiter = RateLimiter::with_durations(
            Duration::from_secs(60), // Long duration
            Duration::from_secs(30),
            Duration::from_secs(15),
        );
        let chat_id = ChatId(12348);

        // Set limit
        limiter.update_rate_limit(chat_id, "free").await;

        // Verify rate limited
        let is_limited = limiter.is_rate_limited(chat_id, "free").await;
        assert!(is_limited);

        // Remove limit
        limiter.remove_rate_limit(chat_id).await;

        // Should not be rate limited anymore
        let is_limited = limiter.is_rate_limited(chat_id, "free").await;
        assert!(!is_limited);
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

        // Set limits for each user with their respective plans
        limiter.update_rate_limit(free_user, "free").await;
        limiter.update_rate_limit(premium_user, "premium").await;
        limiter.update_rate_limit(vip_user, "vip").await;

        // All should be rate limited
        assert!(limiter.is_rate_limited(free_user, "free").await);
        assert!(limiter.is_rate_limited(premium_user, "premium").await);
        assert!(limiter.is_rate_limited(vip_user, "vip").await);

        // Wait for VIP to expire
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(!limiter.is_rate_limited(vip_user, "vip").await);
        assert!(limiter.is_rate_limited(premium_user, "premium").await);
        assert!(limiter.is_rate_limited(free_user, "free").await);

        // Wait for premium to expire
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!limiter.is_rate_limited(premium_user, "premium").await);
        assert!(limiter.is_rate_limited(free_user, "free").await);

        // Wait for free to expire
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

        // Set limit for user1 only
        limiter.update_rate_limit(user1, "free").await;

        // User1 should be limited, user2 should not
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

        // Set limit on original
        limiter.update_rate_limit(chat_id, "free").await;

        // Should be visible on clone (Arc shared state)
        assert!(cloned.is_rate_limited(chat_id, "free").await);

        // Remove on clone
        cloned.remove_rate_limit(chat_id).await;

        // Should be removed on original too
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

        // Set initial limit
        limiter.update_rate_limit(chat_id, "free").await;

        // Wait a bit
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Refresh limit (should reset the timer)
        limiter.update_rate_limit(chat_id, "free").await;

        // Wait the original duration - should still be limited due to refresh
        tokio::time::sleep(Duration::from_millis(60)).await;

        // Should still be limited because we refreshed
        assert!(limiter.is_rate_limited(chat_id, "free").await);

        // Wait for the new limit to expire
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Now should be unlocked
        assert!(!limiter.is_rate_limited(chat_id, "free").await);
    }
}
