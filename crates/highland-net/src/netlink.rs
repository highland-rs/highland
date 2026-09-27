// Rust guideline compliant 2026-09-27

//! The Linux Netlink implementation of [`NetworkBackend`].
//!
//! Selected by `cfg(target_os = "linux")`. The library choice, and the reason
//! this module is platform-gated rather than feature-gated, are recorded in
//! `docs/adr/ADR-0003-netlink-library.md`.
//!
//! # Confirmation, not acknowledgment
//!
//! Every mutation here is followed by a read-back, because a netlink
//! acknowledgment only means the kernel accepted the request. `add_address`
//! returning `Ok` therefore means the address is present in the kernel, which is
//! what `I-19` requires and what the state machine's ownership handshake depends
//! on.

use std::io;
use std::net::IpAddr;

use rtnetlink::{
    AddressAddReq, AddressDeleteReq, AddressListOptions, Handle, LinkGetOptions,
    LinkSubscribeOptions, NetlinkSocketError,
};
use tokio::net::UnixDatagram as NetlinkSocket;

use crate::backend::NetworkBackend;
use crate::error::{NetError, Result};
use crate::types::{Interface, InterfaceId, IpCidr, LinkState};

/// The address protocol used for addresses this project adds, so a diagnostic
/// can tell where a VIP came from. `RTPROT_BOOT` is 3.
const ADDRESS_PROTOCOL: u8 = 3;

/// A [`NetworkBackend`] over Netlink.
#[derive(Debug)]
pub struct NetlinkBackend {
    handle: Handle<NetlinkSocketError>,
}

impl NetlinkBackend {
    /// Opens a Netlink handle.
    ///
    /// # Errors
    ///
    /// Returns [`NetError::Io`] when a Netlink socket cannot be created, which
    /// needs `CAP_NET_ADMIN` for the operations this backend performs.
    pub fn open() -> Result<Self> {
        let socket = NetlinkSocket::unbound().map_err(|source| NetError::Io {
            operation: "opening a netlink socket",
            source,
        })?;
        Ok(Self {
            handle: Handle::new(socket),
        })
    }

    /// Creates a backend over an existing handle.
    #[must_use]
    pub fn from_handle(handle: Handle<NetlinkSocketError>) -> Self {
        Self { handle }
    }

    /// Returns the underlying handle, for the subscription the daemon drives.
    #[must_use]
    pub fn handle(&self) -> &Handle<NetlinkSocketError> {
        &self.handle
    }

    /// Subscribes to link and address changes.
    ///
    /// # Errors
    ///
    /// Returns [`NetError::Io`] when the subscription cannot be installed.
    pub async fn subscribe(&self) -> Result<()> {
        let mut request = LinkSubscribeOptions::new()
            .set_link_flags(true)
            .set_address_flags(true)
            .set_route_flags(false)
            .set_neigh_flags(false);
        self.handle
            .link()
            .request(&mut request)
            .await
            .map_err(|error| NetError::Io {
                operation: "subscribing to link changes",
                source: io_from(error),
            })
    }

    /// Returns the addresses currently configured on an interface.
    ///
    /// # Errors
    ///
    /// Returns [`NetError::Io`] when Netlink does not answer.
    pub async fn addresses_on(&self, name: &str) -> Result<Vec<IpCidr>> {
        let interface = self.interface(name).await?;
        self.addresses_on_index(interface.id).await
    }

    /// Returns the addresses configured on an interface index.
    ///
    /// # Errors
    ///
    /// Returns [`NetError::Io`] when Netlink does not answer.
    pub async fn addresses_on_index(&self, index: InterfaceId) -> Result<Vec<IpCidr>> {
        let mut request = AddressListOptions::new().set_index(index.get());
        let mut messages = self
            .handle
            .address()
            .request_iter(&mut request)
            .await
            .map_err(|error| NetError::Io {
                operation: "listing addresses",
                source: io_from(error),
            })?;

        let mut addresses = Vec::new();
        while let Some(message) = messages.next().await {
            let message = message.map_err(|error| NetError::Io {
                operation: "listing addresses",
                source: io_from(error),
            })?;
            if let Some((address, prefix)) = address_from_message(&message) {
                addresses.push(IpCidr::new(address, prefix));
            }
        }
        Ok(addresses)
    }

    /// Returns `true` when `address` is present, read back from the kernel.
    ///
    /// # Errors
    ///
    /// Returns [`NetError::Io`] when Netlink does not answer.
    pub async fn address_present(&self, name: &str, address: &IpCidr) -> Result<bool> {
        Ok(self.addresses_on(name).await?.contains(address))
    }

    /// Returns `true` when an interface other than `interface` holds `address`.
    ///
    /// # Errors
    ///
    /// Returns [`NetError::Io`] when Netlink does not answer.
    pub async fn held_by_another(
        &self,
        interface: &Interface,
        address: IpCidr,
    ) -> Result<Option<String>> {
        let mut request = LinkGetOptions::new();
        let mut messages = self
            .handle
            .link()
            .request_iter(&mut request)
            .await
            .map_err(|error| NetError::Io {
                operation: "listing interfaces",
                source: io_from(error),
            })?;

        while let Some(message) = messages.next().await {
            let message = message.map_err(|error| NetError::Io {
                operation: "listing interfaces",
                source: io_from(error),
            })?;
            let Some(index) = message.header.index else {
                continue;
            };
            if index == interface.id.get() {
                continue;
            }
            let Ok(id) = InterfaceId::new(index) else {
                continue;
            };
            for found in self.addresses_on_index(id).await? {
                if found == address {
                    return Ok(Some(
                        message
                            .header
                            .name
                            .unwrap_or_else(|| format!("index {index}")),
                    ));
                }
            }
        }
        Ok(None)
    }
}

impl NetworkBackend for NetlinkBackend {
    async fn interface(&self, name: &str) -> Result<Interface> {
        let mut request = LinkGetOptions::new().set_name_filter(name.to_owned().into());
        let mut messages = self
            .handle
            .link()
            .request_iter(&mut request)
            .await
            .map_err(|error| NetError::Io {
                operation: "looking up an interface",
                source: io_from(error),
            })?;

        let Some(message) = messages.next().await else {
            return Err(NetError::InterfaceNotFound {
                name: name.to_owned(),
            });
        };
        let message = message.map_err(|error| NetError::Io {
            operation: "looking up an interface",
            source: io_from(error),
        })?;
        let Some(index) = message.header.index else {
            return Err(NetError::InterfaceNotFound {
                name: name.to_owned(),
            });
        };
        let id = InterfaceId::new(index).map_err(|error| NetError::Io {
            operation: "reading an interface index",
            source: io::Error::other(error.to_string()),
        })?;

        let up = message.header.flags.contains(rtnetlink::LinkFlags::UP);
        let operational = message.link.as_ref().and_then(|link| link.oper_state);
        let state = if !up {
            LinkState::Down
        } else {
            match operational {
                Some(rtnetlink::OperState::Up) => LinkState::Up,
                Some(rtnetlink::OperState::Unknown) | None => LinkState::Unknown,
                Some(_) => LinkState::NoCarrier,
            }
        };

        let addresses = self.addresses_on_index(id).await?;
        let mtu = message.link.as_ref().and_then(|link| link.mtu);
        let reported = message.header.name.unwrap_or_else(|| name.to_owned());

        Ok(Interface {
            id,
            name: reported,
            state,
            addresses,
            mtu,
        })
    }

    async fn add_address(&self, interface: InterfaceId, address: IpCidr) -> Result<()> {
        let current = self.interface_by_index(interface).await?;
        if current.addresses.contains(&address) {
            return Ok(());
        }
        if let Some(other) = self.held_by_another(&current, address).await? {
            return Err(NetError::AddressAlreadyPresent {
                address,
                existing: other,
            });
        }

        let request = AddressAddReq {
            index: interface.get(),
            address: address.address(),
            prefix_len: address.prefix_len(),
            scope: None,
            index_spec: None,
            flags: 0,
            valid_lft: None,
            preferred_lft: None,
            protocol: Some(ADDRESS_PROTOCOL),
            peer: None,
        };
        self.handle
            .address()
            .add(request)
            .await
            .map_err(|error| NetError::AddAddress {
                interface: current.name.clone(),
                address,
                source: io_from(error),
            })?;

        if !self.addresses_on_index(interface).await?.contains(&address) {
            return Err(NetError::AddAddress {
                interface: current.name.clone(),
                address,
                source: io::Error::other("the address is absent after a successful request"),
            });
        }
        Ok(())
    }

    async fn remove_address(&self, interface: InterfaceId, address: IpCidr) -> Result<()> {
        let current = self.interface_by_index(interface).await?;
        if !current.addresses.contains(&address) {
            return Ok(());
        }
        let request = AddressDeleteReq {
            index: interface.get(),
            address: address.address(),
            prefix_len: address.prefix_len(),
            peer: None,
            flags: 0,
        };
        self.handle
            .address()
            .delete(request)
            .await
            .map_err(|error| NetError::RemoveAddress {
                interface: current.name.clone(),
                address,
                source: io_from(error),
            })?;

        if self.addresses_on_index(interface).await?.contains(&address) {
            return Err(NetError::RemoveAddress {
                interface: current.name.clone(),
                address,
                source: io::Error::other("the address is present after a successful request"),
            });
        }
        Ok(())
    }

    async fn send_gratuitous_update(
        &self,
        _interface: InterfaceId,
        _address: IpAddr,
    ) -> Result<()> {
        // Gratuitous ARP needs an AF_PACKET socket, whose `sockaddr_ll` has no
        // safe representation. It arrives in Milestone 4; the state machine
        // treats the failure as non-fatal, so nothing else changes meanwhile.
        Err(NetError::Unsupported {
            operation: "gratuitous ARP",
        })
    }
}

impl NetlinkBackend {
    /// Looks an interface up by index, which is how the executor addresses it.
    async fn interface_by_index(&self, index: InterfaceId) -> Result<Interface> {
        let mut request = LinkGetOptions::new().set_index(index.get());
        let mut messages = self
            .handle
            .link()
            .request_iter(&mut request)
            .await
            .map_err(|error| NetError::Io {
                operation: "looking up an interface",
                source: io_from(error),
            })?;

        let Some(message) = messages.next().await else {
            return Err(NetError::Io {
                operation: "looking up an interface",
                source: io::Error::other("no interface with that index"),
            });
        };
        let message = message.map_err(|error| NetError::Io {
            operation: "looking up an interface",
            source: io_from(error),
        })?;
        let name = message
            .header
            .name
            .unwrap_or_else(|| format!("index {}", index.get()));
        let up = message.header.flags.contains(rtnetlink::LinkFlags::UP);
        let operational = message.link.as_ref().and_then(|link| link.oper_state);
        let state = if !up {
            LinkState::Down
        } else {
            match operational {
                Some(rtnetlink::OperState::Up) => LinkState::Up,
                Some(rtnetlink::OperState::Unknown) | None => LinkState::Unknown,
                Some(_) => LinkState::NoCarrier,
            }
        };
        let addresses = self.addresses_on_index(index).await?;
        let mtu = message.link.as_ref().and_then(|link| link.mtu);
        Ok(Interface {
            id: index,
            name,
            state,
            addresses,
            mtu,
        })
    }
}

/// Extracts the address and prefix length a Netlink address message carries.
fn address_from_message(
    message: &rtnetlink::netlink_packet_route::address::AddressMessage,
) -> Option<(IpAddr, u8)> {
    let prefix_len = message.address.prefix_len;
    if let Some(local) = message.address.local
        && !local.is_unspecified()
    {
        return Some((*local, prefix_len));
    }
    message.address.address.map(|address| (address, prefix_len))
}

/// Converts a Netlink failure into an operating-system error.
///
/// Netlink reports failures as a negative `errno`, so the value is exactly the
/// `io::Error` a syscall would have produced. Keeping a real `io::Error` in the
/// `NetError` variants is what lets an operator read a permission failure as a
/// permission failure (`R-23`).
fn io_from(error: NetlinkSocketError) -> io::Error {
    match error {
        NetlinkSocketError::SocketError(source) => source,
        NetlinkSocketError::NetlinkError(source) => io::Error::other(source.to_string()),
        NetlinkSocketError::InvalidRouteMessage => {
            io::Error::other("the kernel returned an unparseable netlink message")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_netlink_failure_keeps_its_cause() {
        let error = io_from(NetlinkSocketError::InvalidRouteMessage);
        assert!(error.to_string().contains("unparseable"));
    }

    #[test]
    fn the_address_protocol_is_the_boot_protocol() {
        assert_eq!(ADDRESS_PROTOCOL, 3);
    }
}
