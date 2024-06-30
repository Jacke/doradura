use std::sync::Mutex;
use std::collections::VecDeque;
use teloxide::types::ChatId;

#[derive(Debug, Clone)]
pub struct DownloadTask {
    pub url: String,
    pub chat_id: ChatId,
    pub is_video: bool,
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
        println!("get_task queue: {:?}", queue);
        queue.pop_front()
    }
}
