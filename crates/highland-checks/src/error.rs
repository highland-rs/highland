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

    /// A check's target could not be used as configured.
    ///
    /// This is a configuration error found when the check is built, not a probe
    /// failure: a check that can never succeed should stop the daemon starting
    /// rather than fail on every interval forever.
    #[error("check {check} cannot use {target}: {detail}")]
    UnresolvableTarget {
        /// The check whose target is unusable.
        check: String,
        /// The target as configured.
        target: String,
        /// What is wrong with it, in a sentence an operator can act on.
        detail: String,
    },

    /// The state of an interface could not be read.
    #[error("could not read the state of interface {interface}: {detail}")]
    InterfaceUnreadable {
        /// The interface the check asked about.
        interface: String,
        /// The underlying error, as text: the caller may have a typed error, and
        /// this crate must not depend on where it came from. The field is not
        /// called `source` because that name means "the error" to `thiserror`,
        /// and a string is not one.
        detail: String,
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
