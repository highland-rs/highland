// Rust guideline compliant 2026-09-28

//! Enumerating the addresses configured on this node.
//!
//! Configuration validation needs to know which addresses are local so it can
//! reject an instance from peering with itself (`V-08`), and it needs that answer
//! *before* the daemon binds any socket: a node that advertises to its own
//! address will receive its own advertisements, and the resulting behaviour
//! depends on the state machine rather than on configuration.
//!
//! That ordering is the whole difficulty. The document being validated says
//! nothing about the host it will run on, so the answer has to come from the
//! kernel, and it has to be available synchronously -- validation happens on the
//! startup path, before the runtime exists.
//!
//! [`getifaddrs`] is the right source for exactly that reason. Netlink would be
//! the more idiomatic choice for this crate, and it is used everywhere else, but
//! it is async and Linux-only, and requiring a runtime merely to read four
//! addresses would put a tokio dependency in the middle of startup for no gain.
//! `getifaddrs` is a single libc call, it is what the gratuituous-ARP path
//! already uses for the hardware address, and it is synchronous.

use std::io;
use std::net::IpAddr;

/// Returns every IP address configured on this host.
///
/// The list is the kernel's, not the document's: loopback, every interface, and
/// both families. It is deliberately not filtered by interface, because
/// validation does not yet know which interface an instance will bind, and a peer
/// list naming an address on any local interface is self-peering.
///
/// # Errors
///
/// Returns an error if the kernel's interface list cannot be read. Callers
/// should treat that as "the local addresses are unknown" rather than as
/// "there are none": the two are different, and only the first is safe to
/// proceed on.
#[cfg(unix)]
pub fn local_addresses() -> io::Result<Vec<IpAddr>> {
    use nix::ifaddrs::getifaddrs;

    let mut addresses = Vec::new();
    for entry in getifaddrs()? {
        // An interface has one entry per configured address and one `AF_PACKET`
        // entry for the hardware address; only the inet families name an
        // address, and only they are meaningful here.
        let Some(address) = entry.address else {
            continue;
        };
        if let Some(inet) = address.as_sockaddr_in() {
            addresses.push(IpAddr::V4(inet.ip()));
        } else if let Some(inet) = address.as_sockaddr_in6() {
            addresses.push(IpAddr::V6(inet.ip()));
        }
    }
    Ok(addresses)
}

/// Returns every IP address configured on this host.
///
/// # Errors
///
/// Always fails here. There is no supported way to enumerate a host's addresses
/// on this platform, and the Unix implementation is the only one that can
/// answer the question `V-08` asks. Returning an error rather than an empty list
/// keeps "unknown" distinguishable from "none configured", which is the
/// distinction the caller needs in order to decide whether to enforce the check.
#[cfg(not(unix))]
pub fn local_addresses() -> io::Result<Vec<IpAddr>> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "enumerating local addresses is not implemented on this platform",
    ))
}

#[cfg(test)]
mod tests {
    use super::local_addresses;

    /// Loopback is configured on every platform Highland builds on, so it is the
    /// one address whose presence can be asserted without touching the network.
    #[test]
    fn loopback_is_local() {
        let Ok(addresses) = local_addresses() else {
            // A platform that cannot answer must say so rather than claim the
            // host has no addresses.
            return;
        };
        let loopback: std::net::IpAddr = "127.0.0.1".parse().expect("valid address");
        assert!(
            addresses.contains(&loopback),
            "127.0.0.1 is configured on every host, so it must be reported local"
        );
    }

    /// The check is only useful if the answer changes with the host's
    /// configuration, and the cheapest witness is that the list is not empty.
    #[test]
    fn the_list_is_not_empty_on_unix() {
        if !cfg!(unix) {
            return;
        }
        let addresses = local_addresses().expect("Linux can enumerate its addresses");
        assert!(
            !addresses.is_empty(),
            "a Unix host has at least loopback configured"
        );
    }
}
