use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_time::{Duration, MockDriver};
use emcyphal::frame::{DataSpecifier, Mtu};
use emcyphal::node::{MinimalNode, MinimalNodeRunner};
use emcyphal_core::{NodeId, Priority, PrioritySet, SubjectId};
use emcyphal_driver::link;
use futures_executor::LocalPool;
use futures_task::LocalSpawn;
use std::boxed::Box;
use std::sync::atomic::{AtomicBool, Ordering};

const NODE_ID: NodeId = NodeId::new(10).unwrap();
const HEARTBEAT_SUBJECT: SubjectId = SubjectId::new(7509).unwrap();
const HEARTBEAT_PERIOD: Duration = Duration::from_secs(1);

#[test]
fn test_heartbeat() {
    let mut executor = LocalPool::new();
    let spawner = executor.spawner();

    let time = MockDriver::get();

    let ((_, _, tx), runner) = {
        let status = Default::default();
        let node = MinimalNode::<CriticalSectionRawMutex>::new(NODE_ID, status, 0);
        let node = Box::leak(Box::new(node));
        let (_, link, _, runner) = node.split();
        (link.split(), runner)
    };

    let rx_complete = Box::leak(Box::new(AtomicBool::new(false)));

    spawner
        .spawn_local_obj(Box::new(node_runner(runner)).into())
        .unwrap();

    spawner
        .spawn_local_obj(Box::new(test_tx_receive(tx, rx_complete)).into())
        .unwrap();

    executor.run_until_stalled();
    time.advance(2 * HEARTBEAT_PERIOD);
    executor.run_until_stalled();

    assert!(rx_complete.load(Ordering::SeqCst));
}

async fn node_runner(mut runner: MinimalNodeRunner<'static>) {
    print!("Start runner");
    runner.run().await
}

async fn test_tx_receive(mut tx: link::Tx<'static>, complete: &'static AtomicBool) {
    let frame = tx
        .pop(PrioritySet::new_eq(Priority::Nominal), Mtu::Classic)
        .await;
    assert_eq!(
        frame.header.data_spec,
        DataSpecifier::Message(HEARTBEAT_SUBJECT)
    );

    complete.store(true, Ordering::SeqCst);
}
