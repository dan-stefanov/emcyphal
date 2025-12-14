use core::ptr::NonNull;

use crate::buffer::BufferToken;
use crate::core::ServiceId;
use crate::registry;
use crate::time::Duration;

pub mod priority_fifo;

pub use priority_fifo::PriorityFifo;

pub(crate) trait SealedBuffer<T> {
    // Safety: User must drop the pointee in place before the buffer reference expired
    unsafe fn init(
        &mut self,
        service: ServiceId,
        timeout: Duration,
    ) -> (NonNull<registry::rx_req::Entry>, BufferToken<'_>);
}

#[allow(private_bounds)]
pub trait Buffer<T>: SealedBuffer<T> {}
