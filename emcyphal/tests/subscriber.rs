use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use emcyphal::buffer;
use emcyphal::core::{NodeId, Priority, SubjectId};
use emcyphal::data_types::ByteArray;
use emcyphal::frame::{Data, DataSpecifier, Frame, Header, Mtu};
use emcyphal::node::{CoreNode, Hub};
use emcyphal::socket::Subscriber;
use emcyphal::time::{Duration, Instant};
use emcyphal_driver::link::{self, FilterUpdate};
use futures_executor::LocalPool;
use futures_task::LocalSpawn;
use std::boxed::Box;
use std::sync::atomic::{AtomicBool, Ordering};

const NODE_ID: NodeId = NodeId::new(20).unwrap();
const MTU: Mtu = Mtu::Classic;

#[test]
fn test_rx() {
    let mut executor = LocalPool::new();
    let spawner = executor.spawner();

    let (hub, (rx_filter, rx, _)) = {
        let node = CoreNode::<CriticalSectionRawMutex>::new(NODE_ID, 1);
        let node = Box::leak(Box::new(node));
        let (hub, link) = node.split();
        (hub, link.split())
    };

    let complete = Box::leak(Box::new(AtomicBool::new(false)));
    spawner
        .spawn_local_obj(Box::new(test_rx_receive(hub, complete)).into())
        .unwrap();
    spawner
        .spawn_local_obj(Box::new(test_rx_send(rx_filter, rx)).into())
        .unwrap();
    executor.run_until_stalled();

    assert!(complete.load(Ordering::SeqCst));
}

async fn test_rx_receive(hub: Hub<'static>, complete: &'static AtomicBool) {
    let mut buffer: buffer::rx_msg::PriorityFifo<ByteArray, 10, 10> = Default::default();

    let mut subscriber = Subscriber::create(
        hub,
        &mut buffer,
        SubjectId::try_from(0x1a).unwrap(),
        Duration::from_micros(2_000_000),
    )
    .unwrap();

    let transfer = subscriber.pop().await;
    assert_eq!(
        transfer,
        ByteArray {
            bytes: heapless::Vec::from_slice(&[0x02]).unwrap(),
        },
    );

    complete.store(true, Ordering::SeqCst);
}

async fn test_rx_send(mut rx_filter: link::RxFilter<'static>, mut rx: link::Rx<'static>) {
    // Skip AddDestination request
    rx_filter.pop().await;

    let subject_update = rx_filter.pop().await;
    assert_eq!(
        subject_update,
        FilterUpdate::AddSubject(SubjectId::try_from(0x1a).unwrap()),
    );

    let frame = Frame {
        header: Header {
            priority: Priority::Nominal,
            data_spec: DataSpecifier::Message(SubjectId::new(0x1a).unwrap()),
            destination: None,
            source: Some(NodeId::new(0x0a).unwrap()),
        },
        data: Data::new(&[0x01, 0x00, 0x02, 0b1110_0101]).unwrap(),
        timestamp: ts(1000),
        loop_back: false,
    };
    rx.push(frame, MTU).await;
}

fn ts(us: u64) -> Instant {
    Instant::MIN.saturating_add(Duration::from_micros(us))
}
