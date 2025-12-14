use crate::core::{Priority, TransferId};
use crate::format::{SOT_TOGGLE_BIT, TailByte, TransferCrc};
use crate::frame::Mtu;
use crate::time::{Duration, Instant};

#[derive(Clone, Copy)]
struct Segment<'a> {
    transfer_id: TransferId,
    priority: Priority,
    payload: &'a [u8],
    timestamp: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Transfer {
    pub id: TransferId,
    pub priority: Priority,
    pub timestamp: Instant,
    pub length: u32,
}

#[derive(Debug, Clone, Copy)]
enum State {
    Idle,             // sof_timestamp and transfer_id are not set
    IdleLastTransfer, // transfer_id is not set
    AccumulatedFirst,
    AccumulatedOdd,
    AccumulatedEven,
    Assembled,
}

/// Transfer reception state machine
///
/// This implementation does not support redundant interfaces.
/// This implementation does not support in-session preemption, i.e. different priorities for
/// transfers within a single session.
///
/// Through the specification \[1; 4.1-4.2\] is a bit obscure and partially contradicts the
/// reference implementation \[2\], we have chosen our own logic as specified here.
/// The following assumes an independent timeout timer, reassembly process, and assembled status for
/// each TransferId.
///
/// Rules:
/// 1.  A frame with empty data is ignored
///     Rationale: without tail byte the frame does not possesses TransferId and can not participate
///     in transfer reassembly.
///     * \[1\] does not define correct behavior here.
///     * \[2\] ignores such frames.
///   
/// 2.  A frame with a specific TransferId stops reassembly, clears the assembled status, and resets
///     timeout timers for all other TransferIds.
///     Rationale: Multi-frame reassembly relies on frames transmission order within each session.
///     Unless there is in-session preemption, a frame with new TransferId marks the end of any
///     previous transfer.
///     * \[1\] mandates independent tracking for each TransferId and encourages implementers to
///       tolerate out-of-order frame reception. However, this approach requires more memory
///       (32 timers per session) and increases the requirements for timeout selection:
///       TransferId overflow within the timeout will cause drops.
///     * \[2\] resets other TransferId states when an SOT frame with the correct TOGGLE bit is
///       received.
///  
/// 3.  A SOT frame with an incorrect TOGGLE bit stops an unfinished reassembly for its TransferId.
///     Rationale: This is a clear format error, so the semantic of subsequent frames is ambiguous.
///     * \[1\] does not define correct behavior here.
///     * \[2\] ignores any SOT frame with an incorrect TOGGLE bit.
///  
/// 4.  A !EOT frame with a data length different from valid MTU (8 or 64 bytes) stops an unfinished
///     reassembly for its TransferId. Such frame does not participate in further processing.
///     Rationale: This is a clear format error, so the semantic of subsequent frames is ambiguous.
///     However, we do not forbid MTU=8 byte reception in MTU=64 network to enable CAN-Classic &
///     CAN-FD interoperation.
///     * \[1\] mandates !EOT frame to utilized the whole MTU. It also requires all node to share MTU.
///       However, it does not specify frame reception.
///     * \[2\] ignores !EOT frames with data length below 8.
///  
/// 5.  A SOT & EOT frame with the correct TOGGLE bit (re)starts the TransferId timeout timer, stops
///     reassembly, produces a complete transfer and asserts the assembled status, unless there is an
///     active timeout and an _assembled_ transfer for this TransferId.
///     * \[1\] requires timeouts for successful transfers only to avoid duplicates.
///     * \[2\] ignores SOT frames with the same TransferId until the timeout timer expires.
///       An unfinished reassembly blocks SOT frames with the previous TransferId as well.
///  
/// 6.  A SOT & !EOT frame with the correct TOGGLE bit (re)starts the TransferId timeout timer and
///     (re)starts a new reassembly, unless there is an active timeout and an _assembled_ transfer
///     for this TransferId.
///     Rationale: An unfinished multi-frame transfer reassembly, e.g., due to a lost EOT frame,
///     should not block the reception of its replica.
///     * \[1\] requires timeout for successful transfer only to avoid duplicates.
///     * \[2\] ignores SOT frames with the same TransferId until the timeout timer expires.
///       An unfinished reassembly blocks SOT frames with the previous TransferId as well.
///  
/// 7.  A !SOT frame received after a TransferId timeout stops an unfinished reassembly for the
///     TransferId.
///     * \[1\] mandates attributing frames separated by a timeout to a different transfer,
///       even if their TransferId matches.
///     * \[2\] accepts a correct multi-frame transfer regardless of timeout.
///  
/// 8.  A next-to-SOT frame with an incorrect TOGGLE bit stops an unfinished reassembly for the
///     TransferId.
///     Rationale: This condition may occur due to a missing frame. It is safer to dismiss such
///     transmissions even if the CRC matches.
///     * \[1\] does not define correct behavior here.
///     * \[2\] ignores frames with an incorrect TOGGLE bit.
///  
/// 9.  A !SOT & !EOT frame with a new TOGGLE bit value contributes to TransferId reassembly.
///     A frame with a repeated TOGGLE bit is ignored.
///     \[1, 2\] describe the same behavior.
///  
/// 10. An EOT frame with incorrect TOGGLE bit stops an unfinished reassembly for the TransferId.
///     Rationale: This condition may occur due to a missing frame. It is safer to dismiss such
///     transmissions even if the CRC matches.
///     * \[1\] does not define correct behavior here.
///     * \[2\] ignores frames with an incorrect TOGGLE bit.
///   
/// 11. A !SOT & EOT frame finishes the ongoing reassembly. It produces an assembled transfer and
///     asserts the assembled status for the TransferId, provided the TransferId reassembly was
///     running, frame payload was non-empty and CRC matches.
///     * \[1\] does not specify empty payload frame processing.
///     * \[2\] ignores !SOT frames with empty payload.
///  
/// 12. Transfer priority is determined by the EOT frame.
///     * \[1\] mandates all frames in a multi-frame transfer to maintain the same priority.
///     * \[2\] implements the same behavior.
///   
/// # References:
///
/// * \[1\] Cyphal Specification v1.0
///   <https://opencyphal.org/specification/Cyphal_Specification.pdf>
/// * \[2\] libcanard, <https://github.com/OpenCyphal/libcanard>
#[derive(Debug)]
pub struct Gather {
    state: State,
    sof_timestamp: Option<Instant>,
    transfer_id: Option<TransferId>,
    acc: PayloadAccumulator,
}

impl Default for Gather {
    fn default() -> Self {
        Self {
            state: State::Idle,
            sof_timestamp: None,
            transfer_id: None,
            acc: Default::default(),
        }
    }
}

impl Gather {
    pub fn last_transfer_timestamp(&self) -> Option<Instant> {
        match self.state {
            State::Idle => None,
            _ => Some(unwrap!(self.sof_timestamp)),
        }
    }

    #[rustfmt::skip]
    pub fn push_frame(
        &mut self,
        timeout: Duration,
        buffer: &mut [u8],
        priority: Priority,
        data: &[u8],
        timestamp: Instant,
        _mtu: Mtu,
    ) -> Option<Transfer> {
        // R1: skip frame without data
        let (tail_byte, payload) = data.split_last()?;
        let tail = TailByte::from(*tail_byte);

        let segment = Segment {
            transfer_id: tail.transfer_id(),
            priority,
            payload,
            timestamp,
        };

        let tid_match = match self.state {
            State::Idle | State::IdleLastTransfer => false,
            State::AccumulatedFirst
            | State::AccumulatedEven
            | State::AccumulatedOdd
            | State::Assembled => {
                tail.transfer_id() == unwrap!(self.transfer_id)
                    && timestamp <= unwrap!(self.sof_timestamp).saturating_add(timeout)
            }
        };

        let toggle_odd = tail.toggle() ^ SOT_TOGGLE_BIT;

        match (self.state, tail.sot(), tail.eot(), toggle_odd, tid_match) {
            (State::Assembled, _, _, _, true) => self.skip_frame(),
            (_, true, _, true, _) => self.stop_assembly(),
            (_, true, true, false, _) => self.single_frame_assembly(buffer, segment),
            (_, true, false, false, _) => self.accumulate_first(buffer, segment),
            (_, _, _, _, false) => self.stop_assembly(),
            (State::Idle, _, _, _, _) => self.skip_frame(),
            (State::IdleLastTransfer, _, _, _, _) => self.skip_frame(),
            (State::AccumulatedFirst, false, false, false, true) => self.stop_assembly(),
            (State::AccumulatedFirst, false, false, true, true) => self.accumulate_odd(buffer, segment),
            (State::AccumulatedFirst, false, true, false, true) => self.stop_assembly(),
            (State::AccumulatedFirst, false, true, true, true) => self.finish_assembly(buffer, segment),
            (State::AccumulatedOdd, false, false, false, true) => self.accumulate_even(buffer, segment),
            (State::AccumulatedOdd, false, false, true, true) => self.skip_frame(),
            (State::AccumulatedOdd, false, true, false, true) => self.finish_assembly(buffer, segment),
            (State::AccumulatedOdd, false, true, true, true) => self.stop_assembly(),
            (State::AccumulatedEven, false, false, false, true) => self.skip_frame(),
            (State::AccumulatedEven, false, false, true, true) => self.accumulate_odd(buffer, segment),
            (State::AccumulatedEven, false, true, false, true) => self.stop_assembly(),
            (State::AccumulatedEven, false, true, true, true) => self.finish_assembly(buffer, segment),
        }
    }

    fn skip_frame(&mut self) -> Option<Transfer> {
        None
    }

    fn single_frame_assembly(&mut self, buffer: &mut [u8], segment: Segment) -> Option<Transfer> {
        let store_length = core::cmp::min(segment.payload.len(), buffer.len());
        buffer[..store_length].copy_from_slice(&segment.payload[..store_length]);

        self.state = State::Assembled;
        self.sof_timestamp = Some(segment.timestamp);
        self.transfer_id = Some(segment.transfer_id);
        self.acc = Default::default();

        Some(Transfer {
            id: segment.transfer_id,
            priority: segment.priority,
            timestamp: segment.timestamp,
            length: unwrap!(segment.payload.len().try_into()),
        })
    }

    fn accumulate_first(&mut self, buffer: &mut [u8], segment: Segment) -> Option<Transfer> {
        if Mtu::try_from(segment.payload.len() + 1).is_err() {
            return self.stop_assembly();
        }

        self.state = State::AccumulatedFirst;
        self.sof_timestamp = Some(segment.timestamp);
        self.transfer_id = Some(segment.transfer_id);
        self.acc = Default::default();

        if let Err(()) = self.acc.append(buffer, segment.payload) {
            return self.stop_assembly();
        }
        None
    }

    fn accumulate_odd(&mut self, buffer: &mut [u8], segment: Segment) -> Option<Transfer> {
        if Mtu::try_from(segment.payload.len() + 1).is_err() {
            return self.stop_assembly();
        }

        self.state = State::AccumulatedOdd;
        if let Err(()) = self.acc.append(buffer, segment.payload) {
            return self.stop_assembly();
        }
        None
    }

    fn accumulate_even(&mut self, buffer: &mut [u8], segment: Segment) -> Option<Transfer> {
        if Mtu::try_from(segment.payload.len() + 1).is_err() {
            return self.stop_assembly();
        }

        self.state = State::AccumulatedEven;
        if let Err(()) = self.acc.append(buffer, segment.payload) {
            return self.stop_assembly();
        }
        None
    }

    fn finish_assembly(&mut self, buffer: &mut [u8], segment: Segment) -> Option<Transfer> {
        if segment.payload.is_empty() {
            return self.stop_assembly();
        }

        if let Err(()) = self.acc.append(buffer, segment.payload) {
            return self.stop_assembly();
        }

        let length = match self.acc.try_get() {
            Ok(length) => length,
            Err(_) => return self.stop_assembly(),
        };

        self.state = State::Assembled;
        self.acc = Default::default();

        Some(Transfer {
            id: unwrap!(self.transfer_id),
            priority: segment.priority,
            timestamp: unwrap!(self.sof_timestamp),
            length,
        })
    }

    fn stop_assembly(&mut self) -> Option<Transfer> {
        self.state = if self.sof_timestamp.is_some() {
            State::IdleLastTransfer
        } else {
            State::Idle
        };
        self.transfer_id = None;
        self.acc = Default::default();
        None
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct PayloadAccumulator {
    length: u32,
    crc: TransferCrc,
}

impl PayloadAccumulator {
    pub fn append(&mut self, buffer: &mut [u8], segment: &[u8]) -> Result<(), ()> {
        let new_length = u32::try_from(segment.len())
            .ok()
            .and_then(|len| self.length.checked_add(len))
            .ok_or(())?;
        let buffer_offset = core::cmp::min(
            buffer.len(),
            usize::try_from(self.length).unwrap_or(usize::MAX),
        );
        let store_length = core::cmp::min(segment.len(), buffer.len() - buffer_offset);
        buffer[buffer_offset..buffer_offset + store_length]
            .copy_from_slice(&segment[..store_length]);

        self.length = new_length;
        self.crc.add_bytes(segment);
        Ok(())
    }

    pub fn try_get(&self) -> Result<u32, ()> {
        let tail_len = unwrap!(u32::try_from(TransferCrc::LENGTH));
        let crc_match = self.crc.get() == 0;
        if self.length >= tail_len && crc_match {
            Ok(self.length - tail_len)
        } else {
            Err(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const TIMEOUT: Duration = Duration::from_micros(2_000_000);
    const MTU: Mtu = Mtu::Classic;
    const PRIORITY: Priority = Priority::Nominal;

    fn ts(us: u64) -> Instant {
        Instant::MIN.saturating_add(Duration::from_micros(us))
    }

    #[test]
    fn test_zero_payload_transfer() {
        let mut gather = Gather::default();
        let mut buffer = [0xff; 8];

        let data = [0b1110_0000 + 27];
        let transfer = gather.push_frame(TIMEOUT, &mut buffer, PRIORITY, &data, ts(10), MTU);
        assert_eq!(
            transfer,
            Some(Transfer {
                id: TransferId::try_from(27).unwrap(),
                priority: PRIORITY,
                timestamp: ts(10),
                length: 0,
            })
        );
    }

    #[test]
    fn test_regular_transfer() {
        let mut gather = Gather::default();
        let mut buffer = [0xff; 8];

        let data = [0, 1, 2, 3, 0b1110_0000 + 27];
        let transfer = gather.push_frame(TIMEOUT, &mut buffer, PRIORITY, &data, ts(10), MTU);
        assert_eq!(
            transfer,
            Some(Transfer {
                id: TransferId::try_from(27).unwrap(),
                priority: PRIORITY,
                timestamp: ts(10),
                length: 4,
            })
        );
        assert_eq!(buffer[..4], [0, 1, 2, 3]);
    }

    #[test]
    fn test_saturating_transfer() {
        let mut gather = Gather::default();
        let mut buffer = [0xff; 4];

        let data = [0, 1, 2, 3, 4, 5, 0b1110_0000 + 27];
        let transfer = gather.push_frame(TIMEOUT, &mut buffer, PRIORITY, &data, ts(10), MTU);
        assert_eq!(
            transfer,
            Some(Transfer {
                id: TransferId::try_from(27).unwrap(),
                priority: PRIORITY,
                timestamp: ts(10),
                length: 6,
            })
        );
        assert_eq!(buffer, [0, 1, 2, 3]);
    }

    #[test]
    fn test_two_frame_transfer() {
        let mut gather = Gather::default();
        let mut buffer = [0xff; 16];

        let data = [0, 1, 2, 3, 4, 5, 6, 0b1010_0000 + 27];
        let transfer = gather.push_frame(TIMEOUT, &mut buffer, PRIORITY, &data, ts(10), MTU);
        assert_eq!(transfer, None);

        let data = [7, 0x17, 0x8d, 0b0100_0000 + 27];
        let transfer = gather.push_frame(TIMEOUT, &mut buffer, PRIORITY, &data, ts(10), MTU);
        assert_eq!(
            transfer,
            Some(Transfer {
                id: TransferId::try_from(27).unwrap(),
                priority: PRIORITY,
                timestamp: ts(10),
                length: 8,
            })
        );
        assert_eq!(buffer[..8], [0, 1, 2, 3, 4, 5, 6, 7]);
    }

    #[test]
    fn test_multi_frame_saturating_transfer() {
        let mut buffer = [0xff; 4];

        let data0 = [0, 1, 2, 3, 4, 5, 6, 0b1010_0000 + 27];
        let data1 = [7, 0x17, 0x8d, 0b0100_0000 + 27];
        let data1_err = [7, 0x17, 0x8d + 1, 0b0100_0000 + 27];

        let mut gather = Gather::default();
        let transfer = gather.push_frame(TIMEOUT, &mut buffer, PRIORITY, &data0, ts(10), MTU);
        assert_eq!(transfer, None);
        let transfer = gather.push_frame(TIMEOUT, &mut buffer, PRIORITY, &data1, ts(10), MTU);
        assert_eq!(
            transfer,
            Some(Transfer {
                id: TransferId::try_from(27).unwrap(),
                priority: PRIORITY,
                timestamp: ts(10),
                length: 8,
            })
        );
        assert_eq!(buffer, [0, 1, 2, 3]);

        let mut gather = Gather::default();
        let transfer = gather.push_frame(TIMEOUT, &mut buffer, PRIORITY, &data0, ts(20), MTU);
        assert_eq!(transfer, None);
        let transfer = gather.push_frame(TIMEOUT, &mut buffer, PRIORITY, &data1_err, ts(20), MTU);
        assert_eq!(transfer, None);
    }

    #[test]
    fn test_tree_frame_transfer() {
        let mut gather = Gather::default();
        let mut buffer = [0xff; 16];

        let data = [0, 1, 2, 3, 4, 5, 6, 0b1010_0000 + 27];
        let transfer = gather.push_frame(TIMEOUT, &mut buffer, PRIORITY, &data, ts(10), MTU);
        assert_eq!(transfer, None);

        let data = [7, 8, 9, 10, 11, 12, 0xac, 0b0000_0000 + 27];
        let transfer = gather.push_frame(TIMEOUT, &mut buffer, PRIORITY, &data, ts(10), MTU);
        assert_eq!(transfer, None);

        let data = [0xdd, 0b0110_0000 + 27];
        let transfer = gather.push_frame(TIMEOUT, &mut buffer, PRIORITY, &data, ts(10), MTU);
        assert_eq!(
            transfer,
            Some(Transfer {
                id: TransferId::try_from(27).unwrap(),
                priority: PRIORITY,
                timestamp: ts(10),
                length: 13,
            })
        );
        assert_eq!(buffer[..13], [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);
    }

    #[test]
    fn test_four_frame_transfer() {
        let mut gather = Gather::default();
        let mut buffer = [0xff; 32];

        let data = [0, 1, 2, 3, 4, 5, 6, 0b1010_0000 + 27];
        let transfer = gather.push_frame(TIMEOUT, &mut buffer, PRIORITY, &data, ts(10), MTU);
        assert_eq!(transfer, None);

        let data = [7, 8, 9, 10, 11, 12, 13, 0b0000_0000 + 27];
        let transfer = gather.push_frame(TIMEOUT, &mut buffer, PRIORITY, &data, ts(10), MTU);
        assert_eq!(transfer, None);

        let data = [14, 15, 16, 17, 18, 19, 20, 0b0010_0000 + 27];
        let transfer = gather.push_frame(TIMEOUT, &mut buffer, PRIORITY, &data, ts(10), MTU);
        assert_eq!(transfer, None);

        let data = [0xdd, 0x0a, 0b0100_0000 + 27];
        let transfer = gather.push_frame(TIMEOUT, &mut buffer, PRIORITY, &data, ts(10), MTU);
        assert_eq!(
            transfer,
            Some(Transfer {
                id: TransferId::try_from(27).unwrap(),
                priority: PRIORITY,
                timestamp: ts(10),
                length: 21,
            })
        );
        assert_eq!(
            buffer[..21],
            [
                0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20
            ]
        );
    }

    #[test]
    fn test_four_frame_transfer_with_duplicates() {
        let mut gather = Gather::default();
        let mut buffer = [0xff; 32];

        let data = [0, 1, 2, 3, 4, 5, 6, 0b1010_0000 + 27];
        let transfer = gather.push_frame(TIMEOUT, &mut buffer, PRIORITY, &data, ts(10), MTU);
        assert_eq!(transfer, None);
        let transfer = gather.push_frame(TIMEOUT, &mut buffer, PRIORITY, &data, ts(10), MTU);
        assert_eq!(transfer, None);

        let data = [7, 8, 9, 10, 11, 12, 13, 0b0000_0000 + 27];
        let transfer = gather.push_frame(TIMEOUT, &mut buffer, PRIORITY, &data, ts(10), MTU);
        assert_eq!(transfer, None);
        let transfer = gather.push_frame(TIMEOUT, &mut buffer, PRIORITY, &data, ts(10), MTU);
        assert_eq!(transfer, None);

        let data = [14, 15, 16, 17, 18, 19, 20, 0b0010_0000 + 27];
        let transfer = gather.push_frame(TIMEOUT, &mut buffer, PRIORITY, &data, ts(10), MTU);
        assert_eq!(transfer, None);
        let transfer = gather.push_frame(TIMEOUT, &mut buffer, PRIORITY, &data, ts(10), MTU);
        assert_eq!(transfer, None);

        let data = [0xdd, 0x0a, 0b0100_0000 + 27];
        let transfer = gather.push_frame(TIMEOUT, &mut buffer, PRIORITY, &data, ts(10), MTU);
        assert_eq!(
            transfer,
            Some(Transfer {
                id: TransferId::try_from(27).unwrap(),
                priority: PRIORITY,
                timestamp: ts(10),
                length: 21,
            })
        );
        assert_eq!(
            buffer[..21],
            [
                0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20
            ]
        );

        let transfer = gather.push_frame(TIMEOUT, &mut buffer, PRIORITY, &data, ts(10), MTU);
        assert_eq!(transfer, None);
    }

    #[test]
    fn test_one_frame_transfer_timeout() {
        let mut gather = Gather::default();
        let mut buffer = [0xff; 8];

        let data = [0, 1, 2, 3, 0b1110_0000 + 27];
        let transfer = gather.push_frame(TIMEOUT, &mut buffer, PRIORITY, &data, ts(10), MTU);
        assert!(transfer.is_some());

        let transfer = gather.push_frame(TIMEOUT, &mut buffer, PRIORITY, &data, ts(20), MTU);
        assert!(transfer.is_none());

        let transfer = gather.push_frame(TIMEOUT, &mut buffer, PRIORITY, &data, ts(2_000_011), MTU);
        assert!(transfer.is_some());
    }

    #[test]
    fn test_transfer_separated_by_another() {
        let mut gather = Gather::default();
        let mut buffer = [0xff; 8];

        let data0 = [0, 1, 2, 3, 0b1110_0000 + 27];
        let transfer = gather.push_frame(TIMEOUT, &mut buffer, PRIORITY, &data0, ts(10), MTU);
        assert!(transfer.is_some());

        let data1 = [0, 1, 2, 3, 0b1110_0000 + 28];
        let transfer = gather.push_frame(TIMEOUT, &mut buffer, PRIORITY, &data1, ts(10), MTU);
        assert!(transfer.is_some());

        let transfer = gather.push_frame(TIMEOUT, &mut buffer, PRIORITY, &data0, ts(10), MTU);
        assert!(transfer.is_some());
    }

    #[test]
    fn test_multi_frame_transfer_interrupted_by_another() {
        let mut buffer = [0xff; 16];

        let data0 = [0, 1, 2, 3, 4, 5, 6, 0b1010_0000 + 27];
        let data1 = [7, 0x17, 0x8d, 0b0100_0000 + 27];
        let data_another = [0b0000_0000 + 28];

        let mut gather = Gather::default();
        let transfer = gather.push_frame(TIMEOUT, &mut buffer, PRIORITY, &data0, ts(10), MTU);
        assert!(transfer.is_none());
        let transfer = gather.push_frame(TIMEOUT, &mut buffer, PRIORITY, &data1, ts(10), MTU);
        assert!(transfer.is_some());

        let mut gather = Gather::default();
        let transfer = gather.push_frame(TIMEOUT, &mut buffer, PRIORITY, &data0, ts(10), MTU);
        assert!(transfer.is_none());
        let transfer =
            gather.push_frame(TIMEOUT, &mut buffer, PRIORITY, &data_another, ts(10), MTU);
        assert!(transfer.is_none());
        let transfer = gather.push_frame(TIMEOUT, &mut buffer, PRIORITY, &data1, ts(10), MTU);
        assert!(transfer.is_none());
    }

    #[test]
    fn test_multi_frame_transfer_interrupted_timeout() {
        let mut buffer = [0xff; 16];

        let data0 = [0, 1, 2, 3, 4, 5, 6, 0b1010_0000 + 27];
        let data1 = [7, 0x17, 0x8d, 0b0100_0000 + 27];

        let mut gather = Gather::default();
        let transfer = gather.push_frame(TIMEOUT, &mut buffer, PRIORITY, &data0, ts(10), MTU);
        assert!(transfer.is_none());
        let transfer = gather.push_frame(TIMEOUT, &mut buffer, PRIORITY, &data1, ts(10), MTU);
        assert!(transfer.is_some());

        let mut gather = Gather::default();
        let transfer = gather.push_frame(TIMEOUT, &mut buffer, PRIORITY, &data0, ts(10), MTU);
        assert!(transfer.is_none());
        let transfer =
            gather.push_frame(TIMEOUT, &mut buffer, PRIORITY, &data1, ts(2_000_020), MTU);
        assert!(transfer.is_none());
    }

    #[test]
    fn test_assembly_from_second_replica() {
        let mut buffer = [0xff; 16];

        let data0 = [0, 1, 2, 3, 4, 5, 6, 0b1010_0000 + 27];
        let data1 = [7, 0x17, 0x8d, 0b0100_0000 + 27];

        let mut gather = Gather::default();
        let transfer = gather.push_frame(TIMEOUT, &mut buffer, PRIORITY, &data0, ts(10), MTU);
        assert!(transfer.is_none());
        let transfer = gather.push_frame(TIMEOUT, &mut buffer, PRIORITY, &data0, ts(10), MTU);
        assert!(transfer.is_none());
        let transfer = gather.push_frame(TIMEOUT, &mut buffer, PRIORITY, &data1, ts(10), MTU);
        assert!(transfer.is_some());
    }
}
