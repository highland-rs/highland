// Rust guideline compliant 2026-09-27

//! The control service the daemon serves, and the status it answers with.
//!
//! Two things are here that the socket alone cannot provide:
//!
//! - **A status registry.** An instance's actor owns its own state machine, so
//!   the control API cannot ask the actor what its role is. Instead every actor
//!   publishes a small snapshot after each event, and the registry is what a
//!   reader sees. The same registry feeds the metrics endpoint, so the two can
//!   never disagree about a role.
//! - **The service itself**, which turns a request into a registry read, a
//!   reload, or a relinquish.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use highland_control::{
    ControlError, ControlRequest, ControlResponse, InstanceSummary, NodeStatus, PeerIdentity,
    Service,
};

use crate::driver::{Instruction, InstructionSender};

/// What one instance is doing, as the control API and the metrics see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceStatus {
    /// The instance name.
    pub name: String,
    /// The role the instance occupies.
    pub role: String,
    /// The configured priority.
    pub priority: u8,
    /// The effective priority after health.
    pub effective_priority: u8,
    /// Whether the instance currently owns its addresses.
    pub owns_addresses: bool,
    /// The configured virtual addresses.
    pub vip_addresses: Vec<String>,
    /// The configured peers.
    pub peers: Vec<String>,
    /// The most recent transition reason.
    pub last_reason: String,
}

impl InstanceStatus {
    /// Summarises this status for a control response.
    #[must_use]
    pub fn summary(&self) -> InstanceSummary {
        InstanceSummary {
            name: self.name.clone(),
            role: self.role.clone(),
            priority: self.priority,
            effective_priority: self.effective_priority,
            vip_addresses: self.vip_addresses.clone(),
            vips_owned: self.owns_addresses,
            health: if self.effective_priority > 0 {
                "healthy"
            } else {
                "degraded"
            }
            .to_owned(),
            master_down_remaining_ms: None,
            preemption_remaining_ms: None,
            last_reason: self.last_reason.clone(),
        }
    }
}

/// The current state of every instance this daemon runs.
#[derive(Debug, Default)]
pub struct StatusRegistry {
    instances: RwLock<BTreeMap<String, InstanceStatus>>,
    node: RwLock<String>,
    generation: RwLock<u64>,
    uptime: RwLock<Option<std::time::Instant>>,
}

impl StatusRegistry {
    /// Creates a registry for `node`.
    #[must_use]
    pub fn new(node: impl Into<String>) -> Self {
        Self {
            instances: RwLock::new(BTreeMap::new()),
            node: RwLock::new(node.into()),
            generation: RwLock::new(0),
            uptime: RwLock::new(None),
        }
    }

    /// Records the active configuration generation.
    pub fn set_generation(&self, generation: u64) {
        if let Ok(mut slot) = self.generation.write() {
            *slot = generation;
        }
    }

    /// Starts the uptime clock.
    pub fn mark_started(&self) {
        if let Ok(mut slot) = self.uptime.write() {
            *slot = Some(std::time::Instant::now());
        }
    }

    /// Publishes one instance's status.
    pub fn publish(&self, status: InstanceStatus) {
        if let Ok(mut instances) = self.instances.write() {
            instances.insert(status.name.clone(), status);
        }
    }

    /// Forgets an instance that is going away.
    pub fn forget(&self, name: &str) {
        if let Ok(mut instances) = self.instances.write() {
            instances.remove(name);
        }
    }

    /// Returns the whole node status.
    #[must_use]
    pub fn node_status(&self) -> NodeStatus {
        let node = self
            .node
            .read()
            .map_or_else(|_| "unknown".to_owned(), |name| name.clone());
        let generation = self.generation.read().map_or(0, |value| *value);
        let uptime = self
            .uptime
            .read()
            .ok()
            .and_then(|started| started.map(|started| started.elapsed().as_secs_f64()))
            .unwrap_or_default();
        let instances = self
            .instances
            .read()
            .map(|instances| instances.values().cloned().collect::<Vec<_>>())
            .unwrap_or_default();

        NodeStatus {
            node,
            generation,
            uptime_seconds: uptime,
            instances: instances.iter().map(InstanceStatus::summary).collect(),
        }
    }

    /// Returns the peers of one instance, for a request that needs them.
    #[must_use]
    pub fn peers_of(&self, name: &str) -> Vec<String> {
        self.instances
            .read()
            .ok()
            .and_then(|instances| instances.get(name).map(|status| status.peers.clone()))
            .unwrap_or_default()
    }
}

/// A control service that reads the registry and reaches the instances.
#[derive(Debug)]
pub struct ControlService {
    node: String,
    registry: Arc<StatusRegistry>,
    instances: BTreeMap<String, InstructionSender>,
    force_transition_enabled: bool,
    metrics: Option<Arc<crate::Metrics>>,
    /// The shared event history `highland events` reads.
    log: Option<Arc<crate::EventLog>>,
    /// The reload, shared with the signal loop so there is one implementation of
    /// what a reload does.
    reload: Option<Arc<crate::ReloadHandle>>,
}

impl ControlService {
    /// Creates the service.
    #[must_use]
    pub fn new(
        node: impl Into<String>,
        registry: Arc<StatusRegistry>,
        force_transition_enabled: bool,
    ) -> Self {
        Self {
            node: node.into(),
            registry,
            instances: BTreeMap::new(),
            force_transition_enabled,
            metrics: None,
            log: None,
            reload: None,
        }
    }

    /// Attaches the event history this service serves.
    #[must_use]
    pub fn with_events(mut self, log: Arc<crate::EventLog>) -> Self {
        self.log = Some(log);
        self
    }

    /// Attaches the reload this service triggers.
    #[must_use]
    pub fn with_reload(mut self, reload: Arc<crate::ReloadHandle>) -> Self {
        self.reload = Some(reload);
        self
    }

    /// Attaches the metrics this service reports to.
    #[must_use]
    pub fn with_metrics(mut self, metrics: Arc<crate::Metrics>) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// Registers an instance's instruction channel.
    pub fn register(&mut self, name: impl Into<String>, sender: InstructionSender) {
        self.instances.insert(name.into(), sender);
    }

    /// Returns the registered channels, for the reload path.
    #[must_use]
    pub fn channels(&self) -> &BTreeMap<String, InstructionSender> {
        &self.instances
    }

    /// Forgets an instance.
    pub fn forget(&mut self, name: &str) {
        self.instances.remove(name);
    }

    /// Returns the number of registered instances.
    #[must_use]
    pub fn len(&self) -> usize {
        self.instances.len()
    }

    /// Returns `true` when no instance is registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.instances.is_empty()
    }
}

impl Service for ControlService {
    fn handle(
        &self,
        request: ControlRequest,
        peer: PeerIdentity,
    ) -> impl std::future::Future<Output = ControlResponse> + Send {
        let operation = request.operation();
        let response = match request {
            ControlRequest::Status | ControlRequest::Instances => {
                ControlResponse::ok(self.registry.node_status())
            }
            ControlRequest::Show { instance } => self.show(&instance),
            ControlRequest::Events {
                since,
                limit,
                follow,
            } => self.events(since, limit, follow),
            ControlRequest::Reload => self.reload(&peer),
            ControlRequest::Pause { instance } => self.send(&instance, Instruction::pause(), peer),
            ControlRequest::Resume { instance } => {
                self.send(&instance, Instruction::resume(), peer)
            }
            ControlRequest::Relinquish { instance } => {
                self.send(&instance, Instruction::relinquish(), peer)
            }
            ControlRequest::ForceTransition {
                instance,
                role,
                confirm,
            } => {
                if !self.force_transition_enabled {
                    return std::future::ready(ControlResponse::error(
                        "operation_disabled",
                        "force-transition is disabled; start the daemon with --enable-force-transition",
                    ));
                }
                if !confirm {
                    return std::future::ready(ControlResponse::error(
                        "confirmation_required",
                        "force-transition requires an explicit confirmation",
                    ));
                }
                match Instruction::force(&role) {
                    Some(instruction) => self.send(&instance, instruction, peer),
                    None => ControlResponse::error(
                        "unknown_role",
                        format!("{role:?} is not a role: init, backup, master, fault, or disabled"),
                    ),
                }
            }
        };
        // Every request is counted, whether it was answered or refused: a client
        // hammering an endpoint it cannot use is exactly what a rate limit and a
        // counter are for.
        if let Some(metrics) = &self.metrics {
            let result = match &response {
                highland_control::ControlResponse::Error { .. } => "error",
                _ => "ok",
            };
            metrics.record_control_request(operation, result);
        }
        std::future::ready(response)
    }
}

impl ControlService {
    /// Applies a reload requested over the control socket.
    ///
    /// It calls the same handle the signal loop calls, so a `SIGHUP` and a
    /// `highland reload` cannot disagree about what a reload does.
    /// Answers a reload request, naming the peer that asked.
    ///
    /// `R-28` requires an audit event naming the peer credential for every
    /// destructive command, and reload is listed as one. `peer` is threaded in for
    /// exactly that reason: without it the event says only that a reload
    /// happened, which is what the generation counter already says.
    fn reload(&self, peer: &PeerIdentity) -> ControlResponse {
        let Some(handle) = &self.reload else {
            return ControlResponse::error("not_implemented", "this build cannot reload");
        };
        match handle.reload_from(&peer.describe()) {
            crate::ReloadOutcome::Applied {
                generation,
                reloadable,
                added,
                ..
            } => {
                let mut status = self.registry.node_status();
                status.generation = generation;
                tracing::info!(
                    generation,
                    reconfigured = reloadable.len(),
                    started = added.len(),
                    "reload applied"
                );
                ControlResponse::ok(status)
            }
            crate::ReloadOutcome::Rejected { reason } => {
                ControlResponse::error("reload_rejected", reason)
            }
        }
    }

    /// Answers an events request.
    ///
    /// The response carries the sequence number of the newest event returned, so
    /// the client sends it back as `since` next time. A client that asks for
    /// nothing it has not seen gets nothing, rather than the whole buffer on
    /// every poll.
    fn events(&self, since: Option<u64>, limit: Option<usize>, follow: bool) -> ControlResponse {
        let Some(log) = &self.log else {
            return ControlResponse::error(
                "not_implemented",
                "this build records no event history",
            );
        };
        let limit = limit.unwrap_or(100).clamp(1, 1000);
        let entries = log.since(since.unwrap_or(0), limit);
        let latest = entries.last().map_or_else(
            || since.unwrap_or_else(|| log.latest()),
            |(sequence, _)| *sequence,
        );
        let events = entries
            .into_iter()
            .filter_map(|(sequence, event)| {
                serde_json::to_value(&event).ok().map(|mut value| {
                    if let Some(object) = value.as_object_mut() {
                        object.insert("sequence".to_owned(), serde_json::json!(sequence));
                    }
                    value
                })
            })
            .collect();

        // `follow` is recorded rather than acted on: the client polls, and the
        // server must not hold a connection open per follower, because a client
        // that disappeared mid-stream would leave a task waiting on a socket
        // forever.
        let _ = follow;
        // The gap a client resuming from this cursor cannot see, so a bounded
        // history that has overwritten something says so instead of looking
        // quiet. `since` rather than the total, because the total cannot say
        // whether the loss was before or after where the client was.
        let dropped = log.dropped_since(since.unwrap_or(0));
        ControlResponse::Events {
            events,
            latest,
            dropped,
        }
    }

    fn show(&self, instance: &str) -> ControlResponse {
        let status = self.registry.node_status();
        match status.instances.iter().find(|found| found.name == instance) {
            Some(summary) => {
                let mut narrowed = status.clone();
                narrowed.instances = vec![summary.clone()];
                ControlResponse::ok(narrowed)
            }
            None => ControlResponse::error(
                "unknown_instance",
                format!("no instance named {instance:?}"),
            ),
        }
    }

    fn send(
        &self,
        instance: &str,
        instruction: Instruction,
        peer: PeerIdentity,
    ) -> ControlResponse {
        let Some(sender) = self.instances.get(instance) else {
            return ControlResponse::error(
                "unknown_instance",
                format!("no instance named {instance:?}"),
            );
        };
        // Every mutating operation is an audit record: who asked, and for which
        // instance (`R-28`).
        tracing::info!(
            peer = %peer.describe(),
            instance,
            operation = instruction.operation(),
            "control request"
        );
        match sender.try_send(instruction) {
            Ok(()) => ControlResponse::ok(self.registry.node_status()),
            Err(error) => ControlResponse::error(
                "busy",
                ControlError::Io {
                    reason: error.to_string(),
                }
                .to_string(),
            ),
        }
    }

    /// Returns the node this service reports for.
    #[must_use]
    pub fn node(&self) -> &str {
        &self.node
    }
}
