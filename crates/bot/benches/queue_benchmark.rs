//! Queue benchmarks for performance testing
//!
//! Run with: cargo bench --bench queue_benchmark

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Simplified download task for benchmarking
#[derive(Debug, Clone)]
struct BenchTask {
    #[allow(dead_code)]
    id: u64,
    priority: u8,
}

impl BenchTask {
    fn new(id: u64, priority: u8) -> Self {
        Self { id, priority }
    }
}

/// Simplified priority queue for benchmarking
struct BenchQueue {
    tasks: parking_lot::Mutex<VecDeque<BenchTask>>,
    size: AtomicUsize,
}

impl BenchQueue {
    fn new() -> Self {
        Self {
            tasks: parking_lot::Mutex::new(VecDeque::new()),
            size: AtomicUsize::new(0),
        }
    }

    fn push(&self, task: BenchTask) {
        let mut tasks = self.tasks.lock();
        let pos = tasks
            .iter()
            .position(|t| t.priority < task.priority)
            .unwrap_or(tasks.len());
        tasks.insert(pos, task);
        self.size.fetch_add(1, Ordering::Relaxed);
    }

    fn pop(&self) -> Option<BenchTask> {
        let mut tasks = self.tasks.lock();
        let task = tasks.pop_front();
        if task.is_some() {
            self.size.fetch_sub(1, Ordering::Relaxed);
        }
        task
    }

    fn len(&self) -> usize {
        self.size.load(Ordering::Relaxed)
    }
}

fn benchmark_queue_push(c: &mut Criterion) {
    let mut group = c.benchmark_group("queue_push");

    for size in [10, 100, 1000].iter() {
        group.throughput(Throughput::Elements(*size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            b.iter(|| {
                let queue = BenchQueue::new();
                for i in 0..size {
                    let task = BenchTask::new(i as u64, (i % 3) as u8);
                    queue.push(task);
                }
                black_box(queue.len())
            })
        });
    }

    group.finish();
}

fn benchmark_queue_pop(c: &mut Criterion) {
    let mut group = c.benchmark_group("queue_pop");

    for size in [10, 100, 1000].iter() {
        group.throughput(Throughput::Elements(*size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            b.iter_batched(
                || {
                    let queue = BenchQueue::new();
                    for i in 0..size {
                        queue.push(BenchTask::new(i as u64, (i % 3) as u8));
                    }
                    queue
                },
                |queue| {
                    let mut count = 0;
                    while queue.pop().is_some() {
                        count += 1;
                    }
                    black_box(count)
                },
                criterion::BatchSize::SmallInput,
            )
        });
    }

    group.finish();
}

fn benchmark_queue_mixed(c: &mut Criterion) {
    let mut group = c.benchmark_group("queue_mixed");

    // Simulate realistic workload: push/pop interleaved
    for ops in [100, 500, 1000].iter() {
        group.throughput(Throughput::Elements(*ops as u64));
        group.bench_with_input(BenchmarkId::from_parameter(ops), ops, |b, &ops| {
            b.iter(|| {
                let queue = BenchQueue::new();
                let mut id = 0u64;

                // Interleave push and pop
                for _ in 0..ops {
                    // Push 3 tasks
                    for _ in 0..3 {
                        queue.push(BenchTask::new(id, (id % 3) as u8));
                        id += 1;
                    }
                    // Pop 2 tasks
                    queue.pop();
                    queue.pop();
                }

                black_box(queue.len())
            })
        });
    }

    group.finish();
}

fn benchmark_priority_ordering(c: &mut Criterion) {
    let mut group = c.benchmark_group("priority_ordering");

    // Benchmark priority insertion overhead
    group.bench_function("insert_low_priority", |b| {
        b.iter_batched(
            || {
                let queue = BenchQueue::new();
                // Pre-fill with high priority tasks
                for i in 0..100 {
                    queue.push(BenchTask::new(i, 2));
                }
                queue
            },
            |queue| {
                // Insert low priority (should go to end)
                for i in 0..10 {
                    queue.push(BenchTask::new(100 + i, 0));
                }
                black_box(queue.len())
            },
            criterion::BatchSize::SmallInput,
        )
    });

    group.bench_function("insert_high_priority", |b| {
        b.iter_batched(
            || {
                let queue = BenchQueue::new();
                // Pre-fill with low priority tasks
                for i in 0..100 {
                    queue.push(BenchTask::new(i, 0));
                }
                queue
            },
            |queue| {
                // Insert high priority (should go to front)
                for i in 0..10 {
                    queue.push(BenchTask::new(100 + i, 2));
                }
                black_box(queue.len())
            },
            criterion::BatchSize::SmallInput,
        )
    });

    group.finish();
}

fn benchmark_concurrent_access(c: &mut Criterion) {
    use std::sync::Arc;
    use std::thread;

    let mut group = c.benchmark_group("concurrent_access");

    group.bench_function("4_threads_push", |b| {
        b.iter(|| {
            let queue = Arc::new(BenchQueue::new());
            let handles: Vec<_> = (0..4)
                .map(|thread_id| {
                    let q = Arc::clone(&queue);
                    thread::spawn(move || {
                        for i in 0..250 {
                            let id = thread_id * 250 + i;
                            q.push(BenchTask::new(id as u64, (id % 3) as u8));
                        }
                    })
                })
                .collect();

            for handle in handles {
                handle.join().unwrap();
            }

            black_box(queue.len())
        })
    });

    group.bench_function("4_threads_mixed", |b| {
        b.iter(|| {
            let queue = Arc::new(BenchQueue::new());

            // Pre-fill queue
            for i in 0..500 {
                queue.push(BenchTask::new(i, (i % 3) as u8));
            }

            let handles: Vec<_> = (0..4)
                .map(|thread_id| {
                    let q = Arc::clone(&queue);
                    thread::spawn(move || {
                        for i in 0..100 {
                            let id = 500 + thread_id * 100 + i;
                            q.push(BenchTask::new(id as u64, (id % 3) as u8));
                            q.pop();
                        }
                    })
                })
                .collect();

            for handle in handles {
                handle.join().unwrap();
            }

            black_box(queue.len())
        })
    });

    group.finish();
}

criterion_group!(
    benches,
    benchmark_queue_push,
    benchmark_queue_pop,
    benchmark_queue_mixed,
    benchmark_priority_ordering,
    benchmark_concurrent_access,
);

criterion_main!(benches);
