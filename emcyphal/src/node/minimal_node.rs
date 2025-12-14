use core::cell::Cell;
use core::sync::atomic::{AtomicBool, Ordering};
use core::task::{Context, Poll};
use embassy_sync::blocking_mutex::{Mutex, raw::RawMutex};
use embassy_time::Ticker;
use emcyphal_driver::internal::{DynamicLink, DynamicRx, DynamicRxFilter, DynamicTx};
use emcyphal_driver::link::{FilterUpdate, Link};

use crate::buffer;
use crate::core::{NodeId, Priority, PrioritySet};
use crate::data_types::node::Heartbeat;
use crate::frame::{DataSpecifier, Frame, Mtu};
use crate::marker::{Message, Request, Response};
use crate::node::{Control, DynamicControl, DynamicHub, Hub, Status};
use crate::registry::{self, RegistrationError};
use crate::socket::Publisher;
use crate::time::{Duration, Instant};

/// Minimal Cyphal-compliant node.
///
/// Publishes heartbeats in the background. The user can change the status through the `Control`
/// handle.
pub struct MinimalNode<M: RawMutex> {
    node: NodeState<M>,
    runner: RunnerState,
}

trait DynamicNode {
    fn status(&self) -> Status;
}

impl<M: RawMutex + Sync> MinimalNode<M> {
    pub fn new(address: NodeId, status: Status, subject_slot_count: usize) -> Self {
        Self {
            node: NodeState::new(address, status, subject_slot_count),
            runner: Default::default(),
        }
    }

    pub fn split(&mut self) -> (Hub<'_>, Link<'_>, Control<'_>, Runner<'_>) {
        let hub = Hub::new(&self.node);
        let link = Link::new(&self.node);
        let control = unsafe { Control::new(&self.node) };
        let runner = unwrap!(self.runner.make_runner(&self.node, hub));
        (hub, link, control, runner)
    }
}

pub struct NodeState<M: RawMutex> {
    address: NodeId,
    destination_added: AtomicBool,
    status: Mutex<M, Cell<Status>>,
    hubs: Hubs<M>,
}

struct Hubs<M: RawMutex> {
    rx_msg: registry::rx_msg::Registry<M>,
    rx_req: registry::rx_req::Registry<M>,
    tx_msg: registry::tx_msg::Registry<M>,
    tx_resp: registry::tx_resp::Registry<M>,
}

impl<M: RawMutex> NodeState<M> {
    pub fn new(address: NodeId, status: Status, subject_slot_count: usize) -> Self {
        Self {
            address,
            destination_added: Default::default(),
            status: Mutex::new(Cell::new(status)),
            hubs: Hubs {
                rx_msg: registry::rx_msg::Registry::new(subject_slot_count),
                rx_req: Default::default(),
                tx_msg: Default::default(),
                tx_resp: Default::default(),
            },
        }
    }
}

impl<M: RawMutex> DynamicNode for NodeState<M> {
    fn status(&self) -> Status {
        self.status.lock(|cell| cell.get())
    }
}

impl<M: RawMutex> DynamicRxFilter for NodeState<M> {
    fn poll_pop(&self, cx: &mut Context<'_>) -> Poll<FilterUpdate> {
        // Public interface forces sequential access. No need for compare_exchange
        if !self.destination_added.load(Ordering::Relaxed) {
            self.destination_added.store(true, Ordering::Relaxed);
            return Poll::Ready(FilterUpdate::AddDestination(self.address));
        }

        self.hubs.rx_msg.poll_pop(cx)
    }
}

impl<M: RawMutex> DynamicRx for NodeState<M> {
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

impl<M: RawMutex> DynamicTx for NodeState<M> {
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

impl<M: RawMutex> DynamicLink for NodeState<M> {}

impl<M: RawMutex> DynamicControl for NodeState<M> {
    fn status(&self) -> Status {
        self.status.lock(|cell| cell.get())
    }

    fn set_status(&self, status: Status) {
        self.status.lock(|cell| cell.set(status));
    }
}

impl<M: RawMutex + Sync> DynamicHub for NodeState<M> {
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

#[derive(Default)]
struct RunnerState {
    heartbeat_buf: buffer::tx_msg::Signal<Heartbeat>,
}

impl RunnerState {
    fn make_runner<'a>(
        &'a mut self,
        node: &'a (dyn DynamicNode + Sync),
        hub: Hub<'a>,
    ) -> Result<Runner<'a>, RegistrationError> {
        const TIMEOUT: Duration = Duration::from_secs(Heartbeat::MAX_PUBLICATION_PERIOD as u64);
        let heartbeat_pub = Publisher::create(
            hub,
            &mut self.heartbeat_buf,
            Heartbeat::SUBJECT,
            Priority::Nominal,
            TIMEOUT,
            false,
        )?;
        Ok(Runner {
            node,
            heartbeat_pub,
        })
    }
}

/// MinimalNode background task runner.
///
/// Run for proper node operation.
pub struct Runner<'a> {
    node: &'a (dyn DynamicNode + Sync),
    heartbeat_pub: Publisher<'a, Heartbeat>,
}

impl<'a> Runner<'a> {
    pub async fn run(&mut self) {
        const PERIOD: Duration = Duration::from_secs(Heartbeat::MAX_PUBLICATION_PERIOD as u64);
        let mut ticker = Ticker::every(PERIOD);

        loop {
            ticker.next().await;
            let uptime = u32::try_from(Instant::now().as_secs()).unwrap_or(u32::MAX);
            let status = self.node.status();
            let msg = Heartbeat {
                uptime,
                health: status.health,
                mode: status.mode,
                vendor_specific_status_code: status.vendor_specific_code,
            };
            unwrap!(self.heartbeat_pub.try_push(msg));
        }
    }
}
