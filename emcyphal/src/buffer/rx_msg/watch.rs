use core::mem::MaybeUninit;
use core::ptr::NonNull;
use core::task::{Context, Poll};
use emcyphal_encoding::BufferType;

use crate::buffer::rx_msg::{Buffer, SealedBuffer};
use crate::buffer::{BufferToken, DynamicRxBuffer, TransferMeta, gather::Gather};
use crate::core::{NodeId, Priority, PrioritySet, SubjectId};
use crate::format::{SOT_TOGGLE_BIT, TailByte};
use crate::frame::{Data, DataSpecifier, Frame, Mtu};
use crate::registry;
use crate::time::{Duration, Instant};
use crate::utils::{DuplexArray, IndexQueue, PriorityTrigger};

/// Last message buffer
///
/// The buffer maintains up to SN (0 <= SN <= 128) session slots and a single transfer slot.
///
/// To receive non-anonymous transfers from a particular node, the buffer uses a dedicated session
/// slot. The slot tracks the source address, transfer-ID sequence, and accumulates multi-frame
/// transfers. Once a unique transfer is assembled, it is pushed to the transfer slot.
///
/// The buffer allocates a session slot when receiving a start-of-transfer (SOT) frame from an
/// untracked source address. After `timeout` has elapsed since the last SOT frame was received,
/// the slot may be reused for a different address.
///
/// Anonymous transfers do not require session slots and are pushed to the transfer slot
/// immediately.
///
/// A newly received transfer is accepted if its timestamp is greater than or equal to the
/// timestamps of all previously received transfers. Thus, a low-priority multiframe transfer
/// that has been assembled for a while cannot replace a newly received one.
///
/// The transfer slot can store a single transfer. A newly accepted transfer replaces the previous
/// one if present.
///
/// The buffer reserves space for SN + 2 data type buffers. It uses the additional data type
/// buffer to enable zero-copy readout.
pub struct Watch<T: BufferType, const SN: usize> {
    buffers: DuplexArray<T::Buffer, SN, 2>,
    inner: MaybeUninit<Inner<T, SN>>,
    entry: MaybeUninit<registry::rx_msg::Entry>,
}

impl<T: BufferType + 'static, const SN: usize> Watch<T, SN> {
    pub fn new() -> Self {
        Self {
            buffers: Default::default(),
            inner: MaybeUninit::uninit(),
            entry: MaybeUninit::uninit(),
        }
    }
}

impl<T: BufferType + 'static, const SN: usize> Default for Watch<T, SN> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: BufferType + 'static, const SN: usize> SealedBuffer<T> for Watch<T, SN> {
    unsafe fn init(
        &mut self,
        subject: SubjectId,
        timeout: Duration,
    ) -> (NonNull<registry::rx_msg::Entry>, BufferToken<'_>) {
        let buffers: &mut [T::Buffer] = &mut self.buffers;
        let inner = &mut self.inner;
        let entry = &mut self.entry;

        // Safety: Entry will drop the object before the buffers reference expired
        let buffer_value = unsafe {
            Inner::new(
                DataSpecifier::Message(subject),
                timeout,
                buffers.as_mut_ptr(),
                buffers.len(),
            )
        };
        let buffer_ptr = NonNull::from(inner.write(buffer_value));

        // Safety: The user must drop the object before the buffers reference expired
        let entry_value = unsafe { registry::rx_msg::Entry::new(subject, buffer_ptr) };
        let entry_ptr = NonNull::from(entry.write(entry_value));

        // Safety: object lifetime is bounded by original reference
        let buf_token = unsafe { BufferToken::create() };
        (entry_ptr, buf_token)
    }
}

impl<T: BufferType + 'static, const SN: usize> Buffer<T> for Watch<T, SN> {}

#[derive(Debug)]
struct Session {
    gather: Gather,
    buffer_idx: u8,
}

#[derive(Debug)]
struct TransferEntry {
    meta: TransferMeta,
    buffer_length: usize,
}
pub struct Inner<T: BufferType, const SN: usize> {
    data_specifier: DataSpecifier,
    buffers_ptr: *mut T::Buffer,
    sessions: IndexQueue<NodeId, Session, { NodeId::MAX_VALUE as usize }, SN>,
    session_buffer_base: u8,
    transfer: Option<TransferEntry>,
    transfer_min_timestamp: Instant,
    transfer_buffer_idx: u8,
    readout_buffer_idx: u8,
    timeout: Duration,
    priority_trigger: PriorityTrigger,
}

pub const MAX_SESSION_COUNT: usize = NodeId::MAX_VALUE as usize + 1;

impl<T: BufferType, const SN: usize> Inner<T, SN> {
    const _ASSERT_MAX_SN: usize = MAX_SESSION_COUNT - SN;

    // Safety: The buffers must outlive the object
    pub unsafe fn new(
        data_specifier: DataSpecifier,
        timeout: Duration,
        buffers_ptr: *mut T::Buffer,
        buffers_len: usize,
    ) -> Self {
        assert!(!buffers_ptr.is_null());
        assert_eq!(buffers_len, SN + 2);

        Self {
            data_specifier,
            buffers_ptr,
            sessions: IndexQueue::new(),
            session_buffer_base: 0,
            transfer: None,
            transfer_min_timestamp: Instant::MIN,
            transfer_buffer_idx: unwrap!(SN.try_into()),
            readout_buffer_idx: unwrap!((SN + 1).try_into()),
            timeout,
            priority_trigger: PriorityTrigger::new(),
        }
    }

    fn push_regular_frame(
        &mut self,
        source: NodeId,
        priority: Priority,
        data: &Data,
        timestamp: Instant,
        loop_back: bool,
        mtu: Mtu,
    ) {
        let sot = data.last().is_some_and(|byte| TailByte::from(*byte).sot());

        let mut session = self.sessions.get_mut(source);
        if session.is_none() && sot {
            self.try_start_new_session(source, timestamp);
            session = self.sessions.get_mut(source);
        }

        let session = match session {
            Some(s) => s,
            None => return,
        };

        // Safety: all buffers are part of the same allocation
        let buffer_ptr = unsafe { self.buffers_ptr.add(session.buffer_idx.into()) };
        // Safety: the function has an exclusive access to the session buffer
        let buffer = unsafe { buffer_ptr.as_mut().unwrap_unchecked() }.as_mut();

        let transfer =
            session
                .gather
                .push_frame(self.timeout, buffer, priority, data, timestamp, mtu);

        if let Some(transfer) = transfer
            && self.transfer_min_timestamp <= transfer.timestamp
        {
            self.transfer_min_timestamp = transfer.timestamp;

            let meta = TransferMeta {
                priority: transfer.priority,
                address: Some(source),
                transfer_id: transfer.id,
                timestamp: transfer.timestamp,
                loop_back,
            };

            let buffer_length = core::cmp::min(
                transfer.length.try_into().unwrap_or(usize::MAX),
                buffer.len(),
            );

            self.transfer = Some(TransferEntry {
                meta,
                buffer_length,
            });
            core::mem::swap(&mut session.buffer_idx, &mut self.transfer_buffer_idx);

            self.priority_trigger.wake(PrioritySet::new_eq(priority));
        }

        if session.gather.last_transfer_timestamp() == Some(timestamp) {
            unwrap!(self.sessions.enqueue_back(source).ok());
        }
    }

    fn push_anonymous_frame(
        &mut self,
        priority: Priority,
        data: &Data,
        timestamp: Instant,
        loop_back: bool,
    ) {
        let (tail_byte, payload) = match data.split_last() {
            Some(pair) => pair,
            None => return,
        };
        let tail = TailByte::from(*tail_byte);
        let valid_frame = tail.sot() && tail.eot() && tail.toggle() == SOT_TOGGLE_BIT;
        if !valid_frame {
            return;
        }

        if self.transfer_min_timestamp > timestamp {
            return;
        }

        self.transfer_min_timestamp = timestamp;
        let meta = TransferMeta {
            priority,
            address: None,
            transfer_id: tail.transfer_id(),
            timestamp,
            loop_back,
        };

        // Safety: all buffers are part of the same allocation
        let buffer_ptr = unsafe { self.buffers_ptr.add(self.transfer_buffer_idx.into()) };
        // Safety: the function has an exclusive access to the allocated buffer
        let buffer = unsafe { buffer_ptr.as_mut().unwrap_unchecked() }.as_mut();

        // A semantic compatible message may exceed the buffer length
        let buffer_length = core::cmp::min(payload.len(), buffer.len());
        buffer[..buffer_length].copy_from_slice(&payload[..buffer_length]);

        self.transfer = Some(TransferEntry {
            meta,
            buffer_length,
        });

        self.priority_trigger.wake(PrioritySet::new_eq(priority));
    }

    fn try_start_new_session(&mut self, source: NodeId, timestamp: Instant) {
        assert!(!self.sessions.contains(source));

        let mut buffer_idx = self.allocate_session_buffer();
        if buffer_idx.is_none() {
            let oldest_source = unwrap!(self.sessions.front());
            let oldest_session = unwrap!(self.sessions.get(oldest_source));
            let expired = oldest_session
                .gather
                .last_transfer_timestamp()
                .is_none_or(|ts| ts.saturating_add(self.timeout) < timestamp);
            if expired {
                buffer_idx = Some(unwrap!(self.sessions.remove(oldest_source)).buffer_idx)
            }
        }
        if let Some(idx) = buffer_idx {
            let session = Session {
                gather: Default::default(),
                buffer_idx: idx,
            };
            unwrap!(self.sessions.insert(source, session).ok());
            unwrap!(self.sessions.enqueue_front(source));
        }
    }

    fn allocate_session_buffer(&mut self) -> Option<u8> {
        if usize::from(self.session_buffer_base) < SN {
            let buffer = self.session_buffer_base;
            self.session_buffer_base += 1;
            Some(buffer)
        } else {
            None
        }
    }
}

impl<T: BufferType, const SN: usize> DynamicRxBuffer for Inner<T, SN> {
    fn push_frame(&mut self, frame: &Frame, mtu: Mtu) {
        if frame.header.data_spec != self.data_specifier {
            return;
        }

        let header = &frame.header;
        match frame.header.source {
            Some(node) => self.push_regular_frame(
                node,
                header.priority,
                &frame.data,
                frame.timestamp,
                frame.loop_back,
                mtu,
            ),
            None => self.push_anonymous_frame(
                header.priority,
                &frame.data,
                frame.timestamp,
                frame.loop_back,
            ),
        }
    }

    fn pop_readiness(&self) -> PrioritySet {
        self.transfer
            .as_ref()
            .map_or(PrioritySet::NONE, |transfer| {
                PrioritySet::new_eq(transfer.meta.priority)
            })
    }

    fn poll_pop_readiness(
        &mut self,
        cx: &mut Context,
        priority_mask: PrioritySet,
    ) -> Poll<PrioritySet> {
        let priorities = self.pop_readiness() & priority_mask;
        if !priorities.is_empty() {
            Poll::Ready(priorities)
        } else {
            self.priority_trigger.register(cx.waker(), priority_mask);
            Poll::Pending
        }
    }

    fn try_pop<'b>(
        &mut self,
        _token: BufferToken<'b>,
        priority_mask: PrioritySet,
    ) -> Option<(TransferMeta, &'b [u8])> {
        if self
            .transfer
            .as_ref()
            .is_none_or(|transfer| !priority_mask.contains(transfer.meta.priority))
        {
            return None;
        }

        let transfer = unwrap!(self.transfer.take());

        // Safety: buffer access is guarded by _token
        core::mem::swap(&mut self.transfer_buffer_idx, &mut self.readout_buffer_idx);

        // Safety: all buffers are part of the same allocation
        let buffer_ptr = unsafe { self.buffers_ptr.add(self.readout_buffer_idx.into()) };
        // Safety: self.readout_buffer_idx)
        let buffer = unsafe { buffer_ptr.as_ref().unwrap_unchecked() }.as_ref();
        let valid_buffer = &buffer[..transfer.buffer_length];
        Some((transfer.meta, valid_buffer))
    }
}

// Safety: Object owns a unique reference to the buffer array
unsafe impl<T: BufferType, const SN: usize> Send for Inner<T, SN> {}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use emcyphal_core::SubjectId;
    use emcyphal_encoding::Deserialize;
    use heapless::Vec;

    use crate::core::TransferId;
    use crate::data_types::ByteArray;
    use crate::endpoint::Transfer;
    use crate::frame::{Data, DataSpecifier, Frame, Header};

    const TIMEOUT: Duration = Duration::from_micros(2_000_000);
    const DATA_SPEC: DataSpecifier = DataSpecifier::Message(SubjectId::from_truncating(0));
    const MTU: Mtu = Mtu::Classic;

    fn ts(us: u64) -> Instant {
        Instant::MIN.saturating_add(Duration::from_micros(us))
    }

    fn make_buffer<T: BufferType, const SN: usize>(
        timeout: Duration,
    ) -> (Inner<T, SN>, BufferToken<'static>) {
        let mut raw_buffers = std::vec::Vec::<T::Buffer>::new();
        raw_buffers.resize_with(SN + 2, Default::default);
        let raw_buffers = std::boxed::Box::new(raw_buffers).leak();
        let buffer = unsafe {
            Inner::new(
                DATA_SPEC,
                timeout,
                raw_buffers.as_mut_ptr(),
                raw_buffers.len(),
            )
        };
        let buf_token = unsafe { BufferToken::create() };
        (buffer, buf_token)
    }

    fn pop_transfer<T: Deserialize>(
        buffer: &mut dyn DynamicRxBuffer,
        token: BufferToken<'_>,
        priority_mask: PrioritySet,
    ) -> Option<Transfer<T>> {
        let (meta, buf) = buffer.try_pop(token, priority_mask)?;
        let payload = T::deserialize_from_bytes(buf).unwrap();
        Some(Transfer { meta, payload })
    }

    fn make_frame(
        priority: Priority,
        source: Option<NodeId>,
        data: &[u8],
        timestamp: Instant,
    ) -> Frame {
        Frame {
            header: Header {
                priority: priority,
                data_spec: DataSpecifier::Message(SubjectId::from_truncating(0)),
                source,
                destination: None,
            },
            data: Data::new(data).unwrap(),
            timestamp,
            loop_back: false,
        }
    }

    #[test]
    fn test_anonymous_transfer() {
        let (mut buffer, mut token) = make_buffer::<ByteArray, 10>(TIMEOUT);

        assert_eq!(
            pop_transfer::<ByteArray>(&mut buffer, token.reborrow(), PrioritySet::ALL),
            None
        );

        let data = [4, 0, 0, 1, 2, 3, 0b1110_0000 + 27];
        buffer.push_frame(&make_frame(Priority::High, None, &data, ts(10)), MTU);

        let transfer = Transfer {
            meta: TransferMeta {
                address: None,
                transfer_id: TransferId::try_from(27).unwrap(),
                priority: Priority::High,
                timestamp: ts(10),
                loop_back: false,
            },
            payload: ByteArray {
                bytes: Vec::from_slice(&[0, 1, 2, 3]).unwrap(),
            },
        };

        assert_eq!(
            pop_transfer(&mut buffer, token.reborrow(), PrioritySet::ALL),
            Some(transfer)
        )
    }

    #[test]
    fn test_regular_transfer() {
        let node = NodeId::try_from(10).unwrap();

        let (mut buffer, mut token) = make_buffer::<ByteArray, 10>(TIMEOUT);

        assert_eq!(
            pop_transfer::<ByteArray>(&mut buffer, token.reborrow(), PrioritySet::ALL),
            None
        );

        let data = [4, 0, 0, 1, 2, 3, 0b1110_0000 + 27];
        buffer.push_frame(&make_frame(Priority::High, Some(node), &data, ts(10)), MTU);

        let transfer = Transfer {
            meta: TransferMeta {
                address: Some(node),
                transfer_id: TransferId::try_from(27).unwrap(),
                priority: Priority::High,
                timestamp: ts(10),
                loop_back: false,
            },
            payload: ByteArray {
                bytes: Vec::from_slice(&[0, 1, 2, 3]).unwrap(),
            },
        };

        assert_eq!(
            pop_transfer(&mut buffer, token.reborrow(), PrioritySet::ALL),
            Some(transfer)
        )
    }

    #[test]
    fn test_multiframe_transfer() {
        let node = NodeId::try_from(10).unwrap();

        let (mut buffer, mut token) = make_buffer::<ByteArray, 10>(TIMEOUT);

        assert_eq!(
            pop_transfer::<ByteArray>(&mut buffer, token.reborrow(), PrioritySet::ALL),
            None
        );

        let data = [6, 0, 0, 1, 2, 3, 4, 0b1010_0000 + 27];
        buffer.push_frame(&make_frame(Priority::High, Some(node), &data, ts(10)), MTU);
        assert_eq!(
            pop_transfer::<ByteArray>(&mut buffer, token.reborrow(), PrioritySet::ALL),
            None
        );

        let data = [5, 0x33, 0xfd, 0b0100_0000 + 27];
        buffer.push_frame(&make_frame(Priority::High, Some(node), &data, ts(10)), MTU);
        let transfer = Transfer {
            meta: TransferMeta {
                address: Some(node),
                transfer_id: TransferId::try_from(27).unwrap(),
                priority: Priority::High,
                timestamp: ts(10),
                loop_back: false,
            },
            payload: ByteArray {
                bytes: Vec::from_slice(&[0, 1, 2, 3, 4, 5]).unwrap(),
            },
        };

        assert_eq!(
            pop_transfer(&mut buffer, token.reborrow(), PrioritySet::ALL),
            Some(transfer)
        )
    }

    #[test]
    fn test_interleaved_transfer() {
        let node_a = NodeId::try_from(10).unwrap();
        let node_b = NodeId::try_from(20).unwrap();

        let (mut buffer, mut token) = make_buffer::<ByteArray, 10>(TIMEOUT);

        assert_eq!(
            pop_transfer::<ByteArray>(&mut buffer, token.reborrow(), PrioritySet::ALL),
            None
        );

        let data0 = [6, 0, 0, 1, 2, 3, 4, 0b1010_0000 + 27];
        let data1 = [5, 0x33, 0xfd, 0b0100_0000 + 27];
        let message = ByteArray {
            bytes: Vec::from_slice(&[0, 1, 2, 3, 4, 5]).unwrap(),
        };

        buffer.push_frame(
            &make_frame(Priority::High, Some(node_a), &data0, ts(10)),
            MTU,
        );
        assert_eq!(
            pop_transfer::<ByteArray>(&mut buffer, token.reborrow(), PrioritySet::ALL),
            None
        );

        buffer.push_frame(
            &make_frame(Priority::High, Some(node_b), &data0, ts(10)),
            MTU,
        );
        assert_eq!(
            pop_transfer::<ByteArray>(&mut buffer, token.reborrow(), PrioritySet::ALL),
            None
        );

        buffer.push_frame(
            &make_frame(Priority::High, Some(node_a), &data1, ts(10)),
            MTU,
        );
        assert_eq!(
            pop_transfer(&mut buffer, token.reborrow(), PrioritySet::ALL),
            Some(Transfer {
                meta: TransferMeta {
                    address: Some(node_a),
                    transfer_id: TransferId::try_from(27).unwrap(),
                    priority: Priority::High,
                    timestamp: ts(10),
                    loop_back: false,
                },
                payload: message.clone(),
            })
        );

        buffer.push_frame(
            &make_frame(Priority::High, Some(node_b), &data1, ts(10)),
            MTU,
        );
        assert_eq!(
            pop_transfer(&mut buffer, token.reborrow(), PrioritySet::ALL),
            Some(Transfer {
                meta: TransferMeta {
                    address: Some(node_b),
                    transfer_id: TransferId::try_from(27).unwrap(),
                    priority: Priority::High,
                    timestamp: ts(10),
                    loop_back: false,
                },
                payload: message.clone(),
            })
        );

        assert_eq!(
            pop_transfer::<ByteArray>(&mut buffer, token.reborrow(), PrioritySet::ALL),
            None
        );
    }

    #[test]
    fn test_replace_priority() {
        let node_a = NodeId::try_from(10).unwrap();
        let node_b = NodeId::try_from(20).unwrap();
        let node_c = NodeId::try_from(30).unwrap();

        let (mut buffer, mut token) = make_buffer::<ByteArray, 10>(TIMEOUT);

        let data = [4, 0, 1, 2, 3, 0b1110_0000 + 27];

        buffer.push_frame(&make_frame(Priority::Low, Some(node_a), &data, ts(10)), MTU);
        buffer.push_frame(
            &make_frame(Priority::High, Some(node_b), &data, ts(10)),
            MTU,
        );
        buffer.push_frame(
            &make_frame(Priority::Nominal, Some(node_c), &data, ts(10)),
            MTU,
        );

        let transfer =
            pop_transfer::<ByteArray>(&mut buffer, token.reborrow(), PrioritySet::ALL).unwrap();
        assert_eq!(transfer.meta.address, Some(node_c));

        assert_eq!(
            pop_transfer::<ByteArray>(&mut buffer, token.reborrow(), PrioritySet::ALL),
            None
        );
    }

    #[test]
    fn test_replace_timestamp() {
        let node_a = NodeId::try_from(10).unwrap();
        let node_b = NodeId::try_from(20).unwrap();
        let node_c = NodeId::try_from(30).unwrap();

        let (mut buffer, mut token) = make_buffer::<ByteArray, 10>(TIMEOUT);

        let data = [4, 0, 1, 2, 3, 0b1110_0000 + 27];

        buffer.push_frame(
            &make_frame(Priority::Nominal, Some(node_a), &data, ts(20)),
            MTU,
        );
        buffer.push_frame(
            &make_frame(Priority::Nominal, Some(node_b), &data, ts(20)),
            MTU,
        );
        buffer.push_frame(
            &make_frame(Priority::Nominal, Some(node_c), &data, ts(10)),
            MTU,
        );

        let transfer =
            pop_transfer::<ByteArray>(&mut buffer, token.reborrow(), PrioritySet::ALL).unwrap();
        assert_eq!(transfer.meta.address, Some(node_b));

        assert_eq!(
            pop_transfer::<ByteArray>(&mut buffer, token.reborrow(), PrioritySet::ALL),
            None
        );
    }
}
