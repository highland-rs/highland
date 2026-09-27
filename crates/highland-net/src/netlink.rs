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
//!
//! # Link state
//!
//! Link state comes from `IFLA_OPERSTATE` rather than from `IFF_UP`. The
//! operational state is the one that answers the question VRRP actually cares
//! about: an interface can be administratively up with no carrier, and taking
//! ownership of an address on a link that cannot carry a packet is exactly the
//! failure `I-15` exists to prevent.

use std::io;
use std::net::IpAddr;

use futures_util::StreamExt as _;

use rtnetlink::packet_core::NetlinkPayload;
use rtnetlink::packet_route::RouteNetlinkMessage;
use rtnetlink::packet_route::address::AddressAttribute;
use rtnetlink::packet_route::address::AddressHeaderFlags;
use rtnetlink::packet_route::link::{LinkAttribute, LinkMessage};
use rtnetlink::{Error as RtnetlinkError, Handle};

use crate::backend::NetworkBackend;
use crate::error::{NetError, Result};
use crate::types::{Interface, InterfaceId, IpCidr, LinkState};

/// Something the kernel told us changed.
///
/// The daemon does not need the whole message: it needs to know that a link or
/// an address moved, and then it re-reads the state it cares about. That keeps
/// this crate's types out of the daemon's, and keeps a subscription from becoming
/// a second, divergent source of truth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LinkEvent {
    /// A link attribute changed.
    Link,
    /// An address changed.
    Address,
}

/// A [`NetworkBackend`] over Netlink.
#[derive(Debug, Clone)]
pub struct NetlinkBackend {
    handle: Handle,
}

impl NetlinkBackend {
    /// Opens a Netlink handle.
    ///
    /// # Errors
    ///
    /// Returns [`NetError::Io`] when a Netlink socket cannot be created, which
    /// needs `CAP_NET_ADMIN` for the operations this backend performs.
    pub fn open() -> Result<Self> {
        let (connection, handle, _messages) =
            rtnetlink::new_connection().map_err(|source| NetError::Io {
                operation: "opening a netlink socket",
                source,
            })?;

        // The connection is the future that drives the socket, and the handle is
        // only a channel to it. Dropping the connection without a task polling it
        // leaves a socket that accepts every request and answers none of them, so
        // every lookup fails with "not acknowledged". This is not visible from
        // the types; only a real kernel shows it.
        tokio::spawn(connection);

        Ok(Self { handle })
    }

    /// Creates a backend over an existing handle.
    #[must_use]
    pub fn from_handle(handle: Handle) -> Self {
        Self { handle }
    }

    /// Returns the underlying handle, for a caller that needs a request this
    /// backend does not wrap.
    #[must_use]
    pub fn handle(&self) -> &Handle {
        &self.handle
    }

    /// Returns the addresses configured on an interface index.
    ///
    /// # Errors
    ///
    /// Returns [`NetError::Io`] when Netlink does not answer.
    pub async fn addresses_on_index(&self, index: InterfaceId) -> Result<Vec<IpCidr>> {
        let request = self
            .handle
            .address()
            .get()
            .set_link_index_filter(u32::try_from(index.get()).unwrap_or(0));
        let mut stream = request.execute();

        let mut addresses = Vec::new();
        while let Some(message) = stream.next().await {
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

    /// Returns the addresses configured on an interface, by name.
    ///
    /// # Errors
    ///
    /// Returns [`NetError::InterfaceNotFound`] when no such interface exists.
    pub async fn addresses_on(&self, name: &str) -> Result<Vec<IpCidr>> {
        let interface = self.interface(name).await?;
        self.addresses_on_index(interface.id).await
    }

    /// Returns `true` when `address` is present, read back from the kernel.
    ///
    /// # Errors
    ///
    /// Returns [`NetError::Io`] when Netlink does not answer.
    pub async fn address_present(&self, name: &str, address: &IpCidr) -> Result<bool> {
        Ok(self.addresses_on(name).await?.contains(address))
    }

    /// Looks an interface up by index.
    ///
    /// # Errors
    ///
    /// Returns [`NetError::Io`] when Netlink does not answer.
    pub async fn interface_by_index(&self, index: InterfaceId) -> Result<Interface> {
        let request = self
            .handle
            .link()
            .get()
            .match_index(u32::try_from(index.get()).unwrap_or(0));
        let mut stream = request.execute();

        let Some(message) = stream.next().await else {
            return Err(NetError::Io {
                operation: "looking up an interface",
                source: io::Error::other("no interface with that index"),
            });
        };
        let message = message.map_err(|error| NetError::Io {
            operation: "looking up an interface",
            source: io_from(error),
        })?;
        let name = link_name(&message).unwrap_or_else(|| format!("index {}", index.get()));
        let state = link_state(&message);
        let mtu = link_mtu(&message);
        let addresses = self.addresses_on_index(index).await?;
        Ok(Interface {
            id: index,
            name,
            state,
            addresses,
            mtu,
        })
    }

    /// Subscribes to link and address changes.
    ///
    /// The subscription is what lets the daemon notice that an interface went
    /// away, which is how a node learns it is no longer able to hold ownership
    /// (`I-15`). It is installed on its own connection because a Netlink socket
    /// is either a request socket or a multicast group socket, not both.
    ///
    /// # Errors
    ///
    /// Returns [`NetError::Io`] when the group socket cannot be opened.
    pub fn subscribe_link_and_address(
        &self,
    ) -> impl std::future::Future<Output = Result<tokio::sync::mpsc::UnboundedReceiver<LinkEvent>>> + Send
    {
        let answer = self.answer_subscribe_link_and_address();
        std::future::ready(answer)
    }

    #[allow(
        clippy::unused_self,
        reason = "the socket belongs to the connection, not the handle"
    )]
    fn answer_subscribe_link_and_address(
        &self,
    ) -> Result<tokio::sync::mpsc::UnboundedReceiver<LinkEvent>> {
        use rtnetlink::MulticastGroup;

        let (connection, _handle, mut messages) = rtnetlink::new_multicast_connection(&[
            MulticastGroup::Link,
            MulticastGroup::Ipv4Ifaddr,
            MulticastGroup::Ipv6Ifaddr,
        ])
        .map_err(|source| NetError::Io {
            operation: "subscribing to link changes",
            source,
        })?;

        // The connection is the future that polls the socket. Without a task
        // driving it, the subscription installs successfully and then never
        // delivers anything, which is the worst shape of bug to have here.
        tokio::spawn(connection);

        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Some((message, _address)) = messages.next().await {
                // `NetlinkPayload` is either the parsed message, an ACK, a
                // parse error, or the end of a multi-part reply. Only the
                // message is interesting here.
                let NetlinkPayload::InnerMessage(payload) = message.payload else {
                    continue;
                };
                let event = match payload {
                    RouteNetlinkMessage::NewLink(_)
                    | RouteNetlinkMessage::DelLink(_)
                    | RouteNetlinkMessage::SetLink(_) => LinkEvent::Link,
                    RouteNetlinkMessage::NewAddress(_) | RouteNetlinkMessage::DelAddress(_) => {
                        LinkEvent::Address
                    }
                    _ => continue,
                };
                if sender.send(event).is_err() {
                    return;
                }
            }
        });
        Ok(receiver)
    }

    /// Creates a dummy interface with `name`.
    ///
    /// A dummy device is a real netdevice that accepts addresses but carries no
    /// traffic, which makes it the right fixture for a test or a namespace
    /// harness: it gives an instance a real interface to bind to without
    /// needing a peer, a cable, or a second namespace.
    ///
    /// This exists for the test harness and the namespace suite, not for the
    /// daemon, which never creates interfaces. It is therefore only compiled
    /// with `netlink-tests`.
    ///
    /// # Errors
    ///
    /// Returns [`NetError::Io`] when the kernel refuses, typically because the
    /// name is taken or `CAP_NET_ADMIN` is missing.
    #[cfg(feature = "netlink-tests")]
    pub async fn create_dummy(&self, name: &str) -> Result<()> {
        use rtnetlink::LinkDummy;

        self.handle
            .link()
            .add(LinkDummy::new(name).build())
            .execute()
            .await
            .map_err(|error| NetError::Io {
                operation: "creating a dummy interface",
                source: io_from(error),
            })?;
        Ok(())
    }

    /// Removes an interface by name.
    ///
    /// The counterpart of [`NetlinkBackend::create_dummy`], and the same
    /// caveat applies: this is harness support, not daemon behavior.
    ///
    /// # Errors
    ///
    /// Returns [`NetError::Io`] when the kernel refuses.
    #[cfg(feature = "netlink-tests")]
    pub async fn remove_dummy(&self, name: &str) -> Result<()> {
        let Ok(interface) = self.interface(name).await else {
            return Ok(());
        };
        self.handle
            .link()
            .del(u32::try_from(interface.id.get()).unwrap_or(0))
            .execute()
            .await
            .map_err(|error| NetError::Io {
                operation: "removing a dummy interface",
                source: io_from(error),
            })?;
        Ok(())
    }

    /// Returns the name of an interface other than `interface` already holding
    /// `address`, if any.
    ///
    /// # Errors
    ///
    /// Returns [`NetError::Io`] when Netlink does not answer.
    pub async fn held_by_another(
        &self,
        interface: &Interface,
        address: IpCidr,
    ) -> Result<Option<String>> {
        let mut stream = self.handle.link().get().execute();

        while let Some(message) = stream.next().await {
            let message = message.map_err(|error| NetError::Io {
                operation: "listing interfaces",
                source: io_from(error),
            })?;
            if i32::try_from(message.header.index).unwrap_or(-1) == interface.id.get() {
                continue;
            }
            let Ok(id) = InterfaceId::new(i32::try_from(message.header.index).unwrap_or(-1)) else {
                continue;
            };
            if self.addresses_on_index(id).await?.contains(&address) {
                return Ok(Some(
                    link_name(&message).unwrap_or_else(|| format!("index {}", id.get())),
                ));
            }
        }
        Ok(None)
    }
}

impl NetworkBackend for NetlinkBackend {
    async fn interface(&self, name: &str) -> Result<Interface> {
        let request = self.handle.link().get().match_name(name.to_owned());
        let mut stream = request.execute();

        // A name filter that matches nothing comes back as an errno, not as an
        // empty dump: Linux answers `ERANGE` for a dump it filtered to zero
        // rows. "No such interface" is the answer this call is asking for, so
        // whatever the kernel said, an absent interface is `InterfaceNotFound`
        // rather than a transport failure.
        let Some(Ok(message)) = stream.next().await else {
            return Err(NetError::InterfaceNotFound {
                name: name.to_owned(),
            });
        };
        let id = InterfaceId::new(i32::try_from(message.header.index).unwrap_or(-1)).map_err(
            |error| NetError::Io {
                operation: "reading an interface index",
                source: io::Error::other(error.to_string()),
            },
        )?;

        let addresses = self.addresses_on_index(id).await?;
        Ok(Interface {
            id,
            name: link_name(&message).unwrap_or_else(|| name.to_owned()),
            state: link_state(&message),
            addresses,
            mtu: link_mtu(&message),
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

        let index = u32::try_from(interface.get()).unwrap_or(0);
        // `IFA_F_NODAD` for IPv6, and the reason is specific to what an address
        // here is: a tentative address cannot be bound to and cannot be used as
        // a source, so for about a second after it is added the node owns an
        // address it cannot send from and cannot announce. The address has just
        // been claimed by an election, so there is nothing to detect either — the
        // question "is anyone else using this" was answered by the protocol, and
        // answering it again with a solicitation delays the handover it is part
        // of.
        // The kernel's reply is the acknowledgment. The read-back below is the
        // confirmation, and it is the one that matters.
        let mut request = self
            .handle
            .address()
            .add(index, address.address(), address.prefix_len());
        if matches!(address.address(), IpAddr::V6(_)) {
            // `IFA_F_NODAD`, and the reason is specific to what an address here
            // is: a tentative address cannot be bound to and cannot be used as a
            // source, so for about a second after it is added the node owns an
            // address it cannot send from and cannot announce. The address has
            // just been claimed by an election, so there is nothing left to
            // detect — the question "is anyone else using this" was answered by
            // the protocol — and answering it again with a solicitation delays
            // the handover it is part of.
            request.message_mut().header.flags |= AddressHeaderFlags::Nodad;
        }
        request
            .execute()
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

        let index = u32::try_from(interface.get()).unwrap_or(0);
        // The builder is generic over the address family, so each family gets
        // its own construction rather than a shared one.
        let message = match address.address() {
            IpAddr::V4(v4) => rtnetlink::AddressMessageBuilder::<std::net::Ipv4Addr>::new()
                .address(v4, address.prefix_len())
                .index(index)
                .build(),
            IpAddr::V6(v6) => rtnetlink::AddressMessageBuilder::<std::net::Ipv6Addr>::new()
                .address(v6, address.prefix_len())
                .index(index)
                .build(),
        };

        self.handle
            .address()
            .del(message)
            .execute()
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

    /// Announces an address that was just claimed.
    ///
    /// IPv4 gets a gratuitous ARP and IPv6 an unsolicited Neighbor Advertisement
    /// (§14.1 steps 5 and 6). Both are sent rather than left to the kernel: the
    /// kernel announces an address when it is added, but only for the interface
    /// it was added to, and a takeover needs the announcement to be deliberate
    /// and logged.
    ///
    /// A failure is returned rather than swallowed, and the caller treats it as
    /// non-fatal, because a node that owns an address and cannot announce it is
    /// still better than a node that gives the address up (§11.4).
    fn send_gratuitous_update(
        &self,
        interface: InterfaceId,
        address: IpAddr,
    ) -> impl std::future::Future<Output = Result<()>> + Send {
        // The interface lookup goes to the kernel, so this stays asynchronous
        // rather than becoming a blocking call in the caller's task.
        self.answer_gratuitous_update(interface, address)
    }
}

impl NetlinkBackend {
    async fn answer_gratuitous_update(
        &self,
        interface: InterfaceId,
        address: IpAddr,
    ) -> Result<()> {
        let current = self.interface_by_index(interface).await?;
        let send = match address {
            IpAddr::V4(address) => {
                let hardware =
                    crate::gratuitous::hardware_address(&current.name).map_err(|source| {
                        NetError::SendGratuitousUpdate {
                            interface: current.name.clone(),
                            address: IpAddr::V4(address).to_string(),
                            source,
                        }
                    })?;
                crate::gratuitous::send_gratuitous_arp(interface, hardware, address)
            }
            IpAddr::V6(address) => {
                crate::gratuitous::send_neighbour_advertisement(interface, &current.name, address)
            }
        };
        send.map_err(|source| NetError::SendGratuitousUpdate {
            interface: current.name,
            address: address.to_string(),
            source,
        })
    }
}

/// Extracts the address and prefix length a Netlink address message carries.
///
/// The local address is preferred over the peer address, because an address
/// added with a peer has both, and only the local one is the interface's.
fn address_from_message(
    message: &rtnetlink::packet_route::address::AddressMessage,
) -> Option<(IpAddr, u8)> {
    let prefix_len = message.header.prefix_len;
    let local = message
        .attributes
        .iter()
        .find_map(|attribute| match attribute {
            AddressAttribute::Local(address) => Some(*address),
            _ => None,
        });
    local
        .or_else(|| {
            message
                .attributes
                .iter()
                .find_map(|attribute| match attribute {
                    AddressAttribute::Address(address) => Some(*address),
                    _ => None,
                })
        })
        .map(|address| (address, prefix_len))
}

/// Reads the interface name out of a link message.
fn link_name(message: &LinkMessage) -> Option<String> {
    message
        .attributes
        .iter()
        .find_map(|attribute| match attribute {
            LinkAttribute::IfName(name) => Some(name.clone()),
            _ => None,
        })
}

/// Reads the MTU out of a link message.
fn link_mtu(message: &LinkMessage) -> Option<u32> {
    message
        .attributes
        .iter()
        .find_map(|attribute| match attribute {
            LinkAttribute::Mtu(mtu) => Some(*mtu),
            _ => None,
        })
}

/// Maps `IFLA_OPERSTATE` onto the states VRRP distinguishes.
fn link_state(message: &LinkMessage) -> LinkState {
    use rtnetlink::packet_route::link::State as OperState;

    message
        .attributes
        .iter()
        .find_map(|attribute| match attribute {
            LinkAttribute::OperState(state) => Some(match state {
                OperState::Up => LinkState::Up,
                OperState::Down | OperState::LowerLayerDown | OperState::Dormant => {
                    LinkState::NoCarrier
                }
                OperState::NotPresent => LinkState::Down,
                // Unknown, testing, and anything a future kernel adds: not
                // usable, and never silently treated as up.
                _ => LinkState::Unknown,
            }),
            _ => None,
        })
        .unwrap_or(LinkState::Unknown)
}

/// Converts a Netlink failure into an operating-system error.
///
/// Netlink reports failures as a negative `errno`, which is exactly the
/// `io::Error` a syscall would have produced. Keeping a real `io::Error` in the
/// `NetError` variants is what lets an operator read a permission failure as a
/// permission failure (`R-23`).
fn io_from(error: RtnetlinkError) -> io::Error {
    match error {
        RtnetlinkError::NetlinkError(message) => {
            // A netlink NACK carries the kernel's `errno`, which is what an
            // operator needs: EADDRNOTAVAIL, EPERM, EEXADDR.
            match message.code.map(i32::from) {
                Some(code) if code < 0 => io::Error::from_raw_os_error(-code),
                Some(code) => io::Error::other(format!("netlink error {code}")),
                None => io::Error::other("the kernel reported an error with no code"),
            }
        }
        RtnetlinkError::RequestFailed => {
            io::Error::other("the netlink request was not acknowledged")
        }
        other => io::Error::other(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_netlink_error_keeps_its_cause() {
        let error = io_from(RtnetlinkError::RequestFailed);
        assert!(error.to_string().contains("not acknowledged"));
    }

    #[test]
    fn a_link_without_attributes_is_unknown_rather_than_up() {
        // The safe default matters: an unparseable link must not look usable.
        let message = LinkMessage::default();
        assert_eq!(link_state(&message), LinkState::Unknown);
        assert_eq!(link_name(&message), None);
        assert_eq!(link_mtu(&message), None);
    }
}
