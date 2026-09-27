// Rust guideline compliant 2026-09-27

//! The VRRP transport: a raw IP socket, peer filtering, and rate limiting.
//!
//! RFC 5798 §5.1.1.4 and §5.1.2.4 assign VRRP IP protocol 112, and §5.1.1.3 and
//! §5.1.2.3 require the TTL or hop limit to be exactly 255. Both are enforced
//! here, on the packet, before an advertisement is handed to the state machine:
//! a packet that fails either check never reaches it.
//!
//! Peer filtering lives here rather than in the codec, because the codec is a
//! pure function of bytes and a peer list is not (SPEC.md, §9.2).
//!
//! # Why the socket is built the way it is
//!
//! A raw IP socket for protocol 112 does not include the IP header on receive,
//! so the TTL cannot be read from the datagram. The kernel offers it as
//! ancillary data, which needs `recvmsg`, which `socket2` does not expose. So the
//! socket is created and sent through `socket2`, and received through `nix`.
//! Both are actively maintained, and neither needs `unsafe` in this crate.
//! `pnet_datalink` would have covered this too, but it has been unmaintained
//! since May 2024.

use std::net::IpAddr;
use std::time::Duration;

use highland_vrrp::{
    Advertisement, ChecksumScope, DecodeError, IpFamily, MaxAdverInt, Peek, VRRP_PROTOCOL,
};

use crate::error::{NetError, Result};

/// The IP protocol number assigned to VRRP, as a socket constructor takes it.
pub const VRRP_IP_PROTOCOL: i32 = VRRP_PROTOCOL as i32;

/// The TTL or hop limit a VRRP packet must carry (RFC 5798 §5.1.1.3, §5.1.2.3).
pub const REQUIRED_TTL: u8 = 255;

/// The largest datagram the transport will read, which bounds what a hostile
/// peer can make the kernel copy (`S-05`).
pub const MAX_DATAGRAM: usize = 4096;

/// The advertisements accepted per second, above which the excess is dropped and
/// counted (`L-11`).
pub const DEFAULT_ACCEPT_RATE: u32 = 2000;

/// Why a datagram was not turned into an advertisement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Rejection {
    /// The datagram was shorter than the fixed header.
    TooShort,
    /// The version field was not 3.
    BadVersion,
    /// The type field was not 1.
    BadType,
    /// The TTL or hop limit was not 255.
    BadTtl,
    /// The datagram's length disagreed with its own count.
    BadLength,
    /// The checksum did not verify.
    BadChecksum,
    /// The source address is not a configured peer.
    UnknownPeer,
    /// The VRID is not the one this socket serves.
    WrongVrid,
    /// The rate limit was exceeded.
    RateLimited,
}

impl Rejection {
    /// Returns the stable name used in the `highland_rejected_packets_total`
    /// metric's `reason` label (`R-20`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Rejection::TooShort => "too_short",
            Rejection::BadVersion => "bad_version",
            Rejection::BadType => "bad_type",
            Rejection::BadTtl => "bad_ttl",
            Rejection::BadLength => "bad_length",
            Rejection::BadChecksum => "bad_checksum",
            Rejection::UnknownPeer => "unknown_peer",
            Rejection::WrongVrid => "wrong_vrid",
            Rejection::RateLimited => "rate_limited",
        }
    }
}

impl std::fmt::Display for Rejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An advertisement that passed every check, and who sent it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptedAdvertisement {
    /// The decoded advertisement.
    pub advertisement: Advertisement,
    /// The address the datagram came from.
    ///
    /// This is the peer's identity, and it is the address the VRRP header was
    /// sent from. The state machine records it so an operator can see which
    /// neighbour changed the outcome (`R-29`).
    pub source: IpAddr,
}

/// The result of offering a datagram to the transport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Accepted {
    /// The datagram is a valid advertisement from a configured peer.
    Advertisement(AcceptedAdvertisement),
    /// The datagram was rejected, with the reason.
    Rejected(Rejection),
}

impl Accepted {
    /// Returns the advertisement, when it was accepted.
    #[must_use]
    pub fn advertisement(&self) -> Option<&Advertisement> {
        match self {
            Accepted::Advertisement(accepted) => Some(&accepted.advertisement),
            Accepted::Rejected(_) => None,
        }
    }

    /// Returns who sent the advertisement, when it was accepted.
    #[must_use]
    pub fn source(&self) -> Option<IpAddr> {
        match self {
            Accepted::Advertisement(accepted) => Some(accepted.source),
            Accepted::Rejected(_) => None,
        }
    }

    /// Returns the rejection reason, when it was rejected.
    #[must_use]
    pub fn rejection(&self) -> Option<Rejection> {
        match self {
            Accepted::Advertisement(_) => None,
            Accepted::Rejected(reason) => Some(*reason),
        }
    }
}

/// What the transport needs to know about a received datagram, as reported by
/// the platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Datagram<'a> {
    /// The bytes read from the socket.
    pub bytes: &'a [u8],
    /// The source address of the packet, which is the address the VRRP header was
    /// sent from and is therefore the peer's identity.
    pub source: IpAddr,
    /// The TTL or hop limit, read from the IP header by the kernel.
    pub ttl: u8,
}

/// Validates a received datagram against the instance it belongs to.
///
/// The order is deliberate and cheap: the rate limit and the TTL first, then the
/// length and the header, then the peer list, and only then the checksum, which
/// is the most expensive check. Nothing here allocates from the packet's own
/// count field before the length has been checked against it.
///
/// # Examples
///
/// ```
/// use highland_net::{Accepted, AllowedSources, Datagram, PeerSet, validate};
/// use std::net::IpAddr;
/// use std::time::Duration;
///
/// let peers = PeerSet::new([IpAddr::from([192, 0, 2, 11])]);
/// let bytes = highland_net::fixture_advertisement();
///
/// let accepted = validate(
///     Datagram { bytes: &bytes, source: IpAddr::from([192, 0, 2, 11]), ttl: 255 },
///     &AllowedSources::Peers(peers),
///     IpAddr::from([192, 0, 2, 11]),
///     42,
///     Duration::ZERO,
///     None,
/// );
/// assert!(matches!(accepted, Accepted::Advertisement(_)));
/// ```
#[must_use]
/// How an instance reaches its peers.
///
/// The two modes differ in more than the address list, which is why this is a
/// sum type rather than a flag: a multicast instance joins a group on its socket
/// and sends one datagram, while a unicast instance names its peers and sends one
/// datagram each. The mode also decides whether a peer list is required at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Peering {
    /// An explicit list of peer addresses. Peers of the other family are
    /// ignored, which is what makes a mixed-family list legal.
    Unicast(PeerSet),
    /// One multicast group, joined on the socket and sent to with the TTL VRRP
    /// requires.
    Multicast {
        /// The group to join and send to, which must be of the instance's family.
        group: IpAddr,
        /// The TTL, which RFC 5798 requires to be 255.
        ttl: u8,
    },
}

impl Peering {
    /// Returns the mode's name, for a diagnostic.
    #[must_use]
    pub fn mode(&self) -> &'static str {
        match self {
            Peering::Unicast(_) => "unicast",
            Peering::Multicast { .. } => "multicast",
        }
    }
}

/// Who is allowed to speak VRRP to this instance.
///
/// A sum type rather than a flag, because the two modes enforce different rules
/// and a flag would make "no peers" mean both "allow nobody" and "allow anybody".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AllowedSources {
    /// Exactly the configured peers (`I-43`). An empty set allows nothing.
    ///
    /// Owned rather than borrowed: the set is a handful of addresses, and a
    /// borrow would tie every caller's validation to the lifetime of a
    /// configuration it does not otherwise need.
    Peers(PeerSet),
    /// Any source that reached the group. In multicast mode the group membership
    /// *is* the authorisation: a node on the segment that joined the group is a
    /// node on the segment, and there is no list to be in.
    Group,
}

/// Validates one datagram against the protocol and the instance's rules.
///
/// `destination` is the address the datagram was sent to, which an IPv6 checksum
/// covers: the pseudo-header includes it, so a receiver that guessed wrong would
/// reject every valid advertisement.
///
/// # Errors
///
/// This function cannot fail; every outcome is a typed [`Accepted`].
#[must_use]
pub fn validate(
    datagram: Datagram<'_>,
    allowed: &AllowedSources,
    destination: IpAddr,
    vrid: u8,
    now: Duration,
    limiter: Option<&mut RateWindow>,
) -> Accepted {
    if let Some(limiter) = limiter
        && !limiter.admit(now)
    {
        return Accepted::Rejected(Rejection::RateLimited);
    }

    if datagram.ttl != REQUIRED_TTL {
        return Accepted::Rejected(Rejection::BadTtl);
    }
    if datagram.bytes.len() > MAX_DATAGRAM {
        return Accepted::Rejected(Rejection::TooShort);
    }

    let family = IpFamily::of(&datagram.source);
    let header = match Peek::read(datagram.bytes, family) {
        Ok(header) => header,
        Err(DecodeError::Truncated { .. }) => return Accepted::Rejected(Rejection::TooShort),
        Err(error) => return Accepted::Rejected(rejection_for(&error)),
    };
    if header.message_len != datagram.bytes.len() {
        return Accepted::Rejected(Rejection::BadLength);
    }
    if header.vrid.get() != vrid {
        return Accepted::Rejected(Rejection::WrongVrid);
    }
    if let AllowedSources::Peers(peers) = allowed
        && !peers.contains(&datagram.source)
    {
        return Accepted::Rejected(Rejection::UnknownPeer);
    }

    let scope = ChecksumScope::for_packet(family, datagram.source, destination);
    match Advertisement::decode(datagram.bytes, family, scope) {
        Ok(advertisement) => Accepted::Advertisement(AcceptedAdvertisement {
            advertisement,
            source: datagram.source,
        }),
        Err(error) => Accepted::Rejected(rejection_for(&error)),
    }
}

fn rejection_for(error: &DecodeError) -> Rejection {
    match error {
        DecodeError::ChecksumMismatch { .. } => Rejection::BadChecksum,
        DecodeError::UnexpectedPacketType { .. } => Rejection::BadType,
        DecodeError::InvalidField { field, .. } if *field == "version" => Rejection::BadVersion,
        DecodeError::InvalidField { field, .. } if *field == "type" => Rejection::BadType,
        // A length, a VRID of zero, or an interval of zero: all of them mean
        // the message does not describe itself consistently.
        DecodeError::InconsistentLength { .. } | DecodeError::InvalidField { .. } => {
            Rejection::BadLength
        }
        // The error type is `#[non_exhaustive]`, so a new variant lands as a
        // rejection rather than as a missing match arm.
        _ => Rejection::BadLength,
    }
}

/// The set of addresses allowed to speak VRRP to this instance.
///
/// An empty set is a real configuration error caught by `V-07`, not an
/// "allow everyone" default: a transport with no peers must accept nothing
/// (`I-43`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PeerSet {
    peers: Vec<IpAddr>,
}

impl PeerSet {
    /// Creates a peer set.
    #[must_use]
    pub fn new(peers: impl IntoIterator<Item = IpAddr>) -> Self {
        Self {
            peers: peers.into_iter().collect(),
        }
    }

    /// Returns `true` when `address` is a configured peer.
    #[must_use]
    pub fn contains(&self, address: &IpAddr) -> bool {
        self.peers.contains(address)
    }

    /// Returns the number of peers.
    #[must_use]
    pub fn len(&self) -> usize {
        self.peers.len()
    }

    /// Returns `true` when no peers are configured.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.peers.is_empty()
    }

    /// Returns every configured peer, in configuration order.
    #[must_use]
    pub fn list(&self) -> &[IpAddr] {
        &self.peers
    }

    /// Returns the peers of `family`, for a per-family socket.
    #[must_use]
    pub fn of_family(&self, family: IpFamily) -> Vec<IpAddr> {
        self.peers
            .iter()
            .copied()
            .filter(|peer| family.matches(peer))
            .collect()
    }
}

/// A fixed window of one second that admits at most `limit` datagrams.
///
/// A window rather than a token bucket, because the limit is a ceiling on how
/// much work a peer can create, not a pacing decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RateWindow {
    limit: u32,
    admitted: u32,
    window_start: Option<Duration>,
}

impl RateWindow {
    /// Creates a window admitting `limit` datagrams per second.
    #[must_use]
    pub fn per_second(limit: u32) -> Self {
        Self {
            limit,
            admitted: 0,
            window_start: None,
        }
    }

    /// Consumes one slot, refilling at `now`.
    pub fn admit(&mut self, now: Duration) -> bool {
        match self.window_start {
            Some(start) if now.saturating_sub(start) < Duration::from_secs(1) => {}
            _ => {
                self.window_start = Some(now);
                self.admitted = 0;
            }
        }
        if self.admitted >= self.limit {
            return false;
        }
        self.admitted += 1;
        true
    }
}

impl Default for RateWindow {
    fn default() -> Self {
        Self::per_second(DEFAULT_ACCEPT_RATE)
    }
}

/// The set of addresses an advertisement is sent to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Destinations {
    peers: Vec<IpAddr>,
}

impl Destinations {
    /// Creates a destination set.
    #[must_use]
    pub fn new(peers: impl IntoIterator<Item = IpAddr>) -> Self {
        Self {
            peers: peers.into_iter().collect(),
        }
    }

    /// Returns the destinations in send order, which is the configured order so
    /// that a packet capture is readable.
    #[must_use]
    pub fn peers(&self) -> &[IpAddr] {
        &self.peers
    }

    /// Returns `true` when there is nowhere to send.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.peers.is_empty()
    }
}

/// Builds the advertisement the transport sends for `advertisement`.
///
/// # Errors
///
/// Returns [`NetError::Encode`] when the advertisement cannot be encoded, and
/// [`NetError::Unsupported`] when the family needs a checksum scope that was not
/// supplied.
pub fn build(
    advertisement: &Advertisement,
    family: IpFamily,
    source: IpAddr,
    destination: IpAddr,
) -> Result<Vec<u8>> {
    let scope = ChecksumScope::for_packet(family, source, destination);
    advertisement
        .encode_with_checksum(family, scope)
        .map_err(|error| NetError::Encode {
            family,
            reason: error.to_string(),
        })
}

/// Returns the default advertisement interval, one second.
///
/// # Panics
///
/// Never. One second is inside the range the wire field allows, and the
/// assertion documents that.
#[must_use]
pub fn default_interval() -> MaxAdverInt {
    MaxAdverInt::from_duration(Duration::from_secs(1)).expect("one second is in range")
}

/// A small, valid IPv4 advertisement, used by the tests and the examples.
///
/// # Examples
///
/// ```
/// use highland_net::fixture_advertisement;
/// use highland_vrrp::Peek;
///
/// let bytes = fixture_advertisement();
/// assert_eq!(bytes.len(), 12);
/// assert_eq!(Peek::read(&bytes, highland_vrrp::IpFamily::V4).unwrap().vrid.get(), 42);
/// ```
///
/// # Panics
///
/// Never. Every field is a constant, and the assertions document that they are in
/// range.
#[must_use]
pub fn fixture_advertisement() -> Vec<u8> {
    let advertisement = Advertisement::new(
        highland_vrrp::Vrid::new(42).expect("42 is a valid VRID"),
        highland_vrrp::Priority::new(150).expect("150 is representable"),
        default_interval(),
        vec!["192.0.2.10".parse().expect("valid address")],
    )
    .expect("the fixture is valid");
    advertisement.encode_v4().expect("the fixture encodes")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_protocol_constants_match_the_rfc() {
        assert_eq!(VRRP_IP_PROTOCOL, 112, "RFC 5798 section 5.1.1.4");
        assert_eq!(REQUIRED_TTL, 255, "RFC 5798 section 5.1.1.3");
        assert_eq!(MAX_DATAGRAM, 4096, "a VRRP message is at most 8 + 255 * 16");
    }
}
