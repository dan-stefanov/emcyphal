use emcyphal_core::{Priority, PrioritySet};

use crate::message_ram as ram;

pub const PRIORITY_LEVEL_COUNT: usize = Priority::MAX.into_u8() as usize + 1;

#[derive(Debug, Clone)]
pub struct PriorityMap<V> {
    keys: PrioritySet,
    values: [Option<V>; PRIORITY_LEVEL_COUNT],
}

impl<V> PriorityMap<V> {
    pub const fn new() -> Self {
        Self {
            keys: PrioritySet::NONE,
            values: [const { None }; PRIORITY_LEVEL_COUNT],
        }
    }

    pub fn keys(&self) -> PrioritySet {
        self.keys
    }

    pub fn insert(&mut self, key: Priority, value: V) -> Result<(), V> {
        self.keys.insert(key);
        let slot = &mut self.values[usize::from(key)];
        slot.replace(value).map_or(Ok(()), Err)
    }

    pub fn remove(&mut self, key: Priority) -> Option<V> {
        self.keys.remove(key);
        let slot = &mut self.values[usize::from(key)];
        slot.take()
    }
}

impl<V> Default for PriorityMap<V> {
    fn default() -> Self {
        PriorityMap::new()
    }
}

impl<V> core::ops::Index<Priority> for PriorityMap<V> {
    type Output = V;
    fn index(&self, index: Priority) -> &Self::Output {
        let slot = &self.values[usize::from(index)];
        unwrap!(slot.as_ref())
    }
}

impl<V> core::ops::IndexMut<Priority> for PriorityMap<V> {
    fn index_mut(&mut self, index: Priority) -> &mut Self::Output {
        let slot = &mut self.values[usize::from(index)];
        unwrap!(slot.as_mut())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct TxMailboxIdx(u8);

impl TxMailboxIdx {
    pub const MAX_VALUE: u8 = ram::TX_FIFO_MAX - 1;

    pub const fn new(value: u8) -> Option<Self> {
        if value <= Self::MAX_VALUE {
            Some(Self(value))
        } else {
            None
        }
    }
}

impl From<TxMailboxIdx> for u8 {
    fn from(idx: TxMailboxIdx) -> u8 {
        idx.0
    }
}

impl From<TxMailboxIdx> for usize {
    fn from(idx: TxMailboxIdx) -> usize {
        idx.0.into()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct TxMailboxSet(u8);

impl TxMailboxSet {
    const MASK: u8 = u8::MAX >> (u8::BITS as u8 - (TxMailboxIdx::MAX_VALUE + 1));

    pub const NONE: Self = Self(0);

    pub const ALL: Self = Self(Self::MASK);

    pub fn new_eq(key: TxMailboxIdx) -> Self {
        let mut set = Self::NONE;
        set.insert(key);
        set
    }

    pub fn from_bits_truncating(bits: u8) -> Self {
        Self(bits & Self::MASK)
    }

    pub fn into_bits(self) -> u8 {
        self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0 == Self::NONE.0
    }

    pub fn contains(&self, key: TxMailboxIdx) -> bool {
        (self.0 >> u8::from(key)) & 0x1 != 0
    }

    pub fn first(&self) -> Option<TxMailboxIdx> {
        let idx = self.0.trailing_zeros() as u8;
        TxMailboxIdx::new(idx)
    }

    pub fn insert(&mut self, key: TxMailboxIdx) {
        self.0 |= 1 << u8::from(key);
    }

    pub fn remove(&mut self, key: TxMailboxIdx) {
        self.0 &= !(1 << u8::from(key));
    }
}

impl Default for TxMailboxSet {
    fn default() -> Self {
        Self::NONE
    }
}

impl core::ops::Not for TxMailboxSet {
    type Output = TxMailboxSet;
    fn not(self) -> Self::Output {
        Self::from_bits_truncating(!self.0)
    }
}

impl core::ops::BitAnd for TxMailboxSet {
    type Output = TxMailboxSet;
    fn bitand(self, rhs: Self) -> Self::Output {
        Self(self.0 & rhs.0)
    }
}

impl core::ops::BitOr for TxMailboxSet {
    type Output = TxMailboxSet;
    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl core::iter::IntoIterator for TxMailboxSet {
    type IntoIter = TxMailboxSetIter;
    type Item = TxMailboxIdx;
    fn into_iter(self) -> Self::IntoIter {
        TxMailboxSetIter(self)
    }
}

pub struct TxMailboxSetIter(TxMailboxSet);

impl core::iter::Iterator for TxMailboxSetIter {
    type Item = TxMailboxIdx;
    fn next(&mut self) -> Option<Self::Item> {
        let key = self.0.first()?;
        self.0.remove(key);
        Some(key)
    }
}

#[derive(Debug, Default, Clone)]
pub struct MailboxPriorityMap {
    forward: [Option<Priority>; (TxMailboxIdx::MAX_VALUE + 1) as usize],
    backward: [Option<TxMailboxIdx>; (Priority::MAX.into_u8() + 1) as usize],
}

impl MailboxPriorityMap {
    pub fn insert(&mut self, mailbox: TxMailboxIdx, priority: Priority) -> Result<(), ()> {
        // Check if either key is already in use
        if self.forward[usize::from(mailbox)].is_some()
            || self.backward[usize::from(priority)].is_some()
        {
            return Err(());
        }

        self.forward[usize::from(mailbox)] = Some(priority);
        self.backward[usize::from(priority)] = Some(mailbox);
        Ok(())
    }

    pub fn remove_by_mailbox(&mut self, mailbox: TxMailboxIdx) -> Option<Priority> {
        let priority = self.forward[usize::from(mailbox)].take()?;
        self.backward[usize::from(priority)] = None;
        Some(priority)
    }

    pub fn remove_by_priority(&mut self, priority: Priority) -> Option<TxMailboxIdx> {
        let mailbox = self.backward[usize::from(priority)].take()?;
        self.forward[usize::from(mailbox)] = None;
        Some(mailbox)
    }

    #[allow(dead_code)]
    pub fn get_by_mailbox(&self, mailbox: TxMailboxIdx) -> Option<Priority> {
        self.forward[usize::from(mailbox)]
    }

    pub fn get_by_priority(&self, priority: Priority) -> Option<TxMailboxIdx> {
        self.backward[usize::from(priority)]
    }

    #[allow(dead_code)]
    pub fn contains_mailbox(&self, mailbox: TxMailboxIdx) -> bool {
        self.get_by_mailbox(mailbox).is_some()
    }

    pub fn contains_priority(&self, priority: Priority) -> bool {
        self.get_by_priority(priority).is_some()
    }
}
