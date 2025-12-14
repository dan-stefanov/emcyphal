use core::sync::atomic::{AtomicBool, Ordering};
use core::task::{Context, Poll};
use embassy_sync::blocking_mutex::raw::RawMutex;
use emcyphal_driver::internal::{DynamicLink, DynamicRx, DynamicRxFilter, DynamicTx};
use emcyphal_driver::link::{FilterUpdate, Link};

use crate::core::{NodeId, PrioritySet};
use crate::frame::{DataSpecifier, Frame, Mtu};
use crate::marker::{Message, Request, Response};
use crate::node::{DynamicHub, Hub};
use crate::registry;

/// Node without any background activity.
///
/// Only delivers transfers to and from endpoints.
/// Users should publish heartbeats to comply with the Cyphal protocol.
pub struct CoreNode<M: RawMutex> {
    address: NodeId,
    destination_added: AtomicBool,
    hubs: Hubs<M>,
}

struct Hubs<M: RawMutex> {
    rx_msg: registry::rx_msg::Registry<M>,
    rx_req: registry::rx_req::Registry<M>,
    tx_msg: registry::tx_msg::Registry<M>,
    tx_resp: registry::tx_resp::Registry<M>,
}

impl<M: RawMutex + Sync> CoreNode<M> {
    pub fn new(address: NodeId, subject_slot_count: usize) -> Self {
        Self {
            address,
            destination_added: Default::default(),
            hubs: Hubs {
                rx_msg: registry::rx_msg::Registry::new(subject_slot_count),
                rx_req: Default::default(),
                tx_msg: Default::default(),
                tx_resp: Default::default(),
            },
        }
    }

    pub fn split(&mut self) -> (Hub<'_>, Link<'_>) {
        let hub = Hub::new(self);
        let link = Link::new(self);
        (hub, link)
    }
}

impl<M: RawMutex> DynamicRxFilter for CoreNode<M> {
    fn poll_pop(&self, cx: &mut Context<'_>) -> Poll<FilterUpdate> {
        // Public interface forces sequential access. No need for compare_exchange
        if !self.destination_added.load(Ordering::Relaxed) {
            self.destination_added.store(true, Ordering::Relaxed);
            return Poll::Ready(FilterUpdate::AddDestination(self.address));
        }

        self.hubs.rx_msg.poll_pop(cx)
    }
}

impl<M: RawMutex> DynamicRx for CoreNode<M> {
    fn poll_push(&self, cx: &mut Context<'_>, frame: &Frame, mtu: Mtu) -> Poll<()> {
        match (frame.header.data_spec, frame.loop_back) {
            (DataSpecifier::Message(_), _) => self.hubs.rx_msg.poll_push(cx, frame, mtu),
            (DataSpecifier::Request(_), false)
                if frame.header.destination == Some(self.address) =>
            {
                self.hubs.rx_req.poll_push(cx, frame, mtu)
            }
            _ => Poll::Ready(()),
        }
    }
}

impl<M: RawMutex> DynamicTx for CoreNode<M> {
    fn poll_pop(&self, cx: &mut Context, priority_mask: PrioritySet, mtu: Mtu) -> Poll<Frame> {
        if let Poll::Ready(mut frame) = self.hubs.tx_msg.poll_pop(cx, priority_mask, mtu) {
            frame.header.source = Some(self.address);
            return Poll::Ready(frame);
        }

        if let Poll::Ready(mut frame) = self.hubs.tx_resp.poll_pop(cx, priority_mask, mtu) {
            frame.header.source = Some(self.address);
            return Poll::Ready(frame);
        }
        Poll::Pending
    }
}

impl<M: RawMutex> DynamicLink for CoreNode<M> {}

impl<M: RawMutex + Sync> DynamicHub for CoreNode<M> {
    fn rx_msg(&self) -> &(dyn registry::RxReg<Message> + Sync) {
        &self.hubs.rx_msg
    }
    fn rx_req(&self) -> &(dyn registry::RxReg<Request> + Sync) {
        &self.hubs.rx_req
    }
    fn tx_msg(&self) -> &(dyn registry::TxReg<Message> + Sync) {
        &self.hubs.tx_msg
    }
    fn tx_resp(&self) -> &(dyn registry::TxReg<Response> + Sync) {
        &self.hubs.tx_resp
    }
}
