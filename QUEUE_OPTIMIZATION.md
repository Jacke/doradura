# Оптимизация очереди задач

## Текущая проблема

**Файл**: `src/download/queue.rs`
**Проблема**: Использование `VecDeque` с ручной сортировкой по приоритету

### Неэффективный код (строки 256-281):

```rust
let mut queue = self.queue.lock().await;

// Находим позицию для вставки с учетом приоритета
let insert_pos = queue
    .iter()
    .position(|t| t.priority < task.priority)
    .unwrap_or(queue.len());

// Вставляем задачу в нужную позицию
let mut new_queue = VecDeque::new();      // ❌ O(n) память
let mut inserted = false;

for (idx, existing_task) in queue.iter().enumerate() {  // ❌ O(n) итерация
    if idx == insert_pos && !inserted {
        new_queue.push_back(task.clone());
        inserted = true;
    }
    new_queue.push_back(existing_task.clone());  // ❌ O(n) клонирование
}

if !inserted {
    new_queue.push_back(task);
}

*queue = new_queue;  // ❌ Замена всей очереди
```

**Сложность**: O(n) время + O(n) память + лишние аллокации

## Решение: BinaryHeap

### Преимущества:
- ✅ **O(log n)** вставка вместо O(n)
- ✅ **O(log n)** извлечение максимума
- ✅ Встроенная поддержка приоритетов
- ✅ Меньше аллокаций памяти
- ✅ Меньше клонирований

### Реализация:

```rust
use std::collections::BinaryHeap;
use std::cmp::Ordering;

// Обертка для обратного порядка (max-heap -> min-heap по времени)
#[derive(Clone)]
struct PriorityTask {
    task: DownloadTask,
}

impl Eq for PriorityTask {}

impl PartialEq for PriorityTask {
    fn eq(&self, other: &Self) -> bool {
        self.task.priority == other.task.priority
            && self.task.created_timestamp == other.task.created_timestamp
    }
}

impl Ord for PriorityTask {
    fn cmp(&self, other: &Self) -> Ordering {
        // Сначала по приоритету (High > Medium > Low)
        match self.task.priority.cmp(&other.task.priority) {
            Ordering::Equal => {
                // При равном приоритете - старые задачи первыми (FIFO)
                other.task.created_timestamp.cmp(&self.task.created_timestamp)
            }
            other => other,
        }
    }
}

impl PartialOrd for PriorityTask {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

pub struct DownloadQueue {
    queue: Mutex<BinaryHeap<PriorityTask>>,
}

impl DownloadQueue {
    pub async fn add_task(&self, task: DownloadTask, db_pool: Option<Arc<DbPool>>) {
        // ... сохранение в БД ...

        let mut queue = self.queue.lock().await;
        queue.push(PriorityTask { task });  // ✅ O(log n)
    }

    pub async fn get_task(&self) -> Option<DownloadTask> {
        let mut queue = self.queue.lock().await;
        queue.pop().map(|pt| pt.task)  // ✅ O(log n)
    }

    pub async fn size(&self) -> usize {
        self.queue.lock().await.len()  // ✅ O(1)
    }
}
```

## Результаты оптимизации

### Производительность:

| Операция | VecDeque (старая) | BinaryHeap (новая) | Улучшение |
|----------|-------------------|-------------------|-----------|
| add_task | O(n) | O(log n) | 10-100x быстрее при n>100 |
| get_task | O(1) | O(log n) | Немного медленнее |
| size | O(1) | O(1) | Без изменений |

### Пример:
- **n=1000 задач**:
  - VecDeque: ~1000 операций для вставки
  - BinaryHeap: ~10 операций для вставки
  - **Ускорение в 100 раз!**

## План внедрения

1. ✅ Создать новую реализацию с BinaryHeap
2. ✅ Обновить тесты для новой структуры
3. ✅ Убедиться что приоритеты работают корректно
4. ✅ Проверить FIFO порядок внутри одного приоритета
5. ✅ Запустить все тесты
6. ✅ Замерить производительность на больших очередях

## Совместимость

Изменения затрагивают только внутреннюю реализацию. Публичный API остается прежним:
- `add_task(task, db_pool)`
- `get_task()`
- `size()`
- `filter_tasks_by_chat_id()`
- `remove_old_tasks()`

Все существующие тесты должны пройти без изменений.
