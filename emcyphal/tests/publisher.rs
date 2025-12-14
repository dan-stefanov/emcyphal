use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_time::Duration;
use emcyphal::buffer;
use emcyphal::core::SubjectId;
use emcyphal::data_types::ByteArray;
use emcyphal::frame::{DataSpecifier, Header, Mtu};
use emcyphal::node::{CoreNode, Hub};
use emcyphal::socket::Publisher;
use emcyphal_core::{NodeId, Priority, PrioritySet};
use emcyphal_driver::link;
use futures_executor::LocalPool;
use futures_task::LocalSpawn;
use std::boxed::Box;
use std::future::pending;
use std::sync::atomic::{AtomicBool, Ordering};

const NODE_ID: NodeId = NodeId::new(10).unwrap();
const PRIORITY: Priority = Priority::Nominal;
const MTU: Mtu = Mtu::Classic;

#[test]
fn test_tx() {
    let mut executor = LocalPool::new();
    let spawner = executor.spawner();

    let (hub, (_, _, tx)) = {
        let node = CoreNode::<CriticalSectionRawMutex>::new(NODE_ID, 0);
        let node = Box::leak(Box::new(node));
        let (hub, link) = node.split();
        (hub, link.split())
    };

    let tx_complete = Box::leak(Box::new(AtomicBool::new(false)));
    let rx_complete = Box::leak(Box::new(AtomicBool::new(false)));

    spawner
        .spawn_local_obj(Box::new(test_tx_send(hub, tx_complete)).into())
        .unwrap();

    spawner
        .spawn_local_obj(Box::new(test_tx_receive(tx, rx_complete)).into())
        .unwrap();

    executor.run_until_stalled();

    assert!(rx_complete.load(Ordering::SeqCst));
}

async fn test_tx_send(hub: Hub<'static>, complete: &'static AtomicBool) {
    let mut buffer: buffer::tx_msg::Blocking<ByteArray> = Default::default();

    {
        let mut publisher = Publisher::create(
            hub,
            &mut buffer,
            SubjectId::try_from(0x1a).unwrap(),
            PRIORITY,
            Duration::from_secs(2),
            false,
        )
        .unwrap();

        publisher
            .push(ByteArray {
                bytes: heapless::Vec::from_slice(&[0x02]).unwrap(),
            })
            .await;

        // keep socket alive till transfer is fetched
        pending().await
    }

    complete.store(true, Ordering::SeqCst);
}

async fn test_tx_receive(mut tx: link::Tx<'static>, complete: &'static AtomicBool) {
    let frame = tx.pop(PrioritySet::new_eq(PRIORITY), MTU).await;
    assert_eq!(
        frame.header,
        Header {
            priority: PRIORITY,
            data_spec: DataSpecifier::Message(SubjectId::from_truncating(0x1a)),
            destination: None,
            source: Some(NodeId::from_truncating(0x0a)),
        }
    );
    assert_eq!(frame.data.as_ref(), [0x01, 0x00, 0x02, 0b1110_0000]);

    complete.store(true, Ordering::SeqCst);
}
