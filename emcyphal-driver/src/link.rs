//! Channels connecting driver and Emcyphal stack

use core::future::poll_fn;
use emcyphal_core::{NodeId, PrioritySet, SubjectId};

use crate::frame::{Frame, Mtu};
use crate::internal;

/// Receiver filter update request
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterUpdate {
    AddSubject(SubjectId),
    RemoveSubjectRange([SubjectId; 2]),
    AddDestination(NodeId),
    RemoveDestinationRange([NodeId; 2]),
}

/// Producer of receiver filter update requests
///
/// Range removal requests may cover the entire ID range. The driver should efficiently process
/// range requests with complexity bounded by the number of removed or remaining entries, not the
/// range size.
pub struct RxFilter<'a>(&'a (dyn internal::DynamicRxFilter + Sync));

impl<'a> RxFilter<'a> {
    /// Asynchronously fetches the next filter update request. Safe to drop.
    pub async fn pop(&mut self) -> FilterUpdate {
        poll_fn(|cx| self.0.poll_pop(cx)).await
    }
}

/// Consumer of received frames
///
/// The frame destination address should be consistent with the data specifier:
/// * `None` for messages
/// * `Some` for services
///
/// Frame timestamp encodes the instant it appeared on the bus:
/// * reception time for regular frames
/// * transmission time for loop-back frames
///
/// MTU must be constant for the link lifetime. A driver in FD mode should set FD MTU for
/// Classic frames as well.
///
/// Drivers may push unsolicited frames, though such traffic affects performance.
///
/// The channel may block only for short periods.
pub struct Rx<'a>(&'a (dyn internal::DynamicRx + Sync));

impl<'a> Rx<'a> {
    /// Asynchronously pushes a frame. Safe to drop.
    pub async fn push(&mut self, frame: Frame, mtu: Mtu) {
        poll_fn(|cx| self.0.poll_push(cx, &frame, mtu)).await;
    }
}

/// Producer of frames for transmission
///
/// MTU must be constant for the link lifetime.
///
/// The frame destination address should be consistent with the data specifier:
/// * `None` for messages
/// * `Some` for services
///
/// Frame timestamp represents a transmission deadline. Driver should drop frames
/// that were not transmitted in time.
pub struct Tx<'a>(&'a (dyn internal::DynamicTx + Sync));

impl<'a> Tx<'a> {
    /// Asynchronously fetches the next frame. Safe to drop.
    ///
    /// Blocks until a frame with a priority matching the priority mask is available.
    /// Returns the frame with the highest matching priority.
    pub async fn pop(&mut self, priority_mask: PrioritySet, mtu: Mtu) -> Frame {
        poll_fn(|cx| self.0.poll_pop(cx, priority_mask, mtu)).await
    }
}

/// Channel container. A driver should consume it.
pub struct Link<'a>(&'a (dyn internal::DynamicLink + Sync));

impl<'a> Link<'a> {
    pub fn new(access: &'a (dyn internal::DynamicLink + Sync)) -> Self {
        Self(access)
    }

    pub fn split(self) -> (RxFilter<'a>, Rx<'a>, Tx<'a>) {
        (RxFilter(self.0), Rx(self.0), Tx(self.0))
    }
}
