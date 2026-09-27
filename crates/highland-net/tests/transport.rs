// Rust guideline compliant 2026-09-27

//! Tests for the VRRP transport's validation.
//!
//! This is where the protocol's receiver-side rules are enforced before anything
//! reaches the state machine: the TTL must be 255, the source must be a
//! configured peer, the VRID must match, and the length must agree with the
//! count (RFC 5798 §5.1.1.3, §7.1).

use std::net::IpAddr;
use std::time::Duration;

use highland_net::{
    Accepted, AllowedSources, Datagram, PeerSet, RateWindow, Rejection, fixture_advertisement,
    validate,
};
use highland_vrrp::{Advertisement, IpFamily, MaxAdverInt, Priority, Vrid};

const PEER: [u8; 4] = [192, 0, 2, 11];
const VRID: u8 = 42;

fn peer() -> IpAddr {
    IpAddr::from(PEER)
}

fn peers() -> PeerSet {
    PeerSet::new([peer()])
}

fn datagram(bytes: &[u8]) -> Datagram<'_> {
    Datagram {
        bytes,
        source: peer(),
        ttl: 255,
        destination: None,
    }
}

/// The address a unicast datagram is treated as having been sent to.
///
/// A unicast transport knows this without being told: it is the address its
/// socket is bound to, which is the receiver's own address.
fn destination() -> IpAddr {
    "192.0.2.12".parse().expect("valid")
}

fn allowed(peers: PeerSet) -> AllowedSources {
    AllowedSources::Peers(peers)
}

fn accepted(bytes: &[u8]) -> Accepted {
    validate(
        datagram(bytes),
        &allowed(peers()),
        destination(),
        VRID,
        Duration::ZERO,
        None,
    )
}

fn advertisement(priority: u8) -> Advertisement {
    Advertisement::new(
        Vrid::new(VRID).expect("valid"),
        Priority::new(priority).expect("representable"),
        MaxAdverInt::from_duration(Duration::from_secs(1)).expect("in range"),
        vec!["192.0.2.10".parse().expect("valid address")],
    )
    .expect("valid")
}

#[test]
fn a_well_formed_advertisement_from_a_peer_is_accepted() {
    let bytes = fixture_advertisement();
    let outcome = accepted(&bytes);

    let accepted = outcome.advertisement().expect("accepted");
    assert_eq!(accepted.vrid().get(), VRID);
    assert_eq!(accepted.priority().get(), 150);
    assert_eq!(accepted.addresses().len(), 1);
}

#[test]
fn a_ttl_other_than_255_is_rejected() {
    let bytes = fixture_advertisement();
    for ttl in [0u8, 1, 64, 254] {
        let outcome = validate(
            Datagram {
                bytes: &bytes,
                source: peer(),
                ttl,
                destination: None,
            },
            &allowed(peers()),
            destination(),
            VRID,
            Duration::ZERO,
            None,
        );
        assert_eq!(outcome.rejection(), Some(Rejection::BadTtl), "ttl {ttl}");
    }
}

#[test]
fn an_advertisement_from_a_stranger_is_rejected() {
    let bytes = fixture_advertisement();
    let outcome = validate(
        Datagram {
            bytes: &bytes,
            source: "192.0.2.99".parse().expect("valid"),
            ttl: 255,
            destination: None,
        },
        &allowed(peers()),
        destination(),
        VRID,
        Duration::ZERO,
        None,
    );
    assert_eq!(outcome.rejection(), Some(Rejection::UnknownPeer));
}

#[test]
fn an_empty_peer_list_accepts_nothing() {
    let bytes = fixture_advertisement();
    let outcome = validate(
        datagram(&bytes),
        &allowed(PeerSet::default()),
        destination(),
        VRID,
        Duration::ZERO,
        None,
    );
    assert_eq!(outcome.rejection(), Some(Rejection::UnknownPeer));
}

#[test]
fn an_advertisement_for_another_vrid_is_rejected() {
    let bytes = advertisement(150).encode_v4().expect("encodes");
    let outcome = validate(
        datagram(&bytes),
        &allowed(peers()),
        destination(),
        7,
        Duration::ZERO,
        None,
    );
    assert_eq!(outcome.rejection(), Some(Rejection::WrongVrid));
}

#[test]
fn a_version_two_packet_is_rejected() {
    let mut bytes = fixture_advertisement();
    bytes[0] = 0x21;
    assert_eq!(accepted(&bytes).rejection(), Some(Rejection::BadVersion));
}

#[test]
fn an_unknown_type_is_rejected() {
    let mut bytes = fixture_advertisement();
    bytes[0] = 0x32;
    assert_eq!(accepted(&bytes).rejection(), Some(Rejection::BadType));
}

#[test]
fn a_short_datagram_is_rejected_without_reading_past_it() {
    for length in 0..8 {
        let bytes = fixture_advertisement();
        assert_eq!(
            accepted(&bytes[..length]).rejection(),
            Some(Rejection::TooShort),
            "length {length}"
        );
    }
}

#[test]
fn a_count_that_exceeds_the_message_is_rejected_without_allocating() {
    let mut bytes = fixture_advertisement();
    bytes[3] = 255;
    assert_eq!(accepted(&bytes).rejection(), Some(Rejection::BadLength));
}

#[test]
fn a_trailing_octet_is_rejected() {
    let mut bytes = fixture_advertisement();
    bytes.push(0);
    assert_eq!(accepted(&bytes).rejection(), Some(Rejection::BadLength));
}

#[test]
fn a_corrupted_checksum_is_rejected() {
    let mut bytes = fixture_advertisement();
    bytes[7] ^= 0xff;
    assert_eq!(accepted(&bytes).rejection(), Some(Rejection::BadChecksum));
}

#[test]
fn a_datagram_above_the_bound_is_rejected() {
    let oversized = vec![0x31u8; highland_net::MAX_DATAGRAM + 1];
    assert_eq!(accepted(&oversized).rejection(), Some(Rejection::TooShort));
}

#[test]
fn the_relinquishing_advertisement_is_accepted_as_an_advertisement() {
    // Priority zero is a normal advertisement carrying a special value. The
    // transport's job is to accept it; the state machine decides what it means.
    let bytes = advertisement(0).encode_v4().expect("encodes");
    let outcome = accepted(&bytes);
    let accepted = outcome.advertisement().expect("accepted");
    assert!(accepted.priority().is_relinquish());
}

#[test]
fn a_zero_interval_is_rejected() {
    let mut bytes = fixture_advertisement();
    bytes[4] = 0;
    bytes[5] = 0;
    assert_eq!(accepted(&bytes).rejection(), Some(Rejection::BadLength));
}

#[test]
fn the_rate_window_drops_the_excess() {
    let bytes = fixture_advertisement();
    let mut window = RateWindow::per_second(2);

    assert!(matches!(
        validate(
            datagram(&bytes),
            &allowed(peers()),
            destination(),
            VRID,
            Duration::ZERO,
            Some(&mut window)
        ),
        Accepted::Advertisement(_)
    ));
    assert!(matches!(
        validate(
            datagram(&bytes),
            &allowed(peers()),
            destination(),
            VRID,
            Duration::from_millis(10),
            Some(&mut window)
        ),
        Accepted::Advertisement(_)
    ));
    assert_eq!(
        validate(
            datagram(&bytes),
            &allowed(peers()),
            destination(),
            VRID,
            Duration::from_millis(20),
            Some(&mut window)
        )
        .rejection(),
        Some(Rejection::RateLimited),
        "the third datagram in one second exceeds the limit"
    );

    // The window refills after a second.
    assert!(matches!(
        validate(
            datagram(&bytes),
            &allowed(peers()),
            destination(),
            VRID,
            Duration::from_millis(1020),
            Some(&mut window)
        ),
        Accepted::Advertisement(_)
    ));
}

#[test]
fn the_rate_limit_is_checked_before_the_expensive_checks() {
    let bytes = fixture_advertisement();
    let mut window = RateWindow::per_second(0);
    assert_eq!(
        validate(
            datagram(&bytes),
            &allowed(peers()),
            destination(),
            VRID,
            Duration::ZERO,
            Some(&mut window)
        )
        .rejection(),
        Some(Rejection::RateLimited)
    );
}

#[test]
fn peers_can_be_grouped_by_family() {
    let peers = PeerSet::new([
        "192.0.2.11".parse().expect("valid"),
        "fe80::1".parse().expect("valid"),
    ]);
    assert_eq!(peers.of_family(IpFamily::V4).len(), 1);
    assert_eq!(peers.of_family(IpFamily::V6).len(), 1);
    assert!(!peers.is_empty());
    assert_eq!(peers.len(), 2);
}

#[test]
fn rejection_reasons_have_stable_names_for_the_metric_label() {
    for (reason, expected) in [
        (Rejection::TooShort, "too_short"),
        (Rejection::BadVersion, "bad_version"),
        (Rejection::BadType, "bad_type"),
        (Rejection::BadTtl, "bad_ttl"),
        (Rejection::BadLength, "bad_length"),
        (Rejection::BadChecksum, "bad_checksum"),
        (Rejection::UnknownPeer, "unknown_peer"),
        (Rejection::WrongVrid, "wrong_vrid"),
        (Rejection::RateLimited, "rate_limited"),
    ] {
        assert_eq!(reason.as_str(), expected);
    }
}

#[test]
fn an_advertisement_is_built_for_the_wire_with_a_scope() {
    let scope = highland_vrrp::ChecksumScope::for_packet(
        IpFamily::V4,
        peer(),
        IpFamily::V4.default_group(),
    );
    let bytes = highland_net::build(
        &advertisement(150),
        IpFamily::V4,
        peer(),
        IpFamily::V4.default_group(),
    )
    .expect("encodes");
    assert!(highland_vrrp::verify(
        &bytes,
        highland_vrrp::CHECKSUM_OFFSET,
        scope
    ));
}

// ----- multicast reception -------------------------------------------------

/// A multicast instance accepts any source, because membership of the group is
/// the authorisation, and the kernel has already enforced it before the packet
/// was delivered.
#[test]
fn a_group_instance_accepts_a_source_that_is_not_on_any_list() {
    let bytes = fixture_advertisement();
    let group = IpAddr::V4(std::net::Ipv4Addr::new(224, 0, 0, 18));
    let outcome = validate(
        Datagram {
            bytes: &bytes,
            source: "192.0.2.99".parse().expect("valid"),
            ttl: 255,
            destination: Some(group),
        },
        &AllowedSources::Group { group },
        group,
        VRID,
        Duration::ZERO,
        None,
    );
    assert!(
        outcome.advertisement().is_some(),
        "a member of the group is a peer: {outcome:?}"
    );
}

/// A datagram that was addressed to this host rather than to the group is not
/// group traffic, and a multicast instance must not take it.
///
/// This is the narrowing that a socket bound to *any* address makes necessary:
/// an IPv4 group socket cannot be bound to its own unicast address, so it also
/// sees datagrams sent to the host itself, and nothing else distinguishes them.
#[test]
fn a_group_instance_rejects_a_datagram_addressed_to_the_host() {
    let bytes = fixture_advertisement();
    let group = IpAddr::V4(std::net::Ipv4Addr::new(224, 0, 0, 18));
    let outcome = validate(
        Datagram {
            bytes: &bytes,
            source: peer(),
            ttl: 255,
            destination: Some(peer()),
        },
        &AllowedSources::Group { group },
        group,
        VRID,
        Duration::ZERO,
        None,
    );
    assert!(
        matches!(outcome.rejection(), Some(Rejection::WrongGroup { .. })),
        "a datagram for the host is not group traffic: {outcome:?}"
    );
}

/// The destination is not decoration: an IPv6 checksum covers it, so a receiver
/// that validates against the wrong address rejects a perfectly good packet.
///
/// This is the check that IPv6 unicast needs and never had, and it is why the
/// destination travels with the datagram rather than being assumed.
#[test]
fn an_ipv6_checksum_is_verified_against_the_address_the_packet_arrived_at() {
    let source: IpAddr = "2001:db8::11".parse().expect("valid");
    let group: IpAddr = "ff02::12".parse().expect("valid");
    let advertisement = Advertisement::new(
        Vrid::new(VRID).expect("valid"),
        Priority::new(150).expect("representable"),
        MaxAdverInt::from_duration(Duration::from_secs(1)).expect("in range"),
        vec!["2001:db8::100".parse().expect("valid address")],
    )
    .expect("valid");
    let bytes = advertisement
        .encode_with_checksum(
            IpFamily::V6,
            highland_vrrp::ChecksumScope::PseudoHeader {
                source,
                destination: group,
            },
        )
        .expect("encodes");

    let accepted = validate(
        Datagram {
            bytes: &bytes,
            source,
            ttl: 255,
            destination: Some(group),
        },
        &AllowedSources::Group { group },
        group,
        VRID,
        Duration::ZERO,
        None,
    );
    assert!(accepted.advertisement().is_some(), "{accepted:?}");

    // The same bytes, offered to an instance that speaks for a different group.
    // The group check passes — the datagram really did arrive at the address this
    // instance joined — and the checksum does not, because the pseudo-header
    // names an address the bytes were not built for.
    let elsewhere: IpAddr = "ff02::20".parse().expect("valid");
    let rejected = validate(
        Datagram {
            bytes: &bytes,
            source,
            ttl: 255,
            destination: Some(elsewhere),
        },
        &AllowedSources::Group { group: elsewhere },
        elsewhere,
        VRID,
        Duration::ZERO,
        None,
    );
    assert_eq!(
        rejected.rejection(),
        Some(Rejection::BadChecksum),
        "a checksum covers the destination, so the wrong one must not verify"
    );
}
