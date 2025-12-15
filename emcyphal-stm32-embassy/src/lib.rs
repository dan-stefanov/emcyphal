//! STM32 FDCAN driver adapter for the Emcyphal stack
//!
//! This adapter integrates the FDCAN driver from the Embassy framework with the Emcyphal stack.
//!
//! # Features
//!
//! * 7 subject filters
//! * 1 destination (node address) filter
//! * Partial TX timeout support
//!
//! # Limitations
//!
//! * Anonymous frame transmission is not supported
//! * Loop-back frames are not supported
//! * Frame ordering is partially preserved per priority; frames from different sessions may
//!   preempt each other
//! * Partial TX timeout support: timestamps are checked when inserting frames into the 3-element
//!   hardware queue. Frames may remain enqueued indefinitely if the bus is occupied by
//!   higher-priority traffic
//! * Discontinuous transmission: a runner task wakes up to initial transmission of each frame
//!   within the same session
//! * A 32-frame buffer is required for worst-case preemption
//! * embassy-stm32 v0.4.0 does not enable transmitter delay compensation; data bit rates
//!   above 4 Mbps may be unavailable with typical transceivers
//!
//! # Examples
//!
//! See the `pubsub_native` example in the 
//! [nucleo-g431rb](https://github.com/dan-stefanov/emcyphal/tree/emcyphal-stm32-embassy-v0.1.0/nucleo-g431rb)
//! crate.

#![no_std]

// This mod MUST go first, so that the others see its macros.
pub(crate) mod fmt;

pub mod config;
mod driver;
mod format;
mod utils;

pub use driver::SUBJECT_SLOT_COUNT;
pub use driver::{RxFilterRunner, RxRunner, TxRunner, bind};
