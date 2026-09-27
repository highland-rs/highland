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
    /// The health checks, already validated, in configuration order.
    pub checks: Vec<CheckPlan>,
    /// The health policy the state machine applies to them.
    pub health: highland_core::health::HealthPolicyConfig,
}

/// The health policy in force, with the instance's own total weight as the
/// ceiling (`I-24`).
fn health_policy(
    instance: &highland_config::InstanceConfig,
) -> highland_core::health::HealthPolicyConfig {
    use highland_config::FailurePolicy;
    let health = &instance.health;
    highland_core::health::HealthPolicyConfig {
        policy: match health.failure_policy {
            FailurePolicy::FailClosed => highland_core::health::HealthPolicy::FailClosed,
            FailurePolicy::Weighted => highland_core::health::HealthPolicy::Weighted,
            FailurePolicy::Manual => highland_core::health::HealthPolicy::Manual,
        },
        minimum_effective_priority: health.minimum_effective_priority,
        all_checks_required: health.all_checks_required,
        send_zero_priority_advert: health.send_zero_priority_advert,
        total_weight: instance.electoral_weight(),
    }
}

/// One configured health check, in the terms the checks crate needs.
///
/// The same shape as the configuration, without the document: a plan is what
/// the daemon runs, and a plan must not be able to be wrong in a way the
/// configuration layer already refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckPlan {
    /// The check's name, unique within the instance (`V-29`).
    pub name: String,
    /// The probe type, as the configuration spells it.
    pub check_type: String,
    /// The priority penalty while failing. Zero is observational (`R-14`).
    pub weight: u16,
    /// How often the probe runs.
    pub interval: Duration,
    /// How long a probe may take.
    pub timeout: Duration,
    /// Consecutive failures required to enter the failing state.
    pub failure_threshold: u32,
    /// Consecutive successes required to recover.
    pub success_threshold: u32,
    /// Failures are ignored for this long after the check starts.
    pub initial_grace_period: Duration,
    /// The target address, for `tcp`.
    pub address: Option<String>,
    /// The target URL, for `http` and `https`.
    pub url: Option<String>,
    /// The accepted status codes, for `http` and `https`.
    pub expected_status: Option<Vec<u16>>,
    /// The socket path, for `unix`.
    pub path: Option<String>,
    /// The interface name, for `interface`.
    pub interface: Option<String>,
    /// The address an `interface` check must find.
    pub expected_address: Option<String>,
}

impl CheckPlan {
    /// Converts one configured check into a plan.
    #[must_use]
    pub fn from_config(check: &highland_config::CheckConfig) -> Self {
        Self {
            name: check.name.clone(),
            check_type: check.check_type.clone(),
            weight: check.weight,
            interval: check.interval.as_duration(),
            timeout: check.timeout.as_duration(),
            failure_threshold: check.failure_threshold,
            success_threshold: check.success_threshold,
            initial_grace_period: check.initial_grace_period.as_duration(),
            address: check.address.clone(),
            url: check.url.clone(),
            expected_status: check.expected_status.clone(),
            path: check.path.clone(),
            interface: check.interface.clone(),
            // An `interface` check may also require an address, which the
            // configuration spells in the same `address` key: a check watching
            // "this link, holding 192.0.2.10" is written that way naturally, and
            // it catches a link that is up but unconfigured.
            expected_address: check.address.clone(),
        }
    }
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
            checks: Vec::new(),
            health: highland_core::health::HealthPolicyConfig::default(),
        }
    }

    /// Converts a configured instance into a plan.
    ///
    /// The one conversion from configuration to plan, so the daemon, the reload
    /// planner, and a test all see the same instance. Two copies of this is how a
    /// reloaded field quietly keeps its old value.
    #[must_use]
    pub fn from_config(instance: &highland_config::InstanceConfig) -> Self {
        Self {
            name: instance.name.clone(),
            vrid: instance.vrid,
            priority: instance.priority,
            advertisement_interval: instance.advertisement_interval.as_duration(),
            preempt: instance.preempt,
            preempt_delay: instance.preempt_delay.as_duration(),
            startup_delay: instance.startup_delay.as_duration(),
            checks: instance.checks.iter().map(CheckPlan::from_config).collect(),
            health: health_policy(instance),
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
