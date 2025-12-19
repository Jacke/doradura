# Task Queue Optimization

## Current issue
**File:** `src/download/queue.rs`
**Problem:** `VecDeque` with manual priority insertion.

Inefficient code (previous):
```rust
// find insert position
// build a new VecDeque with cloning (O(n) time, O(n) extra memory)
```
Complexity: O(n) time + O(n) memory + extra allocations/clones.

## Solution: BinaryHeap

### Advantages
- ✅ O(log n) insert
- ✅ O(log n) pop max
- ✅ Built-in priority handling
- ✅ Fewer allocations/clones

### Implementation sketch
```rust
use std::cmp::Ordering;
use std::collections::BinaryHeap;

#[derive(Clone)]
struct PriorityTask {
    task: DownloadTask,
}

impl Eq for PriorityTask {}
impl PartialEq for PriorityTask {
    fn eq(&self, other: &Self) -> bool {
        self.task.priority == other.task.priority && self.task.created_timestamp == other.task.created_timestamp
    }
}
impl Ord for PriorityTask {
    fn cmp(&self, other: &Self) -> Ordering {
        self.task
            .priority
            .cmp(&other.task.priority)
            .then_with(|| other.task.created_timestamp.cmp(&self.task.created_timestamp))
    }
}
impl PartialOrd for PriorityTask {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

struct DownloadQueue {
    queue: Mutex<BinaryHeap<PriorityTask>>,
}
```

### Notes
- Use `BinaryHeap::push` and `BinaryHeap::pop` for enqueue/dequeue.
- Tie-breaker: older tasks first (compare timestamps as shown).
- Wrap existing queue stats/position helpers accordingly.

## Expected outcome
- Better performance with many tasks.
- Simpler insertion logic.
- Lower memory churn.
