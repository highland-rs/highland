// Rust guideline compliant 2026-09-27

//! The per-instance state machine.
//!
//! The machine is synchronous, deterministic, and performs no I/O. It consumes
//! [`Event`] values and returns [`Action`] values for an executor to apply
//! (SPEC.md, §11). It is the only authority over role and ownership.
//!
//! # The two-phase handshake
//!
//! The machine never assumes that an action succeeded. Taking ownership is a
//! request (`Action::AddVirtualAddresses`) followed by
//! [`Event::ActionSucceeded`] carrying [`ActionKind::AddAddresses`]; only then
//! does the machine enter `MASTER` and start advertising. That is what makes
//! `I-04` structural rather than a matter of action ordering: there is no path
//! that reaches `MASTER` and emits an advertisement without confirmed
//! ownership.
//!
//! # Example
//!
//! ```
//! use highland_core::clock::ManualClock;
//! use highland_core::machine::{InstanceStateMachine, PendingOwnership};
//! use highland_core::state::{Action, ActionKind, Event, InstanceConfig, Role, TimerId};
//! use std::time::Duration;
//!
//! let clock = ManualClock::new();
//! let config = InstanceConfig {
//!     name: "api".to_owned(),
//!     vrid: 42,
//!     priority: 150,
//!     ..InstanceConfig::default()
//! };
//!
//! let mut machine = InstanceStateMachine::new(config, clock.clone());
//! let actions = machine.handle(Event::Startup);
//!
//! assert_eq!(machine.role(), Role::Backup);
//! assert!(actions.contains(&Action::ArmTimer {
//!     timer: TimerId::MasterDown,
//!     deadline: Duration::from_millis(3410),
//! }));
//!
//! // The master-down timer fires, and the machine asks for the addresses.
//! clock.advance(Duration::from_millis(3410));
//! let actions = machine.handle(Event::TimerExpired(TimerId::MasterDown));
//! assert!(actions.contains(&Action::AddVirtualAddresses));
//! assert_eq!(machine.role(), Role::Backup, "ownership is not yet confirmed");
//!
//! // Only the executor's confirmation promotes the instance.
//! let actions = machine.handle(Event::ActionSucceeded { kind: ActionKind::AddAddresses });
//! assert_eq!(machine.role(), Role::Master);
//! assert!(actions.contains(&Action::SendAdvertisement { priority: 150 }));
//! assert_eq!(machine.pending(), PendingOwnership::None);
//! ```

#![forbid(unsafe_code)]

use std::net::IpAddr;
use std::time::Duration;

use crate::clock::Clock;
use crate::election::{self, Candidate};
use crate::health::{self, HealthSummary, PriorityState};
use crate::state::{
    Action, ActionKind, Event, Generation, InstanceConfig, LogLevel, PeerAdvertisement, Role,
    TimerId, TransitionReason, skew_time,
};
use crate::timer::TimerSet;

/// The number of consecutive advertisement failures that fault an instance,
/// The streak is cleared by a successful advertisement, a role change, or an
/// ownership confirmation, so this counts consecutive failures and not a total.
pub const ADVERTISE_FAILURE_LIMIT: u32 = 3;

/// How long the advertisement-failure path would allow a streak to span.
///
/// The path deliberately does not consult this; see
/// `on_advertisement_failure` for why a rolling window is not kept.
pub const ADVERTISE_FAILURE_WINDOW: Duration = Duration::from_secs(10);

/// An ownership operation awaiting the executor's confirmation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pending {
    /// Addresses have been requested and not yet confirmed.
    Acquire {
        /// Why the instance is taking ownership.
        reason: TransitionReason,
    },
    /// Removal has been requested and not yet confirmed.
    Release {
        /// Why the instance is releasing ownership.
        reason: TransitionReason,
        /// The role to enter once removal is confirmed.
        next: Role,
    },
}

/// A read-only view of the work the machine has asked for and not yet had
/// confirmed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingOwnership {
    /// Addresses have been requested.
    Acquiring,
    /// Removal has been requested.
    Releasing,
    /// Nothing is in flight.
    None,
}

/// The per-instance state machine.
#[derive(Debug)]
pub struct InstanceStateMachine<C> {
    config: InstanceConfig,
    clock: C,
    role: Role,
    generation: Generation,
    timers: TimerSet,
    health: HealthSummary,
    priority: PriorityState,
    interface_up: bool,
    pending: Option<Pending>,
    owns_addresses: bool,
    retry_attempt: u32,
    advertise_failures: u32,
    preemption_armed_for: Option<u8>,
    deferred_advertisement: Option<PeerAdvertisement>,
    shutting_down: bool,
    master_advert_interval: Duration,
    last_reason: TransitionReason,
    last_peer: Option<IpAddr>,
}

impl<C> InstanceStateMachine<C>
where
    C: Clock,
{
    /// Creates a machine in [`Role::Init`] for `config`.
    ///
    /// The configuration is assumed to have been validated by `highland-config`;
    /// see [`InstanceConfig`].
    #[must_use]
    pub fn new(config: InstanceConfig, clock: C) -> Self {
        let health = HealthSummary::healthy();
        let priority = health::evaluate(config.priority, &config.health, &health);
        Self {
            role: Role::Init,
            generation: Generation::initial(),
            timers: TimerSet::new(),
            health,
            priority,
            interface_up: true,
            pending: None,
            owns_addresses: false,
            retry_attempt: 0,
            advertise_failures: 0,
            preemption_armed_for: None,
            deferred_advertisement: None,
            shutting_down: false,
            master_advert_interval: config.advertisement_interval,
            last_reason: TransitionReason::Startup,
            last_peer: None,
            config,
            clock,
        }
    }

    // ----- accessors -------------------------------------------------------

    /// Returns the instance configuration.
    #[must_use]
    pub fn config(&self) -> &InstanceConfig {
        &self.config
    }

    /// Returns the clock the machine reads.
    ///
    /// Exposed so that a test or an executor can advance a fake clock without
    /// the machine knowing which implementation it holds.
    #[must_use]
    pub fn clock(&self) -> &C {
        &self.clock
    }

    /// Returns the current role.
    #[must_use]
    pub fn role(&self) -> Role {
        self.role
    }

    /// Returns the active configuration generation.
    #[must_use]
    pub fn generation(&self) -> Generation {
        self.generation
    }

    /// Returns the current effective priority and eligibility.
    #[must_use]
    pub fn priority(&self) -> PriorityState {
        self.priority
    }

    /// Returns the effective priority.
    #[must_use]
    pub fn effective_priority(&self) -> u8 {
        self.priority.effective
    }

    /// Returns the aggregate health the machine has been told about.
    #[must_use]
    pub fn health(&self) -> HealthSummary {
        self.health
    }

    /// Returns the timers owned by this instance.
    #[must_use]
    pub fn timers(&self) -> &TimerSet {
        &self.timers
    }

    /// Returns the deadline of `timer`, when it is armed.
    #[must_use]
    pub fn deadline_of(&self, timer: TimerId) -> Option<Duration> {
        self.timers.deadline(timer)
    }

    /// Returns `true` when `timer` is armed and has expired.
    #[must_use]
    pub fn is_due(&self, timer: TimerId) -> bool {
        self.timers.is_due(timer, self.clock.now())
    }

    /// Returns `true` when the VIPs are present in the kernel.
    #[must_use]
    pub fn owns_virtual_addresses(&self) -> bool {
        self.owns_addresses
    }

    /// Returns the work in flight, for the status API.
    #[must_use]
    pub fn pending(&self) -> PendingOwnership {
        match self.pending {
            None => PendingOwnership::None,
            Some(Pending::Acquire { .. }) => PendingOwnership::Acquiring,
            Some(Pending::Release { .. }) => PendingOwnership::Releasing,
        }
    }

    /// Returns `true` when the instance may take ownership.
    ///
    /// Eligibility is a property of health and of the interface, not of the
    /// role: a faulted instance with a usable interface must be able to attempt
    /// ownership again, which is what the retry timer is for.
    #[must_use]
    pub fn is_eligible(&self) -> bool {
        self.priority.eligible && self.interface_up
    }

    /// Returns `true` once shutdown has begun, after which the instance makes no
    /// further ownership change (`I-32`).
    #[must_use]
    pub fn is_shutting_down(&self) -> bool {
        self.shutting_down
    }

    /// Returns `true` when the instance currently transmits advertisements.
    #[must_use]
    pub fn is_armed_for_advertising(&self) -> bool {
        self.role == Role::Master && self.timers.is_armed(TimerId::Advertisement)
    }

    /// Returns the reason for the most recent transition.
    #[must_use]
    pub fn last_reason(&self) -> TransitionReason {
        self.last_reason
    }

    /// Returns the peer that most recently changed the instance's state.
    #[must_use]
    pub fn last_peer(&self) -> Option<IpAddr> {
        self.last_peer
    }

    /// Returns the priority of the peer that armed preemption, if any.
    #[must_use]
    pub fn preemption_target(&self) -> Option<u8> {
        self.preemption_armed_for
    }

    /// Returns the interval the current master claims, as learned from its
    /// advertisements (RFC 5798 §6.1, `Master_Adver_Interval`).
    ///
    /// It starts as this node's own configured interval and is replaced whenever
    /// a valid advertisement from a peer of equal or greater priority is
    /// accepted, which is what makes the takeover delay follow the master rather
    /// than this node's configuration.
    #[must_use]
    pub fn master_advert_interval(&self) -> Duration {
        self.master_advert_interval
    }

    /// Returns the candidate this instance presents in an election.
    #[must_use]
    pub fn candidate(&self) -> Candidate {
        let address = self
            .config
            .primary_ipv4
            .map_or_else(
                || self.config.primary_ipv6.map(IpAddr::V6),
                |address| Some(IpAddr::V4(address)),
            )
            .unwrap_or_else(|| IpAddr::from([0, 0, 0, 0]));
        Candidate::new(self.priority.effective, address)
    }

    /// Sets the interface's primary addresses, which the tie-break compares.
    ///
    /// A machine that does not know its own address cannot resolve an equal
    /// priority, and two nodes that start together always tie.
    pub fn set_primary_addresses(
        &mut self,
        ipv4: Option<std::net::Ipv4Addr>,
        ipv6: Option<std::net::Ipv6Addr>,
    ) {
        self.config.primary_ipv4 = ipv4;
        self.config.primary_ipv6 = ipv6;
    }

    /// Replaces the configuration, as a reload does.
    ///
    /// A reload never changes the role by itself; it changes the parameters
    /// used by the next decision (`I-10`).
    pub fn reconfigure(&mut self, config: InstanceConfig, generation: Generation) {
        if config.advertisement_interval != self.config.advertisement_interval {
            // The learned interval started as the local one, so a change to the
            // local interval invalidates it until the next advertisement.
            self.master_advert_interval = config.advertisement_interval;
        }
        self.config = config;
        self.generation = generation;
        self.priority = health::evaluate(self.config.priority, &self.config.health, &self.health);
    }

    // ----- event handling --------------------------------------------------

    /// Handles one event and returns the actions the executor must apply.
    ///
    /// The machine performs no I/O, so this function is total and deterministic
    /// for a given clock reading and event sequence (SPEC.md, `R-26`).
    // The event is taken by value because it is consumed by the match below.
    #[allow(clippy::needless_pass_by_value)]
    #[must_use]
    pub fn handle(&mut self, event: Event) -> Vec<Action> {
        let mut actions = Vec::new();
        match event {
            Event::Startup | Event::InterfaceUp | Event::InterfaceBroughtUp => {
                self.on_startup(&mut actions);
            }
            // Routed through the timer path so that it is ignored unless the
            // startup delay is actually armed, which stops a stale delivery
            // from resetting the role of a running instance (`I-30`).
            Event::StartupDelayElapsed => self.on_timer(TimerId::StartupDelay, &mut actions),
            Event::InterfaceDown | Event::CarrierLost => self.on_interface_down(&mut actions),
            Event::AdvertisementReceived(advertisement) => {
                self.on_advertisement(advertisement, &mut actions);
            }
            Event::AdvertisementTimeout => self.on_master_down(&mut actions),
            Event::HealthChanged(summary) => self.on_health_changed(summary, &mut actions),
            Event::TimerExpired(timer) => self.on_timer(timer, &mut actions),
            Event::ActionSucceeded { kind } => self.on_action_succeeded(kind, &mut actions),
            Event::ActionFailed { kind, error } => {
                self.on_action_failed(kind, &error, &mut actions);
            }
            Event::OperatorRelinquishRequested => {
                self.on_relinquish(TransitionReason::OperatorRelinquish, true, &mut actions);
            }
            Event::OperatorPauseRequested => self.pause(&mut actions),
            Event::OperatorResumeRequested => self.resume(&mut actions),
            Event::OperatorForceTransitionRequested { target, reason } => {
                self.force_transition(target, &reason, &mut actions);
            }
            Event::ConfigurationReloaded { generation } => {
                self.on_reload(generation, &mut actions);
            }
            Event::ShutdownRequested => self.on_shutdown(&mut actions),
        }
        actions
    }

    // ----- startup and interface ------------------------------------------

    fn on_startup(&mut self, actions: &mut Vec<Action>) {
        self.interface_up = true;
        match self.role {
            Role::Backup | Role::Master => {
                // The interface came back under a participating role. Re-arm a
                // full master-down interval rather than taking over at once.
                self.rearm_master_down(actions);
            }
            Role::Fault => {
                if self.timers.is_armed(TimerId::HoldDown) {
                    // Re-verification waits for the hold-down to expire
                    // (`I-21`).
                    log(actions, LogLevel::Debug, "interface up during hold-down");
                } else {
                    self.begin_startup(actions);
                }
            }
            Role::Disabled => log(actions, LogLevel::Debug, "interface up while disabled"),
            Role::Init => self.begin_startup(actions),
        }
    }

    fn begin_startup(&mut self, actions: &mut Vec<Action>) {
        if self.pending.is_some() {
            return;
        }
        if !self.config.startup_delay.is_zero() {
            self.arm(TimerId::StartupDelay, self.config.startup_delay, actions);
            return;
        }
        self.enter_election(actions);
    }

    fn enter_election(&mut self, actions: &mut Vec<Action>) {
        if self.pending.is_some() {
            return;
        }
        self.cancel(TimerId::StartupDelay, actions);
        self.rearm_master_down(actions);
        self.transition(Role::Backup, TransitionReason::Startup, actions);
        actions.push(Action::SetEffectivePriority {
            priority: self.priority.effective,
        });
    }

    fn on_interface_down(&mut self, actions: &mut Vec<Action>) {
        self.interface_up = false;
        self.cancel(TimerId::Advertisement, actions);
        self.cancel(TimerId::MasterDown, actions);
        self.cancel(TimerId::PreemptionDelay, actions);
        self.preemption_armed_for = None;

        match self.role {
            Role::Master => {
                // Advertising stops immediately; the role changes only once
                // removal is confirmed, because a non-master role must not own
                // addresses (`I-14`).
                self.request_release(TransitionReason::InterfaceDown, Role::Fault, actions);
            }
            Role::Init | Role::Backup | Role::Fault => {
                log(actions, LogLevel::Warn, "interface is down");
            }
            Role::Disabled => {}
        }
    }

    /// Arms the master-down timer from the current `Master_Adver_Interval` and
    /// this node's effective priority.
    ///
    /// The interval has been validated by `V-04`, so the saturating form of the
    /// RFC's formula is used: overflow is unreachable, and a fault would be worse
    /// than a long timer.
    fn rearm_master_down(&mut self, actions: &mut Vec<Action>) {
        let interval =
            master_down_interval_saturating(self.priority.effective, self.master_advert_interval);
        self.arm(TimerId::MasterDown, interval, actions);
    }

    // ----- advertisements --------------------------------------------------

    fn on_advertisement(&mut self, advertisement: PeerAdvertisement, actions: &mut Vec<Action>) {
        if advertisement.vrid != self.config.vrid {
            log(actions, LogLevel::Warn, "advertisement for another VRID");
            return;
        }
        self.last_peer = Some(advertisement.source);

        if self.pending.is_some() {
            // Ownership is in flight. Deferring the advertisement and replaying
            // it afterwards is what prevents a late advertisement from being
            // silently lost during a takeover.
            self.deferred_advertisement = Some(advertisement);
            return;
        }

        match self.role {
            Role::Master => self.on_advertisement_as_master(&advertisement, actions),
            Role::Backup | Role::Init => self.on_advertisement_as_backup(&advertisement, actions),
            Role::Fault | Role::Disabled => log(actions, LogLevel::Debug, "advertisement ignored"),
        }
    }

    /// What a master does when a peer advertises.
    ///
    /// A strictly higher priority always wins (`R-16`). An *equal* priority is
    /// the interesting case, and it is the one two nodes that start together
    /// produce: both time out at the same instant and both advertise. The
    /// specification resolves it by address (§12.3), so the same rule that
    /// `election::decide` documents decides here, rather than a second
    /// comparison invented next to it.
    fn on_advertisement_as_master(
        &mut self,
        advertisement: &PeerAdvertisement,
        actions: &mut Vec<Action>,
    ) {
        let steps_down = election::must_step_down(self.priority.effective, advertisement.priority)
            || self.peer_wins_an_equal_priority(advertisement);

        if steps_down {
            self.last_reason = TransitionReason::HigherPriorityPeerAdvertisement;
            self.request_release(
                TransitionReason::HigherPriorityPeerAdvertisement,
                Role::Backup,
                actions,
            );
        }
        // A lower priority is expected traffic from a peer that has not yet
        // learned about this node's role, and needs no action.
    }

    /// Returns `true` when the peer wins a tie at equal priority.
    fn peer_wins_an_equal_priority(&self, advertisement: &PeerAdvertisement) -> bool {
        if advertisement.priority != self.priority.effective {
            return false;
        }
        let ours = self.candidate();
        let theirs = election::Candidate {
            priority: advertisement.priority,
            address: advertisement.source,
            peer_set: Vec::new(),
        };
        election::decide(&ours, &theirs, election::Incumbent::Local).winner
            == election::Winner::Remote
    }

    /// RFC 5798 §6.4.2: what a backup does with a valid advertisement.
    ///
    /// - Priority zero means the master is stopping, and the takeover delay
    ///   becomes `Skew_Time` rather than a full interval.
    /// - An advertisement from a peer of equal or greater priority resets the
    ///   master-down timer, and its interval is learned.
    /// - An advertisement from a lower-priority peer is **discarded**: neither
    ///   the timer nor the learned interval changes. This is deliberate. A
    ///   higher-priority node that extended the incumbent's lease would keep a
    ///   lower-priority master alive indefinitely, and the two would never
    ///   converge. Highland additionally arms its own preemption timer, which is
    ///   what turns the discard into an actual takeover; with a preemption delay
    ///   of zero that takeover is immediate, which is Keepalived's behavior.
    fn on_advertisement_as_backup(
        &mut self,
        advertisement: &PeerAdvertisement,
        actions: &mut Vec<Action>,
    ) {
        if advertisement.is_relinquish() {
            self.cancel(TimerId::PreemptionDelay, actions);
            self.preemption_armed_for = None;
            let delay = skew_time(self.priority.effective, self.master_advert_interval);
            self.arm(TimerId::MasterDown, delay, actions);
            return;
        }

        if advertisement.priority >= self.priority.effective || !self.config.preempt {
            self.master_advert_interval = advertisement.advert_interval;
            self.rearm_master_down(actions);
            self.cancel(TimerId::PreemptionDelay, actions);
            self.preemption_armed_for = None;
            return;
        }

        // Discard: the interval is not learned and the timer is not reset.
        if !self.timers.is_armed(TimerId::PreemptionDelay) {
            self.arm(TimerId::PreemptionDelay, self.config.preempt_delay, actions);
            self.preemption_armed_for = Some(advertisement.priority);
        }
    }

    fn on_master_down(&mut self, actions: &mut Vec<Action>) {
        if self.role != Role::Backup {
            return;
        }
        if !self.is_eligible() {
            log(
                actions,
                LogLevel::Debug,
                "master down but the instance is not eligible",
            );
            self.rearm_master_down(actions);
            return;
        }
        self.request_ownership(TransitionReason::MasterDownTimeout, actions);
    }

    // ----- health ----------------------------------------------------------

    fn on_health_changed(&mut self, summary: HealthSummary, actions: &mut Vec<Action>) {
        self.health = summary;
        self.priority = health::evaluate(self.config.priority, &self.config.health, &self.health);

        if !self.priority.eligible {
            self.cancel(TimerId::PreemptionDelay, actions);
            self.preemption_armed_for = None;
            if self.role == Role::Master {
                let announce_zero =
                    health::requires_immediate_relinquish(&self.config.health, &self.health)
                        && self.config.health.send_zero_priority_advert;
                if announce_zero {
                    actions.push(Action::SendAdvertisement { priority: 0 });
                } else {
                    // A node whose effective priority reached zero relinquishes
                    // silently; zero is never sent as a normal advertisement
                    // (`I-23`, `I-27`).
                    log(actions, LogLevel::Warn, "effective priority reached zero");
                }
                self.request_release(TransitionReason::HealthIneligible, Role::Backup, actions);
            }
            return;
        }

        actions.push(Action::SetEffectivePriority {
            priority: self.priority.effective,
        });
    }

    // ----- timers ----------------------------------------------------------

    fn on_timer(&mut self, timer: TimerId, actions: &mut Vec<Action>) {
        if !self.timers.is_armed(timer) {
            // `I-30`: a cancelled timer is never delivered, so a stale event
            // cannot revive an instance that has stopped participating.
            return;
        }
        self.cancel(timer, actions);
        match timer {
            TimerId::StartupDelay => self.enter_election(actions),
            TimerId::Advertisement => {
                if self.role == Role::Master && self.owns_addresses {
                    actions.push(Action::SendAdvertisement {
                        priority: self.priority.effective,
                    });
                    // The advertisement timer is periodic, so it is armed again
                    // here rather than only at takeover. A master that
                    // advertised once and then went quiet is indistinguishable
                    // from a dead one to its peers: they would time it out and
                    // take the address, and it would take it back on its next
                    // master-down timer. The address would then move every
                    // `Master_Down_Interval`, which is worse than an outage
                    // because every client sees it.
                    self.arm(
                        TimerId::Advertisement,
                        self.config.advertisement_interval,
                        actions,
                    );
                }
            }
            TimerId::MasterDown => self.on_master_down(actions),
            TimerId::PreemptionDelay => {
                self.preemption_armed_for = None;
                if self.role == Role::Backup && self.is_eligible() {
                    self.request_ownership(TransitionReason::Preemption, actions);
                }
            }
            TimerId::HoldDown => {
                // The hold-down has expired. Re-verification is the interface
                // state, which the executor has already reported (`I-21`).
                if self.interface_up {
                    self.begin_startup(actions);
                } else {
                    log(
                        actions,
                        LogLevel::Debug,
                        "hold-down expired but the interface is down",
                    );
                }
            }
            TimerId::Retry => self.on_retry(actions),
        }
    }

    fn on_retry(&mut self, actions: &mut Vec<Action>) {
        if !self.config.retry.permits(self.retry_attempt) {
            log(
                actions,
                LogLevel::Error,
                "retry budget exhausted; operator action required",
            );
            return;
        }
        self.retry_attempt = self.retry_attempt.saturating_add(1);
        match self.role {
            Role::Fault if self.is_eligible() && self.interface_up => {
                if self.last_reason == TransitionReason::InterfaceDown {
                    // The instance was *away*, not unable to add an address. It
                    // has no idea what happened on the segment while its link was
                    // down, so it goes back to listening for a full
                    // `Master_Down_Interval` rather than taking the address on a
                    // retry backoff. Taking it here is a split brain: the peer has
                    // been master for as long as this node was gone, and nothing
                    // here would find out. The link-flap scenario in
                    // `tests/chaos.rs` is what found this.
                    log(
                        actions,
                        LogLevel::Debug,
                        "retry after an interface failure: returning to the election",
                    );
                    self.begin_startup(actions);
                } else {
                    // A local failure, such as an address that would not be
                    // added: re-attempt the same acquisition, which is what the
                    // hold-down bounds (`I-21`) and what makes a bounded number
                    // of attempts meaningful (`R-12`).
                    self.request_ownership(TransitionReason::OwnershipFailed, actions);
                }
            }
            Role::Master if !self.owns_addresses => {
                self.request_release(TransitionReason::OwnershipFailed, Role::Fault, actions);
            }
            _ => log(
                actions,
                LogLevel::Debug,
                "retry timer fired with nothing to retry",
            ),
        }
    }

    // ----- executor outcomes ----------------------------------------------

    fn on_action_succeeded(&mut self, kind: ActionKind, actions: &mut Vec<Action>) {
        match kind {
            ActionKind::AddAddresses => self.on_ownership_confirmed(actions),
            ActionKind::RemoveAddresses => self.on_release_confirmed(actions),
            ActionKind::Advertisement => self.advertise_failures = 0,
            _ => {}
        }
    }

    fn on_action_failed(&mut self, kind: ActionKind, error: &str, actions: &mut Vec<Action>) {
        actions.push(Action::Log {
            level: LogLevel::Warn,
            message: format!("{} failed: {error}", kind_name(kind)),
        });
        actions.push(Action::EmitEvent {
            name: "action_failed",
        });

        match kind {
            ActionKind::AddAddresses => {
                self.owns_addresses = false;
                self.pending = None;
                // Clean up any partially added addresses before faulting
                // (`I-20`).
                actions.push(Action::RemoveVirtualAddresses);
                self.enter_fault(TransitionReason::OwnershipFailed, actions);
            }
            ActionKind::RemoveAddresses => {
                // The role stays `MASTER` because the addresses are still
                // present, and `I-14` forbids a non-master role from owning
                // them. Removal is retried with backoff.
                actions.push(Action::EmitEvent {
                    name: "ownership_release_failed",
                });
                self.schedule_retry(actions);
            }
            ActionKind::Advertisement => self.on_advertisement_failure(actions),
            ActionKind::Timer | ActionKind::SetPriority | ActionKind::EnterRole => {
                self.enter_fault(TransitionReason::OwnershipFailed, actions);
            }
            ActionKind::GratuitousUpdate | ActionKind::EmitEvent | ActionKind::Log => {
                log(actions, LogLevel::Debug, "non-critical action failed");
            }
        }
    }

    fn on_advertisement_failure(&mut self, actions: &mut Vec<Action>) {
        self.advertise_failures = self.advertise_failures.saturating_add(1);
        if self.advertise_failures < ADVERTISE_FAILURE_LIMIT {
            return;
        }

        // `ADVERTISE_FAILURE_LIMIT` failures, however far apart, are enough.
        // The streak counter is only cleared by a success, a role change, or
        // an ownership confirmation, so it cannot reach the limit again while
        // this call keeps returning. Counting failures "within
        // ADVERTISE_FAILURE_WINDOW", as the constant's doc comment says, would
        // need a rolling window this struct does not keep, and the previous
        // attempt at one anchored on the first failure of the streak: once
        // that instant aged past the window the guard was permanently false,
        // so a master that kept failing to advertise stayed master forever,
        // advertising into a black hole.
        self.request_release(TransitionReason::OwnershipFailed, Role::Fault, actions);
    }

    // ----- ownership -------------------------------------------------------

    fn request_ownership(&mut self, reason: TransitionReason, actions: &mut Vec<Action>) {
        if self.shutting_down {
            // `I-32`: after shutdown begins, the daemon never adds an address.
            log(actions, LogLevel::Debug, "ownership refused: shutting down");
            return;
        }
        if self.pending.is_some() || self.role == Role::Master {
            return;
        }
        if !self.is_eligible() || !self.interface_up {
            log(
                actions,
                LogLevel::Debug,
                "ownership requested while not eligible",
            );
            return;
        }

        self.cancel(TimerId::MasterDown, actions);
        self.cancel(TimerId::PreemptionDelay, actions);
        self.preemption_armed_for = None;
        self.cancel(TimerId::HoldDown, actions);
        self.pending = Some(Pending::Acquire { reason });
        self.last_reason = reason;

        actions.push(Action::EmitEvent {
            name: "ownership_acquiring",
        });
        actions.push(Action::AddVirtualAddresses);
    }

    fn request_release(&mut self, reason: TransitionReason, next: Role, actions: &mut Vec<Action>) {
        // Advertising stops before removal begins, so a peer never sees a
        // relinquishing node that still claims the address.
        self.cancel(TimerId::Advertisement, actions);
        if !self.owns_addresses {
            self.on_release_confirmed_with(reason, next, actions);
            return;
        }
        self.pending = Some(Pending::Release { reason, next });
        self.last_reason = reason;
        actions.push(Action::RemoveVirtualAddresses);
    }

    fn on_ownership_confirmed(&mut self, actions: &mut Vec<Action>) {
        let Some(Pending::Acquire { reason }) = self.pending.take() else {
            return;
        };

        if !self.is_eligible() {
            // Health or the interface changed while the addresses were being
            // added. The addresses are now present, so the role is corrected to
            // `MASTER` rather than left claiming otherwise (`I-14`); nothing is
            // advertised, because advertising needs a usable interface, and
            // ownership is released at once.
            self.owns_addresses = true;
            self.transition(Role::Master, reason, actions);
            log(
                actions,
                LogLevel::Warn,
                "ownership confirmed after ineligibility; releasing",
            );
            self.request_release(TransitionReason::HealthIneligible, Role::Backup, actions);
            return;
        }

        self.owns_addresses = true;
        self.retry_attempt = 0;
        self.advertise_failures = 0;

        actions.push(Action::SendGratuitousUpdates);
        self.transition(Role::Master, reason, actions);
        actions.push(Action::SetEffectivePriority {
            priority: self.priority.effective,
        });
        self.arm(
            TimerId::Advertisement,
            self.config.advertisement_interval,
            actions,
        );
        actions.push(Action::SendAdvertisement {
            priority: self.priority.effective,
        });

        // A late advertisement that arrived during the takeover is replayed
        // now, so that a peer which outranks this node still wins (`R-16`).
        if let Some(advertisement) = self.deferred_advertisement.take() {
            self.on_advertisement(advertisement, actions);
        }
    }

    fn on_release_confirmed(&mut self, actions: &mut Vec<Action>) {
        let Some(Pending::Release { reason, next }) = self.pending.take() else {
            return;
        };
        self.on_release_confirmed_with(reason, next, actions);
    }

    fn on_release_confirmed_with(
        &mut self,
        reason: TransitionReason,
        next: Role,
        actions: &mut Vec<Action>,
    ) {
        self.owns_addresses = false;
        self.pending = None;
        self.advertise_failures = 0;

        if next == Role::Fault {
            self.enter_fault(reason, actions);
            return;
        }

        self.transition(next, reason, actions);
        if next == Role::Backup {
            self.rearm_master_down(actions);
        }
    }

    fn enter_fault(&mut self, reason: TransitionReason, actions: &mut Vec<Action>) {
        if self.owns_addresses {
            // `I-14`: an instance that owns addresses cannot be `FAULT`, so
            // ownership is released first and the fault is entered once removal
            // is confirmed.
            self.request_release(reason, Role::Fault, actions);
            return;
        }
        self.abandon_acquisition(actions);
        self.timers.cancel_all();
        for timer in [
            TimerId::Advertisement,
            TimerId::MasterDown,
            TimerId::PreemptionDelay,
            TimerId::StartupDelay,
            TimerId::Retry,
        ] {
            actions.push(Action::CancelTimer { timer });
        }
        self.preemption_armed_for = None;
        self.transition(Role::Fault, reason, actions);
        self.arm(TimerId::HoldDown, self.config.hold_down, actions);
        self.schedule_retry(actions);
    }

    fn schedule_retry(&mut self, actions: &mut Vec<Action>) {
        if self.shutting_down || self.role == Role::Disabled {
            // An instance that has stopped participating, or a daemon that is
            // stopping, must not be kept alive by a retry timer.
            log(
                actions,
                LogLevel::Debug,
                "retry not scheduled: not participating",
            );
            return;
        }
        let attempt = self.retry_attempt.saturating_add(1);
        let Some(delay) = self.config.retry.delay_for(attempt) else {
            log(
                actions,
                LogLevel::Error,
                "retry budget exhausted; operator action required",
            );
            return;
        };
        self.retry_attempt = attempt;
        self.arm(TimerId::Retry, delay, actions);
    }

    // ----- operator and lifecycle ------------------------------------------

    fn on_relinquish(
        &mut self,
        reason: TransitionReason,
        announce_zero: bool,
        actions: &mut Vec<Action>,
    ) {
        if self.role == Role::Master {
            if announce_zero {
                actions.push(Action::SendAdvertisement { priority: 0 });
            }
            self.request_release(reason, Role::Backup, actions);
            return;
        }
        log(
            actions,
            LogLevel::Debug,
            "relinquish requested while not master",
        );
    }

    fn pause(&mut self, actions: &mut Vec<Action>) {
        if self.role == Role::Master {
            self.request_release(
                TransitionReason::OperatorRelinquish,
                Role::Disabled,
                actions,
            );
            return;
        }
        self.timers.cancel_all();
        self.abandon_acquisition(actions);
        self.transition(
            Role::Disabled,
            TransitionReason::OperatorRelinquish,
            actions,
        );
    }

    fn resume(&mut self, actions: &mut Vec<Action>) {
        match self.role {
            Role::Disabled => {
                self.timers.cancel_all();
                self.transition(Role::Init, TransitionReason::ConfigurationReloaded, actions);
                self.begin_startup(actions);
            }
            Role::Fault => {
                self.cancel(TimerId::HoldDown, actions);
                self.begin_startup(actions);
            }
            _ => log(
                actions,
                LogLevel::Debug,
                "resume requested while already participating",
            ),
        }
    }

    fn force_transition(&mut self, target: Role, reason: &str, actions: &mut Vec<Action>) {
        actions.push(Action::EmitEvent {
            name: "operator_action",
        });
        match target {
            Role::Master => {
                if self.role == Role::Backup {
                    self.request_ownership(TransitionReason::OperatorForceTransition, actions);
                }
            }
            Role::Backup => {
                if self.role == Role::Master {
                    actions.push(Action::SendAdvertisement { priority: 0 });
                    self.request_release(
                        TransitionReason::OperatorRelinquish,
                        Role::Backup,
                        actions,
                    );
                }
            }
            Role::Disabled => self.pause(actions),
            Role::Fault => self.enter_fault(TransitionReason::OwnershipFailed, actions),
            Role::Init => {
                // `I-14`: a forced return to `INIT` releases the addresses
                // exactly as every other role change does. This arm used to set
                // `owns_addresses = false` and clear `pending` on its own, which
                // told the executor nothing: the advertisement timer was
                // cancelled, so the node went silent, but the virtual address
                // stayed on the interface. A peer then timed out, added the
                // same address, and both nodes believed they owned it.
                // `abandon_acquisition` covers the case where the instance was
                // still acquiring, so a late add-success cannot leak either.
                self.cancel(TimerId::Advertisement, actions);
                if self.owns_addresses || self.pending.is_some() {
                    actions.push(Action::RemoveVirtualAddresses);
                }
                self.owns_addresses = false;
                self.advertise_failures = 0;
                self.abandon_acquisition(actions);
                self.timers.cancel_all();
                self.preemption_armed_for = None;
                self.transition(
                    Role::Init,
                    TransitionReason::OperatorForceTransition,
                    actions,
                );
            }
        }
        log(
            actions,
            LogLevel::Warn,
            &format!("forced transition: {reason}"),
        );
    }

    fn on_reload(&mut self, generation: Generation, actions: &mut Vec<Action>) {
        if generation < self.generation {
            // A result from a superseded configuration is discarded (`I-12`).
            log(
                actions,
                LogLevel::Debug,
                "reload from an older generation ignored",
            );
            return;
        }
        self.generation = generation;
        actions.push(Action::EmitEvent {
            name: "reload_accepted",
        });
        if self.role == Role::Disabled {
            self.timers.cancel_all();
            self.transition(Role::Init, TransitionReason::ConfigurationReloaded, actions);
            self.begin_startup(actions);
        }
    }

    fn on_shutdown(&mut self, actions: &mut Vec<Action>) {
        if self.shutting_down {
            // A second signal is ignored; the sequence is already running
            // (`I-31`).
            log(actions, LogLevel::Debug, "shutdown already in progress");
            return;
        }
        self.shutting_down = true;
        actions.push(Action::EmitEvent {
            name: "daemon_lifecycle",
        });
        if self.role == Role::Master {
            actions.push(Action::SendAdvertisement { priority: 0 });
            self.request_release(TransitionReason::Shutdown, Role::Backup, actions);
            return;
        }
        self.timers.cancel_all();
        self.abandon_acquisition(actions);
    }

    /// Cancels an in-flight ownership request.
    ///
    /// The executor may already have added some addresses when the request was
    /// cancelled, so a best-effort removal is still owed. Without this, a late
    /// success confirmation would promote the instance to `MASTER` after
    /// shutdown had begun.
    fn abandon_acquisition(&mut self, actions: &mut Vec<Action>) {
        if matches!(self.pending.take(), Some(Pending::Acquire { .. })) {
            actions.push(Action::Log {
                level: LogLevel::Warn,
                message: "ownership request cancelled; removing any addresses that were added"
                    .to_owned(),
            });
            actions.push(Action::RemoveVirtualAddresses);
        }
    }

    // ----- helpers ---------------------------------------------------------

    fn transition(&mut self, role: Role, reason: TransitionReason, actions: &mut Vec<Action>) {
        if self.role == role && role != Role::Backup {
            return;
        }
        self.role = role;
        self.last_reason = reason;
        actions.push(Action::EnterRole { role, reason });
        actions.push(Action::EmitEvent {
            name: "role_transition",
        });
    }

    fn arm(&mut self, timer: TimerId, delay: Duration, actions: &mut Vec<Action>) {
        let deadline = self.clock.now() + delay;
        self.timers.arm(timer, deadline);
        actions.push(Action::ArmTimer { timer, deadline });
    }

    fn cancel(&mut self, timer: TimerId, actions: &mut Vec<Action>) {
        if self.timers.is_armed(timer) {
            self.timers.cancel(timer);
            actions.push(Action::CancelTimer { timer });
        }
    }
}

/// Returns `3 * Master_Adver_Interval + Skew_Time`, saturating instead of
/// overflowing. See [`InstanceStateMachine::rearm_master_down`].
fn master_down_interval_saturating(
    local_priority: u8,
    master_advert_interval: Duration,
) -> Duration {
    master_advert_interval
        .saturating_mul(3)
        .saturating_add(skew_time(local_priority, master_advert_interval))
}

fn log(actions: &mut Vec<Action>, level: LogLevel, message: &str) {
    actions.push(Action::Log {
        level,
        message: message.to_owned(),
    });
}

fn kind_name(kind: ActionKind) -> &'static str {
    match kind {
        ActionKind::Timer => "timer",
        ActionKind::Advertisement => "advertisement",
        ActionKind::AddAddresses => "add_virtual_addresses",
        ActionKind::RemoveAddresses => "remove_virtual_addresses",
        ActionKind::GratuitousUpdate => "gratuitous_update",
        ActionKind::SetPriority => "set_effective_priority",
        ActionKind::EnterRole => "enter_role",
        ActionKind::EmitEvent => "emit_event",
        ActionKind::Log => "log",
    }
}
