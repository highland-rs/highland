// Rust guideline compliant 2026-09-27

//! Reading frames off an interface.
//!
//! `tcpdump` is the obvious tool and it is not always available: in some
//! containerised networking stacks it captures nothing at all while `ip` and
//! `tc netem` work perfectly, which makes "the packet never arrived" and "the
//! capture cannot see it" indistinguishable. This module is that distinction
//! resolved, in about fifty lines and with one audited `unsafe`:
//!
//! - `AF_PACKET` with `SOCK_RAW` returns whole frames, Ethernet header first,
//!   which is what a diagnostic wants to print;
//! - binding by device *name* needs no `sockaddr_ll`, so there is no address to
//!   lay out by hand;
//! - the one `unsafe` is `MaybeUninit::assume_init` on bytes the kernel has just
//!   written, which is the contract `socket2`'s read API is built on.
//!
//! Nothing in the daemon captures. This exists because a protocol
//! interoperability question — "does the peer's checksum cover the same bytes
//! ours does?" — cannot be answered any other way when a capture is blind.

#![allow(unsafe_code)]

use std::mem::MaybeUninit;
use std::time::Duration;

use socket2::{Domain, Protocol, Socket, Type};

/// The VRRP protocol number, IP protocol 112.
pub const VRRP_PROTOCOL: u8 = 112;

/// One frame as the interface received it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// The whole frame, Ethernet header first.
    pub bytes: Vec<u8>,
}

impl Frame {
    /// The `EtherType`, from the two bytes at offset 12.
    #[must_use]
    pub fn ether_type(&self) -> u16 {
        u16::from_be_bytes([
            *self.bytes.get(12).unwrap_or(&0),
            *self.bytes.get(13).unwrap_or(&0),
        ])
    }

    /// The IP payload, skipping the Ethernet header and any VLAN tag.
    #[must_use]
    pub fn payload(&self) -> &[u8] {
        let start = if self.ether_type() == 0x8100 { 18 } else { 14 };
        self.bytes.get(start..).unwrap_or_default()
    }

    /// The IP protocol number, for IPv4 and IPv6 alike.
    #[must_use]
    pub fn ip_protocol(&self) -> u8 {
        match self.ether_type() {
            0x0800 => self.bytes.get(23).copied().unwrap_or(0),
            0x86dd => self.bytes.get(20).copied().unwrap_or(0),
            _ => 0,
        }
    }

    /// Returns `true` when this frame carries VRRP.
    #[must_use]
    pub fn is_vrrp(&self) -> bool {
        self.ip_protocol() == VRRP_PROTOCOL
    }
}

/// Opens a capture on `interface`.
///
/// # Errors
///
/// Returns the socket error: `CAP_NET_RAW` is missing, or the interface does not
/// exist.
pub fn open(interface: &str) -> std::io::Result<Socket> {
    // `ETH_P_ALL` (3), which is every protocol: a capture that has to be told
    // which protocol to watch is a capture that silently sees nothing.
    let socket = Socket::new(Domain::PACKET, Type::RAW, Some(Protocol::from(3_i32)))?;
    socket.set_read_timeout(Some(Duration::from_millis(500)))?;
    // Binding by name is what makes this safe to write: no `sockaddr_ll`, and
    // therefore no address to get wrong.
    socket.bind_device(Some(interface.as_bytes()))?;
    Ok(socket)
}

/// Reads frames until `count` have arrived or the budget runs out.
///
/// # Errors
///
/// Returns the read error, which for a timeout is `WouldBlock` and is not a
/// failure: a quiet segment is the normal case.
pub fn read(socket: &Socket, count: usize, budget: Duration) -> std::io::Result<Vec<Frame>> {
    let deadline = std::time::Instant::now() + budget;
    let mut frames = Vec::new();
    // The read API is uninitialised-by-design, which is the right default for a
    // socket: the kernel is told how much space there is and writes only what it
    // wrote, so there is no stale zero tail to be mistaken for payload.
    let mut buffer = [MaybeUninit::<u8>::uninit(); 2048];
    while frames.len() < count && std::time::Instant::now() < deadline {
        match socket.recv(&mut buffer) {
            Ok(read) => {
                let bytes = buffer[..read]
                    .iter()
                    // SAFETY: the kernel reported writing `read` bytes into
                    // `buffer[..read]`, so every element in that range is
                    // initialised, and the range is the one being copied out.
                    .map(|byte| unsafe { byte.assume_init() })
                    .collect();
                frames.push(Frame { bytes });
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(frames)
}

/// Reads the VRRP frames among `frames`.
#[must_use]
pub fn vrrp_frames(frames: &[Frame]) -> Vec<&Frame> {
    frames.iter().filter(|frame| frame.is_vrrp()).collect()
}

/// Formats bytes as spaced hex, for a log a human can read.
#[must_use]
pub fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The RFC 1071 one's complement sum of `bytes`, and the folded value.
///
/// A valid checksum over a message folds to `0xffff`.
///
/// # Panics
///
/// Never: the arithmetic is on `u32` values that cannot overflow at this size.
#[must_use]
pub fn ones_complement(bytes: &[u8]) -> (u32, u32) {
    let mut sum: u32 = 0;
    let mut index = 0;
    while index + 1 < bytes.len() {
        sum += u32::from(u16::from_be_bytes([bytes[index], bytes[index + 1]]));
        index += 2;
    }
    if index < bytes.len() {
        // An odd trailing byte is the high half of a word, which is what makes
        // an odd-length message work at all.
        sum += u32::from(bytes[index]) << 8;
    }
    let mut folded = sum;
    while folded > 0xffff {
        folded = (folded & 0xffff) + (folded >> 16);
    }
    (sum, folded)
}
