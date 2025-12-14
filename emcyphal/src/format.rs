use emcyphal_driver::frame::Frame;

use crate::core::TransferId;

#[derive(Debug, Clone, Copy)]
pub struct TransferCrc(u16);

impl Default for TransferCrc {
    fn default() -> Self {
        Self(Self::INIT_VALUE)
    }
}

impl TransferCrc {
    pub const LENGTH: usize = 2;
    const INIT_VALUE: u16 = 0xffff;
    const POLYNOMIAL: u16 = 0x1021;

    pub fn add(&mut self, byte: u8) {
        self.0 ^= u16::from(byte) << 8;
        for _bit in 0..8 {
            if (self.0 & 0x8000) != 0 {
                self.0 = (self.0 << 1) ^ Self::POLYNOMIAL;
            } else {
                self.0 <<= 1;
            }
        }
    }

    pub fn add_bytes(&mut self, bytes: &[u8]) {
        bytes.iter().for_each(|&byte| self.add(byte));
    }

    pub fn get(&self) -> u16 {
        self.0
    }
}

impl From<u16> for TransferCrc {
    fn from(value: u16) -> Self {
        Self(value)
    }
}

#[derive(Clone, Copy, Default)]
pub struct TailByte(u8);

impl TailByte {
    const START_OF_TRANSFER: u8 = 7;
    const END_OF_TRANSFER: u8 = 6;
    const TOGGLE_BIT: u8 = 5;
    const TRANSFER_ID: u8 = 0;

    pub fn new(sot: bool, eof: bool, toggle: bool, transfer_id: TransferId) -> Self {
        Self(
            (sot as u8) << Self::START_OF_TRANSFER
                | (eof as u8) << Self::END_OF_TRANSFER
                | (toggle as u8) << Self::TOGGLE_BIT
                | u8::from(transfer_id) << Self::TRANSFER_ID,
        )
    }

    pub fn sot(&self) -> bool {
        (self.0 >> Self::START_OF_TRANSFER) & 0x1 != 0
    }

    pub fn eot(&self) -> bool {
        (self.0 >> Self::END_OF_TRANSFER) & 0x1 != 0
    }

    pub fn toggle(&self) -> bool {
        (self.0 >> Self::TOGGLE_BIT) & 0x1 != 0
    }

    pub fn transfer_id(&self) -> TransferId {
        TransferId::from_truncating(self.0 >> Self::TRANSFER_ID)
    }
}

impl From<TailByte> for u8 {
    fn from(value: TailByte) -> Self {
        value.0
    }
}

impl From<u8> for TailByte {
    fn from(value: u8) -> Self {
        Self(value)
    }
}

/// Toggle bit value for start-of-transfer frame [1; table 4.4]
pub const SOT_TOGGLE_BIT: bool = true;

pub const PAD_VALUE: u8 = 0;

pub fn is_eot_frame(frame: &Frame) -> Option<bool> {
    let byte = *frame.data.last()?;
    Some(TailByte::from(byte).eot())
}
