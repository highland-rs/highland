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
