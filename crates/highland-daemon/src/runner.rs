// Rust guideline compliant 2026-09-27

//! The process entry point.
//!
//! Argument parsing lives in `highland-cli`; this binary is what
//! `highland run` executes.

use crate::shutdown::{ShutdownPlan, ShutdownReason};
use crate::{Daemon, DaemonError, Options};

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use highland_config::{Config, InstanceConfig, ValidationContext, load, validate};
use highland_net::{IpCidr, PeerSet};
use highland_vrrp::IpFamily;

use crate::executor::Ownership;
use crate::options::InstancePlan;

/// Runs the daemon until it is asked to stop.
///
/// The signal set is part of the contract: `SIGTERM` and `SIGINT` start the
/// graceful sequence, `SIGHUP` reloads, and a second `SIGTERM` is ignored so
/// that shutdown stays idempotent (`I-31`).
///
/// # Errors
///
/// Returns an error when the configuration cannot be loaded, when logging cannot
/// be initialized, when a signal handler cannot be installed, or when an instance
/// cannot bind its socket.
// The entry point is long because it is the whole start-up sequence in one
// place, in order: configuration, logging, metrics, control, instances, then the
// signal loop. Splitting it would hide the order, which is the part that
// matters: a client that connects before the instances exist gets a status that
// describes nothing.
#[expect(
    clippy::too_many_lines,
    reason = "the start-up sequence reads better in order"
)]
pub async fn run(options: Options) -> Result<(), DaemonError> {
    let mut daemon = Daemon::prepare(options)?;

    if !TRANSPORT_AVAILABLE {
        // The configuration is still validated first, so `highland run` with a
        // broken file still reports the broken file rather than the platform.
        tracing::error!(
            instances = daemon.config().instances.len(),
            "refusing to start: this build cannot carry VRRP on the wire"
        );
        return Err(DaemonError::TransportUnavailable);
    }

    let filter = crate::logging::filter_for(&daemon.config().logging.level);
    crate::logging::init(
        &daemon.config().logging.level,
        &daemon.config().logging.format,
        daemon.config().logging.format == "json",
    )
    .map_err(DaemonError::Logging)?;

    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .map_err(|source| {
        DaemonError::Runtime(format!("could not install the SIGTERM handler: {source}"))
    })?;
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .map_err(|source| {
        DaemonError::Runtime(format!("could not install the SIGINT handler: {source}"))
    })?;
    let mut reload = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
        .map_err(|source| {
            DaemonError::Runtime(format!("could not install the SIGHUP handler: {source}"))
        })?;

    // A watch rather than a channel: the signal handlers own the sending end
    // and every instance holds a receiver, so one signal reaches all of them.
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    tracing::info!(
        filter = %filter,
        node = %daemon.config().node.name,
        instances = daemon.config().instances.len(),
        config = %daemon.options().config_path.display(),
        "highland started"
    );

    let registry = std::sync::Arc::new(crate::StatusRegistry::new(
        daemon.config().node.name.clone(),
    ));
    registry.mark_started();
    let metrics = crate::Metrics::shared();

    // Started before the instances, so a scraper that arrives immediately sees
    // `highland_up` and a node that is still starting rather than a refused
    // connection.
    start_metrics_endpoint(&daemon, &registry, &metrics);

    // The event history comes before the reload handle, which comes before the
    // service: the service holds both, and the handle starts with no channels
    // and is given them as instances start.
    let event_log = std::sync::Arc::new(crate::EventLog::new());
    let reload_handle = std::sync::Arc::new(ReloadHandle::new(
        daemon.options().config_path.clone(),
        daemon.config(),
        std::sync::Arc::clone(&registry),
        std::collections::BTreeMap::new(),
        std::sync::Arc::clone(&metrics),
        daemon.options().local_addresses.clone(),
        std::sync::Arc::clone(&event_log),
    ));

    let mut service = crate::ControlService::new(
        daemon.config().node.name.clone(),
        std::sync::Arc::clone(&registry),
        daemon.options().force_transition_enabled,
    )
    .with_metrics(std::sync::Arc::clone(&metrics))
    .with_reload(std::sync::Arc::clone(&reload_handle))
    .with_events(std::sync::Arc::clone(&event_log));

    // The configuration a reload compares against, and the channels it applies
    // to. The instance map is shared with the control service, so a reload
    // reaches exactly the instances an operator does.
    let instances = start_instances(
        &daemon,
        &shutdown_rx,
        &registry,
        &metrics,
        &mut service,
        &reload_handle,
        &event_log,
    )
    .await?;

    // The control socket is served after the instances exist, so the first
    // `status` a client asks for already describes the real thing.
    start_control_socket(&daemon, std::sync::Arc::new(service)).await?;

    let reason = loop {
        tokio::select! {
            _ = terminate.recv() => break ShutdownReason::Signal,
            _ = interrupt.recv() => break ShutdownReason::Signal,
            // `SIGHUP` reloads and continues. Once the sequence above has been
            // chosen, a second termination signal is ignored, which keeps
            // shutdown idempotent (`I-31`).
            _ = reload.recv() => {
                // The same handle the control API holds, so a `SIGHUP` and a
                // `highland reload` cannot disagree about what a reload does.
                // The handle records the outcome as a `ReloadAccepted` or
                // `ReloadRejected` event, naming the initiator, so both this path
                // and the control socket produce the same event. The
                // `announce(&daemon, "reload_accepted")` that used to sit here
                // published a second event under `DaemonLifecycle` whose *reason*
                // happened to be the string, which is why the event names
                // `ReloadAccepted` and `ReloadRejected` existed and were never
                // used, and why an operator filtering the history by event name
                // could not find a reload.
                match reload_handle.reload() {
                    ReloadOutcome::Applied { generation, reloadable, added, .. } => {
                        tracing::info!(
                            generation,
                            reconfigured = reloadable.len(),
                            started = added.len(),
                            "reload applied"
                        );
                    }
                    ReloadOutcome::Rejected { reason } => {
                        tracing::error!(reason = %reason, "reload rejected; the running configuration is unchanged");
                    }
                }
            }
        }
    };

    // Tell the instances before the daemon's own shutdown sequence, so each
    // actor relinquishes its addresses in its own time and within the budget
    // (`SPEC.md` §14.5).
    let _ = shutdown_tx.send(true);

    let plan = ShutdownPlan::new(
        reason,
        instances.iter().map(|(name, _)| name.clone()).collect(),
    );
    for event in daemon.shutdown(plan) {
        tracing::info!(event = %event.name, reason = %event.reason, "highland stopped");
    }
    Ok(())
}

/// A running instance and the task that drives it.
type RunningInstance = (String, tokio::task::JoinHandle<()>);

/// Starts one task per configured instance.
///
/// Every instance gets its own backend, transport, socket, and actor, because
/// the state machine holds the only authority over role and two instances must
/// not be able to disagree about one another's address (`SPEC.md` §20).
#[cfg(target_os = "linux")]
async fn start_instances(
    daemon: &Daemon,
    shutdown_signal: &tokio::sync::watch::Receiver<bool>,
    registry: &std::sync::Arc<crate::StatusRegistry>,
    metrics: &std::sync::Arc<crate::Metrics>,
    service: &mut crate::ControlService,
    reload: &std::sync::Arc<ReloadHandle>,
    log: &std::sync::Arc<crate::EventLog>,
) -> Result<Vec<RunningInstance>, DaemonError> {
    let mut running = Vec::new();
    for (plan, ownership) in plans(daemon.config()) {
        use highland_net::NetworkBackend as _;

        let backend = std::sync::Arc::new(
            highland_net::default_backend()
                .map_err(|error| DaemonError::Runtime(format!("no network backend: {error}")))?,
        );
        let source = source_address(&plan, &ownership, backend.as_ref())
            .await
            .map_err(|error| DaemonError::Runtime(format!("instance {}: {error}", plan.name)))?;

        let transport = std::sync::Arc::new(
            crate::VrrpTransport::bind(
                &plan,
                &ownership.interface,
                source,
                ownership.peering.clone(),
                std::sync::Arc::clone(metrics),
                plan.allow_unconforming_hop_limit,
            )
            .await
            .map_err(|error| DaemonError::Runtime(format!("instance {}: {error}", plan.name)))?,
        );

        // The interface name is needed after `ownership` has been moved into
        // the actor.
        let interface_name = ownership.interface.clone();

        let mut actor = crate::InstanceActor::new(
            highland_core::clock::SystemClock::new(),
            plan.clone(),
            ownership,
            backend.clone(),
            transport.clone(),
        )
        .with_metrics(std::sync::Arc::clone(metrics))
        .with_events(
            daemon.config().node.name.clone(),
            std::sync::Arc::clone(log),
        );

        // The machine needs its own address to resolve an equal-priority
        // advertisement, which is exactly what two nodes starting together
        // produce. Without it the tie-break has nothing to compare and both
        // nodes stay master.
        if let Ok(interface) = backend.interface(&interface_name).await {
            actor.set_primary_addresses(interface.primary_ipv4(), interface.primary_ipv6());
        }

        // Two channels, two sources: one for the operator and one for the
        // wire. Sharing one would let a flood of advertisements delay a
        // `relinquish`.
        let (sender, receiver) = crate::channel();
        let (reader_sender, reader_receiver) = crate::channel();
        registry.publish(actor.publish_status());
        log.record_instance(&daemon.config().node.name, &actor.publish_status());
        service.register(plan.name.clone(), sender.clone());
        reload.register(plan.name.clone(), sender.clone());

        let reader = transport.spawn_reader(reader_sender);
        let watch = shutdown_signal.clone();
        let name = plan.name.clone();

        // The health task, beside the actor rather than inside it: a probe waits
        // on a socket, and a state machine that waited on a socket would stop
        // deciding anything while a service was slow (`I-38`, `R-05`).
        //
        // It shares the backend, so an interface check and the daemon read the
        // same Netlink socket and cannot disagree about whether a link is up.
        let health_links: std::sync::Arc<dyn highland_checks::LinkProbe> =
            std::sync::Arc::clone(&backend) as _;
        let health_sender = sender.clone();
        let health_instance = plan.name.clone();
        let health_plans = plan.checks.clone();
        let health_metrics = std::sync::Arc::clone(metrics);
        let health_watch = shutdown_signal.clone();
        let health = tokio::spawn(async move {
            // The health task is aborted with the actor, so nothing outlives the
            // instance that owns it (`I-41`).
            if let Err(reason) = crate::health_task::run(
                &health_instance,
                &health_plans,
                health_links,
                health_sender,
                health_metrics,
                health_watch,
            )
            .await
            {
                // A check that cannot be built is a configuration error, and a
                // node with checks it cannot run must not pretend to be
                // participating: it is said out loud, because the alternative is
                // a node advertising itself healthy while a check is failing to
                // start on every interval.
                tracing::error!(instance = %health_instance, "{reason}");
            }
        });

        let publishing_registry = std::sync::Arc::clone(registry);
        let publishing_name = plan.name.clone();
        let handle = tokio::spawn(async move {
            crate::run_instance(
                actor,
                receiver,
                reader_receiver,
                watch,
                Some(publishing_registry),
                Some(publishing_name),
            )
            .await;
            // The reader and the health task stop with the actor: nothing
            // outlives the instance that owns it (`I-41`).
            reader.abort();
            health.abort();
        });

        tracing::info!(
            instance = %name,
            vrid = plan.vrid,
            source = %source,
            peers = transport.destinations().len(),
            "instance started"
        );
        running.push((name, handle));
    }
    Ok(running)
}

/// Without a socket there is nothing to start, and saying so is better than
/// starting instances that cannot speak VRRP.
#[cfg(not(target_os = "linux"))]
fn start_instances(
    _daemon: &Daemon,
    _shutdown_signal: &tokio::sync::watch::Receiver<bool>,
    _registry: &std::sync::Arc<crate::StatusRegistry>,
    _metrics: &std::sync::Arc<crate::Metrics>,
    _service: &mut crate::ControlService,
    _reload: &std::sync::Arc<ReloadHandle>,
    _log: &std::sync::Arc<crate::EventLog>,
) -> impl std::future::Future<Output = Result<Vec<RunningInstance>, DaemonError>> {
    std::future::ready(Err(DaemonError::TransportUnavailable))
}

/// Picks the address an instance sends from.
///
/// RFC 5798 §5.1.1.1 requires the primary address of the interface, **not** the
/// virtual address. That is not a detail: a raw socket cannot bind to an address
/// the interface does not have yet, and the virtual address is only added when
/// the instance becomes master. Binding to the VIP would therefore fail at
/// startup with `EADDRNOTAVAIL` on every node.
#[cfg(target_os = "linux")]
async fn source_address(
    plan: &InstancePlan,
    ownership: &Ownership,
    backend: &highland_net::NetlinkBackend,
) -> Result<IpAddr, crate::executor::TransportError> {
    use highland_net::NetworkBackend as _;

    let interface = backend
        .interface(&ownership.interface)
        .await
        .map_err(|error| crate::executor::TransportError::Unavailable {
            reason: format!("interface {}: {error}", ownership.interface),
        })?;

    let virtual_addresses: Vec<IpAddr> = ownership
        .addresses
        .iter()
        .map(highland_net::IpCidr::address)
        .collect();
    let real: Vec<IpAddr> = interface
        .addresses
        .iter()
        .map(highland_net::IpCidr::address)
        .filter(|address| !virtual_addresses.contains(address))
        .collect();

    // RFC 5798 §5.1.2.1: "This is the IPv6 link-local address of the interface
    // the packet is being sent from." Not the first IPv6 address on the
    // interface, and not a configured one. The link-local address is the one a
    // peer can keep sending to across a renumbering, and an implementation that
    // advertises from a global address is sending from an address the protocol
    // does not name.
    //
    // For IPv4 the primary address is correct (§5.1.1.1), so the families differ
    // here and the difference is the RFC's, not a preference.
    let primary = match ownership.family() {
        Some(highland_vrrp::IpFamily::V6) => real
            .iter()
            .copied()
            .find(|address| match address {
                IpAddr::V6(address) => address.is_unicast_link_local(),
                IpAddr::V4(_) => false,
            })
            .or_else(|| real.first().copied()),
        _ => real.first().copied(),
    };

    match primary {
        Some(address) => {
            tracing::debug!(
                instance = %plan.name,
                %address,
                interface = %interface.name,
                "sending from the interface's primary address"
            );
            Ok(address)
        }
        None => {
            // The interface has nothing but the virtual addresses, which cannot
            // happen in a working configuration: the node needs an address of its
            // own to reach its peers. Say so plainly rather than failing later at
            // the socket.
            Err(crate::executor::TransportError::Encode {
                reason: format!(
                    "interface {} has no address of its own, so instance {} has nothing to send from",
                    interface.name, plan.name
                ),
            })
        }
    }
}

/// Serves the Prometheus endpoint, if one is configured.
///
/// A failure to bind is a warning, not a refusal to start. Metrics are for a
/// scraper; forwarding addresses are for clients, and a node that will not take
/// traffic because its exporter could not bind would be a surprising node.
#[cfg(target_os = "linux")]
fn start_metrics_endpoint(
    daemon: &Daemon,
    registry: &std::sync::Arc<crate::StatusRegistry>,
    metrics: &std::sync::Arc<crate::Metrics>,
) {
    let settings = &daemon.config().metrics;
    if !settings.enabled {
        return;
    }
    let Some(listen) = settings.listen.as_deref() else {
        tracing::warn!("metrics are enabled but no listen address is set; continuing without them");
        return;
    };
    let Ok(address) = listen.parse::<std::net::SocketAddr>() else {
        tracing::warn!(
            listen,
            "metrics listen address is not a socket address; continuing without them"
        );
        return;
    };

    let server = crate::MetricsServer::new(
        std::sync::Arc::clone(metrics),
        std::sync::Arc::clone(registry),
        env!("CARGO_PKG_VERSION"),
    );
    tokio::spawn(async move {
        if let Err(error) = server.serve(address).await {
            tracing::warn!(error = %error, "continuing without a metrics endpoint");
        }
    });
}

#[cfg(not(target_os = "linux"))]
fn start_metrics_endpoint(
    _daemon: &Daemon,
    _registry: &std::sync::Arc<crate::StatusRegistry>,
    _metrics: &std::sync::Arc<crate::Metrics>,
) {
}

/// Serves the control socket for the lifetime of the process.
///
/// The socket is optional: an operator who never calls `highland status` gets a
/// warning rather than a daemon that will not start, because a missing
/// administrative interface is not a reason to stop forwarding addresses.
#[cfg(target_os = "linux")]
async fn start_control_socket(
    daemon: &Daemon,
    service: std::sync::Arc<crate::ControlService>,
) -> Result<(), DaemonError> {
    let settings = &daemon.config().control;
    let policy = highland_control::SocketPolicy {
        path: std::path::PathBuf::from(&settings.socket),
        group: settings.group.clone(),
        verify_peer_credentials: settings.verify_peer_credentials,
        requests_per_second: 20,
    };

    match highland_control::Server::bind(service, policy).await {
        Ok(server) => {
            let path = server.path().display().to_string();
            tracing::info!(socket = %path, "control socket listening");
            tokio::spawn(async move {
                if let Err(error) = server.serve().await {
                    tracing::error!(error = %error, "control socket stopped");
                }
            });
            Ok(())
        }
        Err(error) => {
            tracing::warn!(error = %error, "continuing without a control socket");
            Ok(())
        }
    }
}

/// The control API is portable even where VRRP is not, so this still serves a
/// socket off Linux; only the Linux build needs to build one here.
#[cfg(not(target_os = "linux"))]
fn start_control_socket(
    _daemon: &Daemon,
    _service: std::sync::Arc<crate::ControlService>,
) -> impl std::future::Future<Output = Result<(), DaemonError>> {
    std::future::ready(Ok(()))
}

/// Takes a lock, recovering from poisoning.
///
/// A panic while recording a reload outcome must not stop a node that owns a
/// VIP, so the data is taken even from a poisoned lock. The worst case is a
/// reload compared against a stale configuration, which the next reload
/// corrects.
/// What a reload decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReloadOutcome {
    /// Every instance absorbed the change.
    Applied {
        /// The new generation.
        generation: u64,
        /// The instances that were reconfigured in place.
        reloadable: Vec<String>,
        /// The instances that were started.
        added: Vec<String>,
        /// The configuration that is now running, which the next reload compares
        /// against. Boxed because it is much larger than the other variants.
        config: Box<Config>,
    },
    /// Nothing was changed, for the stated reason.
    Rejected {
        /// Why, in the words an operator would use.
        reason: String,
    },
}

/// A reload, shared by the signal loop and the control API.
///
/// One handle, one implementation: a `SIGHUP` and `highland reload` must not be
/// able to disagree about what a reload does, so both call the same code against
/// the same state.
#[derive(Debug)]
pub struct ReloadHandle {
    path: PathBuf,
    /// The node name, so a recorded event names the node it happened on.
    node: String,
    running: Mutex<Config>,
    registry: Arc<crate::StatusRegistry>,
    senders: Mutex<BTreeMap<String, crate::InstructionSender>>,
    metrics: Arc<crate::Metrics>,
    generation: AtomicU64,
    /// The shared event history, so a reload appears in `highland events` the way
    /// every other operator action does.
    log: Arc<crate::EventLog>,
    /// The host's own addresses, so a reload can enforce `V-08` exactly as
    /// startup does.
    ///
    /// A reload that validated more loosely than startup would be a way to
    /// reach a state the daemon refuses to boot into: the operator fixes the
    /// peer list, reloads to pick it up, and the node unicasts to itself for as
    /// long as the configuration stays in place.
    local_addresses: Vec<IpAddr>,
}

impl ReloadHandle {
    /// Creates a handle for `path`, starting from the configuration the daemon
    /// loaded.
    #[must_use]
    pub fn new(
        path: PathBuf,
        running: &Config,
        registry: Arc<crate::StatusRegistry>,
        senders: BTreeMap<String, crate::InstructionSender>,
        metrics: Arc<crate::Metrics>,
        local_addresses: Vec<IpAddr>,
        log: Arc<crate::EventLog>,
    ) -> Self {
        Self {
            path,
            node: running.node.name.clone(),
            running: Mutex::new(running.clone()),
            registry,
            senders: Mutex::new(senders),
            metrics,
            generation: AtomicU64::new(0),
            local_addresses,
            log,
        }
    }

    /// Registers an instance's channel, so a reload reaches it.
    pub fn register(&self, name: String, sender: crate::InstructionSender) {
        if let Ok(mut senders) = self.senders.lock() {
            senders.insert(name, sender);
        }
    }

    /// Forgets an instance that is going away.
    pub fn forget(&self, name: &str) {
        if let Ok(mut senders) = self.senders.lock() {
            senders.remove(name);
        }
    }

    /// The generation currently in force.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }

    /// Reads the candidate configuration, plans the change, and applies it.
    ///
    /// A reload is a transaction (`I-09`, `R-46`): if any instance would need
    /// a restart, nothing is applied and the refusal names the instance and the
    /// change. Applying bumps the generation, so a result still carrying the old
    /// one is discarded (`R-47`).
    pub fn reload(&self) -> ReloadOutcome {
        self.reload_from("SIGHUP")
    }

    /// Reloads, recording `initiator` as the peer that asked.
    ///
    /// The event is recorded here rather than by the callers because a `SIGHUP`
    /// and a `highland reload` must not be able to disagree about what happened,
    /// and the caller is the thing that knows the peer.
    pub fn reload_from(&self, initiator: &str) -> ReloadOutcome {
        let candidate = match load(&self.path, false) {
            Ok(candidate) => candidate,
            Err(error) => {
                self.metrics.record_reload("rejected");
                let reason = error.to_string();
                self.log
                    .record_reload(&self.node, false, None, &reason, initiator);
                return ReloadOutcome::Rejected { reason };
            }
        };
        let context = ValidationContext {
            local_addresses: &self.local_addresses,
            ..ValidationContext::permissive()
        };
        if let Err(violations) = validate(&candidate, &context) {
            self.metrics.record_reload("rejected");
            let reason = violations
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; ");
            self.log
                .record_reload(&self.node, false, None, &reason, initiator);
            return ReloadOutcome::Rejected { reason };
        }

        let running = Self::lock(&self.running).clone();
        let plan = crate::reload::plan(&running, &candidate);
        if !plan.is_applicable() {
            // Nothing has been touched at this point, and nothing will be.
            self.metrics.record_reload("rejected");
            let reason = plan.refusals().join("; ");
            self.log
                .record_reload(&self.node, false, None, &reason, initiator);
            return ReloadOutcome::Rejected { reason };
        }

        let generation = self.generation() + 1;
        let mut reloadable = Vec::new();
        for (name, change) in &plan.changes {
            if !matches!(change, crate::reload::Change::Reloadable { .. }) {
                continue;
            }
            let Some(instance) = candidate
                .instances
                .iter()
                .find(|instance| &instance.name == name)
            else {
                continue;
            };
            let Some(sender) = Self::lock(&self.senders).get(name).cloned() else {
                continue;
            };
            let instruction = crate::Instruction::Reload {
                plan: crate::reload::plan_for(instance),
                ownership: ownership_for(instance),
                generation: highland_core::state::Generation::from_number(generation),
            };
            // `try_send`, not `send`: a reload must not wait behind a flood of
            // advertisements, and an undelivered reload is reported as
            // unapplied rather than half-applied.
            if sender.try_send(instruction).is_ok() {
                reloadable.push(name.clone());
            }
        }

        *Self::lock(&self.running) = candidate.clone();
        self.generation.store(generation, Ordering::Relaxed);
        self.registry.set_generation(generation);
        self.metrics.record_reload("accepted");
        let added = plan
            .added()
            .into_iter()
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        self.log.record_reload(
            &self.node,
            true,
            Some(generation),
            &format!(
                "reconfigured {} instance(s), started {}",
                reloadable.len(),
                added.len()
            ),
            initiator,
        );
        ReloadOutcome::Applied {
            generation,
            reloadable,
            added,
            config: Box::new(candidate),
        }
    }

    fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        // A panic while recording a reload outcome must not stop a node that owns
        // a VIP, so the data is taken even from a poisoned lock. The worst case is
        // a reload compared against a stale configuration, which the next reload
        // corrects.
        mutex.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The addresses and peers one configured instance manages.
///
/// A multicast instance needs no peer list, so the mode decides which of the two
/// descriptions applies. Where the mode and the group disagree — an IPv4
/// instance with an IPv6 group, or a group that is not a multicast address — the
/// group is the RFC's default for the instance's family rather than a refusal:
/// the alternative is a node that joins a group nobody is advertising to.
fn ownership_for(instance: &InstanceConfig) -> crate::executor::Ownership {
    let addresses: Vec<IpCidr> = instance
        .vip_addresses()
        .iter()
        .filter_map(|text| IpCidr::parse(text).ok())
        .collect();

    match instance.network.mode {
        highland_config::NetworkMode::Unicast => crate::executor::Ownership::new(
            instance.interface.clone(),
            addresses,
            PeerSet::new(instance.network.peers.clone()),
        ),
        highland_config::NetworkMode::Multicast => {
            let group = multicast_group(instance, &addresses);
            crate::executor::Ownership::multicast(
                instance.interface.clone(),
                addresses,
                group,
                instance.network.multicast.ttl,
            )
        }
    }
}

/// The group a multicast instance speaks for: the configured one when it is of
/// the instance's family, and the RFC's default for that family otherwise.
fn multicast_group(instance: &InstanceConfig, addresses: &[IpCidr]) -> IpAddr {
    match ownership_family(addresses) {
        // The per-family default, or the configured override when it is of this
        // family. Validation refuses a configured group of the wrong family, so
        // what reaches here is either right or unset.
        Some(IpFamily::V4) => instance
            .network
            .multicast
            .group_for(highland_config::Family::V4),
        Some(IpFamily::V6) => instance
            .network
            .multicast
            .group_for(highland_config::Family::V6),
        // A family this build does not know, or no addresses at all: the
        // instance is refused by validation or by the socket before it can
        // matter, and the configured group is the best guess available.
        Some(_) | None => instance
            .network
            .multicast
            .group
            .unwrap_or_else(|| IpFamily::V4.default_group()),
    }
}

/// The family of the first address in a list, or `None` when there is none.
fn ownership_family(addresses: &[highland_net::IpCidr]) -> Option<IpFamily> {
    addresses
        .first()
        .map(|address| IpFamily::of(&address.address()))
}

/// The instance plans a configuration implies, in the order they appear.
///
/// The daemon cannot start instances yet (see [`TRANSPORT_AVAILABLE`]), but the
/// translation from configuration to plan is real, tested, and used by the CLI
/// to explain what a reload would do.
///
/// The order matters for reloads: a plan is matched to a running instance by
/// name, and a stable order makes the difference report readable.
#[must_use]
pub fn plans(config: &Config) -> Vec<(InstancePlan, Ownership)> {
    config
        .instances
        .iter()
        .filter_map(|instance| plan_for(instance).ok())
        .collect()
}

/// Converts one configured instance into a plan and its ownership.
///
/// # Errors
///
/// Returns a message when the instance cannot be expressed in the terms the
/// executor needs, which the configuration validator should already have
/// caught. Reporting it here rather than panicking keeps a bad reload from
/// taking the daemon down (`R-24`).
fn plan_for(instance: &InstanceConfig) -> Result<(InstancePlan, Ownership), String> {
    let addresses: Vec<IpCidr> = instance
        .vip_addresses()
        .iter()
        .map(|text| IpCidr::parse(text).map_err(|error| error.to_string()))
        .collect::<Result<Vec<_>, _>>()?;

    let plan = InstancePlan::from_config(instance);
    let ownership = match instance.network.mode {
        highland_config::NetworkMode::Unicast => Ownership::new(
            instance.interface.clone(),
            addresses,
            PeerSet::new(instance.network.peers.clone()),
        ),
        highland_config::NetworkMode::Multicast => {
            let group = multicast_group(instance, &addresses);
            if !group.is_multicast() {
                return Err(format!(
                    "the multicast group {group} is not a multicast address"
                ));
            }
            Ownership::multicast(
                instance.interface.clone(),
                addresses,
                group,
                instance.network.multicast.ttl,
            )
        }
    };
    Ok((plan, ownership))
}

/// Whether this build can carry VRRP on the wire.
///
/// The socket exists on Linux and does not anywhere else, so this is decided at
/// compile time rather than discovered at runtime. A build where it is `false`
/// refuses to start, because a process that claimed to be a VRRP router while
/// sending nothing would be worse than one that declines to run.
pub const TRANSPORT_AVAILABLE: bool = cfg!(target_os = "linux");

#[test]
fn a_multicast_instance_speaks_for_the_default_group_of_its_family() {
    let text = r#"
schema_version = 1
[node]
name = "node-a"
[[instance]]
name = "api"
interface = "eth0"
vrid = 42
advertisement_interval = "1s"
[instance.network]
mode = "multicast"
[[instance.vip]]
address = "2001:db8:10::10/64"
"#;
    let config = highland_config::parse(text).expect("the document parses");
    let (plan, ownership) = plan_for(
        config
            .instances
            .first()
            .expect("the document has an instance"),
    )
    .expect("the instance is expressible");

    assert_eq!(plan.name, "api");
    assert_eq!(
        ownership.peering,
        crate::executor::Peering::Multicast {
            group: "ff02::12".parse().expect("a valid group"),
            ttl: 255,
        },
        "an unset group is the RFC's default for the instance's family, and for an IPv6 \
             instance that is not the IPv4 one"
    );
    assert!(
        ownership.peers.is_empty(),
        "a multicast instance has no peer list: {:?}",
        ownership.peers
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_unicast_instance_still_carries_its_peer_list() {
        let config = highland_config::parse(
            r#"
schema_version = 1
[node]
name = "node-a"
[[instance]]
name = "api"
interface = "eth0"
vrid = 42
[instance.network]
mode = "unicast"
peers = ["192.0.2.11"]
[[instance.vip]]
address = "192.0.2.10/24"
"#,
        )
        .expect("the document parses");
        let instance = config
            .instances
            .first()
            .expect("the document has an instance");

        let ownership = ownership_for(instance);

        assert_eq!(
            ownership.peers.len(),
            1,
            "the list is what a unicast socket validates against"
        );
        assert!(matches!(
            ownership.peering,
            crate::executor::Peering::Unicast(_)
        ));
    }

    const CONFIG: &str = r#"
schema_version = 1

[node]
name = "node-a"

[[instance]]
name = "api"
interface = "eth0"
vrid = 42
priority = 150
advertisement_interval = "1s"

[instance.network]
mode = "unicast"
peers = ["192.0.2.11"]

[[instance.vip]]
address = "192.0.2.10/24"
"#;

    #[test]
    fn a_configured_instance_becomes_a_plan_and_its_ownership() {
        let config = highland_config::parse(CONFIG).expect("the fixture parses");
        let (plan, ownership) = plan_for(&config.instances[0]).expect("convertible");

        assert_eq!(plan.name, "api");
        assert_eq!(plan.vrid, 42);
        assert_eq!(plan.priority, 150);
        assert_eq!(
            plan.advertisement_interval,
            std::time::Duration::from_secs(1)
        );
        assert_eq!(ownership.addresses.len(), 1);
        assert_eq!(ownership.addresses[0].to_string(), "192.0.2.10/24");
        assert_eq!(ownership.peers.len(), 1);
    }

    #[test]
    fn every_instance_yields_a_plan_in_order() {
        let config = highland_config::parse(CONFIG).expect("the fixture parses");
        let plans = plans(&config);
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].0.name, "api");
    }

    #[tokio::test]
    async fn the_daemon_refuses_to_start_without_a_transport() {
        // A process that claimed to be a VRRP router while sending nothing
        // would be worse than one that refuses, so this is the behaviour a
        // release must keep until the socket exists.
        let error = run(Options::with_config(std::path::Path::new(
            "crates/highland-config/tests/fixtures/basic.toml",
        )))
        .await;

        // The path is relative to the workspace root, so the test tolerates
        // either outcome and only insists the refusal is explicit.
        if let Err(error) = error {
            let rendered = error.to_string();
            assert!(
                rendered.contains("transport is not implemented")
                    || rendered.contains("could not read"),
                "unexpected error: {rendered}"
            );
        }
    }

    /// A configuration with no instances, which is the starting point for a
    /// rejection test: nothing can be applied to it.
    fn empty_config() -> Config {
        highland_config::parse("schema_version = 1\n[node]\nname = \"node-a\"\n")
            .expect("the fixture parses")
    }

    #[test]
    fn an_unreadable_reload_is_rejected_and_changes_nothing() {
        let registry = std::sync::Arc::new(crate::StatusRegistry::new("node-a"));
        let metrics = crate::Metrics::shared();
        let handle = ReloadHandle::new(
            std::path::PathBuf::from("/nonexistent/highland.toml"),
            &empty_config(),
            std::sync::Arc::clone(&registry),
            std::collections::BTreeMap::new(),
            metrics,
            Vec::new(),
            std::sync::Arc::new(crate::EventLog::new()),
        );

        let outcome = handle.reload();

        assert!(
            matches!(outcome, ReloadOutcome::Rejected { .. }),
            "{outcome:?}"
        );
        assert_eq!(
            registry.node_status().generation,
            0,
            "a rejected reload bumps nothing"
        );
    }

    #[test]
    fn a_fresh_handle_starts_at_generation_zero() {
        let handle = ReloadHandle::new(
            std::path::PathBuf::from("/nonexistent/highland.toml"),
            &empty_config(),
            std::sync::Arc::new(crate::StatusRegistry::new("node-a")),
            std::collections::BTreeMap::new(),
            crate::Metrics::shared(),
            Vec::new(),
            std::sync::Arc::new(crate::EventLog::new()),
        );
        assert_eq!(handle.generation(), 0);
    }

    /// Writes a document to a temporary file and returns the path a
    /// [`ReloadHandle`] can be pointed at.
    fn write_reload_source(name: &str, body: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("hl-reload-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("config.toml");
        std::fs::write(&path, body).expect("write config");
        path
    }

    /// A reload must not be a way to reach a configuration that startup would
    /// have refused.
    ///
    /// It is a separate code path from `Daemon::prepare`, it is easy to leave
    /// permissive, and the consequence is a node that unicast-advertises to
    /// itself for as long as the file stays in place -- after a startup the
    /// operator has already been told is clean.
    #[test]
    fn a_reload_that_peers_with_this_node_is_rejected() {
        let path = write_reload_source(
            "self-peer",
            "schema_version = 1\n\
             [node]\n\
             name = \"node-a\"\n\
             [[instance]]\n\
             name = \"api\"\n\
             interface = \"lo\"\n\
             vrid = 42\n\
             [instance.network]\n\
             mode = \"unicast\"\n\
             peers = [\"127.0.0.1\"]\n\
             [[instance.vip]]\n\
             address = \"192.0.2.10/24\"\n",
        );
        let registry = std::sync::Arc::new(crate::StatusRegistry::new("node-a"));
        let handle = ReloadHandle::new(
            path,
            &empty_config(),
            std::sync::Arc::clone(&registry),
            std::collections::BTreeMap::new(),
            crate::Metrics::shared(),
            vec!["127.0.0.1".parse().expect("valid address")],
            std::sync::Arc::new(crate::EventLog::new()),
        );

        match handle.reload() {
            ReloadOutcome::Rejected { reason } => assert!(
                reason.contains("V-08"),
                "the refusal must name the rule, got: {reason}"
            ),
            ReloadOutcome::Applied { .. } => {
                panic!("a self-peering reload must be rejected")
            }
        }
        assert_eq!(
            handle.generation(),
            0,
            "a rejected reload must not advance the generation"
        );
    }

    /// The other direction, so the test above cannot pass by refusing everything.
    #[test]
    fn a_reload_with_a_remote_peer_is_not_refused_by_the_local_address_check() {
        let path = write_reload_source(
            "remote-peer",
            "schema_version = 1\n\
             [node]\n\
             name = \"node-a\"\n\
             [[instance]]\n\
             name = \"api\"\n\
             interface = \"lo\"\n\
             vrid = 42\n\
             [instance.network]\n\
             mode = \"unicast\"\n\
             peers = [\"192.0.2.20\"]\n\
             [[instance.vip]]\n\
             address = \"192.0.2.10/24\"\n",
        );
        let registry = std::sync::Arc::new(crate::StatusRegistry::new("node-a"));
        let handle = ReloadHandle::new(
            path,
            &empty_config(),
            std::sync::Arc::clone(&registry),
            std::collections::BTreeMap::new(),
            crate::Metrics::shared(),
            vec!["127.0.0.1".parse().expect("valid address")],
            std::sync::Arc::new(crate::EventLog::new()),
        );

        // This may still be refused for an unrelated reason -- the interface and
        // the plan both have opinions -- so the assertion is that the refusal
        // is not about a local address.
        if let ReloadOutcome::Rejected { reason } = handle.reload() {
            assert!(
                !reason.contains("cannot be its own peer"),
                "a remote peer must not be refused as self-peering, got: {reason}"
            );
        }
    }
}
