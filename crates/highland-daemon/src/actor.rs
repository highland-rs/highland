// Rust guideline compliant 2026-09-27

//! The per-instance actor.
//!
//! One actor owns one state machine, one executor, and one set of timers. It is
//! the only thing that turns an event into a role change, and the only thing
//! that turns a role change into a kernel mutation. Everything it does is
//! `handle` followed by `apply`, twice, because an action's outcome is an event
//! that the machine must see (`I-45`).
//!
//! # Determinism
//!
//! The actor takes its time from a [`Clock`], not from the runtime, so a test
//! drives the whole failover with a `ManualClock` and no sleeping. Timers are
//! absolute deadlines for the same reason (`R-27`).

use std::sync::Arc;

use highland_core::clock::Clock;
use highland_core::machine::{InstanceStateMachine, PendingOwnership};
use highland_core::state::Generation;
use highland_core::state::{Action, Event, Role, TimerId};
use highland_net::NetworkBackend;
use highland_vrrp::Advertisement;

use crate::executor::{Executor, Ownership, Transport};
use crate::options::InstancePlan;

/// What one applied event did, for a caller that wants to log or count it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    /// The role the instance occupies afterwards.
    pub role: Role,
    /// The ownership state afterwards.
    pub ownership: PendingOwnership,
    /// The actions the machine requested.
    pub actions: Vec<Action>,
}

/// The most rounds one event may take before the machine is judged not to
/// settle. A healthy instance settles in three: request ownership, confirm,
/// advertise.
const MAX_ROUNDS: usize = 16;

/// One instance's event loop.
#[derive(Debug)]
pub struct InstanceActor<C, B, T> {
    machine: InstanceStateMachine<C>,
    executor: Executor<B, T>,
    metrics: Option<Arc<crate::Metrics>>,
    log: Option<Arc<crate::EventLog>>,
    node: String,
    previous_role: highland_core::state::Role,
}

impl<C, B, T> InstanceActor<C, B, T>
where
    C: Clock,
    B: NetworkBackend,
    T: Transport,
{
    /// Creates an actor for `plan`, owning `addresses` on `ownership`'s
    /// interface.
    #[must_use]
    pub fn new(
        clock: C,
        plan: InstancePlan,
        ownership: Ownership,
        backend: Arc<B>,
        transport: Arc<T>,
    ) -> Self {
        let machine_config = highland_core::state::InstanceConfig {
            name: plan.name.clone(),
            vrid: plan.vrid,
            priority: plan.priority,
            advertisement_interval: plan.advertisement_interval,
            startup_delay: plan.startup_delay,
            preempt: plan.preempt,
            preempt_delay: plan.preempt_delay,
            ..highland_core::state::InstanceConfig::default()
        };
        let machine = InstanceStateMachine::new(machine_config, clock);
        let executor = Executor::new(backend, transport, plan, ownership);
        Self {
            machine,
            executor,
            metrics: None,
            log: None,
            node: String::new(),
            previous_role: highland_core::state::Role::Init,
        }
    }

    /// Attaches the node name and the event history this actor reports to.
    #[must_use]
    pub fn with_events(mut self, node: impl Into<String>, log: Arc<crate::EventLog>) -> Self {
        self.node = node.into();
        self.log = Some(log);
        self
    }

    /// Attaches the metrics this actor reports to.
    #[must_use]
    pub fn with_metrics(mut self, metrics: Arc<crate::Metrics>) -> Self {
        self.executor.set_metrics(std::sync::Arc::clone(&metrics));
        self.metrics = Some(metrics);
        self
    }

    /// Returns the state machine, for inspection and for the status API.
    #[must_use]
    pub fn machine(&self) -> &InstanceStateMachine<C> {
        &self.machine
    }

    /// Reconfigures the instance in place, as a reload does.
    ///
    /// The role, the ownership, and the armed timers survive: a reload changes
    /// the parameters used by the next decision, not the current state
    /// (`R-48`). A generation older than the one the instance already runs is
    /// refused, so a reload that arrives out of order cannot undo a newer one
    /// (`I-12`).
    pub fn reconfigure(
        &mut self,
        plan: crate::options::InstancePlan,
        ownership: crate::executor::Ownership,
        generation: Generation,
    ) -> bool {
        use highland_core::state::InstanceConfig;

        if generation < self.machine.generation() {
            return false;
        }
        self.machine.reconfigure(
            InstanceConfig {
                name: plan.name.clone(),
                vrid: plan.vrid,
                priority: plan.priority,
                advertisement_interval: plan.advertisement_interval,
                startup_delay: plan.startup_delay,
                preempt: plan.preempt,
                preempt_delay: plan.preempt_delay,
                ..InstanceConfig::default()
            },
            generation,
        );
        self.executor.reconfigure(plan, ownership);
        true
    }

    /// Sets the interface's primary addresses, which the equal-priority
    /// tie-break compares (`SPEC.md` §12.3).
    pub fn set_primary_addresses(
        &mut self,
        ipv4: Option<std::net::Ipv4Addr>,
        ipv6: Option<std::net::Ipv6Addr>,
    ) {
        self.machine.set_primary_addresses(ipv4, ipv6);
    }

    /// Returns the executor, for inspection and for the status API.
    #[must_use]
    pub fn executor(&self) -> &Executor<B, T> {
        &self.executor
    }

    /// Returns the role the instance occupies.
    #[must_use]
    pub fn role(&self) -> Role {
        self.machine.role()
    }

    /// Handles one event and applies everything it produces, to exhaustion.
    ///
    /// Each action is applied and its outcome is fed straight back into the
    /// machine, which may itself produce more actions: confirming the addresses
    /// is what makes the machine enter `MASTER` and ask to advertise. So this is
    /// a worklist rather than a single pass, and applying only the first round
    /// would leave a master that owns its address and never says so.
    ///
    /// The worklist is drained rather than sampled, and the drain is bounded
    /// because a machine that keeps producing confirmed actions forever would
    /// otherwise spin.
    ///
    /// # Panics
    ///
    /// Panics if the machine produces actions for more than sixteen rounds
    /// without settling. That is a defect in the machine rather than a runtime
    /// condition, and a spinning actor would be far worse than a clear failure.
    pub async fn handle(&mut self, event: Event) -> Applied {
        self.trace_incoming(&event);

        let mut applied: Vec<Action> = self.machine.handle(event);
        let mut round = 0;
        let mut queue: Vec<Action> = applied.clone();

        while !queue.is_empty() {
            round += 1;
            assert!(
                round <= MAX_ROUNDS,
                "the machine produced actions without settling"
            );

            let mut produced = Vec::new();
            for action in queue.drain(..) {
                // Every role change is announced with its reason. A failover
                // that cannot be explained from the log is a failover nobody
                // can debug (`D-08`, `R-33`).
                if let Action::EnterRole { role, reason } = action {
                    if let Some(log) = &self.log {
                        // Recorded as it happens, so `highland events` is the
                        // history that occurred rather than a reconstruction.
                        log.record_transition(
                            &self.node,
                            &self.machine.config().name,
                            &self.previous_role.to_string(),
                            &role.to_string(),
                            &reason.to_string(),
                        );
                    }
                    tracing::info!(
                        instance = %self.machine.config().name,
                        role = %role,
                        reason = %reason,
                        effective_priority = self.machine.effective_priority(),
                        "role changed"
                    );
                    if let Some(metrics) = &self.metrics {
                        metrics.record_transition(
                            &self.machine.config().name,
                            &self.previous_role.to_string(),
                            &role.to_string(),
                        );
                        if reason == highland_core::state::TransitionReason::MasterDownTimeout {
                            metrics.record_master_down(&self.machine.config().name);
                        }
                        self.previous_role = role;
                    }
                }
                if let Some(outcome) = self.executor.apply(&action).await {
                    produced.extend(self.machine.handle(outcome));
                }
            }
            applied.extend(produced.clone());
            queue = produced;
        }

        Applied {
            role: self.machine.role(),
            ownership: self.machine.pending(),
            actions: applied,
        }
    }

    /// Whether the instance's interface is usable, as the kernel reports it.
    pub async fn interface_usable(&self) -> bool {
        self.executor.interface_usable().await
    }

    /// Records an event the instance received, for the log.
    fn trace_incoming(&self, event: &Event) {
        match event {
            Event::AdvertisementReceived(advertisement) => tracing::debug!(
                instance = %self.machine.config().name,
                peer = %advertisement.source,
                vrid = advertisement.vrid,
                priority = advertisement.priority,
                "advertisement received"
            ),
            Event::Startup => tracing::info!(
                instance = %self.machine.config().name,
                vrid = self.machine.config().vrid,
                priority = self.machine.config().priority,
                interval = ?self.machine.config().advertisement_interval,
                "instance starting"
            ),
            _ => {}
        }
    }

    /// Publishes a status snapshot for the control API and the metrics.
    ///
    /// The snapshot exists because the actor owns its state machine and a reader
    /// cannot ask the actor what its role is. Publishing after every event,
    /// rather than on request, is what keeps the registry and the machine from
    /// disagreeing about a role.
    #[must_use]
    pub fn publish_status(&self) -> crate::control::InstanceStatus {
        let machine = self.machine();
        crate::control::InstanceStatus {
            name: machine.config().name.clone(),
            role: machine.role().to_string(),
            priority: machine.config().priority,
            effective_priority: machine.effective_priority(),
            owns_addresses: machine.owns_virtual_addresses(),
            vip_addresses: self
                .executor
                .addresses()
                .iter()
                .map(ToString::to_string)
                .collect(),
            peers: self
                .executor
                .peers()
                .iter()
                .map(ToString::to_string)
                .collect(),
            last_reason: machine.last_reason().to_string(),
        }
    }

    /// Fires every timer whose deadline has passed, in a stable order.
    ///
    /// Returns what each firing did, so a test can assert the whole sequence.
    pub async fn fire_due_timers(&mut self) -> Vec<(TimerId, Applied)> {
        let now = self.machine.clock().now();
        let due = self.machine.timers().due(now);
        let mut fired = Vec::with_capacity(due.len());
        for timer in due {
            let applied = self.handle(Event::TimerExpired(timer)).await;
            fired.push((timer, applied));
        }
        fired
    }

    /// Builds the advertisement this instance would send, for a test or a
    /// diagnostic.
    pub fn advertisement(&self, priority: u8) -> Option<Advertisement> {
        self.executor.advertisement_for(priority)
    }
}
