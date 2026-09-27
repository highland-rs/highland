// Rust guideline compliant 2026-09-27

//! Check results.

use std::fmt;
use std::time::{Duration, Instant};

use highland_core::state::Generation;

use crate::error::{CheckError, Result};

/// The outcome of a single probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CheckStatus {
    /// The probe succeeded.
    Passing,
    /// The probe failed.
    Failing,
    /// The probe exceeded its timeout.
    TimedOut,
    /// The check is not running, for example because the instance is disabled.
    Disabled,
}

impl CheckStatus {
    /// Returns `true` when a probe with this status counts as a failure for
    /// threshold purposes.
    ///
    /// A timeout is a failure, not a separate stable state
    /// (SPEC.md, §15.3).
    #[must_use]
    pub fn is_failure(self) -> bool {
        matches!(self, CheckStatus::Failing | CheckStatus::TimedOut)
    }
}

impl fmt::Display for CheckStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            CheckStatus::Passing => "passing",
            CheckStatus::Failing => "failing",
            CheckStatus::TimedOut => "timed_out",
            CheckStatus::Disabled => "disabled",
        })
    }
}

/// The result of one probe.
///
/// `sequence` increases monotonically per check. A result whose sequence is
/// lower than the last accepted result is discarded (SPEC.md, `I-26`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckResult {
    /// The name of the check that produced the result.
    pub check: String,
    /// The probe outcome.
    pub status: CheckStatus,
    /// How long the probe took, when it completed.
    pub latency: Option<Duration>,
    /// A human-readable explanation, safe to log.
    pub reason: String,
    /// When the probe completed, on the instance's clock.
    pub observed_at: Instant,
    /// The instance's configuration generation when the probe started.
    pub generation: Generation,
    /// The probe's sequence number within this check.
    pub sequence: u64,
}

impl CheckResult {
    /// Creates a passing result.
    ///
    /// # Errors
    ///
    /// Returns [`CheckError::EmptyReason`] when `reason` is blank, so that a
    /// result is never unexplainable (SPEC.md, `D-08`).
    pub fn passing(
        check: impl Into<String>,
        latency: Duration,
        observed_at: Instant,
        generation: Generation,
        sequence: u64,
        reason: impl Into<String>,
    ) -> Result<Self> {
        Self::new(
            check,
            CheckStatus::Passing,
            Some(latency),
            observed_at,
            generation,
            sequence,
            reason,
        )
    }

    /// Creates a failing result.
    ///
    /// # Errors
    ///
    /// Returns [`CheckError::EmptyReason`] when `reason` is blank.
    pub fn failing(
        check: impl Into<String>,
        latency: Option<Duration>,
        observed_at: Instant,
        generation: Generation,
        sequence: u64,
        reason: impl Into<String>,
    ) -> Result<Self> {
        Self::new(
            check,
            CheckStatus::Failing,
            latency,
            observed_at,
            generation,
            sequence,
            reason,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn new(
        check: impl Into<String>,
        status: CheckStatus,
        latency: Option<Duration>,
        observed_at: Instant,
        generation: Generation,
        sequence: u64,
        reason: impl Into<String>,
    ) -> Result<Self> {
        let reason = reason.into();
        if reason.trim().is_empty() {
            return Err(CheckError::EmptyReason {
                check: check.into(),
            });
        }
        Ok(Self {
            check: check.into(),
            status,
            latency,
            reason,
            observed_at,
            generation,
            sequence,
        })
    }

    /// Returns `true` when this result is newer than `other`.
    #[must_use]
    pub fn supersedes(&self, other: &Self) -> bool {
        self.check == other.check && self.sequence > other.sequence
    }

    /// Returns `true` when this result was produced under an older generation.
    #[must_use]
    pub fn is_stale_for(&self, current: Generation) -> bool {
        self.generation < current
    }
}

/// The aggregated health of one instance, as consumed by the state machine.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HealthSummary {
    /// The sum of the weights of the checks currently failing.
    pub penalty: u16,
    /// The number of checks currently failing.
    pub failing: usize,
    /// The number of checks currently passing.
    pub passing: usize,
    /// The number of checks whose result has been discarded as stale.
    pub stale_discarded: usize,
}

impl HealthSummary {
    /// Returns `true` when nothing is failing.
    #[must_use]
    pub fn is_healthy(&self) -> bool {
        self.failing == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(sequence: u64, status: CheckStatus) -> CheckResult {
        let constructed = match status {
            CheckStatus::Passing => CheckResult::passing(
                "api",
                Duration::from_millis(5),
                Instant::now(),
                Generation::initial(),
                sequence,
                "connection accepted",
            ),
            _ => CheckResult::failing(
                "api",
                None,
                Instant::now(),
                Generation::initial(),
                sequence,
                "connection refused",
            ),
        };
        constructed.expect("reason is not blank")
    }

    #[test]
    fn a_timeout_counts_as_a_failure() {
        assert!(CheckStatus::TimedOut.is_failure());
        assert!(CheckStatus::Failing.is_failure());
        assert!(!CheckStatus::Passing.is_failure());
        assert!(!CheckStatus::Disabled.is_failure());
    }

    #[test]
    fn only_a_newer_sequence_supersedes() {
        let first = result(1, CheckStatus::Passing);
        let second = result(2, CheckStatus::Failing);

        assert!(second.supersedes(&first));
        assert!(!first.supersedes(&second));
        assert!(!first.supersedes(&first));
    }

    #[test]
    fn results_from_an_older_generation_are_stale() {
        let older = CheckResult::failing(
            "api",
            None,
            Instant::now(),
            Generation::initial(),
            1,
            "connection refused",
        )
        .expect("reason is not blank");
        assert!(older.is_stale_for(Generation::initial().next()));
        assert!(!older.is_stale_for(Generation::initial()));
    }

    #[test]
    fn a_result_without_a_reason_is_rejected() {
        let error = CheckResult::passing(
            "api",
            Duration::from_millis(1),
            Instant::now(),
            Generation::initial(),
            1,
            "   ",
        );
        assert!(matches!(error, Err(CheckError::EmptyReason { .. })));
    }

    #[test]
    fn statuses_render_in_the_documented_spelling() {
        assert_eq!(CheckStatus::TimedOut.to_string(), "timed_out");
        assert_eq!(CheckStatus::Disabled.to_string(), "disabled");
    }
}
