use core::mem::MaybeUninit;
use core::ptr::NonNull;
use core::task::{Context, Poll};
use embassy_sync::waitqueue::WakerRegistration;
use emcyphal_encoding::BufferType;
use heapless::Vec;

use super::{Buffer, SealedBuffer};
use crate::buffer::scatter::Scatter;
use crate::buffer::{BufferError, BufferToken, DynamicTxBuffer};
use crate::core::{Priority, PrioritySet, SubjectId};
use crate::format::TransferCrc;
use crate::frame::{DataSpecifier, Frame, Header, Mtu};
use crate::registry;
use crate::time::Instant;

const BUFFER_COUNT: usize = 2;

/// Non-blocking single message buffer
///
/// The buffer can store one complete and one partially fetched transfer of a predefined priority.
/// The buffer always accepts a new transfer, replacing the previous complete one.
/// The buffer always completes a partially sent transfer regardless of updates.
///
/// The scratchpad buffer lock during serialization does not block the fetching of the previous
/// complete or incomplete transfer. Thus, transmission will proceed even if the endpoint
/// updates the message in a tight loop.
///
/// The buffer reserves space for two data type buffers.
pub struct Signal<T: BufferType> {
    buffers: [T::Buffer; BUFFER_COUNT],
    inner: MaybeUninit<Inner<T>>,
    entry: MaybeUninit<registry::tx_msg::Entry>,
}

impl<T: BufferType> Signal<T> {
    pub fn new() -> Self {
        Self {
            buffers: Default::default(),
            inner: MaybeUninit::uninit(),
            entry: MaybeUninit::uninit(),
        }
    }
}

impl<T: BufferType> Default for Signal<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: BufferType + 'static> SealedBuffer<T> for Signal<T> {
    unsafe fn init(
        &mut self,
        subject: SubjectId,
        priority: Priority,
    ) -> (NonNull<registry::tx_msg::Entry>, BufferToken<'_>) {
        let buffers: &mut [T::Buffer] = &mut self.buffers;
        let inner = &mut self.inner;
        let entry = &mut self.entry;

        // Safety: The entry will drop the object before the buffers reference expired
        let buffer = unsafe { Inner::new(subject, priority, buffers.as_mut_ptr(), buffers.len()) };
        let buffer_ptr = NonNull::from_mut(inner.write(buffer));

        // Safety: User must drop the object before the buffers reference expired
        let entry_value = unsafe { registry::tx_msg::Entry::new(subject, buffer_ptr) };
        let entry_ptr = NonNull::from_mut(entry.write(entry_value));

        // Safety: object lifetime is bounded by original reference
        let buf_token = unsafe { BufferToken::create() };

        (entry_ptr, buf_token)
    }
}

impl<T: BufferType + 'static> Buffer<T> for Signal<T> {}

struct TransferEntry {
    buffer_idx: u8,
    scatter: Scatter,
    timestamp: Instant,
    loop_back: bool,
}

struct Inner<T: BufferType> {
    subject: SubjectId,
    buffers_ptr: *mut T::Buffer,
    priority: Priority,
    pending_transfer: Option<TransferEntry>,
    fragmented_transfer: Option<TransferEntry>,
    scratchpad_buffer_idx: Option<u8>,
    free_buffer_indexes: Vec<u8, BUFFER_COUNT>,
    empty_waker: WakerRegistration,
}

impl<T: BufferType> Inner<T> {
    // Safety: The buffers must outlive the object
    unsafe fn new(
        subject: SubjectId,
        priority: Priority,
        buffers_ptr: *mut T::Buffer,
        buffers_len: usize,
    ) -> Self {
        assert!(!buffers_ptr.is_null());
        assert_eq!(buffers_len, BUFFER_COUNT);

        Self {
            subject,
            buffers_ptr,
            priority,
            pending_transfer: None,
            fragmented_transfer: None,
            scratchpad_buffer_idx: None,
            free_buffer_indexes: (0..unwrap!(BUFFER_COUNT.try_into())).collect(),
            empty_waker: Default::default(),
        }
    }
}

impl<T: BufferType> DynamicTxBuffer for Inner<T> {
    fn pop_readiness(&self) -> PrioritySet {
        if !self.is_empty() {
            PrioritySet::new_eq(self.priority)
        } else {
            PrioritySet::NONE
        }
    }

    fn try_pop(&mut self, priority_mask: PrioritySet, mtu: Mtu) -> Option<Frame> {
        if !priority_mask.contains(self.priority) {
            return None;
        }

        if self.fragmented_transfer.is_none() {
            self.fragmented_transfer = self.pending_transfer.take();
        }

        let entry = self.fragmented_transfer.as_mut()?;

        // Safety: all buffers are part of the same allocation
        let buffer_ptr = unsafe { self.buffers_ptr.add(entry.buffer_idx.into()) };
        // Safety: the transfer entry has an exclusive access to its buffer
        let buffer = unsafe { buffer_ptr.as_ref().unwrap_unchecked() }.as_ref();

        let data = unwrap!(entry.scatter.fetch_frame_data(buffer, mtu));
        let frame = Some(Frame {
            header: Header {
                priority: self.priority,
                data_spec: DataSpecifier::Message(self.subject),
                source: None,
                destination: None,
            },
            data,
            timestamp: entry.timestamp,
            loop_back: entry.loop_back,
        });

        if entry.scatter.is_exhausted() {
            let entry = unwrap!(self.fragmented_transfer.take());
            unwrap!(self.free_buffer_indexes.push(entry.buffer_idx));
        }

        if self.is_empty() {
            self.empty_waker.wake();
        }

        frame
    }

    fn is_empty(&self) -> bool {
        self.fragmented_transfer.is_none() && self.pending_transfer.is_none()
    }

    fn poll_is_empty(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        if self.is_empty() {
            Poll::Ready(())
        } else {
            self.empty_waker.register(cx.waker());
            Poll::Pending
        }
    }

    fn push_readiness(&self) -> PrioritySet {
        PrioritySet::new_eq(self.priority)
    }

    fn poll_push_readiness(
        &mut self,
        _cx: &mut Context<'_>,
        priority_mask: PrioritySet,
    ) -> Poll<PrioritySet> {
        if priority_mask.contains(self.priority) {
            Poll::Ready(PrioritySet::new_eq(self.priority))
        } else {
            Poll::Pending
        }
    }

    fn get_scratchpad<'a>(
        &mut self,
        _buf_token: BufferToken<'a>,
    ) -> Option<(PrioritySet, &'a mut [u8])> {
        if self.scratchpad_buffer_idx.is_none() {
            if let Some(idx) = self.free_buffer_indexes.pop() {
                self.scratchpad_buffer_idx = Some(idx);
            } else {
                let entry = unwrap!(self.pending_transfer.take());
                self.scratchpad_buffer_idx = Some(entry.buffer_idx);
            }
        }

        let idx = unwrap!(self.scratchpad_buffer_idx);
        // Safety: all buffers are part of the same allocation
        let buffer_ptr = unsafe { self.buffers_ptr.add(idx.into()) };
        // Safety: the buffer allocations is guard by _buf_token, so it could not happen
        // as long as current token reference is alive
        let buffer = unsafe { buffer_ptr.as_mut().unwrap_unchecked() }.as_mut();
        Some((PrioritySet::new_eq(self.priority), buffer))
    }

    fn try_push(
        &mut self,
        _buf_token: BufferToken<'_>,
        meta: crate::endpoint::TransferMeta,
        length: usize,
        crc: TransferCrc,
    ) -> Result<(), BufferError> {
        if meta.address.is_some() {
            return Err(BufferError::MessageAddressSpecified);
        }

        if !self.push_readiness().contains(meta.priority) {
            return Err(BufferError::PriorityNotReady);
        }

        // Safety: the buffer allocations is guard by _buf_token
        let buffer_idx = self
            .scratchpad_buffer_idx
            .take()
            .ok_or(BufferError::ScratchpadNotInitialized)?;

        // Safety: all buffers are part of the same allocation
        let buffer_ptr = unsafe { self.buffers_ptr.add(buffer_idx.into()) };
        // Safety: the transfer entry has an exclusive access to its buffer
        let buffer = unsafe { buffer_ptr.as_ref().unwrap_unchecked() }.as_ref();

        let entry = TransferEntry {
            buffer_idx,
            scatter: Scatter::new(meta.transfer_id, buffer, length, crc),
            timestamp: meta.timestamp,
            loop_back: meta.loop_back,
        };

        if let Some(prev_transfer) = self.pending_transfer.replace(entry) {
            unwrap!(self.free_buffer_indexes.push(prev_transfer.buffer_idx));
        }

        Ok(())
    }
}

// Safety: Object owns a unique reference to the buffer array
unsafe impl<T: BufferType> Send for Inner<T> {}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use emcyphal_encoding::Serialize;
    use futures_test::task::new_count_waker;
    use heapless::Vec;

    use crate::core::TransferId;
    use crate::data_types::{ByteArray, Empty};
    use crate::frame::Data;

    const SUBJECT: SubjectId = SubjectId::from_truncating(100);
    const PRIORITY: Priority = Priority::Nominal;
    const PRIORITY_MASK: PrioritySet = PrioritySet::new_eq(PRIORITY);
    const DEADLINE: Instant = Instant::from_millis(100);
    const MTU: Mtu = Mtu::Classic;
    const LOOP_BACK: bool = false;

    fn make_buffer<T: BufferType>() -> (Inner<T>, BufferToken<'static>) {
        let mut raw_buffers = std::vec::Vec::<T::Buffer>::new();
        raw_buffers.resize_with(2, Default::default);
        let raw_buffers = std::boxed::Box::new(raw_buffers).leak();
        let buffer = unsafe {
            Inner::new(
                SUBJECT,
                PRIORITY,
                raw_buffers.as_mut_ptr(),
                raw_buffers.len(),
            )
        };
        let buf_token = unsafe { BufferToken::create() };
        (buffer, buf_token)
    }

    fn push_transfer<T: Serialize>(
        buffer: &mut dyn DynamicTxBuffer,
        mut token: BufferToken<'_>,
        payload: &T,
        transfer_id: TransferId,
    ) -> Result<(), ()> {
        let (priorities, scratchpad) = buffer.get_scratchpad(token.reborrow()).ok_or(())?;
        if !priorities.contains(PRIORITY) {
            return Err(());
        }
        payload.serialize_to_bytes(scratchpad);
        let length = payload.size_bits().div_ceil(8);
        let mut crc: TransferCrc = Default::default();
        crc.add_bytes(&scratchpad.as_ref()[..length]);
        let meta = crate::endpoint::TransferMeta {
            priority: PRIORITY,
            address: None,
            transfer_id,
            timestamp: DEADLINE,
            loop_back: LOOP_BACK,
        };
        buffer.try_push(token, meta, length, crc).unwrap();
        Ok(())
    }

    fn tid(value: u8) -> TransferId {
        TransferId::from_truncating(value)
    }

    #[test]
    fn test_single_frame_transfer() {
        let (mut buffer, mut token) = make_buffer::<ByteArray>();
        let msg = ByteArray {
            bytes: Vec::from_slice(&[0, 1, 2, 3]).unwrap(),
        };

        assert_eq!(buffer.push_readiness(), PRIORITY_MASK);
        assert_eq!(buffer.pop_readiness(), PrioritySet::NONE);
        let frame = buffer.try_pop(PrioritySet::ALL, MTU);
        assert_eq!(frame, None);

        push_transfer(&mut buffer, token.reborrow(), &msg, tid(0)).unwrap();

        assert_eq!(buffer.pop_readiness(), PRIORITY_MASK);

        let frame = buffer.try_pop(PrioritySet::ALL, MTU);
        assert_eq!(
            frame,
            Some(Frame {
                header: Header {
                    priority: PRIORITY,
                    data_spec: DataSpecifier::Message(SUBJECT),
                    source: None,
                    destination: None,
                },
                data: Data::new(&[4, 0, 0, 1, 2, 3, 0b1110_0000]).unwrap(),
                timestamp: DEADLINE,
                loop_back: LOOP_BACK,
            })
        );

        assert_eq!(buffer.pop_readiness(), PrioritySet::NONE);
        let frame = buffer.try_pop(PrioritySet::ALL, MTU);
        assert_eq!(frame, None);
    }

    #[test]
    fn test_multiframe_transfer() {
        let (mut buffer, mut token) = make_buffer::<ByteArray>();
        let msg = ByteArray {
            bytes: Vec::from_slice(&[0, 1, 2, 3, 4, 5]).unwrap(),
        };

        assert_eq!(buffer.push_readiness(), PRIORITY_MASK);
        assert_eq!(buffer.pop_readiness(), PrioritySet::NONE);
        let frame = buffer.try_pop(PrioritySet::ALL, MTU);
        assert_eq!(frame, None);

        push_transfer(&mut buffer, token.reborrow(), &msg, tid(0)).unwrap();

        assert_eq!(buffer.pop_readiness(), PRIORITY_MASK);
        let frame = buffer.try_pop(PrioritySet::ALL, MTU).unwrap();
        assert_eq!(frame.data.as_ref(), [6, 0, 0, 1, 2, 3, 4, 0b1010_0000]);

        assert_eq!(buffer.pop_readiness(), PRIORITY_MASK);
        let frame = buffer.try_pop(PrioritySet::ALL, MTU).unwrap();
        assert_eq!(frame.data.as_ref(), [5, 0x33, 0xfd, 0b0100_0000]);

        assert_eq!(buffer.pop_readiness(), PrioritySet::NONE);
        assert_eq!(buffer.try_pop(PrioritySet::ALL, MTU), None);
    }

    #[test]
    fn test_replace() {
        let (mut buffer, mut token) = make_buffer::<Empty>();

        let msg = Empty {};
        push_transfer(&mut buffer, token.reborrow(), &msg, tid(0)).unwrap();
        push_transfer(&mut buffer, token.reborrow(), &msg, tid(1)).unwrap();

        let frame = buffer.try_pop(PRIORITY_MASK, MTU).unwrap();
        assert_eq!(frame.data.as_ref(), [0b1110_0000 + 1]);

        let res = buffer.try_pop(PRIORITY_MASK, MTU);
        assert_eq!(res, None);
    }

    #[test]
    fn test_replace_concurrent() {
        let (mut buffer, mut token) = make_buffer::<Empty>();

        let msg = Empty {};
        push_transfer(&mut buffer, token.reborrow(), &msg, tid(0)).unwrap();

        // Get scratchpad for replacement
        unwrap!(buffer.get_scratchpad(token.reborrow()));

        // Send previous message during replacement
        let frame = buffer.try_pop(PRIORITY_MASK, MTU).unwrap();
        assert_eq!(frame.data.as_ref(), [0b1110_0000 + 0]);

        push_transfer(&mut buffer, token.reborrow(), &msg, tid(1)).unwrap();

        let frame = buffer.try_pop(PRIORITY_MASK, MTU).unwrap();
        assert_eq!(frame.data.as_ref(), [0b1110_0000 + 1]);

        let res = buffer.try_pop(PRIORITY_MASK, MTU);
        assert_eq!(res, None);
    }

    #[test]
    fn test_replace_multiframe() {
        let (mut buffer, mut token) = make_buffer::<ByteArray>();

        let msg = ByteArray {
            bytes: Vec::from_slice(&[0, 1, 2, 3, 4, 5, 6, 7, 8]).unwrap(),
        };
        push_transfer(&mut buffer, token.reborrow(), &msg, tid(0)).unwrap();

        // Start multi-frame transmission
        let frame = buffer.try_pop(PRIORITY_MASK, MTU).unwrap();
        assert_eq!(*frame.data.last().unwrap(), 0b1010_0000 + 0);

        push_transfer(&mut buffer, token.reborrow(), &msg, tid(1)).unwrap();
        push_transfer(&mut buffer, token.reborrow(), &msg, tid(2)).unwrap();

        // The started multi-frame transmission should complete anyway
        let frame = buffer.try_pop(PRIORITY_MASK, MTU).unwrap();
        assert_eq!(*frame.data.last().unwrap(), 0b0100_0000 + 0);

        // Second message is replaced
        let frame = buffer.try_pop(PRIORITY_MASK, MTU).unwrap();
        assert_eq!(*frame.data.last().unwrap(), 0b1010_0000 + 2);

        let frame = buffer.try_pop(PRIORITY_MASK, MTU).unwrap();
        assert_eq!(*frame.data.last().unwrap(), 0b0100_0000 + 2);

        let res = buffer.try_pop(PRIORITY_MASK, MTU);
        assert_eq!(res, None);
    }

    #[test]
    fn test_replace_multiframe_concurrent() {
        let (mut buffer, mut token) = make_buffer::<ByteArray>();

        let msg = ByteArray {
            bytes: Vec::from_slice(&[0, 1, 2, 3, 4, 5, 6, 7, 8]).unwrap(),
        };
        push_transfer(&mut buffer, token.reborrow(), &msg, tid(0)).unwrap();

        // Start multi-frame transmission
        let frame = buffer.try_pop(PRIORITY_MASK, MTU).unwrap();
        assert_eq!(*frame.data.last().unwrap(), 0b1010_0000 + 0);

        push_transfer(&mut buffer, token.reborrow(), &msg, tid(1)).unwrap();

        // Get scratchpad for the third transfer
        unwrap!(buffer.get_scratchpad(token.reborrow()));

        // Complete the started transfer anyway
        let frame = buffer.try_pop(PRIORITY_MASK, MTU).unwrap();
        assert_eq!(*frame.data.last().unwrap(), 0b0100_0000 + 0);

        // Only single transfer (complete or incomplete) can be sent during update
        let res = buffer.try_pop(PRIORITY_MASK, MTU);
        assert_eq!(res, None);
    }

    #[test]
    fn test_empty() {
        let (waker, count) = new_count_waker();
        let cx = &mut Context::from_waker(&waker);
        assert_eq!(count.get(), 0);

        let (mut buffer, mut token) = make_buffer::<Empty>();
        let msg = Empty {};

        assert_eq!(buffer.is_empty(), true);
        assert_eq!(buffer.poll_is_empty(cx), Poll::Ready(()));
        assert_eq!(count.get(), 0);

        push_transfer(&mut buffer, token.reborrow(), &msg, tid(0)).unwrap();

        assert_eq!(buffer.is_empty(), false);
        assert_eq!(buffer.poll_is_empty(cx), Poll::Pending);
        assert_eq!(count.get(), 0);

        buffer.try_pop(PRIORITY_MASK, MTU).unwrap();

        assert_eq!(buffer.is_empty(), true);
        assert_eq!(count.get(), 1);
    }
}
