use core::future::pending;
use embassy_futures::select::{Either3, select3};
use embassy_stm32::can;
use embassy_time::Timer;
use embedded_can::{ExtendedId, Id};
use emcyphal_core::{NodeId, Priority, PrioritySet, SubjectId};
use emcyphal_driver::frame;
use emcyphal_driver::link::{self, FilterUpdate};
use emcyphal_driver::time::Instant;

use crate::config::{Config, FrameFormat};
use crate::format;
use crate::utils::{PriorityArray, PriorityDeque};

const TX_FIFO_DEPTH: usize = 3;
// write_fd will block on the next frame
const PREEMPTION_DEPTH: usize = TX_FIFO_DEPTH + 1;

const MESSAGE_FILTER_COUNT: u8 = can::filter::EXTENDED_FILTER_MAX - 1;
const MESSAGE_FILTER_OFFSET: u8 = 0;
const SERVICE_FILTER_OFFSET: u8 = MESSAGE_FILTER_OFFSET + MESSAGE_FILTER_COUNT;

/// Maximum number of message subjects the driver can simultaneously subscribe to.
pub const SUBJECT_SLOT_COUNT: usize = MESSAGE_FILTER_COUNT as usize;

/// Connect the CAN driver to the link.
///
/// Run the produced runners for proper operation.
///
/// For optimal operation the global filters should reject remote and non-matching frames.
pub fn bind<'a>(
    can: can::Can<'a>,
    link: link::Link<'a>,
    config: Config,
) -> (RxFilterRunner<'a>, RxRunner<'a>, TxRunner<'a>) {
    let (can_tx, can_rx, can_properties) = can.split();
    let (link_rx_filter, link_rx, link_tx) = link.split();
    let rx_filter_runner = RxFilterRunner {
        can: can_properties,
        link: link_rx_filter,
        subjects: Default::default(),
        destination: Default::default(),
    };
    let rx_runner = RxRunner {
        can: can_rx,
        link: link_rx,
        frame_format: config.frame_format,
    };
    let tx_runner = TxRunner {
        can: can_tx,
        link: link_tx,
        frame_format: config.frame_format,
    };
    (rx_filter_runner, rx_runner, tx_runner)
}

/// RX filter configuration runner.
///
/// Run for proper driver operation.
pub struct RxFilterRunner<'a> {
    can: can::Properties,
    link: link::RxFilter<'a>,
    subjects: [Option<SubjectId>; SUBJECT_SLOT_COUNT],
    destination: Option<NodeId>,
}

#[derive(Debug)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
struct NoFreeSlots();

impl<'a> RxFilterRunner<'a> {
    pub async fn run(&mut self) -> ! {
        loop {
            let filter_update = self.link.pop().await;
            match filter_update {
                FilterUpdate::AddSubject(subject) => unwrap!(self.add_subject(subject)),
                FilterUpdate::RemoveSubjectRange(range) => self.remove_subject_range(range),
                FilterUpdate::AddDestination(node) => unwrap!(self.add_destination(node)),
                FilterUpdate::RemoveDestinationRange(range) => self.remove_destination_range(range),
            }
        }
    }

    fn add_subject(&mut self, subject: SubjectId) -> Result<(), NoFreeSlots> {
        let idx: usize = self
            .subjects
            .iter()
            .position(|slot| slot.is_none())
            .ok_or(NoFreeSlots())?;
        self.subjects[idx] = Some(subject);
        self.set_subject_filter(unwrap!(idx.try_into()), Some(subject));
        Ok(())
    }

    fn remove_subject_range(&mut self, range: [SubjectId; 2]) {
        for i in 0..SUBJECT_SLOT_COUNT {
            let value = &mut self.subjects[i];
            if Some(range[0]) <= *value && *value <= Some(range[1]) {
                *value = None;
                self.set_subject_filter(unwrap!(i.try_into()), None);
            }
        }
    }

    fn add_destination(&mut self, node: NodeId) -> Result<(), NoFreeSlots> {
        if self.destination.is_some() {
            return Err(NoFreeSlots());
        }
        self.destination = Some(node);
        self.set_service_filter(self.destination);
        Ok(())
    }

    fn remove_destination_range(&mut self, range: [NodeId; 2]) {
        if self
            .destination
            .is_some_and(|val| range[0] <= val && val <= range[1])
        {
            self.destination = None;
            self.set_service_filter(self.destination);
        }
    }

    fn set_subject_filter(&mut self, slot: u8, value: Option<SubjectId>) {
        let filter = can::filter::ExtendedFilter {
            filter: can::filter::FilterType::<ExtendedId, u32>::BitMask {
                filter: format::make_msg_filter_pattern(value),
                mask: format::MSG_FILTER_MASK,
            },
            action: can::filter::Action::StoreInFifo0,
        };

        self.can
            .set_extended_filter((MESSAGE_FILTER_OFFSET + slot).into(), filter);
    }

    fn set_service_filter(&mut self, value: Option<NodeId>) {
        let filter = can::filter::ExtendedFilter {
            filter: can::filter::FilterType::<ExtendedId, u32>::BitMask {
                filter: format::make_srv_filter_pattern(value),
                mask: format::SRV_FILTER_MASK,
            },
            action: can::filter::Action::StoreInFifo1,
        };

        self.can
            .set_extended_filter((SERVICE_FILTER_OFFSET).into(), filter);
    }
}

/// Frame receiving runner.
///
/// Run for proper driver operation.
pub struct RxRunner<'a> {
    can: can::CanRx<'a>,
    link: link::Rx<'a>,
    frame_format: FrameFormat,
}

impl<'a> RxRunner<'a> {
    pub async fn run(&mut self) -> ! {
        loop {
            if let Some(frame) = self.fetch_frame().await {
                self.link.push(frame, self.frame_format.mtu()).await;
            }
        }
    }

    async fn fetch_frame(&mut self) -> Option<frame::Frame> {
        let raw_envelope = self.can.read_fd().await.ok()?;
        if raw_envelope.frame.header().fdcan() && !self.frame_format.fd() {
            return None;
        }
        convert_raw_envelope(&raw_envelope)
    }
}

/// Frame transmitting runner.
///
/// Run for proper driver operation.
pub struct TxRunner<'a> {
    can: can::CanTx<'a>,
    link: link::Tx<'a>,
    frame_format: FrameFormat,
}

impl<'a> TxRunner<'a> {
    pub async fn run(&mut self) -> ! {
        let mut frames: PriorityDeque<can::frame::FdFrame, PREEMPTION_DEPTH> = Default::default();
        let mut deadlines = PriorityArray::<Instant>::repeat(Instant::MAX);
        loop {
            let priorities = frames.priorities();
            let next_deadline = *unwrap!(deadlines.iter().min());

            match select3(
                async {
                    // Timer always yield on first poll. Skip it explicitly if deadline has expired
                    if next_deadline > Instant::now() {
                        // embassy-time v0.5.0 bug. Timer does not serve the consequent calls once
                        // called with Instant::MAX
                        if next_deadline < Instant::MAX {
                            Timer::at(next_deadline).await
                        } else {
                            pending().await
                        }
                    }
                },
                self.link.pop(!priorities, self.frame_format.mtu()),
                async {
                    if let Some(priority) = priorities.first() {
                        let frame = unwrap!(frames.front(priority));
                        (priority, self.can.write_fd(frame).await)
                    } else {
                        pending().await
                    }
                },
            )
            .await
            {
                Either3::First(()) => {
                    let now = Instant::now();
                    for p in PrioritySet::ALL.into_iter() {
                        if now > deadlines[p] {
                            deadlines[p] = Instant::MAX;
                            unwrap!(frames.pop_back(p));
                        }
                    }
                }
                Either3::Second(frame) => {
                    // anonymous frame transmission is not supported
                    if frame.header.source.is_some() {
                        let priority = frame.header.priority;
                        let raw_frame = unwrap!(make_raw_frame(&frame, self.frame_format,));
                        unwrap!(frames.push_back(priority, raw_frame));
                        deadlines[priority] = frame.timestamp;
                    }
                }
                Either3::Third((delivered_priority, preempted_frame)) => {
                    unwrap!(frames.pop_front(delivered_priority));
                    if !frames.priorities().contains(delivered_priority) {
                        deadlines[delivered_priority] = Instant::MAX;
                    }

                    if let Some(frame) = preempted_frame {
                        unwrap!(frames.push_front(unwrap!(raw_frame_priority(&frame)), frame));
                    }
                }
            }
        }
    }
}

fn convert_raw_header(can_id: u32) -> Option<frame::Header> {
    if let Some(msg_id) = format::MessageCanId::from_can_id(can_id) {
        return Some(frame::Header {
            priority: msg_id.priority(),
            data_spec: frame::DataSpecifier::Message(msg_id.subject()),
            source: (!msg_id.anonymous()).then_some(msg_id.source()),
            destination: None,
        });
    }

    if let Some(srv_id) = format::ServiceCanId::from_can_id(can_id) {
        return Some(frame::Header {
            priority: srv_id.priority(),
            data_spec: if srv_id.request() {
                frame::DataSpecifier::Request(srv_id.service())
            } else {
                frame::DataSpecifier::Response(srv_id.service())
            },
            source: Some(srv_id.source()),
            destination: Some(srv_id.destination()),
        });
    }

    None
}

fn convert_raw_envelope(envelope: &can::frame::FdEnvelope) -> Option<frame::Frame> {
    let extended_id = match envelope.frame.id() {
        Id::Extended(id) => Some(*id),
        _ => None,
    }?;

    let header = convert_raw_header(extended_id.as_raw())?;
    let length = usize::from(envelope.frame.header().len());
    Some(frame::Frame {
        header,
        data: unwrap!(frame::Data::new(&envelope.frame.data()[..length])),
        timestamp: envelope.ts,
        loop_back: false,
    })
}

fn raw_frame_priority(frame: &can::frame::FdFrame) -> Option<Priority> {
    let extended_id = match frame.header().id() {
        Id::Extended(id) => Some(*id),
        _ => None,
    }?;

    Some(format::decode_can_id_priority(extended_id.as_raw()))
}

#[derive(Debug)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
struct InvalidFrameHeader();

fn make_can_id(header: &frame::Header) -> Result<u32, InvalidFrameHeader> {
    // anonymous frame transmission is not supported
    match (header.data_spec, header.destination, header.source) {
        (frame::DataSpecifier::Message(subject), None, Some(src)) => {
            let mut msg_id = format::MessageCanId::new();
            msg_id.set_priority(header.priority);
            msg_id.set_anonymous(false);
            msg_id.set_subject(subject);
            msg_id.set_source(src);
            Ok(msg_id.into())
        }
        (frame::DataSpecifier::Request(service), Some(dst), Some(src)) => {
            let mut srv_id = format::ServiceCanId::new();
            srv_id.set_priority(header.priority);
            srv_id.set_request(true);
            srv_id.set_service(service);
            srv_id.set_destination(dst);
            srv_id.set_source(src);
            Ok(srv_id.into())
        }
        (frame::DataSpecifier::Response(service), Some(dst), Some(src)) => {
            let mut srv_id = format::ServiceCanId::new();
            srv_id.set_priority(header.priority);
            srv_id.set_request(false);
            srv_id.set_service(service);
            srv_id.set_destination(dst);
            srv_id.set_source(src);
            Ok(srv_id.into())
        }
        _ => Err(InvalidFrameHeader()),
    }
}

fn make_raw_frame(
    frame: &frame::Frame,
    format: FrameFormat,
) -> Result<can::frame::FdFrame, InvalidFrameHeader> {
    let can_id = make_can_id(&frame.header)?;

    let header = if format.fd() {
        can::frame::Header::new_fd(
            unwrap!(ExtendedId::new(can_id)).into(),
            unwrap!(frame.data.len().try_into()),
            false,
            format.bit_rate_switch(),
        )
    } else {
        can::frame::Header::new(
            unwrap!(ExtendedId::new(can_id)).into(),
            unwrap!(frame.data.len().try_into()),
            false,
        )
    };
    Ok(unwrap!(can::frame::FdFrame::new(header, &frame.data)))
}
