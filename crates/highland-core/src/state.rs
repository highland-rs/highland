// Rust guideline compliant 2026-09-27

//! The domain types of the state machine: roles, events, actions, and the
//! configuration subset the machine depends on.
//!
//! The machine itself is in [`crate::machine`]. This module holds only types, so
//! that a reader can see the vocabulary without reading the logic.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;

use crate::error::CoreError;
use crate::health::HealthPolicyConfig;
use crate::health::HealthSummary;
use crate::timer::RetryPolicy;

/// The role an instance occupies.
///
/// A role is the single source of truth about ownership: ownership exists only
/// in [`Role::Master`] (SPEC.md, `I-03`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Role {
    /// Startup state before the instance participates in election.
    Init,
    /// Monitoring advertisements and eligible to become [`Role::Master`].
    Backup,
    /// Owning the VIP set and sending advertisements.
    Master,
    /// A prior ownership attempt failed; ownership is undefined.
    Fault,
    /// Operator- or configuration-disabled; not participating.
    Disabled,
}

impl Role {
    /// Returns the numeric encoding used by the `highland_instance_role`
    /// metric (SPEC.md, `R-19`).
    #[must_use]
    pub fn as_metric(self) -> u8 {
        match self {
            Role::Init => 0,
            Role::Backup => 1,
            Role::Master => 2,
            Role::Fault => 3,
            Role::Disabled => 4,
        }
    }

    /// Returns the stable, upper-case name used in events and status output.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Init => "INIT",
            Role::Backup => "BACKUP",
            Role::Master => "MASTER",
            Role::Fault => "FAULT",
            Role::Disabled => "DISABLED",
        }
    }

    /// Parses a role name, as accepted by `highland force-transition --role`.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::UnknownRole`] when `text` is not a role name.
    pub fn parse(text: &str) -> Result<Self, CoreError> {
        match text.to_ascii_lowercase().as_str() {
            "init" => Ok(Role::Init),
            "backup" => Ok(Role::Backup),
            "master" => Ok(Role::Master),
            "fault" => Ok(Role::Fault),
            "disabled" => Ok(Role::Disabled),
            other => Err(CoreError::UnknownRole {
                role: other.to_owned(),
            }),
        }
    }

    /// Returns `true` when the role owns VIPs.
    #[must_use]
    pub fn owns_virtual_addresses(self) -> bool {
        matches!(self, Role::Master)
    }

    /// Returns `true` when the role may transmit a normal advertisement.
    #[must_use]
    pub fn advertises(self) -> bool {
        matches!(self, Role::Master)
    }

    /// Returns `true` when the role participates in election.
    #[must_use]
    pub fn participates(self) -> bool {
        matches!(self, Role::Init | Role::Backup | Role::Master)
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A timer identifier owned by a single instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TimerId {
    /// Delay between the instance starting and entering election.
    StartupDelay,
    /// Periodic advertisement transmission while [`Role::Master`].
    Advertisement,
    /// `BACKUP` takeover delay, `3 * adver_int + Skew_Time`.
    MasterDown,
    /// Delay before a higher-priority `BACKUP` preempts.
    PreemptionDelay,
    /// Refusal to reclaim ownership after a fault.
    HoldDown,
    /// Bounded retry delay after a failed action.
    Retry,
}

impl fmt::Display for TimerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            TimerId::StartupDelay => "StartupDelay",
            TimerId::Advertisement => "Advertisement",
            TimerId::MasterDown => "MasterDown",
            TimerId::PreemptionDelay => "PreemptionDelay",
            TimerId::HoldDown => "HoldDown",
            TimerId::Retry => "Retry",
        })
    }
}

/// The clock-resolution allowance defined by RFC 5798.
pub const SKEW_TIME: Duration = Duration::from_millis(10);

/// A monotonically increasing configuration revision identifier.
///
/// Results carrying a generation older than the instance's active generation
/// MUST be discarded (SPEC.md, `I-12`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Generation(u64);

impl Generation {
    /// Returns the initial generation, which precedes any loaded configuration.
    #[must_use]
    pub fn initial() -> Self {
        Self(0)
    }

    /// Returns the next generation.
    #[must_use]
    pub fn next(self) -> Self {
        Self(self.0 + 1)
    }

    /// Returns the numeric value.
    #[must_use]
    pub fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for Generation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The domain-level view of a received advertisement.
///
/// The protocol crate owns wire decoding; the state machine only needs these
/// fields, which keeps `highland-core` independent of the wire format
/// (SPEC.md, §9.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerAdvertisement {
    /// The VRID carried by the advertisement.
    pub vrid: u8,
    /// The priority carried by the advertisement.
    pub priority: u8,
    /// The address the advertisement was received from.
    pub source: IpAddr,
    /// The advertisement interval the peer claims.
    pub advert_interval: Duration,
}

impl PeerAdvertisement {
    /// Returns `true` when this advertisement announces a relinquish.
    #[must_use]
    pub fn is_relinquish(&self) -> bool {
        self.priority == 0
    }
}

/// The reason attached to a role transition.
///
/// The set of reasons is closed: adding a variant is a specification change
/// (SPEC.md, `R-17`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TransitionReason {
    /// The instance started and entered election.
    Startup,
    /// The interface became usable.
    InterfaceUp,
    /// The interface became unusable.
    InterfaceDown,
    /// No advertisement arrived within `Master_Down_Interval`.
    MasterDownTimeout,
    /// A higher-priority peer advertised, so this `MASTER` stepped down.
    HigherPriorityPeerAdvertisement,
    /// The preemption delay expired, so a higher-priority `BACKUP` took over.
    Preemption,
    /// Health policy made the instance ineligible.
    HealthIneligible,
    /// An operator requested relinquishment.
    OperatorRelinquish,
    /// An operator forced a transition; requires a daemon flag.
    OperatorForceTransition,
    /// The configuration changed in a way that permits the transition.
    ConfigurationReloaded,
    /// The hold-down period expired and re-verification succeeded.
    HoldDownExpired,
    /// Applying or releasing ownership failed.
    OwnershipFailed,
    /// The process is shutting down.
    Shutdown,
}

impl fmt::Display for TransitionReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            TransitionReason::Startup => "startup",
            TransitionReason::InterfaceUp => "interface_up",
            TransitionReason::InterfaceDown => "interface_down",
            TransitionReason::MasterDownTimeout => "master_down_timeout",
            TransitionReason::HigherPriorityPeerAdvertisement => {
                "higher_priority_peer_advertisement"
            }
            TransitionReason::Preemption => "preemption_delay_elapsed",
            TransitionReason::HealthIneligible => "health_ineligible",
            TransitionReason::OperatorRelinquish => "operator_relinquish",
            TransitionReason::OperatorForceTransition => "operator_force_transition",
            TransitionReason::ConfigurationReloaded => "configuration_reloaded",
            TransitionReason::HoldDownExpired => "hold_down_expired",
            TransitionReason::OwnershipFailed => "ownership_failed",
            TransitionReason::Shutdown => "shutdown",
        })
    }
}

/// A log level used by [`Action::Log`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum LogLevel {
    /// A condition that prevented an intended action.
    Error,
    /// A recoverable condition the operator should know about.
    Warn,
    /// Detail useful for diagnosis.
    Debug,
}

/// The kind of an [`Action`], used when reporting an outcome back to the
/// machine.
///
/// The classification is deliberately coarser than [`Action`]: adding an action
/// must not require a new outcome path in the executor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ActionKind {
    /// Arm or cancel a timer.
    Timer,
    /// Transmit an advertisement.
    Advertisement,
    /// Add VIPs to an interface.
    AddAddresses,
    /// Remove VIPs from an interface.
    RemoveAddresses,
    /// Emit gratuitous ARP or unsolicited Neighbor Advertisements.
    GratuitousUpdate,
    /// Record a new effective priority.
    SetPriority,
    /// Record a role change.
    EnterRole,
    /// Emit an event.
    EmitEvent,
    /// Write a log line.
    Log,
}

/// An input to the state machine.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Event {
    /// The daemon started, or the instance was created by a reload.
    Startup,
    /// The startup-delay timer expired.
    StartupDelayElapsed,
    /// The instance's interface became usable.
    InterfaceUp,
    /// The instance's interface became unusable.
    InterfaceDown,
    /// The instance lost carrier while administratively up.
    CarrierLost,
    /// A valid advertisement arrived from a configured peer.
    AdvertisementReceived(PeerAdvertisement),
    /// The master-down timer expired without a valid advertisement.
    AdvertisementTimeout,
    /// The instance's aggregate health changed.
    HealthChanged(HealthSummary),
    /// A timer owned by this instance expired.
    TimerExpired(TimerId),
    /// The executor applied an action successfully.
    ///
    /// Success is reported, not assumed: a node enters `MASTER` only after its
    /// addresses are confirmed present, which is what makes `I-04` structural
    /// rather than a matter of ordering.
    ActionSucceeded {
        /// The action that succeeded.
        kind: ActionKind,
    },
    /// The executor failed to apply one of the machine's actions.
    ActionFailed {
        /// The action that failed.
        kind: ActionKind,
        /// A description of the failure.
        error: String,
    },
    /// An operator asked the instance to relinquish ownership.
    OperatorRelinquishRequested,
    /// An operator paused the instance.
    OperatorPauseRequested,
    /// An operator resumed the instance.
    OperatorResumeRequested,
    /// An operator forced a transition. Requires a daemon flag (`R-10`).
    OperatorForceTransitionRequested {
        /// The role to force.
        target: Role,
        /// Why, for the audit record.
        reason: String,
    },
    /// The interface was administratively brought up by an operator.
    InterfaceBroughtUp,
    /// A new configuration generation was accepted.
    ConfigurationReloaded {
        /// The generation that is now active.
        generation: Generation,
    },
    /// The process is shutting down.
    ShutdownRequested,
}

/// A side effect the machine requests from an executor.
///
/// Actions are requests, not completed facts. The executor applies them and
/// reports outcomes as [`Event::ActionSucceeded`] or [`Event::ActionFailed`]
/// (SPEC.md, `R-11`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Action {
    /// Arm a timer for `timer` at absolute `deadline`.
    ArmTimer {
        /// The timer to arm.
        timer: TimerId,
        /// The absolute deadline, in [`crate::clock::Clock::now`] units.
        deadline: Duration,
    },
    /// Cancel a timer. Cancellation is idempotent (`I-29`).
    CancelTimer {
        /// The timer to cancel.
        timer: TimerId,
    },
    /// Transmit an advertisement announcing `priority`.
    ///
    /// A priority of `0` is only ever emitted on a relinquish path (§12.4), and
    /// never as a normal advertisement (`I-27`).
    SendAdvertisement {
        /// The priority to announce.
        priority: u8,
    },
    /// Add the configured VIPs and confirm them by read-back.
    AddVirtualAddresses,
    /// Remove the configured VIPs and confirm their absence.
    RemoveVirtualAddresses,
    /// Emit gratuitous ARP or unsolicited Neighbor Advertisements.
    SendGratuitousUpdates,
    /// Record a new effective priority.
    SetEffectivePriority {
        /// The new effective priority.
        priority: u8,
    },
    /// Enter a role, with a machine-readable reason.
    EnterRole {
        /// The role being entered.
        role: Role,
        /// Why the role changed.
        reason: TransitionReason,
    },
    /// Emit an event to the observability layer.
    EmitEvent {
        /// The event name, from the closed set in `docs/user/operations.md`.
        name: &'static str,
    },
    /// Record a log line.
    Log {
        /// The log level.
        level: LogLevel,
        /// The message text.
        message: String,
    },
}

impl Action {
    /// Returns the [`ActionKind`] of this action.
    #[must_use]
    pub fn kind(&self) -> ActionKind {
        match self {
            Action::ArmTimer { .. } | Action::CancelTimer { .. } => ActionKind::Timer,
            Action::SendAdvertisement { .. } => ActionKind::Advertisement,
            Action::AddVirtualAddresses => ActionKind::AddAddresses,
            Action::RemoveVirtualAddresses => ActionKind::RemoveAddresses,
            Action::SendGratuitousUpdates => ActionKind::GratuitousUpdate,
            Action::SetEffectivePriority { .. } => ActionKind::SetPriority,
            Action::EnterRole { .. } => ActionKind::EnterRole,
            Action::EmitEvent { .. } => ActionKind::EmitEvent,
            Action::Log { .. } => ActionKind::Log,
        }
    }
}

/// The configuration subset the state machine depends on.
///
/// Values are expected to have been validated by `highland-config`; the state
/// machine does not re-validate them (SPEC.md, `V-01` through `V-04`). That
/// split keeps the machine free of configuration parsing, and it is why
/// `InstanceStateMachine::new` is infallible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceConfig {
    /// The instance name, used in events and metrics.
    pub name: String,
    /// The VRID. Must be in `1..=255`.
    pub vrid: u8,
    /// The configured priority. Must be in `1..=255`.
    pub priority: u8,
    /// The advertisement interval. Must be in `10ms..=2550ms`.
    pub advertisement_interval: Duration,
    /// The delay between startup and entering election.
    pub startup_delay: Duration,
    /// Whether a higher-priority `BACKUP` may preempt.
    pub preempt: bool,
    /// The delay before a higher-priority `BACKUP` preempts.
    pub preempt_delay: Duration,
    /// The refusal period after a fault.
    pub hold_down: Duration,
    /// The backoff policy applied to a failed action.
    pub retry: RetryPolicy,
    /// The health policy and its parameters.
    pub health: HealthPolicyConfig,
    /// The primary IPv4 address of the interface, used for tie-breaking.
    pub primary_ipv4: Option<Ipv4Addr>,
    /// The primary IPv6 address of the interface, used for tie-breaking.
    pub primary_ipv6: Option<Ipv6Addr>,
}

impl Default for InstanceConfig {
    /// Returns the default instance configuration used by tests and examples.
    ///
    /// The default VRID is 1. A production configuration must set every field
    /// explicitly, and duplicate `(interface, vrid)` pairs are rejected by
    /// `V-06`.
    fn default() -> Self {
        Self {
            name: "default".to_owned(),
            vrid: 1,
            priority: 100,
            advertisement_interval: Duration::from_secs(1),
            startup_delay: Duration::ZERO,
            preempt: true,
            preempt_delay: Duration::ZERO,
            hold_down: Duration::from_secs(10),
            retry: RetryPolicy::default(),
            health: HealthPolicyConfig::default(),
            primary_ipv4: None,
            primary_ipv6: None,
        }
    }
}

/// Returns `3 * adver_int + Skew_Time`, the `BACKUP` takeover delay.
///
/// # Errors
///
/// Returns [`CoreError::TimerDurationOverflow`] when the computation leaves the
/// representable `Duration` range.
///
/// # Examples
///
/// ```
/// use highland_core::state::master_down_interval;
/// use std::time::Duration;
///
/// assert_eq!(master_down_interval(&Duration::from_secs(1)).unwrap(), Duration::from_millis(3010));
/// ```
pub fn master_down_interval(advert_interval: &Duration) -> Result<Duration, CoreError> {
    advert_interval
        .checked_mul(3)
        .and_then(|scaled| scaled.checked_add(SKEW_TIME))
        .ok_or(CoreError::TimerDurationOverflow {
            timer: TimerId::MasterDown,
            operation: "3 * adver_int + Skew_Time",
            max: Duration::MAX,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_metric_encoding_matches_the_specification() {
        assert_eq!(Role::Init.as_metric(), 0);
        assert_eq!(Role::Backup.as_metric(), 1);
        assert_eq!(Role::Master.as_metric(), 2);
        assert_eq!(Role::Fault.as_metric(), 3);
        assert_eq!(Role::Disabled.as_metric(), 4);
    }

    #[test]
    fn only_the_master_role_owns_addresses_and_advertises() {
        for role in [
            Role::Init,
            Role::Backup,
            Role::Master,
            Role::Fault,
            Role::Disabled,
        ] {
            assert_eq!(role.owns_virtual_addresses(), role == Role::Master);
            assert_eq!(role.advertises(), role == Role::Master);
        }
    }

    #[test]
    fn roles_parse_from_their_command_line_spelling() {
        for (text, expected) in [
            ("init", Role::Init),
            ("BACKUP", Role::Backup),
            ("master", Role::Master),
            ("fault", Role::Fault),
            ("disabled", Role::Disabled),
        ] {
            assert_eq!(Role::parse(text), Ok(expected));
        }
        assert!(matches!(
            Role::parse("sideways"),
            Err(CoreError::UnknownRole { .. })
        ));
    }

    #[test]
    fn participation_is_limited_to_the_three_election_roles() {
        assert!(Role::Init.participates());
        assert!(Role::Backup.participates());
        assert!(Role::Master.participates());
        assert!(!Role::Fault.participates());
        assert!(!Role::Disabled.participates());
    }

    #[test]
    fn action_kinds_classify_actions() {
        assert_eq!(Action::AddVirtualAddresses.kind(), ActionKind::AddAddresses);
        assert_eq!(
            Action::RemoveVirtualAddresses.kind(),
            ActionKind::RemoveAddresses
        );
        assert_eq!(
            Action::SendAdvertisement { priority: 150 }.kind(),
            ActionKind::Advertisement
        );
        assert_eq!(
            Action::CancelTimer {
                timer: TimerId::Retry
            }
            .kind(),
            ActionKind::Timer
        );
    }

    #[test]
    fn a_relinquish_advertisement_is_recognized() {
        let mut advertisement = PeerAdvertisement {
            vrid: 42,
            priority: 0,
            source: "192.0.2.11".parse().expect("literal is a valid address"),
            advert_interval: Duration::from_secs(1),
        };
        assert!(advertisement.is_relinquish());
        advertisement.priority = 150;
        assert!(!advertisement.is_relinquish());
    }

    #[test]
    fn master_down_interval_follows_rfc_5798() {
        assert_eq!(
            master_down_interval(&Duration::from_millis(10)).unwrap(),
            Duration::from_millis(40)
        );
        assert_eq!(
            master_down_interval(&Duration::from_millis(2550)).unwrap(),
            Duration::from_millis(7660)
        );
        assert!(master_down_interval(&Duration::MAX).is_err());
    }

    #[test]
    fn generations_increase_monotonically() {
        let generation = Generation::initial().next().next();
        assert_eq!(generation.get(), 2);
        assert!(Generation::initial() < generation);
    }
}
