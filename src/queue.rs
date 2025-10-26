use std::collections::VecDeque;
use tokio::sync::Mutex;
use teloxide::types::ChatId;
use chrono::{DateTime, Utc};
use log::info; // Использование логирования вместо println

/// Структура, представляющая задачу загрузки.
#[derive(Debug, Clone)]
pub struct DownloadTask {
    pub id: String, // Unique identifier for the task
    pub url: String,
    pub chat_id: ChatId,
    pub is_video: bool,
    pub created_timestamp: DateTime<Utc>,
}

impl DownloadTask {
    /// Creates a new download task with a unique ID
    pub fn new(url: String, chat_id: ChatId, is_video: bool) -> Self {
        let id = uuid::Uuid::new_v4().to_string();
        Self {
            id,
            url,
            chat_id,
            is_video,
            created_timestamp: Utc::now(),
        }
    }
}

/// Очередь для задач загрузки с потокобезопасной реализацией.
pub struct DownloadQueue {
    pub queue: Mutex<VecDeque<DownloadTask>>,
}

impl DownloadQueue {
    /// Создает новую пустую очередь.
    pub fn new() -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
        }
    }

    /// Добавляет задачу в очередь.
    pub async fn add_task(&self, task: DownloadTask) {
        info!("Добавляем задачу: {:?}", task);
        let mut queue = self.queue.lock().await;
        queue.push_back(task);
    }

    /// Возвращает первую задачу из очереди или None, если очередь пуста.
    pub async fn get_task(&self) -> Option<DownloadTask> {
        let mut queue = self.queue.lock().await;
        if queue.len() != 0 {
            info!("Получаем задачу из очереди, размер: {}", queue.len());
        }
        queue.pop_front()
    }

    /// Возвращает размер очереди.
    pub async fn size(&self) -> usize {
        let queue = self.queue.lock().await;
        queue.len()
    }

    /// Фильтрует задачи по chat_id и возвращает список задач, связанных с данным chat_id.
    pub async fn filter_tasks_by_chat_id(&self, chat_id: ChatId) -> Vec<DownloadTask> {
        let queue = self.queue.lock().await;
        queue
            .iter()
            .filter(|task| task.chat_id == chat_id)
            .cloned()
            .collect()
    }

    /// Удаляет задачи, которые старше заданного временного порога.
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
            false
        );

        queue.add_task(task.clone()).await;
        assert_eq!(queue.size().await, 1);

        let fetched_task = queue.get_task().await.unwrap();
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
            created_timestamp: Utc::now() - Duration::days(2),
        };
        let new_task = DownloadTask::new(
            "http://example.com/new".to_string(),
            ChatId(456),
            true
        );

        queue.add_task(old_task).await;
        queue.add_task(new_task).await;

        let removed = queue.remove_old_tasks(Duration::days(1)).await;
        assert_eq!(removed, 1);
        assert_eq!(queue.size().await, 1);
    }
}
