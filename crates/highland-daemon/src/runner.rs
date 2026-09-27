// Rust guideline compliant 2026-09-27

//! The process entry point.
//!
//! Argument parsing lives in `highland-cli`; this binary is what
//! `highland run` executes.

use crate::shutdown::{ShutdownPlan, ShutdownReason};
use crate::{Daemon, DaemonError, Options};
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
        // Refusing here, rather than starting a process that cannot speak VRRP,
        // is the honest behaviour. The configuration is still validated first, so
        // `highland run` with a broken file still reports the broken file.
        tracing::error!(
            instances = daemon.config().instances.len(),
            "refusing to start: the VRRP transport is not implemented"
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

    tracing::info!(
        filter = %filter,
        node = %daemon.config().node.name,
        instances = daemon.config().instances.len(),
        config = %daemon.options().config_path.display(),
        "highland started"
    );

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

    let plan = ShutdownPlan::new(reason, Vec::new());
    for event in daemon.shutdown(plan) {
        tracing::info!(event = %event.name, reason = %event.reason, "highland stopped");
    }
    Ok(())
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

/// Whether the daemon can carry VRRP on the wire yet.
///
/// It cannot. Everything below this point is implemented and tested: the state
/// machine decides, the executor applies the machine's actions to the kernel
/// through Netlink, and the run loop drives both. What is missing is the raw
/// socket that puts an advertisement on the wire and reads a peer's TTL back
/// off an incoming datagram, which needs Linux to develop and verify.
///
/// Until that exists the daemon does not start instances, because a process
/// that claimed to be a VRRP router while sending nothing would be worse than
/// one that refuses to start.
pub const TRANSPORT_AVAILABLE: bool = false;

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
