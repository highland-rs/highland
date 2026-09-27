// Rust guideline compliant 2026-09-27

//! Property tests for the state machine.
//!
//! The unit tests in `tests/state_machine.rs` check one behaviour each. These
//! tests instead check that the invariants hold for **every** reachable state,
//! by generating arbitrary event sequences and asserting the invariants after
//! each one. A bug that only appears after an unusual ordering of events is what
//! a state machine actually produces, so this is the test tier that matters.

use std::net::IpAddr;
use std::time::Duration;

use highland_core::clock::ManualClock;
use highland_core::election::{
    Candidate, ElectionReason, Incumbent, Winner, decide, may_preempt, must_step_down,
};
use highland_core::health::{HealthPolicy, HealthPolicyConfig, HealthSummary, evaluate};
use highland_core::machine::{InstanceStateMachine, PendingOwnership};
use highland_core::state::{
    Action, ActionKind, Event, Generation, InstanceConfig, PeerAdvertisement, Role, TimerId,
    master_down_interval,
};
use highland_core::timer::{RetryPolicy, TimerSet};
use proptest::prelude::*;

const VRID: u8 = 42;

// ----- generators --------------------------------------------------------

fn arb_priority() -> impl Strategy<Value = u8> {
    prop_oneof![
        Just(1),
        Just(100),
        Just(150),
        Just(254),
        Just(255),
        1u8..=255
    ]
}

fn arb_interval() -> impl Strategy<Value = Duration> {
    prop_oneof![
        Just(Duration::from_millis(10)),
        Just(Duration::from_millis(100)),
        Just(Duration::from_secs(1)),
        Just(Duration::from_millis(2550)),
    ]
}

fn arb_preempt_delay() -> impl Strategy<Value = Duration> {
    prop_oneof![
        Just(Duration::ZERO),
        Just(Duration::from_secs(3)),
        Just(Duration::from_secs(30))
    ]
}

/// A small set of peer addresses, including a mix of families so that the
/// tie-break path is exercised.
fn arb_source() -> impl Strategy<Value = IpAddr> {
    prop_oneof![
        Just("192.0.2.1".parse().expect("valid address")),
        Just("192.0.2.99".parse().expect("valid address")),
        Just("2001:db8::1".parse().expect("valid address")),
    ]
}

/// An advertisement with any priority the protocol allows, including the
/// reserved zero.
fn arb_advert() -> impl Strategy<Value = Event> {
    (arb_source(), 0u8..=255).prop_map(|(source, priority)| {
        Event::AdvertisementReceived(PeerAdvertisement {
            vrid: VRID,
            priority,
            source,
            advert_interval: Duration::from_secs(1),
        })
    })
}

/// Builds a machine from a generated configuration.
fn machine_with(config: InstanceConfig) -> InstanceStateMachine<ManualClock> {
    InstanceStateMachine::new(config, ManualClock::new())
}

fn arb_config() -> impl Strategy<Value = InstanceConfig> {
    (
        arb_priority(),
        arb_interval(),
        arb_preempt_delay(),
        any::<bool>(),
        arb_priority(),
    )
        .prop_map(
            |(priority, interval, preempt_delay, preempt, minimum)| InstanceConfig {
                name: "api".to_owned(),
                vrid: VRID,
                priority,
                advertisement_interval: interval,
                startup_delay: Duration::ZERO,
                preempt,
                preempt_delay: if preempt {
                    preempt_delay
                } else {
                    Duration::ZERO
                },
                hold_down: Duration::from_secs(10),
                retry: RetryPolicy::default(),
                health: HealthPolicyConfig {
                    policy: HealthPolicy::Weighted,
                    minimum_effective_priority: minimum.clamp(0, priority),
                    all_checks_required: false,
                    send_zero_priority_advert: true,
                    total_weight: 255,
                },
                primary_ipv4: Some("192.0.2.10".parse().expect("valid address")),
                primary_ipv6: None,
            },
        )
}

/// Builds a sequence of events from a small alphabet that covers every
/// transition path.
fn arb_events() -> impl Strategy<Value = Vec<Event>> {
    let leaf = prop_oneof![
        Just(Event::Startup),
        Just(Event::StartupDelayElapsed),
        Just(Event::InterfaceUp),
        Just(Event::InterfaceDown),
        Just(Event::CarrierLost),
        Just(Event::AdvertisementTimeout),
        Just(Event::OperatorRelinquishRequested),
        Just(Event::OperatorPauseRequested),
        Just(Event::OperatorResumeRequested),
        Just(Event::ShutdownRequested),
        arb_timer(),
        arb_outcome(),
        arb_advert(),
        arb_health(),
        (0u8..=2).prop_map(|slot| match slot {
            0 => Event::ConfigurationReloaded {
                generation: Generation::initial().next()
            },
            1 => Event::ConfigurationReloaded {
                generation: Generation::initial()
            },
            _ => Event::OperatorForceTransitionRequested {
                target: Role::Master,
                reason: "property test".to_owned(),
            },
        }),
    ];
    prop::collection::vec(leaf, 0..24)
}

fn arb_timer() -> impl Strategy<Value = Event> {
    prop_oneof![
        Just(TimerId::StartupDelay),
        Just(TimerId::Advertisement),
        Just(TimerId::MasterDown),
        Just(TimerId::PreemptionDelay),
        Just(TimerId::HoldDown),
        Just(TimerId::Retry),
    ]
    .prop_map(Event::TimerExpired)
}

fn arb_outcome() -> impl Strategy<Value = Event> {
    (any::<bool>(), 0u8..9).prop_map(|(ok, slot)| {
        let kind = match slot {
            0 => ActionKind::AddAddresses,
            1 => ActionKind::RemoveAddresses,
            2 => ActionKind::Advertisement,
            3 => ActionKind::Timer,
            4 => ActionKind::SetPriority,
            5 => ActionKind::EnterRole,
            6 => ActionKind::EmitEvent,
            7 => ActionKind::Log,
            _ => ActionKind::GratuitousUpdate,
        };
        if ok {
            Event::ActionSucceeded { kind }
        } else {
            Event::ActionFailed {
                kind,
                error: "property test".to_owned(),
            }
        }
    })
}

fn arb_health() -> impl Strategy<Value = Event> {
    (0u16..=300, 0usize..4, 0usize..4).prop_map(|(penalty, electoral, total)| {
        Event::HealthChanged(HealthSummary {
            penalty,
            electoral_failures: electoral,
            total_failures: total.max(electoral),
            passing: 0,
            stale_discarded: 0,
        })
    })
}

/// Asserts every invariant that must hold in any reachable state.
fn assert_invariants(machine: &InstanceStateMachine<ManualClock>, context: &str) {
    let role = machine.role();

    // I-01, I-35: exactly one role, always.
    assert!(matches!(
        role,
        Role::Init | Role::Backup | Role::Master | Role::Fault | Role::Disabled
    ));

    // I-03, I-14: ownership exists only in MASTER.
    assert_eq!(
        machine.owns_virtual_addresses(),
        role == Role::Master,
        "{context}: ownership and role must agree, role is {role}"
    );

    // I-04, I-18: a master advertises only while it owns.
    if machine.is_armed_for_advertising() {
        assert_eq!(role, Role::Master, "{context}: only a master advertises");
        assert!(
            machine.owns_virtual_addresses(),
            "{context}: a master must own to advertise"
        );
    }

    // I-16: a faulted or disabled instance neither owns nor advertises.
    if matches!(role, Role::Fault | Role::Disabled) {
        assert!(
            !machine.owns_virtual_addresses(),
            "{context}: {role} must not own"
        );
        assert!(
            !machine.is_armed_for_advertising(),
            "{context}: {role} must not advertise"
        );
    }

    // I-23, I-24: the effective priority is sane.
    let effective = machine.effective_priority();
    assert!(
        effective <= machine.config().priority,
        "{context}: I-24: never above configured"
    );
    if machine.is_eligible() {
        assert!(
            effective > 0,
            "{context}: I-23: an eligible instance has priority"
        );
    }

    // I-29: a cancelled timer is not armed.
    if matches!(role, Role::Disabled) {
        assert!(
            machine.timers().is_empty(),
            "{context}: a disabled instance arms no timers"
        );
    }

    // I-32: a shutting-down instance stops advertising immediately. It stays
    // `MASTER` until removal is confirmed, because `I-14` forbids a non-master
    // role from owning addresses; what must never happen is a new acquisition.
    if machine.is_shutting_down() {
        assert!(
            !machine.is_armed_for_advertising(),
            "{context}: a shutting-down instance stops advertising at once"
        );
    }

    // The pending operation and the role must be consistent.
    match machine.pending() {
        PendingOwnership::Acquiring => {
            assert_ne!(
                role,
                Role::Master,
                "{context}: ownership is requested before it is held"
            );
        }
        PendingOwnership::Releasing => {
            assert!(
                role == Role::Master || !machine.owns_virtual_addresses(),
                "{context}: releasing is only meaningful while owning"
            );
        }
        PendingOwnership::None => {}
    }
}

// ----- properties over event sequences -----------------------------------

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    /// The invariants hold after every prefix of any event sequence.
    #[test]
    fn invariants_hold_after_every_event(config in arb_config(), events in arb_events()) {
        let mut machine = machine_with(config);
        let clock = ManualClock::new();

        for (index, event) in events.into_iter().enumerate() {
            // Advance time irregularly, so timers fire in a non-obvious order.
            clock.advance(Duration::from_millis(1 + (index as u64 % 97)));
            let actions = machine.handle(event);
            assert_invariants(&machine, &format!("event {index}"));

            // A master is never asked to advertise without ownership.
            if machine.role() == Role::Master && !machine.owns_virtual_addresses() {
                let advertises = actions
                    .iter()
                    .any(|action| matches!(action, Action::SendAdvertisement { .. }));
                prop_assert!(!advertises);
            }

            // I-32: no address is ever added once shutdown has begun.
            if machine.is_shutting_down() {
                let adds = actions.contains(&Action::AddVirtualAddresses);
                prop_assert!(!adds, "a shutting-down instance never adds an address");
            }
        }
    }

    /// The machine is deterministic: the same events produce the same actions.
    #[test]
    fn the_machine_is_deterministic(config in arb_config(), events in arb_events()) {
        let first = run_once(&config, &events);
        let second = run_once(&config, &events);
        prop_assert_eq!(first, second);
    }

    /// A takeover never advertises before ownership is confirmed.
    #[test]
    fn ownership_precedes_every_advertisement(
        config in arb_config(),
        events in arb_events(),
    ) {
        let mut machine = machine_with(config);

        for event in events {
            let owned_before = machine.owns_virtual_addresses();
            let actions = machine.handle(event);

            for action in &actions {
                if let Action::SendAdvertisement { priority } = action {
                    // The confirmation and the first advertisement are emitted
                    // in the same batch, so ownership is checked after the batch
                    // rather than before it: either it was already held, or it
                    // was confirmed by the very event that asked for it.
                    prop_assert!(
                        owned_before || machine.owns_virtual_addresses(),
                        "an advertisement was emitted without confirmed ownership"
                    );
                    if *priority == 0 {
                        // I-27: zero is only ever a relinquish, and a
                        // relinquishing node stops advertising at once.
                        prop_assert!(
                            !machine.is_armed_for_advertising(),
                            "a zero-priority advertisement must stop normal advertising"
                        );
                    }
                }
            }
        }
    }
}

fn run_once(config: &InstanceConfig, events: &[Event]) -> Vec<Action> {
    let mut machine = machine_with(config.clone());
    let mut actions = Vec::new();
    for event in events {
        actions.extend(machine.handle(event.clone()));
    }
    actions
}

// ----- timer properties --------------------------------------------------

proptest! {
    /// A timer is due exactly at or after its deadline, and never before.
    #[test]
    fn a_timer_is_due_exactly_at_its_deadline(
        deadline in 0u64..10_000,
        elapsed in 0u64..10_000,
    ) {
        let mut timers = TimerSet::new();
        timers.arm(TimerId::MasterDown, Duration::from_millis(deadline));

        let now = Duration::from_millis(elapsed);
        prop_assert_eq!(
            timers.is_due(TimerId::MasterDown, now),
            now >= Duration::from_millis(deadline)
        );
    }

    /// Backoff is monotone, bounded by the cap, and stops at the limit.
    #[test]
    fn backoff_is_bounded_and_monotone(policy in arb_retry_policy(), attempt in 1u32..40) {
        let Some(delay) = policy.delay_for(attempt) else {
            prop_assert!(attempt > policy.max_attempts);
            return Ok(());
        };
        prop_assert!(delay <= policy.cap);
        if attempt > 1 {
            if let Some(previous) = policy.delay_for(attempt - 1) {
                prop_assert!(delay >= previous, "backoff must not decrease");
            }
        }
    }
}

fn arb_retry_policy() -> impl Strategy<Value = RetryPolicy> {
    (1u64..5, 1u64..60, 1u32..12).prop_map(|(base, cap, max_attempts)| RetryPolicy {
        base: Duration::from_secs(base),
        cap: Duration::from_secs(cap),
        max_attempts,
    })
}

// ----- health properties -------------------------------------------------

proptest! {
    /// The effective priority never rises above the configured priority, and a
    /// node is ineligible exactly when its effective priority is zero.
    #[test]
    fn effective_priority_respects_both_bounds(
        configured in 1u8..=255,
        minimum in 0u8..=255,
        penalty in 0u16..=600,
    ) {
        let config = HealthPolicyConfig {
            policy: HealthPolicy::Weighted,
            minimum_effective_priority: minimum,
            ..HealthPolicyConfig::default()
        };
        let health = HealthSummary { penalty, ..HealthSummary::default() };
        let state = evaluate(configured, &config, &health);

        prop_assert!(state.effective <= configured, "I-24");
        prop_assert_eq!(state.eligible, state.effective > 0, "I-23");
        prop_assert!(state.effective >= minimum.min(configured));
    }

    /// Manual health never changes anything, whatever the checks report.
    #[test]
    fn manual_health_is_inert(configured in 1u8..=255, penalty in 0u16..=600, failures in 0usize..8) {
        let config = HealthPolicyConfig { policy: HealthPolicy::Manual, ..HealthPolicyConfig::default() };
        let health = HealthSummary {
            penalty,
            electoral_failures: failures,
            total_failures: failures,
            passing: 0,
            stale_discarded: 0,
        };
        let state = evaluate(configured, &config, &health);
        prop_assert_eq!(state.effective, configured);
        prop_assert!(state.eligible);
    }

    /// `fail_closed` is blocked by exactly the failures it is defined to react to.
    #[test]
    fn fail_closed_blocks_on_the_configured_failures(
        electoral in 0usize..4,
        total in 0usize..4,
        all_required in any::<bool>(),
    ) {
        let config = HealthPolicyConfig {
            policy: HealthPolicy::FailClosed,
            all_checks_required: all_required,
            ..HealthPolicyConfig::default()
        };
        let health = HealthSummary {
            penalty: 0,
            electoral_failures: electoral,
            total_failures: total.max(electoral),
            passing: 0,
            stale_discarded: 0,
        };
        let state = evaluate(150, &config, &health);
        let expected = if all_required { total.max(electoral) } else { electoral };
        prop_assert_eq!(state.eligible, expected == 0);
    }
}

// ----- election properties -----------------------------------------------

proptest! {
    /// Two candidates that can be told apart always resolve antisymmetrically:
    /// swapping the arguments swaps the winner, so exactly one node is chosen.
    #[test]
    fn a_decidable_election_chooses_exactly_one_node(
        local in arb_candidate(),
        remote in arb_candidate(),
    ) {
        let forward = decide(&local, &remote, Incumbent::None);
        let reverse = decide(&remote, &local, Incumbent::None);

        if local.is_distinguishable_from(&remote) {
            prop_assert_ne!(forward.winner, reverse.winner, "R-15: exactly one candidate wins");
        } else {
            // Two candidates that agree on priority and address cannot both
            // exist on one segment, so the answer is "the operator decides".
            prop_assert_eq!(forward.reason, ElectionReason::Undecidable);
            prop_assert_eq!(reverse.reason, ElectionReason::Undecidable);
        }
    }

    /// Priority and address outrank the incumbent, and the incumbent decides
    /// only what those two cannot. This is the order in `SPEC.md` 12.3.
    #[test]
    fn the_incumbent_decides_only_what_priority_and_address_cannot(
        local in arb_candidate(),
        remote in arb_candidate(),
    ) {
        let with_local_incumbent = decide(&local, &remote, Incumbent::Local);
        let with_remote_incumbent = decide(&local, &remote, Incumbent::Remote);
        let with_none = decide(&local, &remote, Incumbent::None);

        if local.priority != remote.priority {
            let expected = if local.priority > remote.priority { Winner::Local } else { Winner::Remote };
            prop_assert_eq!(with_local_incumbent.winner, expected);
            prop_assert_eq!(with_remote_incumbent.winner, expected);
            prop_assert_eq!(with_none.winner, expected);
        } else if local.address != remote.address {
            let expected = if local.address > remote.address { Winner::Local } else { Winner::Remote };
            prop_assert_eq!(with_local_incumbent.winner, expected);
            prop_assert_eq!(with_remote_incumbent.winner, expected);
        } else {
            prop_assert_eq!(with_local_incumbent.winner, Winner::Local);
            prop_assert_eq!(with_remote_incumbent.winner, Winner::Remote);
        }
    }

    /// A higher priority always wins, whatever the addresses and incumbent are.
    #[test]
    fn priority_dominates_every_other_signal(
        high in arb_candidate(),
        low in arb_candidate(),
        incumbent in prop_oneof![Just(Incumbent::Local), Just(Incumbent::Remote), Just(Incumbent::None)],
    ) {
        let high = Candidate { priority: 200, ..high };
        let low = Candidate { priority: 100, ..low };

        let decision = decide(&high, &low, incumbent);
        prop_assert_eq!(decision.winner, Winner::Local);
        prop_assert_eq!(decision.reason, ElectionReason::HigherPriority);
    }

    /// A master steps down for exactly the peers that would preempt it.
    ///
    /// `must_step_down(a, b)` asks "does a master at `a` step down for a peer at
    /// `b`", and `may_preempt(b, a, true)` asks "would a backup at `b` preempt a
    /// master at `a`". They must be the same relation, or a pair of nodes could
    /// each expect the other to yield.
    #[test]
    fn step_down_and_preempt_describe_the_same_relation(
        master in 1u8..=255,
        peer in 1u8..=255,
    ) {
        prop_assert_eq!(must_step_down(master, peer), may_preempt(peer, master, true));
    }
}

fn arb_candidate() -> impl Strategy<Value = Candidate> {
    (arb_priority(), arb_source()).prop_map(|(priority, address)| Candidate::new(priority, address))
}

// ----- timer arithmetic properties ---------------------------------------

proptest! {
    /// The takeover budget is exactly three intervals plus the skew allowance.
    #[test]
    fn the_takeover_budget_follows_the_formula(centis in 1u64..=255) {
        let interval = Duration::from_millis(centis * 10);
        let expected = interval * 3 + Duration::from_millis(10);
        prop_assert_eq!(master_down_interval(&interval).unwrap(), expected);
    }

    /// The budget grows with the advertisement interval.
    #[test]
    fn a_shorter_interval_gives_a_shorter_budget(shorter in 1u64..200, longer in 200u64..255) {
        let small = master_down_interval(&Duration::from_millis(shorter * 10)).unwrap();
        let large = master_down_interval(&Duration::from_millis(longer * 10)).unwrap();
        prop_assert!(small < large);
    }
}
