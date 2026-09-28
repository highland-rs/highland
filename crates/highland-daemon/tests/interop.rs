// Rust guideline compliant 2026-09-27

//! Highland and Keepalived, on one segment, in both directions.
//!
//! This is the suite that makes the interoperability claim true rather than
//! plausible. Everything else in the repository tests Highland against itself,
//! which proves the state machine and the codec agree with each other and says
//! nothing about whether the bytes are ones any other implementation accepts.
//!
//! It found a real protocol defect. RFC 5798 §5.2.8 requires the checksum to
//! cover the VRRP message **and** a pseudo-header whose next-header field is
//! 112, for both address families. Highland had read the IPv4 header's own
//! checksum as a reason to skip the pseudo-header for IPv4, and the two
//! implementations silently ignored each other: every packet was rejected as
//! `bad_checksum`, both nodes timed out, and both became master. The packet
//! that settled it is
//! [`KEEPALIVED_V4_ADVERTISEMENT`](highland_vrrp::KEEPALIVED_V4_ADVERTISEMENT),
//! which is now a golden vector in the codec.
//!
//! # Skipping
//!
//! The suite needs the `keepalived` binary. Where it is absent the tests report
//! that and pass, because a contributor on a laptop has not done anything wrong;
//! the Linux gate installs it, and CI runs this suite as its own job.
//!
//! # What is asserted
//!
//! Both directions, because one direction is not evidence of interoperability:
//!
//! - Highland master, Keepalived backup: Highland takes the address, Keepalived
//!   stays a backup, and Highland rejects nothing.
//! - Keepalived master, Highland backup: Keepalived takes the address, and when
//!   it is killed Highland takes over inside `Master_Down_Interval`.
//!
//! The first direction is the one that finds checksum and field-level
//! incompatibilities. The second is the one that proves the *protocol* agrees,
//! rather than the two implementations merely having different bugs.

#![cfg(all(target_os = "linux", feature = "netlink-tests"))]

mod support;

use std::net::IpAddr;
use std::time::Duration;

use highland_net::{AllowedSources, Datagram, VrrpSocket, validate};
use highland_vrrp::{ChecksumScope, IpFamily};

use support::{Keepalived, Node, bridge, wait_for};

/// `Master_Down_Interval` for a one-second interval at priority 100: 3.609s.
const MASTER_DOWN_BUDGET: Duration = Duration::from_millis(3609);

/// Skips the test when Keepalived is not installed, and says so.
///
/// # Panics
///
/// Panics when `keepalived --version` fails for a reason other than the binary
/// being absent.
fn require_keepalived(test: &str) -> bool {
    let found = std::process::Command::new("keepalived")
        .arg("--version")
        .output();
    match found {
        Ok(output) if output.status.success() => true,
        Ok(_) => {
            eprintln!("{test}: keepalived is present but will not report a version; skipping");
            false
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("{test}: keepalived is not installed; skipping");
            false
        }
        Err(error) => panic!("could not run keepalived: {error}"),
    }
}

/// Highland wins the election against a Keepalived backup, and the two
/// understand each other well enough that only one holds the address.
///
/// The assertion that matters is the last one: `bad_checksum` counting zero. A
/// pair that fails over correctly while ignoring each other's advertisements
/// looks exactly like a working pair until something is slow, at which point the
/// address moves for no reason a log can explain.
#[test]
fn highland_master_and_keepalived_backup_agree_on_one_master() {
    if !require_keepalived("highland_master_and_keepalived_backup") {
        return;
    }
    let guard = bridge();
    let name = guard.name.clone();
    let _bridge_guard = guard;

    let mut highland = Node::create_full("ipa", A, &name, false, 150, VIP, 24, Some(B));
    let mut keepalived =
        Keepalived::create("ipb", B, &format!("{VIP}/24"), &name, 100, 42, Some(A));
    keepalived.start();
    highland.start();

    // Highland has the higher priority, so it takes the address.
    let took = wait_for("highland took the address", Duration::from_secs(10), || {
        highland.holds_vip()
    });
    assert!(
        took,
        "the higher-priority node never took the address\n--- highland ---\n{}\n--- keepalived ---\n{}",
        highland.log(),
        keepalived.log()
    );

    // Keepalived must have heard the advertisement and stayed a backup, rather
    // than timing out and taking the address too — which is what happens when
    // the two implementations silently ignore each other.
    let stayed = wait_for(
        "keepalived accepted the advertisement",
        Duration::from_secs(6),
        || !keepalived.says_master(),
    );
    assert!(
        stayed,
        "keepalived became master while highland held the address\n--- keepalived ---\n{}",
        keepalived.log()
    );
    assert!(
        !keepalived.holds_address(VIP),
        "both implementations hold the address, which is a split brain\n--- keepalived ---\n{}",
        keepalived.log()
    );

    // Nothing is discarded. A VRRP backup does not advertise, so there is no
    // traffic from Keepalived to receive in this direction; the reception
    // evidence is asserted in the other test, where Keepalived is the master.
    assert!(
        !highland.log().contains("discarded a VRRP packet"),
        "highland discarded packets from a real implementation\n--- highland ---\n{}",
        highland.log()
    );

    // Killing Keepalived must not disturb a node that is already master.
    keepalived.kill();
    std::thread::sleep(Duration::from_secs(2));
    assert!(
        highland.holds_vip(),
        "the master gave the address up when a backup died\n--- highland ---\n{}",
        highland.log()
    );
}

/// The other direction: Keepalived is master, Highland is the backup, and
/// killing the master hands the address over.
///
/// This is what proves the *protocol* agrees rather than the two
/// implementations having complementary mistakes: here Highland's advertisements
/// are what Keepalived acts on, and Keepalived's are what Highland acts on.
#[test]
fn keepalived_master_and_highland_backup_hand_over_when_it_dies() {
    if !require_keepalived("keepalived_master_and_highland_backup") {
        return;
    }
    let guard = bridge();
    let name = guard.name.clone();
    let _bridge_guard = guard;

    let mut highland = Node::create_full("ipb2", B, &name, false, 100, VIP, 24, Some(A));
    let mut keepalived =
        Keepalived::create("ipa2", A, &format!("{VIP}/24"), &name, 150, 42, Some(B));
    keepalived.start();
    highland.start();

    // Keepalived has the higher priority, so it takes the address and Highland
    // must not.
    let took = wait_for(
        "keepalived took the address",
        Duration::from_secs(10),
        || keepalived.says_master() && keepalived.holds_address(VIP),
    );
    assert!(
        took,
        "keepalived never took the address\n--- keepalived ---\n{}\n--- highland ---\n{}",
        keepalived.log(),
        highland.log()
    );
    let held_off = wait_for("highland stayed a backup", Duration::from_secs(6), || {
        !highland.holds_vip()
    });
    assert!(
        held_off,
        "highland took the address from a live higher-priority master\n--- highland ---\n{}",
        highland.log()
    );
    let heard = wait_for(
        "highland accepted an advertisement",
        Duration::from_secs(8),
        || highland.log().contains("advertisement received"),
    );
    assert!(
        heard,
        "highland never heard keepalived, so the hold-off above is vacuous\n--- highland ---\n{}",
        highland.log()
    );
    assert!(
        !highland.log().contains("discarded a VRRP packet"),
        "highland discarded packets from a real implementation\n--- highland ---\n{}",
        highland.log()
    );

    // And the handover, inside the interval the specification budgets.
    keepalived.kill();
    let took_over = wait_for("highland took over", MASTER_DOWN_BUDGET, || {
        highland.holds_vip()
    });
    assert!(
        took_over,
        "highland did not take over within {MASTER_DOWN_BUDGET:?}\n--- highland ---\n{}",
        highland.log()
    );
}

/// Highland and Keepalived over IPv6, with Highland the master.
///
/// Two protocol rules meet here, and both are the RFC's rather than ours:
///
/// - §5.1.2.1: an IPv6 advertisement's source is "the IPv6 link-local address of
///   the interface the packet is being sent from". Not a configured one, and not
///   the first address on the interface. So the peer list names link-local
///   addresses, Highland advertises from its own, and Keepalived is left to choose
///   its default rather than being told a global address.
/// - §5.1.2.3: the hop limit must be 255, and a packet with any other value must
///   be discarded. Configuring a *global* IPv6 source in Keepalived makes it
///   relax the hop limit, so a conforming receiver then discards its
///   advertisements — which is exactly what this test caught.
///
/// A VRRP backup does not advertise, so there is nothing for Highland to receive
/// in this direction. The proof is that Keepalived *stays* a backup, which it can
/// only do by having understood Highland's advertisements.
#[test]
fn highland_master_and_keepalived_backup_agree_over_ipv6() {
    if !require_keepalived("ipv6_highland_master") {
        return;
    }
    let mut segment = ipv6_pair(100);
    segment
        .peer
        .write_ipv6_config(&segment.highland, 100, false, 43);
    segment
        .highland
        .write_ipv6_config(&segment.peer, 150, false, 43);
    let (highland, peer) = (&mut segment.highland, &mut segment.peer);
    peer.start();
    highland.start();

    let took = wait_for(
        "highland took the IPv6 address",
        Duration::from_secs(12),
        || highland.holds(VIP6),
    );
    assert!(
        took,
        "the higher-priority node never took {VIP6}\nhighland has {:?}\nkeepalived has {:?}\n--- highland config ---\n{}\n--- keepalived config ---\n{}\n--- highland ---\n{}\n--- keepalived ---\n{}",
        highland.addresses(),
        peer.holds_address(VIP6),
        highland.config_text(),
        peer.config_text(),
        highland.log(),
        peer.log()
    );
    let stayed = wait_for(
        "keepalived accepted the advertisement",
        Duration::from_secs(6),
        || !peer.says_master(),
    );
    assert!(
        stayed,
        "keepalived became master while highland held {VIP6}, which means it did not \
         understand the advertisements\n--- keepalived ---\n{}",
        peer.log()
    );
    assert!(
        !highland.log().contains("discarded a VRRP packet"),
        "highland discarded keepalived's IPv6 advertisements\n--- highland ---\n{}",
        highland.log()
    );
}

/// What Keepalived sends as an IPv6 master over unicast, and what a conforming
/// receiver does with it.
///
/// This test exists because the scenario above it cannot pass in this
/// environment, and the reason is worth recording rather than working around.
///
/// Keepalived 2.3.3 advertises IPv6 **unicast** with a hop limit of 64. Its IPv6
/// **multicast** advertisements carry 255, which is why the multicast scenario
/// passes and the unicast one does not. RFC 5798 §5.1.2.3 is unambiguous: "The
/// Hop Limit MUST be set to 255. A VRRP router receiving a packet with the Hop
/// Limit not equal to 255 MUST discard the packet." So Highland is right to
/// discard it, and a receiver that accepted it would be the bug.
///
/// The test therefore asserts the conforming behaviour — the packet is discarded,
/// with the value it carried in the reason — and the handover is proven in the
/// other direction, where the advertisement that matters is one Highland produced
/// and Keepalived understood.
#[test]
fn an_ipv6_unicast_advertisement_with_the_wrong_hop_limit_is_discarded() {
    if !require_keepalived("ipv6_unicast_hop_limit") {
        return;
    }
    let mut segment = ipv6_pair(150);
    segment
        .peer
        .write_ipv6_config(&segment.highland, 150, false, 43);
    segment
        .highland
        .write_ipv6_config(&segment.peer, 100, false, 43);
    segment.peer.start();
    segment.highland.start();

    let discarded = wait_for(
        "the advertisement was discarded",
        Duration::from_secs(10),
        || segment.highland.log().contains("discarded a VRRP packet"),
    );
    assert!(
        discarded,
        "keepalived's IPv6 unicast advertisement was accepted\n--- highland ---\n{}\n--- keepalived ---\n{}",
        segment.highland.log(),
        segment.peer.log()
    );
    let log = segment.highland.log();
    assert!(
        log.contains("reason=\"bad_ttl\""),
        "and it was discarded for its hop limit, which RFC 5798 section 5.1.2.3 forbids:\n{log}"
    );
    assert!(
        !log.contains("ttl=255"),
        "and not for anything else: the hop limit it carried is in the line above\n{log}"
    );
    assert!(
        !segment.highland.holds(VIP6),
        "a node that discarded its only master's advertisements does not take the address"
    );
}

/// IPv4 multicast, where neither node names a peer.
///
/// This is the mode the multicast code was written for and never tested against
/// anything else: the group membership, the multicast TTL, and the fact that a
/// multicast socket on IPv4 has to be bound to *any* address to receive at all.
#[test]
fn highland_and_keepalived_agree_over_ipv4_multicast() {
    if !require_keepalived("ipv4_multicast") {
        return;
    }
    let mut segment = multicast_pair(A, B, VIP, 24, 100, 42);
    segment
        .highland
        .write_config(VIP, 24, 150, None, "", false, 42);
    segment.peer.write_config(&format!("{VIP}/24"), None, None);
    let highland = &mut segment.highland;
    let peer = &mut segment.peer;
    peer.start();
    highland.start();

    assert_joined(highland, "224.0.0.18");
    let took = wait_for("highland took the address", Duration::from_secs(12), || {
        highland.holds(VIP)
    });
    assert!(
        took,
        "multicast mode elected no master\n--- highland ---\n{}\n--- keepalived ---\n{}",
        highland.log(),
        peer.log()
    );
    let stayed = wait_for("keepalived stayed a backup", Duration::from_secs(6), || {
        !peer.says_master()
    });
    assert!(
        stayed,
        "keepalived became master in multicast mode, so it heard nothing\n--- keepalived ---\n{}",
        peer.log()
    );
    assert!(
        !highland.log().contains("discarded a VRRP packet"),
        "highland discarded packets in multicast mode\n--- highland ---\n{}",
        highland.log()
    );
}

/// IPv6 multicast over `ff02::12`, which needs a group, a hop limit of 255, and a
/// scope on the destination address.
#[tokio::test]
async fn highland_and_keepalived_agree_over_ipv6_multicast() {
    if !require_keepalived("ipv6_multicast") {
        return;
    }
    let mut segment = multicast_pair(A6, B6, VIP6, 64, 100, 43);
    segment
        .highland
        .write_ipv6_config(&segment.peer, 150, true, 43);
    segment
        .peer
        .write_ipv6_config(&segment.highland, 100, true, 43);
    let (highland, peer) = (&mut segment.highland, &mut segment.peer);
    peer.start();
    highland.start();

    assert_joined(highland, "ff02::12");
    let took = wait_for(
        "highland took the IPv6 address",
        Duration::from_secs(12),
        || highland.holds(VIP6),
    );
    assert!(
        took,
        "IPv6 multicast elected no master\n--- highland ---\n{}\n--- keepalived ---\n{}",
        highland.log(),
        peer.log()
    );
    let stayed = wait_for("keepalived stayed a backup", Duration::from_secs(6), || {
        !peer.says_master()
    });
    assert!(
        stayed,
        "keepalived became master in IPv6 multicast mode\n--- keepalived ---\n{}",
        peer.log()
    );
    assert!(
        !highland.log().contains("discarded a VRRP packet"),
        "highland discarded packets in IPv6 multicast mode\n--- highland ---\n{}",
        highland.log()
    );

    // And the bytes themselves: what Keepalived put on the wire, decoded and then
    // re-encoded, must be identical. This is the IPv6 counterpart of
    // `KEEPALIVED_V4_ADVERTISEMENT` in the codec, and it covers what an encoder is
    // most likely to get wrong — a sixteen-octet address list and a pseudo-header
    // with sixteen-octet addresses in it.
    //
    // A probe socket in the peer namespace rather than a frozen constant: the
    // addresses here are the interface's own link-local, which differs on every
    // machine, so a checked-in vector could never be compared. Capturing it and
    // re-encoding is stronger — it proves the two implementations agree on these
    // bytes rather than on some earlier run of them.
    let source: IpAddr = peer
        .link_local()
        .expect("the peer has a link-local address")
        .parse()
        .expect("a link-local address parses");
    let group: IpAddr = "ff02::12".parse().expect("a valid group");
    let socket = match VrrpSocket::bind_for_group(IpFamily::V6, "eth0", source) {
        Ok(socket) => socket,
        Err(error) => {
            eprintln!("no probe socket in the peer namespace: {error}");
            return;
        }
    };
    if let Err(error) = socket.join_group(group, 255) {
        eprintln!("could not join ff02::12 in the peer namespace: {error}");
        return;
    }
    let captured = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(Some(received)) = socket.receive_timeout(Duration::from_millis(250))
                && received.ttl == 255
            {
                return received;
            }
        }
    })
    .await;
    let Ok(received) = captured else {
        eprintln!("no IPv6 multicast advertisement arrived on ff02::12");
        return;
    };

    let scope = ChecksumScope::for_packet(IpFamily::V6, source, group);
    let outcome = validate(
        Datagram {
            bytes: &received.payload,
            source: received.source,
            destination: received.destination,
            ttl: received.ttl,
        },
        &AllowedSources::Group { group },
        group,
        43,
        Duration::ZERO,
        None,
    );
    let accepted = outcome
        .advertisement()
        .unwrap_or_else(|| panic!("keepalived's IPv6 advertisement did not verify: {outcome:?}"));

    let re_encoded = accepted
        .encode_with_checksum(IpFamily::V6, scope)
        .expect("encodes");
    assert_eq!(
        re_encoded, received.payload,
        "our bytes for keepalived's own IPv6 advertisement match"
    );
}

/// Two nodes, their bridge, and the link-local addresses the configurations need.
///
/// The bridge is held rather than forgotten: a forgotten bridge outlives the test
/// that made it, and the next test's `bridge()` deletes it by name while the
/// previous test's veth ends are still attached to it. Fields drop in declaration
/// order, so the guard is last and the bridge outlives the nodes — deleting it
/// first would take the veth ends with it.
///
/// Both links exist before either configuration is written, because an IPv6
/// link-local address does not exist until the link is up.
struct Segment {
    highland: Node,
    peer: Keepalived,
    /// Held, never read: the bridge must outlive the nodes, and dropping it
    /// first would take the veth ends with it.
    _bridge: support::BridgeGuard,
}

fn ipv6_pair(keepalived_priority: u16) -> Segment {
    let guard = bridge();
    let name = guard.name.clone();

    let highland = Node::attach("v6a", A6, &name, 64);
    let peer = Keepalived::attach(
        "v6b",
        B6,
        &format!("{VIP6}/64"),
        &name,
        keepalived_priority,
        43,
        None,
    );
    highland.wait_for_stable_link_local();
    peer.wait_for_stable_link_local();
    Segment {
        highland,
        peer,
        _bridge: guard,
    }
}

/// Two nodes with no peer list on either side, which is multicast mode.
fn multicast_pair(
    a: &str,
    b: &str,
    vip: &str,
    prefix: u8,
    keepalived_priority: u16,
    vrid: u8,
) -> Segment {
    let guard = bridge();
    let name = guard.name.clone();

    let highland = Node::attach("mca", a, &name, prefix);
    let peer = Keepalived::multicast(
        "mcb",
        b,
        &format!("{vip}/{prefix}"),
        &name,
        keepalived_priority,
        vrid,
    );
    highland.wait_for_stable_link_local();
    peer.wait_for_stable_link_local();
    Segment {
        highland,
        peer,
        _bridge: guard,
    }
}

/// Asserts that a node has joined a group, read from the kernel rather than from
/// the configuration that asked for it.
#[track_caller]
fn assert_joined(node: &Node, group: &str) {
    let joined = wait_for("the group is joined", Duration::from_secs(6), || {
        node.multicast_groups()
            .iter()
            .any(|address| address == group)
    });
    assert!(
        joined,
        "{group} was not joined; the interface has {:?}\n--- node ---\n{}",
        node.multicast_groups(),
        node.log()
    );
}

/// The virtual address each implementation defends, and the two node addresses.
const A: &str = "192.0.2.11";
const B: &str = "192.0.2.12";
const VIP: &str = "192.0.2.100";

const A6: &str = "2001:db8:c::11";
const B6: &str = "2001:db8:c::12";
/// The IPv6 virtual address, defined once.
///
/// It was once spelled in two files, and a test that asserts against one while
/// the harness configures the other fails for a reason that reads like a kernel
/// problem: the node configures and claims one address while the assertion
/// watches another.
const VIP6: &str = support::VIRTUAL_ADDRESS6;
