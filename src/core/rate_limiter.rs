use std::collections::HashMap;
use std::sync::Arc;
use teloxide::types::ChatId;
use tokio::sync::Mutex;
use tokio::time::{Duration, Instant};

/// Rate limiter для ограничения частоты запросов пользователей.
///
/// Ограничивает количество запросов от каждого пользователя в течение определенного периода времени.
/// Использует разные лимиты для разных планов подписки.
#[derive(Clone)]
pub struct RateLimiter {
    /// Хранилище временных меток последнего запроса для каждого пользователя
    limits: Arc<Mutex<HashMap<ChatId, Instant>>>,
    /// Базовое время между запросами для free плана
    free_duration: Duration,
    /// Время между запросами для premium плана
    premium_duration: Duration,
    /// Время между запросами для vip плана
    vip_duration: Duration,
}

impl RateLimiter {
    /// Создает новый rate limiter с разными лимитами для разных планов.
    ///
    /// # Returns
    ///
    /// Новый экземпляр `RateLimiter` с настройками:
    /// - Free: 30 секунд между запросами
    /// - Premium: 10 секунд между запросами
    /// - VIP: 5 секунд между запросами
    ///
    /// # Example
    ///
    /// ```no_run
    /// use doradura::rate_limiter::RateLimiter;
    ///
    /// let limiter = RateLimiter::new();
    /// ```
    pub fn new() -> Self {
        Self {
            limits: Arc::new(Mutex::new(HashMap::new())),
            free_duration: Duration::from_secs(30),
            premium_duration: Duration::from_secs(10),
            vip_duration: Duration::from_secs(5),
        }
    }

    /// Создает новый rate limiter с кастомными лимитами.
    ///
    /// # Arguments
    ///
    /// * `free_duration` - Время между запросами для free плана
    /// * `premium_duration` - Время между запросами для premium плана
    /// * `vip_duration` - Время между запросами для vip плана
    pub fn with_durations(
        free_duration: Duration,
        premium_duration: Duration,
        vip_duration: Duration,
    ) -> Self {
        Self {
            limits: Arc::new(Mutex::new(HashMap::new())),
            free_duration,
            premium_duration,
            vip_duration,
        }
    }

    /// Получает длительность ограничения для указанного плана.
    fn get_duration_for_plan(&self, plan: &str) -> Duration {
        match plan {
            "premium" => self.premium_duration,
            "vip" => self.vip_duration,
            _ => self.free_duration, // По умолчанию free
        }
    }

    /// Проверяет, ограничен ли пользователь по частоте запросов.
    ///
    /// # Arguments
    ///
    /// * `chat_id` - ID чата пользователя для проверки
    /// * `plan` - План пользователя ("free", "premium", "vip")
    ///
    /// # Returns
    ///
    /// Возвращает `true` если пользователь все еще ограничен, `false` если может сделать запрос.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use teloxide::types::ChatId;
    /// use doradura::rate_limiter::RateLimiter;
    ///
    /// # async fn example() {
    /// let limiter = RateLimiter::new();
    /// if limiter.is_rate_limited(ChatId(123456789), "free").await {
    ///     println!("Пользователь ограничен");
    /// }
    /// # }
    /// ```
    pub async fn is_rate_limited(&self, chat_id: ChatId, _plan: &str) -> bool {
        let limits = self.limits.lock().await;
        if let Some(&instant) = limits.get(&chat_id) {
            if Instant::now() < instant {
                return true;
            }
        }
        false
    }

    /// Получает оставшееся время до снятия ограничения для пользователя.
    ///
    /// # Arguments
    ///
    /// * `chat_id` - ID чата пользователя
    ///
    /// # Returns
    ///
    /// Возвращает `Some(Duration)` если пользователь ограничен, иначе `None`.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use teloxide::types::ChatId;
    /// use doradura::rate_limiter::RateLimiter;
    ///
    /// # async fn example() {
    /// let limiter = RateLimiter::new();
    /// if let Some(remaining) = limiter.get_remaining_time(ChatId(123456789)).await {
    ///     println!("Осталось ждать: {:?}", remaining);
    /// }
    /// # }
    /// ```
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

    /// Обновляет временную метку последнего запроса пользователя.
    ///
    /// Вызывается после успешного запроса для установки нового периода ограничения.
    ///
    /// # Arguments
    ///
    /// * `chat_id` - ID чата пользователя
    /// * `plan` - План пользователя ("free", "premium", "vip")
    ///
    /// # Example
    ///
    /// ```no_run
    /// use teloxide::types::ChatId;
    /// use doradura::rate_limiter::RateLimiter;
    ///
    /// # async fn example() {
    /// let limiter = RateLimiter::new();
    /// // После успешной обработки запроса
    /// limiter.update_rate_limit(ChatId(123456789), "free").await;
    /// # }
    /// ```
    pub async fn update_rate_limit(&self, chat_id: ChatId, plan: &str) {
        let mut limits = self.limits.lock().await;
        let duration = self.get_duration_for_plan(plan);
        limits.insert(chat_id, Instant::now() + duration);
    }

    /// Удаляет ограничение для указанного пользователя.
    ///
    /// Полезно для административных действий или сброса ограничений.
    ///
    /// # Arguments
    ///
    /// * `chat_id` - ID чата пользователя
    ///
    /// # Example
    ///
    /// ```no_run
    /// use teloxide::types::ChatId;
    /// use doradura::rate_limiter::RateLimiter;
    /// use std::time::Duration;
    ///
    /// # async fn example() {
    /// let limiter = RateLimiter::new(Duration::from_secs(30));
    /// // Снять ограничение для пользователя
    /// limiter.remove_rate_limit(ChatId(123456789)).await;
    /// # }
    /// ```
    pub async fn remove_rate_limit(&self, chat_id: ChatId) {
        let mut limits = self.limits.lock().await;
        limits.remove(&chat_id);
    }
}
