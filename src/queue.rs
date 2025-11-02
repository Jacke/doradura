use std::collections::VecDeque;
use tokio::sync::Mutex;
use teloxide::types::ChatId;
use chrono::{DateTime, Utc};
use log::info; // Использование логирования вместо println

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
}

impl DownloadTask {
    /// Создает новую задачу загрузки с уникальным ID
    /// 
    /// # Arguments
    /// 
    /// * `url` - URL для загрузки
    /// * `chat_id` - ID чата пользователя в Telegram
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
    /// use doradura::queue::DownloadTask;
    /// 
    /// let task = DownloadTask::new(
    ///     "https://youtube.com/watch?v=...".to_string(),
    ///     ChatId(123456789),
    ///     false,
    ///     "mp3".to_string(),
    ///     None,
    ///     Some("320k".to_string())
    /// );
    /// ```
    pub fn new(url: String, chat_id: ChatId, is_video: bool, format: String, video_quality: Option<String>, audio_bitrate: Option<String>) -> Self {
        let id = uuid::Uuid::new_v4().to_string();
        Self {
            id,
            url,
            chat_id,
            is_video,
            format,
            video_quality,
            audio_bitrate,
            created_timestamp: Utc::now(),
        }
    }
}

/// Очередь для задач загрузки с потокобезопасной реализацией.
/// 
/// Использует `Mutex` для синхронизации доступа к внутренней очереди.
/// Задачи обрабатываются в порядке FIFO (First In, First Out).
pub struct DownloadQueue {
    /// Внутренняя очередь задач, защищенная мьютексом
    pub queue: Mutex<VecDeque<DownloadTask>>,
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
    /// use doradura::queue::DownloadQueue;
    /// 
    /// let queue = DownloadQueue::new();
    /// ```
    pub fn new() -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
        }
    }

    /// Добавляет задачу в конец очереди.
    /// 
    /// # Arguments
    /// 
    /// * `task` - Задача для добавления в очередь
    /// 
    /// # Example
    /// 
    /// ```no_run
    /// use teloxide::types::ChatId;
    /// use doradura::queue::{DownloadQueue, DownloadTask};
    /// 
    /// # async fn example() {
    /// let queue = DownloadQueue::new();
    /// let task = DownloadTask::new(
    ///     "https://youtube.com/watch?v=...".to_string(),
    ///     ChatId(123456789),
    ///     false,
    ///     "mp3".to_string()
    /// );
    /// queue.add_task(task).await;
    /// # }
    /// ```
    pub async fn add_task(&self, task: DownloadTask) {
        info!("Добавляем задачу: {:?}", task);
        let mut queue = self.queue.lock().await;
        queue.push_back(task);
    }

    /// Извлекает и возвращает первую задачу из очереди (FIFO).
    /// 
    /// # Returns
    /// 
    /// Возвращает `Some(DownloadTask)` если очередь не пуста, иначе `None`.
    /// 
    /// # Example
    /// 
    /// ```no_run
    /// use doradura::queue::DownloadQueue;
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
        if queue.len() != 0 {
            info!("Получаем задачу из очереди, размер: {}", queue.len());
        }
        queue.pop_front()
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
    /// use doradura::queue::DownloadQueue;
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
    /// use doradura::queue::DownloadQueue;
    /// 
    /// # async fn example() {
    /// let queue = DownloadQueue::new();
    /// let user_tasks = queue.filter_tasks_by_chat_id(ChatId(123456789)).await;
    /// # }
    /// ```
    pub async fn filter_tasks_by_chat_id(&self, chat_id: ChatId) -> Vec<DownloadTask> {
        let queue = self.queue.lock().await;
        queue
            .iter()
            .filter(|task| task.chat_id == chat_id)
            .cloned()
            .collect()
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
    /// use doradura::queue::DownloadQueue;
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

    #[tokio::test]
    async fn test_add_and_get_task() {
        let queue = DownloadQueue::new();
        let task = DownloadTask::new(
            "http://example.com".to_string(),
            ChatId(123),
            false,
            "mp3".to_string(),
            None,
            Some("320k".to_string())
        );

        queue.add_task(task.clone()).await;
        assert_eq!(queue.size().await, 1);

        let fetched_task = queue.get_task().await.expect("Should retrieve task that was just added");
        assert_eq!(fetched_task.url, task.url);
    }

    #[tokio::test]
    async fn test_remove_old_tasks() {
        let queue = DownloadQueue::new();
        // Create old task manually with custom timestamp
        let old_task = DownloadTask {
            id: uuid::Uuid::new_v4().to_string(),
            url: "http://example.com/old".to_string(),
            chat_id: ChatId(123),
            is_video: false,
            format: "mp3".to_string(),
            video_quality: None,
            audio_bitrate: Some("320k".to_string()),
            created_timestamp: Utc::now() - Duration::days(2),
        };
        let new_task = DownloadTask::new(
            "http://example.com/new".to_string(),
            ChatId(456),
            true,
            "mp4".to_string(),
            Some("1080p".to_string()),
            None
        );

        queue.add_task(old_task).await;
        queue.add_task(new_task).await;

        let removed = queue.remove_old_tasks(Duration::days(1)).await;
        assert_eq!(removed, 1);
        assert_eq!(queue.size().await, 1);
    }
}
