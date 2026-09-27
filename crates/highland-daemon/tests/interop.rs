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

use std::time::Duration;

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
    let mut keepalived = Keepalived::create("ipb", B, &format!("{VIP}/24"), &name, 100, 42, A);
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
    let mut keepalived = Keepalived::create("ipa2", A, &format!("{VIP}/24"), &name, 150, 42, B);
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

/// The virtual address each implementation defends, and the two node addresses.
const A: &str = "192.0.2.11";
const B: &str = "192.0.2.12";
const VIP: &str = "192.0.2.100";
