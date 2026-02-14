//! Publisher/subscriber test with multi-priority executer configuration.

#![no_std]
#![no_main]

use defmt::*;
use embassy_executor::{InterruptExecutor, Spawner};
use embassy_stm32::peripherals;
use embassy_stm32::{bind_interrupts, interrupt, interrupt::InterruptExt};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_time::{Duration, Ticker};
use emcyphal::buffer;
use emcyphal::core::{NodeId, Priority, SubjectId};
use emcyphal::data_types::ByteArray;
use emcyphal::node::{Hub, MinimalNode, MinimalNodeRunner};
use emcyphal::socket;
use emcyphal_stm32_native::{self as can, config};
use nucleo_g431rb::board;
use static_cell::StaticCell;
use {defmt_rtt as _, panic_probe as _};

bind_interrupts!(struct Irqs {
    FDCAN1_IT0 => can::IT0InterruptHandler<peripherals::FDCAN1>;
    FDCAN1_IT1 => can::IT1InterruptHandler<peripherals::FDCAN1>;
});

static EXECUTOR_HIGH: InterruptExecutor = InterruptExecutor::new();
static EXECUTOR_MED: InterruptExecutor = InterruptExecutor::new();

#[interrupt]
unsafe fn USART3() {
    unsafe { EXECUTOR_HIGH.on_interrupt() }
}

#[interrupt]
unsafe fn UART4() {
    unsafe { EXECUTOR_MED.on_interrupt() }
}

const NODE_ID: NodeId = NodeId::new(10).unwrap();
const TEST_SUBJECT: SubjectId = SubjectId::new(10).unwrap();

#[embassy_executor::main]
async fn main(spawner_low: Spawner) {
    let p = embassy_stm32::init(board::make_peripheral_config());

    // High-priority executor
    interrupt::USART3.set_priority(interrupt::Priority::P6);
    let spawner_high = EXECUTOR_HIGH.start(interrupt::USART3);

    // Med-priority executor
    interrupt::UART4.set_priority(interrupt::Priority::P7);
    let spawner_med = EXECUTOR_MED.start(interrupt::UART4);

    let mut can_config = board::make_can_config_native();
    can_config.test_mode = Some(config::TestMode::ExternalLoopBack);
    let can_driver = can::Driver::new(p.FDCAN1, p.PA11, p.PA12, Irqs, can_config);

    let (hub, link) = {
        static CELL: StaticCell<MinimalNode<CriticalSectionRawMutex>> = StaticCell::new();
        let status = Default::default();
        let node = CELL.init(MinimalNode::new(NODE_ID, status, can::SUBJECT_SLOT_COUNT));
        let (hub, link, _, runner) = node.split();
        unwrap!(spawner_low.spawn(node_runner(runner)));
        (hub, link)
    };

    let (rx_filter, rx, tx) = can_driver.start(link);
    unwrap!(spawner_high.spawn(driver_rx_filter_runner(rx_filter)));
    unwrap!(spawner_high.spawn(driver_rx_runner(rx)));
    unwrap!(spawner_high.spawn(driver_tx_runner(tx)));

    unwrap!(spawner_med.spawn(sender(hub)));
    unwrap!(spawner_med.spawn(receiver(hub)));

    // Keep IO initialized
    let () = core::future::pending().await;
    defmt::unreachable!();
}

#[embassy_executor::task]
async fn sender(hub: Hub<'static>) -> ! {
    let mut tx_buffer = buffer::tx_msg::Blocking::<ByteArray>::new();

    let mut publisher = unwrap!(socket::Publisher::create(
        hub,
        &mut tx_buffer,
        TEST_SUBJECT,
        Priority::Nominal,
        Duration::from_secs(2),
        true,
    ));

    let mut ticker = Ticker::every(Duration::from_secs(2));
    let mut seq = 0u8;
    loop {
        ticker.next().await;
        let msg = ByteArray {
            bytes: unwrap!(heapless::Vec::from_slice(&[seq])),
        };
        seq = seq.wrapping_add(1);

        info!("Send a message: {}", &msg.bytes);
        unwrap!(publisher.try_push(msg));
    }
}

#[embassy_executor::task]
async fn receiver(hub: Hub<'static>) -> ! {
    let mut rx_buffer = buffer::rx_msg::PriorityFifo::<ByteArray, 10, 10>::new();
    let mut subscriber = unwrap!(socket::Subscriber::create(
        hub,
        &mut rx_buffer,
        TEST_SUBJECT,
        Duration::from_secs(2),
    ));

    loop {
        let message = subscriber.pop().await;
        info!("Received a message: {}", &message.bytes,);
    }
}

#[embassy_executor::task]
async fn driver_rx_filter_runner(mut runner: can::RxFilterRunner<'static>) {
    runner.run().await
}

#[embassy_executor::task]
async fn driver_rx_runner(mut runner: can::RxRunner<'static>) {
    runner.run().await
}

#[embassy_executor::task]
async fn driver_tx_runner(mut runner: can::TxRunner<'static>) {
    runner.run().await
}

#[embassy_executor::task]
async fn node_runner(mut runner: MinimalNodeRunner<'static>) {
    runner.run().await
}
