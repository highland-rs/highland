// Rust guideline compliant 2026-09-27

//! Native health checks for Highland.
//!
//! A check answers one question on an interval and reports a stable
//! `Passing` or `Failing` verdict after its thresholds are met
//! (SPEC.md, §15.3). Checks never block the state machine task; results are
//! delivered to the instance with a sequence number so that a slow check can
//! never overwrite a newer verdict (SPEC.md, `I-26`).
//!
//! Command execution lives behind the default-off `command-checks` feature and
//! is not implemented in this milestone.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

mod check;
mod error;
mod result;

pub use check::{Check, CheckKind, CheckSpec, Debouncer, Stability};
pub use error::{CheckError, Result};
pub use result::{CheckResult, CheckStatus, HealthSummary};
