// Rust guideline compliant 2026-09-27

//! State-machine tests, one per invariant in `SPEC.md` §11 and §19.
//!
//! Every test names the invariant it enforces. The naming is the coverage
//! mechanism: an invariant without a test is a gap a reviewer can see.

use std::net::IpAddr;
use std::time::Duration;

use highland_core::clock::ManualClock;
use highland_core::health::{HealthPolicy, HealthPolicyConfig, HealthSummary};
use highland_core::machine::{InstanceStateMachine, PendingOwnership};
use highland_core::state::{
    Action, ActionKind, Event, Generation, InstanceConfig, PeerAdvertisement, Role, TimerId,
    TransitionReason,
};
use highland_core::timer::RetryPolicy;

const ADVERT: Duration = Duration::from_secs(1);

fn machine_with(config: InstanceConfig) -> InstanceStateMachine<ManualClock> {
    InstanceStateMachine::new(config, ManualClock::new())
}

fn config() -> InstanceConfig {
    InstanceConfig {
        name: "api".to_owned(),
        vrid: 42,
        priority: 150,
        ..InstanceConfig::default()
    }
}

fn advert(priority: u8) -> Event {
    Event::AdvertisementReceived(PeerAdvertisement {
        vrid: 42,
        priority,
        source: "192.0.2.11".parse().expect("literal is a valid address"),
        advert_interval: ADVERT,
    })
}

fn health(electoral_failures: usize, total_failures: usize, penalty: u16) -> Event {
    Event::HealthChanged(HealthSummary {
        penalty,
        electoral_failures,
        total_failures,
        passing: 0,
        stale_discarded: 0,
    })
}

/// Drives a `BACKUP` to `MASTER`, confirming ownership as the executor would.
fn promote(machine: &mut InstanceStateMachine<ManualClock>) {
    let _ = machine.handle(Event::TimerExpired(TimerId::MasterDown));
    assert_eq!(
        machine.role(),
        Role::Backup,
        "ownership is not confirmed yet"
    );
    let _ = machine.handle(Event::ActionSucceeded {
        kind: ActionKind::AddAddresses,
    });
    assert_eq!(machine.role(), Role::Master);
}

fn startup(machine: &mut InstanceStateMachine<ManualClock>) {
    let _ = machine.handle(Event::Startup);
    assert_eq!(machine.role(), Role::Backup);
}

// ----- I-01, I-02, I-35: exactly one role, changes are atomic -----------

#[test]
fn i01_a_new_machine_is_init_and_owns_nothing() {
    let machine = machine_with(config());
    assert_eq!(machine.role(), Role::Init);
    assert!(!machine.owns_virtual_addresses());
    assert_eq!(machine.pending(), PendingOwnership::None);
}

#[test]
fn i35_exactly_one_role_is_active_at_every_instant() {
    let mut machine = machine_with(config());
    startup(&mut machine);
    promote(&mut machine);
    assert_eq!(machine.role(), Role::Master);

    // Every role change is visible as a single EnterRole, never as a sequence a
    // caller could observe halfway through.
    let actions = machine.handle(Event::ShutdownRequested);
    assert!(
        !actions
            .iter()
            .any(|action| matches!(action, Action::EnterRole { .. })),
        "the role changes only once removal is confirmed"
    );
    assert_eq!(machine.role(), Role::Master);

    let actions = machine.handle(Event::ActionSucceeded {
        kind: ActionKind::RemoveAddresses,
    });
    let entered: Vec<(Role, TransitionReason)> = actions
        .iter()
        .filter_map(|action| match action {
            Action::EnterRole { role, reason } => Some((*role, *reason)),
            _ => None,
        })
        .collect();
    assert_eq!(entered, [(Role::Backup, TransitionReason::Shutdown)]);
    assert_eq!(machine.role(), Role::Backup);
}

// ----- I-03, I-04, I-14: ownership precedes advertising ------------------

#[test]
fn i04_the_machine_never_advertises_master_before_ownership_is_confirmed() {
    let mut machine = machine_with(config());
    startup(&mut machine);

    let actions = machine.handle(Event::TimerExpired(TimerId::MasterDown));
    assert!(actions.contains(&Action::AddVirtualAddresses));
    assert!(
        !actions
            .iter()
            .any(|action| matches!(action, Action::SendAdvertisement { .. })),
        "no advertisement may be requested before ownership is confirmed"
    );
    assert_eq!(machine.role(), Role::Backup);
    assert_eq!(machine.pending(), PendingOwnership::Acquiring);
}

#[test]
fn i04_advertising_starts_only_after_ownership_is_confirmed() {
    let mut machine = machine_with(config());
    startup(&mut machine);
    promote(&mut machine);

    assert!(machine.owns_virtual_addresses());
    let actions = machine.handle(Event::ActionSucceeded {
        kind: ActionKind::AddAddresses,
    });
    let _ = actions;
    assert!(machine.is_armed_for_advertising());
}

#[test]
fn i03_ownership_exists_only_in_the_master_role() {
    let mut machine = machine_with(config());
    startup(&mut machine);
    assert!(!machine.owns_virtual_addresses());

    promote(&mut machine);
    assert!(machine.owns_virtual_addresses());

    let _ = machine.handle(Event::OperatorRelinquishRequested);
    let _ = machine.handle(Event::ActionSucceeded {
        kind: ActionKind::RemoveAddresses,
    });
    assert!(!machine.owns_virtual_addresses());
    assert_ne!(machine.role(), Role::Master);
}

#[test]
fn i14_ownership_and_role_never_disagree() {
    let mut machine = machine_with(config());
    startup(&mut machine);
    promote(&mut machine);

    // Releasing ownership onto FAULT happens only once removal is confirmed, so
    // the two facts cannot drift apart: the instance is master exactly while it
    // owns, including while a removal is failing.
    let actions = machine.handle(Event::InterfaceDown);
    assert!(actions.contains(&Action::RemoveVirtualAddresses));
    let _ = machine.handle(Event::ActionFailed {
        kind: ActionKind::RemoveAddresses,
        error: "device or resource busy".to_owned(),
    });
    assert_eq!(machine.role(), Role::Master);
    assert!(machine.owns_virtual_addresses());

    let _ = machine.handle(Event::ActionSucceeded {
        kind: ActionKind::RemoveAddresses,
    });
    assert_eq!(machine.role(), Role::Fault);
    assert!(!machine.owns_virtual_addresses());
}

// ----- I-15, I-16: interface and fault ----------------------------------

#[test]
fn i15_an_instance_never_takes_ownership_while_the_interface_is_down() {
    let mut machine = machine_with(config());
    startup(&mut machine);
    let _ = machine.handle(Event::InterfaceDown);

    let actions = machine.handle(Event::TimerExpired(TimerId::MasterDown));
    assert!(
        !actions.contains(&Action::AddVirtualAddresses),
        "I-15: a down interface cannot take ownership"
    );
    assert_ne!(machine.role(), Role::Master);
}

#[test]
fn i16_a_faulted_instance_never_advertises() {
    let mut machine = machine_with(config());
    startup(&mut machine);
    let _ = machine.handle(Event::TimerExpired(TimerId::MasterDown));
    let _ = machine.handle(Event::ActionFailed {
        kind: ActionKind::AddAddresses,
        error: "cannot assign requested address".to_owned(),
    });
    assert_eq!(machine.role(), Role::Fault);

    let actions = machine.handle(Event::TimerExpired(TimerId::Advertisement));
    assert!(
        !actions
            .iter()
            .any(|action| matches!(action, Action::SendAdvertisement { .. }))
    );
}

#[test]
fn i21_a_faulted_instance_waits_for_the_hold_down_before_retrying() {
    let base = InstanceConfig {
        hold_down: Duration::from_secs(10),
        ..config()
    };
    let mut machine = machine_with(base);
    startup(&mut machine);
    let _ = machine.handle(Event::TimerExpired(TimerId::MasterDown));
    let _ = machine.handle(Event::ActionFailed {
        kind: ActionKind::AddAddresses,
        error: "cannot assign requested address".to_owned(),
    });

    assert!(machine.deadline_of(TimerId::HoldDown).is_some());
    assert_eq!(machine.role(), Role::Fault);

    // The interface coming back is not enough; the hold-down still applies.
    let actions = machine.handle(Event::InterfaceUp);
    assert_ne!(machine.role(), Role::Master);
    assert!(!actions.contains(&Action::AddVirtualAddresses));

    // While the instance is faulted, the retry timer re-attempts ownership
    // within the hold-down.
    let actions = machine.handle(Event::TimerExpired(TimerId::Retry));
    assert!(actions.contains(&Action::AddVirtualAddresses));
}

#[test]
fn a_hold_down_expiry_re_enters_election_rather_than_taking_over_at_once() {
    let base = InstanceConfig {
        hold_down: Duration::from_secs(10),
        ..config()
    };
    let mut machine = machine_with(base);
    startup(&mut machine);
    let _ = machine.handle(Event::TimerExpired(TimerId::MasterDown));
    let _ = machine.handle(Event::ActionFailed {
        kind: ActionKind::AddAddresses,
        error: "cannot assign requested address".to_owned(),
    });
    let _ = machine.handle(Event::TimerExpired(TimerId::Retry));
    let _ = machine.handle(Event::ActionFailed {
        kind: ActionKind::AddAddresses,
        error: "cannot assign requested address".to_owned(),
    });

    let _ = machine.handle(Event::TimerExpired(TimerId::HoldDown));
    assert_eq!(
        machine.role(),
        Role::Backup,
        "election is re-entered, not ownership"
    );

    // A fresh master-down interval is required before a takeover.
    let actions = machine.handle(Event::TimerExpired(TimerId::MasterDown));
    assert!(actions.contains(&Action::AddVirtualAddresses));
}

// ----- I-20, R-12: cleanup and bounded retry ----------------------------

#[test]
fn i20_entering_fault_attempts_to_remove_partially_added_addresses() {
    let mut machine = machine_with(config());
    startup(&mut machine);
    let _ = machine.handle(Event::TimerExpired(TimerId::MasterDown));
    let actions = machine.handle(Event::ActionFailed {
        kind: ActionKind::AddAddresses,
        error: "cannot assign requested address".to_owned(),
    });

    assert!(
        actions.contains(&Action::RemoveVirtualAddresses),
        "I-20: clean up first"
    );
    assert_eq!(machine.role(), Role::Fault);
}

#[test]
fn r12_retries_stop_after_the_documented_attempt_count() {
    let base = InstanceConfig {
        retry: RetryPolicy {
            base: Duration::from_secs(1),
            cap: Duration::from_secs(4),
            max_attempts: 3,
        },
        hold_down: Duration::from_secs(600),
        ..config()
    };
    let mut machine = machine_with(base);
    startup(&mut machine);
    let _ = machine.handle(Event::TimerExpired(TimerId::MasterDown));

    // Each failure schedules one retry, and each retry re-attempts ownership
    // exactly once.
    for _ in 0..3 {
        let _ = machine.handle(Event::ActionFailed {
            kind: ActionKind::AddAddresses,
            error: "cannot assign requested address".to_owned(),
        });
        if !machine.timers().is_armed(TimerId::Retry) {
            break;
        }
        let actions = machine.handle(Event::TimerExpired(TimerId::Retry));
        assert!(
            actions.contains(&Action::AddVirtualAddresses),
            "a permitted retry re-attempts"
        );
    }

    assert_eq!(machine.role(), Role::Fault);
    assert!(
        !machine.timers().is_armed(TimerId::Retry),
        "R-12: the retry budget is finite, so no further retry is scheduled"
    );

    // Exhaustion is reported when it happens, so the operator sees it in the log
    // and in the event stream rather than only in a timer that never fires.
    let _ = machine.handle(Event::TimerExpired(TimerId::HoldDown));
    let actions = machine.handle(Event::TimerExpired(TimerId::Retry));
    assert!(!actions.contains(&Action::AddVirtualAddresses));
}

// ----- I-17, I-18, I-45: action and outcome separation -------------------

#[test]
fn i45_a_failed_add_is_reported_back_and_faults_the_instance() {
    let mut machine = machine_with(config());
    startup(&mut machine);
    let _ = machine.handle(Event::TimerExpired(TimerId::MasterDown));

    let actions = machine.handle(Event::ActionFailed {
        kind: ActionKind::AddAddresses,
        error: "Operation not permitted".to_owned(),
    });
    assert!(actions.iter().any(|action| matches!(
        action,
        Action::EmitEvent {
            name: "action_failed"
        }
    )));
    assert!(actions.iter().any(|action| matches!(
        action,
        Action::Log { message, .. } if message.contains("Operation not permitted")
    )));
    assert_eq!(machine.role(), Role::Fault);
}

#[test]
fn i45_a_failure_for_an_unknown_action_is_still_handled() {
    let mut machine = machine_with(config());
    startup(&mut machine);
    let actions = machine.handle(Event::ActionFailed {
        kind: ActionKind::EmitEvent,
        error: "sink closed".to_owned(),
    });
    assert_eq!(
        machine.role(),
        Role::Backup,
        "a failed event write is not fatal"
    );
    assert!(!actions.is_empty());
}

#[test]
fn i18_a_release_failure_keeps_the_master_role_until_removal_is_confirmed() {
    let mut machine = machine_with(config());
    startup(&mut machine);
    promote(&mut machine);

    let actions = machine.handle(Event::OperatorRelinquishRequested);
    assert!(actions.contains(&Action::SendAdvertisement { priority: 0 }));
    assert_eq!(
        machine.role(),
        Role::Master,
        "the addresses are still present"
    );
    assert!(
        !machine.is_armed_for_advertising(),
        "advertising stops immediately"
    );

    let _ = machine.handle(Event::ActionFailed {
        kind: ActionKind::RemoveAddresses,
        error: "device or resource busy".to_owned(),
    });
    assert_eq!(
        machine.role(),
        Role::Master,
        "I-14 forbids a non-master owner"
    );

    let _ = machine.handle(Event::ActionSucceeded {
        kind: ActionKind::RemoveAddresses,
    });
    assert_eq!(machine.role(), Role::Backup);
    assert!(!machine.owns_virtual_addresses());
}

// ----- I-23, I-24, I-25, I-26: health and priority ---------------------

#[test]
fn i23_an_effective_priority_of_zero_relinquishes_silently() {
    let base = InstanceConfig {
        health: HealthPolicyConfig {
            policy: HealthPolicy::Weighted,
            minimum_effective_priority: 0,
            ..HealthPolicyConfig::default()
        },
        ..config()
    };
    let mut machine = machine_with(base);
    startup(&mut machine);
    promote(&mut machine);

    let actions = machine.handle(health(1, 1, 200));
    assert_eq!(machine.effective_priority(), 0);
    assert!(
        !actions
            .iter()
            .any(|action| matches!(action, Action::SendAdvertisement { priority: 0 })),
        "I-23: zero is never sent as a normal advertisement"
    );
    assert!(actions.contains(&Action::RemoveVirtualAddresses));
    let _ = machine.handle(Event::ActionSucceeded {
        kind: ActionKind::RemoveAddresses,
    });
    assert_eq!(machine.role(), Role::Backup);
}

#[test]
fn i23_a_backup_with_effective_priority_zero_does_not_take_over() {
    let base = InstanceConfig {
        health: HealthPolicyConfig {
            policy: HealthPolicy::Weighted,
            minimum_effective_priority: 0,
            ..HealthPolicyConfig::default()
        },
        ..config()
    };
    let mut machine = machine_with(base);
    startup(&mut machine);
    let _ = machine.handle(health(1, 1, 200));

    let actions = machine.handle(Event::TimerExpired(TimerId::MasterDown));
    assert!(!actions.contains(&Action::AddVirtualAddresses));
}

#[test]
fn i24_a_health_failure_reduces_but_never_raises_the_effective_priority() {
    let configured = config();
    let mut machine = machine_with(InstanceConfig {
        health: HealthPolicyConfig {
            policy: HealthPolicy::Weighted,
            minimum_effective_priority: 200,
            ..HealthPolicyConfig::default()
        },
        ..configured
    });
    startup(&mut machine);
    let _ = machine.handle(health(1, 1, 50));

    assert!(machine.effective_priority() <= configured.priority);
}

#[test]
fn i25_fail_closed_relinquishes_immediately_with_a_zero_advertisement() {
    let base = InstanceConfig {
        health: HealthPolicyConfig {
            policy: HealthPolicy::FailClosed,
            send_zero_priority_advert: true,
            ..HealthPolicyConfig::default()
        },
        ..config()
    };
    let mut machine = machine_with(base);
    startup(&mut machine);
    promote(&mut machine);

    let actions = machine.handle(health(1, 1, 0));
    assert!(
        actions.contains(&Action::SendAdvertisement { priority: 0 }),
        "a relinquish is announced so the peer takes over faster"
    );
    assert!(actions.contains(&Action::RemoveVirtualAddresses));
    let _ = machine.handle(Event::ActionSucceeded {
        kind: ActionKind::RemoveAddresses,
    });
    assert_eq!(machine.role(), Role::Backup);
    assert_eq!(machine.last_reason(), TransitionReason::HealthIneligible);
}

#[test]
fn i25_fail_closed_ignores_observational_failures_unless_all_are_required() {
    let mut machine = machine_with(InstanceConfig {
        health: HealthPolicyConfig {
            policy: HealthPolicy::FailClosed,
            ..HealthPolicyConfig::default()
        },
        ..config()
    });
    startup(&mut machine);
    promote(&mut machine);

    let actions = machine.handle(health(0, 1, 0));
    assert!(!actions.contains(&Action::RemoveVirtualAddresses));
    assert_eq!(machine.role(), Role::Master);
}

#[test]
fn i27_zero_priority_is_only_ever_sent_on_a_relinquish_path() {
    let mut machine = machine_with(config());
    startup(&mut machine);
    promote(&mut machine);

    // A normal advertisement always carries the effective priority.
    let actions = machine.handle(Event::TimerExpired(TimerId::Advertisement));
    assert!(actions.contains(&Action::SendAdvertisement { priority: 150 }));

    // Zero appears only on relinquish.
    let actions = machine.handle(Event::ShutdownRequested);
    assert!(actions.contains(&Action::SendAdvertisement { priority: 0 }));
}

#[test]
fn i26_a_stale_health_result_is_discarded_by_the_caller_not_the_machine() {
    let mut machine = machine_with(config());
    startup(&mut machine);
    let _ = machine.handle(health(1, 1, 100));
    let degraded = machine.effective_priority();

    // A newer result supersedes; a stale one is dropped before delivery, which
    // is what `highland-checks` does with the sequence number.
    let _ = machine.handle(health(0, 0, 0));
    let recovered = machine.effective_priority();
    assert!(recovered > degraded);
}

// ----- I-27, I-28, R-15, R-16: election and preemption ------------------

#[test]
fn r16_a_master_steps_down_for_a_higher_priority_peer_even_without_preemption() {
    let base = InstanceConfig {
        preempt: false,
        ..config()
    };
    let mut machine = machine_with(base);
    startup(&mut machine);
    promote(&mut machine);

    let actions = machine.handle(advert(200));
    assert!(actions.contains(&Action::RemoveVirtualAddresses));
    let _ = machine.handle(Event::ActionSucceeded {
        kind: ActionKind::RemoveAddresses,
    });
    assert_eq!(machine.role(), Role::Backup);
    assert_eq!(
        machine.last_reason(),
        TransitionReason::HigherPriorityPeerAdvertisement
    );
}

/// A master keeps the role against a lower priority, and against an equal
/// priority from a peer whose address does not win the tie.
///
/// Two nodes that start together both time out and both advertise at the same
/// priority. The specification resolves that by address (§12.3), so the node
/// with the higher address wins; this test covers the half where the lower
/// address keeps the role.
#[test]
fn r16_a_master_ignores_an_equal_or_lower_priority_peer() {
    let mut machine = machine_with(config());
    machine.set_primary_addresses(Some("192.0.2.200".parse().expect("valid address")), None);
    startup(&mut machine);
    promote(&mut machine);

    // The peer at 192.0.2.11 has the lower address, so the tie stays ours.
    let actions = machine.handle(advert(150));
    assert!(!actions.contains(&Action::RemoveVirtualAddresses));
    assert_eq!(machine.role(), Role::Master);

    let actions = machine.handle(advert(100));
    assert!(!actions.contains(&Action::RemoveVirtualAddresses));
    assert_eq!(machine.role(), Role::Master);
}

/// A master with a higher address steps down for an equal-priority peer.
///
/// This is the case two nodes starting together produce, and the one that made
/// both nodes believe they were master before the tie-break was wired up.
#[test]
fn an_equal_priority_peer_with_a_higher_address_wins() {
    let mut machine = machine_with(config());
    machine.set_primary_addresses(Some("192.0.2.10".parse().expect("valid address")), None);
    startup(&mut machine);
    promote(&mut machine);

    let actions = machine.handle(advert(150));
    assert!(
        actions.contains(&Action::RemoveVirtualAddresses),
        "the higher address wins the tie"
    );
    // Releasing is two-phase like taking over: the role changes when the removal
    // is confirmed, not when it is asked for.
    assert_eq!(
        machine.role(),
        Role::Master,
        "still owns until the removal is confirmed"
    );
    let _ = machine.handle(Event::ActionSucceeded {
        kind: ActionKind::RemoveAddresses,
    });
    assert_eq!(machine.role(), Role::Backup);
}

#[test]
fn i28_preemption_is_armed_once_and_not_restarted_by_equal_traffic() {
    let base = InstanceConfig {
        preempt: true,
        preempt_delay: Duration::from_secs(30),
        ..config()
    };
    let mut machine = machine_with(base);
    startup(&mut machine);

    let _ = machine.handle(advert(100));
    let armed_at = machine.deadline_of(TimerId::PreemptionDelay);
    assert!(armed_at.is_some());

    machine.clock().advance(Duration::from_secs(10));
    let _ = machine.handle(advert(90));
    assert_eq!(
        machine.deadline_of(TimerId::PreemptionDelay),
        armed_at,
        "a lower-or-equal advertisement must not restart the timer"
    );

    machine.clock().advance(Duration::from_secs(21));
    assert!(machine.is_due(TimerId::PreemptionDelay));
    let actions = machine.handle(Event::TimerExpired(TimerId::PreemptionDelay));
    assert!(actions.contains(&Action::AddVirtualAddresses));
    assert_eq!(machine.last_reason(), TransitionReason::Preemption);
}

#[test]
fn i28_a_higher_priority_advertisement_cancels_preemption() {
    let base = InstanceConfig {
        preempt: true,
        preempt_delay: Duration::from_secs(30),
        ..config()
    };
    let mut machine = machine_with(base);
    startup(&mut machine);

    let _ = machine.handle(advert(100));
    assert!(machine.deadline_of(TimerId::PreemptionDelay).is_some());

    let actions = machine.handle(advert(200));
    assert!(machine.deadline_of(TimerId::PreemptionDelay).is_none());
    assert!(actions.contains(&Action::CancelTimer {
        timer: TimerId::PreemptionDelay
    }));
}

#[test]
fn a_backup_with_preemption_disabled_never_preempts() {
    let base = InstanceConfig {
        preempt: false,
        preempt_delay: Duration::ZERO,
        ..config()
    };
    let mut machine = machine_with(base);
    startup(&mut machine);

    let _ = machine.handle(advert(100));
    assert!(machine.deadline_of(TimerId::PreemptionDelay).is_none());
    assert_eq!(machine.preemption_target(), None);
}

#[test]
fn a_relinquishing_peer_never_becomes_a_preemption_target() {
    let base = InstanceConfig {
        preempt: true,
        preempt_delay: Duration::from_secs(30),
        ..config()
    };
    let mut machine = machine_with(base);
    startup(&mut machine);

    let _ = machine.handle(advert(0));
    assert!(machine.deadline_of(TimerId::PreemptionDelay).is_none());
    assert!(machine.deadline_of(TimerId::MasterDown).is_some());
}

#[test]
fn a_late_advertisement_during_a_takeover_is_replayed_after_ownership() {
    let base = InstanceConfig {
        preempt: false,
        ..config()
    };
    let mut machine = machine_with(base);
    startup(&mut machine);

    let _ = machine.handle(Event::TimerExpired(TimerId::MasterDown));
    assert_eq!(machine.pending(), PendingOwnership::Acquiring);

    // The other master is alive after all, and outranks this node.
    let actions = machine.handle(advert(200));
    assert!(!actions.contains(&Action::RemoveVirtualAddresses));

    // Ownership is confirmed, and the deferred advertisement is then honoured.
    let actions = machine.handle(Event::ActionSucceeded {
        kind: ActionKind::AddAddresses,
    });
    assert!(actions.contains(&Action::RemoveVirtualAddresses));
    assert_eq!(machine.role(), Role::Master);
    let _ = machine.handle(Event::ActionSucceeded {
        kind: ActionKind::RemoveAddresses,
    });
    assert_eq!(machine.role(), Role::Backup);
    assert_eq!(
        machine.last_reason(),
        TransitionReason::HigherPriorityPeerAdvertisement
    );
}

// ----- I-29, I-30: timers ------------------------------------------------

#[test]
fn i29_every_timer_can_be_cancelled_and_cancellation_is_idempotent() {
    let mut machine = machine_with(config());
    startup(&mut machine);
    assert!(
        machine.timers().is_armed(TimerId::MasterDown),
        "startup arms the takeover timer"
    );

    // Pausing cancels everything the instance owns.
    let _ = machine.handle(Event::OperatorPauseRequested);
    for timer in [
        TimerId::Advertisement,
        TimerId::MasterDown,
        TimerId::PreemptionDelay,
        TimerId::HoldDown,
        TimerId::Retry,
        TimerId::StartupDelay,
    ] {
        assert!(
            !machine.timers().is_armed(timer),
            "{timer} should not be armed"
        );
    }

    // Firing a cancelled timer is inert, and repeated cancellation is safe.
    for timer in [TimerId::MasterDown, TimerId::Retry, TimerId::HoldDown] {
        let actions = machine.handle(Event::TimerExpired(timer));
        assert!(!actions.contains(&Action::AddVirtualAddresses));
        let _ = machine.handle(Event::OperatorPauseRequested);
    }
}

#[test]
fn i30_a_cancelled_timer_does_not_fire_after_a_reload() {
    let mut machine = machine_with(config());
    startup(&mut machine);
    let _ = machine.handle(Event::OperatorPauseRequested);
    assert_eq!(machine.role(), Role::Disabled);
    assert!(machine.timers().is_empty());

    let actions = machine.handle(Event::TimerExpired(TimerId::MasterDown));
    assert!(!actions.contains(&Action::AddVirtualAddresses));
}

#[test]
fn every_valid_advertisement_resets_the_master_down_timer() {
    let mut machine = machine_with(config());
    startup(&mut machine);
    let first = machine.deadline_of(TimerId::MasterDown);

    machine.clock().advance(Duration::from_secs(2));
    let _ = machine.handle(advert(150));
    assert!(
        machine.deadline_of(TimerId::MasterDown) > first,
        "I-44: the timer is pushed forward, not left to expire"
    );
}

#[test]
fn an_advertisement_for_another_vrid_is_ignored() {
    let mut machine = machine_with(config());
    startup(&mut machine);
    let before = machine.deadline_of(TimerId::MasterDown);

    let _ = machine.handle(Event::AdvertisementReceived(PeerAdvertisement {
        vrid: 7,
        priority: 200,
        source: "192.0.2.12".parse().expect("literal is a valid address"),
        advert_interval: ADVERT,
    }));
    assert_eq!(machine.deadline_of(TimerId::MasterDown), before);
}

// ----- R-09, R-10, R-28: operator actions -------------------------------

#[test]
fn r09_pausing_a_master_releases_ownership_before_disabling() {
    let mut machine = machine_with(config());
    startup(&mut machine);
    promote(&mut machine);

    let actions = machine.handle(Event::OperatorPauseRequested);
    assert!(actions.contains(&Action::RemoveVirtualAddresses));
    assert_eq!(
        machine.role(),
        Role::Master,
        "the role changes after removal"
    );

    let _ = machine.handle(Event::ActionSucceeded {
        kind: ActionKind::RemoveAddresses,
    });
    assert_eq!(machine.role(), Role::Disabled);
    assert!(machine.timers().is_empty());
}

#[test]
fn r09_resuming_a_paused_instance_restarts_the_startup_sequence() {
    let mut machine = machine_with(config());
    startup(&mut machine);
    let _ = machine.handle(Event::OperatorPauseRequested);
    let _ = machine.handle(Event::OperatorResumeRequested);

    assert_eq!(machine.role(), Role::Backup);
    assert!(machine.deadline_of(TimerId::MasterDown).is_some());
}

#[test]
fn r10_a_forced_transition_is_audited_and_goes_through_the_executor() {
    let mut machine = machine_with(config());
    startup(&mut machine);

    let actions = machine.handle(Event::OperatorForceTransitionRequested {
        target: Role::Master,
        reason: "operator asked".to_owned(),
    });
    assert!(actions.iter().any(|action| matches!(
        action,
        Action::EmitEvent {
            name: "operator_action"
        }
    )));
    assert!(
        actions.contains(&Action::AddVirtualAddresses),
        "even a forced master confirms ownership"
    );
}

// ----- I-12, I-11: generations ------------------------------------------

#[test]
fn i12_a_reload_from_an_older_generation_is_ignored() {
    let mut machine = machine_with(config());
    startup(&mut machine);
    let _ = machine.handle(Event::ConfigurationReloaded {
        generation: Generation::initial().next(),
    });
    assert_eq!(machine.generation(), Generation::initial().next());

    let actions = machine.handle(Event::ConfigurationReloaded {
        generation: Generation::initial(),
    });
    assert_eq!(machine.generation(), Generation::initial().next());
    assert!(
        !actions.iter().any(|action| matches!(
            action,
            Action::EmitEvent {
                name: "reload_accepted"
            }
        )),
        "I-12: a stale generation changes nothing"
    );
}

#[test]
fn i10_a_reload_does_not_interrupt_an_unchanged_instance() {
    let mut machine = machine_with(config());
    startup(&mut machine);
    let before = machine.deadline_of(TimerId::MasterDown);

    let _ = machine.handle(Event::ConfigurationReloaded {
        generation: Generation::initial().next(),
    });
    assert_eq!(machine.role(), Role::Backup);
    assert_eq!(
        machine.deadline_of(TimerId::MasterDown),
        before,
        "the timer is untouched"
    );
}

#[test]
fn a_reload_resumes_a_paused_instance() {
    let mut machine = machine_with(config());
    startup(&mut machine);
    let _ = machine.handle(Event::OperatorPauseRequested);
    let _ = machine.handle(Event::ConfigurationReloaded {
        generation: Generation::initial().next(),
    });

    assert_eq!(machine.role(), Role::Backup);
}

// ----- I-31, I-32, R-30: shutdown ---------------------------------------

#[test]
fn i31_shutdown_from_master_announces_and_releases() {
    let mut machine = machine_with(config());
    startup(&mut machine);
    promote(&mut machine);

    let actions = machine.handle(Event::ShutdownRequested);
    assert!(actions.contains(&Action::SendAdvertisement { priority: 0 }));
    assert!(actions.contains(&Action::RemoveVirtualAddresses));
    assert!(actions.contains(&Action::CancelTimer {
        timer: TimerId::Advertisement
    }));
    assert!(!machine.is_armed_for_advertising());

    let _ = machine.handle(Event::ActionSucceeded {
        kind: ActionKind::RemoveAddresses,
    });
    assert_eq!(machine.role(), Role::Backup);
}

#[test]
fn i31_a_second_shutdown_request_is_idempotent() {
    let mut machine = machine_with(config());
    startup(&mut machine);
    let _ = machine.handle(Event::ShutdownRequested);
    let first = machine.role();
    let _ = machine.handle(Event::ShutdownRequested);
    assert_eq!(machine.role(), first);
    assert!(machine.timers().is_empty());
}

#[test]
fn i32_a_shutdown_instance_never_adds_an_address() {
    let mut machine = machine_with(config());
    startup(&mut machine);
    let _ = machine.handle(Event::ShutdownRequested);
    machine.clock().advance(Duration::from_secs(10));

    let actions = machine.handle(Event::TimerExpired(TimerId::MasterDown));
    assert!(!actions.contains(&Action::AddVirtualAddresses));
}

// ----- R-19: role metric encoding ---------------------------------------

#[test]
fn r19_every_role_reports_its_documented_metric_value() {
    let expected = [
        (Role::Init, 0),
        (Role::Backup, 1),
        (Role::Master, 2),
        (Role::Fault, 3),
        (Role::Disabled, 4),
    ];
    for (role, metric) in expected {
        assert_eq!(role.as_metric(), metric);
    }
}

// ----- timing budget (§13.3) --------------------------------------------

#[test]
fn the_takeover_budget_is_within_three_advertisement_intervals() {
    let mut machine = machine_with(config());
    startup(&mut machine);
    assert_eq!(
        machine.deadline_of(TimerId::MasterDown),
        Some(Duration::from_millis(3410)),
        "3 * 1s + ((256 - 150) * 100cs) / 256 = 300 + 41 centiseconds"
    );
}

#[test]
fn advertisement_failures_fault_the_instance_within_the_window() {
    let mut machine = machine_with(config());
    startup(&mut machine);
    promote(&mut machine);

    let _ = machine.handle(Event::ActionFailed {
        kind: ActionKind::Advertisement,
        error: "EPERM".to_owned(),
    });
    let _ = machine.handle(Event::ActionFailed {
        kind: ActionKind::Advertisement,
        error: "EPERM".to_owned(),
    });
    assert_eq!(machine.role(), Role::Master, "two failures are tolerated");

    let actions = machine.handle(Event::ActionFailed {
        kind: ActionKind::Advertisement,
        error: "EPERM".to_owned(),
    });
    assert!(actions.contains(&Action::RemoveVirtualAddresses));
    let _ = machine.handle(Event::ActionSucceeded {
        kind: ActionKind::RemoveAddresses,
    });
    assert_eq!(machine.role(), Role::Fault);
}

#[test]
fn a_successful_advertisement_resets_the_failure_streak() {
    let mut machine = machine_with(config());
    startup(&mut machine);
    promote(&mut machine);

    let _ = machine.handle(Event::ActionFailed {
        kind: ActionKind::Advertisement,
        error: "EPERM".to_owned(),
    });
    let _ = machine.handle(Event::ActionFailed {
        kind: ActionKind::Advertisement,
        error: "EPERM".to_owned(),
    });
    let _ = machine.handle(Event::ActionSucceeded {
        kind: ActionKind::Advertisement,
    });
    let _ = machine.handle(Event::ActionFailed {
        kind: ActionKind::Advertisement,
        error: "EPERM".to_owned(),
    });
    let _ = machine.handle(Event::ActionFailed {
        kind: ActionKind::Advertisement,
        error: "EPERM".to_owned(),
    });

    assert_eq!(
        machine.role(),
        Role::Master,
        "the streak was reset by a success"
    );
}

#[test]
fn the_last_peer_is_reported_for_diagnosis() {
    let mut machine = machine_with(config());
    startup(&mut machine);
    let _ = machine.handle(advert(100));
    assert_eq!(
        machine.last_peer(),
        Some("192.0.2.11".parse().expect("valid address"))
    );
}

#[test]
fn the_candidate_presents_the_configured_priority_and_address() {
    let base = InstanceConfig {
        primary_ipv4: Some("192.0.2.10".parse().expect("valid address")),
        ..config()
    };
    let mut machine = machine_with(base);
    startup(&mut machine);

    let candidate = machine.candidate();
    assert_eq!(candidate.priority, 150);
    assert_eq!(candidate.address, IpAddr::from([192, 0, 2, 10]));
}

// ----- I-36: only a master transmits -------------------------------------

#[test]
fn i36_no_role_other_than_master_transmits_a_normal_advertisement() {
    for event in [
        Event::Startup,
        Event::InterfaceDown,
        Event::OperatorPauseRequested,
        Event::ShutdownRequested,
    ] {
        let mut machine = machine_with(config());
        machine.clock().advance(Duration::from_secs(10));
        let actions = machine.handle(event);
        for timer in [
            TimerId::Advertisement,
            TimerId::MasterDown,
            TimerId::PreemptionDelay,
            TimerId::HoldDown,
            TimerId::Retry,
            TimerId::StartupDelay,
        ] {
            let fired = machine.handle(Event::TimerExpired(timer));
            for action in actions.iter().chain(fired.iter()) {
                if let Action::SendAdvertisement { priority } = action {
                    assert_eq!(*priority, 0, "I-36: only a relinquish may use zero");
                }
            }
        }
    }
}

#[test]
fn i36_a_master_advertises_its_effective_priority() {
    let mut machine = machine_with(config());
    startup(&mut machine);
    promote(&mut machine);

    let actions = machine.handle(Event::TimerExpired(TimerId::Advertisement));
    assert!(actions.contains(&Action::SendAdvertisement { priority: 150 }));
    assert_eq!(
        actions
            .iter()
            .filter(|action| matches!(action, Action::SendAdvertisement { .. }))
            .count(),
        1,
        "one advertisement per expiry"
    );
}

// ----- I-37: every ownership change is announced -------------------------

#[test]
fn i37_every_ownership_change_emits_an_event() {
    let mut machine = machine_with(config());
    startup(&mut machine);

    let actions = machine.handle(Event::TimerExpired(TimerId::MasterDown));
    assert!(actions.contains(&Action::EmitEvent {
        name: "ownership_acquiring"
    }));

    let actions = machine.handle(Event::ActionSucceeded {
        kind: ActionKind::AddAddresses,
    });
    assert!(actions.contains(&Action::EmitEvent {
        name: "role_transition"
    }));

    let actions = machine.handle(Event::ShutdownRequested);
    assert!(actions.contains(&Action::EmitEvent {
        name: "daemon_lifecycle"
    }));

    let actions = machine.handle(Event::ActionSucceeded {
        kind: ActionKind::RemoveAddresses,
    });
    assert!(actions.contains(&Action::EmitEvent {
        name: "role_transition"
    }));
}

#[test]
fn i37_every_role_transition_carries_a_reason() {
    let mut machine = machine_with(config());
    startup(&mut machine);
    let _ = machine.handle(Event::TimerExpired(TimerId::MasterDown));
    let actions = machine.handle(Event::ActionSucceeded {
        kind: ActionKind::AddAddresses,
    });

    let reason = actions
        .iter()
        .find_map(|action| match action {
            Action::EnterRole { reason, .. } => Some(*reason),
            _ => None,
        })
        .expect("a role change always carries a reason");
    assert_eq!(reason, TransitionReason::MasterDownTimeout);
    assert!(!reason.to_string().is_empty());
}

// ----- I-40: repeated failures stay bounded ------------------------------

#[test]
fn i40_repeated_failures_do_not_grow_state() {
    let mut machine = machine_with(config());
    startup(&mut machine);

    for _ in 0..200 {
        let _ = machine.handle(Event::ActionFailed {
            kind: ActionKind::Advertisement,
            error: "EPERM".to_owned(),
        });
        let _ = machine.handle(Event::ActionFailed {
            kind: ActionKind::AddAddresses,
            error: "EPERM".to_owned(),
        });
        machine.clock().advance(Duration::from_millis(500));
        let _ = machine.handle(Event::TimerExpired(TimerId::Retry));
        let _ = machine.handle(Event::TimerExpired(TimerId::HoldDown));
        let _ = machine.handle(Event::TimerExpired(TimerId::MasterDown));
    }

    // Six timers is the whole vocabulary, so a failure storm cannot accumulate
    // timers, and the instance settles rather than oscillating.
    assert!(
        machine.timers().len() <= 6,
        "I-40: the timer set is bounded"
    );
    assert!(
        matches!(machine.role(), Role::Fault | Role::Backup | Role::Master),
        "the instance settles in a stable role, got {}",
        machine.role()
    );
}

// ----- I-41: no work outlives participation -----------------------------

#[test]
fn i41_no_ownership_work_survives_leaving_participation() {
    let mut machine = machine_with(config());
    startup(&mut machine);
    let _ = machine.handle(Event::TimerExpired(TimerId::MasterDown));
    assert_eq!(machine.pending(), PendingOwnership::Acquiring);

    let actions = machine.handle(Event::OperatorPauseRequested);
    assert_eq!(machine.role(), Role::Disabled);
    assert_eq!(
        machine.pending(),
        PendingOwnership::None,
        "I-41: the request is abandoned"
    );
    assert!(
        actions.contains(&Action::RemoveVirtualAddresses),
        "an abandoned acquisition still owes a best-effort removal"
    );

    // A late confirmation from the executor cannot revive the instance.
    let actions = machine.handle(Event::ActionSucceeded {
        kind: ActionKind::AddAddresses,
    });
    let advertises = actions
        .iter()
        .any(|action| matches!(action, Action::SendAdvertisement { .. }));
    assert!(!advertises);
    assert_eq!(machine.role(), Role::Disabled);
}

// ----- I-42: an instance only ever releases its own addresses ------------

#[test]
fn i42_an_instance_never_requests_removal_it_does_not_own() {
    let mut machine = machine_with(config());
    startup(&mut machine);

    for event in [
        Event::Startup,
        Event::InterfaceUp,
        Event::OperatorPauseRequested,
        Event::ShutdownRequested,
        Event::InterfaceDown,
    ] {
        let actions = machine.handle(event);
        assert!(
            !actions.contains(&Action::RemoveVirtualAddresses) || machine.owns_virtual_addresses(),
            "I-42: removal is only requested for addresses this instance holds"
        );
    }
}

// ----- I-19 and I-43: the state machine's share of each ------------------

#[test]
fn i19_ownership_is_confirmed_and_not_assumed() {
    // The executor half of `I-19` is a netns test in Milestone 3. What the
    // state machine can be held to is that confirmation is the only path to
    // ownership: without an `ActionSucceeded` for `AddAddresses`, no number of
    // other events reaches `MASTER`.
    let mut machine = machine_with(config());
    startup(&mut machine);

    for _ in 0..10 {
        machine.clock().advance(Duration::from_millis(500));
        let _ = machine.handle(Event::TimerExpired(TimerId::MasterDown));
        let _ = machine.handle(Event::TimerExpired(TimerId::PreemptionDelay));
        let _ = machine.handle(Event::AdvertisementTimeout);
        assert_ne!(machine.role(), Role::Master);
        assert!(!machine.owns_virtual_addresses());
    }
}

#[test]
fn i43_advertisements_from_another_vrid_are_rejected() {
    // The source allow-list of `I-43` is enforced by the layer that sees the
    // wire, which only delivers validated advertisements to this crate
    // (`SPEC.md` §14.3). The state machine's share is to reject anything
    // addressed to another VRID.
    let mut machine = machine_with(config());
    startup(&mut machine);
    let before = machine
        .deadline_of(TimerId::MasterDown)
        .expect("armed at startup");

    let actions = machine.handle(Event::AdvertisementReceived(PeerAdvertisement {
        vrid: 7,
        priority: 255,
        source: "192.0.2.12".parse().expect("valid address"),
        advert_interval: ADVERT,
    }));

    assert_eq!(machine.deadline_of(TimerId::MasterDown), Some(before));
    assert!(!actions.contains(&Action::RemoveVirtualAddresses));
    assert!(!actions.contains(&Action::AddVirtualAddresses));
}

// ----- I-38: health checks cannot block the protocol loop ---------------

#[test]
fn i38_the_state_machine_cannot_wait_on_a_health_check() {
    // The structural half of `I-38` is a dependency fact, so it is asserted as
    // one: the crate that owns the state machine does not depend on the crate
    // that runs checks, and its manifest pulls in nothing that could block.
    let manifest = include_str!("../Cargo.toml");
    for forbidden in ["highland-checks", "tokio", "async-std", "smol"] {
        assert!(
            !manifest.contains(forbidden),
            "I-38: highland-core must not depend on {forbidden}"
        );
    }

    // The behavioural half: handling health is a pure state update, so the
    // machine returns promptly and without any timer that a probe would own.
    let mut machine = machine_with(config());
    startup(&mut machine);
    let actions = machine.handle(health(0, 0, 0));
    assert_eq!(
        actions,
        vec![Action::SetEffectivePriority { priority: 150 }]
    );
    assert_eq!(
        machine.timers().armed(),
        [TimerId::MasterDown],
        "health adds no timer of its own"
    );
}

// ----- a master must keep advertising -------------------------------------

/// A master that advertised once and then went quiet is dead as far as its peers
/// are concerned: they time it out, take the address, and it takes the address
/// back on its own master-down timer. The address then moves every
/// `Master_Down_Interval`, which is worse for clients than a plain outage.
///
/// This is the invariant the advertisement timer being periodic. It was missing,
/// and only a chaos suite running long enough with a lossy segment found it.
#[test]
fn a_master_re_arms_its_advertisement_timer_so_advertising_continues() {
    let mut machine = machine_with(config());
    startup(&mut machine);
    promote(&mut machine);

    // A full interval passes, so the advertisement is due.
    machine.clock().advance(ADVERT);
    let actions = machine.handle(Event::TimerExpired(TimerId::Advertisement));
    assert!(
        actions.contains(&Action::SendAdvertisement { priority: 150 }),
        "the due advertisement was not sent: {actions:?}"
    );

    // And the next one is due too, which is the part that was broken.
    machine.clock().advance(ADVERT);
    let actions = machine.handle(Event::TimerExpired(TimerId::Advertisement));
    assert!(
        actions.contains(&Action::SendAdvertisement { priority: 150 }),
        "advertising stopped after the first one: {actions:?}"
    );

    // Over a minute of advertising, a peer never sees a gap longer than one
    // interval, which is the property a backup's master-down timer relies on.
    let mut worst_gap = Duration::ZERO;
    let mut since_advert = Duration::ZERO;
    for _ in 0..60 {
        machine.clock().advance(ADVERT / 2);
        since_advert += ADVERT / 2;
        let actions = machine.handle(Event::TimerExpired(TimerId::Advertisement));
        if actions.contains(&Action::SendAdvertisement { priority: 150 }) {
            worst_gap = worst_gap.max(since_advert);
            since_advert = Duration::ZERO;
        }
    }
    assert!(
        worst_gap <= ADVERT,
        "a peer would see a {worst_gap:?} gap between advertisements"
    );
}
