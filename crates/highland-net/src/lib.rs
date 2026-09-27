// Rust guideline compliant 2026-09-27

//! Linux networking integration for Highland.
//!
//! Every Linux-specific operation sits behind a trait so that the daemon can
//! be tested against a scripted backend (SPEC.md, `R-03`). The production
//! backend is delivered in Milestone 3; the library selects its Netlink
//! implementation in Milestone 0 (Appendix B, `B-02`).
//!
//! This crate does not own role state. It applies the actions emitted by
//! `highland-core` and reports the outcome back, including confirmation by
//! read-back (SPEC.md, `I-19`).

#![deny(missing_docs)]
#![forbid(unsafe_code)]

mod backend;
mod error;
mod types;

pub use backend::NetworkBackend;
pub use error::{NetError, Result};
pub use types::{
    AddrParseError, Interface, InterfaceId, IpCidr, LinkState, NegativeInterfaceIndex,
    PrefixLenError,
};
