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
    A_ADDRESS, B_ADDRESS, BridgeGuard, Node, Observer, VIRTUAL_ADDRESS as VIP, bridge, wait_for,
    wait_stable,
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
