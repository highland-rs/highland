// Rust guideline compliant 2026-09-27

//! The local administrative control API.
//!
//! The control API is a Unix domain socket carrying newline-delimited JSON
//! (SPEC.md, §9.7). It has one operation set, and the CLI maps onto it exactly
//! (§22.1): a command that exists in the CLI exists here, and nothing else.
//!
//! The API is local only. It MUST NOT be exposed over a network transport in
//! any release covered by this specification (`S-02`), and the socket MUST be
//! created with mode `0660` (`S-03`).

#![deny(missing_docs)]
#![forbid(unsafe_code)]

mod error;
mod message;
mod rate;

pub use error::{ControlError, Result};
pub use message::{
    ControlRequest, ControlResponse, InstanceSummary, MAX_REQUEST_BYTES, NodeStatus,
};
pub use rate::RateLimiter;
