// Rust guideline compliant 2026-09-27

//! The check abstraction and its debounce state machine.

use std::fmt;
use std::pin::Pin;
use std::time::Duration;

use highland_core::state::Generation;

use crate::error::{CheckError, Result};
use crate::result::{CheckResult, CheckStatus};

/// The type of probe a check performs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum CheckKind {
    /// A TCP connect.
    Tcp,
    /// An HTTP request.
    #[default]
    Http,
    /// An HTTPS request.
    Https,
    /// A DNS query.
    Dns,
    /// A Unix domain socket connect.
    Unix,
    /// A process-existence probe. Observational by default.
    Process,
    /// An interface or carrier probe.
    Interface,
    /// A file-existence probe confined to an allowed base directory.
    File,
    /// An aggregation of other checks.
    Composite,
    /// An external command. Requires the `command-checks` feature.
    Command,
}

impl CheckKind {
    /// Returns `true` when this check type is part of the initial public
    /// release (SPEC.md, §3.1).
    #[must_use]
    pub fn is_initial_release(self) -> bool {
        matches!(self, CheckKind::Tcp | CheckKind::Http)
    }

    /// Returns the configuration spelling of this check type.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            CheckKind::Tcp => "tcp",
            CheckKind::Http => "http",
            CheckKind::Https => "https",
            CheckKind::Dns => "dns",
            CheckKind::Unix => "unix",
            CheckKind::Process => "process",
            CheckKind::Interface => "interface",
            CheckKind::File => "file",
            CheckKind::Composite => "composite",
            CheckKind::Command => "command",
        }
    }
}

impl fmt::Display for CheckKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The configured parameters of one check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckSpec {
    /// The check name, unique within its instance.
    pub name: String,
    /// The probe type.
    pub kind: CheckKind,
    /// How often the probe runs.
    pub interval: Duration,
    /// How long a probe may take.
    pub timeout: Duration,
    /// Consecutive failures required to enter the failing state.
    pub failure_threshold: u32,
    /// Consecutive successes required to recover.
    pub success_threshold: u32,
    /// The priority penalty applied while failing. Zero is observational
    /// (SPEC.md, `R-14`).
    pub weight: u16,
    /// Failures are ignored for this long after the check starts.
    pub initial_grace_period: Duration,
}

impl CheckSpec {
    /// Creates a check specification with the documented defaults.
    ///
    /// # Errors
    ///
    /// Returns [`CheckError::NonPositiveTimeout`] for a zero timeout and
    /// [`CheckError::ZeroThreshold`] for a zero threshold.
    pub fn new(
        name: impl Into<String>,
        kind: CheckKind,
        interval: Duration,
        timeout: Duration,
        failure_threshold: u32,
        success_threshold: u32,
        weight: u16,
    ) -> Result<Self> {
        let name = name.into();
        if timeout.is_zero() {
            return Err(CheckError::NonPositiveTimeout {
                check: name,
                timeout,
            });
        }
        if failure_threshold == 0 {
            return Err(CheckError::ZeroThreshold {
                check: name,
                threshold: "failure",
            });
        }
        if success_threshold == 0 {
            return Err(CheckError::ZeroThreshold {
                check: name,
                threshold: "success",
            });
        }
        Ok(Self {
            name,
            kind,
            interval,
            timeout,
            failure_threshold,
            success_threshold,
            weight,
            initial_grace_period: Duration::ZERO,
        })
    }

    /// Returns `true` when this check can affect election.
    #[must_use]
    pub fn is_electoral(&self) -> bool {
        self.weight > 0
    }
}

/// The debounce state of one check.
///
/// # Examples
///
/// ```
/// use highland_checks::{CheckKind, CheckSpec, CheckStatus, Debouncer, Stability};
/// use std::time::Duration;
///
/// let spec = CheckSpec::new("api", CheckKind::Tcp, Duration::from_secs(2), Duration::from_millis(500), 3, 2, 50).unwrap();
/// let mut debouncer = Debouncer::new(&spec);
/// assert_eq!(debouncer.state(), Stability::Unknown);
///
/// // Two consecutive successes are required before the verdict becomes Passing.
/// assert_eq!(debouncer.observe(CheckStatus::Passing), None);
/// assert_eq!(debouncer.observe(CheckStatus::Passing), Some(Stability::Passing));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Stability {
    /// No conclusion has been reached yet.
    Unknown,
    /// The check is passing.
    Passing,
    /// The check is failing.
    Failing,
}

/// Debounces raw probe results into a stable verdict.
#[derive(Debug, Clone)]
pub struct Debouncer {
    state: Stability,
    consecutive_failures: u32,
    consecutive_successes: u32,
    failure_threshold: u32,
    success_threshold: u32,
}

impl Debouncer {
    /// Creates a debouncer in the unknown state.
    #[must_use]
    pub fn new(spec: &CheckSpec) -> Self {
        Self {
            state: Stability::Unknown,
            consecutive_failures: 0,
            consecutive_successes: 0,
            failure_threshold: spec.failure_threshold,
            success_threshold: spec.success_threshold,
        }
    }

    /// Returns the current stable verdict.
    #[must_use]
    pub fn state(&self) -> Stability {
        self.state
    }

    /// Feeds one probe result and returns the new verdict if it changed.
    ///
    /// A single failed probe never changes the verdict unless
    /// `failure_threshold` is one (SPEC.md, `R-13`).
    pub fn observe(&mut self, status: CheckStatus) -> Option<Stability> {
        let previous = self.state;
        if status.is_failure() {
            self.consecutive_failures = self.consecutive_failures.saturating_add(1);
            self.consecutive_successes = 0;
            if self.consecutive_failures >= self.failure_threshold {
                self.state = Stability::Failing;
            }
        } else {
            self.consecutive_successes = self.consecutive_successes.saturating_add(1);
            self.consecutive_failures = 0;
            if self.consecutive_successes >= self.success_threshold {
                self.state = Stability::Passing;
            }
        }
        (self.state != previous).then_some(self.state)
    }
}

/// A health probe.
///
/// Implementations are async and must be bounded by their configured timeout.
/// The executor provides the timeout; an implementation MUST NOT block
/// (SPEC.md, `R-05`).
pub trait Check: Send + Sync + fmt::Debug {
    /// Returns the specification this check was built from.
    fn spec(&self) -> &CheckSpec;

    /// Runs one probe.
    ///
    /// The future is boxed rather than declared with `impl Future`, and the
    /// reason is the scheduler: one instance runs checks of *different* types —
    /// a TCP connect and an HTTP request are not the same type — so they have to
    /// sit in one collection to be scheduled together. A boxed future is one
    /// allocation per probe, against a network round trip per probe.
    ///
    /// # Errors
    ///
    /// Returns a [`CheckError`] describing why the probe could not be run. A
    /// failed probe is reported as a [`CheckResult`] with a failing status
    /// rather than as an error, so that one broken check never interrupts an
    /// instance.
    fn run(
        &self,
        generation: Generation,
        sequence: u64,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<CheckResult>> + Send + '_>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(failure_threshold: u32, success_threshold: u32) -> CheckSpec {
        CheckSpec::new(
            "api",
            CheckKind::Tcp,
            Duration::from_secs(1),
            Duration::from_millis(500),
            failure_threshold,
            success_threshold,
            100,
        )
        .expect("fixture is valid")
    }

    #[test]
    fn a_single_failure_does_not_change_the_verdict() {
        let mut debouncer = Debouncer::new(&spec(3, 2));
        assert_eq!(debouncer.observe(CheckStatus::Failing), None);
        assert_eq!(debouncer.state(), Stability::Unknown);
    }

    #[test]
    fn the_verdict_changes_on_the_configured_threshold() {
        let mut debouncer = Debouncer::new(&spec(3, 2));
        assert_eq!(debouncer.observe(CheckStatus::Failing), None);
        assert_eq!(debouncer.observe(CheckStatus::Failing), None);
        assert_eq!(
            debouncer.observe(CheckStatus::Failing),
            Some(Stability::Failing)
        );
        assert_eq!(debouncer.state(), Stability::Failing);
    }

    #[test]
    fn recovery_requires_consecutive_successes() {
        let mut debouncer = Debouncer::new(&spec(1, 2));
        assert_eq!(
            debouncer.observe(CheckStatus::Failing),
            Some(Stability::Failing)
        );
        assert_eq!(debouncer.observe(CheckStatus::Passing), None);
        assert_eq!(
            debouncer.observe(CheckStatus::Passing),
            Some(Stability::Passing)
        );
    }

    #[test]
    fn a_zero_weight_check_is_observational() {
        let mut observational = spec(1, 1);
        observational.weight = 0;
        assert!(!observational.is_electoral());
        assert!(spec(1, 1).is_electoral());
    }

    #[test]
    fn invalid_thresholds_are_rejected() {
        assert!(matches!(
            CheckSpec::new(
                "api",
                CheckKind::Tcp,
                Duration::from_secs(1),
                Duration::ZERO,
                1,
                1,
                0
            ),
            Err(CheckError::NonPositiveTimeout { .. })
        ));
        assert!(matches!(
            CheckSpec::new(
                "api",
                CheckKind::Tcp,
                Duration::from_secs(1),
                Duration::from_secs(1),
                0,
                1,
                0
            ),
            Err(CheckError::ZeroThreshold {
                threshold: "failure",
                ..
            })
        ));
        assert!(matches!(
            CheckSpec::new(
                "api",
                CheckKind::Tcp,
                Duration::from_secs(1),
                Duration::from_secs(1),
                1,
                0,
                0
            ),
            Err(CheckError::ZeroThreshold {
                threshold: "success",
                ..
            })
        ));
    }

    #[test]
    fn only_tcp_and_http_are_in_the_initial_release() {
        assert!(CheckKind::Tcp.is_initial_release());
        assert!(CheckKind::Http.is_initial_release());
        assert!(!CheckKind::Command.is_initial_release());
    }
}
