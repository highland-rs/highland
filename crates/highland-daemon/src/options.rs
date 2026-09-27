// Rust guideline compliant 2026-09-27

//! Daemon process options and the per-instance plan the executor applies.

use std::net::IpAddr;
use std::path::PathBuf;
use std::time::Duration;

use crate::shutdown::DEFAULT_SHUTDOWN_BUDGET;

/// The command-line options accepted by `highland run`.
///
/// The defaults are the safe ones: a world-writable configuration file is
/// refused, and `force-transition` is disabled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// The configuration file to load.
    pub config_path: PathBuf,
    /// Whether a world-writable configuration file is accepted (`V-26`).
    pub allow_insecure_config: bool,
    /// Whether `force-transition` is enabled (`R-10`).
    pub force_transition_enabled: bool,
    /// Whether command checks were compiled in (`V-21`).
    pub command_checks_enabled: bool,
    /// Addresses configured on this node, used to reject self-peering (`V-08`).
    pub local_addresses: Vec<IpAddr>,
    /// The shutdown budget (`shutdown.budget`).
    pub shutdown_budget: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            config_path: PathBuf::from("/etc/highland/config.toml"),
            allow_insecure_config: false,
            force_transition_enabled: false,
            command_checks_enabled: cfg!(feature = "command-checks"),
            local_addresses: Vec::new(),
            shutdown_budget: DEFAULT_SHUTDOWN_BUDGET,
        }
    }
}

impl Options {
    /// Returns options for `config_path`, keeping every other default.
    #[must_use]
    pub fn with_config(config_path: impl Into<PathBuf>) -> Self {
        Self {
            config_path: config_path.into(),
            ..Self::default()
        }
    }
}

/// The error returned when options cannot be used.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum OptionsError {
    /// The configuration path is empty.
    #[error("a configuration path is required")]
    MissingConfigPath,

    /// A flag combination is not permitted.
    #[error("{0}")]
    InvalidCombination(String),
}

/// What one instance is configured to do, in the terms the executor needs.
///
/// This is the seam between `highland-config`, which parses and validates a
/// document, and the state machine, which must not know how a document is
/// written (`SPEC.md` §9.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstancePlan {
    /// The instance name.
    pub name: String,
    /// The VRID, which must be in `1..=255` by `V-01`.
    pub vrid: u8,
    /// The configured priority, in `1..=255` by `V-02`.
    pub priority: u8,
    /// The advertisement interval, in `10ms..=40950ms` by `V-04`.
    pub advertisement_interval: Duration,
    /// Whether a higher-priority `BACKUP` may preempt.
    pub preempt: bool,
    /// The delay before preemption, which `V-10` forbids when `preempt` is false.
    pub preempt_delay: Duration,
    /// The delay between startup and entering election.
    pub startup_delay: Duration,
}

impl InstancePlan {
    /// Creates a plan for an instance with the specification's defaults.
    #[must_use]
    pub fn new(name: impl Into<String>, vrid: u8, priority: u8) -> Self {
        Self {
            name: name.into(),
            vrid,
            priority,
            advertisement_interval: Duration::from_secs(1),
            preempt: true,
            preempt_delay: Duration::ZERO,
            startup_delay: Duration::ZERO,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_refuse_an_insecure_file_and_disable_forced_transitions() {
        let options = Options::default();
        assert!(
            !options.allow_insecure_config,
            "V-26: the safe default is to refuse"
        );
        assert!(
            !options.force_transition_enabled,
            "R-10: forced transitions are opt-in"
        );
        assert_eq!(options.shutdown_budget, Duration::from_secs(5));
    }

    #[test]
    fn a_plan_defaults_to_one_second_and_preemption() {
        let plan = InstancePlan::new("api", 42, 150);
        assert_eq!(plan.advertisement_interval, Duration::from_secs(1));
        assert!(plan.preempt);
        assert_eq!(plan.preempt_delay, Duration::ZERO);
        assert_eq!(plan.name, "api");
    }
}
