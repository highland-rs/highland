// Rust guideline compliant 2026-09-27

//! Validated protocol field types, as defined by RFC 5798.
//!
//! Each type here makes an invalid wire value unrepresentable, so a value that
//! exists can be encoded and a value that can be encoded can be decoded
//! (SPEC.md, §9.2, `D-04`). The bounds are the protocol's, not the
//! configuration layer's: a [`Priority`] of 255 is representable because the RFC
//! assigns it to the address owner, while a [`Vrid`] of 0 is not.
//!
//! # The wire layout
//!
//! RFC 5798 §5.1 defines one message format for both families:
//!
//! ```text
//!  0                   1                   2                   3
//!  0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |Version| Type  | Virtual Rtr ID|   Priority    |Count IPvX Addr|
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |(rsvd) |     Max Adver Int     |          Checksum             |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |                                                               |
//! +                        IPvX Address(es)                       +
//! ```
//!
//! Two details are easy to get wrong and are enforced here: the advertisement
//! interval is a **12-bit** field in centiseconds sharing an octet with a
//! 4-bit reserved field, and it is named *Max Adver Int*, not *Adver Int*,
//! because it is the slowest interval the sender will use.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;

use crate::error::{ProtocolError, Result};

/// The length of the fixed part of a VRRP message, in octets (RFC 5798 §5.1).
pub const HEADER_LEN: usize = 8;

/// The IP protocol number assigned to VRRP (RFC 5798 §5.1.1.4, §5.1.2.4).
pub const VRRP_PROTOCOL: u8 = 112;

/// The TTL and hop limit a VRRP packet MUST carry (RFC 5798 §5.1.1.3, §5.1.2.3).
pub const REQUIRED_TTL: u8 = 255;

/// The VRRP protocol version, the only one this implementation speaks.
pub const VERSION_3: u8 = 3;

/// The packet type for an advertisement (RFC 5798 §5.2.2).
pub const TYPE_ADVERTISEMENT: u8 = 1;

/// The smallest value `Max Adver Int` may take, one centisecond.
pub const MIN_MAX_ADVER_INT: u16 = 1;

/// The largest value the 12-bit `Max Adver Int` field may hold, 4095
/// centiseconds, which is 40.95 seconds.
pub const MAX_MAX_ADVER_INT: u16 = 4095;

/// The VRRP protocol version carried in the version field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[non_exhaustive]
pub enum Version {
    /// Version 3, the version RFC 5798 defines.
    #[default]
    V3,
}

impl Version {
    /// Returns the value of the version field on the wire.
    #[must_use]
    pub fn as_u8(self) -> u8 {
        VERSION_3
    }

    /// Parses a raw version field.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::UnsupportedVersion`] for any other value.
    /// A version 2 packet is not accepted: a version 3 router discards one
    /// rather than guessing at its layout (RFC 5798 §7.1).
    pub fn from_u8(raw: u8) -> Result<Self> {
        if raw == VERSION_3 {
            Ok(Version::V3)
        } else {
            Err(ProtocolError::UnsupportedVersion { version: raw })
        }
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "VRRPv{}", self.as_u8())
    }
}

/// The packet type (RFC 5798 §5.2.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[non_exhaustive]
pub enum PacketType {
    /// An advertisement, the only type this version defines.
    #[default]
    Advertisement,
}

impl PacketType {
    /// Returns the value of the type field on the wire.
    #[must_use]
    pub fn as_u8(self) -> u8 {
        TYPE_ADVERTISEMENT
    }

    /// Parses a raw type field.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::UnexpectedPacketType`] for any other value, and
    /// for the version field being something other than 3, which RFC 5798
    /// §5.1.1 requires the receiver to verify before reading further.
    pub fn from_u8(raw: u8, version: u8) -> Result<Self> {
        if version != VERSION_3 {
            return Err(ProtocolError::UnsupportedVersion { version });
        }
        if raw == TYPE_ADVERTISEMENT {
            Ok(PacketType::Advertisement)
        } else {
            Err(ProtocolError::UnexpectedPacketType { packet_type: raw })
        }
    }
}

/// A validated Virtual Router Identifier, in `1..=255` (RFC 5798 §5.2.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Vrid(u8);

impl Vrid {
    /// The lowest valid VRID.
    pub const MIN: u8 = 1;
    /// The highest valid VRID.
    pub const MAX: u8 = 255;

    /// Creates a VRID from a raw value.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::VridOutOfRange`] when `raw` is `0`.
    ///
    /// # Examples
    ///
    /// ```
    /// use highland_vrrp::Vrid;
    ///
    /// assert_eq!(Vrid::new(42).unwrap().get(), 42);
    /// assert!(Vrid::new(0).is_err());
    /// ```
    pub fn new(raw: u8) -> Result<Self> {
        if raw < Self::MIN {
            return Err(ProtocolError::VridOutOfRange {
                vrid: raw,
                min: Self::MIN,
                max: Self::MAX,
            });
        }
        Ok(Self(raw))
    }

    /// Returns the raw value.
    #[must_use]
    pub fn get(self) -> u8 {
        self.0
    }
}

impl TryFrom<u8> for Vrid {
    type Error = ProtocolError;

    fn try_from(raw: u8) -> Result<Self> {
        Self::new(raw)
    }
}

impl fmt::Display for Vrid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A validated VRRP priority, in `0..=255` (RFC 5798 §5.2.4).
///
/// Priority 255 is reserved for the router that owns the virtual router's
/// address, and 0 is the relinquishing value a master sends when it stops
/// participating. Neither is representable as an effective priority in the state
/// machine, which is a separate concern (SPEC.md, `I-23`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Priority(u8);

impl Priority {
    /// The reserved relinquishing priority.
    pub const RESERVED: u8 = 0;
    /// The priority assigned to the address owner.
    pub const ADDRESS_OWNER: u8 = 255;
    /// The lowest priority a backing-up router may be configured with.
    pub const MIN_CONFIGURABLE: u8 = 1;
    /// The highest priority a backing-up router may be configured with.
    pub const MAX_CONFIGURABLE: u8 = 254;
    /// The default priority for a backing-up router.
    pub const DEFAULT: u8 = 100;

    /// Creates a priority from a raw value.
    ///
    /// Every `u8` is representable, so this cannot fail; it exists so that a
    /// wider wire type would be validated in one place.
    ///
    /// # Errors
    ///
    /// Never returns an error today. The signature is fallible so that widening
    /// the wire type later is not a breaking change.
    pub fn new(raw: u8) -> Result<Self> {
        Ok(Self(raw))
    }

    /// Returns the raw value.
    #[must_use]
    pub fn get(self) -> u8 {
        self.0
    }

    /// Returns `true` when this is the reserved relinquishing priority.
    #[must_use]
    pub fn is_relinquish(self) -> bool {
        self.0 == Self::RESERVED
    }

    /// Returns `true` when this priority identifies the address owner.
    #[must_use]
    pub fn is_address_owner(self) -> bool {
        self.0 == Self::ADDRESS_OWNER
    }

    /// Returns `true` when a router may be configured with this priority.
    #[must_use]
    pub fn is_configurable(self) -> bool {
        (Self::MIN_CONFIGURABLE..=Self::MAX_CONFIGURABLE).contains(&self.0)
    }
}

impl TryFrom<u8> for Priority {
    type Error = ProtocolError;

    fn try_from(raw: u8) -> Result<Self> {
        Self::new(raw)
    }
}

impl fmt::Display for Priority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The 12-bit `Max Adver Int` field, in centiseconds (RFC 5798 §5.2.7).
///
/// The field is the *maximum* interval the sender will use, not the interval it
/// happens to be using, which is what makes a peer's rate discoverable before
/// its first advertisement is trusted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MaxAdverInt(u16);

impl MaxAdverInt {
    /// Creates the field from a count of centiseconds.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::AdverIntOutOfRange`] outside
    /// `1..=4095`, which is the range the 12-bit field allows once zero is
    /// excluded.
    pub fn from_centiseconds(centiseconds: u16) -> Result<Self> {
        if !(MIN_MAX_ADVER_INT..=MAX_MAX_ADVER_INT).contains(&centiseconds) {
            return Err(ProtocolError::AdverIntOutOfRange {
                centiseconds,
                min: MIN_MAX_ADVER_INT,
                max: MAX_MAX_ADVER_INT,
            });
        }
        Ok(Self(centiseconds))
    }

    /// Creates the field from a duration, rounding down to whole centiseconds.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::AdverIntOutOfRange`] when the duration is zero,
    /// shorter than a centisecond, or longer than 40.95 seconds.
    pub fn from_duration(duration: Duration) -> Result<Self> {
        // Centiseconds are computed from nanoseconds so that a duration such as
        // 1.5s does not lose precision to a millisecond truncation first.
        let centiseconds = u16::try_from(duration.as_nanos() / 10_000_000).map_err(|_| {
            ProtocolError::AdverIntOutOfRange {
                centiseconds: MAX_MAX_ADVER_INT.saturating_add(1),
                min: MIN_MAX_ADVER_INT,
                max: MAX_MAX_ADVER_INT,
            }
        })?;
        Self::from_centiseconds(centiseconds)
    }

    /// Returns the value in centiseconds.
    #[must_use]
    pub fn centiseconds(self) -> u16 {
        self.0
    }

    /// Returns the value as a duration.
    #[must_use]
    pub fn as_duration(self) -> Duration {
        Duration::from_millis(u64::from(self.0) * 10)
    }
}

impl TryFrom<Duration> for MaxAdverInt {
    type Error = ProtocolError;

    fn try_from(duration: Duration) -> Result<Self> {
        Self::from_duration(duration)
    }
}

impl fmt::Display for MaxAdverInt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}cs", self.0)
    }
}

/// The address family an advertisement belongs to.
///
/// The family is an explicit parameter of every encode and decode call, so that
/// a mixed-family configuration cannot silently produce a packet for the wrong
/// family. RFC 5798 §5.2.9 requires that one message never carries both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[non_exhaustive]
pub enum IpFamily {
    /// IPv4, advertised to `224.0.0.18`.
    #[default]
    V4,
    /// IPv6, advertised to `ff02::12`.
    V6,
}

impl IpFamily {
    /// Returns the number of octets in one address of this family.
    #[must_use]
    pub fn octets(self) -> usize {
        match self {
            IpFamily::V4 => 4,
            IpFamily::V6 => 16,
        }
    }

    /// Returns the length of a message carrying `count` addresses.
    #[must_use]
    pub fn message_len(self, count: u8) -> usize {
        HEADER_LEN + usize::from(count) * self.octets()
    }

    /// Returns the default multicast group for this family.
    #[must_use]
    pub fn default_group(self) -> IpAddr {
        match self {
            IpFamily::V4 => IpAddr::V4(Ipv4Addr::new(224, 0, 0, 18)),
            IpFamily::V6 => IpAddr::V6(Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 0, 0x0012)),
        }
    }

    /// Returns `true` when `address` belongs to this family.
    #[must_use]
    pub fn matches(self, address: &IpAddr) -> bool {
        match (self, address) {
            (IpFamily::V4, IpAddr::V4(_)) | (IpFamily::V6, IpAddr::V6(_)) => true,
            (IpFamily::V4 | IpFamily::V6, IpAddr::V4(_) | IpAddr::V6(_)) => false,
        }
    }

    /// Returns the family of `address`.
    #[must_use]
    pub fn of(address: &IpAddr) -> Self {
        match address {
            IpAddr::V4(_) => IpFamily::V4,
            IpAddr::V6(_) => IpFamily::V6,
        }
    }
}

impl fmt::Display for IpFamily {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            IpFamily::V4 => "IPv4",
            IpFamily::V6 => "IPv6",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_version_is_three_and_nothing_else() {
        assert_eq!(Version::from_u8(3), Ok(Version::V3));
        assert_eq!(Version::default().as_u8(), 3);
        for raw in [0u8, 1, 2, 4, 255] {
            assert!(matches!(
                Version::from_u8(raw),
                Err(ProtocolError::UnsupportedVersion { .. })
            ));
        }
    }

    #[test]
    fn a_type_is_readable_only_under_version_three() {
        assert_eq!(PacketType::from_u8(1, 3), Ok(PacketType::Advertisement));
        assert!(matches!(
            PacketType::from_u8(1, 2),
            Err(ProtocolError::UnsupportedVersion { version: 2 })
        ));
        assert!(matches!(
            PacketType::from_u8(2, 3),
            Err(ProtocolError::UnexpectedPacketType { packet_type: 2 })
        ));
    }

    #[test]
    fn a_vrid_of_zero_is_not_representable() {
        assert_eq!(
            Vrid::try_from(0),
            Err(ProtocolError::VridOutOfRange {
                vrid: 0,
                min: 1,
                max: 255
            })
        );
        assert_eq!(Vrid::try_from(255), Ok(Vrid(255)));
    }

    #[test]
    fn the_reserved_and_owner_priorities_are_distinguishable() {
        assert!(
            Priority::new(0)
                .expect("zero is representable")
                .is_relinquish()
        );
        assert!(
            Priority::new(255)
                .expect("255 is representable")
                .is_address_owner()
        );
        assert!(Priority::new(150).expect("representable").is_configurable());
        assert!(!Priority::new(255).expect("representable").is_configurable());
        assert!(!Priority::new(0).expect("representable").is_configurable());
    }

    #[test]
    fn the_advertisement_interval_is_a_twelve_bit_centisecond_field() {
        assert_eq!(
            MaxAdverInt::from_centiseconds(100).unwrap().centiseconds(),
            100
        );
        assert_eq!(
            MaxAdverInt::from_centiseconds(4095).unwrap().as_duration(),
            Duration::from_millis(40_950)
        );
        assert!(MaxAdverInt::from_centiseconds(0).is_err());
        assert!(MaxAdverInt::from_centiseconds(4096).is_err());
    }

    #[test]
    fn an_interval_below_a_centisecond_is_rejected_rather_than_rounded_up() {
        assert!(MaxAdverInt::from_duration(Duration::from_micros(9_999)).is_err());
        assert_eq!(
            MaxAdverInt::from_duration(Duration::from_micros(10_999))
                .unwrap()
                .centiseconds(),
            1,
            "a duration truncates to whole centiseconds"
        );
        assert_eq!(
            MaxAdverInt::from_duration(Duration::from_millis(1_500))
                .unwrap()
                .centiseconds(),
            150,
            "sub-millisecond precision is not lost to an intermediate truncation"
        );
    }

    #[test]
    fn the_default_interval_is_one_second() {
        assert_eq!(
            MaxAdverInt::from_duration(Duration::from_secs(1))
                .unwrap()
                .centiseconds(),
            100
        );
    }

    #[test]
    fn an_interval_above_the_field_width_is_rejected() {
        assert!(MaxAdverInt::from_duration(Duration::from_secs(41)).is_err());
        assert!(MaxAdverInt::from_duration(Duration::from_secs(3600)).is_err());
    }

    #[test]
    fn message_length_follows_the_family() {
        assert_eq!(IpFamily::V4.message_len(1), 12);
        assert_eq!(IpFamily::V4.message_len(2), 16);
        assert_eq!(IpFamily::V6.message_len(1), 24);
        assert_eq!(IpFamily::V6.message_len(2), 40);
    }

    #[test]
    fn families_carry_their_multicast_group_and_never_match_across() {
        assert_eq!(IpFamily::V4.default_group().to_string(), "224.0.0.18");
        assert_eq!(IpFamily::V6.default_group().to_string(), "ff02::12");
        let v4: IpAddr = "192.0.2.10".parse().expect("valid address");
        let v6: IpAddr = "2001:db8::10".parse().expect("valid address");
        assert!(IpFamily::V4.matches(&v4));
        assert!(!IpFamily::V4.matches(&v6));
        assert_eq!(IpFamily::of(&v6), IpFamily::V6);
    }

    #[test]
    fn the_protocol_constants_match_the_rfc() {
        assert_eq!(HEADER_LEN, 8);
        assert_eq!(VRRP_PROTOCOL, 112);
        assert_eq!(REQUIRED_TTL, 255);
    }
}
