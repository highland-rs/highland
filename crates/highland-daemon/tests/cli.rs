// Rust guideline compliant 2026-09-27

//! The CLI against a live daemon.
//!
//! `highland status` is the first thing an operator runs, and until this test
//! nothing proved it worked. The daemon here is the real one, in a real
//! namespace, with a real socket; only the topology is a fixture.
//!
//! It needs the kernel tests' feature, because a daemon that cannot speak VRRP
//! will not start.

#![cfg(all(target_os = "linux", feature = "netlink-tests"))]

use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// The instance under test.
const VIP: &str = "192.0.2.100";
const ADDRESS: &str = "192.0.2.21";
const INTERFACE: &str = "eth0";

struct Node {
    namespace: String,
    directory: PathBuf,
    daemon: Option<Child>,
}

impl Node {
    /// A namespace name unique to this test, so the two tests can run at once.
    fn name(label: &str) -> String {
        static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let serial = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        format!("hl-cli-{}-{serial}-{label}", std::process::id())
    }

    /// A short interface name.
    ///
    /// Linux limits interface names to 15 characters, which a name derived from
    /// the test's process id can exceed. The namespace name has no such limit,
    /// so only these need to be short.
    fn short(serial: u32) -> String {
        format!("h{serial}")
    }

    fn start(label: &str) -> Self {
        let name = Self::name(label);
        let serial: u32 = name
            .split('-')
            .nth(3)
            .and_then(|text| text.parse().ok())
            .unwrap_or(0);
        let _ = Command::new("ip").args(["netns", "del", &name]).output();

        let stem = Self::short(serial);
        let local_end = format!("{stem}l");
        let peer_end = format!("{stem}p");
        run(&["netns", "add", &name]);
        run(&[
            "link", "add", "dev", &local_end, "type", "veth", "peer", "name", &peer_end,
        ]);
        run(&["link", "set", "dev", &peer_end, "netns", &name]);
        run(&[
            "netns", "exec", &name, "ip", "link", "set", "dev", &peer_end, "name", INTERFACE,
        ]);
        run(&[
            "netns", "exec", &name, "ip", "link", "set", "dev", "lo", "up",
        ]);
        run(&[
            "netns",
            "exec",
            &name,
            "ip",
            "addr",
            "add",
            &format!("{ADDRESS}/24"),
            "dev",
            INTERFACE,
        ]);
        run(&[
            "netns", "exec", &name, "ip", "link", "set", "dev", INTERFACE, "up",
        ]);

        let directory = PathBuf::from(format!("/tmp/hl-cli-{}-{serial}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("the instance directory can be created");
        let socket = directory.join("control.sock");
        let socket_text = socket.to_string_lossy().to_string();
        std::fs::write(
            directory.join("config.toml"),
            format!(
                "schema_version = 1\n\
                 \n[node]\nname = \"cli\"\n\
                 \n[control]\nsocket = \"{socket_text}\"\n\
                 \n[[instance]]\nname = \"api\"\ninterface = \"{INTERFACE}\"\n\
                 vrid = 42\npriority = 150\nadvertisement_interval = \"1s\"\n\
                 \n[instance.network]\nmode = \"unicast\"\npeers = [\"192.0.2.99\"]\n\
                 \n[[instance.vip]]\naddress = \"{VIP}/24\"\n"
            ),
        )
        .expect("the configuration can be written");

        let log = std::fs::File::create(directory.join("daemon.log")).expect("a log file");
        let errors = log.try_clone().expect("the log can be duplicated");
        let daemon = Command::new("ip")
            .args(["netns", "exec", &name])
            .arg(cli_binary())
            .arg("run")
            .arg("--config")
            .arg(directory.join("config.toml"))
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(errors))
            .spawn()
            .expect("the daemon starts");

        Self {
            namespace: name,
            directory,
            daemon: Some(daemon),
        }
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.directory.join("daemon.log")).unwrap_or_default()
    }

    fn socket(&self) -> PathBuf {
        self.directory.join("control.sock")
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        if let Some(mut daemon) = self.daemon.take() {
            let _ = daemon.kill();
            let _ = daemon.wait();
        }
        let _ = Command::new("ip")
            .args(["netns", "del", &self.namespace])
            .output();
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

fn run(arguments: &[&str]) {
    let output = Command::new("ip")
        .args(arguments)
        .output()
        .expect("ip runs");
    assert!(
        output.status.success(),
        "ip {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// The path of the `highland` binary cargo built alongside this test.
fn cli_binary() -> PathBuf {
    let mut path = std::env::current_exe().expect("the test binary has a path");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    let candidate = path.join("highland-cli");
    assert!(
        candidate.exists(),
        "expected the CLI at {}; run this through `scripts/linux-tests.sh`",
        candidate.display()
    );
    candidate
}

/// Runs the CLI inside the node's namespace and returns its output.
///
/// The socket is always named explicitly, because the default lives in
/// `/run/highland` and this node's socket is in a temporary directory. A client
/// that silently used the default would be testing the wrong daemon.
fn cli(node: &Node, arguments: &[&str]) -> (bool, String) {
    let socket = node.socket();
    let output = Command::new("ip")
        .args(["netns", "exec", &node.namespace])
        .arg(cli_binary())
        .arg("--socket")
        .arg(&socket)
        .args(arguments)
        .output()
        .expect("the CLI runs");
    (
        output.status.success(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
    )
}

/// Waits for the control socket to appear.
fn wait_for_socket(node: &Node) -> bool {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(10) {
        if node.socket().exists() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

#[test]
fn an_operator_can_ask_a_running_node_what_it_is_doing() {
    let node = Node::start("status");
    assert!(
        wait_for_socket(&node),
        "the daemon never created its control socket:\n{}",
        node.log()
    );

    // Give the instance a moment to take the address, so the status a client
    // reads describes a settled node rather than a starting one.
    let started = Instant::now();
    let mut owned = false;
    while started.elapsed() < Duration::from_secs(10) {
        let (ok, text) = cli(&node, &["status", "--json"]);
        if ok && text.contains("\"role\":\"MASTER\"") && text.contains("\"vips_owned\":true") {
            owned = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    let (ok, text) = cli(&node, &["status", "--json"]);
    assert!(ok, "status failed:\n{text}\n--- daemon ---\n{}", node.log());
    assert!(
        text.contains("\"node\":\"cli\""),
        "the node is named: {text}"
    );
    assert!(
        text.contains("\"role\":\"MASTER\""),
        "the role is reported: {text}"
    );
    assert!(
        text.contains(VIP),
        "the address it protects is reported: {text}"
    );
    assert!(
        owned,
        "the instance never became master within 10s:\n{text}\n{}",
        node.log()
    );

    // `show` narrows to one instance, and an unknown name is named in the
    // refusal rather than failing silently.
    let (ok, text) = cli(&node, &["show", "api", "--json"]);
    assert!(ok, "show failed: {text}");
    assert!(text.contains("\"name\":\"api\""), "got: {text}");

    let (ok, text) = cli(&node, &["show", "nope", "--json"]);
    assert!(!ok, "an unknown instance is a failure: {text}");
    assert!(text.contains("nope"), "the refusal names it: {text}");
}

#[test]
fn an_operator_can_make_a_master_give_up_its_address() {
    let node = Node::start("relinquish");
    assert!(wait_for_socket(&node), "no control socket:\n{}", node.log());

    // Wait until it is master, then ask it to relinquish.
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(10) {
        let (ok, text) = cli(&node, &["status", "--json"]);
        if ok && text.contains("\"role\":\"MASTER\"") {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    let (ok, text) = cli(&node, &["relinquish", "api", "--yes", "--json"]);
    assert!(
        ok,
        "relinquish failed:\n{text}\n--- daemon ---\n{}",
        node.log()
    );

    // The address must actually leave the interface, which is the point: an
    // operator who asks a node to give up the address gets it back.
    let started = Instant::now();
    let mut released = false;
    while started.elapsed() < Duration::from_secs(10) {
        let (_, text) = cli(&node, &["status", "--json"]);
        if text.contains("\"vips_owned\":false") {
            released = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    let (_, text) = cli(&node, &["status", "--json"]);
    assert!(
        released,
        "the address was not released:\n{text}\n--- daemon ---\n{}",
        node.log()
    );
    assert!(
        !holds_vip(&node),
        "the kernel still holds {VIP}:\n{}",
        String::from_utf8_lossy(
            &Command::new("ip")
                .args([
                    "netns",
                    "exec",
                    &node.namespace,
                    "ip",
                    "-4",
                    "-o",
                    "addr",
                    "show",
                    INTERFACE
                ])
                .output()
                .expect("ip runs")
                .stdout
        )
    );
}

fn holds_vip(node: &Node) -> bool {
    let output = Command::new("ip")
        .args([
            "netns",
            "exec",
            &node.namespace,
            "ip",
            "-4",
            "-o",
            "addr",
            "show",
            INTERFACE,
        ])
        .output()
        .expect("ip runs");
    String::from_utf8_lossy(&output.stdout).contains(VIP)
}

/// The test file keeps these so the address and the interface are stated once.
#[allow(dead_code, reason = "documents the fixture")]
const _FIXTURE: (&str, &str, &str) = (VIP, ADDRESS, INTERFACE);

#[allow(dead_code, reason = "documents the fixture")]
fn _unused(_: &Path, _: IpAddr) {}
