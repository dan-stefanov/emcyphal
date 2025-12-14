use core::cell::RefCell;
use core::future::{pending, poll_fn};
use core::marker::PhantomData;
use core::task::{Poll, Waker};
use embassy_futures::select::{Either, Either4, select, select4};
use embassy_stm32::Peri;
use embassy_stm32::can::Instance as EmbassyInstance;
use embassy_stm32::can::{RxPin, TxPin};
use embassy_stm32::interrupt::InterruptExt;
use embassy_stm32::interrupt::typelevel::Binding;
use embassy_stm32::interrupt::typelevel::{Handler, Interrupt};
use embassy_stm32::{gpio, rcc};
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::{channel::Channel, waitqueue::WakerRegistration};
use embassy_time::{Instant, Timer};
use emcyphal_core::{NodeId, PrioritySet, SubjectId};
use emcyphal_driver::link::{self, FilterUpdate};
use emcyphal_driver::{frame, time};

use crate::config::FrameFormat;
use crate::utils::{MailboxPriorityMap, PriorityMap, TxMailboxIdx, TxMailboxSet};
use crate::{config, message_ram as ram, raw};

/// Maximum number of message subjects the driver can simultaneously subscribe to.
pub const SUBJECT_SLOT_COUNT: usize = raw::MESSAGE_FILTER_COUNT * 2;

pub trait SealedInstance: EmbassyInstance {
    fn atomic_methods() -> raw::AtomicMethods;
    unsafe fn make_registers<'a>() -> raw::Registers<'a>;
    fn info() -> &'static Info;
}

#[allow(private_bounds)]
pub trait Instance: SealedInstance {}

struct State {
    _pins: [gpio::Flex<'static>; 2],
    control: Option<raw::Control<'static>>,
    rx_trigger: WakerRegistration,
    tx_unpend_mask: TxMailboxSet,
    tx_trigger: WakerRegistration,
    ref_counter: u8,
}

pub struct Info {
    interrupts: [embassy_stm32::interrupt::Interrupt; 2],
    state: Mutex<CriticalSectionRawMutex, RefCell<Option<State>>>,
    loop_back: Channel<CriticalSectionRawMutex, frame::Frame, 1>,
}

impl Info {
    pub const fn new<I: EmbassyInstance>() -> Self {
        Self {
            interrupts: [I::IT0Interrupt::IRQ, I::IT1Interrupt::IRQ],
            state: Mutex::new(RefCell::new(None)),
            loop_back: Channel::new(),
        }
    }

    fn wake_rx(&self) {
        self.state.lock(|cell| {
            let mut slot = cell.borrow_mut();
            if let Some(state) = slot.as_mut() {
                state.rx_trigger.wake();
            }
        });
    }
    fn wake_tx(&self, pending_mailboxes: TxMailboxSet) {
        self.state.lock(|cell| {
            let mut slot = cell.borrow_mut();
            if let Some(state) = slot.as_mut()
                && (pending_mailboxes & state.tx_unpend_mask).is_empty()
            {
                state.tx_trigger.wake();
            }
        });
    }
}

struct InfoRef<'a>(&'a Info);

impl<'a> InfoRef<'a> {
    fn create(info: &'a Info, pins: [gpio::Flex<'a>; 2]) -> Option<Self> {
        info.state.lock(|cell| {
            let mut slot = cell.borrow_mut();
            if slot.is_none() {
                *slot = Some(State {
                    // Safety: InfoRef will not outlive the pin lifetime and will drop it in the end
                    _pins: pins.map(|pin| unsafe { core::mem::transmute(pin) }),
                    control: Default::default(),
                    rx_trigger: Default::default(),
                    tx_unpend_mask: TxMailboxSet::ALL,
                    tx_trigger: Default::default(),
                    ref_counter: 1,
                });
                Some(Self(info))
            } else {
                None
            }
        })
    }

    fn loop_back(&self) -> &Channel<CriticalSectionRawMutex, frame::Frame, 1> {
        &self.0.loop_back
    }

    fn register_rx_waker(&self, w: &Waker) {
        self.0.state.lock(|cell| {
            let mut slot = cell.borrow_mut();
            let state = unwrap!(slot.as_mut());
            state.rx_trigger.register(w);
        });
    }

    fn register_tx_waker(&self, w: &Waker, unpend_mask: TxMailboxSet) {
        self.0.state.lock(|cell| {
            let mut slot = cell.borrow_mut();
            let state = unwrap!(slot.as_mut());
            state.tx_unpend_mask = unpend_mask;
            state.tx_trigger.register(w);
        });
    }

    fn set_control(&self, control: raw::Control<'a>) {
        // Safety: InfoRef will not outlive the pin lifetime and will drop it in the end
        let control =
            unsafe { core::mem::transmute::<raw::Control<'_>, raw::Control<'_>>(control) };
        self.0.state.lock(|cell| {
            let mut slot = cell.borrow_mut();
            let state = unwrap!(slot.as_mut());
            state.control = Some(control);
        });
    }
}

impl<'a> Clone for InfoRef<'a> {
    fn clone(&self) -> Self {
        self.0.state.lock(|cell| {
            let mut slot = cell.borrow_mut();
            let state = unwrap!(slot.as_mut());
            state.ref_counter += 1;
        });
        Self(self.0)
    }
}

impl<'a> Drop for InfoRef<'a> {
    fn drop(&mut self) {
        self.0.state.lock(|cell| {
            let mut slot = cell.borrow_mut();
            let state = unwrap!(slot.as_mut());
            state.ref_counter -= 1;
            if state.ref_counter == 0 {
                if let Some(control) = state.control.as_mut() {
                    control.stop();
                }
                *slot = None;
                for interrupt in self.0.interrupts {
                    interrupt.disable();
                }
            }

            // TODO: call rcc::disable
        });
    }
}

/// FDCAN Interrupt line 0 handler.
///
/// Bind to FDCANx_IT0 for proper operation.
pub struct IT0InterruptHandler<T: Instance> {
    _phantom: PhantomData<T>,
}

impl<T: Instance> Handler<T::IT0Interrupt> for IT0InterruptHandler<T> {
    unsafe fn on_interrupt() {
        T::atomic_methods().clear_rx_interrupts();
        T::info().wake_rx();
    }
}

/// FDCAN Interrupt line 1 handler.
///
/// Bind to FDCANx_IT1 for proper operation.
pub struct IT1InterruptHandler<T: Instance> {
    _phantom: PhantomData<T>,
}

impl<T: Instance> Handler<T::IT1Interrupt> for IT1InterruptHandler<T> {
    unsafe fn on_interrupt() {
        T::atomic_methods().clear_tx_interrupts();
        let pending_mailboxes = T::atomic_methods().tx_pending();
        T::info().wake_tx(pending_mailboxes);
    }
}

pub struct Driver<'a> {
    regs: Option<raw::Registers<'a>>,
    info: InfoRef<'a>,
    frame_format: config::FrameFormat,
    timestamp_source: config::TimestampSource,
}

impl<'a> Driver<'a> {
    /// Creates the driver and initializes the periphery.
    ///
    /// Initializes pins and FDCAN. Keeps FDCAN in INIT mode.
    pub fn new<T: Instance>(
        _instance: Peri<'a, T>,
        rx: Peri<'a, impl RxPin<T>>,
        tx: Peri<'a, impl TxPin<T>>,
        _irqs: impl Binding<T::IT0Interrupt, IT0InterruptHandler<T>>
        + Binding<T::IT1Interrupt, IT1InterruptHandler<T>>
        + 'a,
        config: config::Config,
    ) -> Self {
        let rx_af_num = rx.af_num();
        let mut rx_pin = gpio::Flex::new(rx);
        rx_pin.set_as_af_unchecked(rx_af_num, gpio::AfType::input(gpio::Pull::None));

        let tx_af_num = tx.af_num();
        let mut tx_pin = gpio::Flex::new(tx);
        tx_pin.set_as_af_unchecked(
            tx_af_num,
            gpio::AfType::output(gpio::OutputType::PushPull, gpio::Speed::VeryHigh),
        );

        let info =
            InfoRef::create(T::info(), [rx_pin, tx_pin]).expect("Peripheral state is occupied");

        rcc::enable_and_reset::<T>();

        // Safety: A unique access is guaranteed by _instance
        let mut regs = unsafe { T::make_registers() };
        regs.configure(&config);

        unsafe {
            T::IT0Interrupt::unpend(); // Not unsafe
            T::IT0Interrupt::enable();

            T::IT1Interrupt::unpend(); // Not unsafe
            T::IT1Interrupt::enable();
        }

        Self {
            regs: Some(regs),
            info,
            frame_format: config.frame_format,
            timestamp_source: config.timestamp_source,
        }
    }

    /// Binds to the link and start FDCAN operation.
    ///
    /// Run the produced tasks for proper driver operation.
    pub fn start(
        mut self,
        access: link::Link<'a>,
    ) -> (RxFilterRunner<'a>, RxRunner<'a>, TxRunner<'a>) {
        let regs = unwrap!(self.regs.take());
        let (mut control, filters, rx_fifo, tx_queue, tx_event_fifo) = regs.split();
        let (channel_rx_filter, channel_rx, channel_tx) = access.split();

        control.start();

        // Approximate timestamp counter value at Instant::from_ticks(0)
        let timestamp_offset = match self.timestamp_source {
            config::TimestampSource::System => None,
            config::TimestampSource::Internal(_) => {
                let (instant, counter) = critical_section::with(|_| {
                    // sample instant first to avoid underestimation
                    (time::Instant::now(), control.internal_timestamp_counter())
                });
                Some(counter.wrapping_sub(instant.as_ticks() as u16))
            }
            config::TimestampSource::ExternalTIM3 => {
                let (instant, counter) = critical_section::with(|_| {
                    // sample instant first to avoid underestimation
                    (time::Instant::now(), control.external_timestamp_counter())
                });
                Some(counter.wrapping_sub(instant.as_ticks() as u16))
            }
        };

        self.info.set_control(control);

        let rx_filter_runner = RxFilterRunner {
            link: channel_rx_filter,
            filters,
            _info: self.info.clone(),
        };
        let rx_runner = RxRunner {
            link: channel_rx,
            rx_fifo,
            info: self.info.clone(),
            frame_format: self.frame_format,
            timestamp_offset,
        };
        let tx_runner = TxRunner {
            link: channel_tx,
            tx_queue,
            tx_event_fifo,
            info: self.info.clone(),
            frame_format: self.frame_format,
            timestamp_offset,
        };

        (rx_filter_runner, rx_runner, tx_runner)
    }
}

/// RX filter configuration task.
///
/// Run this task for proper driver operation.
pub struct RxFilterRunner<'a> {
    link: link::RxFilter<'a>,
    filters: raw::Filters<'a>,
    _info: InfoRef<'a>,
}

type MsgFilter = [Option<SubjectId>; 2];

impl<'a> RxFilterRunner<'a> {
    pub async fn run(&mut self) -> ! {
        let mut msg_filters = self.load_message_filters();
        let mut srv_filter = self.load_service_filter();

        loop {
            let request = self.link.pop().await;
            match request {
                FilterUpdate::AddSubject(subject) => self.add_subject(&mut msg_filters, subject),
                FilterUpdate::RemoveSubjectRange(range) => {
                    self.remove_subject_range(&mut msg_filters, range)
                }

                FilterUpdate::AddDestination(node) => self.add_destination(&mut srv_filter, node),
                FilterUpdate::RemoveDestinationRange(range) => {
                    self.remove_destination_range(&mut srv_filter, range)
                }
            }
        }
    }

    fn load_message_filters(&mut self) -> [MsgFilter; raw::MESSAGE_FILTER_COUNT] {
        let mut filters: [MsgFilter; raw::MESSAGE_FILTER_COUNT] = Default::default();
        for (i, filter) in filters.iter_mut().enumerate() {
            *filter = self.filters.get_message_filter(i);
        }
        filters
    }

    fn add_subject(&mut self, filters: &mut [MsgFilter], subject: SubjectId) {
        let slots = filters.as_flattened_mut();
        let slot_idx = slots
            .iter()
            .position(Option::is_none)
            .expect("No slots left");
        slots[slot_idx] = Some(subject);
        let filter_idx = slot_idx / 2;
        self.filters
            .set_message_filter(filter_idx, filters[filter_idx]);
    }

    fn remove_subject_range(&mut self, filters: &mut [MsgFilter], range: [SubjectId; 2]) {
        for (i, filter) in filters.iter_mut().enumerate() {
            let mut update_filter = false;
            for slot in filter
                .iter_mut()
                .filter(|slot| slot.is_some_and(|val| range[0] <= val && val <= range[1]))
            {
                *slot = None;
                update_filter = true;
            }
            if update_filter {
                self.filters.set_message_filter(i, *filter);
            }
        }
    }

    fn load_service_filter(&mut self) -> Option<NodeId> {
        self.filters.get_service_filter()
    }

    fn add_destination(&mut self, filter: &mut Option<NodeId>, node: NodeId) {
        assert!(filter.is_none(), "No slot left");
        *filter = Some(node);
        self.filters.set_service_filter(*filter);
    }

    fn remove_destination_range(&mut self, filter: &mut Option<NodeId>, range: [NodeId; 2]) {
        if filter.is_some_and(|val| range[0] <= val && val <= range[1]) {
            *filter = None;
            self.filters.set_service_filter(*filter);
        }
    }
}

/// Frame receiving task.
///
/// Run this task for proper driver operation.
pub struct RxRunner<'a> {
    link: link::Rx<'a>,
    rx_fifo: raw::RxFifo<'a>,
    info: InfoRef<'a>,
    frame_format: FrameFormat,
    timestamp_offset: Option<u16>,
}

impl<'a> RxRunner<'a> {
    pub async fn run(&mut self) -> ! {
        loop {
            let frame = match select(
                // Check loop-back first
                self.info.loop_back().receive(),
                Self::pop_rx(&mut self.rx_fifo, &self.info),
            )
            .await
            {
                Either::First(frame) => frame,
                Either::Second(raw_frame) => {
                    let now = time::Instant::now();
                    let timestamp = if let Some(offset) = self.timestamp_offset {
                        make_timestamp(now, raw_frame.timestamp.wrapping_sub(offset))
                    } else {
                        now
                    };

                    frame::Frame {
                        header: raw_frame.header,
                        data: raw_frame.data,
                        timestamp,
                        loop_back: false,
                    }
                }
            };
            self.link.push(frame, self.frame_format.mtu()).await;
        }
    }

    async fn pop_rx(rx_fifo: &mut raw::RxFifo<'a>, info: &InfoRef<'a>) -> raw::RawFrame {
        poll_fn(|cx| {
            info.register_rx_waker(cx.waker());

            // Check after the waker is set
            for i in 0..ram::RX_FIFOS_MAX {
                if let Some(frame) = rx_fifo.pop(i.into()) {
                    return Poll::Ready(frame);
                }
            }
            Poll::Pending
        })
        .await
    }
}

/// Frame transmitting task.
///
/// Run this task for proper driver operation.
pub struct TxRunner<'a> {
    link: link::Tx<'a>,
    tx_queue: raw::TxQueue<'a>,
    tx_event_fifo: raw::TxEventFifo<'a>,
    info: InfoRef<'a>,
    frame_format: config::FrameFormat,
    timestamp_offset: Option<u16>,
}

impl<'a> TxRunner<'a> {
    pub async fn run(&mut self) -> ! {
        let mut tx_queue = TxQueue {
            raw_queue: &mut self.tx_queue,
            info: &self.info,
        };
        let mut frames: PriorityMap<frame::Frame> = Default::default();
        let mut pending_frames: MailboxPriorityMap = Default::default();
        let mut loop_back_pending: PrioritySet = Default::default();

        loop {
            // The query order is important. A transmission may complete after the fetch
            let pending_mailboxes = tx_queue.pending();
            let mut completed_mailboxes = TxMailboxSet::NONE;
            while let Some(event) = self.tx_event_fifo.pop() {
                let idx = unwrap!(TxMailboxIdx::new(event.marker));
                completed_mailboxes.insert(idx);
                let priority = unwrap!(pending_frames.remove_by_mailbox(idx));
                if frames[priority].loop_back {
                    let now = Instant::now();
                    let timestamp = if let Some(offset) = self.timestamp_offset {
                        make_timestamp(now, event.timestamp.wrapping_sub(offset))
                    } else {
                        now
                    };
                    frames[priority].timestamp = timestamp;
                    loop_back_pending.insert(priority);
                } else {
                    frames.remove(priority);
                }
            }
            let free_mailboxes = !pending_mailboxes | completed_mailboxes;
            for idx in free_mailboxes & !completed_mailboxes {
                pending_frames.remove_by_mailbox(idx);
            }

            let occupied_priorities = frames.keys();
            let queued_priorities = occupied_priorities & !loop_back_pending;

            // Order is important: remove old frame, fetch new, enqueue highest priority
            match select4(
                async {
                    if let Some(deadline) = queued_priorities
                        .into_iter()
                        .map(|p| frames[p].timestamp)
                        .min()
                    {
                        // The timer yields at lest once. Do not call it, if deadline has expired
                        if deadline > time::Instant::now() {
                            Timer::at(deadline).await
                        }
                    } else {
                        pending().await
                    }
                },
                async {
                    if let Some(priority) = loop_back_pending.first() {
                        self.info.loop_back().send(frames[priority].clone()).await;
                        priority
                    } else {
                        pending().await
                    }
                },
                self.link.pop(!occupied_priorities, self.frame_format.mtu()),
                async {
                    if let Some(priority) = queued_priorities.first() {
                        if !pending_frames.contains_priority(priority) {
                            let idx = free_mailboxes
                                .first()
                                .expect("One mailbox must always be free");
                            unwrap!(tx_queue.add(
                                idx,
                                &frames[priority],
                                self.frame_format,
                                idx.into()
                            ));
                            tx_queue.cancel(!TxMailboxSet::new_eq(idx));
                            assert_ne!(
                                tx_queue.pending(),
                                TxMailboxSet::ALL,
                                "At least one mailbox should get canceled immediately"
                            );
                            unwrap!(pending_frames.insert(idx, priority));
                        }
                        let idx = unwrap!(pending_frames.get_by_priority(priority));
                        tx_queue.wait_for_unpend(TxMailboxSet::new_eq(idx)).await
                    } else {
                        pending().await
                    }
                },
            )
            .await
            {
                Either4::First(()) => {
                    let now = time::Instant::now();
                    for priority in queued_priorities {
                        if frames[priority].timestamp <= now {
                            unwrap!(frames.remove(priority));
                            if let Some(idx) = pending_frames.remove_by_priority(priority) {
                                tx_queue.cancel(TxMailboxSet::new_eq(idx));
                            }
                        }
                    }
                }
                Either4::Second(priority) => {
                    loop_back_pending.remove(priority);
                    unwrap!(frames.remove(priority));
                }
                Either4::Third(frame) => {
                    // anonymous frame transmission is not supported
                    if frame.header.source.is_some() {
                        let priority = frame.header.priority;
                        unwrap!(frames.insert(priority, frame));
                    }
                }
                Either4::Fourth(()) => {}
            }
        }
    }
}

struct TxQueue<'a, 'b> {
    raw_queue: &'a mut raw::TxQueue<'b>,
    info: &'a InfoRef<'b>,
}

impl<'a, 'b> TxQueue<'a, 'b> {
    pub fn pending(&self) -> TxMailboxSet {
        self.raw_queue.pending()
    }

    pub fn add(
        &mut self,
        index: TxMailboxIdx,
        frame: &frame::Frame,
        frame_format: config::FrameFormat,
        marker: u8,
    ) -> Result<(), ()> {
        self.raw_queue.add(index, frame, frame_format, marker)
    }

    pub fn cancel(&mut self, mailboxes: TxMailboxSet) {
        self.raw_queue.cancel(mailboxes);
    }

    pub async fn wait_for_unpend(&mut self, mailboxes: TxMailboxSet) {
        poll_fn(|cx| {
            self.info.register_tx_waker(cx.waker(), mailboxes);
            self.raw_queue.enable_interrupt(mailboxes);

            // Check after the waker is set
            if (self.pending() & mailboxes).is_empty() {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;
    }
}

impl<'a, 'b> Drop for TxQueue<'a, 'b> {
    fn drop(&mut self) {
        self.raw_queue.cancel(TxMailboxSet::ALL);
    }
}

/// Make timestamp from counter and epoch instant
/// epoch should should lie in [counter_instant, counter_instant + u16::MAX]
fn make_timestamp(epoch: time::Instant, counter: u16) -> Instant {
    let offset = (epoch.as_ticks() as u16).wrapping_sub(counter);
    Instant::from_ticks(epoch.as_ticks().saturating_sub(offset.into()))
}
