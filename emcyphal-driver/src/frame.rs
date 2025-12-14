//! Transport frame object

use emcyphal_core::{NodeId, Priority, ServiceId, SubjectId};

use crate::time::Instant;

/// A transport-layer maximum transmission unit
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Mtu {
    Classic,
    Fd,
}

impl From<Mtu> for usize {
    fn from(value: Mtu) -> Self {
        match value {
            Mtu::Classic => 8,
            Mtu::Fd => 64,
        }
    }
}

#[derive(Debug)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct IncorrectMtu;

impl TryFrom<usize> for Mtu {
    type Error = IncorrectMtu;
    fn try_from(value: usize) -> Result<Self, Self::Error> {
        match value {
            8 => Ok(Mtu::Classic),
            64 => Ok(Mtu::Fd),
            _ => Err(IncorrectMtu),
        }
    }
}

/// Encodes the semantic properties of the data type carried by a transfer and its kind
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum DataSpecifier {
    Message(SubjectId),
    Request(ServiceId),
    Response(ServiceId),
}

/// Transport frame data encoded with CAN frame ID
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Header {
    pub priority: Priority,
    pub data_spec: DataSpecifier,
    pub source: Option<NodeId>,
    pub destination: Option<NodeId>,
}

/// Transport frame for both Classic and FD transport
///
/// The data length should be limited to the relevant MTU.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Frame {
    pub header: Header,
    pub data: Data,
    pub timestamp: Instant,
    pub loop_back: bool,
}

/// CAN-FD-compatible data length
///
/// Data length code (DLC) of CAN-FD frames supports limited data length options.
/// Classic CAN frames support a subset of CAN-FD length options limited by MTU.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct DataLength(u8);

impl DataLength {
    pub const MAX: usize = 64;

    pub const fn new(value: usize) -> Option<Self> {
        let floor = Self::new_floor(value);
        if floor.as_usize() == value {
            Some(floor)
        } else {
            None
        }
    }

    pub const fn new_floor(value: usize) -> Self {
        let floor = match value {
            0..8 => value,
            8..24 => value / 4 * 4,
            24..32 => value / 8 * 8,
            32..64 => value / 16 * 16,
            64.. => 64,
        };
        Self(floor as u8)
    }

    pub const fn new_ceil(value: usize) -> Option<Self> {
        if value <= Self::MAX {
            let ceil = match value {
                0..8 => value,
                8..24 => value.div_ceil(4) * 4,
                24..32 => value.div_ceil(8) * 8,
                32.. => value.div_ceil(16) * 16,
            };
            Some(Self(ceil as u8))
        } else {
            None
        }
    }

    pub const fn as_usize(&self) -> usize {
        self.0 as usize
    }
}

impl From<DataLength> for usize {
    fn from(value: DataLength) -> Self {
        value.as_usize()
    }
}

#[derive(Debug)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct InvalidLength;

/// CAN-FD Frame compatible data vector
///
/// Data length code (DLC) of CAN-FD frames supports limited data length options.
/// Classic CAN frames support a subset of CAN-FD length options limited by MTU.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Data {
    length: DataLength,
    bytes: [u8; 64],
}

impl Data {
    /// Creates a new vector from a slice of compatible length.
    pub fn new(data: &[u8]) -> Result<Self, InvalidLength> {
        let length = DataLength::new(data.len()).ok_or(InvalidLength)?;
        let mut bytes = [0; 64];
        bytes[..data.len()].copy_from_slice(data);

        Ok(Self { length, bytes })
    }

    pub fn new_zeros(length: DataLength) -> Self {
        Self {
            length,
            bytes: [0; 64],
        }
    }

    pub fn length(&self) -> DataLength {
        self.length
    }
}

impl core::ops::Deref for Data {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.bytes[..usize::from(self.length)]
    }
}

impl core::ops::DerefMut for Data {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.bytes[..usize::from(self.length)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_CAN_LENGTH: [usize; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 12, 16, 20, 24, 32, 48, 64];

    fn ceil_length_ref(value: usize) -> Option<usize> {
        match VALID_CAN_LENGTH.binary_search(&value) {
            Ok(pos) => Some(VALID_CAN_LENGTH[pos]),
            Err(pos) => {
                if pos < VALID_CAN_LENGTH.len() {
                    Some(VALID_CAN_LENGTH[pos])
                } else {
                    None
                }
            }
        }
    }

    fn floor_length_ref(value: usize) -> usize {
        match VALID_CAN_LENGTH.binary_search(&value) {
            Ok(pos) => VALID_CAN_LENGTH[pos],
            Err(pos) => VALID_CAN_LENGTH[pos - 1],
        }
    }

    #[test]
    fn test_frame_length() {
        for len in 0usize..100 {
            assert_eq!(
                usize::from(DataLength::new_floor(len)),
                floor_length_ref(len)
            );
            assert_eq!(
                DataLength::new_ceil(len).map(|len| usize::from(len)),
                ceil_length_ref(len)
            );
        }
    }
}
