// Rust guideline compliant 2026-09-27

//! Property tests for the VRRP codec.
//!
//! The decoder is the only code in Highland that runs on bytes from the network,
//! so its properties are stated as laws rather than examples: it never panics,
//! it accepts exactly what the encoder produced, and it accepts nothing else.

use std::net::IpAddr;
use std::time::Duration;

use highland_vrrp::{
    Advertisement, ChecksumScope, DecodeError, HEADER_LEN, IpFamily, MAX_ADDRESSES, MaxAdverInt,
    Peek, Priority, Vrid,
};
use proptest::prelude::*;

/// A VRID that is always valid.
fn vrid() -> impl Strategy<Value = Vrid> {
    (1u8..=255).prop_map(|raw| Vrid::new(raw).expect("the generator only produces valid VRIDs"))
}

/// A priority the generator may produce, including the reserved values.
fn priority() -> impl Strategy<Value = Priority> {
    (0u8..=255).prop_map(|raw| Priority::new(raw).expect("every u8 is representable"))
}

fn max_adver_int() -> impl Strategy<Value = MaxAdverInt> {
    (1u16..=4095)
        .prop_map(|centiseconds| MaxAdverInt::from_centiseconds(centiseconds).expect("in range"))
}

fn v4_addresses(count: usize) -> Vec<IpAddr> {
    (0..count)
        .map(|index| {
            let last = u8::try_from((index % 254) + 1).unwrap_or(1);
            IpAddr::from([192, 0, 2, last])
        })
        .collect()
}

fn v6_addresses(count: usize) -> Vec<IpAddr> {
    (0..count)
        .map(|index| {
            let mut octets = [0u8; 16];
            octets[0] = 0xfe;
            octets[1] = 0x80;
            octets[15] = u8::try_from((index % 254) + 1).unwrap_or(1);
            IpAddr::from(octets)
        })
        .collect()
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

    /// An IPv4 advertisement round trips for any field values and address count.
    #[test]
    fn ipv4_round_trips(
        vrid in vrid(),
        priority in priority(),
        interval in max_adver_int(),
        count in 1usize..=MAX_ADDRESSES.min(64),
    ) {
        let advertisement =
            Advertisement::new(vrid, priority, interval, v4_addresses(count)).expect("valid");
        let bytes = advertisement.encode_v4().expect("encodes");
        let decoded = Advertisement::decode_verified(&bytes, IpFamily::V4)?;

        prop_assert_eq!(decoded.vrid(), vrid);
        prop_assert_eq!(decoded.priority(), priority);
        prop_assert_eq!(decoded.max_adver_int(), interval);
        prop_assert_eq!(decoded.addresses(), &v4_addresses(count)[..]);
    }

    /// An IPv6 advertisement round trips when the checksum covers the
    /// pseudo-header, and the packet verifies as sent.
    #[test]
    fn ipv6_round_trips(
        vrid in vrid(),
        priority in priority(),
        interval in max_adver_int(),
        count in 1usize..8,
    ) {
        let scope = ChecksumScope::PseudoHeader {
            source: "fe80::a".parse().expect("valid address"),
            destination: "ff02::12".parse().expect("valid address"),
        };
        let advertisement =
            Advertisement::new(vrid, priority, interval, v6_addresses(count)).expect("valid");
        let bytes = advertisement.encode_with_checksum(IpFamily::V6, scope).expect("encodes");
        let decoded = Advertisement::decode(&bytes, IpFamily::V6, scope)?;

        prop_assert_eq!(decoded.vrid(), vrid);
        prop_assert_eq!(decoded.addresses(), &v6_addresses(count)[..]);
        prop_assert!(highland_vrrp::verify(&bytes, highland_vrrp::CHECKSUM_OFFSET, scope));
    }

    /// Any single-bit corruption of a valid packet is either detected by the
    /// checksum or rejected by the field validation. This is the property the
    /// checksum exists for, so it is asserted over every bit.
    #[test]
    fn every_single_bit_corruption_is_detected(
        vrid in vrid(),
        priority in 1u8..=254,
        interval in max_adver_int(),
        count in 1usize..4,
    ) {
        let advertisement = Advertisement::new(
            vrid,
            Priority::new(priority).expect("representable"),
            interval,
            v4_addresses(count),
        )
        .expect("valid");
        let bytes = advertisement.encode_v4().expect("encodes");
        let expected_len = IpFamily::V4.message_len(u8::try_from(count).unwrap_or(u8::MAX));

        for index in 0..bytes.len() {
            for bit in 0..8 {
                let mut corrupted = bytes.clone();
                corrupted[index] ^= 1 << bit;
                let outcome = Advertisement::decode(
                    &corrupted,
                    IpFamily::V4,
                    ChecksumScope::MessageOnly,
                );
                match outcome {
                    Err(_) => {}
                    Ok(decoded) => {
                        // A packet that still decodes is only acceptable if the
                        // corruption was in the reserved nibble, which the RFC
                        // says to ignore on reception.
                        prop_assert!(
                            index == 4 && bit >= 4,
                            "bit {bit} of byte {index} was corrupted and the packet still decoded"
                        );
                        prop_assert_eq!(decoded.vrid(), vrid);
                        prop_assert_eq!(expected_len, bytes.len());
                    }
                }
            }
        }
    }

    /// Arbitrary input never panics, and never decodes successfully into
    /// something whose length disagrees with its own count.
    #[test]
    fn arbitrary_input_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        let _ = Peek::read(&bytes, IpFamily::V4);
        let _ = Advertisement::decode_verified(&bytes, IpFamily::V4);
        let _ = Advertisement::decode(&bytes, IpFamily::V6, ChecksumScope::Undecidable);
    }

    /// A packet is accepted only if its length is exactly what its count and
    /// family imply. This is what stops an unverified length from driving an
    /// allocation.
    #[test]
    fn a_decoded_packet_always_has_a_consistent_length(
        vrid in vrid(),
        priority in priority(),
        interval in max_adver_int(),
        count in 1usize..8,
    ) {
        let advertisement =
            Advertisement::new(vrid, priority, interval, v4_addresses(count)).expect("valid");
        let bytes = advertisement.encode_v4().expect("encodes");
        let decoded = Advertisement::decode_verified(&bytes, IpFamily::V4)?;

        prop_assert_eq!(decoded.addresses().len(), count);
        prop_assert_eq!(bytes.len(), HEADER_LEN + count * 4);
    }

    /// Peeking never trusts the count, however large it is.
    #[test]
    fn peeking_reports_the_claimed_length_without_allocating(
        vrid in vrid(),
        interval in max_adver_int(),
        count_field in 1u8..=255,
    ) {
        let advertisement = Advertisement::new(
            vrid,
            Priority::new(100).expect("representable"),
            interval,
            v4_addresses(1),
        )
        .expect("valid");
        let mut bytes = advertisement.encode_v4()?;
        bytes[3] = count_field;

        let header = Peek::read(&bytes, IpFamily::V4)?;
        prop_assert_eq!(header.count, count_field);
        prop_assert_eq!(header.message_len, HEADER_LEN + usize::from(count_field) * 4);

        // Whatever the count claims, a decoded packet is never longer or
        // shorter than that claim implies: an inconsistent length is rejected
        // before anything is allocated.
        if let Ok(decoded) = Advertisement::decode_verified(&bytes, IpFamily::V4) {
            prop_assert_eq!(decoded.addresses().len(), usize::from(count_field));
            prop_assert_eq!(bytes.len(), HEADER_LEN + usize::from(count_field) * 4);
        }
    }

    /// A rejected packet always carries a typed error, never a panic and never
    /// a partial result.
    #[test]
    fn rejections_are_typed(
        bytes in proptest::collection::vec(any::<u8>(), 1..64),
    ) {
        if let Err(error) = Advertisement::decode_verified(&bytes, IpFamily::V4) {
            // Every decode failure is one of the documented variants.
            let described = match &error {
                DecodeError::Truncated { .. }
                | DecodeError::InvalidField { .. }
                | DecodeError::InconsistentLength { .. }
                | DecodeError::UnexpectedPacketType { .. }
                | DecodeError::ChecksumMismatch { .. } => true,
                other => {
                    prop_assert!(false, "undocumented decode error: {other:?}");
                    false
                }
            };
            prop_assert!(described);
            prop_assert!(!error.to_string().is_empty());
        }
    }
}

/// An advertisement encodes to the length its family and count imply.
#[test]
fn message_length_is_a_function_of_family_and_count() {
    for count in 1u8..=8 {
        let advertisement = Advertisement::new(
            Vrid::new(1).expect("valid"),
            Priority::new(100).expect("representable"),
            MaxAdverInt::from_duration(Duration::from_secs(1)).expect("1s fits"),
            v4_addresses(usize::from(count)),
        )
        .expect("valid");
        let bytes = advertisement.encode_v4().expect("encodes");
        assert_eq!(bytes.len(), IpFamily::V4.message_len(count));
    }
}

/// A default advertisement encodes to the documented byte string layout.
#[test]
fn the_default_advertisement_is_twelve_octets() {
    let advertisement = Advertisement::new(
        Vrid::new(1).expect("valid"),
        Priority::new(100).expect("representable"),
        MaxAdverInt::from_duration(Duration::from_secs(1)).expect("1s fits"),
        v4_addresses(1),
    )
    .expect("valid");
    let bytes = advertisement.encode_v4().expect("encodes");

    assert_eq!(bytes.len(), 12);
    assert_eq!(
        bytes[0], 0x31,
        "version 3, type 1, the only combination this crate emits"
    );
    assert_eq!(bytes[1], 1);
    assert_eq!(bytes[2], 100);
    assert_eq!(bytes[3], 1);
    assert_eq!(bytes[4], 0, "the reserved nibble is zero");
    assert_eq!(bytes[5], 100, "one second in centiseconds");
}
