// Rust guideline compliant 2026-09-27

//! The Highland daemon.
//!
//! This crate owns process-level concerns: argument handling, configuration
//! loading, logging and metrics initialization, component construction, signal
//! handling, and the shutdown sequence (SPEC.md, §9.8).
//!
//! The protocol and state-machine work lives in `highland-core` and
//! `highland-vrrp`. This milestone delivers the process skeleton, the
//! configuration load path, and the shutdown budget; VRRP traffic arrives in
//! Milestone 3.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

mod actor;
mod control;
mod driver;
mod executor;
mod logging;
mod options;
mod runner;

mod shutdown;
#[cfg(target_os = "linux")]
mod vrrp_transport;

pub use actor::{Applied, InstanceActor};
pub use control::{ControlService, InstanceStatus, StatusRegistry};
pub use driver::{Instruction, InstructionReceiver, InstructionSender, channel, run_instance};
pub use executor::{
    Executor, Ownership, RecordingTransport, TestHarness, Transport, TransportError,
};
pub use options::{InstancePlan, Options, OptionsError};
pub use runner::{TRANSPORT_AVAILABLE, plan_for, plans, run};

pub use shutdown::{DEFAULT_SHUTDOWN_BUDGET, ShutdownPlan, ShutdownReason};
#[cfg(target_os = "linux")]
pub use vrrp_transport::{READER_INTERVAL, VrrpTransport};

use std::path::{Path, PathBuf};
use std::time::Duration;

use highland_config::{Config, ConfigError, ValidationContext, load, validate};
use highland_core::state::Generation;
use highland_observe::EventLevel;
use highland_observe::{Event, EventName, EventRing, EventSink, RecordingSink};

/// A running daemon, as far as Milestone 0 delivers it.
#[derive(Debug)]
pub struct Daemon {
    options: Options,
    config: Config,
    generation: Generation,
    events: EventRing,
    sink: RecordingSink,
}

impl Daemon {
    /// Loads the configuration and prepares the process.
    ///
    /// # Errors
    ///
    /// Returns [`DaemonError::Config`] when the file cannot be read, parsed, or
    /// validated. A failure here is fatal by design: Highland never starts with
    /// a configuration it could not fully validate (SPEC.md, `I-08`).
    pub fn prepare(options: Options) -> Result<Self, DaemonError> {
        let sink = RecordingSink::new();
        let config = load(&options.config_path, options.allow_insecure_config)
            .map_err(DaemonError::Config)?;
        let context = ValidationContext {
            command_checks_enabled: options.command_checks_enabled,
            ..ValidationContext::permissive()
        };
        validate(&config, &context)
            .map_err(|violations| DaemonError::Config(ConfigError::Invalid { violations }))?;

        let mut events = EventRing::new();
        let startup = Event::new(
            EventName::DaemonLifecycle,
            EventLevel::Info,
            config.node.name.clone(),
            None,
            "startup",
            "1970-01-01T00:00:00Z",
        )
        .with_field("instances", config.instances.len());
        events.push(startup.clone());
        sink.publish(startup);

        Ok(Self {
            options,
            config,
            generation: Generation::initial(),
            events,
            sink,
        })
    }

    /// Returns the process options.
    #[must_use]
    pub fn options(&self) -> &Options {
        &self.options
    }

    /// Returns the loaded configuration.
    #[must_use]
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Returns the active configuration generation.
    #[must_use]
    pub fn generation(&self) -> Generation {
        self.generation
    }

    /// Returns the event history.
    #[must_use]
    pub fn events(&self) -> &EventRing {
        &self.events
    }

    /// Returns the event sink the daemon publishes to.
    #[must_use]
    pub fn sink(&self) -> &RecordingSink {
        &self.sink
    }

    /// Applies the shutdown sequence described by `plan`.
    ///
    /// The plan is data, not a side effect, so that the sequence can be tested
    /// without signals or sockets (SPEC.md, §14.5).
    #[allow(
        clippy::needless_pass_by_value,
        reason = "the plan is consumed as a sequence"
    )]
    pub fn shutdown(&mut self, plan: ShutdownPlan) -> Vec<Event> {
        let event = Event::new(
            EventName::DaemonLifecycle,
            EventLevel::Info,
            self.config.node.name.clone(),
            None,
            plan.reason.as_str(),
            "1970-01-01T00:00:00Z",
        )
        .with_field(
            "budget_ms",
            u64::try_from(plan.budget.as_millis()).unwrap_or(u64::MAX),
        );

        self.events.push(event.clone());
        self.sink.publish(event.clone());
        vec![event]
    }
}

/// An error that prevents the daemon from starting.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DaemonError {
    /// The configuration could not be loaded or validated.
    #[error(transparent)]
    Config(#[from] ConfigError),

    /// Logging could not be initialized.
    #[error("logging could not be initialized: {0}")]
    Logging(String),

    /// The runtime could not be started.
    #[error("the async runtime could not be started: {0}")]
    Runtime(String),

    /// The VRRP transport is not implemented, so the daemon refuses to start.
    ///
    /// Reporting this beats starting a process that claims to be a VRRP router
    /// while sending nothing. It clears with the raw socket, in Milestone 4.
    #[error("the VRRP transport is not implemented; the daemon will not start")]
    TransportUnavailable,
}

/// Reports whether `path` looks like a configuration file the daemon can load.
///
/// Used by the CLI to produce a clearer error than "no such file".
#[must_use]
pub fn config_path_hint(path: &Path) -> String {
    if path
        .extension()
        .is_some_and(|extension| extension == "toml")
    {
        format!("{} (TOML is the only accepted format)", path.display())
    } else {
        path.display().to_string()
    }
}

/// The path the daemon would use for its control socket by default.
#[must_use]
pub fn default_control_socket() -> PathBuf {
    PathBuf::from("/run/highland/control.sock")
}

/// The shutdown budget applied when none is configured.
#[must_use]
pub fn default_shutdown_budget() -> Duration {
    DEFAULT_SHUTDOWN_BUDGET
}
