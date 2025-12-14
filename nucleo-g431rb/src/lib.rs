//! Emcyphal example project for the Nucleo-G431RB board.
//!
//! To use this crate on other STM32G4 boards, update the chip name in `Cargo.toml`
//! and `.cargo/config.toml`.
//!
//! All tests use external-loopback mode and require no additional hardware.
//! Output frames can be observed on the PA12 pin.

#![no_std]

pub mod board;
