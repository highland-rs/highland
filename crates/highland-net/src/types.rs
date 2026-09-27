// Rust guideline compliant 2026-09-27

//! Typed wrappers around the Linux concepts Highland needs.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::str::FromStr;

/// The kernel's identifier for a network interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InterfaceId(i32);

impl InterfaceId {
    /// Creates an interface identifier from its kernel index.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidInterfaceIndex`] when `index` is negative, because the
    /// kernel reserves negative indices.
    ///
    /// [`InvalidInterfaceIndex`]: NetError
    pub fn new(index: i32) -> Result<Self, NegativeInterfaceIndex> {
        if index < 0 {
            return Err(NegativeInterfaceIndex { index });
        }
        Ok(Self(index))
    }

    /// Returns the kernel index.
    #[must_use]
    pub fn get(self) -> i32 {
        self.0
    }
}

impl fmt::Display for InterfaceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The error returned when a negative interface index is supplied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("interface index {index} is negative and therefore invalid")]
pub struct NegativeInterfaceIndex {
    /// The rejected index.
    pub index: i32,
}

/// The operating state of an interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LinkState {
    /// The interface exists and is administratively up.
    Up,
    /// The interface exists but is administratively down.
    Down,
    /// The interface is administratively up but has no carrier.
    NoCarrier,
    /// The interface is present in a state the backend cannot classify.
    Unknown,
}

/// A network interface as the daemon sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Interface {
    /// The kernel index.
    pub id: InterfaceId,
    /// The interface name, for example `eth0`.
    pub name: String,
    /// The current link state.
    pub state: LinkState,
    /// The addresses configured on the interface, in kernel order.
    pub addresses: Vec<IpCidr>,
    /// The interface MTU in bytes, or `None` when the backend does not report it.
    pub mtu: Option<u32>,
}

impl Interface {
    /// Returns the primary IPv4 address, if one is configured.
    ///
    /// The first address of the family in kernel order is treated as primary,
    /// matching the order Linux reports. This is the address compared when
    /// breaking an election tie (SPEC.md, §12.3).
    #[must_use]
    pub fn primary_ipv4(&self) -> Option<Ipv4Addr> {
        self.addresses.iter().find_map(|cidr| match cidr.address() {
            IpAddr::V4(address) => Some(address),
            IpAddr::V6(_) => None,
        })
    }

    /// Returns the primary IPv6 address, if one is configured.
    #[must_use]
    pub fn primary_ipv6(&self) -> Option<Ipv6Addr> {
        self.addresses.iter().find_map(|cidr| match cidr.address() {
            IpAddr::V6(address) => Some(address),
            IpAddr::V4(_) => None,
        })
    }

    /// Returns `true` when the interface can carry VRRP traffic.
    #[must_use]
    pub fn is_usable(&self) -> bool {
        matches!(self.state, LinkState::Up)
    }
}

/// An address with its prefix length.
///
/// # Examples
///
/// ```
/// use highland_net::IpCidr;
///
/// let cidr = IpCidr::parse("192.0.2.10/24").unwrap();
/// assert_eq!(cidr.to_string(), "192.0.2.10/24");
/// assert_eq!(cidr.prefix_len(), 24);
/// assert!(cidr.contains(&"192.0.2.99".parse().unwrap()));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct IpCidr {
    address: IpAddr,
    prefix_len: u8,
}

impl IpCidr {
    /// Creates an address and prefix pair without validation.
    #[must_use]
    pub fn new(address: IpAddr, prefix_len: u8) -> Self {
        Self {
            address,
            prefix_len,
        }
    }

    /// Parses `address/prefix` notation.
    ///
    /// # Errors
    ///
    /// Returns [`net::AddrParseError`] when the text is malformed, and
    /// returns a [`PrefixLenError`] when the prefix length is not valid for
    /// the address family.
    pub fn parse(text: &str) -> Result<Self, AddrParseError> {
        let (address, prefix) =
            text.split_once('/')
                .ok_or(AddrParseError::MissingPrefixLength {
                    input: text.to_owned(),
                })?;
        let address: IpAddr = address
            .parse()
            .map_err(|source| AddrParseError::InvalidAddress {
                input: address.to_owned(),
                source,
            })?;
        let prefix_len: u8 = prefix
            .parse()
            .map_err(|_| AddrParseError::InvalidPrefixLength {
                input: prefix.to_owned(),
            })?;
        let cidr = Self::new(address, prefix_len);
        cidr.validate()
            .map_err(|source| AddrParseError::InvalidPrefix {
                input: text.to_owned(),
                source,
            })?;
        Ok(cidr)
    }

    /// Returns the address.
    #[must_use]
    pub fn address(&self) -> IpAddr {
        self.address
    }

    /// Returns the prefix length.
    #[must_use]
    pub fn prefix_len(&self) -> u8 {
        self.prefix_len
    }

    /// Returns `true` when `other` is inside this prefix.
    #[must_use]
    pub fn contains(&self, other: &IpAddr) -> bool {
        match (self.address, other) {
            (IpAddr::V4(network), IpAddr::V4(candidate)) => {
                if self.prefix_len > 32 {
                    return false;
                }
                masked_v4(network, self.prefix_len) == masked_v4(*candidate, self.prefix_len)
            }
            (IpAddr::V6(network), IpAddr::V6(candidate)) => {
                if self.prefix_len > 128 {
                    return false;
                }
                masked_v6(network, self.prefix_len) == masked_v6(*candidate, self.prefix_len)
            }
            _ => false,
        }
    }

    fn validate(&self) -> Result<(), PrefixLenError> {
        let max = match self.address {
            IpAddr::V4(_) => 32,
            IpAddr::V6(_) => 128,
        };
        if self.prefix_len > max {
            return Err(PrefixLenError {
                prefix_len: self.prefix_len,
                max,
            });
        }
        Ok(())
    }
}

impl fmt::Display for IpCidr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.address, self.prefix_len)
    }
}

impl FromStr for IpCidr {
    type Err = AddrParseError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::parse(text)
    }
}

/// A prefix length that is invalid for its address family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("prefix length {prefix_len} is invalid for this address family (maximum {max})")]
pub struct PrefixLenError {
    /// The rejected prefix length.
    pub prefix_len: u8,
    /// The largest prefix length for the family.
    pub max: u8,
}

/// The error returned when `address/prefix` text cannot be parsed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AddrParseError {
    /// The text had no `/`.
    #[error("address {input} is missing a prefix length")]
    MissingPrefixLength {
        /// The rejected text.
        input: String,
    },

    /// The address portion was malformed.
    #[error("address {input} is invalid: {source}")]
    InvalidAddress {
        /// The rejected text.
        input: String,
        /// The underlying parse error.
        source: std::net::AddrParseError,
    },

    /// The prefix portion was malformed.
    #[error("prefix length {input} is not a number")]
    InvalidPrefixLength {
        /// The rejected text.
        input: String,
    },

    /// The prefix length was not valid for the family.
    #[error("address {input} is invalid: {source}")]
    InvalidPrefix {
        /// The rejected text.
        input: String,
        /// The underlying validation error.
        source: PrefixLenError,
    },
}

/// Returns the IPv4 network mask for a prefix length.
fn mask_v4(prefix_len: u8) -> u32 {
    if prefix_len == 0 {
        0
    } else {
        u32::MAX << (32 - u32::from(prefix_len))
    }
}

/// Returns the IPv4 address masked to `prefix_len`.
fn masked_v4(address: Ipv4Addr, prefix_len: u8) -> u32 {
    u32::from(address) & mask_v4(prefix_len)
}

/// Returns the IPv6 network mask for a prefix length.
fn mask_v6(prefix_len: u8) -> u128 {
    if prefix_len == 0 {
        0
    } else {
        u128::MAX << (128 - u32::from(prefix_len))
    }
}

/// Returns the IPv6 address masked to `prefix_len`.
fn masked_v6(address: Ipv6Addr, prefix_len: u8) -> u128 {
    u128::from(address) & mask_v6(prefix_len)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cidr(text: &str) -> IpCidr {
        IpCidr::parse(text).expect("fixture parses")
    }

    #[test]
    fn cidr_round_trips_through_text() {
        assert_eq!(cidr("192.0.2.10/24").to_string(), "192.0.2.10/24");
        assert_eq!(cidr("2001:db8::10/64").to_string(), "2001:db8::10/64");
    }

    #[test]
    fn containment_handles_full_and_zero_prefixes() {
        assert!(cidr("192.0.2.10/32").contains(&"192.0.2.10".parse().unwrap()));
        assert!(!cidr("192.0.2.10/32").contains(&"192.0.2.11".parse().unwrap()));
        assert!(cidr("0.0.0.0/0").contains(&"198.51.100.1".parse().unwrap()));
        assert!(cidr("2001:db8::/32").contains(&"2001:db8:1::5".parse().unwrap()));
    }

    #[test]
    fn containment_never_matches_across_families() {
        assert!(!cidr("192.0.2.0/24").contains(&"2001:db8::1".parse().unwrap()));
    }

    #[test]
    fn a_prefix_length_beyond_the_family_is_rejected() {
        assert!(IpCidr::parse("192.0.2.10/33").is_err());
        assert!(IpCidr::parse("2001:db8::10/129").is_err());
    }

    #[test]
    fn malformed_text_is_rejected_with_a_located_error() {
        assert!(matches!(
            IpCidr::parse("192.0.2.10"),
            Err(AddrParseError::MissingPrefixLength { .. })
        ));
        assert!(matches!(
            IpCidr::parse("nope/24"),
            Err(AddrParseError::InvalidAddress { .. })
        ));
        assert!(matches!(
            IpCidr::parse("192.0.2.10/x"),
            Err(AddrParseError::InvalidPrefixLength { .. })
        ));
    }

    #[test]
    fn negative_interface_indices_are_rejected() {
        assert_eq!(InterfaceId::new(3).map(InterfaceId::get), Ok(3));
        assert_eq!(
            InterfaceId::new(-1),
            Err(NegativeInterfaceIndex { index: -1 })
        );
    }

    #[test]
    fn only_an_up_interface_is_usable() {
        let interface = Interface {
            id: InterfaceId::new(2).expect("positive index"),
            name: "eth0".to_owned(),
            state: LinkState::Up,
            addresses: vec![cidr("192.0.2.5/24")],
            mtu: Some(1500),
        };
        assert!(interface.is_usable());
        assert_eq!(interface.primary_ipv4(), Some("192.0.2.5".parse().unwrap()));
        assert_eq!(interface.primary_ipv6(), None);

        let down = Interface {
            state: LinkState::NoCarrier,
            ..interface
        };
        assert!(!down.is_usable());
    }
}
