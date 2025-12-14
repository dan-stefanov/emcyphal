use core::ptr::NonNull;

use crate::buffer::BufferToken;
use crate::core::{Priority, SubjectId};
use crate::registry::tx_msg::Entry;

mod blocking;
mod signal;

pub use blocking::Blocking;
pub use signal::Signal;

pub(crate) trait SealedBuffer<T> {
    // Safety: User must drop the pointee in place before the buffer reference expired
    unsafe fn init(
        &mut self,
        subject: SubjectId,
        priority: Priority,
    ) -> (NonNull<Entry>, BufferToken<'_>);
}

#[allow(private_bounds)]
pub trait Buffer<T>: SealedBuffer<T> {}
