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
