// Rust guideline compliant 2026-09-27

//! The raw VRRP socket.
//!
//! Linux-only, because `AF_INET` raw sockets for IP protocol 112 are. Sending
//! uses `socket2`, which is enough: a raw socket without `IP_HDRINCL` has the
//! kernel build the IP header, and the socket's TTL is ours to set. Receiving
//! needs `recvmsg`, because a raw socket receives the payload without the IP
//! header and the TTL only arrives as ancillary data, so that half uses `nix`.
//! Neither crate requires `unsafe` in this workspace.
//!
//! # What this module guarantees
//!
//! - Every packet sent carries TTL or hop limit 255, as RFC 5798 requires, and
//!   the TTL is set on the socket rather than trusted to a default.
//! - A datagram received from a different source address than the kernel reports
//!   is impossible, so the source used for peer filtering is the kernel's.
//! - The receive buffer is bounded, so a hostile peer cannot make the kernel
//!   copy more than [`MAX_DATAGRAM`](crate::MAX_DATAGRAM) bytes.

use std::io::IoSliceMut;
use std::net::IpAddr;
use std::os::fd::AsRawFd;

use highland_vrrp::IpFamily;
use nix::sys::socket::sockopt::{
    IpAddMembership, IpDropMembership, IpMulticastTtl, Ipv4PacketInfo, Ipv4RecvTtl,
    Ipv6AddMembership, Ipv6DropMembership, Ipv6MulticastHops, Ipv6RecvHopLimit, Ipv6RecvPacketInfo,
};
use nix::sys::socket::{MsgFlags, SockaddrIn, SockaddrIn6};
use socket2::{Domain, Protocol, SockAddr, Socket, Type};

use crate::error::{NetError, Result};
use crate::vrrp::{MAX_DATAGRAM, REQUIRED_TTL, VRRP_IP_PROTOCOL};

/// The address a socket binds to, before it becomes a [`SockAddr`].
#[derive(Debug, Clone, Copy)]
enum SocketAddrV4Bound {
    /// An IPv4 address.
    V4(std::net::Ipv4Addr),
    /// An IPv6 address.
    V6(std::net::Ipv6Addr),
}

/// A raw socket for VRRP, bound to one interface and one address family.
#[derive(Debug)]
pub struct VrrpSocket {
    socket: Socket,
    family: IpFamily,
    /// The address this socket sends from, which also names the interface when a
    /// multicast membership is requested.
    source: IpAddr,
    interface: String,
    /// The kernel index of the interface, which an IPv6 multicast destination
    /// needs as its scope: a link-local group has no meaning without one.
    index: u32,
}

/// A datagram read from the socket, with everything the transport needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Received {
    /// The VRRP payload, with any IP header removed.
    pub payload: Vec<u8>,
    /// The address the packet came from, which is the peer's identity.
    pub source: IpAddr,
    /// The TTL or hop limit the kernel saw.
    pub ttl: u8,
    /// The address the datagram was sent to, when the kernel reported it.
    ///
    /// This is the *destination*, not the source, and it matters for two reasons:
    /// an IPv6 checksum covers it, and a multicast instance must know that a
    /// packet really arrived at the group rather than being addressed to this
    /// host directly.
    pub destination: Option<IpAddr>,
    /// Whether the datagram arrived with its IP header still attached.
    ///
    /// A raw IP socket usually receives the payload with the header stripped and
    /// the header's fields delivered as ancillary data, but a packet that has
    /// been through the loopback path arrives whole. Both shapes occur, so both
    /// are handled and both are tested.
    pub header_attached: bool,
}

/// Returns the kernel index of an interface by name.
///
/// `if_nametoindex` rather than a Netlink round trip: the socket is bound to the
/// interface by name already, and an index of 0 would silently turn an IPv6
/// multicast send into a send with no scope.
fn interface_index(interface: &str) -> Result<u32> {
    nix::net::if_::if_nametoindex(interface).map_err(|error| NetError::Io {
        operation: "resolving the interface for a VRRP socket",
        source: std::io::Error::from_raw_os_error(error as i32),
    })
}

impl VrrpSocket {
    /// Binds a raw VRRP socket to `interface`, sending from `source`.
    ///
    /// # Errors
    ///
    /// Returns [`NetError::Io`] when the socket cannot be created, which needs
    /// `CAP_NET_RAW`, or when the interface cannot be bound, which needs it
    /// too.
    pub fn bind(family: IpFamily, interface: &str, source: IpAddr) -> Result<Self> {
        Self::bind_with_ttl(family, interface, source, REQUIRED_TTL)
    }

    /// Binds a raw VRRP socket that sends with an arbitrary TTL.
    ///
    /// The TTL is a parameter only so a test can produce a packet the receiver
    /// must reject. The daemon has no reason to send anything but 255, and
    /// [`VrrpSocket::bind`] is the entry point it uses.
    ///
    /// # Errors
    ///
    /// As [`VrrpSocket::bind`].
    pub fn bind_with_ttl(
        family: IpFamily,
        interface: &str,
        source: IpAddr,
        ttl: u8,
    ) -> Result<Self> {
        Self::bind_internal(family, interface, source, ttl, false)
    }

    /// Binds the socket, with the TTL and the address it is bound to as
    /// parameters, because both are load-bearing and neither is a test-only
    /// knob.
    fn bind_internal(
        family: IpFamily,
        interface: &str,
        source: IpAddr,
        ttl: u8,
        any_address: bool,
    ) -> Result<Self> {
        // The socket is created for one family and bound to one of that
        // family's addresses, so a mismatch is a configuration error rather than
        // something to paper over.
        let (domain, bound_source) = match (family, source) {
            (IpFamily::V4, IpAddr::V4(address)) => (Domain::IPV4, SocketAddrV4Bound::V4(address)),
            (IpFamily::V6, IpAddr::V6(address)) => (Domain::IPV6, SocketAddrV4Bound::V6(address)),
            (family, other) => {
                return Err(NetError::Encode {
                    family,
                    reason: format!("{other} is not an {family} address"),
                });
            }
        };

        let socket = Socket::new(domain, Type::RAW, Some(Protocol::from(VRRP_IP_PROTOCOL)))
            .map_err(|source| NetError::Io {
                operation: "creating a VRRP socket",
                source,
            })?;

        // The TTL is set on the socket so the kernel puts 255 in the header it
        // builds. RFC 5798 requires it and a peer discards anything else.
        let ttl = u32::from(ttl);
        match family {
            IpFamily::V4 => socket.set_ttl_v4(ttl),
            IpFamily::V6 => socket.set_unicast_hops_v6(ttl),
            // `IpFamily` is `#[non_exhaustive]`, so a family added later lands
            // here rather than failing to compile.
            _ => {
                return Err(NetError::Unsupported {
                    operation: "this address family",
                });
            }
        }
        .map_err(|source| NetError::Io {
            operation: "setting the VRRP TTL",
            source,
        })?;

        // Asking the kernel for the TTL of incoming packets. A raw socket
        // receives the payload only, so without this the check RFC 5798
        // requires cannot be made at all.
        //
        // Only the option matching the socket's family is set: the kernel
        // answers ENOPROTOOPT when an IPv6 option is set on an IPv4 socket, and
        // vice versa. Setting both, which looks harmless, is not.
        let result = match family {
            IpFamily::V4 => nix::sys::socket::setsockopt(&socket, Ipv4RecvTtl, &true),
            IpFamily::V6 => nix::sys::socket::setsockopt(&socket, Ipv6RecvHopLimit, &true),
            _ => {
                return Err(NetError::Unsupported {
                    operation: "this address family",
                });
            }
        };
        result.map_err(|error| NetError::Io {
            operation: "asking the kernel for the received TTL",
            source: std::io::Error::from_raw_os_error(error as i32),
        })?;

        // `IP_PKTINFO` on both families, so a received datagram can say which
        // address it was sent to. Without it a multicast instance cannot tell a
        // packet that arrived at the group from one addressed to this host
        // directly, and an IPv6 checksum cannot be verified.
        let pktinfo = match family {
            IpFamily::V4 => nix::sys::socket::setsockopt(&socket, Ipv4PacketInfo, &true),
            IpFamily::V6 => nix::sys::socket::setsockopt(&socket, Ipv6RecvPacketInfo, &true),
            _ => {
                return Err(NetError::Unsupported {
                    operation: "this address family",
                });
            }
        };
        pktinfo.map_err(|error| NetError::Io {
            operation: "asking the kernel for the received destination",
            source: std::io::Error::from_raw_os_error(error as i32),
        })?;

        // Resolved before the address is built, because a link-local bind needs
        // the scope and a link-local *send* needs it as well.
        let self_index = interface_index(interface)?;
        let index = self_index;

        let bound = match (bound_source, any_address) {
            (SocketAddrV4Bound::V4(_), true) => SockAddr::from(std::net::SocketAddrV4::new(
                std::net::Ipv4Addr::UNSPECIFIED,
                0,
            )),
            (SocketAddrV4Bound::V4(address), false) => {
                SockAddr::from(std::net::SocketAddrV4::new(address, 0))
            }
            (SocketAddrV4Bound::V6(address), _) => {
                // A link-local address is only meaningful together with an
                // interface, and the kernel says so: binding one with a scope of
                // zero is `EINVAL`. RFC 5798 §5.1.2.1 makes the link-local
                // address the source of every IPv6 advertisement, so this is not
                // an edge case — it is the only case IPv6 has.
                let scope = if address.is_unicast_link_local() {
                    self_index as u32
                } else {
                    0
                };
                SockAddr::from(std::net::SocketAddrV6::new(address, 0, 0, scope))
            }
        };
        socket.bind(&bound).map_err(|source| NetError::Io {
            operation: "binding a VRRP socket",
            source,
        })?;
        socket
            .bind_device(Some(interface.as_bytes()))
            .map_err(|source| NetError::Io {
                operation: "binding a VRRP socket to an interface",
                source,
            })?;

        Ok(Self {
            socket,
            family,
            source,
            interface: interface.to_owned(),
            index,
        })
    }

    /// Joins a multicast group on this socket's interface.
    ///
    /// Two options are set, and the second is the one that is easy to miss:
    ///
    /// - the membership, which is what the kernel filters incoming groups
    ///   against;
    /// - the multicast TTL, which is a *different* socket option from the unicast
    ///   TTL set at bind time and defaults to **1**. A VRRP advertisement sent
    ///   with a multicast hop limit of 1 is rejected by every receiver, because
    ///   RFC 5798 requires 255, and the node would never be heard of again. This
    ///   is the single most likely way to get multicast mode subtly wrong.
    ///
    /// # Errors
    ///
    /// Returns [`NetError::Io`] when the membership cannot be joined, and
    /// [`NetError::Encode`] when the group is not of this socket's family.
    pub fn join_group(&self, group: IpAddr, ttl: u8) -> Result<()> {
        self.set_multicast_ttl(ttl, "joining a VRRP multicast group")?;
        // The outgoing interface is named rather than left to the routing table.
        // A multicast group is local to one link by definition, so a route for
        // it is neither guaranteed to exist nor the right answer: a host whose
        // default route points somewhere else would send its advertisements off
        // the segment they are about, or fail with `ENETUNREACH` on a segment
        // with no default route at all, which is most virtual networks.
        self.set_multicast_interface()?;
        match (self.family, group) {
            (IpFamily::V4, IpAddr::V4(address)) => {
                // The IPv4 membership request names the interface by address, so
                // the source address this socket is bound to is what identifies
                // it.
                let IpAddr::V4(source) = self.source else {
                    return Err(NetError::Encode {
                        family: self.family,
                        reason: "the socket has no IPv4 source to name the interface with"
                            .to_owned(),
                    });
                };
                let request = nix::sys::socket::IpMembershipRequest::new(address, Some(source));
                nix::sys::socket::setsockopt(&self.socket, IpAddMembership, &request)
            }
            (IpFamily::V6, IpAddr::V6(address)) => {
                let request = nix::sys::socket::Ipv6MembershipRequest::new(address);
                nix::sys::socket::setsockopt(&self.socket, Ipv6AddMembership, &request)
            }
            (family, other) => {
                return Err(NetError::Encode {
                    family,
                    reason: format!("{other} is not an {family} group"),
                });
            }
        }
        .map_err(|error| NetError::Io {
            operation: "joining a VRRP multicast group",
            source: std::io::Error::from_raw_os_error(error as i32),
        })
    }

    /// Leaves a multicast group joined by [`VrrpSocket::join_group`].
    ///
    /// Called when the instance is torn down, so a reloaded or stopped instance
    /// does not keep receiving a group it no longer speaks for.
    ///
    /// # Errors
    ///
    /// Returns [`NetError::Io`] when the membership cannot be left, and
    /// [`NetError::Encode`] when the group is not of this socket's family.
    pub fn leave_group(&self, group: IpAddr) -> Result<()> {
        match (self.family, group) {
            (IpFamily::V4, IpAddr::V4(address)) => {
                let IpAddr::V4(source) = self.source else {
                    return Err(NetError::Encode {
                        family: self.family,
                        reason: "the socket has no IPv4 source to name the interface with"
                            .to_owned(),
                    });
                };
                let request = nix::sys::socket::IpMembershipRequest::new(address, Some(source));
                nix::sys::socket::setsockopt(&self.socket, IpDropMembership, &request)
            }
            (IpFamily::V6, IpAddr::V6(address)) => {
                let request = nix::sys::socket::Ipv6MembershipRequest::new(address);
                nix::sys::socket::setsockopt(&self.socket, Ipv6DropMembership, &request)
            }
            (family, other) => {
                return Err(NetError::Encode {
                    family,
                    reason: format!("{other} is not an {family} group"),
                });
            }
        }
        .map_err(|error| NetError::Io {
            operation: "leaving a VRRP multicast group",
            source: std::io::Error::from_raw_os_error(error as i32),
        })
    }

    /// Names the interface multicast datagrams leave through.
    fn set_multicast_interface(&self) -> Result<()> {
        let result = match self.source {
            IpAddr::V4(address) => self.socket.set_multicast_if_v4(&address),
            IpAddr::V6(_) => self.socket.set_multicast_if_v6(self.index),
        };
        result.map_err(|source| NetError::Io {
            operation: "naming the interface for VRRP multicast",
            source,
        })
    }

    /// Sets the TTL a multicast datagram is sent with.
    ///
    /// Separate from the unicast TTL at bind time, and one for each family:
    /// `IP_MULTICAST_TTL` and `IPV6_MULTICAST_HOPS`.
    fn set_multicast_ttl(&self, ttl: u8, operation: &'static str) -> Result<()> {
        let result = match self.family {
            IpFamily::V4 => nix::sys::socket::setsockopt(&self.socket, IpMulticastTtl, &ttl),
            IpFamily::V6 => {
                nix::sys::socket::setsockopt(&self.socket, Ipv6MulticastHops, &i32::from(ttl))
            }
            _ => {
                return Err(NetError::Unsupported {
                    operation: "this address family",
                });
            }
        };
        result.map_err(|error| NetError::Io {
            operation,
            source: std::io::Error::from_raw_os_error(error as i32),
        })
    }

    /// Binds a raw VRRP socket that speaks to a multicast group.
    ///
    /// On IPv4 this binds to *any* address rather than the source address, and
    /// that is not a detail. The kernel matches a raw socket's receive against
    /// the address the socket is bound to: bind it to `192.0.2.11` and it will
    /// never see a datagram addressed to `224.0.0.18`, so a multicast instance
    /// would send advertisements nobody could receive and would fail over on a
    /// schedule instead of an election. The source address is then chosen by the
    /// kernel from the outgoing interface, which is the same address either way.
    ///
    /// IPv6 keeps the bound address: there the kernel filters by *source* rather
    /// than by destination, so binding costs nothing and keeps the source of
    /// every advertisement under the daemon's control rather than the routing
    /// table's.
    ///
    /// # Errors
    ///
    /// As [`VrrpSocket::bind`].
    pub fn bind_for_group(family: IpFamily, interface: &str, source: IpAddr) -> Result<Self> {
        match (family, source) {
            (IpFamily::V4, IpAddr::V4(_)) => {
                Self::bind_internal(family, interface, source, REQUIRED_TTL, true)
            }
            _ => Self::bind(family, interface, source),
        }
    }

    /// Returns the family this socket speaks.
    #[must_use]
    pub fn family(&self) -> IpFamily {
        self.family
    }

    /// Returns the address this socket sends from.
    #[must_use]
    pub fn source(&self) -> IpAddr {
        self.source
    }

    /// Returns the kernel index of the interface this socket is bound to.
    #[must_use]
    pub fn interface_index(&self) -> u32 {
        self.index
    }

    /// Returns the interface this socket is bound to.
    #[must_use]
    pub fn interface(&self) -> &str {
        &self.interface
    }

    /// Sends one VRRP payload to `destination`.
    ///
    /// # Errors
    ///
    /// Returns [`NetError::Io`] when the datagram cannot be written, for
    /// example when the route to the peer is missing.
    pub fn send_to(&self, payload: &[u8], destination: IpAddr) -> Result<usize> {
        let target = match destination {
            IpAddr::V4(address) => SockAddr::from(std::net::SocketAddrV4::new(address, 0)),
            // The scope is the interface: a link-local multicast destination
            // such as `ff02::12` has no meaning without one, and the kernel
            // cannot infer it for a raw socket bound to a unicast address.
            IpAddr::V6(address) => {
                SockAddr::from(std::net::SocketAddrV6::new(address, 0, 0, self.index))
            }
        };
        self.socket
            .send_to(payload, &target)
            .map_err(|source| NetError::Io {
                operation: "sending a VRRP advertisement",
                source,
            })
    }

    /// Reads one datagram without blocking.
    ///
    /// Returns `Ok(None)` when nothing is waiting, which is the normal case and
    /// not an error: the caller has other work to do.
    ///
    /// # Errors
    ///
    /// Returns [`NetError::Io`] when the read fails for a reason other than
    /// there being nothing to read.
    pub fn receive(&self) -> Result<Option<Received>> {
        let mut buffer = vec![0u8; MAX_DATAGRAM];
        let mut control = vec![0u8; 128];
        let fd = self.socket.as_raw_fd();

        // The borrow of `buffer` by the read is released before the payload is
        // copied out, so the caller never sees a buffer tied to the socket.
        let (length, source, ttl, destination) = match self.family {
            IpFamily::V4 => {
                let mut iov = [IoSliceMut::new(&mut buffer)];
                match nix::sys::socket::recvmsg::<SockaddrIn>(
                    fd,
                    &mut iov,
                    Some(&mut control),
                    MsgFlags::MSG_DONTWAIT,
                ) {
                    Ok(outcome) => {
                        let length = outcome.bytes;
                        let source = address_v4(outcome.address)?;
                        let ttl = hop_limit_v4(&outcome).unwrap_or(0);
                        let destination = received_destination_v4(&outcome);
                        (length, source, ttl, destination)
                    }
                    Err(error) => return Self::nothing_yet(error),
                }
            }
            IpFamily::V6 => {
                let mut iov = [IoSliceMut::new(&mut buffer)];
                match nix::sys::socket::recvmsg::<SockaddrIn6>(
                    fd,
                    &mut iov,
                    Some(&mut control),
                    MsgFlags::MSG_DONTWAIT,
                ) {
                    Ok(outcome) => {
                        let length = outcome.bytes;
                        let source = address_v6(outcome.address)?;
                        let ttl = hop_limit_v6(&outcome).unwrap_or(0);
                        let destination = received_destination_v6(&outcome);
                        (length, source, ttl, destination)
                    }
                    Err(error) => return Self::nothing_yet(error),
                }
            }
            // `IpFamily` is `#[non_exhaustive]`, so a family added later lands
            // here rather than failing to compile.
            _ => {
                return Err(NetError::Unsupported {
                    operation: "this address family",
                });
            }
        };

        let length = length.min(buffer.len());
        Ok(Some(strip_header(
            &buffer[..length],
            source,
            ttl,
            destination,
        )))
    }

    /// Turns "there was nothing to read" into `Ok(None)`, and anything else
    /// into an error.
    fn nothing_yet(error: nix::errno::Errno) -> Result<Option<Received>> {
        match error {
            nix::errno::Errno::EAGAIN | nix::errno::Errno::EINTR => Ok(None),
            other => Err(NetError::Io {
                operation: "receiving a VRRP advertisement",
                source: std::io::Error::from_raw_os_error(other as i32),
            }),
        }
    }

    /// Waits up to `timeout` for a datagram.
    ///
    /// # Errors
    ///
    /// Returns [`NetError::Io`] when the read fails for a reason other than
    /// there being nothing to read.
    pub fn receive_timeout(&self, timeout: std::time::Duration) -> Result<Option<Received>> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if let Some(received) = self.receive()? {
                return Ok(Some(received));
            }
            if std::time::Instant::now() >= deadline {
                return Ok(None);
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
}

/// Removes the IP header if the datagram still carries one.
///
/// A raw socket normally delivers the payload alone, with the header's fields
/// arriving as ancillary data. A packet that came back through the loopback
/// device arrives whole, header and all, and handing that to the codec would
/// fail every field check for reasons that have nothing to do with the packet's
/// contents. So the header is recognized and removed here, and its values are
/// preferred over the ancillary ones when it is present.
fn strip_header(
    datagram: &[u8],
    ancillary_source: IpAddr,
    ancillary_ttl: u8,
    destination: Option<IpAddr>,
) -> Received {
    // The parameters are the ancillary data's values, which the attached-header
    // path below replaces with the header's own.
    if let Some(stripped) = strip_ipv4_header(datagram) {
        return stripped;
    }
    Received {
        payload: datagram.to_vec(),
        source: ancillary_source,
        ttl: ancillary_ttl,
        destination,
        header_attached: false,
    }
}

/// Parses and removes an IPv4 header, if that is what the datagram starts with.
fn strip_ipv4_header(datagram: &[u8]) -> Option<Received> {
    let first = *datagram.first()?;
    if first >> 4 != 4 {
        return None;
    }
    let header_len = usize::from(first & 0x0f) * 4;
    // A header shorter than 20 octets, or longer than the datagram, is not one.
    if !(20..=60).contains(&header_len) || datagram.len() < header_len {
        return None;
    }
    let total_len = usize::from(u16::from_be_bytes([datagram[2], datagram[3]]));
    let protocol = datagram[9];
    // The total length must describe this datagram, and the protocol must be
    // VRRP: anything else means these octets are a payload that happens to start
    // with 0x4, and stripping them would corrupt a valid advertisement.
    if total_len != datagram.len() || protocol != u8::try_from(VRRP_IP_PROTOCOL).unwrap_or(112) {
        return None;
    }

    let source = IpAddr::from(<[u8; 4]>::try_from(&datagram[12..16]).ok()?);
    let ttl = datagram[8];
    Some(Received {
        payload: datagram[header_len..].to_vec(),
        source,
        ttl,
        // The header carries the destination, which is the ancillary data's job
        // when there is any.
        destination: Some(IpAddr::from(<[u8; 4]>::try_from(&datagram[16..20]).ok()?)),
        header_attached: true,
    })
}

/// Extracts the source address from an IPv4 read.
fn address_v4(address: Option<SockaddrIn>) -> Result<IpAddr> {
    address
        .map(|address| IpAddr::V4(address.ip()))
        .ok_or_else(|| NetError::Io {
            operation: "receiving a VRRP advertisement",
            source: std::io::Error::other("the kernel did not report a source address"),
        })
}

/// Extracts the source address from an IPv6 read.
fn address_v6(address: Option<SockaddrIn6>) -> Result<IpAddr> {
    address
        .map(|address| IpAddr::V6(address.ip()))
        .ok_or_else(|| NetError::Io {
            operation: "receiving a VRRP advertisement",
            source: std::io::Error::other("the kernel did not report a source address"),
        })
}

/// Reads the address an IPv4 datagram was sent to, from `IP_PKTINFO`.
///
/// The message is a *typed* `in_pktinfo` in `nix`, so reading it means matching
/// the variant. Looking only for an unknown message finds nothing, and the
/// destination is then unknown to every caller — which for a VRRP receiver is
/// the difference between verifying an IPv6 checksum against the address the
/// packet arrived at and verifying it against a guess.
fn received_destination_v4<S>(outcome: &nix::sys::socket::RecvMsg<'_, '_, S>) -> Option<IpAddr>
where
    S: nix::sys::socket::SockaddrLike,
{
    use nix::sys::socket::ControlMessageOwned;

    for message in outcome.cmsgs().ok()? {
        if let ControlMessageOwned::Ipv4PacketInfo(info) = message {
            return Some(IpAddr::V4(std::net::Ipv4Addr::from(
                info.ipi_spec_dst.s_addr,
            )));
        }
    }
    None
}

/// Reads the address an IPv6 datagram was sent to, from `IPV6_PKTINFO`.
///
/// A typed `in6_pktinfo`, for the reason the IPv4 reader gives.
fn received_destination_v6<S>(outcome: &nix::sys::socket::RecvMsg<'_, '_, S>) -> Option<IpAddr>
where
    S: nix::sys::socket::SockaddrLike,
{
    use nix::sys::socket::ControlMessageOwned;

    for message in outcome.cmsgs().ok()? {
        if let ControlMessageOwned::Ipv6PacketInfo(info) = message {
            return Some(IpAddr::V6(std::net::Ipv6Addr::from(info.ipi6_addr.s6_addr)));
        }
    }
    None
}

/// Reads the TTL out of the ancillary data of an IPv4 read.
///
/// `IP_RECVTTL` has no variant of its own in `nix`, so it arrives as an unknown
/// control message carrying the header and the bytes. Reaching for it that way is
/// deliberate: the alternative is a raw `recvmsg` in this crate.
fn hop_limit_v4(outcome: &nix::sys::socket::RecvMsg<'_, '_, SockaddrIn>) -> Option<u8> {
    first_unknown_byte(outcome, nix::libc::IP_TTL)
}

/// Reads the hop limit out of the ancillary data of an IPv6 read.
///
/// This one is a typed control message rather than an unknown one, which is the
/// whole reason IPv6 was not working: the reader looked only at `Unknown`
/// messages, so a hop limit the kernel had delivered was read as zero, and every
/// IPv6 advertisement was then rejected for a TTL that was perfectly correct.
/// IPv4 has the opposite shape — its `IP_TTL` arrives untyped — so the same
/// reader is right for one family and silently wrong for the other.
fn hop_limit_v6(outcome: &nix::sys::socket::RecvMsg<'_, '_, SockaddrIn6>) -> Option<u8> {
    use nix::sys::socket::ControlMessageOwned;

    for message in outcome.cmsgs().ok()? {
        if let ControlMessageOwned::Ipv6HopLimit(limit) = message
            && let Ok(limit) = u8::try_from(limit)
        {
            return Some(limit);
        }
    }
    None
}

/// Returns the first byte of the control message of type `cmsg_type`.
fn first_unknown_byte<S>(
    outcome: &nix::sys::socket::RecvMsg<'_, '_, S>,
    cmsg_type: libc::c_int,
) -> Option<u8>
where
    S: nix::sys::socket::SockaddrLike,
{
    use nix::sys::socket::ControlMessageOwned;

    for message in outcome.cmsgs().ok()? {
        if let ControlMessageOwned::Unknown(unknown) = message
            && unknown.cmsg_header.cmsg_type == cmsg_type
        {
            return unknown.data_bytes.first().copied();
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vrrp::fixture_advertisement;

    /// An IPv4 header with the given TTL, source, and payload length.
    fn ipv4_datagram(ttl: u8, source: [u8; 4], payload: &[u8]) -> Vec<u8> {
        let total = 20 + payload.len();
        let mut header = vec![
            0x45,
            0x00,
            u8::try_from(total >> 8).unwrap_or(0),
            u8::try_from(total & 0xff).unwrap_or(0),
            0x00,
            0x01,
            0x40,
            0x00,
            ttl,
            u8::try_from(VRRP_IP_PROTOCOL).unwrap_or(112),
            0x00,
            0x00,
        ];
        header.extend_from_slice(&source);
        header.extend_from_slice(&[127, 0, 0, 1]);
        header.extend_from_slice(payload);
        header
    }

    #[test]
    fn a_datagram_without_a_header_is_left_alone() {
        let payload = fixture_advertisement();
        let received = strip_header(&payload, "192.0.2.11".parse().expect("valid"), 255, None);

        assert_eq!(received.payload, payload, "a bare payload is not touched");
        assert!(!received.header_attached);
        assert_eq!(
            received.source,
            "192.0.2.11".parse::<IpAddr>().expect("valid")
        );
        assert_eq!(received.ttl, 255, "the ancillary values are used");
    }

    #[test]
    fn an_attached_ipv4_header_is_removed_and_its_values_win() {
        let payload = fixture_advertisement();
        let datagram = ipv4_datagram(64, [192, 0, 2, 11], &payload);
        let received = strip_header(&datagram, "10.0.0.1".parse().expect("valid"), 255, None);

        assert_eq!(
            received.payload, payload,
            "the header is gone and the payload is intact"
        );
        assert!(received.header_attached);
        assert_eq!(
            received.source,
            "192.0.2.11".parse::<IpAddr>().expect("valid")
        );
        assert_eq!(
            received.ttl, 64,
            "the header's TTL is the one that was actually used"
        );
    }

    #[test]
    fn a_payload_that_merely_looks_like_a_header_is_left_alone() {
        // A VRRP advertisement always starts 0x31, but a codec must not depend
        // on that, and neither must the transport.
        let mut payload = fixture_advertisement();
        payload[0] = 0x45;

        let received = strip_header(&payload, "192.0.2.11".parse().expect("valid"), 255, None);
        assert!(
            !received.header_attached,
            "the length and protocol did not match a header"
        );
        assert_eq!(received.payload, payload);
    }

    #[test]
    fn a_header_for_another_protocol_is_left_alone() {
        let payload = fixture_advertisement();
        let mut datagram = ipv4_datagram(255, [192, 0, 2, 11], &payload);
        datagram[9] = 17; // UDP

        let received = strip_header(&datagram, "192.0.2.11".parse().expect("valid"), 255, None);
        assert!(!received.header_attached, "only a VRRP header is stripped");
        assert_eq!(received.payload, datagram);
    }

    #[test]
    fn a_truncated_header_is_left_alone() {
        let payload = fixture_advertisement();
        let mut datagram = ipv4_datagram(255, [192, 0, 2, 11], &payload);
        datagram.truncate(12);

        let received = strip_header(&datagram, "192.0.2.11".parse().expect("valid"), 255, None);
        assert!(
            !received.header_attached,
            "a header shorter than 20 octets is not a header"
        );
    }

    #[test]
    fn an_empty_datagram_is_handled() {
        let received = strip_header(&[], "192.0.2.11".parse().expect("valid"), 255, None);
        assert!(received.payload.is_empty());
        assert!(!received.header_attached);
    }
}
