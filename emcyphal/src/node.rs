//! Cyphal node implementation
//!
//! Node holds Cyphal peer data (e.g. NodeId, Health, Mode) and type-agnostic links to
//! registered buffers. It distributes incoming frames among RX buffers and collects outgoing
//! frames from TX ones. It also proxies and synchronizes endpoint access to the associated
//! buffers.
//!
//! ## Examples
//!
//! A minimal node can be created as simply as:
//! ```
//! use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex as Mutex;
//! use emcyphal::core::NodeId;
//! use emcyphal::node::{MinimalNode, Status};
//!
//! // A statically assigned Cyphal node identifier. Must be unique within a network.
//! const NODE_ID: NodeId = NodeId::new(10).unwrap();   
//! // A property of a driver.
//! const SUBJECT_SLOT_COUNT: usize = 7;
//! // Initial node status, can be updated through `control`.
//! let status = Status::default();
//!
//! let mut node = MinimalNode::<Mutex>::new(NODE_ID, status, SUBJECT_SLOT_COUNT);
//! let (hub, link, control, runner) = node.split();
//! ```
//! However, static allocation is typically used to obtain `'static` accessors
//! that can be passed to spawned tasks:
//! ```
//! # use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex as Mutex;
//! # use emcyphal::core::NodeId;
//! # use emcyphal::node::{MinimalNode, Status};
//! # const NODE_ID: NodeId = NodeId::new(10).unwrap();
//! # const SUBJECT_SLOT_COUNT: usize = 7;
//! # let status = emcyphal::node::Status::default();
//! use static_cell::StaticCell;
//!
//! static CELL: StaticCell<MinimalNode<Mutex>> = StaticCell::new();
//! let node = CELL.init(MinimalNode::new(NODE_ID, status, SUBJECT_SLOT_COUNT));
//! let (hub, link, control, runner) = node.split();
//! ```

use crate::marker::{Message, Request, Response};
use crate::registry;

mod core_node;
mod minimal_node;

pub use crate::data_types::node::{Health, Mode};
pub use core_node::CoreNode;
pub use emcyphal_driver::link::Link;
pub use minimal_node::MinimalNode;
pub use minimal_node::Runner as MinimalNodeRunner;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Status {
    pub health: Health,
    pub mode: Mode,
    pub vendor_specific_code: u8,
}

impl Default for Status {
    fn default() -> Self {
        Self {
            health: Health::Nominal,
            mode: Mode::Operational,
            vendor_specific_code: 0,
        }
    }
}

pub(crate) trait DynamicControl {
    fn status(&self) -> Status;
    fn set_status(&self, status: Status);
}

/// Node status control handle
pub struct Control<'a>(&'a (dyn DynamicControl + Sync));

impl<'a> Control<'a> {
    unsafe fn new(node: &'a (dyn DynamicControl + Sync)) -> Self {
        Self(node)
    }

    pub fn status(&self) -> Status {
        self.0.status()
    }

    pub fn set_status(&mut self, status: Status) {
        self.0.set_status(status);
    }
}

#[allow(private_bounds)]
pub(crate) trait DynamicHub {
    fn rx_msg(&self) -> &(dyn registry::RxReg<Message> + Sync);
    fn rx_req(&self) -> &(dyn registry::RxReg<Request> + Sync);
    fn tx_msg(&self) -> &(dyn registry::TxReg<Message> + Sync);
    fn tx_resp(&self) -> &(dyn registry::TxReg<Response> + Sync);
}

/// Shared handle for creating new endpoints.
#[derive(Clone, Copy)]
pub struct Hub<'a>(&'a (dyn DynamicHub + Sync));

impl<'a> Hub<'a> {
    pub(crate) fn new(hub: &'a (dyn DynamicHub + Sync)) -> Self {
        Self(hub)
    }

    pub(crate) fn rx_msg(self) -> &'a (dyn registry::RxReg<Message> + Sync) {
        self.0.rx_msg()
    }

    pub(crate) fn rx_req(self) -> &'a (dyn registry::RxReg<Request> + Sync) {
        self.0.rx_req()
    }

    pub(crate) fn tx_msg(self) -> &'a (dyn registry::TxReg<Message> + Sync) {
        self.0.tx_msg()
    }

    pub(crate) fn tx_resp(self) -> &'a (dyn registry::TxReg<Response> + Sync) {
        self.0.tx_resp()
    }
}
