use crate::utils::DuplexArray;
use crate::utils::multi_class_queue::MultiClassQueue;

const QUEUE_CLASS: u8 = 0;
const FREE_CLASS: u8 = QUEUE_CLASS + 1;
const CLASS_COUNT: usize = FREE_CLASS as usize + 1;

pub const MAX_CAPACITY: usize = u8::MAX as usize + 1 - CLASS_COUNT;

#[derive(Debug)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct NoKey;

const NO_ENTRY: u8 = u8::MAX;

pub struct IndexQueue<K, V, const K_MAX: usize, const N: usize> {
    keys: [Option<K>; N],
    values: [Option<V>; N],
    index: DuplexArray<u8, 1, K_MAX>,
    queue: MultiClassQueue<CLASS_COUNT, N>,
}

impl<K: Copy + Into<usize>, V, const K_MAX: usize, const N: usize> IndexQueue<K, V, K_MAX, N> {
    const _ASSERT: usize = MAX_CAPACITY - N;

    pub fn new() -> Self {
        let mut queue = MultiClassQueue::new();
        for i in 0..N {
            let entry = unwrap!(u8::try_from(i));
            queue.move_back(FREE_CLASS, entry);
        }

        Self {
            keys: core::array::from_fn(|_| None),
            values: core::array::from_fn(|_| None),
            index: DuplexArray::repeat(NO_ENTRY),
            queue,
        }
    }

    pub fn contains(&self, key: K) -> bool {
        self.get_index(key).is_ok()
    }

    pub fn get(&self, key: K) -> Option<&V> {
        let idx = self.get_index(key).ok()?;
        Some(unwrap!(self.values[usize::from(idx)].as_ref()))
    }

    pub fn get_mut(&mut self, key: K) -> Option<&mut V> {
        let idx = self.get_index(key).ok()?;
        Some(unwrap!(self.values[usize::from(idx)].as_mut()))
    }

    pub fn insert(&mut self, key: K, value: V) -> Result<Option<V>, (K, V)> {
        if key.into() > K_MAX {
            return Err((key, value));
        }

        if let Ok(idx) = self.get_index(key) {
            let value = unwrap!(self.values[usize::from(idx)].take());
            return Ok(Some(value));
        }

        let idx = match self.queue.front(FREE_CLASS) {
            Some(idx) => idx,
            None => return Err((key, value)),
        };
        self.queue.remove(idx);

        self.keys[usize::from(idx)] = Some(key);
        self.values[usize::from(idx)] = Some(value);
        self.index[key.into()] = idx;
        Ok(None)
    }

    pub fn remove(&mut self, key: K) -> Option<V> {
        let idx = self.get_index(key).ok()?;
        self.index[key.into()] = NO_ENTRY;
        self.queue.move_back(FREE_CLASS, idx);
        self.keys[usize::from(idx)] = None;
        Some(unwrap!(self.values[usize::from(idx)].take()))
    }

    pub fn front(&self) -> Option<K> {
        let idx = self.queue.front(QUEUE_CLASS)?;
        self.keys[usize::from(idx)]
    }

    pub fn back(&self) -> Option<K> {
        let idx = self.queue.back(QUEUE_CLASS)?;
        self.keys[usize::from(idx)]
    }

    pub fn enqueue_front(&mut self, key: K) -> Result<(), NoKey> {
        let idx = self.get_index(key)?;
        self.queue.move_front(QUEUE_CLASS, idx);
        Ok(())
    }

    pub fn enqueue_back(&mut self, key: K) -> Result<(), NoKey> {
        let idx = self.get_index(key)?;
        self.queue.move_back(QUEUE_CLASS, idx);
        Ok(())
    }

    pub fn dequeue(&mut self, key: K) -> Result<(), NoKey> {
        let idx = self.get_index(key)?;
        self.queue.remove(idx);
        Ok(())
    }

    fn get_index(&self, key: K) -> Result<u8, NoKey> {
        if key.into() > K_MAX {
            return Err(NoKey);
        }
        let idx = self.index[key.into()];
        if idx != NO_ENTRY { Ok(idx) } else { Err(NoKey) }
    }
}
