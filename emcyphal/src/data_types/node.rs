use crate::core::SubjectId;
use emcyphal_encoding as enc;

/// `node.Heartbeat.1.0`
///
/// Fixed size 7 bytes
///
/// Abstract node status information.
/// This is the only high-level function that shall be implemented by all nodes.
///
/// All Cyphal nodes that have a node-ID are required to publish this message to its fixed subject periodically.
/// Nodes that do not have a node-ID (also known as "anonymous nodes") shall not publish to this subject.
///
/// The default subject-ID 7509 is 1110101010101 in binary. The alternating bit pattern at the end helps transceiver
/// synchronization (e.g., on CAN-based networks) and on some transports permits automatic bit rate detection.
///
/// Network-wide health monitoring can be implemented by subscribing to the fixed subject.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Heartbeat {
    /// \[second\]
    /// The uptime seconds counter should never overflow. The counter will reach the upper limit in ~136 years,
    /// upon which time it should stay at 0xFFFFFFFF until the node is restarted.
    /// Other nodes may detect that a remote node has restarted when this value leaps backwards.
    ///
    /// `saturated uint32`
    ///
    /// Always aligned,
    /// size 32 bits
    pub uptime: u32,
    /// The abstract health status of this node.
    ///
    /// `node.Health.1.0`
    ///
    /// Always aligned,
    /// size 8 bits
    pub health: Health,
    /// The abstract operating mode of the publishing node.
    /// This field indicates the general level of readiness that can be further elaborated on a per-activity basis
    /// using various specialized interfaces.
    ///
    /// `node.Mode.1.0`
    ///
    /// Always aligned,
    /// size 8 bits
    pub mode: Mode,
    /// Optional, vendor-specific node status code, e.g. a fault code or a status bitmask.
    /// Fits into a single-frame Classic CAN transfer (least capable transport, smallest MTU).
    ///
    /// `saturated uint8`
    ///
    /// Always aligned,
    /// size 8 bits
    pub vendor_specific_status_code: u8,
}
impl enc::DataType for Heartbeat {
    /// This type is delimited with an extent of 12 bytes.
    const EXTENT_BYTES: Option<u32> = Some(12);
}
impl enc::Message for Heartbeat {}
impl enc::BufferType for Heartbeat {
    type Buffer = enc::StaticBuffer<7>;
}
impl Heartbeat {
    /// The fixed subject ID for this message type
    pub const SUBJECT: SubjectId = SubjectId::new(7509).unwrap();

    /// \[second\]
    /// The publication period shall not exceed this limit.
    /// The period should not change while the node is running.
    pub const MAX_PUBLICATION_PERIOD: u16 = 1;
    /// \[second\]
    /// If the last message from the node was received more than this amount of time ago, it should be considered offline.
    #[allow(dead_code)]
    pub const OFFLINE_TIMEOUT: u16 = 3;
}
impl enc::Serialize for Heartbeat {
    fn size_bits(&self) -> usize {
        56
    }
    fn serialize(&self, cursor: &mut enc::WriteCursor<'_>) {
        cursor.write_aligned_u32(self.uptime);
        cursor.write_composite(&self.health);
        cursor.write_composite(&self.mode);
        cursor.write_aligned_u8(self.vendor_specific_status_code);
    }
}
impl enc::Deserialize for Heartbeat {
    fn deserialize(
        cursor: &mut enc::ReadCursor<'_>,
    ) -> ::core::result::Result<Self, enc::DeserializeError>
    where
        Self: Sized,
    {
        Ok(Heartbeat {
            uptime: { cursor.read_u32() as _ },
            health: { cursor.read_composite()? },
            mode: { cursor.read_composite()? },
            vendor_specific_status_code: { cursor.read_u8() as _ },
        })
    }
}

/// `node.Health.1.0`
///
/// Fixed size 1 bytes
///
/// Abstract component health information. If the node performs multiple activities (provides multiple network services),
/// its health status should reflect the status of the worst-performing activity (network service).
/// Follows:
///   <https://www.law.cornell.edu/cfr/text/14/23.1322>
///   <https://www.faa.gov/documentLibrary/media/Advisory_Circular/AC_25.1322-1.pdf> section 6
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Health {
    /// The component is functioning properly (nominal).
    Nominal,
    /// A critical parameter went out of range or the component encountered a minor failure that does not prevent
    /// the subsystem from performing any of its real-time functions.
    Advisory,
    /// The component encountered a major failure and is performing in a degraded mode or outside of its designed limitations.
    Caution,
    /// The component suffered a fatal malfunction and is unable to perform its intended function.
    Warning,
}
impl enc::DataType for Health {
    /// This type is sealed.
    const EXTENT_BYTES: Option<u32> = None;
}
impl enc::Message for Health {}
impl Health {}
impl enc::Serialize for Health {
    fn size_bits(&self) -> usize {
        8
    }
    fn serialize(&self, cursor: &mut enc::WriteCursor<'_>) {
        match self {
            Health::Nominal => {
                cursor.write_u2(0);
            }
            Health::Advisory => {
                cursor.write_u2(1);
            }
            Health::Caution => {
                cursor.write_u2(2);
            }
            Health::Warning => {
                cursor.write_u2(3);
            }
        }
    }
}
impl enc::Deserialize for Health {
    fn deserialize(
        cursor: &mut enc::ReadCursor<'_>,
    ) -> ::core::result::Result<Self, enc::DeserializeError>
    where
        Self: Sized,
    {
        match cursor.read_u2() as _ {
            0 => Ok(Health::Nominal),
            1 => Ok(Health::Advisory),
            2 => Ok(Health::Caution),
            3 => Ok(Health::Warning),
            _ => Err(enc::DeserializeError::UnionTag),
        }
    }
}

/// `node.Mode.1.0`
///
/// Fixed size 1 bytes
///
/// The operating mode of a node.
/// Reserved values can be used in future revisions of the specification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Mode {
    /// Normal operating mode.
    Operational,
    /// Initialization is in progress; this mode is entered immediately after startup.
    Initialization,
    /// E.g., calibration, self-test, etc.
    Maintenance,
    /// New software/firmware is being loaded or the bootloader is running.
    SoftwareUpdate,
}
impl enc::DataType for Mode {
    /// This type is sealed.
    const EXTENT_BYTES: Option<u32> = None;
}
impl enc::Message for Mode {}
impl enc::BufferType for Mode {
    type Buffer = enc::StaticBuffer<1>;
}
impl Mode {}
impl enc::Serialize for Mode {
    fn size_bits(&self) -> usize {
        8
    }
    fn serialize(&self, cursor: &mut enc::WriteCursor<'_>) {
        match self {
            Mode::Operational => {
                cursor.write_u3(0);
            }
            Mode::Initialization => {
                cursor.write_u3(1);
            }
            Mode::Maintenance => {
                cursor.write_u3(2);
            }
            Mode::SoftwareUpdate => {
                cursor.write_u3(3);
            }
        }
    }
}
impl enc::Deserialize for Mode {
    fn deserialize(
        cursor: &mut enc::ReadCursor<'_>,
    ) -> ::core::result::Result<Self, enc::DeserializeError>
    where
        Self: Sized,
    {
        match cursor.read_u3() as _ {
            0 => Ok(Mode::Operational),
            1 => Ok(Mode::Initialization),
            2 => Ok(Mode::Maintenance),
            3 => Ok(Mode::SoftwareUpdate),
            _ => Err(enc::DeserializeError::UnionTag),
        }
    }
}
