//! Publisher/subscriber test with native STM32 FDCAN driver.

#![no_std]
#![no_main]

use defmt::*;
use embassy_executor::Spawner;
use embassy_stm32::{bind_interrupts, peripherals};
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_time::{Duration, Instant, Ticker};
use emcyphal::buffer;
use emcyphal::core::{NodeId, Priority, PrioritySet, SubjectId};
use emcyphal::data_types::{ByteArray, Empty};
use emcyphal::endpoint;
use emcyphal::node::{Hub, MinimalNode, MinimalNodeRunner};
use emcyphal::socket::{Publisher, Subscriber};
use emcyphal_stm32_native::{self as can, config};
use nucleo_g431rb::board;
use static_cell::StaticCell;
use {defmt_rtt as _, panic_probe as _};

bind_interrupts!(struct Irqs {
    FDCAN1_IT0 => can::IT0InterruptHandler<peripherals::FDCAN1>;
    FDCAN1_IT1 => can::IT1InterruptHandler<peripherals::FDCAN1>;
});

const NODE_ID: NodeId = NodeId::new(10).unwrap();
const TEST_SUBJECT: SubjectId = SubjectId::new(10).unwrap();
const LOOP_BACK_SUBJECT: SubjectId = SubjectId::new(11).unwrap();

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_stm32::init(board::make_peripheral_config());

    let mut can_config = board::make_can_config_native();
    can_config.test_mode = Some(config::TestMode::ExternalLoopBack);
    let can_driver = can::Driver::new(p.FDCAN1, p.PA11, p.PA12, Irqs, can_config);

    let (hub, link) = {
        static CELL: StaticCell<MinimalNode<ThreadModeRawMutex>> = StaticCell::new();
        let status = Default::default();
        let node = CELL.init(MinimalNode::new(NODE_ID, status, can::SUBJECT_SLOT_COUNT));
        let (hub, link, _, runner) = node.split();
        unwrap!(spawner.spawn(node_runner(runner)));
        (hub, link)
    };

    let (rx_filter, rx, tx) = can_driver.start(link);
    unwrap!(spawner.spawn(driver_rx_filter_runner(rx_filter)));
    unwrap!(spawner.spawn(driver_rx_runner(rx)));
    unwrap!(spawner.spawn(driver_tx_runner(tx)));

    unwrap!(spawner.spawn(sender(hub)));
    unwrap!(spawner.spawn(receiver(hub)));
    unwrap!(spawner.spawn(loop_back_sender(hub)));

    // Keep IO initialized
    let () = core::future::pending().await;
    defmt::unreachable!();
}

#[embassy_executor::task]
async fn sender(hub: Hub<'static>) -> ! {
    let mut tx_buffer = buffer::tx_msg::Blocking::<ByteArray>::new();

    let mut publisher = unwrap!(Publisher::create(
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
    let mut subscriber = unwrap!(Subscriber::create(
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
async fn loop_back_sender(hub: Hub<'static>) -> ! {
    let mut tx_buffer = buffer::tx_msg::Blocking::<ByteArray>::default();
    let mut rx_buffer = buffer::rx_msg::Watch::<Empty, 1>::default();

    let mut publisher = unwrap!(Publisher::create(
        hub,
        &mut tx_buffer,
        LOOP_BACK_SUBJECT,
        Priority::Nominal,
        Duration::from_secs(2),
        true,
    ));

    let mut rx_endpoint = unwrap!(endpoint::Rx::create_message_loop_back(
        hub,
        &mut rx_buffer,
        LOOP_BACK_SUBJECT,
        Duration::from_micros(1),
    ));

    let mut ticker = Ticker::every(Duration::from_secs(5));
    loop {
        ticker.next().await;
        info!("Send loop-back message");
        let now = Instant::now();
        let msg = ByteArray {
            bytes: Default::default(),
        };
        unwrap!(publisher.try_push(msg));

        let res = rx_endpoint.pop(PrioritySet::ALL).await;
        let transfer = unwrap!(res.ok());
        info!(
            "Sent after {}us",
            (transfer.meta.timestamp - now).as_micros()
        )
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
