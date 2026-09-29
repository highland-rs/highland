// Rust guideline compliant 2026-09-28

//! `check-config` against the host it runs on.
//!
//! `check-config` used to validate against `ValidationContext::permissive()`,
//! which answers "unknown" for everything a document cannot say: which addresses
//! are local (`V-08`) and which interfaces exist (`V-22`). Both rules were
//! documented as rejected and neither could fire, so the pre-flight check an
//! operator runs before deploying a configuration could not catch two
//! misconfigurations the daemon refuses to start on.
//!
//! These tests run the real binary and need no privileges, which is the point:
//! a rule that only a privileged suite can test is a rule that goes unverified
//! for most of its life.

use std::path::PathBuf;
use std::process::Command;

/// The loopback interface's name, which is not the same on every platform.
fn loopback_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "lo0"
    } else {
        "lo"
    }
}

/// A configuration with `interface` and a single unicast peer.
fn config(interface: &str, peer: &str) -> String {
    format!(
        "schema_version = 1\n\
         \n[node]\n\
         name = \"audit\"\n\
         \n[[instance]]\n\
         name = \"api\"\n\
         interface = \"{interface}\"\n\
         vrid = 42\n\
         priority = 150\n\
         \n[instance.network]\n\
         mode = \"unicast\"\n\
         peers = [\"{peer}\"]\n\
         \n[[instance.vip]]\n\
         address = \"192.0.2.10/24\"\n"
    )
}

/// Runs `check-config` and returns whether it accepted the file, with its output.
///
/// `name` must be unique per call: the tests run in parallel, and one shared
/// filename would have them overwriting each other's configuration and asserting
/// on whatever landed last.
fn check(name: &str, body: &str) -> (bool, String) {
    let path = write(name, body);
    let output = Command::new(env!("CARGO_BIN_EXE_highland"))
        .args(["check-config", path.to_str().expect("utf-8 path")])
        .output()
        .expect("the CLI binary runs");
    (
        output.status.success(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
    )
}

fn write(name: &str, body: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("hl-cli-audit-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join(format!("{name}.toml"));
    std::fs::write(&path, body).expect("write config");
    // `V-26` refuses a world-writable file, and the tests are not about that.
    set_mode(&path, 0o600);
    path
}

#[cfg(unix)]
fn set_mode(path: &std::path::Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).expect("chmod");
}

#[cfg(not(unix))]
fn set_mode(_path: &std::path::Path, _mode: u32) {}

/// The baseline: a configuration that should be accepted. Every other test in
/// this file is only meaningful if this one passes, because otherwise "rejected"
/// could mean the harness is broken.
#[test]
fn a_sane_configuration_is_accepted() {
    let (ok, output) = check("sane", &config(loopback_name(), "192.0.2.20"));
    assert!(ok, "a valid configuration must be accepted, got: {output}");
}

/// `V-22`: an interface that does not exist on this host.
///
/// The daemon already refused to start in this case, but only after loading the
/// configuration and reaching the bind, and it reported a netlink error rather
/// than the rule. A pre-flight check that cannot name the rule makes the
/// deployment failure harder to diagnose than it needs to be.
#[test]
fn a_missing_interface_is_rejected() {
    let (ok, output) = check("missing-if", &config("nosuchif0", "192.0.2.20"));
    assert!(!ok, "a missing interface must be rejected, got: {output}");
    assert!(
        output.contains("V-22"),
        "the rejection must name the rule, got: {output}"
    );
}

/// The other direction, so the test above cannot pass by refusing everything.
#[test]
fn an_existing_interface_is_accepted() {
    let (ok, output) = check("existing-if", &config(loopback_name(), "192.0.2.20"));
    assert!(ok, "loopback exists on this host, got: {output}");
}

/// `V-08`: a peer that is this host's own address.
///
/// This is the rule the audit in `scripts/audit-rules.py` found unenforced in
/// the published 0.1.0 binary, which reported a loopback peer as valid.
#[test]
fn a_loopback_peer_is_rejected() {
    let (ok, output) = check("peer-v4", &config(loopback_name(), "127.0.0.1"));
    assert!(!ok, "a node cannot peer with itself, got: {output}");
    assert!(
        output.contains("V-08"),
        "the rejection must name the rule, got: {output}"
    );
}

/// The IPv6 half of the same rule, because the address list is per-family and a
/// check reading only IPv4 would be half a check.
#[test]
fn an_ipv6_loopback_peer_is_rejected() {
    let (ok, output) = check("peer-v6", &config(loopback_name(), "::1"));
    assert!(
        !ok,
        "a node cannot peer with itself over IPv6, got: {output}"
    );
    assert!(output.contains("V-08"), "expected V-08, got: {output}");
}

/// `defer_interface_binding` is the documented escape hatch for an interface
/// that will exist later, so it has to keep working now that the check fires.
#[test]
fn deferring_the_binding_still_permits_a_missing_interface() {
    let body = config("nosuchif0", "192.0.2.20").replace(
        "interface = \"nosuchif0\"\n",
        "interface = \"nosuchif0\"\ndefer_interface_binding = true\n",
    );
    let (ok, output) = check("deferred", &body);
    assert!(
        ok,
        "V-22 is documented to defer when binding is deferred, got: {output}"
    );
}
