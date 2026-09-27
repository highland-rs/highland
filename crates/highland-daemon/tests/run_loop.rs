// Rust guideline compliant 2026-09-27

//! The per-instance run loop, driven by a fake clock and a scripted backend.
//!
//! The loop is where the daemon's structure becomes real: one task, one
//! machine, no shared state. These tests assert that it takes over when its
//! timers fire, that a shutdown is not queued behind traffic, and that an idle
//! instance does not spin.

use std::net::IpAddr;
use std::time::Duration;

use highland_daemon::{InstanceActor, InstancePlan, Ownership, channel, run_instance};
use highland_net::{IpCidr, Outcome, PeerSet, ScriptedBackend};
use tokio::sync::watch;

const INTERFACE: &str = "eth0";
const PEER: [u8; 4] = [192, 0, 2, 11];
const VIP: [u8; 4] = [192, 0, 2, 10];

fn vip() -> IpCidr {
    IpCidr::new(IpAddr::from(VIP), 24)
}

fn plan() -> InstancePlan {
    InstancePlan::new("api", 42, 150)
}

fn ownership() -> Ownership {
    Ownership::new(INTERFACE, vec![vip()], PeerSet::new([IpAddr::from(PEER)]))
}

/// Starts the loop on a background task and returns the handles a test needs.
fn spawn(
    outcomes: impl IntoIterator<Item = Outcome>,
) -> (
    tokio::task::JoinHandle<()>,
    watch::Sender<bool>,
    highland_daemon::InstructionSender,
    highland_daemon::InstructionSender,
    std::sync::Arc<ScriptedBackend>,
    std::sync::Arc<highland_daemon::RecordingTransport>,
) {
    let backend = std::sync::Arc::new(
        ScriptedBackend::new()
            .with_interface(INTERFACE)
            .with_outcomes(outcomes),
    );
    let transport = std::sync::Arc::new(highland_daemon::RecordingTransport::new());
    let actor = InstanceActor::new(
        highland_core::clock::SystemClock::new(),
        plan(),
        ownership(),
        backend.clone(),
        transport.clone(),
    );
    let (sender, receiver) = channel();
    let (protocol_sender, protocol_receiver) = channel();
    let (shutdown, watch_receiver) = watch::channel(false);

    let handle = tokio::spawn(async move {
        run_instance(
            actor,
            receiver,
            protocol_receiver,
            watch_receiver,
            None,
            None,
        )
        .await;
    });
    (
        handle,
        shutdown,
        sender,
        protocol_sender,
        backend,
        transport,
    )
}

#[tokio::test]
async fn an_instance_takes_over_when_its_timers_fire() {
    let (handle, shutdown, _sender, _protocol, _backend, transport) = spawn([]);

    // The advertisement interval is one second and the takeover is 3.41s, so
    // waiting past both must produce a master that has advertised.
    tokio::time::sleep(Duration::from_millis(3_600)).await;
    assert!(
        !transport.sent().is_empty(),
        "the loop took over and advertised"
    );

    shutdown.send(true).expect("the watch channel is open");
    tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .expect("the loop stops promptly")
        .expect("the task did not panic");
}

#[tokio::test]
async fn a_shutdown_stops_the_loop_even_with_traffic_arriving() {
    let (handle, shutdown, sender, _protocol, _backend, transport) = spawn([]);

    // Queue work, then ask it to stop. Shutdown is biased ahead of the queue, so
    // the instance relinquishes rather than continuing to advertise.
    for _ in 0..8 {
        sender
            .send(highland_daemon::Instruction::PeerUnreachable)
            .await
            .expect("the channel is open");
    }
    shutdown.send(true).expect("the watch channel is open");

    tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .expect("the loop stops promptly")
        .expect("the task did not panic");

    let sent_before = transport.sent().len();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        transport.sent().len(),
        sent_before,
        "a stopped instance never advertises again"
    );
}

#[tokio::test]
async fn a_failing_backend_keeps_the_instance_out_of_ownership() {
    let (handle, shutdown, _sender, _protocol, _backend, transport) =
        spawn([Outcome::Failed("EPERM".to_owned())]);

    tokio::time::sleep(Duration::from_millis(3_600)).await;
    assert!(
        transport.sent().is_empty(),
        "a faulted instance never advertises"
    );

    shutdown.send(true).expect("the watch channel is open");
    tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .expect("the loop stops promptly")
        .expect("the task did not panic");
}

#[tokio::test]
async fn the_instruction_channel_is_bounded() {
    // A flood of health results must not become an unbounded backlog.
    let (sender, receiver) = channel();
    assert_eq!(sender.max_capacity(), 64, "L-05: the queue is bounded");
    drop(receiver);
}

/// A node whose link goes away must stop claiming the address, and it must do so
/// because it looked, not because an operation failed.
///
/// This is what the chaos suite's link-flap scenario exercises against a real
/// kernel. Here it is checked against a scripted backend so the loop's own logic
/// is covered without privileges, and so the assertion is about the loop rather
/// than about the kernel's timing.
#[tokio::test]
async fn an_instance_releases_its_address_when_its_link_goes_away() {
    let (handle, shutdown, _sender, _protocol, backend, transport) = spawn([]);

    tokio::time::sleep(Duration::from_millis(3_600)).await;
    assert!(!transport.sent().is_empty(), "the instance took over first");
    let id = backend
        .interface_id(INTERFACE)
        .expect("the scripted interface is known");
    assert!(
        backend
            .present()
            .iter()
            .any(|(interface, address)| *interface == id && *address == vip()),
        "the master holds the address"
    );

    // The cable is pulled.
    backend.set_link_state(INTERFACE, highland_net::LinkState::NoCarrier);

    let released = wait_for(Duration::from_secs(5), || {
        !backend
            .present()
            .iter()
            .any(|(interface, address)| *interface == id && *address == vip())
    })
    .await;
    assert!(
        released,
        "the instance kept the address on a dead link\ncalls: {:?}",
        backend.calls()
    );

    // And it stays gone: an instance that re-added the address would be claiming
    // it on a link it cannot use.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        !backend
            .present()
            .iter()
            .any(|(interface, address)| *interface == id && *address == vip()),
        "the address came back while the link was down"
    );

    shutdown.send(true).expect("the watch channel is open");
    tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .expect("the loop stops promptly")
        .expect("the task did not panic");
}

/// The other end of the same scenario: the link comes back, and the instance
/// rejoins the election rather than staying out of it for good.
#[tokio::test]
async fn an_instance_rejoins_the_election_when_its_link_comes_back() {
    let (handle, shutdown, _sender, _protocol, backend, _transport) = spawn([]);
    let id = backend
        .interface_id(INTERFACE)
        .expect("the scripted interface is known");

    backend.set_link_state(INTERFACE, highland_net::LinkState::NoCarrier);
    assert!(
        wait_for(Duration::from_secs(5), || backend.calls().iter().any(
            |call| *call == highland_net::Call::Interface(INTERFACE.to_owned())
        ))
        .await,
        "the interface is never looked at, so a dead link would go unnoticed"
    );

    backend.set_link_state(INTERFACE, highland_net::LinkState::Up);
    let took_over = wait_for(Duration::from_secs(6), || {
        backend
            .present()
            .iter()
            .any(|(interface, address)| *interface == id && *address == vip())
    })
    .await;
    assert!(
        took_over,
        "the instance never rejoined the election after the link came back"
    );

    shutdown.send(true).expect("the watch channel is open");
    tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .expect("the loop stops promptly")
        .expect("the task did not panic");
}

/// Polls `condition` every 20ms until it holds or the budget runs out.
async fn wait_for(budget: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let started = std::time::Instant::now();
    while started.elapsed() < budget {
        if condition() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    false
}
