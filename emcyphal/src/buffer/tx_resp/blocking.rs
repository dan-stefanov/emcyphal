use core::mem::MaybeUninit;
use core::ptr::NonNull;
use core::task::{Context, Poll};
use embassy_sync::waitqueue::WakerRegistration;
use emcyphal_core::{NodeId, Priority};
use emcyphal_encoding::BufferType;
use heapless::Vec;

use super::{Buffer, SealedBuffer};
use crate::buffer::scatter::Scatter;
use crate::buffer::{BufferError, BufferToken, DynamicTxBuffer, TransferMeta};
use crate::core::{PrioritySet, ServiceId};
use crate::format::TransferCrc;
use crate::frame::{DataSpecifier, Frame, Header, Mtu};
use crate::registry;
use crate::time::Instant;
use crate::utils::{PriorityMap, PriorityTrigger};

const BUFFER_COUNT: usize = 1 + Priority::MAX_VALUE as usize;

/// Blocking response buffer
///
/// The buffer can store a single transfer for each priority. The buffer will not accept
/// a new transfer until the node fetches all fragments of the previous transfer of the
/// same priority.
///
/// The buffer reserves space for 8 data type buffers.
pub struct Blocking<T: BufferType> {
    buffers: [T::Buffer; BUFFER_COUNT],
    inner: MaybeUninit<ResponseBufferInner<T>>,
    entry: MaybeUninit<registry::tx_resp::Entry>,
}

impl<T: BufferType + 'static> Blocking<T> {
    pub fn new() -> Self {
        Self {
            buffers: Default::default(),
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
        service: ServiceId,
    ) -> (NonNull<registry::tx_resp::Entry>, BufferToken<'_>) {
        let buffers: &mut [T::Buffer] = &mut self.buffers;
        let inner = &mut self.inner;
        let entry = &mut self.entry;

        // Safety: Entry will drop the object before the buffers reference expired
        let buffer_value =
            unsafe { ResponseBufferInner::new(service, buffers.as_mut_ptr(), buffers.len()) };
        let buffer_ptr = NonNull::from_mut(inner.write(buffer_value));

        // Safety: User must drop the object before the buffers reference expired
        let entry_value = unsafe { registry::tx_resp::Entry::new(buffer_ptr) };
        let entry_ptr = NonNull::from_mut(entry.write(entry_value));

        // Safety: object lifetime is bounded by original reference
        let buf_token = unsafe { BufferToken::create() };

        (entry_ptr, buf_token)
    }
}

impl<T: BufferType + 'static> Buffer<T> for Blocking<T> {}

struct TransferEntry {
    address: NodeId,
    scatter: Scatter,
    deadline: Instant,
    loop_back: bool,
    buffer_idx: u8,
}

struct ResponseBufferInner<T: BufferType> {
    service: ServiceId,
    buffers_ptr: *mut T::Buffer,
    scratchpad_buffer_idx: Option<u8>,
    free_buffer_idx: Vec<u8, BUFFER_COUNT>,
    transfers: PriorityMap<TransferEntry>,
    non_full_trigger: PriorityTrigger,
    empty_trigger: WakerRegistration,
}

impl<T: BufferType> ResponseBufferInner<T> {
    // Safety: The buffers must outlive the object
    pub unsafe fn new(service: ServiceId, buffers_ptr: *mut T::Buffer, buffers_len: usize) -> Self {
        assert!(!buffers_ptr.is_null());
        assert_eq!(buffers_len, BUFFER_COUNT);

        let buffer_count = unwrap!(u8::try_from(BUFFER_COUNT));

        Self {
            service,
            buffers_ptr,
            scratchpad_buffer_idx: None,
            free_buffer_idx: (0..buffer_count).collect(),
            transfers: PriorityMap::new(),
            non_full_trigger: PriorityTrigger::new(),
            empty_trigger: WakerRegistration::new(),
        }
    }
}

impl<T: BufferType> DynamicTxBuffer for ResponseBufferInner<T> {
    fn pop_readiness(&self) -> PrioritySet {
        self.transfers.keys()
    }

    fn try_pop(&mut self, priority_mask: PrioritySet, mtu: Mtu) -> Option<Frame> {
        let priority = (self.transfers.keys() & priority_mask).first()?;
        let entry: &mut TransferEntry = &mut self.transfers[priority];

        // Safety: all buffers are part of the same allocation
        let buffer_ptr = unsafe { self.buffers_ptr.add(entry.buffer_idx.into()) };
        // Safety: the function has exclusive ownership of the allocated buffer
        let buffer = unsafe { buffer_ptr.as_mut().unwrap_unchecked() }.as_mut();

        let data = unwrap!(entry.scatter.fetch_frame_data(buffer, mtu));
        let frame = Some(Frame {
            header: Header {
                priority,
                data_spec: DataSpecifier::Response(self.service),
                source: None,
                destination: Some(entry.address),
            },
            data,
            timestamp: entry.deadline,
            loop_back: entry.loop_back,
        });

        if entry.scatter.is_exhausted() {
            unwrap!(self.free_buffer_idx.push(entry.buffer_idx));
            self.transfers.remove(priority);
            self.non_full_trigger.wake(PrioritySet::new_eq(priority));
        }

        if self.transfers.is_empty() {
            self.empty_trigger.wake();
        }

        frame
    }

    fn is_empty(&self) -> bool {
        self.transfers.is_empty()
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
        let occupied = self.transfers.keys();
        !occupied
    }

    fn poll_push_readiness(
        &mut self,
        cx: &mut Context<'_>,
        priority_mask: PrioritySet,
    ) -> Poll<PrioritySet> {
        let priorities = self.push_readiness() & priority_mask;
        if !priorities.is_empty() {
            Poll::Ready(priorities)
        } else {
            self.non_full_trigger.register(cx.waker(), priority_mask);
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

        if self.scratchpad_buffer_idx.is_none() {
            let idx = unwrap!(self.free_buffer_idx.pop());
            self.scratchpad_buffer_idx = Some(idx);
        }

        let idx = unwrap!(self.scratchpad_buffer_idx);
        // Safety: all buffers are part of the same allocation
        let buffer_ptr = unsafe { self.buffers_ptr.add(idx.into()) };
        // Safety: the buffer allocation is guard by _buf_token
        let buffer = unsafe { buffer_ptr.as_mut().unwrap_unchecked() }.as_mut();
        Some((priorities, buffer))
    }

    fn try_push(
        &mut self,
        _buf_token: BufferToken<'_>,
        meta: TransferMeta,
        length: usize,
        crc: TransferCrc,
    ) -> Result<(), BufferError> {
        let address = meta
            .address
            .ok_or(BufferError::ServiceAddressNotSpecified)?;
        if !self.push_readiness().contains(meta.priority) {
            return Err(BufferError::PriorityNotReady);
        }
        let buffer_idx = self
            .scratchpad_buffer_idx
            .take()
            .ok_or(BufferError::ScratchpadNotInitialized)?;

        // Safety: all buffers are part of the same allocation
        let buffer_ptr = unsafe { self.buffers_ptr.add(buffer_idx.into()) };
        // Safety: the allocation is guarded by _buf_token
        let buffer = unsafe { buffer_ptr.as_mut().unwrap_unchecked() }.as_mut();

        let scatter = Scatter::new(meta.transfer_id, buffer, length, crc);
        let entry = TransferEntry {
            address,
            scatter,
            deadline: meta.timestamp,
            buffer_idx,
            loop_back: meta.loop_back,
        };
        let prev = self.transfers.insert(meta.priority, entry);
        assert!(prev.is_none());
        Ok(())
    }
}

// Safety: Object owns a unique reference to the buffer array
unsafe impl<T: BufferType> Send for ResponseBufferInner<T> {}
