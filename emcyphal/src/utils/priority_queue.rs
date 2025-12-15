use crate::core::{Priority, PrioritySet};
use crate::utils::multi_class_queue::MultiClassQueue;

const PRIORITY_CLASS: u8 = 0;
const FREE_CLASS: u8 = PRIORITY_CLASS + Priority::MAX.into_u8() + 1;
const CLASS_COUNT: usize = FREE_CLASS as usize + 1;

pub const MAX_CAPACITY: usize = u8::MAX as usize + 1 - CLASS_COUNT;

pub struct PriorityQueue<V, const N: usize> {
    entry_queue: MultiClassQueue<CLASS_COUNT, N>,
    entry_values: [Option<V>; N],
    priority_set: PrioritySet,
}

impl<V, const N: usize> PriorityQueue<V, N> {
    const _ASSERT: usize = MAX_CAPACITY - N;

    pub fn new() -> Self {
        let mut queue = MultiClassQueue::new();
        for i in 0..N {
            let entry = unwrap!(u8::try_from(i));
            queue.move_back(FREE_CLASS, entry);
        }
        Self {
            entry_queue: queue,
            entry_values: core::array::from_fn(|_| None),
            priority_set: PrioritySet::NONE,
        }
    }

    pub fn get_value_mut(&mut self, entry: u8) -> Option<&mut V> {
        self.entry_values[usize::from(entry)].as_mut()
    }

    pub fn push(&mut self, priority: Priority, value: V) -> Result<(), V> {
        let entry = match self.entry_queue.front(FREE_CLASS) {
            Some(entry) => entry,
            None => return Err(value),
        };
        self.entry_queue
            .move_back(PRIORITY_CLASS + u8::from(priority), entry);
        self.priority_set.insert(priority);
        self.entry_values[usize::from(entry)] = Some(value);
        Ok(())
    }

    pub fn pop(&mut self, priority: Priority) -> Option<V> {
        let class = PRIORITY_CLASS + u8::from(priority);
        let entry = self.entry_queue.front(class)?;
        self.entry_queue.move_back(FREE_CLASS, entry);
        if self.entry_queue.is_empty(class) {
            self.priority_set.remove(priority);
        }
        self.entry_values[usize::from(entry)].take()
    }

    pub fn priorities(&self) -> PrioritySet {
        self.priority_set
    }

    pub fn max_priority(&self) -> Option<Priority> {
        self.priority_set.last()
    }
}
