use core::task::Waker;
use embassy_sync::waitqueue::WakerRegistration;

use crate::core::{Priority, PrioritySet};

mod index_queue;
mod multi_class_queue;
mod priority_queue;
mod stacked_array;

pub use index_queue::IndexQueue;
pub use priority_queue::PriorityQueue;
pub use stacked_array::{DuplexArray, TriplexArray};

const PRIORITY_LEVEL_COUNT: usize = Priority::MAX.into_u8() as usize + 1;

#[derive(Default)]
pub struct PriorityArray<T>([T; PRIORITY_LEVEL_COUNT]);

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

#[derive(Debug, Clone)]
pub struct PriorityMap<V> {
    set: PrioritySet,
    values: [Option<V>; PRIORITY_LEVEL_COUNT],
}

impl<V> PriorityMap<V> {
    pub const fn new() -> Self {
        Self {
            set: PrioritySet::NONE,
            values: [const { None }; PRIORITY_LEVEL_COUNT],
        }
    }

    pub fn keys(&self) -> PrioritySet {
        self.set
    }

    pub fn is_empty(&self) -> bool {
        self.set.is_empty()
    }

    pub fn insert(&mut self, key: Priority, value: V) -> Option<V> {
        self.set.insert(key);
        let slot = &mut self.values[usize::from(u8::from(key))];
        slot.replace(value)
    }

    pub fn remove(&mut self, key: Priority) -> Option<V> {
        self.set.remove(key);
        let slot = &mut self.values[usize::from(u8::from(key))];
        slot.take()
    }
}

impl<V> core::ops::Index<Priority> for PriorityMap<V> {
    type Output = V;
    fn index(&self, index: Priority) -> &Self::Output {
        unwrap!(self.values[usize::from(u8::from(index))].as_ref())
    }
}

impl<V> core::ops::IndexMut<Priority> for PriorityMap<V> {
    fn index_mut(&mut self, index: Priority) -> &mut Self::Output {
        unwrap!(self.values[usize::from(u8::from(index))].as_mut())
    }
}

pub struct PriorityTrigger {
    waker: WakerRegistration,
    filter: PrioritySet,
}

impl PriorityTrigger {
    pub const fn new() -> Self {
        Self {
            waker: WakerRegistration::new(),
            filter: PrioritySet::NONE,
        }
    }

    pub fn register(&mut self, w: &Waker, filter: PrioritySet) {
        self.waker.register(w);
        self.filter = filter;
    }

    pub fn wake(&mut self, priorities: PrioritySet) {
        if !(priorities & self.filter).is_empty() {
            self.waker.wake();
        }
    }
}

impl Default for PriorityTrigger {
    fn default() -> Self {
        PriorityTrigger::new()
    }
}
