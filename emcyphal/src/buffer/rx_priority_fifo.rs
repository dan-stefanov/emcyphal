use core::task::{Context, Poll};
use emcyphal_encoding::BufferType;
use heapless::Vec;

use crate::buffer::{BufferToken, DynamicRxBuffer, TransferMeta, gather::Gather};
use crate::core::{NodeId, Priority, PrioritySet, TransferId};
use crate::format::{SOT_TOGGLE_BIT, TailByte};
use crate::frame::{Data, DataSpecifier, Frame, Mtu};
use crate::time::{Duration, Instant};
use crate::utils::{IndexQueue, PriorityQueue, PriorityTrigger};

#[derive(Debug)]
struct Session {
    gather: Gather,
    buffer_idx: u8,
}

#[derive(Debug)]
struct TransferEntry {
    source: Option<NodeId>,
    transfer_id: TransferId,
    timestamp: Instant,
    loop_back: bool,
    buffer_length: usize,
    buffer_idx: u8,
}
pub struct RxPriorityFifoInner<T: BufferType, const SN: usize, const TN: usize> {
    data_specifier: DataSpecifier,
    buffers_ptr: *mut T::Buffer,
    sessions: IndexQueue<NodeId, Session, { NodeId::MAX.into_u8() as usize }, SN>,
    session_buffer_base: u8,
    transfers: TransferQueue<TN>,
    readout_buffer_idx: u8,
    timeout: Duration,
    priority_trigger: PriorityTrigger,
}

pub const MAX_SESSION_COUNT: usize = NodeId::MAX.into_u8() as usize + 1;
pub const MAX_TRANSFER_COUNT: usize = 127;
const _ASSERT_BUF_INDEX: usize = u8::MAX as usize - MAX_SESSION_COUNT - MAX_TRANSFER_COUNT;

impl<T: BufferType, const SN: usize, const TN: usize> RxPriorityFifoInner<T, SN, TN> {
    const _ASSERT_MAX_SN: usize = MAX_SESSION_COUNT - SN;
    const _ASSERT_MAX_TN: usize = MAX_TRANSFER_COUNT - TN;
    const _ASSERT_MIN_TN: usize = TN - 1;

    // Safety: The buffers must outlive the object
    pub unsafe fn new(
        data_specifier: DataSpecifier,
        timeout: Duration,
        buffers_ptr: *mut T::Buffer,
        buffers_len: usize,
    ) -> Self {
        assert!(!buffers_ptr.is_null());
        assert_eq!(buffers_len, SN + TN + 1);

        Self {
            data_specifier,
            buffers_ptr,
            sessions: IndexQueue::new(),
            session_buffer_base: 0,
            transfers: unwrap!(TransferQueue::new(unwrap!(u8::try_from(SN)))),
            readout_buffer_idx: unwrap!((SN + TN).try_into()),
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

        if let Some(transfer) = transfer {
            let transfer_buffer_length = core::cmp::min(
                transfer.length.try_into().unwrap_or(usize::MAX),
                buffer.len(),
            );

            // Allocation may fail if the queue is filled with higher priority transfers
            let new_buffer_idx = self.transfers.allocate_buffer(transfer.priority);
            if let Some(idx) = new_buffer_idx {
                let transfer_buffer_idx = core::mem::replace(&mut session.buffer_idx, idx);
                let entry = TransferEntry {
                    source: Some(source),
                    transfer_id: transfer.id,
                    timestamp: transfer.timestamp,
                    loop_back,
                    buffer_length: transfer_buffer_length,
                    buffer_idx: transfer_buffer_idx,
                };

                unwrap!(self.transfers.push(priority, entry).ok());
                self.priority_trigger.wake(PrioritySet::new_eq(priority));
            }
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

        if let Some(buffer_idx) = self.transfers.allocate_buffer(priority) {
            // Safety: all buffers are part of the same allocation
            let buffer_ptr = unsafe { self.buffers_ptr.add(buffer_idx.into()) };
            // Safety: the function has an exclusive access to the allocated buffer
            let buffer = unsafe { buffer_ptr.as_mut().unwrap_unchecked() }.as_mut();

            // A semantic compatible message may exceed the buffer length
            let length = core::cmp::min(payload.len(), buffer.len());
            buffer[..length].copy_from_slice(&payload[..length]);

            let entry = TransferEntry {
                source: None,
                transfer_id: tail.transfer_id(),
                timestamp,
                buffer_length: length,
                buffer_idx,
                loop_back,
            };

            unwrap!(self.transfers.push(priority, entry).ok());
            self.priority_trigger.wake(PrioritySet::new_eq(priority));
        }
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

impl<T: BufferType, const SN: usize, const TN: usize> DynamicRxBuffer
    for RxPriorityFifoInner<T, SN, TN>
{
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
        self.transfers.priorities()
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
        let (priority, transfer) = self.transfers.pop(priority_mask)?;
        let prev_buffer = core::mem::replace(&mut self.readout_buffer_idx, transfer.buffer_idx);
        unwrap!(self.transfers.free_buffer(prev_buffer));

        let meta = TransferMeta {
            priority,
            address: transfer.source,
            transfer_id: transfer.transfer_id,
            timestamp: transfer.timestamp,
            loop_back: transfer.loop_back,
        };

        // Safety: all buffers are part of the same allocation
        let buffer_ptr = unsafe { self.buffers_ptr.add(self.readout_buffer_idx.into()) };
        // Safety: self.readout_buffer_idx)
        let buffer = unsafe { buffer_ptr.as_ref().unwrap_unchecked() }.as_ref();
        let valid_buffer = &buffer[..transfer.buffer_length];
        Some((meta, valid_buffer))
    }
}

// Safety: Object owns a unique reference to the buffer array
unsafe impl<T: BufferType, const SN: usize, const TN: usize> Send
    for RxPriorityFifoInner<T, SN, TN>
{
}

struct TransferQueue<const N: usize> {
    queue: PriorityQueue<TransferEntry, N>,
    free_buffers: Vec<u8, N>,
}

impl<const N: usize> TransferQueue<N> {
    fn new(buffer_index_base: u8) -> Result<Self, ()> {
        if usize::from(buffer_index_base) + N > usize::from(u8::MAX) + 1 {
            return Err(());
        }
        let buffer_indexes = (0..N).map(|i| buffer_index_base + unwrap!(u8::try_from(i)));
        Ok(Self {
            queue: PriorityQueue::new(),
            free_buffers: buffer_indexes.collect(),
        })
    }

    fn allocate_buffer(&mut self, priority: Priority) -> Option<u8> {
        if let Some(idx) = self.free_buffers.pop() {
            return Some(idx);
        }

        let max_priority = unwrap!(self.queue.max_priority());
        if priority <= max_priority {
            return Some(unwrap!(self.queue.pop(max_priority)).buffer_idx);
        }

        None
    }

    fn free_buffer(&mut self, idx: u8) -> Result<(), u8> {
        self.free_buffers.push(idx)
    }

    fn push(&mut self, priority: Priority, transfer: TransferEntry) -> Result<(), TransferEntry> {
        self.queue.push(priority, transfer)
    }

    fn priorities(&self) -> PrioritySet {
        self.queue.priorities()
    }

    fn pop(&mut self, priority_mask: PrioritySet) -> Option<(Priority, TransferEntry)> {
        let priority = (self.queue.priorities() & priority_mask).first()?;
        let entry = unwrap!(self.queue.pop(priority));
        Some((priority, entry))
    }
}

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
    const DATA_SPEC: DataSpecifier = DataSpecifier::Message(SubjectId::new(0).unwrap());
    const MTU: Mtu = Mtu::Classic;

    fn ts(us: u64) -> Instant {
        Instant::MIN.saturating_add(Duration::from_micros(us))
    }

    fn make_buffer<T: BufferType, const SN: usize, const TN: usize>(
        timeout: Duration,
    ) -> (RxPriorityFifoInner<T, SN, TN>, BufferToken<'static>) {
        let mut raw_buffers = std::vec::Vec::<T::Buffer>::new();
        raw_buffers.resize_with(SN + TN + 1, Default::default);
        let raw_buffers = std::boxed::Box::new(raw_buffers).leak();
        let buffer = unsafe {
            RxPriorityFifoInner::new(
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
                data_spec: DATA_SPEC,
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
        let (mut buffer, mut token) = make_buffer::<ByteArray, 10, 10>(TIMEOUT);

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

        let (mut buffer, mut token) = make_buffer::<ByteArray, 10, 10>(TIMEOUT);

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

        let (mut buffer, mut token) = make_buffer::<ByteArray, 10, 10>(TIMEOUT);

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

        let (mut buffer, mut token) = make_buffer::<ByteArray, 10, 10>(TIMEOUT);

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
    fn test_output_order() {
        let node_a = NodeId::try_from(10).unwrap();
        let node_b = NodeId::try_from(20).unwrap();
        let node_c = NodeId::try_from(30).unwrap();

        let (mut buffer, mut token) = make_buffer::<ByteArray, 10, 10>(TIMEOUT);

        let data = [4, 0, 1, 2, 3, 0b1110_0000 + 27];

        buffer.push_frame(
            &make_frame(Priority::Nominal, Some(node_a), &data, ts(10)),
            MTU,
        );
        buffer.push_frame(
            &make_frame(Priority::Nominal, Some(node_b), &data, ts(20)),
            MTU,
        );
        buffer.push_frame(
            &make_frame(Priority::High, Some(node_c), &data, ts(30)),
            MTU,
        );

        let transfer =
            pop_transfer::<ByteArray>(&mut buffer, token.reborrow(), PrioritySet::ALL).unwrap();
        assert_eq!(transfer.meta.address, Some(node_c));

        let transfer =
            pop_transfer::<ByteArray>(&mut buffer, token.reborrow(), PrioritySet::ALL).unwrap();
        assert_eq!(transfer.meta.address, Some(node_a));

        let transfer =
            pop_transfer::<ByteArray>(&mut buffer, token.reborrow(), PrioritySet::ALL).unwrap();
        assert_eq!(transfer.meta.address, Some(node_b));
    }

    #[test]
    fn test_drop_order() {
        let node_a = NodeId::try_from(10).unwrap();
        let node_b = NodeId::try_from(20).unwrap();
        let node_c = NodeId::try_from(30).unwrap();

        let (mut buffer, mut token) = make_buffer::<ByteArray, 10, 2>(TIMEOUT);

        let data = [4, 0, 1, 2, 3, 0b1110_0000 + 27];
        buffer.push_frame(
            &make_frame(Priority::High, Some(node_a), &data, ts(10)),
            MTU,
        );
        buffer.push_frame(
            &make_frame(Priority::Nominal, Some(node_b), &data, ts(20)),
            MTU,
        );
        buffer.push_frame(
            &make_frame(Priority::Nominal, Some(node_c), &data, ts(30)),
            MTU,
        );

        let transfer =
            pop_transfer::<ByteArray>(&mut buffer, token.reborrow(), PrioritySet::ALL).unwrap();
        assert_eq!(transfer.meta.address, Some(node_a));

        let transfer =
            pop_transfer::<ByteArray>(&mut buffer, token.reborrow(), PrioritySet::ALL).unwrap();
        assert_eq!(transfer.meta.address, Some(node_c));

        assert_eq!(
            pop_transfer::<ByteArray>(&mut buffer, token.reborrow(), PrioritySet::ALL),
            None
        );
    }
}
