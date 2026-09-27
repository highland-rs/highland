// Rust guideline compliant 2026-09-27

//! The raw VRRP socket, against a real kernel.
//!
//! Everything else in this crate is tested against a script. This file talks to
//! a real socket, because the two claims that matter cannot be checked any other
//! way are that a packet this code **sends** carries TTL 255, and that a packet
//! this code **receives** has its TTL read back correctly. A script would only be
//! asserting the script's own assumptions.
//!
//! It needs `CAP_NET_RAW` and creates its own pair of dummy interfaces, so the
//! file is behind the `netlink-tests` feature:
//!
//! ```console
//! $ podman run --rm --privileged -v "$PWD":/src -w /src rust:slim \
//!     cargo test -p highland-net --features netlink-tests --test socket
//! ```

#![cfg(all(target_os = "linux", feature = "netlink-tests"))]

use std::net::{IpAddr, Ipv4Addr};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use highland_net::{IpCidr, LinkState, NetError, NetlinkBackend, NetworkBackend};
use highland_vrrp::{Advertisement, IpFamily, MaxAdverInt, Priority, Vrid};

/// Distinguishes the dummy interfaces one run creates.
static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A dummy interface with one address, created for one test.
struct Node {
    backend: NetlinkBackend,
    name: String,
    address: IpAddr,
}

impl Node {
    async fn new(last_octet: u8) -> Self {
        let backend = NetlinkBackend::open().expect("a netlink socket opens");
        let name = format!(
            "hl{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        backend
            .create_dummy(&name)
            .await
            .unwrap_or_else(|error| panic!("could not create {name}: {error}"));

        // `198.51.100.0/24`, not `192.0.2.0/24`: the Netlink test binary
        // uses the other range, and two test binaries run at once. Two tests
        // claiming one address is not a failure, it is the backend correctly
        // refusing to let them.
        let address = IpAddr::V4(Ipv4Addr::new(198, 51, 100, last_octet));
        let id = backend
            .interface(&name)
            .await
            .unwrap_or_else(|error| panic!("could not resolve {name}: {error}"))
            .id;
        backend
            .add_address(id, IpCidr::new(address, 24))
            .await
            .expect("the node address can be added");

        Self {
            backend,
            name,
            address,
        }
    }

    fn socket(&self) -> highland_net::VrrpSocket {
        highland_net::VrrpSocket::bind(IpFamily::V4, &self.name, self.address).unwrap_or_else(
            |error| panic!("could not bind a VRRP socket to {}: {error}", self.name),
        )
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let backend = self.backend.clone();
            let name = self.name.clone();
            handle.spawn(async move {
                let _ = backend.remove_dummy(&name).await;
            });
        }
    }
}

/// An advertisement a node would send.
fn advertisement(vrid: u8, priority: u8) -> Advertisement {
    Advertisement::new(
        Vrid::new(vrid).expect("valid"),
        Priority::new(priority).expect("representable"),
        MaxAdverInt::from_duration(Duration::from_secs(1)).expect("in range"),
        vec!["192.0.2.100".parse().expect("valid address")],
    )
    .expect("valid")
}

/// An address on the loopback interface, which is the one place a node can send
/// to itself.
///
/// A dummy device has no peer, so the kernel has no route to anything through
/// it. Loopback does, and a packet that goes out and comes back is still a real
/// socket send, a real IP header, and a real TTL as the kernel reports it.
///
/// Each test takes its own address out of `127/8`, which the whole range is
/// routed to. Sharing one address would let the tests cross-talk: these run in
/// parallel, and every packet on loopback is visible to every socket bound to
/// it, which is exactly what the "quiet link" test is asserting is absent.
fn loopback(last: u8) -> IpAddr {
    IpAddr::V4(Ipv4Addr::new(127, 0, 0, last))
}

#[tokio::test]
async fn an_advertisement_travels_and_keeps_its_ttl() {
    let address = loopback(21);
    let sender =
        highland_net::VrrpSocket::bind(IpFamily::V4, "lo", address).expect("the sender binds");
    let receiver =
        highland_net::VrrpSocket::bind(IpFamily::V4, "lo", address).expect("the receiver binds");

    let bytes = advertisement(42, 150).encode_v4().expect("encodes");
    sender
        .send_to(&bytes, address)
        .expect("the advertisement is written to the socket");

    let arrival = receiver
        .receive_timeout(Duration::from_secs(2))
        .expect("the read does not fail")
        .expect("the advertisement arrives");

    assert_eq!(
        arrival.payload, bytes,
        "the payload survives the round trip"
    );
    assert_eq!(
        arrival.ttl, 255,
        "RFC 5798 section 5.1.1.3: the TTL the kernel reports is the one the socket set"
    );
    assert_eq!(
        arrival.source, address,
        "the peer is identified by the source address"
    );
}

#[tokio::test]
async fn a_packet_that_did_not_travel_with_255_is_discarded() {
    let address = loopback(22);
    let sender = highland_net::VrrpSocket::bind_with_ttl(IpFamily::V4, "lo", address, 64)
        .expect("a socket with a non-conforming TTL can be bound");
    let receiver =
        highland_net::VrrpSocket::bind(IpFamily::V4, "lo", address).expect("the receiver binds");

    let bytes = advertisement(42, 150).encode_v4().expect("encodes");
    sender
        .send_to(&bytes, address)
        .expect("the packet is written");

    let arrival = receiver
        .receive_timeout(Duration::from_secs(2))
        .expect("the read does not fail")
        .expect("the packet arrives");
    assert_eq!(
        arrival.ttl, 64,
        "the kernel reports the TTL that was actually used"
    );

    // A packet that arrived with the wrong TTL must never reach the state
    // machine, which is what this rejection enforces.
    let peers = highland_net::PeerSet::new([address]);
    let outcome = highland_net::validate(
        highland_net::Datagram {
            bytes: &arrival.payload,
            source: arrival.source,
            ttl: arrival.ttl,
        },
        &peers,
        42,
        Duration::ZERO,
        None,
    );
    assert_eq!(
        outcome.rejection(),
        Some(highland_net::Rejection::BadTtl),
        "a packet that did not travel with TTL 255 is discarded"
    );
}

#[tokio::test]
async fn an_advertisement_that_passes_every_check_is_accepted() {
    let address = loopback(23);
    let sender = highland_net::VrrpSocket::bind(IpFamily::V4, "lo", address).expect("binds");
    let receiver = highland_net::VrrpSocket::bind(IpFamily::V4, "lo", address).expect("binds");

    let bytes = advertisement(42, 150).encode_v4().expect("encodes");
    sender.send_to(&bytes, address).expect("written");

    let arrival = receiver
        .receive_timeout(Duration::from_secs(2))
        .expect("the read does not fail")
        .expect("the packet arrives");

    let peers = highland_net::PeerSet::new([address]);
    let outcome = highland_net::validate(
        highland_net::Datagram {
            bytes: &arrival.payload,
            source: arrival.source,
            ttl: arrival.ttl,
        },
        &peers,
        42,
        Duration::ZERO,
        None,
    );
    let accepted = outcome.advertisement().expect("accepted");
    assert_eq!(accepted.vrid().get(), 42);
    assert_eq!(accepted.priority().get(), 150);
    assert_eq!(accepted.addresses().len(), 1);
}

#[tokio::test]
async fn a_socket_cannot_be_bound_to_an_address_of_another_family() {
    let node = Node::new(14).await;
    let error =
        highland_net::VrrpSocket::bind(IpFamily::V4, &node.name, "fe80::1".parse().expect("valid"))
            .expect_err("a v6 address cannot bind a v4 socket");

    assert!(
        matches!(error, NetError::Encode { .. }),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn a_socket_reports_nothing_when_the_link_is_quiet() {
    let socket = highland_net::VrrpSocket::bind(IpFamily::V4, "lo", loopback(24)).expect("binds");

    // The dummy device has no peer, so nothing can arrive. The read must say so
    // rather than block or fail, because this is the path the run loop takes
    // thousands of times an hour.
    let started = Instant::now();
    let received = socket
        .receive_timeout(Duration::from_millis(200))
        .expect("the read does not fail");

    assert!(
        received.is_none(),
        "nothing arrived, and that is not an error"
    );
    assert!(
        started.elapsed() >= Duration::from_millis(150),
        "the read waited for the timeout"
    );
}

#[tokio::test]
async fn a_bound_socket_reports_its_interface_and_family() {
    let node = Node::new(16).await;
    let socket = node.socket();

    assert_eq!(socket.family(), IpFamily::V4);
    assert_eq!(socket.interface(), node.name);
    assert_eq!(
        node.backend
            .interface(&node.name)
            .await
            .expect("resolves")
            .state,
        LinkState::NoCarrier
    );
}
