// Rust guideline compliant 2026-09-27

//! Gratuitous announcements: telling the segment that an address is now here.
//!
//! A node that takes over a virtual address has to tell the neighbours, because
//! every one of them has a cache entry pointing at the node that had the address
//! last. Without an announcement they keep sending to a dead node until that
//! cache entry ages out, which is a black hole measured in minutes rather than
//! in the millisecond the failover took. This is the same reason the
//! specification requires these announcements as step 5 and 6 of becoming
//! `MASTER` (§14.1).
//!
//! Two announcements, one per family:
//!
//! - IPv4: an ARP **reply** whose sender and target protocol address are both
//!   the address being claimed, sent to the broadcast hardware address. A reply
//!   is the correct form rather than a request; a request would ask the question
//!   the sender already knows the answer to, and the target hardware address is
//!   broadcast for the same reason.
//! - IPv6: an unsolicited Neighbor Advertisement (§7.2.4 of RFC 4861) sent to
//!   the all-nodes multicast group, with the override flag set and a
//!   link-layer address option, because the new node's claim outranks whatever
//!   the cache says.
//!
//! # Where the frames are built and where they are sent
//!
//! The frame builders are pure functions and are tested on every platform, so a
//! malformed announcement is caught without a kernel. Only the sending needs
//! Linux, and only the sending needs one audited `unsafe` island: an `AF_PACKET`
//! send takes a `sockaddr_ll`, and `socket2` has no safe constructor for one.
//! The reasoning, and the alternative that was rejected, are recorded in
//! `docs/adr/ADR-0004-gratuitous-arp.md`.
//!
//! A failure here is never fatal. The state machine treats a failed gratuitous
//! update as non-fatal by design (§11.4), because a node that owns an address
//! and cannot announce it is still better than a node that gives the address up.

#[cfg(target_os = "linux")]
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr};

/// The Ethernet broadcast address.
pub const BROADCAST: [u8; 6] = [0xff; 6];

/// The length of an Ethernet hardware address.
pub const HARDWARE_ADDRESS_LENGTH: usize = 6;

/// An Ethernet hardware address.
pub type HardwareAddress = [u8; HARDWARE_ADDRESS_LENGTH];

/// `EtherType` for ARP.
const ETHERTYPE_ARP: u16 = 0x0806;
/// ARP hardware type 1: Ethernet.
const ARP_HARDWARE_ETHERNET: u16 = 1;
/// ARP protocol type for IPv4, which is the `EtherType` for IPv4.
const ARP_PROTOCOL_IPV4: u16 = 0x0800;
/// ARP operation 2: reply. A gratuitous announcement is a reply to nobody.
const ARP_OPERATION_REPLY: u16 = 2;
/// `ICMPv6` type 136: Neighbor Advertisement.
const ICMPV6_NEIGHBOUR_ADVERTISEMENT: u8 = 136;
/// The all-nodes multicast group, which is who must hear a takeover.
const ALL_NODES_MULTICAST: [u8; 16] = [0xff, 0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x01];

/// Builds the gratuitous ARP frame announcing `address`.
///
/// The frame is a complete Ethernet frame, because an `AF_PACKET` send takes
/// one: the kernel does not add a header of its own at this layer, and adding
/// the wrong one is how an announcement ends up invisible to the neighbour it
/// was meant for.
///
/// # Arguments
///
/// - `hardware`: the announcing interface's hardware address, which becomes the
///   sender hardware address and the target hardware address is broadcast.
/// - `address`: the address being claimed, which is both the sender and the
///   target protocol address.
///
/// # Returns
///
/// A 42-byte frame: 14 bytes of Ethernet header and 28 bytes of ARP.
#[must_use]
pub fn gratuitous_arp_frame(hardware: HardwareAddress, address: Ipv4Addr) -> Vec<u8> {
    let mut frame = Vec::with_capacity(42);

    // Ethernet header: broadcast, because nobody owns the address yet.
    frame.extend_from_slice(&BROADCAST);
    frame.extend_from_slice(&hardware);
    frame.extend_from_slice(&ETHERTYPE_ARP.to_be_bytes());

    // ARP: a reply from this address to this address.
    frame.extend_from_slice(&ARP_HARDWARE_ETHERNET.to_be_bytes());
    frame.extend_from_slice(&ARP_PROTOCOL_IPV4.to_be_bytes());
    // The length is a constant six, so the conversion cannot fail; written this
    // way so the frame cannot be silently wrong if that ever stops being true.
    let hardware_length = u8::try_from(HARDWARE_ADDRESS_LENGTH).unwrap_or(u8::MAX);
    frame.extend_from_slice(&hardware_length.to_be_bytes());
    frame.extend_from_slice(&4u8.to_be_bytes());
    frame.extend_from_slice(&ARP_OPERATION_REPLY.to_be_bytes());
    frame.extend_from_slice(&hardware);
    frame.extend_from_slice(&address.octets());
    frame.extend_from_slice(&BROADCAST);
    frame.extend_from_slice(&address.octets());

    frame
}

/// Builds the `ICMPv6` payload of an unsolicited Neighbor Advertisement.
///
/// This is the `ICMPv6` message only. The kernel builds the IPv6 header and, for a
/// raw `ICMPv6` socket, always computes and inserts the checksum, so writing one
/// here would be wrong rather than merely redundant.
///
/// # Arguments
///
/// - `hardware`: the announcing interface's hardware address, carried in the
///   link-layer address option so the neighbour can update the cache entry it is
///   being sent to correct.
/// - `address`: the address being claimed, which is the advertisement's target
///   address. The override flag is set, because a takeover outranks whatever the
///   receiver's cache claims, and the solicited flag is clear because no
///   neighbour asked.
#[must_use]
pub fn neighbour_advertisement(hardware: HardwareAddress, address: Ipv6Addr) -> Vec<u8> {
    let mut message = Vec::with_capacity(32);

    // Type, code, and checksum. The checksum is the kernel's to write.
    message.push(ICMPV6_NEIGHBOUR_ADVERTISEMENT);
    message.push(0);
    message.extend_from_slice(&[0, 0]);

    // The target address is the address being claimed.
    message.extend_from_slice(&address.octets());

    // Flags: override set, solicited clear, router clear. The top four bits are
    // reserved and zero.
    message.push(0b1000_0000);

    // Option type 2: target link-layer address, length one unit of 8 octets.
    message.push(2);
    message.extend_from_slice(&1u16.to_be_bytes());
    message.extend_from_slice(&hardware);

    message
}

/// Returns the all-nodes multicast address, which is where an IPv6 announcement
/// is sent.
#[must_use]
pub fn all_nodes_multicast() -> Ipv6Addr {
    Ipv6Addr::from(ALL_NODES_MULTICAST)
}

/// Sends a gratuitous ARP frame for `address` out of `interface`.
///
/// # Arguments
///
/// - `interface`: the kernel index to send from.
/// - `hardware`: the interface's hardware address, as
///   [`hardware_address`] reports it.
/// - `address`: the address being claimed.
///
/// # Errors
///
/// Returns the kernel's error if the socket cannot be opened or the frame cannot
/// be written. The caller treats that as non-fatal (§11.4).
#[cfg(target_os = "linux")]
pub fn send_gratuitous_arp(
    interface: crate::types::InterfaceId,
    hardware: HardwareAddress,
    address: Ipv4Addr,
) -> io::Result<()> {
    let frame = gratuitous_arp_frame(hardware, address);
    let socket = packet_socket(interface, ETHERTYPE_ARP)?;
    socket
        .send_to(&frame, &link_layer_address(interface, ETHERTYPE_ARP))
        .map(|_| ())
}

/// Sends an unsolicited Neighbor Advertisement for `address` out of `interface`.
///
/// # Arguments
///
/// - `interface`: the name of the interface to send from, which the kernel needs
///   to pick a source address for the multicast group.
/// - `address`: the address being claimed, which is also the source address of
///   the message, so the socket is bound to it.
///
/// # Errors
///
/// Returns the kernel's error if the socket cannot be opened, cannot be bound to
/// the address, or the message cannot be written. The caller treats that as
/// non-fatal (§11.4).
#[cfg(target_os = "linux")]
pub fn send_neighbour_advertisement(
    interface: crate::types::InterfaceId,
    name: &str,
    address: Ipv6Addr,
) -> io::Result<()> {
    use socket2::{Domain, Protocol, SockAddr, Socket, Type};
    use std::net::SocketAddrV6;

    // The hardware address is read from the kernel for the link-layer option,
    // because a wrong one teaches the neighbour a wrong cache entry, which is
    // worse than no announcement at all.
    let hardware = hardware_address(name)?;

    let socket = Socket::new(Domain::IPV6, Type::RAW, Some(Protocol::ICMPV6))?;

    // Bound to the device rather than left to the routing table: the destination
    // is a multicast group, and a host with several interfaces must not announce
    // on the wrong one.
    socket.bind_device(Some(name.as_bytes()))?;

    // A raw ICMPv6 socket is handed the ICMPv6 message only. The kernel builds
    // the IPv6 header, and for this protocol it always computes and inserts the
    // checksum, so writing one here would be wrong rather than merely redundant.
    // Binding to the address being claimed is what makes it the source, since a
    // route lookup would otherwise pick the interface's primary address and the
    // announcement would claim something the node does not own.
    let source = SockAddr::from(SocketAddrV6::new(address, 0, 0, 0));
    socket.bind(&source)?;

    // The scope is the interface index: a multicast address that is not
    // link-local still has to be sent through the interface the claim is on.
    let destination = SockAddr::from(SocketAddrV6::new(
        all_nodes_multicast(),
        0,
        0,
        u32::try_from(interface.get()).unwrap_or(0),
    ));
    socket
        .send_to(&neighbour_advertisement(hardware, address), &destination)
        .map(|_| ())
}

/// Returns an `AF_PACKET` socket bound for sending on `interface`.
///
/// The interface is bound rather than chosen at send time, so two instances on
/// one host cannot have their announcements attributed to each other's link.
#[cfg(target_os = "linux")]
fn packet_socket(
    interface: crate::types::InterfaceId,
    protocol: u16,
) -> io::Result<socket2::Socket> {
    use socket2::{Domain, Protocol, Socket, Type};

    let socket = Socket::new(
        Domain::PACKET,
        Type::RAW,
        Some(Protocol::from(i32::from(protocol))),
    )?;
    socket.bind(&link_layer_address(interface, protocol))?;
    Ok(socket)
}

/// Builds the `sockaddr_ll` an `AF_PACKET` send needs.
///
/// This is one of the two audited `unsafe` islands in this crate. `sockaddr_ll`
/// has no safe representation in `socket2` or `nix`, and it is the only way to
/// say "this frame, on this interface, in this protocol" at layer two.
///
/// # Safety
///
/// Every field the kernel reads is written before the value is handed over: the
/// family, the interface index, the protocol, and the hardware address length
/// are all set explicitly, and the remainder is zeroed, so the kernel cannot
/// read uninitialised memory. The value is a local, it is never aliased, and it
/// outlives the send that borrows it.
#[cfg(target_os = "linux")]
// The two casts below narrow C constants into the widths their own fields
// declare. Both values are small and both are fixed by the platform headers, so
// there is nothing to handle; the alternative is a runtime check on a constant.
#[allow(unsafe_code, clippy::cast_possible_truncation)]
fn link_layer_address(interface: crate::types::InterfaceId, protocol: u16) -> socket2::SockAddr {
    let mut storage = socket2::SockAddrStorage::zeroed();

    // SAFETY: `sockaddr_ll` is one of this platform's `sockaddr_*` types and is
    // smaller than the storage, which is what `view_as` asserts.
    let link = unsafe { storage.view_as::<libc::sockaddr_ll>() };
    link.sll_family = libc::AF_PACKET as libc::sa_family_t;
    link.sll_protocol = protocol.to_be();
    link.sll_ifindex = interface.get();
    link.sll_halen = HARDWARE_ADDRESS_LENGTH as _;
    link.sll_addr[..HARDWARE_ADDRESS_LENGTH].copy_from_slice(&BROADCAST);

    // SAFETY: the storage holds a fully initialised `sockaddr_ll` and the length
    // is that of the type it holds, which is what the kernel reads to know how
    // much of the address is meaningful. The storage is a local, moved into the
    // returned value, and never aliased.
    unsafe {
        socket2::SockAddr::new(
            storage,
            libc::socklen_t::try_from(std::mem::size_of::<libc::sockaddr_ll>())
                .expect("a sockaddr_ll is far smaller than a socklen_t"),
        )
    }
}

/// Returns the hardware address of a named interface.
///
/// The kernel reports it in the `AF_PACKET` entry of `getifaddrs`, as a
/// `sockaddr_ll` whose `sll_addr` holds the address. `rtnetlink` 0.23 does not
/// expose `IFLA_ADDRESS`, and there is no socket option that reports an
/// interface's own hardware address, so this is where it comes from.
///
/// This is the second and last audited `unsafe` island in this crate. It is
/// needed because a `sockaddr_storage` is a family-agnostic buffer and reading
/// it as a `sockaddr_ll` is a cast, not a conversion.
///
/// # Arguments
///
/// - `interface`: the interface name, for example `eth0`.
///
/// # Errors
///
/// Returns an error if the interfaces cannot be listed, if the interface is not
/// found, or if it has no hardware address at all — a tunnel, a loopback, or a
/// device whose address the kernel reports as all zeroes. An interface with no
/// hardware address has nothing to announce with, and a frame built with a zero
/// address would teach every neighbour a wrong cache entry.
///
/// # Safety of the reasoning
///
/// The read is bounded twice over: the family is checked to be `AF_PACKET`
/// before the structure is read as a `sockaddr_ll`, and `sll_halen` is checked
/// against the six octets of an Ethernet address before any of it is copied. The
/// storage glibc returns is a `sockaddr_storage`, which is larger than a
/// `sockaddr_ll`, so the read stays inside the allocation. A test asserts the
/// behaviour against a real interface, because this is exactly the kind of code
/// that is right in reasoning and wrong in practice.
#[cfg(target_os = "linux")]
// The one cast narrows a C constant into the width of the field that holds it.
// `AF_PACKET` is 17 and a `sa_family_t` is 16 bits, so there is nothing to
// handle; the alternative is a runtime check on a constant.
#[allow(unsafe_code, clippy::cast_possible_truncation)]
pub fn hardware_address(interface: &str) -> io::Result<HardwareAddress> {
    use nix::sys::socket::SockaddrLike;

    let addresses = nix::ifaddrs::getifaddrs().map_err(io::Error::from)?;
    for entry in addresses {
        if entry.interface_name != interface {
            continue;
        }
        // An interface with a hardware address has an `AF_PACKET` entry as well
        // as one per configured address, and that is the entry carrying it.
        let Some(storage) = entry.address else {
            continue;
        };

        // SAFETY: `storage` is a `sockaddr_storage` from the C library. It is
        // larger than a `sockaddr_ll`, so reading the family is inside the
        // allocation, and the read as a `sockaddr_ll` happens only when that
        // family says the contents are one.
        let hardware = unsafe {
            let link = storage.as_ptr().cast::<libc::sockaddr_ll>();
            if (*link).sll_family != libc::AF_PACKET as libc::sa_family_t {
                continue;
            }
            let length = (*link).sll_halen as usize;
            if length < HARDWARE_ADDRESS_LENGTH {
                continue;
            }
            let bytes = *std::ptr::addr_of!((*link).sll_addr);
            let mut address = [0u8; HARDWARE_ADDRESS_LENGTH];
            address.copy_from_slice(&bytes[..HARDWARE_ADDRESS_LENGTH]);
            address
        };

        // All zeroes is what an interface with no hardware address reports, and
        // sending it would be worse than not sending.
        if hardware.iter().any(|byte| *byte != 0) {
            return Ok(hardware);
        }
    }

    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        format!("interface {interface} has no hardware address to announce with"),
    ))
}

#[cfg(test)]
mod tests {
    use super::{
        BROADCAST, ETHERTYPE_ARP, HARDWARE_ADDRESS_LENGTH, ICMPV6_NEIGHBOUR_ADVERTISEMENT,
        gratuitous_arp_frame, neighbour_advertisement,
    };
    use std::net::{Ipv4Addr, Ipv6Addr};

    const HARDWARE: [u8; HARDWARE_ADDRESS_LENGTH] = [0x02, 0x00, 0x00, 0x00, 0x00, 0x01];

    /// The frame is checked field by field rather than against a golden blob, so a
    /// failure says which field is wrong.
    #[test]
    fn a_gratuitous_arp_is_a_reply_from_the_address_to_itself() {
        let address = Ipv4Addr::new(192, 0, 2, 100);
        let frame = gratuitous_arp_frame(HARDWARE, address);

        assert_eq!(frame.len(), 42, "an Ethernet frame plus an ARP payload");
        assert_eq!(
            &frame[0..6],
            &BROADCAST,
            "broadcast, because nobody owns it yet"
        );
        assert_eq!(&frame[6..12], &HARDWARE, "sent by the announcing interface");
        assert_eq!(u16::from_be_bytes([frame[12], frame[13]]), ETHERTYPE_ARP);

        assert_eq!(u16::from_be_bytes([frame[14], frame[15]]), 1, "Ethernet");
        assert_eq!(u16::from_be_bytes([frame[16], frame[17]]), 0x0800, "IPv4");
        assert_eq!(frame[18], 6, "six octets of hardware address");
        assert_eq!(frame[19], 4, "four octets of protocol address");
        assert_eq!(
            u16::from_be_bytes([frame[20], frame[21]]),
            2,
            "a reply, which is what a gratuitous announcement is"
        );
        assert_eq!(&frame[22..28], &HARDWARE, "the sender is this node");
        assert_eq!(
            &frame[28..32],
            &address.octets(),
            "announcing the address it claims"
        );
        assert_eq!(&frame[32..38], &BROADCAST, "to everyone");
        assert_eq!(
            &frame[38..42],
            &address.octets(),
            "about itself, which is what makes it gratuitous"
        );
    }

    #[test]
    fn a_neighbour_advertisement_overrides_the_cache_and_asks_nothing() {
        let address = Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 100);
        let message = neighbour_advertisement(HARDWARE, address);

        assert_eq!(message[0], ICMPV6_NEIGHBOUR_ADVERTISEMENT);
        assert_eq!(message[1], 0, "a neighbour advertisement has no code");
        assert_eq!(&message[2..4], &[0, 0], "the kernel writes the checksum");
        assert_eq!(
            &message[4..20],
            &address.octets(),
            "the target is the address claimed"
        );
        assert_eq!(
            message[20] & 0b1000_0000,
            0b1000_0000,
            "override, or the receiver would ignore the new claim"
        );
        assert_eq!(
            message[20] & 0b0100_0000,
            0,
            "not solicited, because nobody asked"
        );
        assert_eq!(
            message[20] & 0b0010_0000,
            0,
            "not a router, because it is not one"
        );
        assert_eq!(message[21], 2, "the link-layer address option");
        assert_eq!(&message[22..24], &[0, 1], "one 8-octet unit");
        assert_eq!(
            &message[24..30],
            &HARDWARE,
            "the new neighbour's hardware address"
        );
    }
}
