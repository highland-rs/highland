// Rust guideline compliant 2026-09-27

//! The Internet checksum of RFC 1071, and the pseudo-header of RFC 2460 §8.1.
//!
//! RFC 5798 §5.2.8 requires the 16-bit one's complement of the one's
//! complement sum of the whole VRRP message **plus** a pseudo-header as defined
//! in RFC 2460 §8.1, with the pseudo-header's next-header field set to 112.
//!
//! For IPv6 that is unambiguous. For IPv4 it is not: the RFC does not say
//! whether the IPv6 pseudo-header applies, and interoperating implementations
//! compute the plain message checksum for IPv4. Highland therefore makes the
//! scope explicit rather than implicit, and [`ChecksumScope::for_family`]
//! records the interoperable default. The interoperability tests in Milestone 8
//! are the arbiter; see `docs/user/compatibility.md`.

use std::net::IpAddr;

use crate::types::{IpFamily, VRRP_PROTOCOL};

/// The value a checksum field carries when it has not been computed.
///
/// A receiver must reject it, because zero is indistinguishable from a genuine
/// result on the wire.
pub const CHECKSUM_UNCOMPUTED: u16 = 0;

/// Which headers the checksum covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum ChecksumScope {
    /// The VRRP message alone. What interoperating implementations compute for
    /// IPv4, whose header carries a checksum of its own.
    #[default]
    MessageOnly,
    /// The VRRP message preceded by the RFC 2460 §8.1 pseudo-header. Required
    /// for IPv6, which has no header checksum.
    PseudoHeader {
        /// The packet's source address.
        source: IpAddr,
        /// The packet's destination address.
        destination: IpAddr,
    },
    /// The family requires a pseudo-header and the addresses are unknown, so no
    /// checksum can be computed.
    Undecidable,
}

impl ChecksumScope {
    /// Returns the scope to use for `family` when no addresses are known.
    ///
    /// An IPv4 packet needs nothing beyond the message. An IPv6 packet needs
    /// the pseudo-header, and without the addresses it cannot be computed at
    /// all, which is [`ChecksumScope::Undecidable`] rather than a silent
    /// fallback to a checksum that would not interoperate.
    #[must_use]
    pub fn for_family(family: IpFamily) -> Self {
        match family {
            IpFamily::V4 => ChecksumScope::MessageOnly,
            IpFamily::V6 => ChecksumScope::Undecidable,
        }
    }

    /// Returns the scope for a packet with known addresses.
    #[must_use]
    pub fn for_packet(family: IpFamily, source: IpAddr, destination: IpAddr) -> Self {
        match family {
            IpFamily::V4 => ChecksumScope::MessageOnly,
            IpFamily::V6 => ChecksumScope::PseudoHeader {
                source,
                destination,
            },
        }
    }
}

/// An accumulator for the 16-bit one's complement sum of RFC 1071.
///
/// The accumulator holds a running total and folds the carry back in as it goes,
/// which is the algorithm RFC 1071 §3 describes. Folding on every add keeps the
/// accumulator inside 17 bits, so a long message cannot overflow it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Checksum {
    sum: u32,
    odd_octet: Option<u8>,
}

impl Checksum {
    /// Creates an empty accumulator.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds bytes to the accumulator.
    ///
    /// An odd-length input is padded with a trailing zero octet, as RFC 1071
    /// requires. Every well-formed VRRP message is even-length, so the padding
    /// only matters when verifying a malformed packet.
    pub fn add_bytes(&mut self, bytes: &[u8]) {
        let mut index = 0;
        while index < bytes.len() {
            let high = bytes[index];
            index += 1;
            let low = match self.odd_octet.take() {
                Some(carried) => carried,
                None if index < bytes.len() => {
                    let low = bytes[index];
                    index += 1;
                    low
                }
                None => {
                    // Odd length: carry the high octet into the next call.
                    self.odd_octet = Some(high);
                    return;
                }
            };
            self.add_word(u16::from_be_bytes([high, low]));
        }
    }

    /// Adds a 16-bit word to the accumulator.
    pub fn add_word(&mut self, word: u16) {
        self.sum += u32::from(word);
        self.sum = (self.sum & 0xffff) + (self.sum >> 16);
    }

    /// Adds the RFC 2460 §8.1 pseudo-header for a VRRP message.
    ///
    /// The layout is the source address, the destination address, the
    /// upper-layer packet length as a 32-bit value, three zero octets, and the
    /// next-header value. Both addresses are written in the IPv6 form, because
    /// that is the form the pseudo-header is defined in.
    pub fn add_pseudo_header(&mut self, source: IpAddr, destination: IpAddr, message_len: usize) {
        self.add_bytes(&to_v6_octets(source));
        self.add_bytes(&to_v6_octets(destination));
        self.add_word(u16::try_from(message_len).unwrap_or(u16::MAX));
        self.add_word(0);
        self.add_bytes(&[0, 0, 0, VRRP_PROTOCOL]);
    }

    /// Returns the finished checksum.
    ///
    /// An unpaired trailing octet is padded with a zero as RFC 1071 requires,
    /// which is the one case where the accumulator's result depends on
    /// finishing rather than on the next call to add.
    ///
    /// The result is the one's complement of the folded sum. A computed zero is
    /// returned as `0xffff`, because a zero checksum field is indistinguishable
    /// from an uncomputed one on the wire.
    #[must_use]
    pub fn finish(mut self) -> u16 {
        if let Some(odd) = self.odd_octet.take() {
            self.add_word(u16::from(odd) << 8);
        }
        let folded = (self.sum & 0xffff) + (self.sum >> 16);
        let folded = (folded & 0xffff) + (folded >> 16);
        // The fold above guarantees the value already fits in sixteen bits.
        let complement = !u16::try_from(folded).unwrap_or(u16::MAX);
        if complement == 0 {
            u16::MAX
        } else {
            complement
        }
    }
}

/// Writes an address in the 16-octet IPv6 form the pseudo-header is defined in.
fn to_v6_octets(address: IpAddr) -> [u8; 16] {
    match address {
        IpAddr::V4(address) => address.to_ipv6_mapped().octets(),
        IpAddr::V6(address) => address.octets(),
    }
}

/// Returns the checksum of `message` under `scope`.
///
/// `offset` is the index of the two-octet checksum field, which contributes zero
/// to the sum.
///
/// # Errors
///
/// Returns [`ChecksumScopeError::Undecidable`](crate::error::ChecksumScopeError::Undecidable)
/// when the scope needs addresses that were not supplied. Returning an error
/// rather than a plausible-looking value is deliberate: an IPv6 packet checked
/// without its pseudo-header would not interoperate, and would fail only in the
/// field.
pub fn checksum(
    message: &[u8],
    offset: usize,
    scope: ChecksumScope,
) -> Result<u16, crate::error::ChecksumScopeError> {
    let mut accumulator = Checksum::new();
    match scope {
        ChecksumScope::MessageOnly => {}
        ChecksumScope::PseudoHeader {
            source,
            destination,
        } => {
            accumulator.add_pseudo_header(source, destination, message.len());
        }
        ChecksumScope::Undecidable => return Err(crate::error::ChecksumScopeError::Undecidable),
    }

    let before = offset.min(message.len());
    accumulator.add_bytes(&message[..before]);
    accumulator.add_bytes(&[0, 0]);
    accumulator.add_bytes(&message[before.saturating_add(2).min(message.len())..]);
    Ok(accumulator.finish())
}

/// Returns `true` when `message` carries a correct checksum under `scope`.
///
/// A packet whose checksum field is zero is rejected, because the sender must
/// have computed it. A packet too short to contain the field is rejected rather
/// than trusted.
#[must_use]
pub fn verify(message: &[u8], offset: usize, scope: ChecksumScope) -> bool {
    if message.len() < offset.saturating_add(2) {
        return false;
    }
    let carried = u16::from_be_bytes([message[offset], message[offset + 1]]);
    if carried == CHECKSUM_UNCOMPUTED {
        return false;
    }
    checksum(message, offset, scope).is_ok_and(|computed| computed == carried)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A second, deliberately naive implementation, used to cross-check the
    /// folding accumulator over many inputs.
    fn reference(message: &[u8], offset: usize, scope: ChecksumScope) -> u16 {
        let mut words: Vec<u16> = Vec::new();

        if let ChecksumScope::PseudoHeader {
            source,
            destination,
        } = scope
        {
            for octets in [to_v6_octets(source), to_v6_octets(destination)] {
                for pair in octets.chunks_exact(2) {
                    words.push(u16::from_be_bytes([pair[0], pair[1]]));
                }
            }
            words.push(u16::try_from(message.len()).unwrap_or(u16::MAX));
            words.push(0);
            words.push(u16::from(VRRP_PROTOCOL));
        }

        let mut octets: Vec<u8> = message.to_vec();
        let length = octets.len();
        octets[offset.min(length.saturating_sub(1))] = 0;
        octets[(offset + 1).min(length.saturating_sub(1))] = 0;
        for pair in octets.chunks_exact(2) {
            words.push(u16::from_be_bytes([pair[0], pair[1]]));
        }
        if octets.len() % 2 == 1 {
            // RFC 1071 pads an odd-length message with a zero *trailing* octet,
            // so the final octet occupies the high half of its word.
            words.push(u16::from(octets[octets.len() - 1]) << 8);
        }

        let total: u64 = words.iter().map(|word| u64::from(*word)).sum();
        let folded = (total & 0xffff) + (total >> 16);
        let folded = (folded & 0xffff) + (folded >> 16);
        let complement = !u16::try_from(folded).unwrap_or(u16::MAX);
        if complement == 0 {
            u16::MAX
        } else {
            complement
        }
    }

    fn message() -> Vec<u8> {
        let mut bytes = vec![0x31, 0x2a, 0x96, 0x01, 0x00, 0x64, 0x00, 0x00];
        bytes.extend_from_slice(&[192, 0, 2, 10]);
        bytes
    }

    #[test]
    fn the_accumulator_matches_an_independent_implementation() {
        for length in 8_usize..40 {
            let bytes: Vec<u8> = (0..length)
                .map(|index| u8::try_from(index).unwrap_or(0).wrapping_mul(37))
                .collect();
            // Only offsets whose two-octet field fits inside the message are
            // compared; a straddling field is malformed input, and the decoder
            // rejects it before a checksum is ever computed.
            for offset in [0_usize, 6]
                .into_iter()
                .filter(|offset| offset + 2 <= length)
            {
                let mut accumulator = Checksum::new();
                accumulator.add_bytes(&bytes);
                let mut zeroed = bytes.clone();
                zeroed[offset.min(length - 1)] = 0;
                let _ = zeroed;

                let direct = checksum(&bytes, offset, ChecksumScope::MessageOnly)
                    .expect("the scope is decidable");
                let naive = reference(&bytes, offset, ChecksumScope::MessageOnly);
                assert_eq!(direct, naive, "length {length}, offset {offset}");
            }
        }
    }

    #[test]
    fn the_accumulator_matches_an_independent_implementation_with_a_pseudo_header() {
        let scope = ChecksumScope::PseudoHeader {
            source: "2001:db8::1".parse().expect("valid address"),
            destination: "ff02::12".parse().expect("valid address"),
        };
        for length in [8_usize, 24, 40] {
            let bytes: Vec<u8> = (0..length)
                .map(|index| u8::try_from(index).unwrap_or(0).wrapping_add(11))
                .collect();
            assert_eq!(
                checksum(&bytes, 6, scope).expect("decidable"),
                reference(&bytes, 6, scope),
                "length {length}"
            );
        }
    }

    #[test]
    fn a_computed_zero_is_transmitted_as_all_ones() {
        let mut accumulator = Checksum::new();
        accumulator.add_word(0xffff);
        assert_eq!(accumulator.finish(), u16::MAX);
    }

    #[test]
    fn an_odd_length_input_is_padded_rather_than_dropped() {
        let mut even = Checksum::new();
        even.add_bytes(&[0x01, 0x02, 0x03, 0x04]);
        let mut padded = Checksum::new();
        padded.add_bytes(&[0x01, 0x02, 0x03, 0x04, 0x00]);
        assert_eq!(
            even.finish(),
            padded.finish(),
            "an explicit zero pad is the same"
        );

        // The last octet of an odd-length message is not padding: it must be
        // counted in the high half of a final word.
        let mut odd = Checksum::new();
        odd.add_bytes(&[0x01, 0x02, 0x03, 0x04, 0x05]);
        let mut reference = Checksum::new();
        reference.add_word(0x0102);
        reference.add_word(0x0304);
        reference.add_word(0x0500);
        assert_eq!(odd.finish(), reference.finish());
    }

    #[test]
    fn a_message_verifies_its_own_checksum() {
        let mut bytes = message();
        let computed =
            checksum(&bytes, 6, ChecksumScope::MessageOnly).expect("the scope is decidable");
        bytes[6..8].copy_from_slice(&computed.to_be_bytes());
        assert!(verify(&bytes, 6, ChecksumScope::MessageOnly));
    }

    #[test]
    fn a_single_flipped_bit_fails_verification() {
        let mut bytes = message();
        let computed =
            checksum(&bytes, 6, ChecksumScope::MessageOnly).expect("the scope is decidable");
        bytes[6..8].copy_from_slice(&computed.to_be_bytes());
        for index in 0..bytes.len() {
            let mut corrupted = bytes.clone();
            corrupted[index] ^= 0x01;
            assert!(
                !verify(&corrupted, 6, ChecksumScope::MessageOnly),
                "flipping bit 0 of byte {index} must be detected"
            );
        }
    }

    #[test]
    fn an_uncomputed_checksum_field_is_rejected() {
        assert!(!verify(&message(), 6, ChecksumScope::MessageOnly));
    }

    #[test]
    fn the_pseudo_header_changes_the_result() {
        let bytes = message();
        let plain = checksum(&bytes, 6, ChecksumScope::MessageOnly).expect("decidable");
        let with_header = checksum(
            &bytes,
            6,
            ChecksumScope::PseudoHeader {
                source: "192.0.2.1".parse().expect("valid address"),
                destination: "224.0.0.18".parse().expect("valid address"),
            },
        )
        .expect("decidable");
        assert_ne!(plain, with_header, "the pseudo-header covers the addresses");
    }

    #[test]
    fn the_pseudo_header_length_is_the_vrrp_message_length() {
        let scope = ChecksumScope::PseudoHeader {
            source: "2001:db8::1".parse().expect("valid address"),
            destination: "ff02::12".parse().expect("valid address"),
        };
        let short = checksum(&message(), 6, scope).expect("decidable");
        let mut longer = message();
        longer.extend_from_slice(&[192, 0, 2, 11]);
        let long = checksum(&longer, 6, scope).expect("decidable");
        assert_ne!(short, long);
    }

    #[test]
    fn an_ipv4_address_is_written_in_the_ipv6_form_inside_the_pseudo_header() {
        // The pseudo-header is defined in terms of IPv6, so an IPv4 address is
        // mapped rather than written in four octets. A receiver that agrees on
        // this detail computes the same value.
        let v4 = checksum(
            &message(),
            6,
            ChecksumScope::PseudoHeader {
                source: "192.0.2.1".parse().expect("valid address"),
                destination: "224.0.0.18".parse().expect("valid address"),
            },
        )
        .expect("decidable");
        let v6 = checksum(
            &message(),
            6,
            ChecksumScope::PseudoHeader {
                source: "::ffff:192.0.2.1".parse().expect("valid address"),
                destination: "::ffff:224.0.0.18".parse().expect("valid address"),
            },
        )
        .expect("decidable");
        assert_eq!(v4, v6);
    }

    #[test]
    fn the_ipv6_default_scope_is_undecidable_rather_than_wrong() {
        assert_eq!(
            ChecksumScope::for_family(IpFamily::V4),
            ChecksumScope::MessageOnly
        );
        assert_eq!(
            ChecksumScope::for_family(IpFamily::V6),
            ChecksumScope::Undecidable
        );
        assert!(checksum(&message(), 6, ChecksumScope::for_family(IpFamily::V6)).is_err());
    }

    #[test]
    fn a_verification_of_a_short_buffer_is_refused_rather_than_panicking() {
        assert!(!verify(&[0x31, 0x01], 6, ChecksumScope::MessageOnly));
        assert!(!verify(&[], 0, ChecksumScope::MessageOnly));
    }

    #[test]
    fn a_mixed_address_pair_cannot_panic_the_accumulator() {
        let mut accumulator = Checksum::new();
        accumulator.add_pseudo_header(
            "192.0.2.1".parse().expect("valid address"),
            "ff02::12".parse().expect("valid address"),
            12,
        );
        assert_ne!(accumulator.finish(), 0);
    }
}
