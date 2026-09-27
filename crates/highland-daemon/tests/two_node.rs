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

use support::{A_ADDRESS, B_ADDRESS, BridgeGuard, Node, VIRTUAL_ADDRESS as VIP, bridge, wait_for};

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
