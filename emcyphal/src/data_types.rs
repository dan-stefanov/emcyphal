//! (De)serializable Rust representation for selected Cyphal data types
//!
//! This module include only types directly used by Emcyphal stack or its tests.
//! The library clients should use types for `emcyphal-data-types-generated` or a custom generated
//! crate.

mod byte_array;
mod empty;
pub(crate) mod node;

pub use byte_array::ByteArray;
pub use empty::Empty;
