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
        }
    }

    /// Registers an instance's instruction channel.
    pub fn register(&mut self, name: impl Into<String>, sender: InstructionSender) {
        self.instances.insert(name.into(), sender);
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
        let response = match request {
            ControlRequest::Status | ControlRequest::Instances => {
                ControlResponse::ok(self.registry.node_status())
            }
            ControlRequest::Show { instance } => self.show(&instance),
            ControlRequest::Events { limit, .. } => ControlResponse::error(
                "not_implemented",
                format!(
                    "the event stream returns at most {} events and is not served yet",
                    limit.unwrap_or(0)
                ),
            ),
            ControlRequest::Reload => ControlResponse::error(
                "not_implemented",
                "reload over the control socket arrives with the transactional reload",
            ),
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
        std::future::ready(response)
    }
}

impl ControlService {
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
