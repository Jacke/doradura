use crate::core::metrics;
use crate::storage::db::{save_task_to_queue, DbPool};
use chrono::{DateTime, Utc};
use log::info; // Использование логирования вместо println
use std::collections::VecDeque;
use std::sync::Arc;
use teloxide::types::ChatId;
use tokio::sync::Mutex;

/// Приоритет задачи в очереди
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TaskPriority {
    /// Низкий приоритет (free пользователи)
    Low = 0,
    /// Средний приоритет (premium пользователи)
    Medium = 1,
    /// Высокий приоритет (vip пользователи)
    High = 2,
}

impl TaskPriority {
    /// Получает приоритет на основе плана пользователя
    pub fn from_plan(plan: &str) -> Self {
        match plan {
            "vip" => TaskPriority::High,
            "premium" => TaskPriority::Medium,
            _ => TaskPriority::Low, // По умолчанию free
        }
    }
}

/// Структура, представляющая задачу загрузки.
///
/// Содержит всю необходимую информацию для загрузки медиафайла:
/// URL источника, идентификатор пользователя, формат загрузки и время создания.
#[derive(Debug, Clone)]
pub struct DownloadTask {
    /// Уникальный идентификатор задачи (UUID)
    pub id: String,
    /// URL источника для загрузки
    pub url: String,
    /// ID чата пользователя в Telegram
    pub chat_id: ChatId,
    /// ID сообщения пользователя (для реакций)
    pub message_id: Option<i32>,
    /// Флаг, указывающий является ли задача загрузкой видео
    pub is_video: bool,
    /// Формат загрузки: "mp3", "mp4", "srt", "txt"
    pub format: String,
    /// Качество видео: "best", "1080p", "720p", "480p", "360p" (только для видео)
    pub video_quality: Option<String>,
    /// Битрейт аудио: "128k", "192k", "256k", "320k" (только для аудио)
    pub audio_bitrate: Option<String>,
    /// Временная метка создания задачи
    pub created_timestamp: DateTime<Utc>,
    /// Приоритет задачи (для приоритетной очереди)
    pub priority: TaskPriority,
}

impl DownloadTask {
    /// Создает новую задачу загрузки с уникальным ID
    ///
    /// # Arguments
    ///
    /// * `url` - URL для загрузки
    /// * `chat_id` - ID чата пользователя в Telegram
    /// * `message_id` - ID сообщения пользователя (опционально, для реакций)
    /// * `is_video` - Флаг, указывающий является ли это видео (true) или аудио (false)
    /// * `format` - Формат загрузки: "mp3", "mp4", "srt", "txt"
    /// * `video_quality` - Качество видео (опционально, только для видео)
    /// * `audio_bitrate` - Битрейт аудио (опционально, только для аудио)
    ///
    /// # Returns
    ///
    /// Возвращает новый экземпляр `DownloadTask` с автоматически сгенерированным UUID и текущим временем.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use teloxide::types::ChatId;
    /// use doradura::download::queue::DownloadTask;
    ///
    /// let task = DownloadTask::new(
    ///     "https://youtube.com/watch?v=...".to_string(),
    ///     ChatId(123456789),
    ///     Some(12345),
    ///     false,
    ///     "mp3".to_string(),
    ///     None,
    ///     Some("320k".to_string())
    /// );
    /// ```
    pub fn new(
        url: String,
        chat_id: ChatId,
        message_id: Option<i32>,
        is_video: bool,
        format: String,
        video_quality: Option<String>,
        audio_bitrate: Option<String>,
    ) -> Self {
        Self::with_priority(
            url,
            chat_id,
            message_id,
            is_video,
            format,
            video_quality,
            audio_bitrate,
            TaskPriority::Low,
        )
    }

    /// Создает новую задачу с указанным приоритетом
    pub fn with_priority(
        url: String,
        chat_id: ChatId,
        message_id: Option<i32>,
        is_video: bool,
        format: String,
        video_quality: Option<String>,
        audio_bitrate: Option<String>,
        priority: TaskPriority,
    ) -> Self {
        let id = uuid::Uuid::new_v4().to_string();
        Self {
            id,
            url,
            chat_id,
            message_id,
            is_video,
            format,
            video_quality,
            audio_bitrate,
            created_timestamp: Utc::now(),
            priority,
        }
    }

    /// Создает новую задачу на основе плана пользователя
    pub fn from_plan(
        url: String,
        chat_id: ChatId,
        message_id: Option<i32>,
        is_video: bool,
        format: String,
        video_quality: Option<String>,
        audio_bitrate: Option<String>,
        plan: &str,
    ) -> Self {
        let priority = TaskPriority::from_plan(plan);
        Self::with_priority(
            url,
            chat_id,
            message_id,
            is_video,
            format,
            video_quality,
            audio_bitrate,
            priority,
        )
    }
}

/// Очередь для задач загрузки с потокобезопасной реализацией.
///
/// Использует `Mutex` для синхронизации доступа к внутренней очереди.
/// Задачи обрабатываются с учетом приоритета: сначала высокий, затем средний, затем низкий.
/// Внутри каждого приоритета задачи обрабатываются в порядке FIFO (First In, First Out).
pub struct DownloadQueue {
    /// Внутренняя очередь задач, защищенная мьютексом
    /// Задачи хранятся в порядке приоритета: High -> Medium -> Low
    pub queue: Mutex<VecDeque<DownloadTask>>,
}

impl Default for DownloadQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl DownloadQueue {
    /// Создает новую пустую очередь.
    ///
    /// # Returns
    ///
    /// Возвращает новый экземпляр `DownloadQueue` с пустой внутренней очередью.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use doradura::download::queue::DownloadQueue;
    ///
    /// let queue = DownloadQueue::new();
    /// ```
    pub fn new() -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
        }
    }

    /// Добавляет задачу в очередь с учетом приоритета.
    ///
    /// Задачи с высоким приоритетом добавляются в начало соответствующей секции,
    /// задачи с низким приоритетом - в конец.
    ///
    /// # Arguments
    ///
    /// * `task` - Задача для добавления в очередь
    /// * `db_pool` - Опциональный пул соединений с БД для сохранения задачи
    ///
    /// # Example
    ///
    /// ```no_run
    /// use teloxide::types::ChatId;
    /// use doradura::download::queue::{DownloadQueue, DownloadTask};
    ///
    /// # async fn example() {
    /// let queue = DownloadQueue::new();
    /// let task = DownloadTask::new(
    ///     "https://youtube.com/watch?v=...".to_string(),
    ///     ChatId(123456789),
    ///     Some(12345),
    ///     false,
    ///     "mp3".to_string(),
    ///     None,
    ///     Some("320k".to_string())
    /// );
    /// queue.add_task(task, None).await;
    /// # }
    /// ```
    pub async fn add_task(&self, task: DownloadTask, db_pool: Option<Arc<DbPool>>) {
        info!("Добавляем задачу с приоритетом {:?}: {:?}", task.priority, task);

        // Сохраняем задачу в БД для гарантированной обработки
        if let Some(ref pool) = db_pool {
            if let Ok(conn) = crate::storage::db::get_connection(pool) {
                let priority_value = task.priority as i32;
                if let Err(e) = save_task_to_queue(
                    &conn,
                    &task.id,
                    task.chat_id.0,
                    &task.url,
                    &task.format,
                    task.is_video,
                    task.video_quality.as_deref(),
                    task.audio_bitrate.as_deref(),
                    priority_value,
                ) {
                    log::error!("Failed to save task {} to database: {}", task.id, e);
                } else {
                    log::debug!("Task {} saved to database", task.id);
                }
            }
        }

        let mut queue = self.queue.lock().await;

        // Находим позицию для вставки с учетом приоритета
        let insert_pos = queue
            .iter()
            .position(|t| t.priority < task.priority)
            .unwrap_or(queue.len());

        // Вставляем задачу в нужную позицию
        let mut new_queue = VecDeque::new();
        let mut inserted = false;

        for (idx, existing_task) in queue.iter().enumerate() {
            if idx == insert_pos && !inserted {
                new_queue.push_back(task.clone());
                inserted = true;
            }
            new_queue.push_back(existing_task.clone());
        }

        if !inserted {
            new_queue.push_back(task);
        }

        *queue = new_queue;

        // Update queue depth metrics by priority
        let low_count = queue.iter().filter(|t| t.priority == TaskPriority::Low).count();
        let medium_count = queue.iter().filter(|t| t.priority == TaskPriority::Medium).count();
        let high_count = queue.iter().filter(|t| t.priority == TaskPriority::High).count();

        metrics::update_queue_depth("low", low_count);
        metrics::update_queue_depth("medium", medium_count);
        metrics::update_queue_depth("high", high_count);
        metrics::update_queue_depth_total(queue.len());
    }

    /// Извлекает и возвращает первую задачу из очереди (с учетом приоритета).
    ///
    /// Задачи с высоким приоритетом обрабатываются первыми.
    ///
    /// # Returns
    ///
    /// Возвращает `Some(DownloadTask)` если очередь не пуста, иначе `None`.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use doradura::download::queue::DownloadQueue;
    ///
    /// # async fn example() {
    /// let queue = DownloadQueue::new();
    /// // ... добавить задачи ...
    /// if let Some(task) = queue.get_task().await {
    ///     // Обработать задачу
    /// }
    /// # }
    /// ```
    pub async fn get_task(&self) -> Option<DownloadTask> {
        let mut queue = self.queue.lock().await;
        if !queue.is_empty() {
            info!(
                "Получаем задачу из очереди, размер: {}, приоритет: {:?}",
                queue.len(),
                queue.front().map(|t| t.priority)
            );
        }
        let task = queue.pop_front();

        // Update queue depth metrics after removing task
        if task.is_some() {
            let low_count = queue.iter().filter(|t| t.priority == TaskPriority::Low).count();
            let medium_count = queue.iter().filter(|t| t.priority == TaskPriority::Medium).count();
            let high_count = queue.iter().filter(|t| t.priority == TaskPriority::High).count();

            metrics::update_queue_depth("low", low_count);
            metrics::update_queue_depth("medium", medium_count);
            metrics::update_queue_depth("high", high_count);
            metrics::update_queue_depth_total(queue.len());
        }

        task
    }

    /// Получает позицию задачи пользователя в очереди
    ///
    /// # Arguments
    ///
    /// * `chat_id` - ID чата пользователя
    ///
    /// # Returns
    ///
    /// Возвращает позицию в очереди (1-based) или None если задача не найдена
    pub async fn get_queue_position(&self, chat_id: ChatId) -> Option<usize> {
        let queue = self.queue.lock().await;
        queue.iter().position(|task| task.chat_id == chat_id).map(|pos| pos + 1)
    }

    /// Возвращает текущее количество задач в очереди.
    ///
    /// # Returns
    ///
    /// Количество задач в очереди.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use doradura::download::queue::DownloadQueue;
    ///
    /// # async fn example() {
    /// let queue = DownloadQueue::new();
    /// // ... добавить задачи ...
    /// let count = queue.size().await;
    /// println!("Задач в очереди: {}", count);
    /// # }
    /// ```
    pub async fn size(&self) -> usize {
        let queue = self.queue.lock().await;
        queue.len()
    }

    /// Фильтрует задачи по chat_id и возвращает список задач для указанного пользователя.
    ///
    /// # Arguments
    ///
    /// * `chat_id` - ID чата пользователя для фильтрации
    ///
    /// # Returns
    ///
    /// Вектор всех задач, принадлежащих указанному пользователю.
    ///
    /// # Note
    ///
    /// Задачи не удаляются из очереди, возвращаются только их копии.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use teloxide::types::ChatId;
    /// use doradura::download::queue::DownloadQueue;
    ///
    /// # async fn example() {
    /// let queue = DownloadQueue::new();
    /// let user_tasks = queue.filter_tasks_by_chat_id(ChatId(123456789)).await;
    /// # }
    /// ```
    pub async fn filter_tasks_by_chat_id(&self, chat_id: ChatId) -> Vec<DownloadTask> {
        let queue = self.queue.lock().await;
        queue.iter().filter(|task| task.chat_id == chat_id).cloned().collect()
    }

    /// Удаляет задачи, которые старше заданного временного порога.
    ///
    /// # Arguments
    ///
    /// * `max_age` - Максимальный возраст задачи (задачи старше этого возраста будут удалены)
    ///
    /// # Returns
    ///
    /// Количество удаленных задач.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use chrono::Duration;
    /// use doradura::download::queue::DownloadQueue;
    ///
    /// # async fn example() {
    /// let queue = DownloadQueue::new();
    /// // Удалить задачи старше 1 дня
    /// let removed = queue.remove_old_tasks(Duration::days(1)).await;
    /// println!("Удалено старых задач: {}", removed);
    /// # }
    /// ```
    pub async fn remove_old_tasks(&self, max_age: chrono::Duration) -> usize {
        let mut queue = self.queue.lock().await;
        let before = queue.len();
        queue.retain(|task| Utc::now() - task.created_timestamp < max_age);
        let removed_count = before - queue.len();
        info!("Удалено старых задач: {}", removed_count);
        removed_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    // ==================== TaskPriority Tests ====================

    #[test]
    fn test_task_priority_from_plan() {
        assert_eq!(TaskPriority::from_plan("vip"), TaskPriority::High);
        assert_eq!(TaskPriority::from_plan("premium"), TaskPriority::Medium);
        assert_eq!(TaskPriority::from_plan("free"), TaskPriority::Low);
        assert_eq!(TaskPriority::from_plan("unknown"), TaskPriority::Low);
    }

    #[test]
    fn test_task_priority_ordering() {
        assert!(TaskPriority::High > TaskPriority::Medium);
        assert!(TaskPriority::Medium > TaskPriority::Low);
        assert!(TaskPriority::High > TaskPriority::Low);
    }

    // ==================== DownloadTask Tests ====================

    #[test]
    fn test_download_task_new() {
        let task = DownloadTask::new(
            "http://example.com".to_string(),
            ChatId(123),
            Some(456),
            false,
            "mp3".to_string(),
            None,
            Some("320k".to_string()),
        );
        assert!(!task.id.is_empty());
        assert_eq!(task.url, "http://example.com");
        assert_eq!(task.chat_id, ChatId(123));
        assert_eq!(task.message_id, Some(456));
        assert!(!task.is_video);
        assert_eq!(task.format, "mp3");
        assert_eq!(task.video_quality, None);
        assert_eq!(task.audio_bitrate, Some("320k".to_string()));
        assert_eq!(task.priority, TaskPriority::Low);
    }

    #[test]
    fn test_download_task_with_priority() {
        let task = DownloadTask::with_priority(
            "http://example.com".to_string(),
            ChatId(123),
            None,
            true,
            "mp4".to_string(),
            Some("1080p".to_string()),
            None,
            TaskPriority::High,
        );
        assert_eq!(task.priority, TaskPriority::High);
        assert!(task.is_video);
        assert_eq!(task.video_quality, Some("1080p".to_string()));
    }

    #[test]
    fn test_download_task_from_plan() {
        let vip_task = DownloadTask::from_plan(
            "http://example.com".to_string(),
            ChatId(123),
            None,
            false,
            "mp3".to_string(),
            None,
            None,
            "vip",
        );
        assert_eq!(vip_task.priority, TaskPriority::High);

        let premium_task = DownloadTask::from_plan(
            "http://example.com".to_string(),
            ChatId(123),
            None,
            false,
            "mp3".to_string(),
            None,
            None,
            "premium",
        );
        assert_eq!(premium_task.priority, TaskPriority::Medium);

        let free_task = DownloadTask::from_plan(
            "http://example.com".to_string(),
            ChatId(123),
            None,
            false,
            "mp3".to_string(),
            None,
            None,
            "free",
        );
        assert_eq!(free_task.priority, TaskPriority::Low);
    }

    // ==================== DownloadQueue Tests ====================

    #[test]
    fn test_download_queue_default() {
        let queue = DownloadQueue::default();
        // Verify the queue is created with default trait
        assert!(matches!(queue, DownloadQueue { .. }));
    }

    #[tokio::test]
    async fn test_add_and_get_task() {
        let queue = DownloadQueue::new();
        let task = DownloadTask::new(
            "http://example.com".to_string(),
            ChatId(123),
            Some(12345),
            false,
            "mp3".to_string(),
            None,
            Some("320k".to_string()),
        );

        queue.add_task(task.clone(), None).await;
        assert_eq!(queue.size().await, 1);

        let fetched_task = queue
            .get_task()
            .await
            .expect("Should retrieve task that was just added");
        assert_eq!(fetched_task.url, task.url);
    }

    #[tokio::test]
    async fn test_queue_empty() {
        let queue = DownloadQueue::new();
        assert_eq!(queue.size().await, 0);
        assert!(queue.get_task().await.is_none());
    }

    #[tokio::test]
    async fn test_priority_ordering() {
        let queue = DownloadQueue::new();

        // Add low priority task first
        let low_task = DownloadTask::from_plan(
            "http://low.com".to_string(),
            ChatId(1),
            None,
            false,
            "mp3".to_string(),
            None,
            None,
            "free",
        );
        queue.add_task(low_task, None).await;

        // Add high priority task second
        let high_task = DownloadTask::from_plan(
            "http://high.com".to_string(),
            ChatId(2),
            None,
            false,
            "mp3".to_string(),
            None,
            None,
            "vip",
        );
        queue.add_task(high_task, None).await;

        // High priority should come out first
        let first = queue.get_task().await.unwrap();
        assert_eq!(first.url, "http://high.com");
        assert_eq!(first.priority, TaskPriority::High);

        let second = queue.get_task().await.unwrap();
        assert_eq!(second.url, "http://low.com");
        assert_eq!(second.priority, TaskPriority::Low);
    }

    #[tokio::test]
    async fn test_filter_tasks_by_chat_id() {
        let queue = DownloadQueue::new();

        let task1 = DownloadTask::new(
            "http://example1.com".to_string(),
            ChatId(100),
            None,
            false,
            "mp3".to_string(),
            None,
            None,
        );
        let task2 = DownloadTask::new(
            "http://example2.com".to_string(),
            ChatId(200),
            None,
            false,
            "mp3".to_string(),
            None,
            None,
        );
        let task3 = DownloadTask::new(
            "http://example3.com".to_string(),
            ChatId(100),
            None,
            false,
            "mp3".to_string(),
            None,
            None,
        );

        queue.add_task(task1, None).await;
        queue.add_task(task2, None).await;
        queue.add_task(task3, None).await;

        let user_100_tasks = queue.filter_tasks_by_chat_id(ChatId(100)).await;
        assert_eq!(user_100_tasks.len(), 2);

        let user_200_tasks = queue.filter_tasks_by_chat_id(ChatId(200)).await;
        assert_eq!(user_200_tasks.len(), 1);

        let user_999_tasks = queue.filter_tasks_by_chat_id(ChatId(999)).await;
        assert_eq!(user_999_tasks.len(), 0);
    }

    #[tokio::test]
    async fn test_get_queue_position() {
        let queue = DownloadQueue::new();

        let task1 = DownloadTask::new(
            "http://example1.com".to_string(),
            ChatId(100),
            None,
            false,
            "mp3".to_string(),
            None,
            None,
        );
        let task2 = DownloadTask::new(
            "http://example2.com".to_string(),
            ChatId(200),
            None,
            false,
            "mp3".to_string(),
            None,
            None,
        );

        queue.add_task(task1, None).await;
        queue.add_task(task2, None).await;

        // Positions are 1-based
        assert_eq!(queue.get_queue_position(ChatId(100)).await, Some(1));
        assert_eq!(queue.get_queue_position(ChatId(200)).await, Some(2));
        assert_eq!(queue.get_queue_position(ChatId(999)).await, None);
    }

    #[tokio::test]
    async fn test_remove_old_tasks() {
        let queue = DownloadQueue::new();
        // Create old task manually with custom timestamp
        let old_task = DownloadTask {
            id: uuid::Uuid::new_v4().to_string(),
            url: "http://example.com/old".to_string(),
            chat_id: ChatId(123),
            message_id: Some(11111),
            is_video: false,
            format: "mp3".to_string(),
            video_quality: None,
            audio_bitrate: Some("320k".to_string()),
            created_timestamp: Utc::now() - Duration::days(2),
            priority: TaskPriority::Low,
        };
        let new_task = DownloadTask::new(
            "http://example.com/new".to_string(),
            ChatId(456),
            Some(22222),
            true,
            "mp4".to_string(),
            Some("1080p".to_string()),
            None,
        );

        queue.add_task(old_task, None).await;
        queue.add_task(new_task, None).await;

        let removed = queue.remove_old_tasks(Duration::days(1)).await;
        assert_eq!(removed, 1);
        assert_eq!(queue.size().await, 1);
    }

    #[tokio::test]
    async fn test_remove_old_tasks_all_new() {
        let queue = DownloadQueue::new();

        let task = DownloadTask::new(
            "http://example.com".to_string(),
            ChatId(123),
            None,
            false,
            "mp3".to_string(),
            None,
            None,
        );
        queue.add_task(task, None).await;

        let removed = queue.remove_old_tasks(Duration::days(1)).await;
        assert_eq!(removed, 0);
        assert_eq!(queue.size().await, 1);
    }
}
