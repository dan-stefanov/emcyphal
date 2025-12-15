//! (De)serializable Rust representations for selected Cyphal data types
//!
//! This module includes only types directly used by the Emcyphal stack or its tests.
//! Library clients should use standard types from the `emcyphal-data-types-generated` crate
//! and/or generate custom types using
//! [emcyphal-dsdl-codegen](https://github.com/dan-stefanov/emcyphal-dsdl-codegen).

mod byte_array;
mod empty;
pub(crate) mod node;

pub use byte_array::ByteArray;
pub use empty::Empty;
