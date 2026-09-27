// Rust guideline compliant 2026-09-27

//! A single instance's whole life, driven through a scripted backend.
//!
//! This is the failover path that Milestone 3 exists to make work, exercised
//! end to end: the state machine decides, the executor acts, the outcomes come
//! back, and the VIP moves. It runs with no privileges, no namespace, and no
//! sleeping, because the clock is a fake and the kernel is a script.
//!
//! The namespace proof is Milestone 4's exit criterion and needs root. What this
//! file establishes is that the logic and the sequencing are right, so that when
//! it runs against a real kernel there is one variable rather than two.

use std::net::IpAddr;
use std::time::Duration;

use highland_core::clock::ManualClock;
use highland_core::machine::PendingOwnership;
use highland_core::state::{Event, Role, TimerId};
use highland_daemon::{InstanceActor, InstancePlan, Ownership};
use highland_net::{Call, IpCidr, Outcome, PeerSet, ScriptedBackend};

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

/// The actor under test, with the handles a test needs to inspect.
struct Fixture {
    actor: InstanceActor<ManualClock, ScriptedBackend, highland_daemon::RecordingTransport>,
    clock: ManualClock,
    backend: std::sync::Arc<ScriptedBackend>,
    transport: std::sync::Arc<highland_daemon::RecordingTransport>,
}

impl Fixture {
    fn new() -> Self {
        Self::with_outcomes([])
    }

    fn with_outcomes(outcomes: impl IntoIterator<Item = Outcome>) -> Self {
        let clock = ManualClock::new();
        let backend = std::sync::Arc::new(
            ScriptedBackend::new()
                .with_interface(INTERFACE)
                .with_outcomes(outcomes),
        );
        let transport = std::sync::Arc::new(highland_daemon::RecordingTransport::new());
        let actor = InstanceActor::new(
            clock.clone(),
            plan(),
            ownership(),
            backend.clone(),
            transport.clone(),
        );
        Self {
            actor,
            clock,
            backend,
            transport,
        }
    }

    fn calls(&self) -> Vec<Call> {
        self.backend.calls()
    }

    fn sent(&self) -> Vec<highland_vrrp::Advertisement> {
        self.transport.sent()
    }
}

#[tokio::test]
async fn a_single_instance_starts_as_backup_and_takes_over_when_the_master_silent() {
    let mut fixture = Fixture::new();

    fixture.actor.handle(Event::Startup).await;
    assert_eq!(
        fixture.actor.role(),
        Role::Backup,
        "one instance starts as backup"
    );
    assert!(fixture.calls().is_empty(), "a backup touches nothing");

    // Nothing was heard for Master_Down_Interval, so the instance takes over.
    fixture.clock.advance(Duration::from_millis(3410));
    let fired = fixture.actor.fire_due_timers().await;

    assert_eq!(fixture.actor.role(), Role::Master);
    assert_eq!(fixture.actor.machine().pending(), PendingOwnership::None);
    assert!(fired.iter().any(|(timer, _)| *timer == TimerId::MasterDown));
}

#[tokio::test]
async fn ownership_is_confirmed_before_the_instance_advertises() {
    let mut fixture = Fixture::new();
    fixture.actor.handle(Event::Startup).await;
    fixture.clock.advance(Duration::from_millis(3410));
    fixture.actor.fire_due_timers().await;

    let calls = fixture.calls();
    let added = calls
        .iter()
        .position(|call| matches!(call, Call::AddAddress(_, address) if *address == vip()))
        .expect("the address is added");
    let first_advertisement = calls
        .iter()
        .position(|call| matches!(call, Call::GratuitousUpdate(_, _)))
        .expect("a gratuitous update follows");

    assert!(
        added < first_advertisement,
        "I-04: the VIP exists before anything announces it"
    );
    assert!(!fixture.sent().is_empty(), "a master advertises");
}

#[tokio::test]
async fn the_first_advertisement_carries_the_effective_priority() {
    let mut fixture = Fixture::new();
    fixture.actor.handle(Event::Startup).await;
    fixture.clock.advance(Duration::from_millis(3410));
    fixture.actor.fire_due_timers().await;

    let sent = fixture.sent();
    let first = sent.first().expect("a master advertises");
    assert_eq!(first.priority().get(), 150);
    assert_eq!(first.vrid().get(), 42);
    assert_eq!(first.addresses(), [IpAddr::from(VIP)]);
    assert_eq!(
        first.max_adver_int().centiseconds(),
        100,
        "the configured one second"
    );
}

#[tokio::test]
async fn a_master_advertises_again_when_its_timer_expires() {
    let mut fixture = Fixture::new();
    fixture.actor.handle(Event::Startup).await;
    fixture.clock.advance(Duration::from_millis(3410));
    fixture.actor.fire_due_timers().await;
    let after_takeover = fixture.sent().len();
    assert!(after_takeover >= 1);

    fixture.clock.advance(Duration::from_millis(1000));
    fixture.actor.fire_due_timers().await;
    assert!(
        fixture.sent().len() > after_takeover,
        "the advertisement timer keeps firing"
    );
}

#[tokio::test]
async fn a_failed_ownership_attempt_faults_instead_of_claiming_the_address() {
    // The interface lookup is the first backend call, so scripting a failure
    // there is the shortest path to a fault.
    let mut fixture = Fixture::with_outcomes([Outcome::Failed("EPERM".to_owned())]);

    fixture.actor.handle(Event::Startup).await;
    fixture.clock.advance(Duration::from_millis(3410));
    fixture.actor.fire_due_timers().await;

    assert_eq!(
        fixture.actor.role(),
        Role::Fault,
        "a failed add is a fault, not a master"
    );
    assert!(
        fixture
            .actor
            .machine()
            .deadline_of(TimerId::HoldDown)
            .is_some(),
        "a hold-down is armed"
    );
    assert!(
        fixture.sent().is_empty(),
        "a faulted instance never advertises"
    );
}

#[tokio::test]
async fn relinquishing_removes_the_address_and_stops_advertising() {
    let mut fixture = Fixture::new();
    fixture.actor.handle(Event::Startup).await;
    fixture.clock.advance(Duration::from_millis(3410));
    fixture.actor.fire_due_timers().await;
    assert_eq!(fixture.actor.role(), Role::Master);
    let while_master = fixture.sent().len();

    fixture
        .actor
        .handle(Event::OperatorRelinquishRequested)
        .await;
    assert_eq!(
        fixture.actor.role(),
        Role::Backup,
        "release is immediate and confirmed"
    );
    assert!(
        fixture
            .calls()
            .iter()
            .any(|call| matches!(call, Call::RemoveAddress(_, address) if *address == vip())),
        "the VIP is removed"
    );

    fixture.clock.advance(Duration::from_secs(5));
    fixture.actor.fire_due_timers().await;
    assert!(
        fixture.sent().len() > while_master,
        "the backup took over again and is advertising"
    );
}

#[tokio::test]
async fn a_shutdown_relinquishes_before_the_process_exits() {
    let mut fixture = Fixture::new();
    fixture.actor.handle(Event::Startup).await;
    fixture.clock.advance(Duration::from_millis(3410));
    fixture.actor.fire_due_timers().await;
    let before = fixture.sent().len();

    fixture.actor.handle(Event::ShutdownRequested).await;

    assert!(
        fixture
            .calls()
            .iter()
            .any(|call| matches!(call, Call::RemoveAddress(_, address) if *address == vip())),
        "the VIP is released on the way down"
    );
    assert!(fixture.actor.machine().is_shutting_down());

    fixture.clock.advance(Duration::from_secs(10));
    fixture.actor.fire_due_timers().await;
    assert_eq!(
        fixture.sent().len(),
        before + 1,
        "I-32: a shutting-down instance never advertises again"
    );
}

#[tokio::test]
async fn a_paused_instance_stops_participating() {
    let mut fixture = Fixture::new();
    fixture.actor.handle(Event::Startup).await;
    fixture.actor.handle(Event::OperatorPauseRequested).await;

    assert_eq!(fixture.actor.role(), Role::Disabled);
    assert!(
        fixture.actor.machine().timers().is_empty(),
        "a disabled instance arms no timers"
    );

    fixture.clock.advance(Duration::from_secs(60));
    fixture.actor.fire_due_timers().await;
    assert!(
        fixture.sent().is_empty(),
        "a disabled instance never takes over"
    );
}

#[tokio::test]
async fn a_higher_priority_advertisement_makes_a_master_step_down() {
    let mut fixture = Fixture::new();
    fixture.actor.handle(Event::Startup).await;
    fixture.clock.advance(Duration::from_millis(3410));
    fixture.actor.fire_due_timers().await;
    assert_eq!(fixture.actor.role(), Role::Master);

    let peer = highland_core::state::PeerAdvertisement {
        vrid: 42,
        priority: 200,
        source: IpAddr::from(PEER),
        advert_interval: Duration::from_secs(1),
    };
    fixture
        .actor
        .handle(Event::AdvertisementReceived(peer))
        .await;

    assert_eq!(
        fixture.actor.role(),
        Role::Backup,
        "R-16: a master steps down for a stronger peer"
    );
    assert!(fixture.actor.machine().last_peer().is_some());
}

#[tokio::test]
async fn the_role_and_ownership_never_disagree() {
    let mut fixture = Fixture::new();
    fixture.actor.handle(Event::Startup).await;
    let events = [
        Event::TimerExpired(TimerId::MasterDown),
        Event::OperatorRelinquishRequested,
        Event::InterfaceDown,
        Event::InterfaceUp,
        Event::ShutdownRequested,
    ];

    for event in events {
        fixture.actor.handle(event).await;
        let role = fixture.actor.role();
        let owns = fixture.actor.machine().owns_virtual_addresses();
        assert_eq!(
            owns,
            role == Role::Master,
            "I-03 and I-14: ownership is a property of MASTER"
        );
    }
}

#[tokio::test]
async fn a_backup_never_touches_the_kernel() {
    let mut fixture = Fixture::new();
    fixture.actor.handle(Event::Startup).await;
    fixture.clock.advance(Duration::from_millis(1000));
    fixture.actor.fire_due_timers().await;

    assert!(
        fixture.calls().is_empty(),
        "I-14: a backup adds no address and sends no advertisement, found {:?}",
        fixture.calls()
    );
}
