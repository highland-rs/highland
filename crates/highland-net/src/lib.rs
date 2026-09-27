// Rust guideline compliant 2026-09-27

//! Linux networking integration for Highland.
//!
//! Every Linux-specific operation sits behind a trait, so the daemon can be
//! tested against a scripted backend (SPEC.md, `R-03`).
//!
//! | Concern | Where it lives |
//! |---|---|
//! | Typed wrappers around the Linux concepts | [`Interface`], [`InterfaceId`], [`IpCidr`], [`LinkState`] |
//! | The backend contract | [`NetworkBackend`] |
//! | The VRRP transport: validation, peer filtering, rate limiting | [`validate`], [`PeerSet`], [`RateWindow`], [`Rejection`] |
//! | The Linux Netlink implementation | `NetlinkBackend`, on Linux only |
//! | The raw VRRP socket | `VrrpSocket`, on Linux only |
//! | The backend used elsewhere | `UnsupportedBackend`, on other platforms |
//! | A scripted backend for tests | [`ScriptedBackend`] |
//!
//! The Netlink implementation is selected by `cfg(target_os = "linux")`, not by
//! a feature, because `rtnetlink` cannot compile elsewhere at all. Every other
//! platform gets `UnsupportedBackend`, whose operations return
//! [`NetError::Unsupported`], so the executor and its tests run everywhere.
//!
//! The library choice and the reason for the platform gate are recorded in
//! `docs/adr/ADR-0003-netlink-library.md`.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

mod backend;
mod error;
mod testing;
mod types;
mod vrrp;

#[cfg(target_os = "linux")]
mod netlink;

#[cfg(target_os = "linux")]
mod socket;

#[cfg(not(target_os = "linux"))]
mod unsupported;

pub use backend::NetworkBackend;
pub use error::{NetError, Result};
pub use testing::ScriptedBackend;
pub use testing::{Call, Outcome};
pub use types::{
    AddrParseError, Interface, InterfaceId, IpCidr, LinkState, NegativeInterfaceIndex,
    PrefixLenError,
};
pub use vrrp::{
    Accepted, DEFAULT_ACCEPT_RATE, Datagram, Destinations, MAX_DATAGRAM, PeerSet, REQUIRED_TTL,
    RateWindow, Rejection, VRRP_IP_PROTOCOL, build, default_interval, fixture_advertisement,
    validate,
};

#[cfg(target_os = "linux")]
pub use netlink::{LinkEvent, NetlinkBackend};
#[cfg(target_os = "linux")]
pub use socket::{Received, VrrpSocket};

#[cfg(not(target_os = "linux"))]
pub use unsupported::UnsupportedBackend;

/// Returns the platform's default backend.
///
/// # Errors
///
/// Returns [`NetError::Io`] when a Netlink socket cannot be opened, which needs
/// `CAP_NET_ADMIN`.
#[cfg(target_os = "linux")]
pub fn default_backend() -> Result<NetlinkBackend> {
    NetlinkBackend::open()
}

/// Returns the platform's default backend.
///
/// # Errors
///
/// Always returns [`NetError::Unsupported`] here: there is no Netlink on this
/// platform, and saying so at the point of use beats failing later.
#[cfg(not(target_os = "linux"))]
pub fn default_backend() -> Result<UnsupportedBackend> {
    Err(NetError::Unsupported {
        operation: "the netlink backend on this platform",
    })
}
