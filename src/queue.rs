use std::sync::Mutex;
use std::collections::VecDeque;
use teloxide::types::ChatId;
use chrono::{DateTime, Utc};

#[derive(Debug, Clone)]
pub struct DownloadTask {
    pub url: String,
    pub chat_id: ChatId,
    pub is_video: bool,
    pub created_timestamp: DateTime<Utc>,
}

pub struct DownloadQueue {
    pub queue: Mutex<VecDeque<DownloadTask>>,
}

impl DownloadQueue {
    pub fn new() -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
        }
    }

    pub fn add_task(&self, task: DownloadTask) {
        println!("add_task: {:?}", task);
        let mut queue = self.queue.lock().unwrap();
        queue.push_back(task);
    }

    pub fn get_task(&self) -> Option<DownloadTask> {
        let mut queue = self.queue.lock().unwrap();
        if queue.len() != 0 {
            println!("get_task queue: {:?}", queue);
        }
        queue.pop_front()
    }

    pub fn size(&self) -> usize {
        let queue = self.queue.lock().unwrap();
        queue.len()
    }

    pub fn filter_tasks_by_chat_id(&self, chat_id: ChatId) -> Vec<DownloadTask> {
        let queue = self.queue.lock().unwrap();
        queue
            .iter()
            .filter(|task| task.chat_id == chat_id)
            .cloned()
            .collect()
    }    
}
