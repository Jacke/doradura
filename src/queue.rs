use std::sync::{Mutex, MutexGuard};
use std::collections::VecDeque;
use teloxide::types::ChatId;
use chrono::{DateTime, Utc};
use log::{info, error}; // Использование логирования вместо println

/// Структура, представляющая задачу загрузки.
#[derive(Debug, Clone)]
pub struct DownloadTask {
    pub url: String,
    pub chat_id: ChatId,
    pub is_video: bool,
    pub created_timestamp: DateTime<Utc>,
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

    /// Добавляет задачу в очередь. Возвращает ошибку в случае, если не удалось заблокировать Mutex.
    pub fn add_task(&self, task: DownloadTask) -> Result<(), String> {
        info!("Добавляем задачу: {:?}", task);
        match self.queue.lock() {
            Ok(mut queue) => {
                queue.push_back(task);
                Ok(())
            }
            Err(e) => {
                error!("Ошибка при добавлении задачи: {:?}", e);
                Err(format!("Ошибка блокировки очереди: {}", e))
            }
        }
    }

    /// Возвращает первую задачу из очереди или None, если очередь пуста.
    pub fn get_task(&self) -> Option<DownloadTask> {
        match self.queue.lock() {
            Ok(mut queue) => {
                if queue.len() != 0 {
                    info!("Получаем задачу из очереди: {:?}", queue);
                }
                queue.pop_front()
            }
            Err(e) => {
                error!("Ошибка при получении задачи: {:?}", e);
                None
            }
        }
    }

    /// Возвращает размер очереди. Если блокировка не удалась, возвращает 0.
    pub fn size(&self) -> usize {
        match self.queue.lock() {
            Ok(queue) => queue.len(),
            Err(e) => {
                error!("Ошибка при получении размера очереди: {:?}", e);
                0
            }
        }
    }

    /// Фильтрует задачи по chat_id и возвращает список задач, связанных с данным chat_id.
    pub fn filter_tasks_by_chat_id(&self, chat_id: ChatId) -> Vec<DownloadTask> {
        match self.queue.lock() {
            Ok(queue) => queue
                .iter()
                .filter(|task| task.chat_id == chat_id)
                .cloned()
                .collect(),
            Err(e) => {
                error!("Ошибка при фильтрации задач: {:?}", e);
                vec![]
            }
        }
    }

    /// Удаляет задачи, которые старше заданного временного порога.
    pub fn remove_old_tasks(&self, max_age: chrono::Duration) -> Result<usize, String> {
        match self.queue.lock() {
            Ok(mut queue) => {
                let before = queue.len();
                queue.retain(|task| Utc::now() - task.created_timestamp < max_age);
                let removed_count = before - queue.len();
                info!("Удалено старых задач: {}", removed_count);
                Ok(removed_count)
            }
            Err(e) => {
                error!("Ошибка при удалении старых задач: {:?}", e);
                Err(format!("Ошибка блокировки очереди: {}", e))
            }
        }
    }

    /// Пример функции для безопасного получения блокировки на очередь
    fn safe_lock_queue(&self) -> Result<MutexGuard<VecDeque<DownloadTask>>, String> {
        self.queue.lock().map_err(|e| format!("Ошибка блокировки очереди: {}", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn test_add_and_get_task() {
        let queue = DownloadQueue::new();
        let task = DownloadTask {
            url: "http://example.com".to_string(),
            chat_id: ChatId(123),
            is_video: false,
            created_timestamp: Utc::now(),
        };

        queue.add_task(task.clone()).unwrap();
        assert_eq!(queue.size(), 1);

        let fetched_task = queue.get_task().unwrap();
        assert_eq!(fetched_task.url, task.url);
    }

    #[test]
    fn test_remove_old_tasks() {
        let queue = DownloadQueue::new();
        let old_task = DownloadTask {
            url: "http://example.com/old".to_string(),
            chat_id: ChatId(123),
            is_video: false,
            created_timestamp: Utc::now() - Duration::days(2),
        };
        let new_task = DownloadTask {
            url: "http://example.com/new".to_string(),
            chat_id: ChatId(456),
            is_video: true,
            created_timestamp: Utc::now(),
        };

        queue.add_task(old_task).unwrap();
        queue.add_task(new_task).unwrap();

        let removed = queue.remove_old_tasks(Duration::days(1)).unwrap();
        assert_eq!(removed, 1);
        assert_eq!(queue.size(), 1);
    }
}
