use core::mem::MaybeUninit;
use core::ptr::NonNull;
use core::task::{Context, Poll};
use embassy_sync::waitqueue::WakerRegistration;
use emcyphal_encoding::BufferType;

use super::{Buffer, SealedBuffer};
use crate::buffer::scatter::Scatter;
use crate::buffer::{BufferError, BufferToken, DynamicTxBuffer, TransferMeta};
use crate::core::{Priority, PrioritySet, SubjectId};
use crate::format::TransferCrc;
use crate::frame::{DataSpecifier, Frame, Header, Mtu};
use crate::registry;
use crate::time::Instant;

/// Blocking message buffer
///
/// The buffer can store a single transfer of a predefined priority. The buffer will not accept
/// a new transfer until the node fetches all fragments of the previous transfer.
///
/// The buffer reserves space for one data type buffer.
pub struct Blocking<T: BufferType> {
    buffer: T::Buffer,
    inner: MaybeUninit<Inner<T>>,
    entry: MaybeUninit<registry::tx_msg::Entry>,
}

impl<T: BufferType + 'static> Blocking<T> {
    pub fn new() -> Self {
        Self {
            buffer: Default::default(),
            inner: MaybeUninit::uninit(),
            entry: MaybeUninit::uninit(),
        }
    }
}

impl<T: BufferType + 'static> Default for Blocking<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: BufferType + 'static> SealedBuffer<T> for Blocking<T> {
    unsafe fn init(
        &mut self,
        subject: SubjectId,
        priority: Priority,
    ) -> (NonNull<registry::tx_msg::Entry>, BufferToken<'_>) {
        let buffer = &mut self.buffer;
        let inner = &mut self.inner;
        let entry = &mut self.entry;

        // Safety: Entry will drop the object before the buffer reference expired
        let buffer_value = unsafe { Inner::new(subject, priority, buffer) };
        let buffer_ptr = NonNull::from_mut(inner.write(buffer_value));

        // Safety: User must drop the object before the buffer reference expired
        let entry_value = unsafe { registry::tx_msg::Entry::new(subject, buffer_ptr) };
        let entry_ptr = NonNull::from_mut(entry.write(entry_value));

        // Safety: object lifetime is bounded by original reference
        let buf_token = unsafe { BufferToken::create() };

        (entry_ptr, buf_token)
    }
}

impl<T: BufferType + 'static> Buffer<T> for Blocking<T> {}

struct TransferEntry {
    scatter: Scatter,
    deadline: Instant,
    loop_back: bool,
}

struct Inner<T: BufferType> {
    subject: SubjectId,
    priority: Priority,
    buffer_ptr: *mut T::Buffer,
    pending_transfer: Option<TransferEntry>,
    scratchpad_provided: bool,
    non_full_trigger: WakerRegistration,
    empty_trigger: WakerRegistration,
}

impl<T: BufferType> Inner<T> {
    // Safety: The buffer must outlive the object
    pub unsafe fn new(subject: SubjectId, priority: Priority, buffer_ptr: *mut T::Buffer) -> Self {
        assert!(!buffer_ptr.is_null());

        Self {
            subject,
            priority,
            buffer_ptr,
            pending_transfer: None,
            scratchpad_provided: false,
            non_full_trigger: WakerRegistration::new(),
            empty_trigger: WakerRegistration::new(),
        }
    }
}

impl<T: BufferType> DynamicTxBuffer for Inner<T> {
    fn pop_readiness(&self) -> PrioritySet {
        if self.pending_transfer.is_some() {
            PrioritySet::new_eq(self.priority)
        } else {
            PrioritySet::NONE
        }
    }

    fn try_pop(&mut self, priority_mask: PrioritySet, mtu: Mtu) -> Option<Frame> {
        if (self.pop_readiness() & priority_mask).is_empty() {
            return None;
        }

        let entry = unwrap!(self.pending_transfer.as_mut());

        // Safety: the pending_transfer has exclusive ownership of the buffer
        let buffer = unsafe { self.buffer_ptr.as_mut().unwrap_unchecked() }.as_mut();

        let data = unwrap!(entry.scatter.fetch_frame_data(buffer, mtu));
        let frame = Some(Frame {
            header: Header {
                priority: self.priority,
                data_spec: DataSpecifier::Message(self.subject),
                source: None,
                destination: None,
            },
            data,
            timestamp: entry.deadline,
            loop_back: entry.loop_back,
        });

        if entry.scatter.is_exhausted() {
            self.pending_transfer = None;
            self.non_full_trigger.wake();
            self.empty_trigger.wake();
        }

        frame
    }

    fn is_empty(&self) -> bool {
        self.pending_transfer.is_none()
    }

    fn poll_is_empty(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        if self.is_empty() {
            Poll::Ready(())
        } else {
            self.empty_trigger.register(cx.waker());
            Poll::Pending
        }
    }

    fn push_readiness(&self) -> PrioritySet {
        if self.pending_transfer.is_none() {
            PrioritySet::new_eq(self.priority)
        } else {
            PrioritySet::NONE
        }
    }

    fn poll_push_readiness(
        &mut self,
        cx: &mut Context<'_>,
        priority_mask: PrioritySet,
    ) -> Poll<PrioritySet> {
        if priority_mask.contains(self.priority) {
            if self.pending_transfer.is_none() {
                Poll::Ready(PrioritySet::new_eq(self.priority))
            } else {
                self.non_full_trigger.register(cx.waker());
                Poll::Pending
            }
        } else {
            Poll::Pending
        }
    }

    fn get_scratchpad<'a>(
        &mut self,
        _buf_token: BufferToken<'a>,
    ) -> Option<(PrioritySet, &'a mut [u8])> {
        let priorities = self.push_readiness();
        if priorities.is_empty() {
            return None;
        }

        self.scratchpad_provided = true;

        assert!(self.pending_transfer.is_none());
        // Safety: the buffer access is guard by _buf_token
        let buffer = unsafe { self.buffer_ptr.as_mut().unwrap_unchecked() }.as_mut();
        Some((priorities, buffer))
    }

    fn try_push(
        &mut self,
        _buf_token: BufferToken<'_>,
        meta: TransferMeta,
        length: usize,
        crc: TransferCrc,
    ) -> Result<(), BufferError> {
        if meta.address.is_some() {
            return Err(BufferError::MessageAddressSpecified);
        }

        if !self.push_readiness().contains(meta.priority) {
            return Err(BufferError::PriorityNotReady);
        }

        if !self.scratchpad_provided {
            return Err(BufferError::ScratchpadNotInitialized);
        }

        self.scratchpad_provided = false;

        // Safety: the buffer access is guarded by _buf_token
        let buffer = unsafe { self.buffer_ptr.as_mut().unwrap_unchecked() }.as_mut();

        let scatter = Scatter::new(meta.transfer_id, buffer, length, crc);
        let entry = TransferEntry {
            scatter,
            deadline: meta.timestamp,
            loop_back: meta.loop_back,
        };
        let prev = self.pending_transfer.replace(entry);
        assert!(prev.is_none());
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
        let raw_buffer = T::Buffer::default();
        let raw_buffer = std::boxed::Box::leak(std::boxed::Box::new(raw_buffer));
        let buffer = unsafe { Inner::new(SUBJECT, PRIORITY, raw_buffer) };
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
    fn test_back_pressure() {
        let (waker, count) = new_count_waker();
        let cx = &mut Context::from_waker(&waker);
        assert_eq!(count.get(), 0);

        let (mut buffer, mut token) = make_buffer::<Empty>();
        let msg = Empty {};

        assert_eq!(buffer.push_readiness(), PRIORITY_MASK);
        assert_eq!(
            buffer.poll_push_readiness(cx, PRIORITY_MASK),
            Poll::Ready(PRIORITY_MASK)
        );
        assert_eq!(count.get(), 0);

        push_transfer(&mut buffer, token.reborrow(), &msg, tid(0)).unwrap();

        assert_eq!(buffer.push_readiness(), PrioritySet::NONE);
        assert_eq!(buffer.poll_push_readiness(cx, PRIORITY_MASK), Poll::Pending);
        assert_eq!(count.get(), 0);

        buffer.try_pop(PRIORITY_MASK, MTU).unwrap();

        assert_eq!(buffer.push_readiness(), PRIORITY_MASK);
        assert_eq!(count.get(), 1);
    }

    #[test]
    fn test_back_pressure_multiframe() {
        let (waker, count) = new_count_waker();
        let cx = &mut Context::from_waker(&waker);
        assert_eq!(count.get(), 0);

        let (mut buffer, mut token) = make_buffer::<ByteArray>();
        let msg = ByteArray {
            bytes: Vec::from_slice(&[0, 1, 2, 3, 4, 5, 6, 7, 8]).unwrap(),
        };

        assert_eq!(buffer.push_readiness(), PRIORITY_MASK);
        assert_eq!(
            buffer.poll_push_readiness(cx, PRIORITY_MASK),
            Poll::Ready(PRIORITY_MASK)
        );
        assert_eq!(count.get(), 0);

        push_transfer(&mut buffer, token.reborrow(), &msg, tid(0)).unwrap();

        assert_eq!(buffer.push_readiness(), PrioritySet::NONE);
        assert_eq!(buffer.poll_push_readiness(cx, PRIORITY_MASK), Poll::Pending);
        assert_eq!(count.get(), 0);

        buffer.try_pop(PRIORITY_MASK, MTU).unwrap();

        assert_eq!(buffer.push_readiness(), PrioritySet::NONE);
        assert_eq!(buffer.poll_push_readiness(cx, PRIORITY_MASK), Poll::Pending);
        assert_eq!(count.get(), 0);

        buffer.try_pop(PRIORITY_MASK, MTU).unwrap();

        assert_eq!(buffer.push_readiness(), PRIORITY_MASK);
        assert_eq!(count.get(), 1);
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
