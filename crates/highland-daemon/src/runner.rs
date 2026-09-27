// Rust guideline compliant 2026-09-27

//! The process entry point.

use highland_observe::{Event, EventLevel, EventName, EventSink};
use tokio::signal::unix::{SignalKind, signal};

use crate::logging::{self, filter_for};
use crate::options::Options;
use crate::shutdown::{ShutdownPlan, ShutdownReason};
use crate::{Daemon, DaemonError};

/// Runs the daemon until it is asked to stop.
///
/// The signal set is part of the contract: `SIGTERM` and `SIGINT` start the
/// graceful sequence, `SIGHUP` reloads, and a second `SIGTERM` is ignored so
/// that shutdown stays idempotent (`I-31`).
///
/// # Errors
///
/// Returns an error when the configuration cannot be loaded, when logging
/// cannot be initialized, or when the runtime cannot be started.
pub async fn run(options: Options) -> Result<(), DaemonError> {
    let mut daemon = Daemon::prepare(options)?;

    let filter = filter_for(&daemon.config().logging.level);
    logging::init(
        &daemon.config().logging.level,
        &daemon.config().logging.format,
        daemon.config().logging.format == "json",
    )
    .map_err(DaemonError::Logging)?;

    let mut terminate = signal(SignalKind::terminate()).map_err(|source| {
        DaemonError::Runtime(format!("could not install the SIGTERM handler: {source}"))
    })?;
    let mut interrupt = signal(SignalKind::interrupt()).map_err(|source| {
        DaemonError::Runtime(format!("could not install the SIGINT handler: {source}"))
    })?;
    let mut reload = signal(SignalKind::hangup()).map_err(|source| {
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
            _ = reload.recv() => announce(&daemon, EventLevel::Info, "reload_accepted", "sighup"),
        }
    };

    let plan = ShutdownPlan::new(reason, Vec::new());
    for event in daemon.shutdown(plan) {
        tracing::info!(event = %event.name, reason = %event.reason, "highland stopped");
    }
    Ok(())
}

/// Emits a lifecycle event and copies it to the tracing subscriber.
fn announce(daemon: &Daemon, level: EventLevel, reason: &'static str, detail: &str) {
    let event = Event::new(
        EventName::DaemonLifecycle,
        level,
        daemon.config().node.name.clone(),
        None,
        reason,
        detail,
    );
    tracing::info!(event = %event.name, reason = %event.reason, "daemon event");
    daemon.sink().publish(event);
}
