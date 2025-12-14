use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use emcyphal::buffer;
use emcyphal::core::{NodeId, Priority, PrioritySet, ServiceId};
use emcyphal::data_types::ByteArray;
use emcyphal::frame::{Data, DataSpecifier, Frame, Header, Mtu};
use emcyphal::node::{CoreNode, Hub};
use emcyphal::socket::Responder;
use emcyphal::time::{Duration, Instant};
use emcyphal_driver::link::{self, FilterUpdate};
use futures_executor::LocalPool;
use futures_task::LocalSpawn;
use std::boxed::Box;
use std::sync::atomic::{AtomicBool, Ordering};

const REQUESTER_NODE: NodeId = NodeId::from_truncating(10);
const RESPONDER_NODE: NodeId = NodeId::from_truncating(20);
const SERVICE_ID: ServiceId = ServiceId::from_truncating(100);
const TIMEOUT: Duration = Duration::from_micros(2_000_000);
const PRIORITY: Priority = Priority::Nominal;
const MTU: Mtu = Mtu::Classic;

#[test]
fn test_rx() {
    let mut executor = LocalPool::new();
    let spawner = executor.spawner();

    let (hub, (rx_filter, rx, tx)) = {
        let node = CoreNode::<CriticalSectionRawMutex>::new(RESPONDER_NODE, 1);
        let node = Box::leak(Box::new(node));
        let (hub, link) = node.split();
        (hub, link.split())
    };

    let complete = Box::leak(Box::new(AtomicBool::new(false)));
    spawner
        .spawn_local_obj(Box::new(run_responder(hub)).into())
        .unwrap();
    spawner
        .spawn_local_obj(Box::new(send_request(rx_filter, rx)).into())
        .unwrap();
    spawner
        .spawn_local_obj(Box::new(test_response(tx, complete)).into())
        .unwrap();
    executor.run_until_stalled();

    assert!(complete.load(Ordering::SeqCst));
}

async fn run_responder(hub: Hub<'static>) {
    let mut req_buffer: buffer::rx_req::PriorityFifo<ByteArray, 10, 10> = Default::default();
    let mut resp_buffer: buffer::tx_resp::Blocking<ByteArray> = Default::default();
    let mut responder = Responder::create(
        hub,
        &mut req_buffer,
        &mut resp_buffer,
        SERVICE_ID,
        TIMEOUT,
        TIMEOUT,
    )
    .unwrap();

    responder
        .run(async |req| {
            let mut bytes = req.bytes.clone();
            for val in bytes.iter_mut() {
                *val = val.wrapping_add(1);
            }
            ByteArray { bytes }
        })
        .await;
}

async fn send_request(mut rx_filter: link::RxFilter<'static>, mut rx: link::Rx<'static>) {
    let subject_update = rx_filter.pop().await;
    assert_eq!(subject_update, FilterUpdate::AddDestination(RESPONDER_NODE));

    let frame = Frame {
        header: Header {
            priority: PRIORITY,
            data_spec: DataSpecifier::Request(SERVICE_ID),
            destination: Some(RESPONDER_NODE),
            source: Some(REQUESTER_NODE),
        },
        data: Data::new(&[0x01, 0x00, 0x02, 0b1110_0101]).unwrap(),
        timestamp: ts(1000),
        loop_back: false,
    };
    rx.push(frame, MTU).await;
}

async fn test_response(mut tx: link::Tx<'static>, complete: &'static AtomicBool) {
    let ref_envelope = Frame {
        header: Header {
            priority: PRIORITY,
            data_spec: DataSpecifier::Response(SERVICE_ID),
            destination: Some(REQUESTER_NODE),
            source: Some(RESPONDER_NODE),
        },
        data: Data::new(&[0x01, 0x00, 0x03, 0b1110_0101]).unwrap(),
        timestamp: ts(1000) + TIMEOUT,
        loop_back: false,
    };
    let test_envelope = tx.pop(PrioritySet::new_eq(PRIORITY), MTU).await;
    assert_eq!(test_envelope, ref_envelope);

    complete.store(true, Ordering::SeqCst);
}

fn ts(us: u64) -> Instant {
    Instant::MIN.saturating_add(Duration::from_micros(us))
}
