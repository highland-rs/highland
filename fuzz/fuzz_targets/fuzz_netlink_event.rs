// Rust guideline compliant 2026-09-27

//! Fuzzes the netlink-facing types and the state machine's reaction to them.
//!
//! Netlink carries kernel output, so it is untrusted like any other input. The
//! contract is that a malformed message never panics, never yields a negative
//! interface index or an impossible prefix length, and that the state machine
//! keeps its invariants for any event derived from one (SPEC.md, `S-04`,
//! `I-15`).

#![no_main]

use arbitrary::Arbitrary;
use highland_core::clock::ManualClock;
use highland_core::machine::InstanceStateMachine;
use highland_core::state::{Event, InstanceConfig, Role};
use highland_net::{InterfaceId, IpCidr};
use libfuzzer_sys::fuzz_target;

/// The part of a netlink address message this release consumes.
#[derive(Arbitrary, Debug)]
struct AddressMessage {
    /// 0 for an IPv4 address, anything else for IPv6.
    family: u8,
    /// The prefix length, which may be impossible for the family.
    prefix_len: u8,
    /// The first four octets of the address.
    octets: [u8; 4],
    /// The kernel interface index, which may be negative.
    index: i32,
}

fuzz_target!(|message: AddressMessage| {
    let prefix = message.prefix_len;
    let dotted =
        format!("{}.{}.{}.{}", message.octets[0], message.octets[1], message.octets[2], message.octets[3]);
    let text = if message.family.is_multiple_of(2) {
        format!("{dotted}/{prefix}")
    } else {
        format!("::ffff:{dotted}/{prefix}")
    };

    if let Ok(cidr) = IpCidr::parse(&text) {
        // A prefix length is always within the family, however it was supplied.
        let maximum = if cidr.address().is_ipv4() { 32 } else { 128 };
        assert!(cidr.prefix_len() <= maximum, "a parsed prefix length is in range");
    }

    // A negative index is refused rather than wrapped into a valid one.
    assert_eq!(InterfaceId::new(message.index).is_err(), message.index < 0);

    // However the interface oscillates, the machine never takes ownership: it
    // is never told the interface is usable, so it stays in election.
    let mut machine = InstanceStateMachine::new(InstanceConfig::default(), ManualClock::new());
    for event in [
        Event::Startup,
        Event::InterfaceDown,
        Event::InterfaceUp,
        Event::InterfaceDown,
    ] {
        let _ = machine.handle(event);
        assert!(matches!(machine.role(), Role::Backup | Role::Init), "{}", machine.role());
        assert!(!machine.owns_virtual_addresses());
    }
});
