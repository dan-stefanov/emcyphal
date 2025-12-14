use core::ptr::NonNull;

use super::BufferToken;
use crate::core::ServiceId;
use crate::registry::tx_resp::Entry;

mod blocking;

pub use blocking::Blocking;

pub(crate) trait SealedBuffer<T> {
    // Safety: User must drop the pointee in place before the buffer reference expired
    unsafe fn init(&mut self, service: ServiceId) -> (NonNull<Entry>, BufferToken<'_>);
}

#[allow(private_bounds)]
pub trait Buffer<T>: SealedBuffer<T> {}
