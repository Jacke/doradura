use crate::core::metrics;
use crate::storage::db::{DbPool, EnqueueResult, TaskQueueEntry};
use crate::storage::{QueueTaskInput, SharedStorage};

/// Maximum number of tasks allowed in the queue to prevent unbounded memory growth.
const MAX_QUEUE_SIZE: usize = 1000;
use chrono::{DateTime, Utc};
use log::info; // Using logging instead of println
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Instant;
use teloxide::types::ChatId;
use tokio::sync::Mutex;

/// The format of a download task.
///
/// `Display` / `FromStr` / `AsRef<str>` are all derived by `strum`. The
/// serialized form is the lowercase variant name — these strings are used
/// as file extensions, column values, and API payloads, so don't change
/// the `serialize_all` casing.
#[derive(Debug, Clone, PartialEq, Eq, strum::Display, strum::EnumString, strum::AsRefStr, strum::IntoStaticStr)]
#[strum(serialize_all = "lowercase")]
pub enum DownloadFormat {
    /// Audio file in MP3 format.
    Mp3,
    /// Video file in MP4 format.
    Mp4,
    /// Subtitles in SRT format.
    Srt,
    /// Subtitles / transcript in plain text format.
    Txt,
}

impl DownloadFormat {
    /// Alias for `Into::<&'static str>::into` so existing `format.as_str()`
    /// call sites keep working unchanged.
    pub fn as_str(&self) -> &'static str {
        self.into()
    }

    /// Returns `true` when the format produces a video file.
    pub fn is_video(&self) -> bool {
        matches!(self, Self::Mp4)
    }

    /// Returns `true` when the format produces an audio file.
    pub fn is_audio(&self) -> bool {
        matches!(self, Self::Mp3)
    }
}

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
///
/// Constructed via the derived builder — the old positional constructors
/// (`new` / `with_priority` / `from_plan`) were replaced after field count
/// grew past 5 and callers started passing `None, false, None, None` chains
/// that were impossible to read at the call site.
///
/// ```ignore
/// use doradura::download::queue::{DownloadFormat, DownloadTask, TaskPriority};
/// use teloxide::types::ChatId;
///
/// let task = DownloadTask::builder()
///     .url("https://youtube.com/watch?v=abc".to_string())
///     .chat_id(ChatId(123))
///     .is_video(false)
///     .format(DownloadFormat::Mp3)
///     .audio_bitrate("320k".to_string())
///     .build();
/// ```
#[derive(Debug, Clone, bon::Builder)]
pub struct DownloadTask {
    /// Unique task identifier (UUID). Auto-generated if not supplied.
    #[builder(default = uuid::Uuid::new_v4().to_string())]
    pub id: String,
    /// Source URL for the download
    pub url: String,
    /// User's Telegram chat ID
    pub chat_id: ChatId,
    /// User's message ID (for reactions)
    pub message_id: Option<i32>,
    /// Flag indicating whether this is a video download
    pub is_video: bool,
    /// Download format.
    pub format: DownloadFormat,
    /// Video quality: "best", "1080p", "720p", "480p", "360p" (video only)
    pub video_quality: Option<String>,
    /// Audio bitrate: "128k", "192k", "256k", "320k" (audio only)
    pub audio_bitrate: Option<String>,
    /// Task creation timestamp. Defaults to `Utc::now()` if omitted.
    #[builder(default = Utc::now())]
    pub created_timestamp: DateTime<Utc>,
    /// Task priority (for the priority queue). Defaults to `Low` if omitted.
    #[builder(default = TaskPriority::Low)]
    pub priority: TaskPriority,
    /// Time range for partial download (start, end), e.g. ("00:01:00", "00:02:30")
    pub time_range: Option<(String, String)>,
    /// "Task added to queue" message ID (to delete when processing starts)
    pub queue_message_id: Option<i32>,
    /// Carousel bitmask: which items to download from a multi-item post (e.g., Instagram carousel).
    /// Bit N = item N selected. None = download all items.
    pub carousel_mask: Option<u32>,
    /// Whether to fetch and send lyrics highlights alongside the audio.
    #[builder(default = false)]
    pub with_lyrics: bool,
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
    /// Maps chat_id -> (message_id, inserted_at).
    /// Separate from the task so deletion works even when the task is already dequeued.
    /// Entries older than 1 hour are cleaned up periodically.
    notification_msgs: Mutex<HashMap<i64, (i32, Instant)>>,
    /// Backend-aware shared storage for multi-instance-safe queue operations.
    shared_storage: Option<Arc<SharedStorage>>,
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
        Self::with_storage(None)
    }

    pub fn with_db_pool(db_pool: Option<Arc<DbPool>>) -> Self {
        Self::with_storage(db_pool.map(|pool| Arc::new(SharedStorage::Sqlite { db_pool: pool })))
    }

    pub fn with_storage(shared_storage: Option<Arc<SharedStorage>>) -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
            active_tasks: Mutex::new(HashSet::new()),
            notification_msgs: Mutex::new(HashMap::new()),
            shared_storage,
        }
    }

    fn backing_storage(&self, db_pool: Option<Arc<DbPool>>) -> Option<Arc<SharedStorage>> {
        self.shared_storage
            .clone()
            .or_else(|| db_pool.map(|pool| Arc::new(SharedStorage::Sqlite { db_pool: pool })))
    }

    fn idempotency_key(task: &DownloadTask) -> String {
        format!(
            "{}:{}:{}:{}:{}:{}",
            task.chat_id.0,
            task.url,
            task.format,
            task.video_quality.as_deref().unwrap_or("-"),
            task.audio_bitrate.as_deref().unwrap_or("-"),
            if task.is_video { "video" } else { "audio" }
        )
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
    /// use doradura::download::queue::{DownloadQueue, DownloadTask, DownloadFormat};
    ///
    /// # async fn example() {
    /// let queue = DownloadQueue::new();
    /// let task = DownloadTask::builder()
    ///     .url("https://youtube.com/watch?v=...".to_string())
    ///     .chat_id(ChatId(123456789))
    ///     .message_id(12345)
    ///     .is_video(false)
    ///     .format(DownloadFormat::Mp3)
    ///     .audio_bitrate("320k".to_string())
    ///     .build();
    /// queue.add_task(task, None).await;
    /// # }
    /// ```
    pub async fn add_task(&self, task: DownloadTask, db_pool: Option<Arc<DbPool>>) {
        info!("Adding task with priority {:?}: {:?}", task.priority, task);

        // Check for duplicates: skip if a task with the same URL, chat_id, and format already exists
        let task_key = (task.url.clone(), task.chat_id.0, task.format.to_string());
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

        let backing_storage = self.backing_storage(db_pool);

        // Hold active_tasks lock while checking queue size and inserting,
        // to prevent race conditions where a task key is added but never queued.
        let mut queue = self.queue.lock().await;

        // Check queue size limit to prevent unbounded memory growth
        let queue_len = if let Some(ref storage) = backing_storage {
            match storage.count_active_tasks().await {
                Ok(count) => count,
                Err(e) => {
                    log::warn!("Failed to get DB-backed queue size: {}", e);
                    queue.len()
                }
            }
        } else {
            queue.len()
        };
        if queue_len >= MAX_QUEUE_SIZE {
            log::warn!("Queue is full ({} tasks), rejecting new task: {}", queue_len, task.url);
            // Don't insert into active_tasks — task is rejected
            return;
        }

        // Add to the active tasks set (only after confirming queue has space)
        active_tasks.insert(task_key.clone());
        drop(active_tasks); // Release after both checks passed

        // Persist the task to the database to guarantee processing
        if let Some(ref storage) = backing_storage {
            let priority_value = task.priority as i32;
            let idempotency_key = Self::idempotency_key(&task);
            match storage
                .save_task_to_queue(QueueTaskInput {
                    task_id: &task.id,
                    user_id: task.chat_id.0,
                    url: &task.url,
                    message_id: task.message_id,
                    format: task.format.as_str(),
                    is_video: task.is_video,
                    video_quality: task.video_quality.as_deref(),
                    audio_bitrate: task.audio_bitrate.as_deref(),
                    time_range_start: task.time_range.as_ref().map(|(start, _)| start.as_str()),
                    time_range_end: task.time_range.as_ref().map(|(_, end)| end.as_str()),
                    carousel_mask: task.carousel_mask,
                    priority: priority_value,
                    idempotency_key: &idempotency_key,
                })
                .await
            {
                Ok(EnqueueResult::Enqueued) => log::debug!("Task {} saved to database", task.id),
                Ok(EnqueueResult::Duplicate) => {
                    log::info!("Skipping duplicate queued task {}", task.id);
                    active_tasks_remove_after_duplicate(&self.active_tasks, task_key).await;
                    return;
                }
                Err(e) => {
                    log::error!("Failed to save task {} to database: {}", task.id, e);
                    active_tasks_remove_after_duplicate(&self.active_tasks, task_key).await;
                    return;
                }
            }
        }

        if backing_storage.is_some() {
            metrics::update_queue_depth_total(queue_len + 1);
            return;
        }

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
        if let Some(ref storage) = self.shared_storage {
            if let Ok(position) = storage.get_queue_position(chat_id.0).await {
                return position;
            }
        }
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
        if let Some(ref storage) = self.shared_storage {
            if let Ok(size) = storage.count_active_tasks().await {
                return size;
            }
        }
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
        if let Some(ref storage) = self.shared_storage {
            if let Ok(entries) = storage.get_pending_tasks_for_user(chat_id.0).await {
                return entries.into_iter().map(Self::task_from_entry).collect();
            }
        }
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
    /// Stores in a separate map (not only on the task) so deletion works even
    /// when the task has already been dequeued (race condition with fast queues).
    pub async fn set_queue_message_id(&self, chat_id: ChatId, msg_id: i32) {
        // Store in the dedicated map (race-condition-safe)
        self.notification_msgs
            .lock()
            .await
            .insert(chat_id.0, (msg_id, Instant::now()));
        if self.shared_storage.is_some() {
            return;
        }
        // Also set on the task if it's still in the queue
        let mut queue = self.queue.lock().await;
        if let Some(task) = queue.iter_mut().rev().find(|t| t.chat_id == chat_id) {
            task.queue_message_id = Some(msg_id);
        }
    }

    /// Removes and returns the queue notification message ID for a chat.
    ///
    /// Returns `Some(msg_id)` if a notification was stored, `None` otherwise.
    /// Calling this also clears the stored ID, so it won't be returned again.
    pub async fn take_notification_message(&self, chat_id: ChatId) -> Option<i32> {
        self.notification_msgs
            .lock()
            .await
            .remove(&chat_id.0)
            .map(|(msg_id, _)| msg_id)
    }

    /// Removes notification_msgs entries older than 1 hour.
    /// Called periodically to prevent unbounded growth from orphaned entries.
    pub async fn cleanup_stale_notifications(&self) -> usize {
        let mut msgs = self.notification_msgs.lock().await;
        let before = msgs.len();
        msgs.retain(|_, (_, inserted_at)| inserted_at.elapsed() < std::time::Duration::from_secs(3600));
        before - msgs.len()
    }

    /// Recovers tasks from DB entries (pending + previously processing).
    /// Called once at startup to restore tasks that survived a restart.
    /// Returns the number of tasks recovered.
    pub async fn recover_from_db(&self, entries: Vec<TaskQueueEntry>) -> usize {
        if self.shared_storage.is_some() {
            return entries.len();
        }
        let count = entries.len();
        let mut queue = self.queue.lock().await;
        let mut active_tasks = self.active_tasks.lock().await;

        for entry in entries {
            let priority = match entry.priority {
                2 => TaskPriority::High,
                1 => TaskPriority::Medium,
                _ => TaskPriority::Low,
            };

            let task_key = (entry.url.clone(), entry.user_id, entry.format.clone());
            if active_tasks.contains(&task_key) {
                continue;
            }
            active_tasks.insert(task_key);

            let task = DownloadTask {
                id: entry.id,
                url: entry.url,
                chat_id: ChatId(entry.user_id),
                message_id: entry.message_id,
                is_video: entry.is_video,
                format: entry.format.parse::<DownloadFormat>().unwrap_or(DownloadFormat::Mp3),
                video_quality: entry.video_quality,
                audio_bitrate: entry.audio_bitrate,
                created_timestamp: chrono::Utc::now(),
                priority,
                time_range: match (entry.time_range_start, entry.time_range_end) {
                    (Some(start), Some(end)) => Some((start, end)),
                    _ => None,
                },
                queue_message_id: None,
                carousel_mask: entry.carousel_mask,
                with_lyrics: false,
            };

            // Insert respecting priority order
            let insert_pos = queue
                .iter()
                .position(|t| t.priority < task.priority)
                .unwrap_or(queue.len());
            queue.insert(insert_pos, task);
        }

        // Update metrics
        let low_count = queue.iter().filter(|t| t.priority == TaskPriority::Low).count();
        let medium_count = queue.iter().filter(|t| t.priority == TaskPriority::Medium).count();
        let high_count = queue.iter().filter(|t| t.priority == TaskPriority::High).count();
        metrics::update_queue_depth("low", low_count);
        metrics::update_queue_depth("medium", medium_count);
        metrics::update_queue_depth("high", high_count);
        metrics::update_queue_depth_total(queue.len());

        count
    }

    /// Saves all remaining in-memory tasks to the database on shutdown.
    /// Tasks already saved (via add_task) get a no-op upsert; tasks that
    /// were only in memory are persisted so they survive the restart.
    /// Returns the number of tasks flushed.
    pub async fn flush_to_db(&self, db_pool: &DbPool) -> usize {
        if self.shared_storage.is_some() {
            return 0;
        }
        let queue = self.queue.lock().await;
        if queue.is_empty() {
            return 0;
        }

        let conn = match crate::storage::db::get_connection(db_pool) {
            Ok(c) => c,
            Err(e) => {
                log::error!("Graceful shutdown: failed to get DB connection: {}", e);
                return 0;
            }
        };

        let mut flushed = 0;
        for task in queue.iter() {
            if let Err(e) = crate::storage::db::save_task_to_queue(
                &conn,
                &task.id,
                task.chat_id.0,
                &task.url,
                task.message_id,
                task.format.as_str(),
                task.is_video,
                task.video_quality.as_deref(),
                task.audio_bitrate.as_deref(),
                task.time_range.as_ref().map(|(start, _)| start.as_str()),
                task.time_range.as_ref().map(|(_, end)| end.as_str()),
                task.carousel_mask,
                task.priority as i32,
                &Self::idempotency_key(task),
            ) {
                log::error!("Graceful shutdown: failed to save task {}: {}", task.id, e);
            } else {
                flushed += 1;
            }
        }
        flushed
    }

    pub(crate) fn task_from_entry(entry: TaskQueueEntry) -> DownloadTask {
        let priority = match entry.priority {
            2 => TaskPriority::High,
            1 => TaskPriority::Medium,
            _ => TaskPriority::Low,
        };
        let created_timestamp = chrono::DateTime::parse_from_rfc3339(&entry.created_at)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());
        DownloadTask {
            id: entry.id,
            url: entry.url,
            chat_id: ChatId(entry.user_id),
            message_id: entry.message_id,
            is_video: entry.is_video,
            format: entry.format.parse::<DownloadFormat>().unwrap_or(DownloadFormat::Mp3),
            video_quality: entry.video_quality,
            audio_bitrate: entry.audio_bitrate,
            created_timestamp,
            priority,
            time_range: match (entry.time_range_start, entry.time_range_end) {
                (Some(start), Some(end)) => Some((start, end)),
                _ => None,
            },
            queue_message_id: None,
            carousel_mask: entry.carousel_mask,
            with_lyrics: false,
        }
    }
}

async fn active_tasks_remove_after_duplicate(
    active_tasks: &Mutex<HashSet<(String, i64, String)>>,
    task_key: (String, i64, String),
) {
    active_tasks.lock().await.remove(&task_key);
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
        let task = DownloadTask::builder()
            .url("http://example.com".to_string())
            .chat_id(ChatId(123))
            .maybe_message_id(Some(456))
            .is_video(false)
            .format(DownloadFormat::Mp3)
            .maybe_video_quality(None)
            .maybe_audio_bitrate(Some("320k".to_string()))
            .build();
        assert!(!task.id.is_empty());
        assert_eq!(task.url, "http://example.com");
        assert_eq!(task.chat_id, ChatId(123));
        assert_eq!(task.message_id, Some(456));
        assert!(!task.is_video);
        assert_eq!(task.format, DownloadFormat::Mp3);
        assert_eq!(task.video_quality, None);
        assert_eq!(task.audio_bitrate, Some("320k".to_string()));
        assert_eq!(task.priority, TaskPriority::Low);
    }

    #[test]
    fn test_download_task_with_priority() {
        let task = DownloadTask::builder()
            .url("http://example.com".to_string())
            .chat_id(ChatId(123))
            .maybe_message_id(None)
            .is_video(true)
            .format(DownloadFormat::Mp4)
            .maybe_video_quality(Some("1080p".to_string()))
            .maybe_audio_bitrate(None)
            .priority(TaskPriority::High)
            .maybe_time_range(None)
            .build();
        assert_eq!(task.priority, TaskPriority::High);
        assert!(task.is_video);
        assert_eq!(task.video_quality, Some("1080p".to_string()));
    }

    #[test]
    fn test_download_task_from_plan() {
        let vip_task = DownloadTask::builder()
            .url("http://example.com".to_string())
            .chat_id(ChatId(123))
            .maybe_message_id(None)
            .is_video(false)
            .format(DownloadFormat::Mp3)
            .maybe_video_quality(None)
            .maybe_audio_bitrate(None)
            .priority(TaskPriority::from_plan("vip"))
            .build();
        assert_eq!(vip_task.priority, TaskPriority::High);

        let premium_task = DownloadTask::builder()
            .url("http://example.com".to_string())
            .chat_id(ChatId(123))
            .maybe_message_id(None)
            .is_video(false)
            .format(DownloadFormat::Mp3)
            .maybe_video_quality(None)
            .maybe_audio_bitrate(None)
            .priority(TaskPriority::from_plan("premium"))
            .build();
        assert_eq!(premium_task.priority, TaskPriority::Medium);

        let free_task = DownloadTask::builder()
            .url("http://example.com".to_string())
            .chat_id(ChatId(123))
            .maybe_message_id(None)
            .is_video(false)
            .format(DownloadFormat::Mp3)
            .maybe_video_quality(None)
            .maybe_audio_bitrate(None)
            .priority(TaskPriority::from_plan("free"))
            .build();
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
        let task = DownloadTask::builder()
            .url("http://example.com".to_string())
            .chat_id(ChatId(123))
            .maybe_message_id(Some(12345))
            .is_video(false)
            .format(DownloadFormat::Mp3)
            .maybe_video_quality(None)
            .maybe_audio_bitrate(Some("320k".to_string()))
            .build();

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
        let low_task = DownloadTask::builder()
            .url("http://low.com".to_string())
            .chat_id(ChatId(1))
            .maybe_message_id(None)
            .is_video(false)
            .format(DownloadFormat::Mp3)
            .maybe_video_quality(None)
            .maybe_audio_bitrate(None)
            .priority(TaskPriority::from_plan("free"))
            .build();
        queue.add_task(low_task, None).await;

        // Add high priority task second
        let high_task = DownloadTask::builder()
            .url("http://high.com".to_string())
            .chat_id(ChatId(2))
            .maybe_message_id(None)
            .is_video(false)
            .format(DownloadFormat::Mp3)
            .maybe_video_quality(None)
            .maybe_audio_bitrate(None)
            .priority(TaskPriority::from_plan("vip"))
            .build();
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

        let task1 = DownloadTask::builder()
            .url("http://example1.com".to_string())
            .chat_id(ChatId(100))
            .maybe_message_id(None)
            .is_video(false)
            .format(DownloadFormat::Mp3)
            .maybe_video_quality(None)
            .maybe_audio_bitrate(None)
            .build();
        let task2 = DownloadTask::builder()
            .url("http://example2.com".to_string())
            .chat_id(ChatId(200))
            .maybe_message_id(None)
            .is_video(false)
            .format(DownloadFormat::Mp3)
            .maybe_video_quality(None)
            .maybe_audio_bitrate(None)
            .build();
        let task3 = DownloadTask::builder()
            .url("http://example3.com".to_string())
            .chat_id(ChatId(100))
            .maybe_message_id(None)
            .is_video(false)
            .format(DownloadFormat::Mp3)
            .maybe_video_quality(None)
            .maybe_audio_bitrate(None)
            .build();

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

        let task1 = DownloadTask::builder()
            .url("http://example1.com".to_string())
            .chat_id(ChatId(100))
            .maybe_message_id(None)
            .is_video(false)
            .format(DownloadFormat::Mp3)
            .maybe_video_quality(None)
            .maybe_audio_bitrate(None)
            .build();
        let task2 = DownloadTask::builder()
            .url("http://example2.com".to_string())
            .chat_id(ChatId(200))
            .maybe_message_id(None)
            .is_video(false)
            .format(DownloadFormat::Mp3)
            .maybe_video_quality(None)
            .maybe_audio_bitrate(None)
            .build();

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
            format: DownloadFormat::Mp3,
            video_quality: None,
            audio_bitrate: Some("320k".to_string()),
            created_timestamp: Utc::now() - Duration::days(2),
            priority: TaskPriority::Low,
            time_range: None,
            queue_message_id: None,
            carousel_mask: None,
            with_lyrics: false,
        };
        let new_task = DownloadTask::builder()
            .url("http://example.com/new".to_string())
            .chat_id(ChatId(456))
            .maybe_message_id(Some(22222))
            .is_video(true)
            .format(DownloadFormat::Mp4)
            .maybe_video_quality(Some("1080p".to_string()))
            .maybe_audio_bitrate(None)
            .build();

        queue.add_task(old_task, None).await;
        queue.add_task(new_task, None).await;

        let removed = queue.remove_old_tasks(Duration::days(1)).await;
        assert_eq!(removed, 1);
        assert_eq!(queue.size().await, 1);
    }

    #[tokio::test]
    async fn test_remove_old_tasks_all_new() {
        let queue = DownloadQueue::new();

        let task = DownloadTask::builder()
            .url("http://example.com".to_string())
            .chat_id(ChatId(123))
            .maybe_message_id(None)
            .is_video(false)
            .format(DownloadFormat::Mp3)
            .maybe_video_quality(None)
            .maybe_audio_bitrate(None)
            .build();
        queue.add_task(task, None).await;

        let removed = queue.remove_old_tasks(Duration::days(1)).await;
        assert_eq!(removed, 0);
        assert_eq!(queue.size().await, 1);
    }
}
