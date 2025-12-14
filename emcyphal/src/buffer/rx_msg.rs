use core::ptr::NonNull;

use crate::buffer::BufferToken;
use crate::core::SubjectId;
use crate::registry;
use crate::time::Duration;

pub mod priority_fifo;
pub mod watch;

pub use priority_fifo::PriorityFifo;
pub use watch::Watch;

pub(crate) trait SealedBuffer<T> {
    // Safety: User must drop the pointee in place before the buffer reference expired
    unsafe fn init(
        &mut self,
        subject: SubjectId,
        timeout: Duration,
    ) -> (NonNull<registry::rx_msg::Entry>, BufferToken<'_>);
}

#[allow(private_bounds)]
pub trait Buffer<T>: SealedBuffer<T> {}
