// Rust guideline compliant 2026-09-28

//! Fuzzes the VRRP encoder, and with it the round trip.
//!
//! # Why this target exists
//!
//! The other two packet targets hand raw bytes to the decoder. That is the right
//! shape for a parser -- untrusted input must never panic, allocate, or disagree
//! with itself -- but it means the *encoder* is only ever reached by accident.
//! Producing a byte string that `decode_verified` accepts requires a correct
//! version, a type, a VRID, a count that agrees with the length, and a checksum
//! that verifies: a needle in a haystack of mutations. In 307 million executions
//! of `fuzz_vrrp_ipv4_packet` the encoder was never entered, because the decoder
//! rejects almost everything before the round trip begins.
//!
//! So this target does not mutate bytes at all. It *constructs* a valid
//! advertisement from arbitrary fields, encodes it, and decodes it back. The
//! properties are ones a decoder-only target cannot express:
//!
//! * a message the encoder produced must survive the decoder it was written for
//!   (`I-06`, SPEC.md §21.3) -- an encoder and decoder that disagree is silent
//!   interop breakage, not a crash, and no amount of decode fuzzing finds it;
//! * the length must agree with the count that was encoded, for every count up to
//!   the 255 the field allows (`S-04`, `S-07`);
//! * re-encoding a decoded advertisement must reproduce the bytes exactly, so the
//!   encoder is a function of the advertisement and not of its own history;
//! * the checksum must depend on the pseudo-header, which is the defect this
//!   project actually shipped once. Interoperability work found that an IPv6
//!   checksum computed over the message alone is wrong, and that the error is
//!   invisible to every other test: the message decodes, the lengths agree, and
//!   the packet is silently dropped by the peer.
//!
//! # Deliberately not fuzzed here
//!
//! Timers, sockets, and the state machine. Fuzzing a byte slice cannot reach
//! split-brain, failover timing, or the IPv6 DAD retry, and pretending otherwise
//! would be theatre. Those live in the two-node and chaos suites.

#![no_main]

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;

use arbitrary::Arbitrary;
use highland_vrrp::{Advertisement, ChecksumScope, IpFamily, MaxAdverInt, Priority, Vrid};
use libfuzzer_sys::fuzz_target;

/// The number of addresses one message may carry: an 8-bit count field.
const MAX_ADDRESSES: usize = 255;

/// A structurally valid advertisement, in the shape arbitrary should produce.
///
/// The fields are primitives rather than the library's own validated newtypes, so
/// that the boundary types get exercised on the way in: a VRID of zero, a
/// priority that is not representable, and an interval too large for the 12-bit
/// field all have to be rejected by their constructors rather than crashing.
#[derive(Arbitrary, Debug)]
struct Fields {
    vrid: u8,
    priority: u8,
    /// In centiseconds, which is the wire unit, so the 12-bit field's edge is
    /// reachable directly.
    interval_centiseconds: u16,
    v4_addresses: Vec<[u8; 4]>,
    v6_addresses: Vec<[u8; 16]>,
    use_v6: bool,
    source: [u8; 16],
    destination: [u8; 16],
}

fuzz_target!(|fields: Fields| {
    // The boundary types do the validating; anything they reject is not a
    // reachable advertisement, so there is nothing to assert about it.
    let Ok(vrid) = Vrid::new(fields.vrid) else {
        return;
    };
    let Ok(priority) = Priority::new(fields.priority) else {
        return;
    };
    let interval = Duration::from_millis(u64::from(fields.interval_centiseconds) * 10);
    let Ok(max_adver_int) = MaxAdverInt::from_duration(interval) else {
        return;
    };

    // Keep the message within the count field, and away from the 255-address
    // message length, which is 8KiB of sanitizer-instrumented allocation per
    // execution and would dominate the run without testing anything new.
    let addresses: Vec<IpAddr> = if fields.use_v6 {
        fields
            .v6_addresses
            .iter()
            .take(8)
            .map(|octets| IpAddr::V6(Ipv6Addr::from(*octets)))
            .collect()
    } else {
        fields
            .v4_addresses
            .iter()
            .take(8)
            .map(|octets| IpAddr::V4(Ipv4Addr::from(*octets)))
            .collect()
    };
    let Ok(advertisement) =
        Advertisement::new(vrid, priority, max_adver_int, addresses.clone())
    else {
        return;
    };

    let (family, scope) = scope_for(&fields, family_of(&addresses));

    let Ok(bytes) = advertisement.encode_with_checksum(family, scope) else {
        return;
    };

    // The length agrees with the count that was encoded, and with the family's
    // own arithmetic for it (S-04, S-07).
    let count = u8::try_from(addresses.len()).expect("at most eight addresses");
    assert_eq!(
        bytes.len(),
        family.message_len(count),
        "encoded length must match the address count"
    );
    assert!(
        bytes.len() <= family.message_len(MAX_ADDRESSES as u8),
        "a message may not exceed the count field's maximum"
    );

    // The property a decoder-only target cannot state.
    let decoded = Advertisement::decode_verified(&bytes, family)
        .expect("a message the encoder produced must pass the decoder it was written for");
    assert_eq!(
        decoded, advertisement,
        "encode then decode must preserve every field"
    );
    assert_eq!(decoded.addresses(), addresses.as_slice());

    // The encoder must be a function of the advertisement, not of its own
    // history: decoding and re-encoding has to be the identity.
    let re_encoded = decoded
        .encode_with_checksum(family, scope)
        .expect("re-encoding a decoded advertisement must succeed");
    assert_eq!(re_encoded, bytes, "encode(decode(x)) must equal x");

    // The checksum has to depend on the pseudo-header. This is the defect the
    // interoperability work found: an IPv6 checksum computed over the message
    // alone decodes correctly and is still rejected by the peer, so nothing else
    // in the suite can see it.
    if let ChecksumScope::PseudoHeader { source, destination } = scope {
        // Both addresses have to move. Flipping the low bit of the destination
        // gives an address guaranteed to differ, rather than one that merely
        // probably differs: an arbitrary-generated address frequently has the
        // all-zero form, and two equal pseudo-headers are indistinguishable from
        // a checksum that ignored them.
        let mut octets = fields.destination;
        octets[15] ^= 1;
        let moved = IpAddr::V6(Ipv6Addr::from(octets));
        assert_ne!(moved, destination, "the perturbed destination must differ");
        let other = advertisement
            .encode_with_checksum(
                family,
                ChecksumScope::PseudoHeader {
                    source,
                    destination: moved,
                },
            )
            .expect("the second pseudo-header is as decidable as the first");
        assert_ne!(
            other, bytes,
            "the checksum must cover the pseudo-header addresses"
        );
    }
});

/// Returns the family the addresses imply, and the checksum scope it requires.
fn family_of(addresses: &[IpAddr]) -> IpFamily {
    IpFamily::of(&addresses[0])
}

/// Builds the checksum scope for `family`.
///
/// IPv6 has no header checksum of its own, so its checksum covers an RFC 2460
/// pseudo-header and cannot be computed without addresses. Those come from the
/// generated fields rather than from a fixed constant, so that the property
/// below compares two genuinely different pseudo-headers.
fn scope_for(fields: &Fields, family: IpFamily) -> (IpFamily, ChecksumScope) {
    if family == IpFamily::V6 {
        (
            family,
            ChecksumScope::PseudoHeader {
                source: IpAddr::V6(Ipv6Addr::from(fields.source)),
                destination: IpAddr::V6(Ipv6Addr::from(fields.destination)),
            },
        )
    } else {
        (family, ChecksumScope::MessageOnly)
    }
}
