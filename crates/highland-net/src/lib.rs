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
//! | Sending and receiving for one instance | `SocketTransport` |
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
// Two `unsafe` islands are audited and documented rather than forbidden: the
// `sockaddr_ll` an `AF_PACKET` send needs, and the hardware address `getifaddrs`
// hides in its netmask field. `socket2` and `nix` have no safe way to express
// either, and the alternative considered was a datalink crate that is no longer
// maintained. Both islands are confined to `gratuitous`, both are documented at
// the function, and the reasoning is recorded in
// `docs/adr/ADR-0004-gratuitous-arp.md`.
#![deny(unsafe_code)]

mod backend;
mod error;
mod local;

#[cfg(all(target_os = "linux", feature = "netlink-tests"))]
pub mod frames;
pub mod gratuitous;
mod testing;
mod types;
mod vrrp;

#[cfg(target_os = "linux")]
mod netlink;

#[cfg(target_os = "linux")]
mod socket;

#[cfg(target_os = "linux")]
mod transport;

#[cfg(not(target_os = "linux"))]
mod unsupported;

pub use backend::NetworkBackend;
pub use error::{NetError, Result};
pub use local::local_addresses;
pub use testing::ScriptedBackend;
pub use testing::{Call, Outcome};
pub use types::{
    AddrParseError, Interface, InterfaceId, IpCidr, LinkState, NegativeInterfaceIndex,
    PrefixLenError,
};
pub use vrrp::{
    Accepted, AllowedSources, DEFAULT_ACCEPT_RATE, Datagram, Destinations, FIXTURE_DESTINATION,
    FIXTURE_SOURCE, MAX_DATAGRAM, PeerSet, Peering, REQUIRED_TTL, RateWindow, ReceptionPolicy,
    Rejection, VRRP_IP_PROTOCOL, build, default_interval, fixture_advertisement,
    fixture_advertisement_between, validate,
};

#[cfg(target_os = "linux")]
pub use netlink::{LinkEvent, NetlinkBackend};
#[cfg(target_os = "linux")]
pub use socket::{Received, VrrpSocket};

#[cfg(target_os = "linux")]
pub use transport::SocketTransport;

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

/// A transport that refuses to do anything, for a platform with no socket.
///
/// It exists so the daemon's types do not change shape on a non-Linux host, and
/// so a test can prove that nothing silently degrades to "no VRRP" on a
/// platform that should have it.
#[derive(Debug, Clone, Copy, Default)]
pub struct UnavailableTransport;

impl UnavailableTransport {
    /// The answer this transport always gives.
    ///
    /// # Errors
    ///
    /// Always. It is a named error rather than a panic, because a daemon that
    /// cannot speak VRRP should say so and stop.
    pub fn unavailable() -> NetError {
        NetError::Unsupported {
            operation: "the VRRP socket on this platform",
        }
    }
}
