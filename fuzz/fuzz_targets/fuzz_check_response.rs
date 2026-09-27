// Rust guideline compliant 2026-09-27

//! Fuzzes the health-check result path.
//!
//! A check that talks to something the node does not control is fed by an
//! untrusted party, so the contract is that a result can never be built without
//! a reason, and that sequence numbers and generations keep a stale result from
//! displacing a newer one (SPEC.md, `D-08`, `I-12`, `I-26`).

#![no_main]

use std::time::{Duration, Instant};

use highland_checks::{CheckResult, CheckStatus, Debouncer, CheckKind, CheckSpec, Stability};
use highland_core::state::Generation;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() < 2 {
        return;
    }
    let sequence = u64::from(data[0]);
    let status = match data[1] % 4 {
        0 => CheckStatus::Passing,
        1 => CheckStatus::Failing,
        2 => CheckStatus::TimedOut,
        _ => CheckStatus::Disabled,
    };
    let generation = Generation::initial().next();

    let built = match status {
        CheckStatus::Passing => CheckResult::passing(
            "fuzz",
            Duration::from_millis(1),
            Instant::now(),
            generation,
            sequence,
            "probe accepted",
        ),
        _ => CheckResult::failing(
            "fuzz",
            None,
            Instant::now(),
            generation,
            sequence,
            "probe failed",
        ),
    };
    // Every probe that got this far produced a reason, so construction cannot
    // have failed for want of one.
    let earlier = built.expect("a non-empty reason always constructs");

    let newer = CheckResult::passing(
        "fuzz",
        Duration::from_millis(1),
        Instant::now(),
        generation,
        sequence.saturating_add(1),
        "probe accepted",
    )
    .expect("a non-empty reason always constructs");
    assert!(newer.supersedes(&earlier), "a newer sequence always wins");
    assert!(!earlier.supersedes(&newer));

    let stale_generation = CheckResult::passing(
        "fuzz",
        Duration::from_millis(1),
        Instant::now(),
        Generation::initial(),
        sequence,
        "probe accepted",
    )
    .expect("a non-empty reason always constructs");
    assert!(stale_generation.is_stale_for(generation), "I-12");

    // A timeout counts as a failure for threshold purposes.
    assert!(CheckStatus::TimedOut.is_failure());
    assert!(!CheckStatus::Disabled.is_failure());

    // The debouncer reaches a stable verdict and then stops changing.
    let spec = CheckSpec::new(
        "fuzz",
        CheckKind::Tcp,
        Duration::from_secs(1),
        Duration::from_millis(100),
        1 + u32::from(data[0] % 4),
        1 + u32::from(data[1] % 4),
        u16::from(data[0]) % 255,
    )
    .expect("the generated thresholds are positive");
    let mut debouncer = Debouncer::new(&spec);
    for _ in 0..16 {
        if debouncer.observe(status).is_some() {
            assert!(matches!(debouncer.state(), Stability::Passing | Stability::Failing));
        }
    }
});
