// Rust guideline compliant 2026-09-27

//! Network error types.
//!
//! Errors preserve the interface, the address, and the operating-system cause
//! so that operator-facing messages can name all three (SPEC.md, `R-23`).

use std::io;

use thiserror::Error;

use crate::types::{Interface, InterfaceId, IpCidr};

/// Result alias for fallible network operations.
pub type Result<T, E = NetError> = std::result::Result<T, E>;

/// An error raised by the network layer.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum NetError {
    /// The named interface does not exist.
    #[error("interface {name} does not exist")]
    InterfaceNotFound {
        /// The requested interface name.
        name: String,
    },

    /// The interface exists but is not in a usable state.
    #[error("interface {interface} is not usable: {reason}")]
    InterfaceNotUsable {
        /// The interface name.
        interface: String,
        /// Why it is unusable.
        reason: &'static str,
    },

    /// Adding an address failed.
    #[error("could not add address {address} to interface {interface}: {source}")]
    AddAddress {
        /// The interface name.
        interface: String,
        /// The address that could not be added.
        address: IpCidr,
        /// The operating-system error.
        source: io::Error,
    },

    /// Removing an address failed.
    #[error("could not remove address {address} from interface {interface}: {source}")]
    RemoveAddress {
        /// The interface name.
        interface: String,
        /// The address that could not be removed.
        address: IpCidr,
        /// The operating-system error.
        source: io::Error,
    },

    /// The address is already configured on a different local interface.
    #[error("address {address} is already present on interface {existing}")]
    AddressAlreadyPresent {
        /// The conflicting address.
        address: IpCidr,
        /// The interface that already holds it.
        existing: String,
    },

    /// Sending a gratuitous update failed.
    #[error("could not send a gratuitous update for {address} on {interface}: {source}")]
    SendGratuitousUpdate {
        /// The interface name.
        interface: String,
        /// The address being announced.
        address: String,
        /// The operating-system error.
        source: io::Error,
    },

    /// A socket operation failed.
    #[error("socket operation on interface {interface} failed: {source}")]
    Socket {
        /// The interface the socket was bound to.
        interface: InterfaceId,
        /// The operating-system error.
        source: io::Error,
    },

    /// The backend does not implement the requested operation.
    #[error("the network backend does not support {operation}")]
    Unsupported {
        /// The unsupported operation.
        operation: &'static str,
    },

    /// An operation failed for a reason that is not specific to addresses or
    /// sockets, such as a subscription failing to install.
    #[error("could not {operation}: {source}")]
    Io {
        /// What was being attempted, in the infinitive.
        operation: &'static str,
        /// The operating-system error.
        source: std::io::Error,
    },

    /// A datagram arrived larger than the receive buffer and was cut short.
    ///
    /// The delivered prefix is not a valid VRRP advertisement: the checksum in
    /// the sender's datagram covers the bytes it wrote and the kernel delivered
    /// fewer, so validating the prefix checksums it against the wrong bytes. The
    /// datagram is refused instead. This is the read side of `L-10`, the bound on
    /// the advertisement the daemon accepts.
    ///
    /// The field is `from` rather than `source` because `thiserror` reserves
    /// `source` for a cause that implements `std::error::Error`, and this is a
    /// peer address.
    #[error("datagram from {from} was truncated by the receive buffer")]
    TruncatedDatagram {
        /// The address the datagram appeared to come from.
        from: std::net::IpAddr,
    },

    /// An advertisement could not be encoded for the wire.
    #[error("could not encode an {family} advertisement: {reason}")]
    Encode {
        /// The family the advertisement was for.
        family: highland_vrrp::IpFamily,
        /// Why encoding failed.
        reason: String,
    },
}

impl NetError {
    /// Builds an [`NetError::AddressAlreadyPresent`] from an interface.
    #[must_use]
    pub fn address_present(address: IpCidr, existing: &Interface) -> Self {
        NetError::AddressAlreadyPresent {
            address,
            existing: existing.name.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_add_failure_names_the_interface_and_the_address() {
        let error = NetError::AddAddress {
            interface: "eth0".to_owned(),
            address: IpCidr::new("192.0.2.10".parse().expect("valid address"), 24),
            source: io::Error::from(io::ErrorKind::PermissionDenied),
        };

        assert_eq!(
            error.to_string(),
            "could not add address 192.0.2.10/24 to interface eth0: permission denied"
        );
    }

    #[test]
    fn a_conflicting_address_reports_the_owning_interface() {
        let existing = Interface {
            id: InterfaceId::new(4).expect("positive index"),
            name: "eth1".to_owned(),
            state: crate::types::LinkState::Up,
            addresses: Vec::new(),
            mtu: None,
        };
        let error = NetError::address_present(
            IpCidr::new("192.0.2.10".parse().expect("valid address"), 24),
            &existing,
        );

        assert_eq!(
            error.to_string(),
            "address 192.0.2.10/24 is already present on interface eth1"
        );
    }
}
