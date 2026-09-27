// Rust guideline compliant 2026-09-27

//! Daemon process options.

use std::net::IpAddr;
use std::path::PathBuf;

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
    pub shutdown_budget: std::time::Duration,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            config_path: PathBuf::from("/etc/highland/config.toml"),
            allow_insecure_config: false,
            force_transition_enabled: false,
            command_checks_enabled: cfg!(feature = "command-checks"),
            local_addresses: Vec::new(),
            shutdown_budget: crate::shutdown::DEFAULT_SHUTDOWN_BUDGET,
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
