//! Native STM32 FDCAN driver for the Emcyphal stack
//!
//! This implementation uses direct register access for better integration with the hardware.
//!
//! # Features
//!
//! * 14 subject filters
//! * 1 destination (node address) filter
//! * Software and hardware timestamps
//! * Loop-back frames for TX timestamping
//! * Full TX timeout support
//! * Transmitter delay compensation for data bit rates up to 8 Mbps
//!
//! # Limitations
//!
//! * Anonymous frame transmission is not supported
//! * System timestamps are measured in the runner task
//! * Discontinuous transmission: a runner task wakes up to initial transmission of each frame
//! * An 8-frame buffer is allocated even in Classic mode
//!
//! # Feature flags
//!
//! To use this crate:
//! * Include the `embassy-stm32` crate with appropriate flags for your target chip
//! * Choose an STM32 chip family flag: `stm32g4` (the same code should work on G0 and L5 families,
//!   but has not been tested)
//! * Enable bindings for available peripherals: `fdcan1`, `fdcan2`, `fdcan3`. Multiple instances
//!   are supported.
//!
//! # Examples
//!
//! See the `pubsub_native` example in the `nucleo-g431rb` project.
#![no_std]

#[cfg(not(feature = "stm32g4"))]
compile_error!("At least one target family should be chosen");

// This mod MUST go first, so that the others see its macros.
pub(crate) mod fmt;

pub mod config;
mod driver;
mod format;
mod message_ram;
#[allow(unused_imports, unused_macros)]
mod periphery;
mod raw;
mod utils;

pub use driver::SUBJECT_SLOT_COUNT;
pub use driver::{Driver, RxFilterRunner, RxRunner, TxRunner};
pub use driver::{IT0InterruptHandler, IT1InterruptHandler};
