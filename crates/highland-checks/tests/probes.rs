// Rust guideline compliant 2026-09-27

//! The probes, against a real socket where one is cheap to make.
//!
//! Every check type gets two things here: it passes against a service that
//! works, and it fails against one that does not — with a *reason* rather than a
//! bare boolean. The services are sockets this test opens itself, so the
//! assertions are about the probe rather than about a fixture nobody can see.
//!
//! The bound on a probe is not tested here, because a probe does not have one:
//! the scheduler owns the timeout (`SPEC.md` §15.4), and `scheduler.rs` tests
//! that where it lives.

use std::net::Ipv4Addr;
use std::time::Duration;

use highland_checks::{
    Check, CheckError, CheckKind, CheckResult, CheckSpec, CheckStatus, TcpCheck, UnixCheck,
};
use highland_core::state::Generation;

/// A listening socket on an ephemeral loopback port.
fn listener() -> std::net::TcpListener {
    std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("loopback accepts a listener")
}

/// A check specification with the parameters a test cares about spelled out.
fn spec(name: &str, kind: CheckKind, timeout: Duration) -> CheckSpec {
    let mut spec = CheckSpec::new(name, kind, Duration::from_millis(50), timeout, 1, 1, 100)
        .expect("the fixture is valid");
    spec.initial_grace_period = Duration::ZERO;
    spec
}

fn run(check: &dyn Check) -> CheckResult {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime")
        .block_on(check.run(Generation::initial(), 1))
        .expect("a probe returns a result rather than an error")
}

// ----- tcp -----------------------------------------------------------------

#[test]
fn a_tcp_check_passes_when_something_is_listening() {
    let listener = listener();
    let address = listener.local_addr().expect("the listener has an address");
    let check = TcpCheck::new(
        spec("api", CheckKind::Tcp, Duration::from_secs(2)),
        &address.to_string(),
    )
    .expect("the address is usable");

    let result = run(&check);

    assert_eq!(result.status, CheckStatus::Passing, "{}", result.reason);
    assert!(
        result.reason.contains(&address.port().to_string()),
        "the reason names what answered: {}",
        result.reason
    );
}

#[test]
fn a_tcp_check_fails_with_a_reason_when_nothing_is_listening() {
    // Bound and dropped, so the port is free and nothing will answer it.
    let address = listener().local_addr().expect("an address");
    let check = TcpCheck::new(
        spec("api", CheckKind::Tcp, Duration::from_secs(2)),
        &address.to_string(),
    )
    .expect("the address is usable");

    let result = run(&check);

    assert_eq!(result.status, CheckStatus::Failing);
    assert!(
        result.reason.contains("could not connect"),
        "a failure says what failed: {}",
        result.reason
    );
}

#[test]
fn a_tcp_check_without_a_port_is_refused_when_it_is_built() {
    // A portless address is a configuration error, and finding it when the check
    // is built stops the daemon rather than failing on every interval forever.
    let error = TcpCheck::new(
        spec("api", CheckKind::Tcp, Duration::from_secs(1)),
        "192.0.2.10",
    )
    .expect_err("a portless address cannot be connected to");

    assert!(
        matches!(error, CheckError::UnresolvableTarget { .. }),
        "{error}"
    );
    assert!(error.to_string().contains("a port"), "{error}");
}

// ----- unix ----------------------------------------------------------------

#[test]
fn a_unix_check_passes_and_fails_with_reasons() {
    let path = std::env::temp_dir().join(format!("highland-check-{}.sock", std::process::id()));
    let listener = std::os::unix::net::UnixListener::bind(&path).expect("a socket path is free");
    let check = UnixCheck::new(
        spec("local", CheckKind::Unix, Duration::from_secs(1)),
        path.clone(),
    );

    let result = run(&check);
    assert_eq!(result.status, CheckStatus::Passing, "{}", result.reason);

    drop(listener);
    let _ = std::fs::remove_file(&path);

    let result = run(&check);
    assert_eq!(result.status, CheckStatus::Failing);
    assert!(
        result.reason.contains("could not connect"),
        "a failure says what failed: {}",
        result.reason
    );
}
