use emcyphal_core::{Priority, PrioritySet};
use heapless::Deque;

pub const PRIORITY_LEVEL_COUNT: usize = Priority::MAX_VALUE as usize + 1;

pub struct PriorityDeque<T, const N: usize> {
    queues: [Deque<T, N>; PRIORITY_LEVEL_COUNT],
    priorities: PrioritySet,
}

impl<T, const N: usize> Default for PriorityDeque<T, N> {
    fn default() -> Self {
        Self {
            queues: core::array::from_fn(|_| Default::default()),
            priorities: PrioritySet::NONE,
        }
    }
}

impl<T, const N: usize> PriorityDeque<T, N> {
    pub fn priorities(&self) -> PrioritySet {
        self.priorities
    }

    pub fn front(&self, priority: Priority) -> Option<&T> {
        self.queues[usize::from(u8::from(priority))].front()
    }

    pub fn pop_front(&mut self, priority: Priority) -> Option<T> {
        let queue = &mut self.queues[usize::from(u8::from(priority))];
        let value = queue.pop_front();
        if queue.is_empty() {
            self.priorities.remove(priority);
        }
        value
    }

    pub fn pop_back(&mut self, priority: Priority) -> Option<T> {
        let queue = &mut self.queues[usize::from(u8::from(priority))];
        let value = queue.pop_back();
        if queue.is_empty() {
            self.priorities.remove(priority);
        }
        value
    }

    pub fn push_front(&mut self, priority: Priority, item: T) -> Result<(), T> {
        let queue = &mut self.queues[usize::from(u8::from(priority))];
        let res = queue.push_front(item);
        self.priorities.insert(priority);
        res
    }

    pub fn push_back(&mut self, priority: Priority, item: T) -> Result<(), T> {
        let queue = &mut self.queues[usize::from(u8::from(priority))];
        let res = queue.push_back(item);
        self.priorities.insert(priority);
        res
    }
}

pub struct PriorityArray<T>([T; PRIORITY_LEVEL_COUNT]);

impl<T: Copy> PriorityArray<T> {
    pub fn repeat(val: T) -> Self {
        Self(core::array::repeat(val))
    }
}

impl<T: Default> Default for PriorityArray<T> {
    fn default() -> Self {
        Self(core::array::from_fn(|_| Default::default()))
    }
}

impl<T> core::ops::Index<Priority> for PriorityArray<T> {
    type Output = T;

    fn index(&self, index: Priority) -> &Self::Output {
        &self.0[usize::from(u8::from(index))]
    }
}

impl<T> core::ops::IndexMut<Priority> for PriorityArray<T> {
    fn index_mut(&mut self, index: Priority) -> &mut Self::Output {
        &mut self.0[usize::from(u8::from(index))]
    }
}

impl<T> core::ops::Deref for PriorityArray<T> {
    type Target = [T];
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
