// Rust guideline compliant 2026-09-27

//! The per-instance VRRP state machine.
//!
//! The state machine is synchronous, deterministic, and performs no I/O. It
//! consumes [`Event`] values and returns [`Action`] values for an executor to
//! apply (SPEC.md, §11). All durations come from a [`Clock`].

use std::fmt;
use std::net::IpAddr;
use std::time::Duration;

use crate::clock::Clock;
use crate::error::CoreError;

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
    /// Returns `true` when `self` outranks `other` under the protocol's
    /// priority comparison (higher priority wins; equal priority does not).
    #[must_use]
    pub fn outranks(&self, other: &Self) -> bool {
        self.priority > other.priority
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
        let text = match self {
            TransitionReason::Startup => "startup",
            TransitionReason::InterfaceUp => "interface_up",
            TransitionReason::InterfaceDown => "interface_down",
            TransitionReason::MasterDownTimeout => "master_down_timeout",
            TransitionReason::HigherPriorityPeerAdvertisement => {
                "higher_priority_peer_advertisement"
            }
            TransitionReason::HealthIneligible => "health_ineligible",
            TransitionReason::OperatorRelinquish => "operator_relinquish",
            TransitionReason::OperatorForceTransition => "operator_force_transition",
            TransitionReason::ConfigurationReloaded => "configuration_reloaded",
            TransitionReason::HoldDownExpired => "hold_down_expired",
            TransitionReason::OwnershipFailed => "ownership_failed",
            TransitionReason::Shutdown => "shutdown",
        };
        f.write_str(text)
    }
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
    /// A valid advertisement arrived from a configured peer.
    AdvertisementReceived(PeerAdvertisement),
    /// The master-down timer expired without a valid advertisement.
    AdvertisementTimeout,
    /// A timer owned by this instance expired.
    TimerExpired(TimerId),
    /// The executor failed to apply one of the machine's actions.
    ActionFailed {
        /// The action that failed.
        kind: ActionKind,
        /// A description of the failure.
        error: String,
    },
    /// An operator asked the instance to relinquish ownership.
    OperatorRelinquishRequested,
    /// The process is shutting down.
    ShutdownRequested,
}

/// The kind of an [`Action`], used when reporting a failure back to the machine.
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
    /// Schedule a bounded retry.
    ScheduleRetry,
    /// Write a log record.
    Log,
}

/// A side effect the machine requests from an executor.
///
/// Actions are requests, not completed facts. The executor applies them and
/// reports outcomes as [`Event::ActionFailed`] (SPEC.md, `R-11`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Action {
    /// Arm a timer for `timer` at absolute `deadline`.
    ArmTimer {
        /// The timer to arm.
        timer: TimerId,
        /// The absolute deadline, in [`Clock::now`] units.
        deadline: Duration,
    },
    /// Cancel a timer. Cancellation is idempotent (`I-29`).
    CancelTimer {
        /// The timer to cancel.
        timer: TimerId,
    },
    /// Transmit an advertisement announcing `priority`.
    SendAdvertisement {
        /// The priority to announce. Never `0` (SPEC.md, `I-27`).
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
        /// The event name, from the closed set in `docs/operations.md`.
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

/// A log level used by [`Action::Log`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LogLevel {
    /// Recoverable condition the operator should know about.
    Warn,
    /// Detail useful for diagnosis.
    Debug,
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
/// machine does not re-validate them (SPEC.md, `V-01` through `V-04`).
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
        }
    }
}

/// The per-instance state machine.
///
/// # Examples
///
/// ```
/// use highland_core::clock::ManualClock;
/// use highland_core::state::{Action, Event, InstanceConfig, InstanceStateMachine, Role, TimerId};
/// use std::time::Duration;
///
/// let clock = ManualClock::new();
/// let config = InstanceConfig {
///     name: "api".to_owned(),
///     vrid: 42,
///     priority: 150,
///     ..InstanceConfig::default()
/// };
///
/// let mut machine = InstanceStateMachine::new(config, clock);
/// let actions = machine.handle(Event::Startup);
///
/// assert_eq!(machine.role(), Role::Backup);
/// assert!(actions.contains(&Action::ArmTimer {
///     timer: TimerId::MasterDown,
///     deadline: Duration::from_millis(3010),
/// }));
/// ```
#[derive(Debug)]
pub struct InstanceStateMachine<C> {
    config: InstanceConfig,
    clock: C,
    role: Role,
    generation: Generation,
    effective_priority: u8,
    armed: Vec<(TimerId, Duration)>,
}

impl<C> InstanceStateMachine<C>
where
    C: Clock,
{
    /// Creates a machine in [`Role::Init`] for `config`.
    #[must_use]
    pub fn new(config: InstanceConfig, clock: C) -> Self {
        Self {
            effective_priority: config.priority,
            config,
            clock,
            role: Role::Init,
            generation: Generation::initial(),
            armed: Vec::new(),
        }
    }

    /// Returns the instance configuration.
    #[must_use]
    pub fn config(&self) -> &InstanceConfig {
        &self.config
    }

    /// Returns the current role.
    #[must_use]
    pub fn role(&self) -> Role {
        self.role
    }

    /// Returns the current effective priority.
    #[must_use]
    pub fn effective_priority(&self) -> u8 {
        self.effective_priority
    }

    /// Returns the active configuration generation.
    #[must_use]
    pub fn generation(&self) -> Generation {
        self.generation
    }

    /// Returns the deadline of an armed timer, if it is armed.
    #[must_use]
    pub fn deadline_of(&self, timer: TimerId) -> Option<Duration> {
        self.armed
            .iter()
            .find(|(id, _)| *id == timer)
            .map(|(_, at)| *at)
    }

    /// Returns `true` when `timer` is armed and its deadline has passed.
    #[must_use]
    pub fn is_due(&self, timer: TimerId) -> bool {
        self.deadline_of(timer)
            .is_some_and(|deadline| self.clock.remaining_until(deadline).is_zero())
    }

    /// Handles one event and returns the actions the executor must apply.
    ///
    /// The machine performs no I/O, so this function is total and deterministic
    /// for a given clock reading and event sequence (SPEC.md, `R-26`).
    /// Handles one event and returns the actions the executor must apply.
    ///
    /// The machine performs no I/O, so this function is total and deterministic
    /// for a given clock reading and event sequence (SPEC.md, `R-26`).
    // The event is taken by value because it is consumed by the match below;
    // taking a reference would only move the decision to the caller.
    #[allow(clippy::needless_pass_by_value)]
    pub fn handle(&mut self, event: Event) -> Vec<Action> {
        match event {
            Event::Startup | Event::InterfaceUp => self.begin_startup(),
            Event::StartupDelayElapsed => self.enter_election(),
            _ => Vec::new(),
        }
    }

    /// Arms the startup delay, or enters election immediately when there is
    /// none.
    fn begin_startup(&mut self) -> Vec<Action> {
        if !matches!(self.role, Role::Init | Role::Fault) {
            return Vec::new();
        }
        if !self.config.startup_delay.is_zero() {
            return vec![Action::ArmTimer {
                timer: TimerId::StartupDelay,
                deadline: self.clock.now() + self.config.startup_delay,
            }];
        }
        self.enter_election()
    }

    fn enter_election(&mut self) -> Vec<Action> {
        if !matches!(self.role, Role::Init | Role::Fault) {
            return Vec::new();
        }

        let deadline =
            self.clock.now() + master_down_interval_saturating(&self.config.advertisement_interval);
        self.armed.retain(|(id, _)| *id != TimerId::MasterDown);
        self.armed.push((TimerId::MasterDown, deadline));
        self.role = Role::Backup;

        vec![
            Action::SetEffectivePriority {
                priority: self.effective_priority,
            },
            Action::EnterRole {
                role: Role::Backup,
                reason: TransitionReason::Startup,
            },
            Action::ArmTimer {
                timer: TimerId::MasterDown,
                deadline,
            },
            Action::EmitEvent {
                name: "role_transition",
            },
        ]
    }
}

/// Returns `3 * adver_int + Skew_Time`, saturating instead of overflowing.
///
/// The state machine uses this form because the advertisement interval has
/// already been validated to `10ms..=2550ms` by `V-04`, which makes saturation
/// unreachable; [`master_down_interval`] returns an error for callers that
/// have not validated their input.
fn master_down_interval_saturating(advert_interval: &Duration) -> Duration {
    /// The clock-resolution allowance defined by RFC 5798.
    const SKEW_TIME: Duration = Duration::from_millis(10);

    advert_interval.saturating_mul(3).saturating_add(SKEW_TIME)
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
    /// The clock-resolution allowance defined by RFC 5798.
    const SKEW_TIME: Duration = Duration::from_millis(10);

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
    use crate::clock::ManualClock;

    fn config() -> InstanceConfig {
        InstanceConfig {
            name: "api".to_owned(),
            vrid: 42,
            priority: 150,
            ..InstanceConfig::default()
        }
    }

    #[test]
    fn a_new_machine_is_init() {
        let machine = InstanceStateMachine::new(config(), ManualClock::new());
        assert_eq!(machine.role(), Role::Init);
        assert_eq!(machine.effective_priority(), 150);
        assert!(!machine.role().owns_virtual_addresses());
    }

    #[test]
    fn startup_enters_backup_and_arms_the_master_down_timer() {
        let mut machine = InstanceStateMachine::new(config(), ManualClock::new());
        let actions = machine.handle(Event::Startup);

        assert_eq!(machine.role(), Role::Backup);
        assert!(actions.iter().any(|a| matches!(
            a,
            Action::EnterRole {
                role: Role::Backup,
                ..
            }
        )));
        assert_eq!(
            machine.deadline_of(TimerId::MasterDown),
            Some(Duration::from_millis(3010))
        );
    }

    #[test]
    fn a_nonzero_startup_delay_defers_entering_election() {
        let mut config = config();
        config.startup_delay = Duration::from_secs(5);
        let mut machine = InstanceStateMachine::new(config, ManualClock::new());

        let actions = machine.handle(Event::Startup);
        assert_eq!(machine.role(), Role::Init);
        assert!(actions.contains(&Action::ArmTimer {
            timer: TimerId::StartupDelay,
            deadline: Duration::from_secs(5),
        }));

        machine.clock.advance(Duration::from_secs(5));
        machine.handle(Event::StartupDelayElapsed);
        assert_eq!(machine.role(), Role::Backup);
    }

    #[test]
    fn startup_is_idempotent_once_the_instance_is_backup() {
        let mut machine = InstanceStateMachine::new(config(), ManualClock::new());
        machine.handle(Event::Startup);
        assert!(machine.handle(Event::Startup).is_empty());
        assert_eq!(machine.role(), Role::Backup);
    }

    #[test]
    fn the_master_down_timer_is_due_only_after_its_deadline() {
        let clock = ManualClock::new();
        let mut machine = InstanceStateMachine::new(config(), clock.clone());
        machine.handle(Event::Startup);

        assert!(!machine.is_due(TimerId::MasterDown));
        clock.advance(Duration::from_millis(3009));
        assert!(!machine.is_due(TimerId::MasterDown));
        clock.advance(Duration::from_millis(1));
        assert!(machine.is_due(TimerId::MasterDown));
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
    fn role_metric_encoding_matches_the_specification() {
        assert_eq!(Role::Init.as_metric(), 0);
        assert_eq!(Role::Backup.as_metric(), 1);
        assert_eq!(Role::Master.as_metric(), 2);
        assert_eq!(Role::Fault.as_metric(), 3);
        assert_eq!(Role::Disabled.as_metric(), 4);
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
    fn higher_priority_advertisements_outrank_lower_ones() {
        let strong = PeerAdvertisement {
            vrid: 42,
            priority: 150,
            source: "192.0.2.11".parse().expect("literal is a valid address"),
            advert_interval: Duration::from_secs(1),
        };
        let weak = PeerAdvertisement {
            priority: 100,
            ..strong.clone()
        };

        assert!(strong.outranks(&weak));
        assert!(!weak.outranks(&strong));
        assert!(!strong.outranks(&strong));
    }

    #[test]
    fn generations_increase_monotonically() {
        let generation = Generation::initial().next().next();
        assert_eq!(generation.get(), 2);
        assert!(Generation::initial() < generation);
    }
}
