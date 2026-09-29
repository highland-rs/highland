// Rust guideline compliant 2026-09-28

//! Startup validation, against the host it actually runs on.
//!
//! The gap this file closes: `V-08` had a unit test in `highland-config` that
//! passed a context containing the local addresses, so the rule was proven to
//! work while nothing in the product ever supplied those addresses. A test that
//! constructs the input itself proves only that the check reads the input. These
//! tests go through `Daemon::prepare`, which is the path an operator's
//! configuration takes, and assert the rule fires there.

use std::path::PathBuf;

use highland_daemon::{Daemon, Options};

/// Writes `body` to a temporary file and returns its path.
///
/// The file must outlive the call, so a leaked temp directory is the honest
/// trade for a test that has no cleanup hook; the alternative is a
/// `tempfile` dependency for two tests.
fn write_config(name: &str, body: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("hl-v08-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("config.toml");
    std::fs::write(&path, body).expect("write config");
    path
}

/// A configuration whose single peer is `peer`.
fn config_with_peer(peer: &str) -> String {
    format!(
        r#"schema_version = 1

[node]
name = "v08"

[[instance]]
name = "api"
interface = "lo"
vrid = 42
priority = 150
startup_delay = "0s"

[instance.network]
mode = "unicast"
peers = ["{peer}"]

[[instance.vip]]
address = "192.0.2.10/24"
"#
    )
}

fn prepare(path: PathBuf) -> Result<Daemon, highland_daemon::DaemonError> {
    Daemon::prepare(Options::with_config(path))
}

/// Loopback is configured on every host, so it is the one peer address that must
/// always be refused, and it needs no privileges to know about.
#[test]
fn a_loopback_peer_is_refused() {
    let path = write_config("loopback", &config_with_peer("127.0.0.1"));
    let error = prepare(path).expect_err("a node cannot peer with itself");
    let text = error.to_string();
    assert!(
        text.contains("V-08"),
        "the rejection must name the rule it enforces, got: {text}"
    );
}

/// The same rule for IPv6 loopback, because the address list is per-family and a
/// check that only consults IPv4 would be half a check.
#[test]
fn an_ipv6_loopback_peer_is_refused() {
    let path = write_config("loopback6", &config_with_peer("::1"));
    let error = prepare(path).expect_err("a node cannot peer with itself over IPv6");
    let text = error.to_string();
    assert!(text.contains("V-08"), "expected the V-08 rule, got: {text}");
}

/// The other direction: a peer that is not local must still be accepted.
///
/// Without this, "refuse everything" would satisfy the test above. The rule is
/// about self-peering, not about unicast.
#[test]
fn a_remote_peer_is_accepted() {
    let path = write_config("remote", &config_with_peer("192.0.2.20"));
    prepare(path).expect("a documentation-range peer is not local and must be allowed");
}

/// A peer in the VIP's own subnet is not automatically local. The only thing
/// that makes an address local is its presence on this host, so a test that
/// expected same-subnet rejection would be asserting a different rule.
#[test]
fn a_peer_sharing_the_vip_subnet_is_accepted() {
    let path = write_config("subnet", &config_with_peer("192.0.2.99"));
    prepare(path).expect("V-08 is about configured addresses, not subnets");
}
