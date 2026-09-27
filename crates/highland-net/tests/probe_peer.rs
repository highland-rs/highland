// Rust guideline compliant 2026-09-27

//! A diagnostic that answers one question with bytes rather than inference.
//!
//! Point it at an interface and it prints every VRRP frame it sees, whole,
//! with the IPv4 header intact — which is the only way to tell "the peer
//! computed the checksum over a different scope" from "the peer computed it
//! wrong" without a packet analyser.
//!
//! ```console
//! HIGHLAND_PROBE_INTERFACE=eth0 cargo test -p highland-net \
//!     --features netlink-tests --test probe_peer -- --nocapture
//! ```
//!
//! With no interface set it says so and passes, so a contributor without a peer
//! is not blocked by it.

#![cfg(all(target_os = "linux", feature = "netlink-tests"))]

use std::time::Duration;

use highland_net::frames;

/// The VRRP protocol number, IP protocol 112.
const VRRP: u8 = 112;

#[test]
fn every_vrrp_frame_on_an_interface_can_be_dumped() {
    let Ok(interface) = std::env::var("HIGHLAND_PROBE_INTERFACE") else {
        eprintln!("no HIGHLAND_PROBE_INTERFACE set: nothing to probe");
        return;
    };
    let budget: u64 = std::env::var("HIGHLAND_PROBE_BUDGET_MS")
        .ok()
        .and_then(|text| text.parse().ok())
        .unwrap_or(5_000);

    let socket = frames::open(&interface).expect("the capture opens");
    let seen =
        frames::read(&socket, 6, Duration::from_millis(budget)).expect("the read does not fail");

    for (index, frame) in seen.iter().enumerate() {
        let payload = frame.payload();
        println!(
            "--- frame {index} ether_type={:#06x} ip_proto={} ---",
            frame.ether_type(),
            frame.ip_protocol()
        );
        println!("frame  {}", frames::hex(&frame.bytes));
        if frame.ip_protocol() == VRRP {
            println!("vrrp   {}", frames::hex(payload));
            let (sum, folded) = ones_complement(payload);
            println!(
                "sum={sum:#06x} folded={folded:#06x} (a valid message checksum folds to 0xffff)"
            );
        }
    }
    println!("{} frames", seen.len());
}

/// The RFC 1071 sum over `bytes`, and the folded value.
///
/// # Safety
///
/// Not applicable: this is a plain function over a plain slice.
fn ones_complement(bytes: &[u8]) -> (u32, u32) {
    let mut sum: u32 = 0;
    let mut index = 0;
    while index + 1 < bytes.len() {
        sum += u32::from(u16::from_be_bytes([bytes[index], bytes[index + 1]]));
        index += 2;
    }
    if index < bytes.len() {
        sum += u32::from(bytes[index]) << 8;
    }
    let mut folded = sum;
    while folded > 0xffff {
        folded = (folded & 0xffff) + (folded >> 16);
    }
    (sum, folded)
}
