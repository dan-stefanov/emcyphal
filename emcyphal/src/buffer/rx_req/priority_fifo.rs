use core::mem::MaybeUninit;
use core::ptr::NonNull;
use emcyphal_encoding::BufferType;

use crate::buffer::BufferToken;
use crate::buffer::rx_priority_fifo::RxPriorityFifoInner;
use crate::buffer::rx_req::{Buffer, SealedBuffer};
use crate::core::ServiceId;
use crate::frame::DataSpecifier;
use crate::registry;
use crate::time::Duration;
use crate::utils::TriplexArray;

pub use crate::buffer::rx_priority_fifo::{MAX_SESSION_COUNT, MAX_TRANSFER_COUNT};

/// Priority FIFO request buffer
///
/// The buffer maintains up to SN (0 <= SN <= 128) session slots and TN (1 <= TN <= 127) transfer
/// slots.
///
/// To receive non-anonymous transfers from a particular node, the buffer uses a dedicated session
/// slot. The slot tracks the source address, transfer-ID sequence, and accumulates multi-frame
/// transfers. Once a unique transfer is assembled, it is pushed to the transfer queue.
///
/// The buffer allocates a session slot when receiving a start-of-transfer (SOT) frame from an
/// untracked source address. After `timeout` has elapsed since the last SOT frame was received,
/// the slot may be reused for a different address.
///
/// Anonymous transfers do not require session slots and are pushed to the transfer queue
/// immediately.
///
/// The transfer queue organizes transfers of each priority in FIFO order. Each transfer occupies
/// a transfer slot until fetched. If all slots are occupied, the queue displaces the oldest
/// transfer of the highest priority, provided the incoming transfer has equal or grater priority.
///
/// The buffer reserves space for SN + TN + 1 data type buffers. It uses the additional data type
/// buffer to enable a zero-copy readout.
pub struct PriorityFifo<T: BufferType, const SN: usize, const TN: usize> {
    buffers: TriplexArray<T::Buffer, 1, SN, TN>,
    inner: MaybeUninit<RxPriorityFifoInner<T, SN, TN>>,
    entry: MaybeUninit<registry::rx_req::Entry>,
}

impl<T: BufferType + 'static, const SN: usize, const TN: usize> PriorityFifo<T, SN, TN> {
    pub fn new() -> Self {
        Self {
            buffers: Default::default(),
            inner: MaybeUninit::uninit(),
            entry: MaybeUninit::uninit(),
        }
    }
}

impl<T: BufferType + 'static, const SN: usize, const TN: usize> Default
    for PriorityFifo<T, SN, TN>
{
    fn default() -> Self {
        Self::new()
    }
}

impl<T: BufferType + 'static, const SN: usize, const TN: usize> SealedBuffer<T>
    for PriorityFifo<T, SN, TN>
{
    unsafe fn init(
        &mut self,
        service: ServiceId,
        timeout: Duration,
    ) -> (NonNull<registry::rx_req::Entry>, BufferToken<'_>) {
        let buffers: &mut [T::Buffer] = &mut self.buffers;
        let inner = &mut self.inner;
        let entry = &mut self.entry;

        // Safety: Entry will drop the object before the buffers reference expired
        let buffer_value = unsafe {
            RxPriorityFifoInner::new(
                DataSpecifier::Request(service),
                timeout,
                buffers.as_mut_ptr(),
                buffers.len(),
            )
        };
        let buffer_ptr = NonNull::from(inner.write(buffer_value));

        // Safety: The user must drop the object before the buffers reference expired
        let entry_value = unsafe { registry::rx_req::Entry::new(service, buffer_ptr) };
        let entry_ptr = NonNull::from(entry.write(entry_value));

        // Safety: object lifetime is bounded by original reference
        let buf_token = unsafe { BufferToken::create() };

        (entry_ptr, buf_token)
    }
}

impl<T: BufferType + 'static, const SN: usize, const TN: usize> Buffer<T>
    for PriorityFifo<T, SN, TN>
{
}
