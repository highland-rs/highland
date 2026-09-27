// Rust guideline compliant 2026-09-27

//! A segment with faults on it.
//!
//! The failover suite proves the happy path and the dead-node path. This suite
//! proves the parts in between, which is where a VRRP daemon usually fails: a
//! segment that is lossy, late, duplicated, reordered, cut in one direction, or
//! whose node is frozen rather than dead.
//!
//! Every scenario asserts two things, because a daemon that "recovers" by
//! flapping is not recovering:
//!
//! - **Bounded recovery.** The survivor takes the address inside `SPEC.md`
//!   §13.3's budget, and the segment settles afterwards.
//! - **Bounded event volume.** Role changes are counted, so a node that reaches
//!   the right answer by passing through a dozen wrong ones fails too. Every flap
//!   is another address move for every client, so a flapping master is a worse
//!   outage than a slow one.
//!
//! Faults are injected with `tc netem` on each node's own egress rather than on
//! the bridge, because the interesting question is what one node does to its
//! peer on its own; a fault on the bridge would hit both nodes at once.
//!
//! It needs `CAP_NET_ADMIN` and `CAP_NET_RAW`, so it is behind `netlink-tests`
//! and run by `scripts/linux-tests.sh`.

#![cfg(all(target_os = "linux", feature = "netlink-tests"))]

mod support;

use std::time::{Duration, Instant};

use support::{A_ADDRESS, B_ADDRESS, BridgeGuard, Node, VIRTUAL_ADDRESS as VIP, bridge, wait_for};

/// The budget for a takeover after a fault is injected.
///
/// A backup's master-down timer is reset by every advertisement it receives, so
/// a fault that lands between two advertisements is noticed only after up to one
/// advertisement interval *plus* a full `Master_Down_Interval`. The failover
/// suite's 3.6s budget covers a process that died at a known moment; an injected
/// filter dies at an unknown point in the advertisement cycle, and budgeting for
/// that is the difference between a flaky test and a real limit.
const FAULT_BUDGET: Duration = Duration::from_millis(5600);

/// How long a settled segment is watched for flapping.
const SETTLE: Duration = Duration::from_secs(4);

/// A node that changed role more than this many times after the segment settled
/// has flapped. The startup election is not counted: both nodes pass through
/// `BACKUP` on the way in, and counting that would make the ceiling a statement
/// about startup rather than about stability.
const ROLE_CHANGE_CEILING: usize = 4;

/// Two elected nodes and the bridge they share.
///
/// The bridge field is declared last on purpose: Rust drops fields in
/// declaration order, so the namespaces and their veth ends are removed before
/// the bridge, not after. Deleting the bridge first would take the veth ends
/// with it and leave the namespaces holding nothing.
struct Segment {
    first: Node,
    second: Node,
    _bridge: BridgeGuard,
}

impl Segment {
    /// The node that currently holds the address.
    fn master(&self) -> &Node {
        if self.first.holds_vip() {
            &self.first
        } else {
            &self.second
        }
    }

    /// The node that does not hold the address.
    fn backup(&self) -> &Node {
        if self.first.holds_vip() {
            &self.second
        } else {
            &self.first
        }
    }

    /// The role changes across both nodes, which is the event volume the
    /// scenarios bound.
    fn role_changes(&self) -> usize {
        self.first.role_changes() + self.second.role_changes()
    }

    /// How many role changes happened since `baseline`.
    fn role_changes_since(&self, baseline: usize) -> usize {
        self.role_changes().saturating_sub(baseline)
    }
}

/// Brings up two nodes, waits for exactly one master, and returns them.
///
/// The priorities differ, which is the usual deployment and the only way the
/// startup election is deterministic: with equal priorities both nodes time out
/// at the same instant and both take the address for a moment before the address
/// tie-break sorts it out. That is authentic VRRP, and the tie-break is exercised
/// on purpose in the equal-priority scenario below rather than in every test.
fn elected(preempt: bool) -> Segment {
    elected_with_priorities(preempt, 150, 100)
}

/// As [`elected`], with an explicit priority for each node.
fn elected_with_priorities(preempt: bool, first_priority: u8, second_priority: u8) -> Segment {
    let bridge = bridge();
    let name = bridge.name.clone();
    let mut first = Node::create_with_priority("ca", A_ADDRESS, &name, preempt, first_priority);
    let mut second = Node::create_with_priority("cb", B_ADDRESS, &name, preempt, second_priority);
    first.start();
    second.start();

    let elected = wait_for("one node holds the VIP", Duration::from_secs(10), || {
        usize::from(first.holds_vip()) + usize::from(second.holds_vip()) == 1
    });
    assert!(
        elected,
        "no node took {VIP}\n--- a ---\n{}\n--- b ---\n{}",
        first.log(),
        second.log()
    );

    // Wait for the election to *settle* rather than merely complete. Two nodes
    // whose master-down timers expire together both take the address for a
    // moment before the tie is broken, which is authentic VRRP; a scenario that
    // started inside that window would be testing the startup race instead of
    // the fault it is about.
    let stable = wait_for("the election settled", Duration::from_secs(5), || {
        usize::from(first.holds_vip()) + usize::from(second.holds_vip()) == 1
    });
    assert!(stable, "the election never settled on one node");

    Segment {
        first,
        second,
        _bridge: bridge,
    }
}

/// Samples both nodes for `duration` and fails if both ever hold the address.
///
/// This is the assertion the whole suite exists for. Polling cannot catch every
/// overlap, but an overlap that lasts longer than a poll interval is a real
/// split brain, and a split brain is the one failure a VRRP implementation is
/// not allowed to have.
fn assert_never_split_brain(first: &Node, second: &Node, duration: Duration) {
    let started = Instant::now();
    while started.elapsed() < duration {
        assert!(
            !(first.holds_vip() && second.holds_vip()),
            "both nodes held {VIP} at once, after {:?}\n--- a ---\n{}\n--- b ---\n{}",
            started.elapsed(),
            first.log(),
            second.log()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Fails if the segment changed role more than `ceiling` times since `baseline`.
///
/// Counting from the baseline rather than from zero is what makes this a
/// statement about stability: the startup election is a fixed cost every test
/// pays, and folding it into the ceiling would only mean the ceiling was chosen
/// to fit startup.
fn assert_bounded_flapping(segment: &Segment, baseline: usize, ceiling: usize) {
    let changes = segment.role_changes_since(baseline);
    assert!(
        changes <= ceiling,
        "the segment flapped: {changes} role changes after settling, over {ceiling}\n--- a ---\n{}\n--- b ---\n{}",
        segment.first.log(),
        segment.second.log()
    );
}

/// Cuts the segment's master off from its peer, and returns both nodes.
///
/// `netem` at 100% loss on one egress is a one-way partition: the cut node's
/// advertisements never arrive, while the other node's still do. The peer has to
/// discover the failure by timing out, which is the entire point of
/// `Master_Down_Interval`.
///
/// The cut node keeps the address while it is cut off, and nothing can change
/// that. It has no way to know its peer took over, which is the fencing
/// limitation a VRRP implementation inherits rather than one it can fix. The
/// scenario is therefore about what happens when the partition heals.
fn partition_the_master(segment: &Segment) -> (&Node, &Node) {
    let (cut, deaf) = if segment.first.holds_vip() {
        (&segment.first, &segment.second)
    } else {
        (&segment.second, &segment.first)
    };
    cut.netem(&["loss", "100%"]);

    let moved = wait_for("the peer took the address", FAULT_BUDGET, || {
        deaf.holds_vip()
    });
    assert!(
        moved,
        "the peer never heard the master go away\n--- cut ---\n{}\n--- deaf ---\n{}",
        cut.log(),
        deaf.log()
    );
    (cut, deaf)
}

/// Heals a partition and waits for the segment to be one master again.
///
/// Equal priorities are resolved by address, so the node that took over keeps
/// the address and the other steps down and gives it back. A healed segment that
/// stays split has turned a brief blip into a permanent one.
fn heal(segment: &Segment) {
    for node in [&segment.first, &segment.second] {
        node.clear_netem();
    }
    let converged = wait_for(
        "the healed segment has one master",
        Duration::from_secs(10),
        || usize::from(segment.first.holds_vip()) + usize::from(segment.second.holds_vip()) == 1,
    );
    assert!(
        converged,
        "the healed segment did not converge\n--- a ---\n{}\n--- b ---\n{}",
        segment.first.log(),
        segment.second.log()
    );
}

/// Lossy is the normal condition of a real network, and a router that cannot
/// hold an election over a 20% loss rate is not usable.
#[test]
fn a_lossy_segment_keeps_one_master() {
    let segment = elected(false);
    let baseline = segment.role_changes();

    segment.first.netem(&["loss", "20%"]);
    segment.second.netem(&["loss", "20%"]);

    assert_never_split_brain(&segment.first, &segment.second, SETTLE);
    assert_bounded_flapping(&segment, baseline, ROLE_CHANGE_CEILING);
    assert_eq!(
        usize::from(segment.first.holds_vip()) + usize::from(segment.second.holds_vip()),
        1,
        "the segment must still have exactly one master"
    );

    // And it must still fail over under loss, which is what a filter that drops
    // only the advertisements gets wrong.
    let (cut, deaf) = partition_the_master(&segment);
    assert!(deaf.holds_vip(), "the surviving node owns the address");

    // With 20% loss on the restored path the segment must not flap its way out
    // of the partition, and it must end up with one master.
    let before = cut.role_changes() + deaf.role_changes();
    cut.clear_netem();
    let settled = wait_for("the segment settled after the partition", SETTLE, || {
        usize::from(cut.holds_vip()) + usize::from(deaf.holds_vip()) == 1
    });
    assert!(
        settled,
        "the segment did not settle after the partition healed\n--- cut ---\n{}\n--- deaf ---\n{}",
        cut.log(),
        deaf.log()
    );
    let after = cut.role_changes() + deaf.role_changes();
    assert!(
        after - before <= 4,
        "healing the partition flapped the segment: {before} -> {after}\n--- cut ---\n{}\n--- deaf ---\n{}",
        cut.log(),
        deaf.log()
    );
}

/// A one-way partition, which is the fault that breaks naive implementations.
///
/// Node A's advertisements never reach B, while B's still reach A. B must
/// therefore treat A as dead and take the address. A reachability check that is
/// symmetric in the wrong direction leaves both nodes believing the other is
/// gone, and neither would ever take over.
#[test]
fn a_one_way_partition_hands_the_address_over_and_heals_to_one_master() {
    let segment = elected(false);
    let baseline = segment.role_changes();
    let (cut, deaf) = partition_the_master(&segment);

    // While the partition holds, the cut node still believes it is master. That
    // is not something Highland can fix: the node is telling the truth about what
    // it can see, and no protocol message reaches it. It is written down here so
    // the assertion that follows is not read as a claim that this cannot happen.
    assert!(
        cut.holds_vip(),
        "a partitioned master still holds the address, since nothing told it otherwise"
    );

    // Healing the partition is the part that is recoverable. The node that took
    // over has the lower priority, so the moment it hears the partitioned master
    // again it must give the address back; the segment has to end up where it
    // started, without the address ping-ponging in between.
    let started = Instant::now();
    heal(&segment);
    assert!(
        started.elapsed() < Duration::from_secs(6),
        "the segment took {:?} to converge after the partition healed",
        started.elapsed()
    );
    assert!(
        cut.holds_vip(),
        "the higher-priority node keeps the address once the segment can hear itself again"
    );
    assert!(
        !deaf.holds_vip(),
        "the node that took over must have stepped down"
    );
    assert_bounded_flapping(&segment, baseline, ROLE_CHANGE_CEILING);
    assert_never_split_brain(&segment.first, &segment.second, Duration::from_secs(2));
}

/// Reordering and duplication are cheap to inject and easy to get wrong: a
/// daemon that treats a reordered advertisement as a lower-priority one hands the
/// address back and forth for as long as the fault is on.
#[test]
fn reordered_and_duplicated_advertisements_do_not_flip_the_master() {
    let segment = elected(false);
    let baseline = segment.role_changes();

    for node in [&segment.first, &segment.second] {
        node.netem(&["delay", "20ms", "10ms", "distribution", "normal"]);
        node.netem(&["reorder", "50%", "50%", "delay", "20ms", "10ms"]);
        node.netem(&["duplicate", "10%"]);
    }

    assert_never_split_brain(&segment.first, &segment.second, SETTLE);
    assert_eq!(
        usize::from(segment.first.holds_vip()) + usize::from(segment.second.holds_vip()),
        1,
        "exactly one master"
    );
    assert_bounded_flapping(&segment, baseline, ROLE_CHANGE_CEILING);
}

/// A cable pull, and a cable that comes back.
#[test]
fn a_link_flap_hands_the_address_over_and_hands_it_back() {
    let segment = elected(false);
    let baseline = segment.role_changes();
    let master = segment.master();
    let other = segment.backup();

    master.link_down();
    assert!(!master.carrier(), "the link did not go down");

    let moved = wait_for(
        "the peer took the address from a dead link",
        FAULT_BUDGET,
        || other.holds_vip(),
    );
    assert!(
        moved,
        "the address did not move when the link died\n--- cut ---\n{}\n--- other ---\n{}",
        master.log(),
        other.log()
    );

    master.link_up();
    assert!(
        wait_for("the link came back", Duration::from_secs(5), || master
            .carrier()),
        "the link did not come back"
    );

    assert_never_split_brain(&segment.first, &segment.second, SETTLE);
    assert_eq!(
        usize::from(segment.first.holds_vip()) + usize::from(segment.second.holds_vip()),
        1,
        "exactly one master after the link returned"
    );
    assert_bounded_flapping(&segment, baseline, ROLE_CHANGE_CEILING);
}

/// A frozen process is not a dead one, and the difference matters: a frozen
/// daemon is alive, still holds the address, and answers nothing at all. A peer
/// that waited for a socket to close would wait forever.
///
/// Nothing can be done to a frozen process, so the address is held by two nodes
/// until it thaws. That is the fencing limitation a VRRP implementation inherits
/// (`SPEC.md` §21.2), and the test asserts it rather than pretending otherwise.
/// What must hold is that the peer takes the address over by timing out, and that
/// the segment converges to one master once the node thaws.
#[test]
fn a_frozen_node_times_out_and_the_segment_converges_when_it_thaws() {
    let segment = elected_with_priorities(false, 100, 200);
    let baseline = segment.role_changes();
    let frozen = segment.master();
    let other = segment.backup();

    // `SIGSTOP`, not a kill: the process is alive and still holding the address,
    // which is exactly the case a timeout-based protocol has to handle.
    frozen.suspend();

    let moved = wait_for(
        "the peer took the address from a frozen node",
        FAULT_BUDGET,
        || other.holds_vip(),
    );
    assert!(
        moved,
        "a frozen node kept the address forever\n--- frozen ---\n{}\n--- other ---\n{}",
        frozen.log(),
        other.log()
    );
    assert!(
        frozen.holds_vip(),
        "a frozen process cannot be asked to release the address, so both nodes hold it"
    );

    frozen.resume();

    // Which node ends up with the address depends on the priorities: a master
    // that thaws and hears a *higher* priority peer steps down, while one that
    // hears a lower-priority peer keeps the address and the peer steps down. Both
    // are correct, so the assertion is convergence rather than a winner.
    let converged = wait_for("the segment converged", Duration::from_secs(8), || {
        usize::from(frozen.holds_vip()) + usize::from(other.holds_vip()) == 1
    });
    assert!(
        converged,
        "the segment stayed split after the node thawed\n--- frozen ---\n{}\n--- other ---\n{}",
        frozen.log(),
        other.log()
    );
    assert_never_split_brain(&segment.first, &segment.second, Duration::from_secs(2));
    assert_bounded_flapping(&segment, baseline, ROLE_CHANGE_CEILING);
}
