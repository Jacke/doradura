use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{Duration, Instant};
use teloxide::types::ChatId;

/// Rate limiter для ограничения частоты запросов пользователей.
/// 
/// Ограничивает количество запросов от каждого пользователя в течение определенного периода времени.
/// Использует токен-бакет алгоритм с периодическим сбросом лимита.
#[derive(Clone)]
pub struct RateLimiter {
    /// Хранилище временных меток последнего запроса для каждого пользователя
    limits: Arc<Mutex<HashMap<ChatId, Instant>>>,
    /// Продолжительность периода ограничения
    duration: Duration,
}

impl RateLimiter {
    /// Создает новый rate limiter с указанной продолжительностью ограничения.
    /// 
    /// # Arguments
    /// 
    /// * `duration` - Время, которое должно пройти между запросами от одного пользователя
    /// 
    /// # Returns
    /// 
    /// Новый экземпляр `RateLimiter`.
    /// 
    /// # Example
    /// 
    /// ```no_run
    /// use std::time::Duration;
    /// use doradura::rate_limiter::RateLimiter;
    /// 
    /// let limiter = RateLimiter::new(Duration::from_secs(30));
    /// ```
    pub fn new(duration: Duration) -> Self {
        Self {
            limits: Arc::new(Mutex::new(HashMap::new())),
            duration,
        }
    }

    /// Проверяет, ограничен ли пользователь по частоте запросов.
    /// 
    /// # Arguments
    /// 
    /// * `chat_id` - ID чата пользователя для проверки
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
    /// use std::time::Duration;
    /// 
    /// # async fn example() {
    /// let limiter = RateLimiter::new(Duration::from_secs(30));
    /// if limiter.is_rate_limited(ChatId(123456789)).await {
    ///     println!("Пользователь ограничен");
    /// }
    /// # }
    /// ```
    pub async fn is_rate_limited(&self, chat_id: ChatId) -> bool {
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
    /// use std::time::Duration;
    /// 
    /// # async fn example() {
    /// let limiter = RateLimiter::new(Duration::from_secs(30));
    /// if let Some(remaining) = limiter.get_remaining_time(ChatId(123456789)).await {
    ///     println!("Осталось ждать: {:?}", remaining);
    /// }
    /// # }
    /// ```
    pub async fn get_remaining_time(&self, chat_id: ChatId) -> Option<Duration> {
        let limits = self.limits.lock().await;
        if let Some(&instant) = limits.get(&chat_id) {
            if Instant::now() < instant {
                return Some(instant - Instant::now());
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
    /// // После успешной обработки запроса
    /// limiter.update_rate_limit(ChatId(123456789)).await;
    /// # }
    /// ```
    pub async fn update_rate_limit(&self, chat_id: ChatId) {
        let mut limits = self.limits.lock().await;
        limits.insert(chat_id, Instant::now() + self.duration);
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
