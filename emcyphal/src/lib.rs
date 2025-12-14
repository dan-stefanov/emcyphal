//! # Emcyphal
//!
//! This library provides an async socket-like API for the Cyphal/CAN protocol \[1\] in no_std
//! environments. It uses user-provided buffers for (de)segmentation and queues, requiring no
//! dynamic memory allocation.
//!
//! The library is designed for systems with tight interrupt latency requirements, keeping all
//! critical section durations bounded.
//!
//! The library primarily targets the Embassy async framework, though tokio/SocketCAN extensions
//! may be possible in the future.
//!
//! ## Architecture
//!
//! ```text
//!           ┌────────┐                             
//!           │ Runner │                             
//!           └────┬───┘                             
//!                ▼                                 
//! ┌──────┐  ┌────────┐  ┌─────┐                    
//! │ Link ├─►│  Node  │◄─┤ Hub │                    
//! └──────┘  └──────┬─┘  └──┬──┘                    
//!                  │       │  ┌───────────────────┐
//!                  │       │  │ Subscriber socket │
//! ┌─────────────┐  │       │  │ ┌───────────────┐ │
//! │ RX Buffer 1 │◄─┤       │◄─┼─┤ RX Endpoint 1 │ │
//! └─────────────┘  │       │  │ └───────────────┘ │
//!                  │       │  └───────────────────┘
//!                  │       │  ┌───────────────────┐
//!                  │       │  │ Publisher socket  │
//! ┌─────────────┐  │       │  │ ┌───────────────┐ │
//! │ TX Buffer 1 │◄─┤       │◄─┼─┤ TX Endpoint 1 │ │
//! └─────────────┘  │       │  │ └───────────────┘ │
//!                  │       │  └───────────────────┘
//!                  │       │  ┌───────────────────┐
//!                  │       │  │ Responder socket  │
//! ┌─────────────┐  │       │  │ ┌───────────────┐ │
//! │ RX Buffer 2 │◄─┤       │◄─┼─┤ RX Endpoint 2 │ │
//! └─────────────┘  │       │  │ └───────────────┘ │
//! ┌─────────────┐  │       │  │ ┌───────────────┐ │
//! │ TX Buffer 2 │◄─┤       │◄─┼─┤ TX Endpoint 2 │ │
//! └─────────────┘  │       │  │ └───────────────┘ │
//!                             └───────────────────┘
//!                     ...                          
//! ```
//! Components:
//! * _Node_ holds Cyphal peer data (e.g., NodeId, Health, Mode) and type-agnostic links to
//!   registered buffers. It distributes incoming frames among RX buffers and collects outgoing
//!   frames from TX buffers. It also proxies and synchronizes endpoint access to the associated
//!   buffers.
//! * _Runner_ is a worker task for background node activities, e.g., publishing heartbeats and port lists,
//!   and answering GetInfo, GetTransportStatistics service requests.
//! * _Link_ is an asynchronous channel for CAN frames that a CAN peripheral driver consumes.
//! * _Hub_ is a shared handle for creating new endpoints.
//! * _Buffer_ is a type-aware object that manages (de)segmentation and a transfer queue.
//! * _Endpoint_ is a low-level API handle for sending or receiving transfers.
//!   It performs (de)serialization and guards the buffer's lifetime.
//! * _Socket_ is a higher-level API handle for networking. Users are encouraged to use sockets
//!   instead of endpoints when possible.
//!
//! Upon creation, an endpoint registers the associated buffer reference with the Hub.
//! The reference lifetime is bounded by the endpoint. Once dropped, it unregisters the buffer
//! immediately. This allows users to use temporarily allocated buffers within async function scopes.
//!
//! Network-capable objects are expected to consume sockets, allowing users to compose independent
//! components within a single Cyphal node.
//!
//! ## Concurrency model
//!
//! The Node owns unique references to the registered buffers and uses a mutex to synchronize
//! Driver, Runner, and endpoint accesses. There are two mutex implementation options:
//! * _CriticalSectionRawMutex_ allows stack components to run concurrently (at different
//!   interrupt levels), but can add bounded priority inversion (interrupt latency) to the
//!   rest of the system.
//! * _ThreadModeRawMutex_ has no system-wide effects but requires all stack components to run
//!   in a thread (non-interrupt) executor. Priority inversion is possible among endpoints,
//!   and long processing delays may affect the Driver.
//!
//! The library keeps critical section durations bounded to O(log(N)) CPU cycles, where N is the
//! number of endpoints of the given type. For example:
//! * The Node maintains the buffer index in a tree-like structure for deterministic worst-case complexity.
//! * Delivery of an incoming frame to each subscribed buffer runs in a dedicated critical section.
//! * (De)serialization runs in the caller's thread, outside the critical section.
//!   The mutex is held briefly for a zero-copy buffer swap.
//!
//! ## Cyphal data types
//!
//! The library relies on a modified code generator from the `canadensis` project to convert Cyphal
//! DSDL to (de)serializable Rust structs. The modified version annotates each data type with an
//! auxiliary buffer type that can hold the longest type serialization.
//!
//! ## Limitations
//!
//! * Service requests are not yet implemented.
//! * Loop-back transfers are supported for messages only.
//! * The no_std target supports single-CPU systems only (embassy_sync limitation).
//! * Redundant transports are not supported.
//! * Anonymous message publication is not supported.
//!
//! # References:
//!
//! * \[1\] Cyphal Specification v1.0
//!   <https://opencyphal.org/specification/Cyphal_Specification.pdf>
#![no_std]

pub use emcyphal_core as core;
pub use emcyphal_driver::{frame, time};

// This mod MUST go first, so that the others see its macros.
pub(crate) mod fmt;

pub mod buffer;
pub mod data_types;
pub mod endpoint;
mod format;
pub mod marker;
pub mod node;
mod registry;
pub mod socket;
#[allow(dead_code)]
mod utils;
