// Rust guideline compliant 2026-09-27

//! Health-check error types.

use std::time::Duration;

use thiserror::Error;

/// Result alias for fallible check operations.
pub type Result<T, E = CheckError> = std::result::Result<T, E>;

/// An error raised by the health-check layer.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum CheckError {
    /// A check result was produced without an explanation.
    #[error("check {check} produced a result without a reason")]
    EmptyReason {
        /// The check that produced the result.
        check: String,
    },

    /// A configured timeout was zero, which is rejected by `V-14`.
    #[error("check {check} has a timeout of {timeout:?}; timeouts must be positive")]
    NonPositiveTimeout {
        /// The check with the invalid timeout.
        check: String,
        /// The rejected timeout.
        timeout: Duration,
    },

    /// A threshold was zero, which is rejected by `V-15`.
    #[error("check {check} has a {threshold} threshold of 0; thresholds start at 1")]
    ZeroThreshold {
        /// The check with the invalid threshold.
        check: String,
        /// Which threshold was invalid.
        threshold: &'static str,
    },

    /// A check type is not implemented in this release.
    #[error("check type {check_type} is not implemented in this release")]
    UnsupportedCheckType {
        /// The requested check type.
        check_type: String,
    },

    /// Command execution was requested without the `command-checks` feature.
    #[error("command checks require the `command-checks` feature and an explicit allow-list")]
    CommandChecksDisabled,
}
