// Rust guideline compliant 2026-09-27

//! A diagnostic that answers one question with bytes rather than inference.
//!
//! Point it at an interface and it prints every VRRP datagram it receives: who
//! sent it, the TTL or hop limit the kernel reported, the header fields, the
//! bytes, and what the validator made of it. Name a group and it joins it first,
//! so multicast traffic is delivered rather than filtered away for want of a
//! membership.
//!
//! This is the tool that answered the checksum question, and it exists because
//! `tcpdump` sees nothing in some containerised networking stacks — as does an
//! `AF_PACKET` socket. What always works is the socket that is already receiving
//! the packets.
//!
//! ```console
//! # unicast, in this node's network namespace
//! HIGHLAND_PROBE_ADDRESS=192.0.2.12 HIGHLAND_PROBE_BIND=192.0.2.11 \
//!   cargo test -p highland-net --features netlink-tests --test probe_peer -- --nocapture
//!
//! # multicast
//! HIGHLAND_PROBE_ADDRESS=224.0.0.18 HIGHLAND_PROBE_BIND=192.0.2.11 \
//! HIGHLAND_PROBE_GROUP=224.0.0.18 \
//!   cargo test -p highland-net --features netlink-tests --test probe_peer -- --nocapture
//! ```
//!
//! With nothing set it says so and passes, so a contributor without a peer is not
//! blocked by it.

#![cfg(all(target_os = "linux", feature = "netlink-tests"))]

use std::net::IpAddr;
use std::time::{Duration, Instant};

use highland_net::{Accepted, AllowedSources, Datagram, ReceptionPolicy, VrrpSocket, validate};
use highland_vrrp::{IpFamily, Peek};

#[test]
fn a_live_peers_packets_can_be_dumped() {
    let Ok(peer) = std::env::var("HIGHLAND_PROBE_ADDRESS") else {
        eprintln!("no HIGHLAND_PROBE_ADDRESS set: nothing to probe");
        return;
    };
    let peer: IpAddr = peer.parse().expect("a valid peer address");
    let bind: IpAddr = std::env::var("HIGHLAND_PROBE_BIND")
        .ok()
        .and_then(|text| text.parse().ok())
        .unwrap_or(peer);
    let interface = std::env::var("HIGHLAND_PROBE_INTERFACE").unwrap_or_else(|_| "eth0".to_owned());
    let budget: u64 = std::env::var("HIGHLAND_PROBE_BUDGET_MS")
        .ok()
        .and_then(|text| text.parse().ok())
        .unwrap_or(5_000);
    let family = IpFamily::of(&bind);

    let socket = match VrrpSocket::bind_for_group(family, &interface, bind) {
        Ok(socket) => socket,
        Err(error) => {
            eprintln!("could not bind to {bind} on {interface}: {error}");
            return;
        }
    };
    // The joined group is also the group the validator is told about: a
    // multicast receiver must know which group it is speaking for, or it rejects
    // its own segment's traffic.
    let mut joined = None;
    if let Ok(group) = std::env::var("HIGHLAND_PROBE_GROUP") {
        match group.parse::<IpAddr>() {
            Ok(group) => {
                if let Err(error) = socket.join_group(group, 255) {
                    eprintln!("could not join {group}: {error}");
                }
                joined = Some(group);
            }
            Err(_) => eprintln!("HIGHLAND_PROBE_GROUP is not an address"),
        }
    }
    eprintln!("probing on {interface} for packets from {peer}, bound to {bind}");

    let deadline = Instant::now() + Duration::from_millis(budget);
    let mut seen = 0_usize;
    while Instant::now() < deadline && seen < 3 {
        let Ok(Some(received)) = socket.receive_timeout(Duration::from_millis(500)) else {
            continue;
        };
        seen += 1;
        println!("--- packet {seen} from {} ---", received.source);
        println!("hex   {}", hex(&received.payload));
        println!("ttl   {}", received.ttl);
        if let Some(destination) = received.destination {
            println!("dest  {destination}");
        }
        let vrid = match Peek::read(&received.payload, family) {
            Ok(header) => {
                println!(
                    "header version={} vrid={} priority={} count={} type={}",
                    header.version, header.vrid, header.priority, header.count, header.packet_type
                );
                header.vrid.get()
            }
            Err(error) => {
                println!("header unreadable: {error}");
                255
            }
        };

        // Validated as a *group* member, because the point is to see what a peer
        // sends rather than what our own peer list expects.
        let outcome = validate(
            Datagram {
                bytes: &received.payload,
                source: received.source,
                destination: received.destination,
                ttl: received.ttl,
            },
            &AllowedSources::Group {
                group: joined.unwrap_or(bind),
            },
            joined.or(received.destination).unwrap_or(bind),
            vrid,
            ReceptionPolicy::Strict,
            Duration::ZERO,
            None,
        );
        match &outcome {
            Accepted::Advertisement(accepted) => println!(
                "valid  vrid={} priority={} addresses={:?}",
                accepted.advertisement.vrid().get(),
                accepted.advertisement.priority().get(),
                accepted.advertisement.addresses()
            ),
            Accepted::Rejected(reason) => println!("INVALID {reason:?}"),
        }
    }
    assert!(seen > 0, "no packets arrived from {peer}");
}

/// Formats bytes as spaced hex, for a log a human can read.
fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}
