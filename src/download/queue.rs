use crate::core::metrics;
use crate::storage::db::{save_task_to_queue, DbPool};

/// Maximum number of tasks allowed in the queue to prevent unbounded memory growth.
const MAX_QUEUE_SIZE: usize = 1000;
use chrono::{DateTime, Utc};
use log::info; // Using logging instead of println
use std::collections::{HashSet, VecDeque};
use std::sync::Arc;
use teloxide::types::ChatId;
use tokio::sync::Mutex;

/// Task priority in the queue
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TaskPriority {
    /// Low priority (free users)
    Low = 0,
    /// Medium priority (premium users)
    Medium = 1,
    /// High priority (vip users)
    High = 2,
}

impl TaskPriority {
    /// Returns the priority based on the user's plan
    pub fn from_plan(plan: &str) -> Self {
        match plan {
            "vip" => TaskPriority::High,
            "premium" => TaskPriority::Medium,
            _ => TaskPriority::Low, // Default: free
        }
    }
}

/// Structure representing a download task.
///
/// Contains all the necessary information for downloading a media file:
/// source URL, user identifier, download format, and creation timestamp.
#[derive(Debug, Clone)]
pub struct DownloadTask {
    /// Unique task identifier (UUID)
    pub id: String,
    /// Source URL for the download
    pub url: String,
    /// User's Telegram chat ID
    pub chat_id: ChatId,
    /// User's message ID (for reactions)
    pub message_id: Option<i32>,
    /// Flag indicating whether this is a video download
    pub is_video: bool,
    /// Download format: "mp3", "mp4", "srt", "txt"
    pub format: String,
    /// Video quality: "best", "1080p", "720p", "480p", "360p" (video only)
    pub video_quality: Option<String>,
    /// Audio bitrate: "128k", "192k", "256k", "320k" (audio only)
    pub audio_bitrate: Option<String>,
    /// Task creation timestamp
    pub created_timestamp: DateTime<Utc>,
    /// Task priority (for the priority queue)
    pub priority: TaskPriority,
    /// Time range for partial download (start, end), e.g. ("00:01:00", "00:02:30")
    pub time_range: Option<(String, String)>,
    /// "Task added to queue" message ID (to delete when processing starts)
    pub queue_message_id: Option<i32>,
    /// Carousel bitmask: which items to download from a multi-item post (e.g., Instagram carousel).
    /// Bit N = item N selected. None = download all items.
    pub carousel_mask: Option<u32>,
}

impl DownloadTask {
    /// Creates a new download task with a unique ID.
    ///
    /// # Arguments
    ///
    /// * `url` - URL to download
    /// * `chat_id` - User's Telegram chat ID
    /// * `message_id` - User's message ID (optional, for reactions)
    /// * `is_video` - Flag indicating whether this is a video (true) or audio (false)
    /// * `format` - Download format: "mp3", "mp4", "srt", "txt"
    /// * `video_quality` - Video quality (optional, video only)
    /// * `audio_bitrate` - Audio bitrate (optional, audio only)
    ///
    /// # Returns
    ///
    /// Returns a new `DownloadTask` instance with an auto-generated UUID and the current timestamp.
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
            None,
        )
    }

    /// Creates a new task with the specified priority.
    pub fn with_priority(
        url: String,
        chat_id: ChatId,
        message_id: Option<i32>,
        is_video: bool,
        format: String,
        video_quality: Option<String>,
        audio_bitrate: Option<String>,
        priority: TaskPriority,
        time_range: Option<(String, String)>,
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
            time_range,
            queue_message_id: None,
            carousel_mask: None,
        }
    }

    /// Creates a new task based on the user's plan.
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
            None,
        )
    }
}

/// Thread-safe queue for download tasks.
///
/// Uses a `Mutex` to synchronize access to the internal queue.
/// Tasks are processed in priority order: High first, then Medium, then Low.
/// Within each priority level, tasks are processed in FIFO (First In, First Out) order.
pub struct DownloadQueue {
    /// Internal task queue protected by a mutex.
    /// Tasks are stored in priority order: High -> Medium -> Low.
    pub queue: Mutex<VecDeque<DownloadTask>>,
    /// Set of active tasks (queued + being processed).
    /// Stores (URL, chat_id, format) tuples to prevent duplicates.
    active_tasks: Mutex<HashSet<(String, i64, String)>>,
}

impl Default for DownloadQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl DownloadQueue {
    /// Creates a new empty queue.
    ///
    /// # Returns
    ///
    /// Returns a new `DownloadQueue` instance with an empty internal queue.
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
            active_tasks: Mutex::new(HashSet::new()),
        }
    }

    /// Adds a task to the queue respecting priority order.
    ///
    /// Higher-priority tasks are inserted before lower-priority tasks in the queue.
    ///
    /// # Arguments
    ///
    /// * `task` - Task to add to the queue
    /// * `db_pool` - Optional database connection pool for persisting the task
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
        info!("Adding task with priority {:?}: {:?}", task.priority, task);

        // Check for duplicates: skip if a task with the same URL, chat_id, and format already exists
        let task_key = (task.url.clone(), task.chat_id.0, task.format.clone());
        let mut active_tasks = self.active_tasks.lock().await;

        if active_tasks.contains(&task_key) {
            log::warn!(
                "⚠️ Duplicate task detected for URL '{}', chat_id {} and format '{}'. Skipping.",
                task.url,
                task.chat_id.0,
                task.format
            );
            return;
        }

        // Add to the active tasks set
        active_tasks.insert(task_key);
        drop(active_tasks); // Release the lock early

        // Check queue size limit to prevent unbounded memory growth
        {
            let queue = self.queue.lock().await;
            if queue.len() >= MAX_QUEUE_SIZE {
                log::warn!(
                    "Queue is full ({} tasks), rejecting new task: {}",
                    queue.len(),
                    task.url
                );
                return;
            }
        }

        // Persist the task to the database to guarantee processing
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

        // Find the insertion position respecting priority order
        let insert_pos = queue
            .iter()
            .position(|t| t.priority < task.priority)
            .unwrap_or(queue.len());

        // Insert the task directly — O(n) without copying all elements
        queue.insert(insert_pos, task);

        // Update queue depth metrics by priority
        let low_count = queue.iter().filter(|t| t.priority == TaskPriority::Low).count();
        let medium_count = queue.iter().filter(|t| t.priority == TaskPriority::Medium).count();
        let high_count = queue.iter().filter(|t| t.priority == TaskPriority::High).count();

        metrics::update_queue_depth("low", low_count);
        metrics::update_queue_depth("medium", medium_count);
        metrics::update_queue_depth("high", high_count);
        metrics::update_queue_depth_total(queue.len());
    }

    /// Pops and returns the first task from the queue (respecting priority).
    ///
    /// Tasks with higher priority are processed first.
    ///
    /// # Returns
    ///
    /// Returns `Some(DownloadTask)` if the queue is non-empty, otherwise `None`.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use doradura::download::queue::DownloadQueue;
    ///
    /// # async fn example() {
    /// let queue = DownloadQueue::new();
    /// // ... add tasks ...
    /// if let Some(task) = queue.get_task().await {
    ///     // Process the task
    /// }
    /// # }
    /// ```
    pub async fn get_task(&self) -> Option<DownloadTask> {
        let mut queue = self.queue.lock().await;
        if !queue.is_empty() {
            info!(
                "Retrieving task from queue, size: {}, priority: {:?}",
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

    /// Returns the user's task position in the queue.
    ///
    /// # Arguments
    ///
    /// * `chat_id` - The user's chat ID
    ///
    /// # Returns
    ///
    /// Returns the 1-based position in the queue, or `None` if no task was found.
    pub async fn get_queue_position(&self, chat_id: ChatId) -> Option<usize> {
        let queue = self.queue.lock().await;
        queue.iter().position(|task| task.chat_id == chat_id).map(|pos| pos + 1)
    }

    /// Returns the current number of tasks in the queue.
    ///
    /// # Returns
    ///
    /// The number of tasks in the queue.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use doradura::download::queue::DownloadQueue;
    ///
    /// # async fn example() {
    /// let queue = DownloadQueue::new();
    /// // ... add tasks ...
    /// let count = queue.size().await;
    /// println!("Tasks in queue: {}", count);
    /// # }
    /// ```
    pub async fn size(&self) -> usize {
        let queue = self.queue.lock().await;
        queue.len()
    }

    /// Filters tasks by chat ID and returns a list of tasks belonging to the specified user.
    ///
    /// # Arguments
    ///
    /// * `chat_id` - User chat ID to filter by
    ///
    /// # Returns
    ///
    /// A vector of all tasks belonging to the specified user.
    ///
    /// # Note
    ///
    /// Tasks are not removed from the queue; only clones are returned.
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

    /// Removes tasks that are older than the given age threshold.
    ///
    /// # Arguments
    ///
    /// * `max_age` - Maximum task age; tasks older than this will be removed
    ///
    /// # Returns
    ///
    /// The number of tasks removed.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use chrono::Duration;
    /// use doradura::download::queue::DownloadQueue;
    ///
    /// # async fn example() {
    /// let queue = DownloadQueue::new();
    /// // Remove tasks older than 1 day
    /// let removed = queue.remove_old_tasks(Duration::days(1)).await;
    /// println!("Old tasks removed: {}", removed);
    /// # }
    /// ```
    pub async fn remove_old_tasks(&self, max_age: chrono::Duration) -> usize {
        let mut queue = self.queue.lock().await;
        let before = queue.len();
        queue.retain(|task| Utc::now() - task.created_timestamp < max_age);
        let removed_count = before - queue.len();
        info!("Old tasks removed: {}", removed_count);
        removed_count
    }

    /// Removes a task from the active tasks set after processing completes.
    ///
    /// Must be called AFTER the task finishes processing (successfully or with an error)
    /// to free the slot in the queue for retry attempts.
    ///
    /// # Arguments
    ///
    /// * `url` - Video/audio URL
    /// * `chat_id` - User's chat ID
    /// * `format` - Task format (mp3, mp4, srt, txt)
    ///
    /// # Example
    ///
    /// ```no_run
    /// use teloxide::types::ChatId;
    /// use doradura::download::queue::DownloadQueue;
    ///
    /// # async fn example() {
    /// let queue = DownloadQueue::new();
    /// // After the task has finished processing
    /// queue.remove_active_task("https://youtube.com/watch?v=...", ChatId(123), "mp4").await;
    /// # }
    /// ```
    pub async fn remove_active_task(&self, url: &str, chat_id: ChatId, format: &str) {
        let mut active_tasks = self.active_tasks.lock().await;
        let task_key = (url.to_string(), chat_id.0, format.to_string());
        if active_tasks.remove(&task_key) {
            log::debug!(
                "✅ Removed task from active_tasks: {} (chat: {}, format: {})",
                url,
                chat_id.0,
                format
            );
        } else {
            log::warn!(
                "⚠️ Tried to remove non-existent task: {} (chat: {}, format: {})",
                url,
                chat_id.0,
                format
            );
        }
    }

    /// Sets the queue message ID for the last task belonging to the given chat.
    ///
    /// Called after sending the "Task added to queue" message so that the
    /// message can be deleted when the task starts being processed.
    pub async fn set_queue_message_id(&self, chat_id: ChatId, msg_id: i32) {
        let mut queue = self.queue.lock().await;
        if let Some(task) = queue.iter_mut().rev().find(|t| t.chat_id == chat_id) {
            task.queue_message_id = Some(msg_id);
        }
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
            None,
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
            time_range: None,
            queue_message_id: None,
            carousel_mask: None,
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
