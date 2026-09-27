// Rust guideline compliant 2026-09-27

//! Two daemons, one segment, and a VIP that moves.
//!
//! This is the Milestone 4 exit criterion (`M-04`): a single-instance daemon
//! fails over a VIP against a second node in a namespace. The topology is the
//! one a VRRP segment actually is, rather than a simulation of one:
//!
//! ```text
//!   netns hla ── hlap ──┬── hlbr0 ──┬── hlbp ── netns hlb
//!                       bridge     (192.0.2.11/24, .12/24)
//! ```
//!
//! Two namespaces mean the two nodes cannot see each other's kernel state, so
//! everything that passes between them went over a socket: the advertisement,
//! and the address that moves.
//!
//! It needs `CAP_NET_ADMIN` and `CAP_NET_RAW` to build the topology and to move
//! an address, so it is behind `netlink-tests` and run by
//! `scripts/linux-tests.sh`.
//!
//! The timing assertions are the point of the exercise. A failover that
//! "works" but takes ten seconds is not a failover, so each deadline below is
//! `SPEC.md` §13.3's budget and not a number chosen to make the test pass.

#![cfg(all(target_os = "linux", feature = "netlink-tests"))]

mod support;

use std::time::Duration;

use support::{
    A_ADDRESS, B_ADDRESS, BridgeGuard, Node, Observer, VIRTUAL_ADDRESS, VIRTUAL_ADDRESS as VIP,
    VIRTUAL_ADDRESS6, bridge, wait_for, wait_stable,
};

/// `Master_Down_Interval` for a one-second interval at priority 150 (SPEC.md §13.3).
const MASTER_DOWN_BUDGET: Duration = Duration::from_millis(3600);

#[test]
fn two_nodes_elect_one_master_and_the_vip_moves_when_it_dies() {
    // A bridge in this namespace, with both nodes' veth ends attached to it.
    let guard: BridgeGuard = bridge();
    let bridge = guard.name.clone();

    let _bridge_guard = guard;
    let mut first = Node::create("a", A_ADDRESS, &bridge, false);
    let mut second = Node::create("b", B_ADDRESS, &bridge, false);

    first.start();
    second.start();

    // Exactly one node takes the address. The other must not, or both nodes
    // believe they are master, which is the split brain the specification names.
    let one_master = wait_for("one node holds the VIP", Duration::from_secs(10), || {
        usize::from(first.holds_vip()) + usize::from(second.holds_vip()) == 1
    });
    assert!(
        one_master,
        "exactly one node must hold {VIP}: a={:?} b={:?}\n--- a ---\n{}\n--- b ---\n{}",
        first.addresses(),
        second.addresses(),
        first.log(),
        second.log()
    );

    let first_was_master = first.holds_vip();

    // Kill the master. The survivor must take the address within
    // `Master_Down_Interval`, not eventually.
    if first_was_master {
        first.kill();
        let moved = wait_for("the VIP moved to the survivor", MASTER_DOWN_BUDGET, || {
            second.holds_vip()
        });
        assert!(
            moved,
            "the VIP did not move within {MASTER_DOWN_BUDGET:?}: a={:?} b={:?}",
            first.addresses(),
            second.addresses()
        );
        assert!(
            !first.holds_vip(),
            "the dead node cannot still hold the address"
        );
    } else {
        second.kill();
        let moved = wait_for("the VIP moved to the survivor", MASTER_DOWN_BUDGET, || {
            first.holds_vip()
        });
        assert!(
            moved,
            "the VIP did not move within {MASTER_DOWN_BUDGET:?}: a={:?} b={:?}",
            first.addresses(),
            second.addresses()
        );
    }
}

/// The announcement is the part of a takeover a client actually notices.
///
/// Without it, every neighbour keeps a cache entry pointing at the node that had
/// the address last, and traffic to the VIP is a black hole until that entry ages
/// out — tens of seconds, against a failover measured in milliseconds. So this
/// asserts the *effect*, not the send: a third namespace on the same segment
/// resolves the address to the old master, the master is killed, and within a
/// second or two the neighbour cache must point at the new one.
///
/// A `sendto` that returned `Ok` would prove nothing, and a capture would prove
/// only that a frame existed. A neighbour that changed its mind is the thing
/// clients depend on.
#[test]
fn a_takeover_announces_the_address_to_the_segment() {
    let bridge = bridge();
    let name = bridge.name.clone();
    let _bridge_guard = bridge;
    let mut first = Node::create("a", A_ADDRESS, &name, false);
    let mut second = Node::create("b", B_ADDRESS, &name, false);
    let observer = Observer::create("obs", "192.0.2.13", &name);

    first.start();
    second.start();

    // Settled, not merely elected: two nodes whose timers expire together both
    // take the address until the tie is broken, and this test needs to know which
    // one the segment will end up with before it reads the neighbour's cache.
    let elected = wait_stable("one node holds the VIP", Duration::from_secs(2), || {
        usize::from(first.holds_vip()) + usize::from(second.holds_vip()) == 1
    });
    assert!(
        elected,
        "the election never settled on one node\n--- a ---\n{}\n--- b ---\n{}",
        first.log(),
        second.log()
    );

    let (mut master, other) = if first.holds_vip() {
        (first, second)
    } else {
        (second, first)
    };
    let old = master.hardware_address();

    // The observer resolves the address first, so it has a cache entry pointing
    // at the old master before anything goes wrong.
    let before = observer.resolve(VIP);
    assert_eq!(
        before, old,
        "the observer should have resolved {VIP} to the master that holds it"
    );

    master.kill();
    let moved = wait_for("the VIP moved", MASTER_DOWN_BUDGET, || other.holds_vip());
    assert!(
        moved,
        "the address did not move\n--- survivor ---\n{}",
        other.log()
    );

    // The announcement is what changes the cache, and it has to do so quickly:
    // an entry that stays wrong for the rest of its lifetime is the failure this
    // exists to catch, and the entry's own lifetime is tens of seconds.
    let new = other.hardware_address();
    let announced = wait_for(
        "the neighbour learned the address moved",
        Duration::from_secs(3),
        || observer.neighbour(VIP) == new,
    );
    assert!(
        announced,
        "no neighbour was told {VIP} moved: the cache points at {} instead of {new}\n--- survivor ---\n{}",
        observer.neighbour(VIP),
        other.log()
    );
}

// ----- multicast, and the other address family ---------------------------

/// A pair of nodes in multicast mode, with the family, the virtual address, and
/// the peering mode under the test's control.
///
/// `vip` is the address they fight over, `prefix` the length for both it and
/// each node's own address, and `group` the address each must have joined by the
/// time the election settles.
fn multicast_pair(
    namespace: &str,
    addresses: [&'static str; 2],
    vip: &'static str,
    prefix: u8,
) -> (Node, Node) {
    let guard = bridge();
    let name = guard.name.clone();
    let nodes = (
        Node::create_full(
            &format!("{namespace}a"),
            addresses[0],
            &name,
            false,
            150,
            vip,
            prefix,
            None,
        ),
        Node::create_full(
            &format!("{namespace}b"),
            addresses[1],
            &name,
            false,
            150,
            vip,
            prefix,
            None,
        ),
    );
    // The guard is forgotten rather than dropped: the bridge has to outlive the
    // nodes, and dropping it first would take the veth ends with it.
    std::mem::forget(guard);
    nodes
}

/// Asserts that a node has joined the group multicast mode is for.
///
/// Read from the kernel, because a configuration that says `multicast` and a
/// daemon that never joined the group look identical in the log. If the group
/// was never joined, nothing on the segment is listening and the election below
/// would pass for the wrong reason: two nodes that hear nothing both time out.
fn assert_joined(node: &Node, group: &str) {
    let joined = wait_for(
        "the multicast group is joined",
        Duration::from_secs(5),
        || {
            node.multicast_groups()
                .iter()
                .any(|address| address == group)
        },
    );
    assert!(
        joined,
        "{group} was not joined; the interface has {:?}\n--- node ---\n{}",
        node.multicast_groups(),
        node.log()
    );
}

/// The election, the address, and the failover, in multicast mode over IPv4.
///
/// The point is not the election, which the unicast suite already proves. The
/// point is that the group was joined, that two nodes reaching each other only
/// through a group elect one master, and that the address still moves inside the
/// budget.
#[test]
fn two_nodes_in_ipv4_multicast_mode_elect_one_master_and_fail_over() {
    let (mut first, mut second) = multicast_pair("mc", [A_ADDRESS, B_ADDRESS], VIRTUAL_ADDRESS, 24);
    first.start();
    second.start();

    assert_joined(&first, "224.0.0.18");
    assert_joined(&second, "224.0.0.18");

    let elected = wait_stable("one node holds the VIP", Duration::from_secs(2), || {
        usize::from(first.holds_vip()) + usize::from(second.holds_vip()) == 1
    });
    assert!(
        elected,
        "multicast mode elected no master: a={:?} b={:?}\n--- a ---\n{}\n--- b ---\n{}",
        first.addresses(),
        second.addresses(),
        first.log(),
        second.log()
    );

    // Whichever node won, killing it must hand the address over inside the
    // budget, and the survivor must be the one holding it.
    let (master, survivor) = if first.holds_vip() {
        (&mut first, &second)
    } else {
        (&mut second, &first)
    };
    master.kill();
    let moved = wait_for("the VIP moved", MASTER_DOWN_BUDGET, || survivor.holds_vip());
    assert!(
        moved,
        "multicast mode did not fail over\n--- survivor ---\n{}",
        survivor.log()
    );
}

/// The same over IPv6, which is the other half of `G-01`.
///
/// IPv6 unicast is the case that had never been run against a kernel, and the
/// reason it is worth a test of its own: an IPv6 advertisement's checksum covers
/// the IPv6 pseudo-header, so a sender or receiver that guesses the destination
/// produces packets that are correct in every field and rejected by the peer.
#[test]
fn two_nodes_in_ipv6_unicast_mode_elect_one_master_and_fail_over() {
    let guard = bridge();
    let name = guard.name.clone();
    let (mut first, mut second) = (
        Node::create_full(
            "v6a",
            "2001:db8:a::11",
            &name,
            false,
            150,
            VIRTUAL_ADDRESS6,
            64,
            Some("2001:db8:a::12"),
        ),
        Node::create_full(
            "v6b",
            "2001:db8:a::12",
            &name,
            false,
            150,
            VIRTUAL_ADDRESS6,
            64,
            Some("2001:db8:a::11"),
        ),
    );
    let _bridge_guard = guard;
    first.start();
    second.start();

    let elected = wait_stable("one node holds the VIP", Duration::from_secs(2), || {
        usize::from(first.holds(VIRTUAL_ADDRESS6)) + usize::from(second.holds(VIRTUAL_ADDRESS6))
            == 1
    });
    assert!(
        elected,
        "IPv6 unicast elected no master: a={:?} b={:?}\n--- a ---\n{}\n--- b ---\n{}",
        first.addresses_of(VIRTUAL_ADDRESS6),
        second.addresses_of(VIRTUAL_ADDRESS6),
        first.log(),
        second.log()
    );

    let (master, survivor) = if first.holds(VIRTUAL_ADDRESS6) {
        (&mut first, &second)
    } else {
        (&mut second, &first)
    };
    let old = master.hardware_address();
    master.kill();
    let moved = wait_for("the IPv6 VIP moved", MASTER_DOWN_BUDGET, || {
        survivor.holds(VIRTUAL_ADDRESS6)
    });
    assert!(
        moved,
        "the IPv6 address did not move\n--- survivor ---\n{}",
        survivor.log()
    );
    assert_ne!(
        survivor.hardware_address(),
        old,
        "the survivor is a different node, which is the point"
    );
}

/// IPv6 over multicast, which needs the group, the hop limit, and the scope that
/// a link-local destination has to carry.
#[test]
fn two_nodes_in_ipv6_multicast_mode_elect_one_master_and_fail_over() {
    let (mut first, mut second) = multicast_pair(
        "v6m",
        ["2001:db8:b::11", "2001:db8:b::12"],
        VIRTUAL_ADDRESS6,
        64,
    );
    first.start();
    second.start();

    assert_joined(&first, "ff02::12");
    assert_joined(&second, "ff02::12");

    let elected = wait_stable("one node holds the VIP", Duration::from_secs(2), || {
        usize::from(first.holds(VIRTUAL_ADDRESS6)) + usize::from(second.holds(VIRTUAL_ADDRESS6))
            == 1
    });
    assert!(
        elected,
        "IPv6 multicast elected no master: a={:?} b={:?}\n--- a ---\n{}\n--- b ---\n{}",
        first.addresses_of(VIRTUAL_ADDRESS6),
        second.addresses_of(VIRTUAL_ADDRESS6),
        first.log(),
        second.log()
    );

    let (master, survivor) = if first.holds(VIRTUAL_ADDRESS6) {
        (&mut first, &second)
    } else {
        (&mut second, &first)
    };
    master.kill();
    let moved = wait_for("the IPv6 VIP moved", MASTER_DOWN_BUDGET, || {
        survivor.holds(VIRTUAL_ADDRESS6)
    });
    assert!(
        moved,
        "the IPv6 multicast address did not move\n--- survivor ---\n{}",
        survivor.log()
    );
}

/// A failing check takes the address away, and says why.
///
/// This is the whole chain in one test, and it is the thing a health check is
/// for: a node that is master while the service behind it is gone will sit
/// there advertising ownership to every client that tries the address, and every
/// one of them fails. The demotion is weighted — the node's effective priority
/// drops below its peer's — so the address moves, and the event says which check
/// and what it said.
#[test]
fn a_failing_check_demotes_the_node_and_says_why() {
    let bridge = bridge();
    let name = bridge.name.clone();
    let _bridge_guard = bridge;

    // A TCP check against a port nothing will listen on, on the higher-priority
    // node. Thresholds of one so the demotion is not a function of timing.
    let checks = r#"
[[instance.check]]
name = "api"
type = "tcp"
weight = 100
interval = "200ms"
timeout = "200ms"
failure_threshold = 1
success_threshold = 1
address = "192.0.2.99:9"
"#;

    let mut first = Node::create_with_checks(
        "hca",
        A_ADDRESS,
        &name,
        false,
        150,
        VIRTUAL_ADDRESS,
        24,
        Some(B_ADDRESS),
        checks,
    );
    // The peer preempts. That is the whole configuration question: a demoted
    // master does not give the address up by itself — RFC 5798 has it keep
    // advertising, and a backup only takes over from a live master when
    // preemption is enabled. A deployment that wants a failing check to move the
    // address has to say so here, and the test says so.
    let mut second = Node::create_with_priority("hcb", B_ADDRESS, &name, true, 100);
    first.start();
    second.start();

    // The checked node must not end up holding the address: its own check is
    // failing from the first probe, so its effective priority is 150 - 100 = 50,
    // below the peer's 100, and the peer takes over.
    let moved = wait_for(
        "the failing check handed the address to the peer",
        Duration::from_secs(15),
        || second.holds_vip() && !first.holds_vip(),
    );
    assert!(
        moved,
        "a failing check did not demote the node: a={:?} b={:?}\n--- checked ---\n{}\n--- peer ---\n{}",
        first.addresses(),
        second.addresses(),
        first.log(),
        second.log()
    );

    // And the reason is in the log, because a demotion nobody can explain is a
    // demotion nobody can act on (`D-08`). What is asserted is the *check*
    // saying what it saw: the address moving is the machine's decision, and the
    // test above already covers that this node is no longer holding it.
    let log = first.log();
    assert!(
        log.contains("the probe did not answer") || log.contains("could not connect"),
        "the check said what it saw:\n{log}"
    );
    assert!(
        log.contains("from=Unknown to=Failing") || log.contains("to=Failing"),
        "and the verdict it reached:\n{log}"
    );
}

/// A check that cannot be built is a configuration error, and the node says so
/// rather than running a probe that cannot work.
///
/// The alternative — a check that fails on every interval forever — is a quieter
/// way to take a node out of service than a refusal at startup, and the refusal
/// is what an operator needs.
#[test]
fn a_check_this_build_cannot_run_is_refused_by_name() {
    let bridge = bridge();
    let name = bridge.name.clone();
    let _bridge_guard = bridge;

    let checks = r#"
[[instance.check]]
name = "secure"
type = "https"
weight = 50
interval = "1s"
timeout = "1s"
failure_threshold = 1
success_threshold = 1
url = "https://192.0.2.10/health"
expected_status = [200]
"#;
    let mut node = Node::create_with_checks(
        "hcc",
        A_ADDRESS,
        &name,
        false,
        150,
        VIRTUAL_ADDRESS,
        24,
        None,
        checks,
    );
    node.start();

    // The daemon starts — an unbuildable check is not fatal to the node — and
    // says which check and why, once, rather than every interval.
    let said = wait_for(
        "the unbuildable check is reported",
        Duration::from_secs(10),
        || {
            let log = node.log();
            log.contains("cannot be built") && log.contains("secure")
        },
    );
    assert!(said, "the reason was not reported:\n{}", node.log());
    let log = node.log();
    assert!(
        log.contains("TLS") || log.contains("https"),
        "and it names why https is refused rather than downgraded:\n{log}"
    );
    assert_eq!(
        log.matches("cannot be built").count(),
        1,
        "said once, not once per interval:\n{log}"
    );
}
