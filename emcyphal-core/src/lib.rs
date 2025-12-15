//! Cyphal protocol core data types
//!
//! This crate provides basic data type definitions used by other Emcyphal crates.
//! Emcyphal users should not depend on this crate directly. Use `emcyphal::core` reexport instead.
#![no_std]

#[derive(Debug)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct InvalidValue;

/// Transfer priority [1; 4.1.1.3]
///
/// The type has explicit numeric encoding to facilitate look-up table implementation.
/// The encoding matches the CAN ID encoding [1; 4.2.1.1], thus the ordering is reversed:
/// Optional > Exceptional
///
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[repr(u8)]
pub enum Priority {
    /// The bus designer can ignore these messages when calculating bus load since they should
    /// only be sent when a total system failure has occurred. For example, a self-destruct message
    /// on a rocket would use this priority. Another analogy is an NMI on a microcontroller.
    Exceptional = 0,
    /// Immediate is a "high priority message" but with additional latency constraints. Since
    /// exceptional messages are not considered when designing a bus, the latency of immediate
    /// messages can be determined by considering only immediate messages.
    Immediate = 1,
    /// Fast and immediate are both "high priority messages" but with additional latency
    /// constraints. Since exceptional messages are not considered when designing a bus, the latency
    /// of fast messages can be determined by considering only immediate and fast messages.
    Fast = 2,
    /// High priority messages are more important than nominal messages but have looser latency
    /// requirements than fast messages. This priority is used so that, in the presence of rogue
    /// nominal messages, important commands can be received. For example, one might envision a
    /// failure mode where a temperature sensor starts to load a vehicle bus with nominal messages.
    /// The vehicle remains operational (for a time) because the controller is exchanging fast and
    /// immediate messages with sensors and actuators. A system safety monitor is able to detect the
    /// distressed bus and command the vehicle to a safe state by sending high priority messages to
    /// the controller.
    High = 3,
    /// This is what all messages should use by default. Specifically, heartbeat messages should
    /// use this priority.
    Nominal = 4,
    /// Low priority messages are expected to be sent on a bus under all conditions but cannot
    /// prevent the delivery of nominal messages. They are allowed to be delayed, but latency should
    /// be constrained by the bus designer.
    Low = 5,
    /// Slow messages are low priority messages that have no time sensitivity at all. The bus
    /// designer need only ensure that for all possible system states, these messages will
    /// eventually be sent.
    Slow = 6,
    /// These messages might never be sent (theoretically) for some possible system states. The
    /// system shall tolerate never exchanging optional messages in every possible state. The bus
    /// designer can ignore these messages when calculating bus load. This should be the priority
    /// used for diagnostic or debug messages that are not required on an operational system.
    Optional = 7,
}

impl Priority {
    pub const MIN: Priority = Priority::Exceptional;
    pub const MAX: Priority = Priority::Optional;

    pub const fn try_from_u8(code: u8) -> Option<Priority> {
        if code <= Self::MAX.into_u8() {
            Some(Priority::from_u8_truncating(code))
        } else {
            None
        }
    }

    pub const fn from_u8_truncating(code: u8) -> Priority {
        match code & 0x7 {
            0 => Priority::Exceptional,
            1 => Priority::Immediate,
            2 => Priority::Fast,
            3 => Priority::High,
            4 => Priority::Nominal,
            5 => Priority::Low,
            6 => Priority::Slow,
            7 => Priority::Optional,
            _ => unreachable!(),
        }
    }

    pub const fn into_u8(self) -> u8 {
        self as u8
    }

    pub const fn next(self) -> Option<Self> {
        Self::try_from_u8(self.into_u8() + 1)
    }

    pub const fn prev(self) -> Option<Self> {
        if let Some(code) = self.into_u8().checked_sub(1) {
            Some(Self::from_u8_truncating(code))
        } else {
            None
        }
    }
}

impl From<Priority> for u8 {
    fn from(value: Priority) -> Self {
        value.into_u8()
    }
}

impl From<Priority> for usize {
    fn from(value: Priority) -> Self {
        u8::from(value).into()
    }
}

impl TryFrom<u8> for Priority {
    type Error = InvalidValue;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Self::try_from_u8(value).ok_or(InvalidValue)
    }
}

/// A set of priority values
///
/// Note that higher priority has a lower numerical value and is ordered first.
/// Methods are named according to numerical priority values, e.g., `new_ge(Priority::Nominal)`
/// returns a set containing `Nominal`, `Low`, `Slow`, and `Optional`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct PrioritySet(u8);

impl PrioritySet {
    pub const NONE: Self = Self(0);
    pub const ALL: Self = Self(u8::MAX);

    pub const fn from_bits(bits: u8) -> Self {
        Self(bits)
    }

    pub const fn into_bits(self) -> u8 {
        self.0
    }

    pub const fn complement(self) -> Self {
        Self(!self.0)
    }

    pub const fn new_eq(priority: Priority) -> Self {
        Self(1u8 << priority.into_u8())
    }

    pub const fn new_ge(priority: Priority) -> Self {
        Self(u8::MAX << priority.into_u8())
    }

    pub const fn new_le(priority: Priority) -> Self {
        Self(u8::MAX >> (Priority::MAX.into_u8() - priority.into_u8()))
    }

    pub const fn new_gt(priority: Priority) -> Self {
        Self::new_le(priority).complement()
    }

    pub const fn new_lt(priority: Priority) -> Self {
        Self::new_ge(priority).complement()
    }

    pub const fn contains(&self, priority: Priority) -> bool {
        (self.0 >> priority.into_u8()) & 0x1 != 0
    }

    pub const fn insert(&mut self, priority: Priority) {
        self.0 |= Self::new_eq(priority).0
    }

    pub const fn remove(&mut self, priority: Priority) {
        self.0 &= Self::new_eq(priority).complement().0
    }

    pub const fn first(&self) -> Option<Priority> {
        Priority::try_from_u8(self.0.trailing_zeros() as u8)
    }

    pub const fn last(&self) -> Option<Priority> {
        let n = u8::BITS - self.0.leading_zeros();
        Priority::try_from_u8((n as u8).wrapping_sub(1))
    }

    pub const fn is_empty(&self) -> bool {
        self.0 == Self::NONE.0
    }
}

impl Default for PrioritySet {
    fn default() -> Self {
        PrioritySet::NONE
    }
}

impl core::ops::Not for PrioritySet {
    type Output = Self;
    fn not(self) -> Self::Output {
        Self(!self.0)
    }
}

impl core::ops::BitAnd<PrioritySet> for PrioritySet {
    type Output = Self;
    fn bitand(self, rhs: PrioritySet) -> Self::Output {
        PrioritySet(self.0 & rhs.0)
    }
}

impl core::ops::BitAndAssign<PrioritySet> for PrioritySet {
    fn bitand_assign(&mut self, rhs: PrioritySet) {
        self.0 &= rhs.0
    }
}

impl core::ops::BitOr<PrioritySet> for PrioritySet {
    type Output = Self;
    fn bitor(self, rhs: PrioritySet) -> Self::Output {
        PrioritySet(self.0 | rhs.0)
    }
}

impl core::ops::BitOrAssign<PrioritySet> for PrioritySet {
    fn bitor_assign(&mut self, rhs: PrioritySet) {
        self.0 |= rhs.0;
    }
}

impl core::iter::IntoIterator for PrioritySet {
    type Item = Priority;
    type IntoIter = PrioritySetIterator;
    fn into_iter(self) -> Self::IntoIter {
        PrioritySetIterator { residual: self }
    }
}

pub struct PrioritySetIterator {
    residual: PrioritySet,
}

impl core::iter::Iterator for PrioritySetIterator {
    type Item = Priority;
    fn next(&mut self) -> Option<Self::Item> {
        let first = self.residual.first();
        if let Some(priority) = first {
            self.residual.remove(priority);
        }
        first
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct NodeId(u8);

impl NodeId {
    const MAX_VALUE: u8 = 0x7f;
    pub const MAX: NodeId = NodeId(0x7f);

    pub const fn new(value: u8) -> Option<Self> {
        if value <= Self::MAX_VALUE {
            Some(Self::from_u8_truncating(value))
        } else {
            None
        }
    }

    pub const fn from_u8_truncating(value: u8) -> Self {
        Self(value & Self::MAX_VALUE)
    }

    pub const fn into_u8(self) -> u8 {
        self.0
    }
}

impl From<NodeId> for u8 {
    fn from(value: NodeId) -> Self {
        value.into_u8()
    }
}

impl From<NodeId> for usize {
    fn from(value: NodeId) -> Self {
        u8::from(value).into()
    }
}

impl TryFrom<u8> for NodeId {
    type Error = InvalidValue;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Self::new(value).ok_or(InvalidValue)
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct SubjectId(u16);

impl SubjectId {
    const MAX_VALUE: u16 = 0x1fff;
    pub const MAX: SubjectId = SubjectId(Self::MAX_VALUE);

    pub const fn new(value: u16) -> Option<Self> {
        if value <= Self::MAX_VALUE {
            Some(Self::from_u16_truncating(value))
        } else {
            None
        }
    }

    pub const fn from_u16_truncating(value: u16) -> Self {
        Self(value & Self::MAX_VALUE)
    }

    pub const fn into_u16(self) -> u16 {
        self.0
    }
}

impl From<SubjectId> for u16 {
    fn from(value: SubjectId) -> Self {
        value.into_u16()
    }
}

impl TryFrom<u16> for SubjectId {
    type Error = InvalidValue;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        Self::new(value).ok_or(InvalidValue)
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct ServiceId(u16);

impl ServiceId {
    const MAX_VALUE: u16 = 0x1ff;
    pub const MAX: SubjectId = SubjectId(Self::MAX_VALUE);

    pub const fn new(value: u16) -> Option<Self> {
        if value <= Self::MAX_VALUE {
            Some(Self::from_u16_truncating(value))
        } else {
            None
        }
    }

    pub const fn from_u16_truncating(value: u16) -> Self {
        Self(value & Self::MAX_VALUE)
    }

    pub const fn into_u16(self) -> u16 {
        self.0
    }
}

impl From<ServiceId> for u16 {
    fn from(value: ServiceId) -> Self {
        value.into_u16()
    }
}

impl TryFrom<u16> for ServiceId {
    type Error = InvalidValue;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        Self::new(value).ok_or(InvalidValue)
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct TransferId(u8);

impl TransferId {
    const MAX_VALUE: u8 = 0x1f;
    pub const MAX: TransferId = TransferId(Self::MAX_VALUE);

    /// TransferId of the first transfer in the session, see [1; 4.1.1.7]
    pub const SESSION_START: TransferId = TransferId(0);

    pub const fn new(value: u8) -> Option<Self> {
        if value <= Self::MAX_VALUE {
            Some(Self::from_u8_truncating(value))
        } else {
            None
        }
    }

    pub const fn from_u8_truncating(value: u8) -> Self {
        Self(value & Self::MAX_VALUE)
    }

    pub const fn into_u8(self) -> u8 {
        self.0
    }

    pub fn next(self) -> Self {
        Self((self.0 + 1) & Self::MAX.0)
    }
}

impl Default for TransferId {
    fn default() -> Self {
        Self::SESSION_START
    }
}

impl From<TransferId> for u8 {
    fn from(value: TransferId) -> Self {
        value.into_u8()
    }
}

impl TryFrom<u8> for TransferId {
    type Error = InvalidValue;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Self::new(value).ok_or(InvalidValue)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_priority_set() {
        let mut set = PrioritySet::NONE;
        set.insert(Priority::Fast);
        set.insert(Priority::Nominal);

        assert_eq!(set.first(), Some(Priority::Fast));
        assert_eq!(set.last(), Some(Priority::Nominal));
    }

    #[test]
    fn test_priority_set_ge() {
        let set = PrioritySet::new_ge(Priority::Optional);
        assert!(!set.contains(Priority::Slow));
        assert!(set.contains(Priority::Optional));

        let set = PrioritySet::new_ge(Priority::Exceptional);
        assert_eq!(set, PrioritySet::ALL);
    }

    #[test]
    fn test_priority_set_le() {
        let set = PrioritySet::new_le(Priority::Optional);
        assert_eq!(set, PrioritySet::ALL);

        let set = PrioritySet::new_le(Priority::Exceptional);
        assert!(set.contains(Priority::Exceptional));
        assert!(!set.contains(Priority::Immediate));
    }

    #[test]
    fn test_priority_set_gt() {
        let set = PrioritySet::new_gt(Priority::Optional);
        assert_eq!(set, PrioritySet::NONE);

        let set = PrioritySet::new_gt(Priority::Exceptional);
        assert!(!set.contains(Priority::Exceptional));
        assert!(set.contains(Priority::Immediate));
    }

    #[test]
    fn test_priority_set_lt() {
        let set = PrioritySet::new_lt(Priority::Optional);
        assert!(set.contains(Priority::Slow));
        assert!(!set.contains(Priority::Optional));

        let set = PrioritySet::new_lt(Priority::Exceptional);
        assert_eq!(set, PrioritySet::NONE);
    }
}
