// Rust guideline compliant 2026-09-27

//! Validated protocol field types.
//!
//! Each type in this module makes an invalid wire value unrepresentable
//! (SPEC.md, `D-04`). The bounds are the protocol's, not the configuration
//! layer's: for example a [`Priority`] of 255 is representable here because
//! RFC 5798 assigns it to the address owner, while a [`Vrid`] of 0 is not.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use crate::error::{ProtocolError, Result};

/// The VRRP protocol version carried in the version field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[non_exhaustive]
pub enum Version {
    /// Version 2, accepted for compatibility but never emitted.
    V2,
    /// Version 3, the only version Highland emits.
    #[default]
    V3,
}

impl Version {
    /// Returns the value of the version field on the wire.
    #[must_use]
    pub fn as_u8(self) -> u8 {
        match self {
            Version::V2 => 2,
            Version::V3 => 3,
        }
    }

    /// Parses a raw version field.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::UnsupportedVersion`] for any other value.
    pub fn from_u8(raw: u8) -> Result<Self> {
        match raw {
            2 => Ok(Version::V2),
            3 => Ok(Version::V3),
            other => Err(ProtocolError::UnsupportedVersion { version: other }),
        }
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "VRRPv{}", self.as_u8())
    }
}

/// A validated Virtual Router Identifier, in `1..=255`.
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

/// A validated VRRP priority, in `0..=255`.
///
/// Priority `0` is reserved by RFC 5798 for a relinquishing router. Highland
/// never transmits it as a normal advertisement and never uses it as an
/// effective priority (SPEC.md, `I-23`, `I-27`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Priority(u8);

impl Priority {
    /// The reserved relinquishing priority.
    pub const RESERVED: u8 = 0;
    /// The priority assigned to the address owner.
    pub const ADDRESS_OWNER: u8 = 255;
    /// The lowest priority a node may be configured with.
    pub const MIN_CONFIGURABLE: u8 = 1;

    /// Creates a priority from a raw value.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::PriorityOutOfRange`] when `raw` is above 255,
    /// which is unreachable for a `u8` input. The check exists so that a wider
    /// wire type can be validated in one place later.
    pub fn new(raw: u8) -> Result<Self> {
        // Every `u8` fits in 0..=255, so this cannot fail. The documented
        // range is kept in one place for a future wider wire type.
        Ok(Self(raw))
    }

    /// Returns the raw value.
    #[must_use]
    pub fn get(self) -> u8 {
        self.0
    }

    /// Returns `true` when this is the reserved relinquishing priority.
    #[must_use]
    pub fn is_reserved(self) -> bool {
        self.0 == Self::RESERVED
    }

    /// Returns `true` when this priority identifies the address owner.
    #[must_use]
    pub fn is_address_owner(self) -> bool {
        self.0 == Self::ADDRESS_OWNER
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

/// The address family an advertisement belongs to.
///
/// The family is an explicit parameter of every encode and decode call
/// (SPEC.md, `R-06`) so that a mixed-family configuration cannot silently
/// produce a packet for the wrong family.
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
        matches!(
            (self, address),
            (IpFamily::V4, IpAddr::V4(_)) | (IpFamily::V6, IpAddr::V6(_))
        )
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
    fn versions_round_trip() {
        assert_eq!(Version::from_u8(3), Ok(Version::V3));
        assert_eq!(Version::default(), Version::V3);
        assert_eq!(Version::V3.as_u8(), 3);
        assert_eq!(
            Version::from_u8(4),
            Err(ProtocolError::UnsupportedVersion { version: 4 })
        );
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
        assert!(Priority::new(0).expect("0 is representable").is_reserved());
        assert!(
            Priority::new(255)
                .expect("255 is representable")
                .is_address_owner()
        );
        assert!(
            !Priority::new(150)
                .expect("150 is representable")
                .is_reserved()
        );
    }

    #[test]
    fn families_carry_their_default_multicast_group() {
        assert_eq!(IpFamily::V4.default_group().to_string(), "224.0.0.18");
        assert_eq!(IpFamily::V6.default_group().to_string(), "ff02::12");
        assert_eq!(IpFamily::V4.octets(), 4);
        assert_eq!(IpFamily::V6.octets(), 16);
    }

    #[test]
    fn family_membership_is_explicit() {
        let v4: IpAddr = "192.0.2.10".parse().expect("literal is a valid address");
        let v6: IpAddr = "2001:db8::10".parse().expect("literal is a valid address");

        assert!(IpFamily::V4.matches(&v4));
        assert!(!IpFamily::V4.matches(&v6));
        assert_eq!(IpFamily::of(&v6), IpFamily::V6);
    }
}
