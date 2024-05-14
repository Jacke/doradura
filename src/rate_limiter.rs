use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{Duration, Instant};
use teloxide::types::ChatId;

#[derive(Clone)]
pub struct RateLimiter {
    limits: Arc<Mutex<HashMap<ChatId, Instant>>>,
    duration: Duration,
}

impl RateLimiter {
    pub fn new(duration: Duration) -> Self {
        Self {
            limits: Arc::new(Mutex::new(HashMap::new())),
            duration,
        }
    }

    pub async fn is_rate_limited(&self, chat_id: ChatId) -> bool {
        let limits = self.limits.lock().await;
        if let Some(&instant) = limits.get(&chat_id) {
            if Instant::now() < instant {
                return true;
            }
        }
        false
    }

    pub async fn get_remaining_time(&self, chat_id: ChatId) -> Option<Duration> {
        let limits = self.limits.lock().await;
        if let Some(&instant) = limits.get(&chat_id) {
            if Instant::now() < instant {
                return Some(instant - Instant::now());
            }
        }
        None
    }

    pub async fn update_rate_limit(&self, chat_id: ChatId) {
        let mut limits = self.limits.lock().await;
        limits.insert(chat_id, Instant::now() + self.duration);
    }
}
