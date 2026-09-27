// Rust guideline compliant 2026-09-27

//! The namespace harness: two daemons on one segment, and the faults to inject
//! into that segment.
//!
//! Both the failover suite and the chaos suite need the same topology, so it
//! lives here rather than being written twice. Two namespaces joined by a
//! bridge, because that is what a VRRP segment is; the namespaces cannot see
//! each other's kernel state, so everything that passes between the two nodes
//! went over a socket.

// A test-support module has more surface than any one suite uses: the failover
// suite needs the VIP constants and not the fault injectors, the chaos suite
// needs both. Restricting every item to `pub(crate)` per suite would be noise.
#![allow(dead_code, unreachable_pub)]

use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// The interface inside each namespace, named as an operator would name it.
pub const INTERFACE: &str = "eth0";

/// The address the two nodes fight over, from the RFC 5737 documentation range.
pub const VIRTUAL_ADDRESS: &str = "192.0.2.100";
/// The address node A sends from.
pub const A_ADDRESS: &str = "192.0.2.11";
/// The address node B sends from.
pub const B_ADDRESS: &str = "192.0.2.12";

/// One node: a namespace, an address, and a daemon.
pub struct Node {
    namespace: String,
    /// Where this node's configuration and logs live.
    pub directory: PathBuf,
    daemon: Option<Child>,
}

impl Node {
    pub fn name(namespace: &str) -> String {
        // A veth end keeps this name in the root namespace, and the kernel
        // allows an interface 15 characters long. Truncating is better than
        // letting `ip` fail with a name the test author cannot see the reason
        // for.
        let mut name = format!("hl-{namespace}-{}", std::process::id());
        name.truncate(15);
        name
    }

    /// Creates the namespace, its veth, and the configuration file.
    ///
    /// The veth pair is created in the root namespace, one end is enslaved to
    /// the bridge there, and only the *peer* end moves into the node's
    /// namespace. A bridge cannot be enslaved to from another namespace, so
    /// this is the only arrangement that gives two namespaces one segment.
    pub fn create(namespace: &str, address: &'static str, bridge: &str, preempt: bool) -> Self {
        Self::create_with_priority(namespace, address, bridge, preempt, 150)
    }

    /// As [`Node::create`], with an explicit priority.
    ///
    /// The chaos suite needs one node to outrank the other: a frozen node that
    /// returns and finds a higher-priority master has to step down, which is the
    /// only way a frozen node can give an address back on its own.
    pub fn create_with_priority(
        namespace: &str,
        address: &'static str,
        bridge: &str,
        preempt: bool,
        priority: u8,
    ) -> Self {
        let name = Self::name(namespace);
        run(&["netns", "del", &name], true);

        let local_end = format!("{name}-l");
        let peer_end = format!("{name}-p");
        // A previous test that panicked between creating these and registering
        // its cleanup would otherwise leave the root-namespace end behind, and
        // the next run would fail with "File exists" for a reason that has
        // nothing to do with what it is testing.
        run(&["link", "del", &local_end], true);
        run(&["link", "del", &peer_end], true);
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
        let config = configuration(
            namespace,
            address,
            &directory,
            preempt,
            VIRTUAL_ADDRESS,
            priority,
        );
        std::fs::write(directory.join("config.toml"), config)
            .expect("the configuration can be written");

        Self {
            namespace: name,
            directory,
            daemon: None,
        }
    }

    /// Starts the daemon, capturing its output so a failure can explain itself.
    pub fn start(&mut self) {
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

    /// The hardware address of this node's interface, as the kernel reports it.
    #[must_use]
    pub fn hardware_address(&self) -> String {
        let output = Command::new("ip")
            .args([
                "netns",
                "exec",
                &self.namespace,
                "ip",
                "-o",
                "link",
                "show",
                INTERFACE,
            ])
            .output()
            .expect("ip runs");
        String::from_utf8_lossy(&output.stdout)
            .split("link/ether ")
            .nth(1)
            .and_then(|rest| rest.split_whitespace().next())
            .unwrap_or_default()
            .to_owned()
    }

    /// Returns this node's daemon output.
    pub fn log(&self) -> String {
        std::fs::read_to_string(self.directory.join("daemon.log")).unwrap_or_default()
    }

    /// Kills the daemon, the way a node failure looks to its peer.
    ///
    /// A `SIGKILL`, not a graceful stop: the point is to look like a power
    /// failure, so the survivor has to discover it by timing out.
    pub fn kill(&mut self) {
        if let Some(mut child) = self.daemon.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// Returns the addresses configured on this node's interface.
    pub fn addresses(&self) -> Vec<IpAddr> {
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
    pub fn holds_vip(&self) -> bool {
        self.addresses()
            .iter()
            .any(|address| address.to_string() == VIRTUAL_ADDRESS)
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        self.kill();
        run(&["netns", "del", &self.namespace], true);
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

pub fn run(arguments: &[&str], allow_failure: bool) {
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
pub fn daemon_binary() -> PathBuf {
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

/// Writes the configuration a node under test runs, with its single instance
/// and the address the two nodes fight over.
#[must_use]
pub fn configuration(
    namespace: &str,
    address: &'static str,
    directory: &Path,
    preempt: bool,
    vip: &str,
    priority: u8,
) -> String {
    // Unicast mode needs a peer, and the only honest peer is the other node.
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
priority = {priority}
advertisement_interval = "1s"
startup_delay = "0s"
preempt = {preempt}

[instance.network]
mode = "unicast"
peers = ["{peer}"]

[[instance.vip]]
address = "{vip}/24"
"#,
        namespace = namespace,
        vip = vip,
        priority = priority,
        socket = directory.join("control.sock").display(),
    )
}

/// Polls `condition` until it has held for `window` in a row, or the budget runs
/// out.
///
/// A single observation is not stability, and the difference is what a test is
/// about. "Exactly one master" observed once is a snapshot that can sit inside a
/// dual-master startup — two nodes whose master-down timers expire together both
/// take the address until the tie is broken, which is authentic VRRP and takes as
/// long as it takes each to hear the other. A test that injects a fault into that
/// window blames the fault for a split brain that was already happening.
#[must_use]
pub fn wait_stable(label: &str, window: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let started = Instant::now();
    let mut held_since = None;
    while started.elapsed() < window * 4 {
        if condition() {
            let since = *held_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= window {
                tracing::info!("{label} after {:?}", started.elapsed());
                return true;
            }
        } else {
            held_since = None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    tracing::warn!("{label}: never held for {window:?}");
    false
}

/// Polls `condition` until it holds or the budget runs out.
pub fn wait_for(label: &str, budget: Duration, mut condition: impl FnMut() -> bool) -> bool {
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

impl Node {
    /// Counts the role changes in this node's output, which is the event
    /// volume a chaos scenario has to keep bounded.
    #[must_use]
    pub fn role_changes(&self) -> usize {
        self.log().matches("role changed").count()
    }

    /// Installs a `netem` fault on this node's egress.
    ///
    /// The node's own interface, not the bridge: a fault on the bridge would
    /// hit both nodes at once, and the interesting question is what one node
    /// does to its peer on its own.
    pub fn netem(&self, arguments: &[&str]) {
        let mut command = vec![
            "netns".to_owned(),
            "exec".to_owned(),
            self.namespace.clone(),
            "tc".to_owned(),
            "qdisc".to_owned(),
            "replace".to_owned(),
            "dev".to_owned(),
            INTERFACE.to_owned(),
            "root".to_owned(),
            "netem".to_owned(),
        ];
        command.extend(arguments.iter().map(|text| (*text).to_owned()));
        run_owned(&command);
    }

    /// Removes any fault from this node's egress.
    pub fn clear_netem(&self) {
        self.netem(&["delay", "0ms"]);
    }

    /// Takes this node's link down, which is what a cable pull looks like.
    pub fn link_down(&self) {
        run(
            &[
                "netns",
                "exec",
                &self.namespace,
                "ip",
                "link",
                "set",
                INTERFACE,
                "down",
            ],
            false,
        );
    }

    /// Brings this node's link back up.
    pub fn link_up(&self) {
        run(
            &[
                "netns",
                "exec",
                &self.namespace,
                "ip",
                "link",
                "set",
                INTERFACE,
                "up",
            ],
            false,
        );
    }

    /// Freezes this node's daemon with `SIGSTOP`.
    ///
    /// A stop, not a kill: the process is alive, still holds the address, and
    /// answers nothing at all, which is the case a timeout-based protocol has to
    /// survive and a connection-close-based one cannot.
    pub fn suspend(&self) {
        self.signal(nix::sys::signal::Signal::SIGSTOP);
    }

    /// Resumes this node's daemon.
    pub fn resume(&self) {
        self.signal(nix::sys::signal::Signal::SIGCONT);
    }

    /// Sends a signal to this node's daemon, or fails the test explaining why.
    fn signal(&self, number: nix::sys::signal::Signal) {
        let child = self
            .daemon
            .as_ref()
            .expect("the daemon must be running to signal it");
        let pid = i32::try_from(child.id()).expect("a process id fits in a pid_t");
        match nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), number) {
            Ok(()) | Err(nix::errno::Errno::ESRCH) => {}
            Err(error) => panic!("could not signal the daemon with {number}: {error}"),
        }
    }

    /// Reports whether this node's link has carrier.
    #[must_use]
    pub fn carrier(&self) -> bool {
        let output = Command::new("ip")
            .args([
                "netns",
                "exec",
                &self.namespace,
                "ip",
                "-o",
                "link",
                "show",
                INTERFACE,
            ])
            .output()
            .expect("ip runs");
        String::from_utf8_lossy(&output.stdout).contains("state UP")
    }
}

/// Runs an `ip` command whose arguments are owned, for the fault helpers.
fn run_owned(arguments: &[String]) {
    let output = Command::new("ip")
        .args(arguments)
        .output()
        .unwrap_or_else(|error| panic!("could not run ip {arguments:?}: {error}"));
    assert!(
        output.status.success(),
        "ip {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Removes the bridge on drop, so a failed test does not leave the host dirty.
pub struct BridgeGuard {
    /// The kernel name of the bridge, so a caller can attach veth ends to it.
    pub name: String,
}

impl Drop for BridgeGuard {
    fn drop(&mut self) {
        let _ = Command::new("ip")
            .args(["link", "del", &self.name])
            .output();
    }
}

/// A namespace on the same segment that runs no daemon.
///
/// This exists to check what a *neighbour* learns, rather than what a sender
/// believes it sent. An announcement is only real if something on the segment
/// acts on it, and a daemon is the wrong witness: it has its own idea of who owns
/// the address.
pub struct Observer {
    namespace: String,
    address: &'static str,
}

impl Observer {
    /// Attaches a namespace to the bridge with one address.
    #[must_use]
    pub fn create(namespace: &str, address: &'static str, bridge: &str) -> Self {
        let name = Node::name(namespace);
        run(&["netns", "del", &name], true);
        let local_end = format!("{name}-l");
        let peer_end = format!("{name}-p");
        run(&["link", "del", &local_end], true);
        run(&["link", "del", &peer_end], true);
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
        Self {
            namespace: name,
            address,
        }
    }

    /// The observer's own hardware address, as the kernel reports it.
    #[must_use]
    pub fn hardware_address(&self) -> String {
        let output = Command::new("ip")
            .args([
                "netns",
                "exec",
                &self.namespace,
                "ip",
                "-o",
                "link",
                "show",
                INTERFACE,
            ])
            .output()
            .expect("ip runs");
        String::from_utf8_lossy(&output.stdout)
            .split("link/ether ")
            .nth(1)
            .and_then(|rest| rest.split_whitespace().next())
            .unwrap_or_default()
            .to_owned()
    }

    /// Sends a datagram to `address`, which is what makes this namespace resolve
    /// it, and returns the hardware address the neighbour cache then holds.
    ///
    /// A datagram rather than a ping because the point is the *cache*, and a
    /// ping brings a tool and its own opinions about what a reply should be.
    /// Nothing has to answer: the resolution happens on the way out.
    #[must_use]
    pub fn resolve(&self, address: &str) -> String {
        let _ = Command::new("ip")
            .args([
                "netns",
                "exec",
                &self.namespace,
                "bash",
                "-c",
                &format!("echo x > /dev/udp/{address}/9"),
            ])
            .output();
        std::thread::sleep(Duration::from_millis(150));
        self.neighbour(address)
    }

    /// The hardware address this namespace's neighbour cache holds for
    /// `address`, or an empty string when it holds none.
    #[must_use]
    pub fn neighbour(&self, address: &str) -> String {
        let output = Command::new("ip")
            .args([
                "netns",
                "exec",
                &self.namespace,
                "ip",
                "neigh",
                "show",
                address,
            ])
            .output()
            .expect("ip runs");
        String::from_utf8_lossy(&output.stdout)
            .split("lladdr ")
            .nth(1)
            .and_then(|rest| rest.split_whitespace().next())
            .unwrap_or_default()
            .to_owned()
    }

    /// The address this observer was given.
    #[must_use]
    pub fn address(&self) -> &str {
        self.address
    }
}

impl Drop for Observer {
    fn drop(&mut self) {
        run(&["netns", "del", &self.namespace], true);
    }
}

/// Creates the bridge the two namespaces share, and removes it on drop.
#[must_use]
pub fn bridge() -> BridgeGuard {
    let name = format!("hl-br-{}", std::process::id());
    let _ = Command::new("ip").args(["link", "del", &name]).output();
    run(&["link", "add", &name, "type", "bridge"], false);
    run(&["link", "set", &name, "up"], false);
    BridgeGuard { name }
}
