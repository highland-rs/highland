// Rust guideline compliant 2026-09-27

//! Fuzzes the IPv6 VRRP decoder.
//!
//! The contract is that untrusted bytes never panic, never drive an allocation,
//! and never yield a packet whose length disagrees with its own count
//! (SPEC.md, `I-05`, `S-04`, `S-07`).

#![no_main]

use highland_vrrp::{Advertisement, ChecksumScope, IpFamily, Peek};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // A receiver reads the fixed header before it trusts the length, so peeking
    // must be safe on anything, including a count of 255 in a 12-byte buffer.
    let header = Peek::read(data, IpFamily::V6);
    if let Ok(header) = header {
        assert_eq!(header.message_len, IpFamily::V6.message_len(header.count));
    }

    // The checksum is verified before the addresses are decoded.
    if let Ok(advertisement) = Advertisement::decode(data, IpFamily::V6, ChecksumScope::MessageOnly) {
        assert!(!advertisement.addresses().is_empty(), "RFC 5798 5.2.5");
        assert_eq!(data.len(), IpFamily::V6.message_len(declared_count(data)));
        for address in advertisement.addresses() {
            assert!(IpFamily::V6.matches(address), "RFC 5798 5.2.9");
        }
    }

    // And the verified path, which is what the state machine consumes.
    if let Ok(advertisement) = Advertisement::decode_verified(data, IpFamily::V6) {
        assert!(advertisement.vrid().get() >= 1, "RFC 5798 5.2.3");
        assert!(!advertisement.addresses().is_empty());
    }
});

/// Returns the address count a message declares, or zero if it is too short.
fn declared_count(data: &[u8]) -> u8 {
    data.get(3).copied().unwrap_or(0)
}
