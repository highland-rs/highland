// Rust guideline compliant 2026-09-27

//! Fuzzes configuration parsing.
//!
//! The contract is that a malformed document produces a typed, self-explaining
//! error and never a panic, and that a document which loads is a document that
//! was fully validated (SPEC.md, `I-08`, `S-07`).

#![no_main]

use highland_config::{ValidationContext, load_and_validate};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Arbitrary bytes need not be UTF-8; a rejected encoding is an outcome, not
    // a crash.
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };

    match load_and_validate(text, &ValidationContext::permissive()) {
        Ok(config) => {
            // Everything the machine relies on is enforced by validation, so a
            // document that loaded satisfies it by construction.
            for instance in &config.instances {
                assert!(instance.vrid >= 1, "V-01");
                assert!(instance.priority >= 1, "V-02");
                assert!(!instance.vips.is_empty(), "V-11");
                assert!(instance.preempt || instance.preempt_delay.is_zero(), "V-10");
            }
            if !config.node.name.trim().is_empty() {
                assert!(!config.instances.is_empty(), "V-31");
            }
        }
        Err(error) => {
            assert!(!error.to_string().is_empty(), "an operator has to be able to read it");
        }
    }
});
