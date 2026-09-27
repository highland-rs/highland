// Rust guideline compliant 2026-09-27

//! Golden vectors for the codec, and the round-trip guarantee over captures.
//!
//! # Provenance, and what these fixtures are not
//!
//! The vectors in `tests/packet-captures/` were produced by this project's
//! encoder, then checked in so that a change to the wire format is visible in a
//! diff. They are **not** captures from another implementation: RFC 5798 does not
//! publish a byte-level worked example, and inventing one would be worse than
//! being explicit about where the bytes came from.
//!
//! The interoperability evidence that these fixtures do not provide arrives in
//! Milestone 8, where Highland and Keepalived run against each other in isolated
//! namespaces and packets are captured from both (`SPEC.md` §21.6). Those
//! captures replace this file. Until then, the property tests in
//! `tests/properties.rs` carry the correctness argument: every bit corruption is
//! detected, and no input decodes into something inconsistent with its own
//! length.

use std::net::IpAddr;
use std::time::Duration;

use highland_vrrp::{
    Advertisement, ChecksumScope, DecodeError, IpFamily, MaxAdverInt, Peek, Priority, Vrid,
};

/// One checked-in message.
struct Vector {
    /// A human description, used in assertion messages.
    name: &'static str,
    /// The message as hex, which is how captures are usually pasted into issues.
    hex: &'static str,
    /// The family the message belongs to.
    family: IpFamily,
    /// The checksum scope the message was built under, and under which it
    /// verifies.
    scope: ChecksumScope,
}

const V4_SCOPE: ChecksumScope = ChecksumScope::MessageOnly;

/// The IPv6 pseudo-header addresses the IPv6 vectors are built under.
const V6_SOURCE: &str = "fe80::a";
const V6_DESTINATION: &str = "ff02::12";

fn v6_scope() -> ChecksumScope {
    ChecksumScope::PseudoHeader {
        source: V6_SOURCE.parse().expect("the fixture address is valid"),
        destination: V6_DESTINATION
            .parse()
            .expect("the fixture address is valid"),
    }
}

/// The vectors, as produced by the encoder. Regenerate with
/// `HIGHLAND_DUMP_VECTORS=1 cargo test -p highland-vrrp --test captures -- --nocapture`.
fn vectors() -> Vec<Vector> {
    vec![
        Vector {
            name: "ipv4, address owner, one address, one second",
            hex: "3101ff0100640d8ec000020a",
            family: IpFamily::V4,
            scope: V4_SCOPE,
        },
        Vector {
            name: "ipv4, backing-up router, one address, one second",
            hex: "310164010064a88ec000020a",
            family: IpFamily::V4,
            scope: V4_SCOPE,
        },
        Vector {
            name: "ipv4, two addresses, half-second interval",
            hex: "312a96020032b48ac000020ac000020b",
            family: IpFamily::V4,
            scope: V4_SCOPE,
        },
        Vector {
            name: "ipv4, relinquishing, priority zero",
            hex: "312a000100640c66c000020a",
            family: IpFamily::V4,
            scope: V4_SCOPE,
        },
        Vector {
            name: "ipv4, the twelve-bit interval at its maximum",
            hex: "31fffe010fff95cac6336401",
            family: IpFamily::V4,
            scope: V4_SCOPE,
        },
        Vector {
            name: "ipv6, one link-local address, one second",
            hex: "3101ff010064d2eefe800000000000000000000000000001",
            family: IpFamily::V6,
            scope: v6_scope(),
        },
        Vector {
            name: "ipv6, two link-local addresses, one second",
            hex: "3102640200646f5afe800000000000000000000000000001fe800000000000000000000000000002",
            family: IpFamily::V6,
            scope: v6_scope(),
        },
    ]
}

/// The advertisements the vectors are built from, in the same order.
fn advertisements() -> Vec<Advertisement> {
    let interval = |centiseconds: u16| {
        MaxAdverInt::from_centiseconds(centiseconds).expect("the fixture interval is in range")
    };
    let vrid = |raw: u8| Vrid::new(raw).expect("the fixture VRID is valid");
    let address = |text: &str| text.parse().expect("the fixture address is valid");

    vec![
        Advertisement::new(
            vrid(1),
            Priority::new(255).expect("representable"),
            interval(100),
            vec![address("192.0.2.10")],
        )
        .expect("valid"),
        Advertisement::new(
            vrid(1),
            Priority::new(100).expect("representable"),
            interval(100),
            vec![address("192.0.2.10")],
        )
        .expect("valid"),
        Advertisement::new(
            vrid(42),
            Priority::new(150).expect("representable"),
            interval(50),
            vec![address("192.0.2.10"), address("192.0.2.11")],
        )
        .expect("valid"),
        Advertisement::new(
            vrid(42),
            Priority::new(0).expect("representable"),
            interval(100),
            vec![address("192.0.2.10")],
        )
        .expect("valid"),
        Advertisement::new(
            vrid(255),
            Priority::new(254).expect("representable"),
            interval(4095),
            vec![address("198.51.100.1")],
        )
        .expect("valid"),
        Advertisement::new(
            vrid(1),
            Priority::new(255).expect("representable"),
            interval(100),
            vec![address("fe80::1")],
        )
        .expect("valid"),
        Advertisement::new(
            vrid(2),
            Priority::new(100).expect("representable"),
            interval(100),
            vec![address("fe80::1"), address("fe80::2")],
        )
        .expect("valid"),
    ]
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut text, byte| {
        use std::fmt::Write as _;
        let _ = write!(text, "{byte:02x}");
        text
    })
}

fn from_hex(text: &str) -> Vec<u8> {
    assert!(text.len() % 2 == 0, "a hex string has an even length");
    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&text[index..index + 2], 16).expect("valid hex"))
        .collect()
}

#[test]
fn the_checked_in_vectors_match_the_encoder() {
    let advertisements = advertisements();
    assert_eq!(advertisements.len(), vectors().len());

    for (index, (vector, advertisement)) in vectors().iter().zip(advertisements.iter()).enumerate()
    {
        let bytes = if vector.family == IpFamily::V4 {
            advertisement.encode_v4().expect("encodes")
        } else {
            advertisement
                .encode_with_checksum(vector.family, vector.scope)
                .expect("encodes")
        };

        assert_eq!(
            to_hex(&bytes),
            vector.hex,
            "{}: the wire format changed, so the checked-in vector is stale",
            vector.name
        );
        let _ = index;
    }
}

#[test]
fn every_vector_decodes_to_the_advertisement_it_encodes() {
    for (vector, advertisement) in vectors().iter().zip(advertisements().iter()) {
        let bytes = if vector.family == IpFamily::V4 {
            advertisement.encode_v4().expect("encodes")
        } else {
            advertisement
                .encode_with_checksum(vector.family, vector.scope)
                .expect("encodes")
        };

        let decoded = Advertisement::decode(&bytes, vector.family, vector.scope)
            .unwrap_or_else(|error| panic!("{}: {error}", vector.name));

        assert_eq!(decoded.vrid(), advertisement.vrid(), "{}", vector.name);
        assert_eq!(
            decoded.priority(),
            advertisement.priority(),
            "{}",
            vector.name
        );
        assert_eq!(
            decoded.max_adver_int(),
            advertisement.max_adver_int(),
            "{}",
            vector.name
        );
        assert_eq!(
            decoded.addresses(),
            advertisement.addresses(),
            "{}",
            vector.name
        );
    }
}

#[test]
fn every_vector_has_the_length_its_count_implies() {
    for (vector, advertisement) in vectors().iter().zip(advertisements().iter()) {
        let bytes = if vector.family == IpFamily::V4 {
            advertisement.encode_v4().expect("encodes")
        } else {
            advertisement
                .encode_with_checksum(vector.family, vector.scope)
                .expect("encodes")
        };

        let header = Peek::read(&bytes, vector.family).expect("the header is well formed");
        assert_eq!(
            header.count as usize,
            advertisement.addresses().len(),
            "{}",
            vector.name
        );
        assert_eq!(header.message_len, bytes.len(), "{}", vector.name);
        assert_eq!(
            header.max_adver_int,
            advertisement.max_adver_int(),
            "{}",
            vector.name
        );
    }
}

#[test]
fn every_vector_carries_a_verifiable_checksum() {
    for (vector, advertisement) in vectors().iter().zip(advertisements().iter()) {
        let bytes = if vector.family == IpFamily::V4 {
            advertisement.encode_v4().expect("encodes")
        } else {
            advertisement
                .encode_with_checksum(vector.family, vector.scope)
                .expect("encodes")
        };

        assert!(
            highland_vrrp::verify(&bytes, highland_vrrp::CHECKSUM_OFFSET, vector.scope),
            "{}",
            vector.name
        );
    }
}

/// Prints the vectors so that `EXPECTED_HEX` can be refreshed deliberately.
#[test]
fn the_vectors_can_be_dumped() {
    if std::env::var("HIGHLAND_DUMP_VECTORS").is_err() {
        return;
    }
    for (vector, advertisement) in vectors().iter().zip(advertisements().iter()) {
        let bytes = if vector.family == IpFamily::V4 {
            advertisement.encode_v4().expect("encodes")
        } else {
            advertisement
                .encode_with_checksum(vector.family, vector.scope)
                .expect("encodes")
        };
        println!("// {}", vector.name);
        println!("    \"{}\",", to_hex(&bytes));
    }
}

#[test]
fn hex_round_trips() {
    let bytes: Vec<u8> = (0..=255u8).collect();
    assert_eq!(from_hex(&to_hex(&bytes)), bytes);
}

#[test]
fn a_vector_decoded_as_the_wrong_family_is_rejected() {
    for (vector, advertisement) in vectors().iter().zip(advertisements().iter()) {
        let other = if vector.family == IpFamily::V4 {
            IpFamily::V6
        } else {
            IpFamily::V4
        };
        let bytes = if vector.family == IpFamily::V4 {
            advertisement.encode_v4().expect("encodes")
        } else {
            advertisement
                .encode_with_checksum(vector.family, vector.scope)
                .expect("encodes")
        };

        assert!(
            matches!(
                Advertisement::decode_verified(&bytes, other),
                Err(DecodeError::InconsistentLength { .. })
            ),
            "{}: decoding as the wrong family must fail",
            vector.name
        );
    }
}

/// The default vector set covers the field values the specification calls out.
#[test]
fn the_vectors_cover_the_specified_field_values() {
    let advertisements = advertisements();

    assert!(
        advertisements[0].priority().is_address_owner(),
        "priority 255 is the address owner"
    );
    assert_eq!(
        advertisements[0].max_adver_int().centiseconds(),
        100,
        "the default is one second"
    );
    assert!(
        advertisements[3].priority().is_relinquish(),
        "priority 0 is the relinquish value"
    );
    assert_eq!(
        advertisements[4].max_adver_int().centiseconds(),
        4095,
        "the 12-bit field maximum"
    );
    assert_eq!(
        advertisements[2].addresses().len(),
        2,
        "several addresses are covered"
    );
    assert_eq!(
        advertisements[5].family(),
        IpFamily::V6,
        "both families are covered"
    );
    assert_eq!(
        advertisements[1].max_adver_int().as_duration(),
        Duration::from_secs(1)
    );
}

/// The address lists in the fixtures are single-family, which RFC 5798 §5.2.9
/// requires.
#[test]
fn no_vector_mixes_families() {
    for advertisement in advertisements() {
        let family = advertisement.family();
        for address in advertisement.addresses() {
            let _: IpAddr = *address;
            assert!(
                family.matches(address),
                "{address} is not an {family} address"
            );
        }
    }
}
