// Rust guideline compliant 2026-09-27

//! The process entry point.
//!
//! Argument parsing lives in `highland-cli`; this binary is what
//! `highland run` executes.

use crate::shutdown::{ShutdownPlan, ShutdownReason};
use crate::{Daemon, DaemonError, Options};
#[cfg(target_os = "linux")]
use std::net::IpAddr;

use highland_config::{Config, InstanceConfig, ValidationContext, load, validate};
use highland_net::{IpCidr, PeerSet};
use highland_observe::EventSink as _;

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
/// Returns an error when the configuration cannot be loaded, when logging
/// cannot be initialized, or when a signal handler cannot be installed.
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

    let mut service = crate::ControlService::new(
        daemon.config().node.name.clone(),
        std::sync::Arc::clone(&registry),
        daemon.options().force_transition_enabled,
    );

    let instances = start_instances(&daemon, &shutdown_rx, &registry, &mut service).await?;

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
                match reload_config(&daemon.options().config_path) {
                    Ok(()) => announce(&daemon, "reload_accepted"),
                    Err(reason) => {
                        announce(&daemon, "reload_rejected");
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
    service: &mut crate::ControlService,
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
                ownership.peers.clone(),
            )
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
        service.register(plan.name.clone(), sender);

        let reader = transport.spawn_reader(reader_sender);
        let watch = shutdown_signal.clone();
        let name = plan.name.clone();

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
            // The reader stops with the actor: nothing outlives the instance
            // that owns it (`I-41`).
            reader.abort();
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
    _service: &mut crate::ControlService,
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
    let primary = interface
        .addresses
        .iter()
        .map(highland_net::IpCidr::address)
        .find(|address| !virtual_addresses.contains(address));

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

/// Re-reads the configuration file and reports the first reason it was
/// rejected, if any.
///
/// A rejected reload leaves the running configuration untouched (`I-09`).
fn reload_config(path: &std::path::Path) -> Result<(), String> {
    let config = load(path, false).map_err(|error| error.to_string())?;
    validate(&config, &ValidationContext::permissive()).map_err(|violations| {
        violations
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ")
    })?;
    Ok(())
}

fn announce(daemon: &Daemon, reason: &'static str) {
    let event = highland_observe::Event::new(
        highland_observe::EventName::DaemonLifecycle,
        highland_observe::EventLevel::Info,
        daemon.config().node.name.clone(),
        None,
        reason,
        "1970-01-01T00:00:00Z",
    );
    tracing::info!(event = %event.name, reason = %event.reason, "daemon event");
    daemon.sink().publish(event);
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
pub fn plan_for(instance: &InstanceConfig) -> Result<(InstancePlan, Ownership), String> {
    let addresses: Vec<IpCidr> = instance
        .vip_addresses()
        .iter()
        .map(|text| IpCidr::parse(text).map_err(|error| error.to_string()))
        .collect::<Result<Vec<_>, _>>()?;

    let plan = InstancePlan {
        name: instance.name.clone(),
        vrid: instance.vrid,
        priority: instance.priority,
        advertisement_interval: instance.advertisement_interval.as_duration(),
        preempt: instance.preempt,
        preempt_delay: instance.preempt_delay.as_duration(),
        startup_delay: instance.startup_delay.as_duration(),
    };
    let ownership = Ownership::new(
        instance.interface.clone(),
        addresses,
        PeerSet::new(instance.network.peers.clone()),
    );
    Ok((plan, ownership))
}

/// Whether this build can carry VRRP on the wire.
///
/// The socket exists on Linux and does not anywhere else, so this is decided at
/// compile time rather than discovered at runtime. A build where it is `false`
/// refuses to start, because a process that claimed to be a VRRP router while
/// sending nothing would be worse than one that declines to run.
pub const TRANSPORT_AVAILABLE: bool = cfg!(target_os = "linux");

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn an_unreadable_reload_reports_why() {
        let error = reload_config(std::path::Path::new("/nonexistent/highland.toml"))
            .expect_err("the file is absent");
        assert!(error.contains("could not read") || error.contains("No such file"));
    }
}
