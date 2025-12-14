/// Private interfaces for emcyphal Node
///
/// Drivers should not use this module.
/// Backward-incompatible changes can be made without major version bump.
use core::task::{Context, Poll};
use emcyphal_core::PrioritySet;

use crate::frame::{Frame, Mtu};
use crate::link::FilterUpdate;

pub trait DynamicRxFilter {
    fn poll_pop(&self, cx: &mut Context<'_>) -> Poll<FilterUpdate>;
}

pub trait DynamicRx {
    fn poll_push(&self, cx: &mut Context<'_>, frame: &Frame, mtu: Mtu) -> Poll<()>;
}

pub trait DynamicTx {
    fn poll_pop(&self, cx: &mut Context<'_>, priority_mask: PrioritySet, mtu: Mtu) -> Poll<Frame>;
}

pub trait DynamicLink: DynamicRxFilter + DynamicRx + DynamicTx {}
