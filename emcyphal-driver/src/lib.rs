//! Emcyphal driver interface
//!
//! The crate provides an interface between CAN device driver and the Emcyphal stack.
//! Limited scope facilitates compatibility across versions.
//! Driver crates should depend on this crate. Emcyphal stack users should depend on
//! the `emcyphal` crate instead.
//!
//! A `Link` encompasses three asynchronous channels:
//! * `RxFilter` produces receiver filter updates
//! * `Rx` consumes received frames
//! * `Tx` produces frames for transmission
//!
//! Unlike other network stack implementations, Emcyphal relies on driver runners to pull
//! and push data. This design works because the basic stack structures are channel-like,
//! while common drivers need their own task to dispatch preempted frames. Thus, the inverse
//! structure eliminates intermediate channels and redundant runners.
//!
//! A driver should be able to filter in message frames on specified subjects, though it may
//! limit the number of simultaneous subscriptions. `RxFilter` provides a stream of subscription
//! add and removal requests. Range removal requests simplify asynchronous cleanup:
//! the node does not need to store all removed subjects but only mark a remaining neighbor.
//!
//! A driver should be able to filter in service frames with the specified destination, though
//! it may limit the number of simultaneous destinations. The node is expected to set the filter
//! to its own address. In properly configured Cyphal networks, there should not be many unexpected
//! frames designated for the node address, so the stack does not need filters for service IDs.
//! `RxFilter` provides a stream of subscription add and removal requests. Range removal requests
//! were added for consistency.
//!
//! Though the `Rx` channel is asynchronous, a driver may expect only short-term blockage.
//! Such design allows the node to use async mutex for better concurrency management.
//! However, it should not be used to exert back-pressure on the driver.

#![no_std]

// This mod MUST go first, so that the others see its macros.
pub(crate) mod fmt;

pub mod frame;
pub mod internal;
pub mod link;

pub mod time {
    pub use embassy_time::{Duration, Instant};
}
