// Rust guideline compliant 2026-09-27

//! Native health checks for Highland.
//!
//! A check answers one question on an interval and reports a stable
//! `Passing` or `Failing` verdict after its thresholds are met
//! (SPEC.md, §15.3). Checks never block the state machine task; results are
//! delivered to the instance with a sequence number so that a slow check can
//! never overwrite a newer verdict (SPEC.md, `I-26`).
//!
//! | Concern | Where it lives |
//! |---|---|
//! | The check contract and the debounce state | [`Check`], [`CheckSpec`], [`Debouncer`] |
//! | The probes | [`TcpCheck`], [`HttpCheck`], [`UnixCheck`], [`InterfaceCheck`] |
//! | Running them, and deciding when a verdict changed | [`Scheduler`], [`Verdict`] |
//!
//! The `https`, `dns`, `process`, `file`, and `composite` check types are not
//! implemented. `https` is refused rather than downgraded: connecting to port 443
//! without validating a certificate would report a service as healthy on the
//! strength of a plaintext exchange.
//!
//! Command execution lives behind the default-off `command-checks` feature and
//! is not implemented.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

mod check;
mod error;
mod http;
mod interface;
mod result;
mod scheduler;
mod tcp;
mod unix;

pub use check::{Check, CheckKind, CheckSpec, Debouncer, Stability};
pub use error::{CheckError, Result};
pub use http::{HttpCheck, MAX_RESPONSE_BYTES, Response, Url, parse_status_line};
pub use interface::{InterfaceCheck, LinkProbe, LinkState};
pub use result::{CheckResult, CheckStatus};
// The summary the state machine consumes, re-exported rather than redefined: two
// summaries with the same name and different fields is a conversion somebody has
// to remember to write, and forgetting it fails silently.
pub use highland_core::health::HealthSummary;
pub use scheduler::{HealthReport, Scheduler, Verdict};
pub use tcp::TcpCheck;
pub use unix::UnixCheck;
