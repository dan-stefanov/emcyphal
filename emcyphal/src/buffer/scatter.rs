use crate::core::TransferId;
use crate::format::{PAD_VALUE, SOT_TOGGLE_BIT, TailByte, TransferCrc};
use crate::frame::{Data, DataLength, Mtu};
use core::cmp::{max, min};

pub struct Scatter {
    transfer_id: TransferId,
    toggle_bit: bool,
    data_length: usize,
    offset: usize,
    crc: TransferCrc,
}

// TODO: consider Iter trait
impl Scatter {
    const CRC_LENGTH: usize = TransferCrc::LENGTH;
    const TAIL_LENGTH: usize = 1;

    pub fn new(
        transfer_id: TransferId,
        _buffer: &[u8],
        data_length: usize,
        crc: TransferCrc,
    ) -> Self {
        Self {
            transfer_id,
            toggle_bit: SOT_TOGGLE_BIT,
            data_length,
            offset: 0,
            crc,
        }
    }

    pub fn is_exhausted(&self) -> bool {
        self.offset == self.data_length + Self::CRC_LENGTH
    }

    pub fn fetch_frame_data(&mut self, buffer: &[u8], mtu: Mtu) -> Option<Data> {
        let max_payload_length = usize::from(mtu) - Self::TAIL_LENGTH;

        // single frame transfer
        if self.offset == 0 && self.data_length <= max_payload_length {
            let frame_length = unwrap!(DataLength::new_ceil(self.data_length + Self::TAIL_LENGTH));
            let mut frame_data = Data::new_zeros(frame_length);
            let (tail, payload) = unwrap!(frame_data.split_last_mut());
            self.fetch_single_segment(buffer, payload);
            *tail = TailByte::new(true, true, SOT_TOGGLE_BIT, self.transfer_id).into();
            return Some(frame_data);
        }

        let residual_length = self.data_length + Self::CRC_LENGTH - self.offset;
        if residual_length == 0 {
            return None;
        }

        let frame_length = unwrap!(DataLength::new_ceil(
            min(residual_length, max_payload_length) + Self::TAIL_LENGTH
        ));
        let mut frame_data = Data::new_zeros(frame_length);
        let (tail, payload) = unwrap!(frame_data.split_last_mut());

        let sot = self.offset == 0;
        if residual_length >= payload.len() {
            self.fetch_full_segment(buffer, payload);
        } else {
            // Padding can only be inserted before first CRC byte is sent.
            // Transport frame must support frame lengths of (1..CRC_LENGTH-1) + TAIL_LENGTH
            assert!(self.offset <= self.data_length);
            self.fetch_padded_segment(buffer, payload);
        }

        *tail = TailByte::new(
            sot,
            self.offset == self.data_length + Self::CRC_LENGTH,
            self.toggle_bit,
            self.transfer_id,
        )
        .into();
        self.toggle_bit = !self.toggle_bit;

        Some(frame_data)
    }

    fn fetch_single_segment(&mut self, buffer: &[u8], payload: &mut [u8]) {
        let (payload_data, payload_pad) = payload.split_at_mut(self.data_length);
        payload_data.copy_from_slice(&buffer[..self.data_length]);
        payload_pad.fill(PAD_VALUE);

        // single frame transfer do not need a CRC, mark it as sent
        self.offset = self.data_length + Self::CRC_LENGTH;
    }

    // Fit the most of residual data and CRC
    fn fetch_full_segment(&mut self, buffer: &[u8], payload: &mut [u8]) {
        let residual_length = self.data_length + Self::CRC_LENGTH - self.offset;
        assert!(residual_length >= payload.len());

        let residual_data = &buffer[min(self.offset, self.data_length)..self.data_length];
        let (payload_data, payload_crc) =
            payload.split_at_mut(min(residual_data.len(), payload.len()));
        payload_data.copy_from_slice(&residual_data[..payload_data.len()]);

        let crc_offset = max(self.offset, self.data_length) - self.data_length;
        let crc_bytes = self.crc.get().to_be_bytes();
        let residual_crc = &crc_bytes[crc_offset..];
        payload_crc.copy_from_slice(&residual_crc[..payload_crc.len()]);

        self.offset += payload.len();
    }

    fn fetch_padded_segment(&mut self, buffer: &[u8], payload: &mut [u8]) {
        assert!(
            self.offset <= self.data_length,
            "Can not insert padding once CRC has started"
        );
        let residual_data = &buffer[self.offset..self.data_length];

        let (payload_data_pad, payload_crc) =
            unwrap!(payload.split_last_chunk_mut::<{ Self::CRC_LENGTH }>());
        let (payload_data, payload_pad) = payload_data_pad.split_at_mut(residual_data.len());
        payload_data.copy_from_slice(residual_data);
        payload_pad.fill(PAD_VALUE);

        let mut crc = self.crc;
        crc.add_bytes(payload_pad);
        *payload_crc = crc.get().to_be_bytes();

        self.offset += payload_data.len() + payload_crc.len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zero_frame_length() {
        let transfer_id = TransferId::from_truncating(27);
        let buffer: [u8; 0] = [];

        let mut scatter = Scatter::new(
            transfer_id,
            &buffer,
            buffer.len(),
            TransferCrc::from(0xffff),
        );
        assert_eq!(
            scatter.fetch_frame_data(&buffer, Mtu::Classic),
            Some(Data::new(&[0b1110_0000 + 27]).unwrap())
        );
        assert_eq!(scatter.fetch_frame_data(&buffer, Mtu::Classic), None);
    }

    #[test]
    fn test_non_full_single_frame() {
        let transfer_id = TransferId::from_truncating(27);
        let buffer: [u8; 6] = [0, 1, 2, 3, 4, 5];

        let mut scatter = Scatter::new(transfer_id, &buffer, buffer.len(), 0x8c18.into());
        assert_eq!(
            scatter.fetch_frame_data(&buffer, Mtu::Classic),
            Some(Data::new(&[0, 1, 2, 3, 4, 5, 0b1110_0000 + 27]).unwrap())
        );
        assert_eq!(scatter.fetch_frame_data(&buffer, Mtu::Classic), None);
    }

    #[test]
    fn test_full_single_frame() {
        let transfer_id = TransferId::from_truncating(27);
        let buffer: [u8; 7] = [0, 1, 2, 3, 4, 5, 6];

        let mut scatter = Scatter::new(transfer_id, &buffer, buffer.len(), 0x28c2.into());
        assert_eq!(
            scatter.fetch_frame_data(&buffer, Mtu::Classic),
            Some(Data::new(&[0, 1, 2, 3, 4, 5, 6, 0b1110_0000 + 27]).unwrap())
        );
        assert_eq!(scatter.fetch_frame_data(&buffer, Mtu::Classic), None);
    }

    #[test]
    fn test_minimum_double_frame() {
        let transfer_id = TransferId::from_truncating(27);
        let buffer: [u8; 8] = [0, 1, 2, 3, 4, 5, 6, 7];

        let mut scatter = Scatter::new(transfer_id, &buffer, buffer.len(), 0x178d.into());
        assert_eq!(
            scatter.fetch_frame_data(&buffer, Mtu::Classic),
            Some(Data::new(&[0, 1, 2, 3, 4, 5, 6, 0b1010_0000 + 27]).unwrap())
        );
        assert_eq!(
            scatter.fetch_frame_data(&buffer, Mtu::Classic),
            Some(Data::new(&[7, 0x17, 0x8d, 0b0100_0000 + 27]).unwrap())
        );
        assert_eq!(scatter.fetch_frame_data(&buffer, Mtu::Classic), None);
    }

    #[test]
    fn test_non_full_double_frame() {
        let transfer_id = TransferId::from_truncating(27);
        let buffer: [u8; 11] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10];

        let mut scatter = Scatter::new(transfer_id, &buffer, buffer.len(), 0x1944.into());
        assert_eq!(
            scatter.fetch_frame_data(&buffer, Mtu::Classic),
            Some(Data::new(&[0, 1, 2, 3, 4, 5, 6, 0b1010_0000 + 27]).unwrap())
        );
        assert_eq!(
            scatter.fetch_frame_data(&buffer, Mtu::Classic),
            Some(Data::new(&[7, 8, 9, 10, 0x19, 0x44, 0b0100_0000 + 27]).unwrap())
        );
        assert_eq!(scatter.fetch_frame_data(&buffer, Mtu::Classic), None);
    }

    #[test]
    fn test_full_double_frame() {
        let transfer_id = TransferId::from_truncating(27);
        let buffer: [u8; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];

        let mut scatter = Scatter::new(transfer_id, &buffer, buffer.len(), 0x7673.into());
        assert_eq!(
            scatter.fetch_frame_data(&buffer, Mtu::Classic),
            Some(Data::new(&[0, 1, 2, 3, 4, 5, 6, 0b1010_0000 + 27]).unwrap())
        );
        assert_eq!(
            scatter.fetch_frame_data(&buffer, Mtu::Classic),
            Some(Data::new(&[7, 8, 9, 10, 11, 0x76, 0x73, 0b0100_0000 + 27]).unwrap())
        );
        assert_eq!(scatter.fetch_frame_data(&buffer, Mtu::Classic), None);
    }

    #[test]
    fn test_minimal_triple_frame() {
        let transfer_id = TransferId::from_truncating(27);
        let buffer: [u8; 13] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];

        let mut scatter = Scatter::new(transfer_id, &buffer, buffer.len(), 0xacdd.into());
        assert_eq!(
            scatter.fetch_frame_data(&buffer, Mtu::Classic),
            Some(Data::new(&[0, 1, 2, 3, 4, 5, 6, 0b1010_0000 + 27]).unwrap())
        );
        assert_eq!(
            scatter.fetch_frame_data(&buffer, Mtu::Classic),
            Some(Data::new(&[7, 8, 9, 10, 11, 12, 0xac, 0b0000_0000 + 27]).unwrap())
        );
        assert_eq!(
            scatter.fetch_frame_data(&buffer, Mtu::Classic),
            Some(Data::new(&[0xdd, 0b0110_0000 + 27]).unwrap())
        );
        assert_eq!(scatter.fetch_frame_data(&buffer, Mtu::Classic), None);
    }

    #[test]
    fn test_crc_only_triple_frame() {
        let transfer_id = TransferId::from_truncating(27);
        let buffer: [u8; 14] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13];

        let mut scatter = Scatter::new(transfer_id, &buffer, buffer.len(), 0x78cb.into());
        assert_eq!(
            scatter.fetch_frame_data(&buffer, Mtu::Classic),
            Some(Data::new(&[0, 1, 2, 3, 4, 5, 6, 0b1010_0000 + 27]).unwrap())
        );
        assert_eq!(
            scatter.fetch_frame_data(&buffer, Mtu::Classic),
            Some(Data::new(&[7, 8, 9, 10, 11, 12, 13, 0b0000_0000 + 27]).unwrap())
        );
        assert_eq!(
            scatter.fetch_frame_data(&buffer, Mtu::Classic),
            Some(Data::new(&[0x78, 0xcb, 0b0110_0000 + 27]).unwrap())
        );
        assert_eq!(scatter.fetch_frame_data(&buffer, Mtu::Classic), None);
    }

    #[test]
    fn test_non_full_triple_frame() {
        let transfer_id = TransferId::from_truncating(27);
        let buffer: [u8; 15] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14];

        let mut scatter = Scatter::new(transfer_id, &buffer, buffer.len(), 0xd551.into());
        assert_eq!(
            scatter.fetch_frame_data(&buffer, Mtu::Classic),
            Some(Data::new(&[0, 1, 2, 3, 4, 5, 6, 0b1010_0000 + 27]).unwrap())
        );
        assert_eq!(
            scatter.fetch_frame_data(&buffer, Mtu::Classic),
            Some(Data::new(&[7, 8, 9, 10, 11, 12, 13, 0b0000_0000 + 27]).unwrap())
        );
        assert_eq!(
            scatter.fetch_frame_data(&buffer, Mtu::Classic),
            Some(Data::new(&[14, 0xd5, 0x51, 0b0110_0000 + 27]).unwrap())
        );
        assert_eq!(scatter.fetch_frame_data(&buffer, Mtu::Classic), None);
    }

    #[test]
    fn test_padding_single_frame() {
        let transfer_id = TransferId::from_truncating(27);
        let buffer: [u8; 8] = [0, 1, 2, 3, 4, 5, 6, 7];

        let mut scatter = Scatter::new(transfer_id, &buffer, buffer.len(), 0x178d.into());
        assert_eq!(
            scatter.fetch_frame_data(&buffer, Mtu::Fd),
            Some(Data::new(&[0, 1, 2, 3, 4, 5, 6, 7, 0, 0, 0, 0b1110_0000 + 27]).unwrap())
        );
        assert_eq!(scatter.fetch_frame_data(&buffer, Mtu::Classic), None);
    }

    #[rustfmt::skip]
    #[test]
    fn test_padding_multi_frame() {
        let transfer_id = TransferId::from_truncating(27);
        let buffer: [u8; 69] = core::array::from_fn(|i| i.try_into().unwrap());

        let mut scatter = Scatter::new(transfer_id, &buffer, buffer.len(), 0xd7de.into());
        let vec: heapless::Vec<u8, 64>  = (0u8..63).chain([0b1010_0000u8 + 27].iter().copied()).collect();
        assert_eq!(
            scatter.fetch_frame_data(&buffer, Mtu::Fd),
            Some(Data::new(&vec).unwrap())
        );
        assert_eq!(
            scatter.fetch_frame_data(&buffer, Mtu::Fd),
            Some(Data::new(&[63, 64, 65, 66, 67, 68, 0, 0, 0, 0xd6, 0x2c, 0b0100_0000 + 27]).unwrap())
        );

        assert_eq!(scatter.fetch_frame_data(&buffer, Mtu::Classic), None);
    }
}
