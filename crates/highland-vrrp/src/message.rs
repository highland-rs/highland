// Rust guideline compliant 2026-09-27

//! Encoding and decoding a VRRP advertisement.
//!
//! # The wire layout
//!
//! RFC 5798 §5.1 defines one message format for both address families:
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
//! Note that the advertisement interval is a 12-bit field sharing an octet with
//! the 4-bit reserved field, and that the field is called *Max Adver Int*,
//! because it carries the slowest interval the sender will use.
//!
//! # Two-phase decoding
//!
//! A checksum cannot be verified before the header has been read, and a header
//! cannot be trusted before the checksum has been verified. [`decode`] therefore
//! runs in two phases: [`Peek`] reads the fixed header so the caller can check
//! the TTL, the source address, and the peer list first, and
//! [`decode_verified`] then verifies the checksum and decodes the addresses. A
//! caller that skips the first phase is trusting the length field of an
//! unauthenticated packet, which is how a decoder is made to allocate.

use std::fmt;
use std::net::IpAddr;
use std::time::Duration;

use thiserror::Error;

use crate::checksum::{ChecksumScope, checksum, verify};
use crate::error::{DecodeError, EncodeError, ProtocolError};
use crate::types::{
    HEADER_LEN, IpFamily, MaxAdverInt, PacketType, Priority, REQUIRED_TTL, Version, Vrid,
};

/// The offset of the checksum field within a VRRP message.
pub const CHECKSUM_OFFSET: usize = 6;

/// The largest number of addresses one message may carry, because the count is
/// an 8-bit field (RFC 5798 §5.2.5).
pub const MAX_ADDRESSES: usize = 255;

/// A VRRP advertisement, validated on construction.
///
/// # Examples
///
/// ```
/// use highland_vrrp::{Advertisement, IpFamily, MaxAdverInt, Priority, Vrid};
/// use std::time::Duration;
///
/// let advertisement = Advertisement::new(
///     Vrid::new(42).expect("42 is valid"),
///     Priority::new(150).expect("150 is representable"),
///     MaxAdverInt::from_duration(Duration::from_secs(1)).expect("1s fits"),
///     vec!["192.0.2.10".parse().expect("valid address")],
/// )?;
///
/// let bytes = advertisement.encode_v4()?;
/// assert_eq!(bytes.len(), 12);
/// assert_eq!(Advertisement::decode_verified(&bytes, IpFamily::V4)?.vrid().get(), 42);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Advertisement {
    vrid: Vrid,
    priority: Priority,
    max_adver_int: MaxAdverInt,
    addresses: Vec<IpAddr>,
}

impl Advertisement {
    /// Creates an advertisement.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::AddressFamilyMismatch`] when the addresses are
    /// mixed or empty, and [`EncodeError::TooManyAddresses`] above
    /// [`MAX_ADDRESSES`].
    pub fn new(
        vrid: Vrid,
        priority: Priority,
        max_adver_int: MaxAdverInt,
        addresses: Vec<IpAddr>,
    ) -> std::result::Result<Self, AdvertisementError> {
        if addresses.is_empty() {
            return Err(AdvertisementError::NoAddresses);
        }
        if addresses.len() > MAX_ADDRESSES {
            return Err(AdvertisementError::TooManyAddresses {
                count: addresses.len(),
            });
        }
        let family = IpFamily::of(&addresses[0]);
        if let Some(offending) = addresses.iter().find(|address| !family.matches(address)) {
            return Err(AdvertisementError::MixedFamilies {
                address: offending.to_string(),
                expected: family,
            });
        }
        Ok(Self {
            vrid,
            priority,
            max_adver_int,
            addresses,
        })
    }

    /// Returns the VRID.
    #[must_use]
    pub fn vrid(&self) -> Vrid {
        self.vrid
    }

    /// Returns the announced priority.
    #[must_use]
    pub fn priority(&self) -> Priority {
        self.priority
    }

    /// Returns the announced maximum advertisement interval.
    #[must_use]
    pub fn max_adver_int(&self) -> MaxAdverInt {
        self.max_adver_int
    }

    /// Returns the announced maximum advertisement interval as a duration.
    #[must_use]
    pub fn advert_interval(&self) -> Duration {
        self.max_adver_int.as_duration()
    }

    /// Returns the advertised addresses, in the order they are sent.
    ///
    /// RFC 5798 §5.2.9 recommends that all routers send their addresses in the
    /// same order so that the list can be compared for misconfiguration.
    #[must_use]
    pub fn addresses(&self) -> &[IpAddr] {
        &self.addresses
    }

    /// Returns the family this advertisement belongs to.
    #[must_use]
    pub fn family(&self) -> IpFamily {
        IpFamily::of(&self.addresses[0])
    }

    /// Encodes the advertisement for IPv4, whose checksum covers the message
    /// alone.
    ///
    /// # Errors
    ///
    /// Returns [`EncodeError::FamilyMismatch`] when the addresses are not IPv4.
    pub fn encode_v4(&self) -> std::result::Result<Vec<u8>, EncodeError> {
        self.encode_body(IpFamily::V4, ChecksumScope::MessageOnly)
    }

    /// Encodes the advertisement and computes its checksum under `scope`.
    ///
    /// IPv6 needs [`ChecksumScope::PseudoHeader`], because the RFC 2460
    /// pseudo-header covers the packet's source and destination addresses and
    /// IPv6 has no header checksum of its own. Producing a value without them
    /// would produce one that fails to interoperate, so the scope is a required
    /// argument rather than a default.
    ///
    /// # Errors
    ///
    /// Returns [`EncodeError::FamilyMismatch`] for a family mismatch, and
    /// [`EncodeError::UndecidableChecksum`] when the scope needs addresses that
    /// were not supplied.
    pub fn encode_with_checksum(
        &self,
        family: IpFamily,
        scope: ChecksumScope,
    ) -> std::result::Result<Vec<u8>, EncodeError> {
        self.encode_body(family, scope)
    }

    fn encode_body(
        &self,
        family: IpFamily,
        scope: ChecksumScope,
    ) -> std::result::Result<Vec<u8>, EncodeError> {
        if self.family() != family {
            return Err(EncodeError::FamilyMismatch {
                expected: family,
                found: self.family(),
            });
        }
        let count =
            u8::try_from(self.addresses.len()).map_err(|_| EncodeError::TooManyAddresses {
                count: self.addresses.len(),
            })?;

        let mut bytes = Vec::with_capacity(family.message_len(count));
        bytes.push((Version::V3.as_u8() << 4) | (PacketType::Advertisement.as_u8() & 0x0f));
        bytes.push(self.vrid.get());
        bytes.push(self.priority.get());
        bytes.push(count);
        // The reserved nibble is transmitted as zero (RFC 5798 §5.2.6) and the
        // 12-bit interval fills the rest of the octet.
        bytes.push(((self.max_adver_int.centiseconds() >> 8) as u8) & 0x0f);
        bytes.push((self.max_adver_int.centiseconds() & 0xff) as u8);
        bytes.extend_from_slice(&[0, 0]);
        for address in &self.addresses {
            match address {
                IpAddr::V4(address) => bytes.extend_from_slice(&address.octets()),
                IpAddr::V6(address) => bytes.extend_from_slice(&address.octets()),
            }
        }

        let computed = checksum(&bytes, CHECKSUM_OFFSET, scope)
            .map_err(|_| EncodeError::UndecidableChecksum { family })?;
        bytes[CHECKSUM_OFFSET..CHECKSUM_OFFSET + 2].copy_from_slice(&computed.to_be_bytes());
        Ok(bytes)
    }

    /// Decodes an advertisement whose checksum has already been verified.
    ///
    /// # Errors
    ///
    /// Returns a [`DecodeError`] for every rejection defined in RFC 5798 §7.1:
    /// an unsupported version, an unknown type, a truncated message, a VRID of
    /// zero, a count that disagrees with the length, an address of the wrong
    /// family, or a missing checksum.
    pub fn decode_verified(
        bytes: &[u8],
        family: IpFamily,
    ) -> std::result::Result<Self, DecodeError> {
        Self::decode_body(bytes, family, None)
    }

    /// Decodes an advertisement and verifies its checksum under `scope`.
    ///
    /// # Errors
    ///
    /// Returns every [`DecodeError`], including a checksum mismatch. Prefer
    /// [`Peek`] and [`Advertisement::decode_verified`] when the caller has checks
    /// to make before trusting the length.
    pub fn decode(
        bytes: &[u8],
        family: IpFamily,
        scope: ChecksumScope,
    ) -> std::result::Result<Self, DecodeError> {
        Self::decode_body(bytes, family, Some(scope))
    }

    fn decode_body(
        bytes: &[u8],
        family: IpFamily,
        scope: Option<ChecksumScope>,
    ) -> std::result::Result<Self, DecodeError> {
        if bytes.len() < HEADER_LEN {
            return Err(DecodeError::Truncated {
                needed: HEADER_LEN,
                available: bytes.len(),
                offset: 0,
            });
        }

        let raw_version = bytes[0] >> 4;
        let version =
            Version::from_u8(raw_version).map_err(|error| field_error("version", error))?;
        let _ = version;
        let raw_type = bytes[0] & 0x0f;
        let _ = PacketType::from_u8(raw_type, raw_version)
            .map_err(|error| field_error("type", error))?;

        let vrid = Vrid::new(bytes[1]).map_err(|error| field_error("vrid", error))?;
        let priority = Priority::new(bytes[2]).map_err(|error| field_error("priority", error))?;
        let count = bytes[3];

        if count == 0 {
            // RFC 5798 §5.2.5: the minimum value is 1.
            return Err(DecodeError::InvalidField {
                field: "count",
                reason: "the count of addresses must be at least 1".to_owned(),
            });
        }
        // The reserved nibble is ignored on reception (RFC 5798 §5.2.6), so a
        // non-zero value is not a reason to reject the packet.

        let max_adver_int =
            MaxAdverInt::from_centiseconds(u16::from(bytes[4] & 0x0f) << 8 | u16::from(bytes[5]))
                .map_err(|error| field_error("max_adver_int", error))?;

        let expected_len = family.message_len(count);
        if bytes.len() != expected_len {
            return Err(DecodeError::InconsistentLength {
                declared: expected_len,
                actual: bytes.len(),
                count,
                family,
            });
        }

        if let Some(scope) = scope
            && !verify(bytes, CHECKSUM_OFFSET, scope)
        {
            return Err(DecodeError::ChecksumMismatch { family });
        }

        let mut addresses = Vec::with_capacity(usize::from(count));
        let width = family.octets();
        for index in 0..usize::from(count) {
            let start = HEADER_LEN + index * width;
            let slice = &bytes[start..start + width];
            let address = match family {
                IpFamily::V4 => {
                    let octets: [u8; 4] = slice.try_into().map_err(|_| DecodeError::Truncated {
                        needed: width,
                        available: slice.len(),
                        offset: start,
                    })?;
                    IpAddr::from(octets)
                }
                IpFamily::V6 => {
                    let octets: [u8; 16] =
                        slice.try_into().map_err(|_| DecodeError::Truncated {
                            needed: width,
                            available: slice.len(),
                            offset: start,
                        })?;
                    IpAddr::from(octets)
                }
            };
            addresses.push(address);
        }

        Self::new(vrid, priority, max_adver_int, addresses).map_err(|error| {
            DecodeError::InvalidField {
                field: "addresses",
                reason: error.to_string(),
            }
        })
    }
}

/// The fixed header of a VRRP message, read before anything is trusted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Peek {
    /// The protocol version found in the first octet.
    pub version: u8,
    /// The packet type found in the first octet.
    pub packet_type: u8,
    /// The VRID.
    pub vrid: Vrid,
    /// The announced priority.
    pub priority: Priority,
    /// The number of addresses the message claims to carry.
    pub count: u8,
    /// The announced maximum advertisement interval.
    pub max_adver_int: MaxAdverInt,
    /// The message length implied by the count and the family.
    pub message_len: usize,
}

impl Peek {
    /// Reads the fixed header of `bytes` for `family`.
    ///
    /// # Errors
    ///
    /// Returns a [`DecodeError`] if the message is shorter than the fixed
    /// header, or if the version, type, VRID, or interval is invalid. It does
    /// **not** trust the count to be within the buffer, which is what makes it
    /// safe to call on an unverified packet.
    pub fn read(bytes: &[u8], family: IpFamily) -> std::result::Result<Self, DecodeError> {
        if bytes.len() < HEADER_LEN {
            return Err(DecodeError::Truncated {
                needed: HEADER_LEN,
                available: bytes.len(),
                offset: 0,
            });
        }
        let raw_version = bytes[0] >> 4;
        Version::from_u8(raw_version).map_err(|error| field_error("version", error))?;
        let raw_type = bytes[0] & 0x0f;
        PacketType::from_u8(raw_type, raw_version).map_err(|error| field_error("type", error))?;

        let vrid = Vrid::new(bytes[1]).map_err(|error| field_error("vrid", error))?;
        let priority = Priority::new(bytes[2]).map_err(|error| field_error("priority", error))?;
        let count = bytes[3];
        if count == 0 {
            return Err(DecodeError::InvalidField {
                field: "count",
                reason: "the count of addresses must be at least 1".to_owned(),
            });
        }
        let max_adver_int =
            MaxAdverInt::from_centiseconds(u16::from(bytes[4] & 0x0f) << 8 | u16::from(bytes[5]))
                .map_err(|error| field_error("max_adver_int", error))?;

        Ok(Self {
            version: raw_version,
            packet_type: raw_type,
            vrid,
            priority,
            count,
            max_adver_int,
            message_len: family.message_len(count),
        })
    }

    /// Returns `true` when `ttl` is the value RFC 5798 requires.
    #[must_use]
    pub fn ttl_is_valid(ttl: u8) -> bool {
        ttl == REQUIRED_TTL
    }
}

/// An error raised while constructing an [`Advertisement`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum AdvertisementError {
    /// The advertisement carried no addresses.
    #[error("an advertisement must carry at least one address")]
    NoAddresses,

    /// The advertisement carried more addresses than the count field can hold.
    #[error("an advertisement carries at most {MAX_ADDRESSES} addresses, not {count}")]
    TooManyAddresses {
        /// The number of addresses supplied.
        count: usize,
    },

    /// The addresses were not all of one family.
    #[error("address {address} is not an {expected} address")]
    MixedFamilies {
        /// The offending address.
        address: String,
        /// The family of the first address.
        expected: IpFamily,
    },
}

impl From<AdvertisementError> for ProtocolError {
    fn from(error: AdvertisementError) -> Self {
        match error {
            AdvertisementError::NoAddresses => ProtocolError::AddressFamilyMismatch {
                address: String::new(),
                expected: IpFamily::V4,
            },
            AdvertisementError::TooManyAddresses { count } => {
                ProtocolError::AddressCountOutOfRange {
                    count,
                    min: 1,
                    max: u16::try_from(MAX_ADDRESSES).unwrap_or(u16::MAX),
                }
            }
            AdvertisementError::MixedFamilies { address, expected } => {
                ProtocolError::AddressFamilyMismatch { address, expected }
            }
        }
    }
}

fn field_error(field: &'static str, error: impl fmt::Display) -> DecodeError {
    DecodeError::InvalidField {
        field,
        reason: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v4_advertisement() -> Advertisement {
        Advertisement::new(
            Vrid::new(42).expect("42 is valid"),
            Priority::new(150).expect("150 is representable"),
            MaxAdverInt::from_duration(Duration::from_secs(1)).expect("1s fits"),
            vec!["192.0.2.10".parse().expect("valid address")],
        )
        .expect("the fixture is valid")
    }

    #[test]
    fn the_ipv4_layout_is_exactly_the_rfc_field_by_field() {
        let bytes = v4_advertisement().encode_v4().expect("encodes");

        assert_eq!(
            bytes.len(),
            12,
            "RFC 5798 section 5.1: 8 fixed octets plus one address"
        );
        assert_eq!(bytes[0], 0x31, "version 3, type 1");
        assert_eq!(bytes[1], 42, "vrid");
        assert_eq!(bytes[2], 150, "priority");
        assert_eq!(bytes[3], 1, "count");
        assert_eq!(bytes[4], 0x00, "the reserved nibble is transmitted as zero");
        assert_eq!(bytes[5], 100, "max adver int, low octet: 100 centiseconds");
        assert_eq!(&bytes[8..12], &[192, 0, 2, 10], "the address");
    }

    #[test]
    fn a_twelve_bit_interval_spills_into_both_octets() {
        let advertisement = Advertisement::new(
            Vrid::new(1).expect("valid"),
            Priority::new(100).expect("representable"),
            MaxAdverInt::from_centiseconds(4095).expect("the field maximum"),
            vec!["192.0.2.10".parse().expect("valid address")],
        )
        .expect("valid");
        let bytes = advertisement.encode_v4().expect("encodes");

        assert_eq!(bytes[4], 0x0f, "the top four bits of the interval");
        assert_eq!(bytes[5], 0xff, "the low eight bits");
    }

    #[test]
    fn the_ipv6_layout_carries_sixteen_octet_addresses() {
        let advertisement = Advertisement::new(
            Vrid::new(1).expect("valid"),
            Priority::new(255).expect("representable"),
            MaxAdverInt::from_duration(Duration::from_secs(1)).expect("1s fits"),
            vec!["fe80::1".parse().expect("valid address")],
        )
        .expect("valid");
        let bytes = advertisement
            .encode_with_checksum(
                IpFamily::V6,
                ChecksumScope::PseudoHeader {
                    source: "fe80::a".parse().expect("valid address"),
                    destination: "ff02::12".parse().expect("valid address"),
                },
            )
            .expect("encodes");

        assert_eq!(bytes.len(), 24);
        assert_eq!(bytes[2], 255, "the address owner priority");
        let expected: std::net::Ipv6Addr = "fe80::1".parse().expect("valid address");
        assert_eq!(&bytes[8..24], &expected.octets()[..]);
    }

    #[test]
    fn several_addresses_are_encoded_in_order() {
        let advertisement = Advertisement::new(
            Vrid::new(7).expect("valid"),
            Priority::new(100).expect("representable"),
            MaxAdverInt::from_duration(Duration::from_secs(1)).expect("1s fits"),
            vec![
                "192.0.2.10".parse().expect("valid address"),
                "192.0.2.11".parse().expect("valid address"),
            ],
        )
        .expect("valid");
        let bytes = advertisement.encode_v4().expect("encodes");

        assert_eq!(bytes[3], 2, "count");
        assert_eq!(bytes.len(), 16);
        assert_eq!(&bytes[8..12], &[192, 0, 2, 10]);
        assert_eq!(&bytes[12..16], &[192, 0, 2, 11]);
    }

    #[test]
    fn an_advertisement_round_trips() {
        let original = v4_advertisement();
        let bytes = original.encode_v4().expect("encodes");
        let decoded = Advertisement::decode_verified(&bytes, IpFamily::V4).expect("decodes");

        assert_eq!(decoded.vrid(), original.vrid());
        assert_eq!(decoded.priority(), original.priority());
        assert_eq!(decoded.max_adver_int(), original.max_adver_int());
        assert_eq!(decoded.addresses(), original.addresses());
    }

    #[test]
    fn encoding_is_byte_stable() {
        let bytes = v4_advertisement().encode_v4().expect("encodes");
        let decoded = Advertisement::decode_verified(&bytes, IpFamily::V4).expect("decodes");
        assert_eq!(decoded.encode_v4().expect("re-encodes"), bytes);
    }

    #[test]
    fn the_default_interval_is_one_hundred_centiseconds() {
        let bytes = v4_advertisement().encode_v4().expect("encodes");
        assert_eq!(u16::from(bytes[4] & 0x0f) << 8 | u16::from(bytes[5]), 100);
    }

    #[test]
    fn a_zero_interval_is_rejected_on_construction_and_on_decoding() {
        assert!(MaxAdverInt::from_centiseconds(0).is_err());

        let mut bytes = v4_advertisement().encode_v4().expect("encodes");
        bytes[4] = 0;
        bytes[5] = 0;
        let error = Advertisement::decode_verified(&bytes, IpFamily::V4).expect_err("rejected");
        assert!(matches!(
            error,
            DecodeError::InvalidField {
                field: "max_adver_int",
                ..
            }
        ));
    }

    #[test]
    fn a_truncated_message_is_rejected() {
        let bytes = v4_advertisement().encode_v4().expect("encodes");
        for length in 0..bytes.len() {
            let error = Advertisement::decode_verified(&bytes[..length], IpFamily::V4)
                .expect_err("rejected");
            assert!(
                matches!(
                    error,
                    DecodeError::Truncated { .. } | DecodeError::InconsistentLength { .. }
                ),
                "length {length} produced {error:?}"
            );
        }
    }

    #[test]
    fn a_trailing_octet_is_rejected() {
        let mut bytes = v4_advertisement().encode_v4().expect("encodes");
        bytes.push(0);
        let error = Advertisement::decode_verified(&bytes, IpFamily::V4).expect_err("rejected");
        assert!(matches!(
            error,
            DecodeError::InconsistentLength {
                declared: 12,
                actual: 13,
                ..
            }
        ));
    }

    #[test]
    fn an_unsupported_version_is_rejected() {
        let mut bytes = v4_advertisement().encode_v4().expect("encodes");
        bytes[0] = 0x21;
        let error = Advertisement::decode_verified(&bytes, IpFamily::V4).expect_err("rejected");
        assert!(matches!(
            error,
            DecodeError::InvalidField {
                field: "version",
                ..
            }
        ));
    }

    #[test]
    fn an_unknown_type_is_rejected() {
        let mut bytes = v4_advertisement().encode_v4().expect("encodes");
        bytes[0] = 0x32;
        let error = Advertisement::decode_verified(&bytes, IpFamily::V4).expect_err("rejected");
        assert!(matches!(
            error,
            DecodeError::InvalidField { field: "type", .. }
        ));
    }

    #[test]
    fn a_vrid_of_zero_is_rejected() {
        let mut bytes = v4_advertisement().encode_v4().expect("encodes");
        bytes[1] = 0;
        let error = Advertisement::decode_verified(&bytes, IpFamily::V4).expect_err("rejected");
        assert!(matches!(
            error,
            DecodeError::InvalidField { field: "vrid", .. }
        ));
    }

    #[test]
    fn a_count_of_zero_is_rejected() {
        let mut bytes = v4_advertisement().encode_v4().expect("encodes");
        bytes[3] = 0;
        let error = Advertisement::decode_verified(&bytes, IpFamily::V4).expect_err("rejected");
        assert!(matches!(
            error,
            DecodeError::InvalidField { field: "count", .. }
        ));
    }

    #[test]
    fn a_count_larger_than_the_message_is_rejected_without_allocating() {
        let mut bytes = v4_advertisement().encode_v4().expect("encodes");
        bytes[3] = 255;
        let error = Advertisement::decode_verified(&bytes, IpFamily::V4).expect_err("rejected");
        assert!(matches!(
            error,
            DecodeError::InconsistentLength {
                declared: 1028,
                actual: 12,
                ..
            }
        ));
    }

    #[test]
    fn a_count_smaller_than_the_message_is_rejected() {
        let advertisement = Advertisement::new(
            Vrid::new(7).expect("valid"),
            Priority::new(100).expect("representable"),
            MaxAdverInt::from_duration(Duration::from_secs(1)).expect("1s fits"),
            vec![
                "192.0.2.10".parse().expect("valid address"),
                "192.0.2.11".parse().expect("valid address"),
            ],
        )
        .expect("valid");
        let mut bytes = advertisement.encode_v4().expect("encodes");
        bytes[3] = 1;
        let error = Advertisement::decode_verified(&bytes, IpFamily::V4).expect_err("rejected");
        assert!(matches!(
            error,
            DecodeError::InconsistentLength {
                declared: 12,
                actual: 16,
                ..
            }
        ));
    }

    #[test]
    fn a_family_mismatch_is_rejected_in_both_directions() {
        let advertisement = v4_advertisement();
        assert!(matches!(
            advertisement.encode_with_checksum(
                IpFamily::V6,
                ChecksumScope::PseudoHeader {
                    source: "fe80::a".parse().expect("valid address"),
                    destination: "ff02::12".parse().expect("valid address"),
                },
            ),
            Err(EncodeError::FamilyMismatch {
                expected: IpFamily::V6,
                found: IpFamily::V4
            })
        ));

        let bytes = advertisement.encode_v4().expect("encodes");
        let error = Advertisement::decode_verified(&bytes, IpFamily::V6).expect_err("rejected");
        assert!(matches!(
            error,
            DecodeError::InconsistentLength {
                family: IpFamily::V6,
                ..
            }
        ));
    }

    #[test]
    fn a_wrong_checksum_is_rejected() {
        let mut bytes = v4_advertisement().encode_v4().expect("encodes");
        bytes[7] ^= 0xff;
        let error = Advertisement::decode(&bytes, IpFamily::V4, ChecksumScope::MessageOnly)
            .expect_err("rejected");
        assert!(matches!(error, DecodeError::ChecksumMismatch { .. }));
    }

    #[test]
    fn an_undecidable_scope_is_an_error_rather_than_a_wrong_checksum() {
        // Encoding IPv6 without the addresses cannot produce a checksum that
        // would interoperate, so it refuses instead of guessing.
        let v6 = Advertisement::new(
            Vrid::new(1).expect("valid"),
            Priority::new(100).expect("representable"),
            MaxAdverInt::from_duration(Duration::from_secs(1)).expect("1s fits"),
            vec!["fe80::1".parse().expect("valid address")],
        )
        .expect("valid");
        assert!(matches!(
            v6.encode_with_checksum(IpFamily::V6, ChecksumScope::Undecidable),
            Err(EncodeError::UndecidableChecksum { .. })
        ));
    }

    #[test]
    fn an_ipv6_checksum_uses_the_pseudo_header() {
        let v6 = Advertisement::new(
            Vrid::new(1).expect("valid"),
            Priority::new(100).expect("representable"),
            MaxAdverInt::from_duration(Duration::from_secs(1)).expect("1s fits"),
            vec!["fe80::1".parse().expect("valid address")],
        )
        .expect("valid");
        let scope = ChecksumScope::PseudoHeader {
            source: "fe80::a".parse().expect("valid address"),
            destination: "ff02::12".parse().expect("valid address"),
        };
        let bytes = v6
            .encode_with_checksum(IpFamily::V6, scope)
            .expect("encodes");

        assert!(
            verify(&bytes, CHECKSUM_OFFSET, scope),
            "the packet carries its own checksum"
        );
        let carried = u16::from_be_bytes([bytes[6], bytes[7]]);
        assert_ne!(
            carried,
            crate::checksum::CHECKSUM_UNCOMPUTED,
            "the checksum is computed, not left at zero"
        );
    }

    #[test]
    fn construction_rejects_an_empty_or_mixed_address_list() {
        let vrid = Vrid::new(1).expect("valid");
        let priority = Priority::new(100).expect("representable");
        let interval = MaxAdverInt::from_duration(Duration::from_secs(1)).expect("1s fits");

        assert_eq!(
            Advertisement::new(vrid, priority, interval, Vec::new()),
            Err(AdvertisementError::NoAddresses)
        );
        assert!(matches!(
            Advertisement::new(
                vrid,
                priority,
                interval,
                vec![
                    "192.0.2.10".parse().expect("valid address"),
                    "fe80::1".parse().expect("valid address")
                ]
            ),
            Err(AdvertisementError::MixedFamilies { .. })
        ));
    }

    #[test]
    fn construction_rejects_more_addresses_than_the_count_field_holds() {
        let addresses: Vec<IpAddr> = (0..=MAX_ADDRESSES)
            .map(|last| IpAddr::from([192, 0, 2, u8::try_from(last % 254).unwrap_or(0) + 1]))
            .collect();
        let error = Advertisement::new(
            Vrid::new(1).expect("valid"),
            Priority::new(100).expect("representable"),
            MaxAdverInt::from_duration(Duration::from_secs(1)).expect("1s fits"),
            addresses,
        )
        .expect_err("rejected");
        assert!(matches!(error, AdvertisementError::TooManyAddresses { .. }));
    }

    #[test]
    fn peeking_does_not_trust_the_count() {
        let mut bytes = v4_advertisement().encode_v4().expect("encodes");
        bytes[3] = 255;

        let header = Peek::read(&bytes, IpFamily::V4).expect("the header itself is well formed");
        assert_eq!(header.count, 255);
        assert_eq!(
            header.message_len, 1028,
            "the claimed length is reported, not allocated"
        );
        assert_eq!(header.vrid.get(), 42);
        assert_eq!(header.max_adver_int.centiseconds(), 100);
        assert!(Peek::ttl_is_valid(255));
        assert!(!Peek::ttl_is_valid(254));
    }

    #[test]
    fn peeking_a_short_buffer_is_an_error_not_a_panic() {
        for length in 0..HEADER_LEN {
            assert!(matches!(
                Peek::read(&vec![0x31; length], IpFamily::V4),
                Err(DecodeError::Truncated { .. })
            ));
        }
    }

    #[test]
    fn the_relinquishing_priority_round_trips() {
        let advertisement = Advertisement::new(
            Vrid::new(1).expect("valid"),
            Priority::new(0).expect("zero is representable"),
            MaxAdverInt::from_duration(Duration::from_secs(1)).expect("1s fits"),
            vec!["192.0.2.10".parse().expect("valid address")],
        )
        .expect("valid");
        let bytes = advertisement.encode_v4().expect("encodes");
        let decoded = Advertisement::decode_verified(&bytes, IpFamily::V4).expect("decodes");

        assert!(decoded.priority().is_relinquish());
    }

    #[test]
    fn a_reserved_nibble_is_ignored_on_reception() {
        // RFC 5798 section 5.2.6: transmitted as zero, ignored on reception.
        let mut bytes = v4_advertisement().encode_v4().expect("encodes");
        bytes[4] |= 0xf0;
        assert!(Advertisement::decode_verified(&bytes, IpFamily::V4).is_ok());
    }
}
