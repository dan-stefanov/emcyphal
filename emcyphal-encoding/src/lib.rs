//! Dependency crate for auto-generate data types
//! 
//! Emcyphal stack users should no depend on this crate directly.

#![no_std]

pub use canadensis_encoding::*;

pub trait BufferType {
    /// Buffer with capacity sufficient for correct message serialization
    type Buffer: Sized + Send + Sync + Default + AsMut<[u8]> + AsRef<[u8]> + 'static;
}

pub struct StaticBuffer<const N: usize>([u8; N]);

impl<const N: usize> Default for StaticBuffer<N> {
    fn default() -> Self {
        Self([0; N])
    }
}

impl<const N: usize> AsRef<[u8]> for StaticBuffer<N> {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl<const N: usize> AsMut<[u8]> for StaticBuffer<N> {
    fn as_mut(&mut self) -> &mut [u8] {
        &mut self.0
    }
}
