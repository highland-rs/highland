// Rust guideline compliant 2026-09-27

//! Errors produced by pure domain logic.
//!
//! Domain errors in this crate are always recoverable conditions detected
//! before or instead of a state transition. Panics are reserved for
//! unrecoverable defects and MUST NOT be used for control flow
//! (`M-PANIC-IS-STOP`).

use std::time::Duration;

use crate::state::{Role, TimerId};

/// Result alias for fallible operations in `highland-core`.
pub type Result<T, E = CoreError> = std::result::Result<T, E>;

/// An error raised by pure domain logic.
///
/// # Examples
///
/// ```
/// use highland_core::{CoreError, state::TimerId};
/// use std::time::Duration;
///
/// let error = CoreError::NonPositiveTimerDuration {
///     timer: TimerId::MasterDown,
///     duration: Duration::ZERO,
/// };
///
/// assert_eq!(error.to_string(), "timer MasterDown cannot be armed for 0ns: duration must be positive");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum CoreError {
    /// A timer was armed with a zero or sub-millisecond duration.
    ///
    /// Timer durations are always derived from configuration and clock values;
    /// a non-positive duration indicates a defect in the caller.
    #[error("timer {timer} cannot be armed for {duration:?}: duration must be positive")]
    NonPositiveTimerDuration {
        /// The timer that was requested.
        timer: TimerId,
        /// The rejected duration.
        duration: Duration,
    },

    /// A timer duration computation exceeded the representable range.
    #[error("timer {timer} duration overflowed: {operation} produced more than {max:?}")]
    TimerDurationOverflow {
        /// The timer whose duration overflowed.
        timer: TimerId,
        /// The arithmetic that overflowed, for example `3 * adver_int`.
        operation: &'static str,
        /// The largest representable duration.
        max: Duration,
    },

    /// A role change was requested that the current role does not permit.
    #[error("cannot enter role {target} from role {from} without an event that permits it")]
    IllegalRoleChange {
        /// The role the instance currently occupies.
        from: Role,
        /// The role that was requested.
        target: Role,
    },

    /// An action referenced a timer that the instance does not own.
    #[error("timer {timer} is not owned by instance {instance}")]
    UnknownTimer {
        /// The instance that received the action.
        instance: String,
        /// The timer that was referenced.
        timer: TimerId,
    },

    /// An event carried a generation older than the instance's active one.
    #[error("stale result for instance {instance}: generation {stale} is older than {current}")]
    StaleGeneration {
        /// The instance that rejected the result.
        instance: String,
        /// The generation carried by the result.
        stale: u64,
        /// The instance's active generation.
        current: u64,
    },
}
