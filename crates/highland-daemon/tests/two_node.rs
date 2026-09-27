// Rust guideline compliant 2026-09-27

//! Two daemons, one segment, and a VIP that moves.
//!
//! This is the Milestone 4 exit criterion (`M-04`): a single-instance daemon
//! fails over a VIP against a second node in a namespace. The topology is the
//! one a VRRP segment actually is, rather than a simulation of one:
//!
//! ```text
//!   netns hla ── hlap ──┬── hlbr0 ──┬── hlbp ── netns hlb
//!                       bridge     (192.0.2.11/24, .12/24)
//! ```
//!
//! Two namespaces mean the two nodes cannot see each other's kernel state, so
//! everything that passes between them went over a socket: the advertisement,
//! and the address that moves.
//!
//! It needs `CAP_NET_ADMIN` and `CAP_NET_RAW` to build the topology and to move
//! an address, so it is behind `netlink-tests` and run by
//! `scripts/linux-tests.sh`.
//!
//! The timing assertions are the point of the exercise. A failover that
//! "works" but takes ten seconds is not a failover, so each deadline below is
//! `SPEC.md` §13.3's budget and not a number chosen to make the test pass.

#![cfg(all(target_os = "linux", feature = "netlink-tests"))]

use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// The virtual address both nodes fight over.
const VIP: &str = "192.0.2.100";
/// The address node A sends from.
const A_ADDRESS: &str = "192.0.2.11";
/// The address node B sends from.
const B_ADDRESS: &str = "192.0.2.12";
/// The interface inside each namespace.
const INTERFACE: &str = "eth0";

/// `Master_Down_Interval` for a one-second interval at priority 150 (SPEC.md §13.3).
const MASTER_DOWN_BUDGET: Duration = Duration::from_millis(3600);

/// One node: a namespace, an address, and a daemon.
struct Node {
    namespace: String,
    directory: PathBuf,
    daemon: Option<Child>,
}

impl Node {
    fn name(namespace: &str) -> String {
        format!("hl-{namespace}-{}", std::process::id())
    }

    /// Creates the namespace, its veth, and the configuration file.
    ///
    /// The veth pair is created in the root namespace, one end is enslaved to
    /// the bridge there, and only the *peer* end moves into the node's
    /// namespace. A bridge cannot be enslaved to from another namespace, so
    /// this is the only arrangement that gives two namespaces one segment.
    fn create(namespace: &str, address: &'static str, bridge: &str, preempt: bool) -> Self {
        let name = Self::name(namespace);
        run(&["netns", "del", &name], true);

        let local_end = format!("{name}-l");
        let peer_end = format!("{name}-p");
        run(&["netns", "add", &name], false);
        run(
            &[
                "link", "add", &local_end, "type", "veth", "peer", "name", &peer_end,
            ],
            false,
        );
        run(&["link", "set", &local_end, "master", bridge], false);
        run(&["link", "set", &local_end, "up"], false);
        run(&["link", "set", &peer_end, "netns", &name], false);
        // Rename the device inside the namespace, so the configuration under
        // test is the one an operator would write.
        run(
            &[
                "netns", "exec", &name, "ip", "link", "set", &peer_end, "name", INTERFACE,
            ],
            false,
        );
        run(
            &["netns", "exec", &name, "ip", "link", "set", "lo", "up"],
            false,
        );
        run(
            &[
                "netns",
                "exec",
                &name,
                "ip",
                "addr",
                "add",
                &format!("{address}/24"),
                "dev",
                INTERFACE,
            ],
            false,
        );
        run(
            &["netns", "exec", &name, "ip", "link", "set", INTERFACE, "up"],
            false,
        );

        let directory = std::env::temp_dir().join(format!("highland-{name}"));
        std::fs::create_dir_all(&directory).expect("the instance directory can be created");
        let config = configuration(namespace, address, &directory, preempt);
        std::fs::write(directory.join("config.toml"), config)
            .expect("the configuration can be written");

        Self {
            namespace: name,
            directory,
            daemon: None,
        }
    }

    /// Starts the daemon, capturing its output so a failure can explain itself.
    fn start(&mut self) {
        let binary = daemon_binary();
        let log_path = self.directory.join("daemon.log");
        let log = std::fs::File::create(&log_path).expect("the log file can be created");
        let errors = log.try_clone().expect("the log file can be duplicated");

        // The daemon must run *inside* the node's namespace. A process in the
        // root namespace would see the container's own interfaces, which is the
        // one thing a namespace exists to prevent.
        let child = Command::new("ip")
            .args(["netns", "exec", &self.namespace])
            .arg(binary)
            .arg("run")
            .arg("--config")
            .arg(self.directory.join("config.toml"))
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(errors))
            .spawn();
        assert!(child.is_ok(), "the daemon binary must be runnable");
        self.daemon = child.ok();
    }

    /// Returns this node's daemon output.
    fn log(&self) -> String {
        std::fs::read_to_string(self.directory.join("daemon.log")).unwrap_or_default()
    }

    /// Kills the daemon, the way a node failure looks to its peer.
    ///
    /// A `SIGKILL`, not a graceful stop: the point is to look like a power
    /// failure, so the survivor has to discover it by timing out.
    fn kill(&mut self) {
        if let Some(mut child) = self.daemon.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// Returns the addresses configured on this node's interface.
    fn addresses(&self) -> Vec<IpAddr> {
        let output = Command::new("ip")
            .args([
                "netns",
                "exec",
                &self.namespace,
                "ip",
                "-4",
                "-o",
                "addr",
                "show",
                INTERFACE,
            ])
            .output()
            .expect("ip runs");
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| line.split_whitespace().nth(3))
            .filter_map(|text| text.split('/').next())
            .filter_map(|text| text.parse::<IpAddr>().ok())
            .collect()
    }

    /// Returns `true` when this node currently holds the VIP.
    fn holds_vip(&self) -> bool {
        self.addresses()
            .iter()
            .any(|address| address.to_string() == VIP)
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        self.kill();
        run(&["netns", "del", &self.namespace], true);
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

fn run(arguments: &[&str], allow_failure: bool) {
    let output = Command::new("ip")
        .args(arguments)
        .output()
        .unwrap_or_else(|error| panic!("could not run ip {arguments:?}: {error}"));
    // A panic only on the failure the caller did not allow, and with the
    // kernel's own message, which is the part worth reading.
    assert!(
        output.status.success() || allow_failure,
        "ip {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// The path of the daemon binary cargo just built.
fn daemon_binary() -> PathBuf {
    // The test binary lives beside the daemon in the same target directory.
    let mut path = std::env::current_exe().expect("the test binary has a path");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    let candidate = path.join("highland-daemon");
    assert!(
        candidate.exists(),
        "expected the daemon binary at {}; run this through `scripts/linux-tests.sh`, which builds it first",
        candidate.display()
    );
    candidate
}

fn configuration(
    namespace: &str,
    address: &'static str,
    directory: &Path,
    preempt: bool,
) -> String {
    let peer = if address == A_ADDRESS {
        B_ADDRESS
    } else {
        A_ADDRESS
    };
    format!(
        r#"schema_version = 1

[node]
name = "hl-{namespace}"

[logging]
level = "debug"

[control]
socket = "{socket}"

[[instance]]
name = "api"
interface = "{INTERFACE}"
vrid = 42
priority = 150
advertisement_interval = "1s"
startup_delay = "0s"
preempt = {preempt}

[instance.network]
mode = "unicast"
peers = ["{peer}"]

[[instance.vip]]
address = "{VIP}/24"
"#,
        namespace = namespace,
        socket = directory.join("control.sock").display(),
    )
}

/// Polls `condition` until it holds or the budget runs out.
fn wait_for(label: &str, budget: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let started = Instant::now();
    while started.elapsed() < budget {
        if condition() {
            tracing::info!("{label} after {:?}", started.elapsed());
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

#[test]
fn two_nodes_elect_one_master_and_the_vip_moves_when_it_dies() {
    // A bridge in this namespace, with both nodes' veth ends attached to it.
    let bridge = format!("hl-br-{}", std::process::id());
    let _ = Command::new("ip").args(["link", "del", &bridge]).output();
    run(&["link", "add", &bridge, "type", "bridge"], false);
    run(&["link", "set", &bridge, "up"], false);
    let _bridge_guard = BridgeGuard {
        name: bridge.clone(),
    };

    let mut first = Node::create("a", A_ADDRESS, &bridge, false);
    let mut second = Node::create("b", B_ADDRESS, &bridge, false);

    first.start();
    second.start();

    // Exactly one node takes the address. The other must not, or both nodes
    // believe they are master, which is the split brain the specification names.
    let one_master = wait_for("one node holds the VIP", Duration::from_secs(10), || {
        usize::from(first.holds_vip()) + usize::from(second.holds_vip()) == 1
    });
    assert!(
        one_master,
        "exactly one node must hold {VIP}: a={:?} b={:?}\n--- a ---\n{}\n--- b ---\n{}",
        first.addresses(),
        second.addresses(),
        first.log(),
        second.log()
    );

    let first_was_master = first.holds_vip();

    // Kill the master. The survivor must take the address within
    // `Master_Down_Interval`, not eventually.
    if first_was_master {
        first.kill();
        let moved = wait_for("the VIP moved to the survivor", MASTER_DOWN_BUDGET, || {
            second.holds_vip()
        });
        assert!(
            moved,
            "the VIP did not move within {MASTER_DOWN_BUDGET:?}: a={:?} b={:?}",
            first.addresses(),
            second.addresses()
        );
        assert!(
            !first.holds_vip(),
            "the dead node cannot still hold the address"
        );
    } else {
        second.kill();
        let moved = wait_for("the VIP moved to the survivor", MASTER_DOWN_BUDGET, || {
            first.holds_vip()
        });
        assert!(
            moved,
            "the VIP did not move within {MASTER_DOWN_BUDGET:?}: a={:?} b={:?}",
            first.addresses(),
            second.addresses()
        );
    }
}

/// Removes the bridge on drop, so a failed test does not leave the host dirty.
struct BridgeGuard {
    name: String,
}

impl Drop for BridgeGuard {
    fn drop(&mut self) {
        let _ = Command::new("ip")
            .args(["link", "del", &self.name])
            .output();
    }
}
