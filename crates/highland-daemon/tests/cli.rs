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

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// The address both nodes fight over.
const VIP: &str = "192.0.2.100";
const INTERFACE: &str = "eth0";

/// The first address a node may use, in the documentation range.
///
/// Each node takes one, because the tests run at once and two interfaces with
/// the same address on one segment make every packet ambiguous.
const FIRST_NODE_ADDRESS: u8 = 21;

struct Node {
    namespace: String,
    directory: PathBuf,
    metrics_address: String,
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
        // The node's own address, its side of the veth, and the port it exports
        // metrics on all derive from the serial, so parallel tests cannot
        // collide.
        let octet = FIRST_NODE_ADDRESS + u8::try_from(serial % 16).unwrap_or(0);
        let address = format!("192.0.2.{octet}");
        let metrics_port = 19990 + u16::try_from(serial).unwrap_or(0);
        let _ = Command::new("ip").args(["netns", "del", &name]).output();

        let stem = Self::short(serial);
        let local_end = format!("{stem}l");
        let peer_end = format!("{stem}p");
        run(&["netns", "add", &name]);
        run(&[
            "link", "add", "dev", &local_end, "type", "veth", "peer", "name", &peer_end,
        ]);
        // The root-namespace end has to be up as well as the one inside the
        // namespace: a veth with no carrier on the far side reports no carrier
        // here, and a node whose interface has no carrier must not claim an
        // address. The daemon checks that, so a harness that leaves this end
        // down measures a node that is correctly refusing to take over.
        run(&["link", "set", "dev", &local_end, "up"]);
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
            &format!("{address}/24"),
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
        // The endpoint is bound to the node's own address, not to loopback, and
        // the test reaches it from outside the namespace. A network namespace
        // has its own loopback, so binding to 127.0.0.1 would prove nothing
        // about reachability, and a scrape endpoint that only answers on
        // loopback is one no scraper outside the host can use.
        let metrics_listen = format!("{address}:{metrics_port}");
        std::fs::write(
            directory.join("config.toml"),
            format!(
                "schema_version = 1\n\
                 \n[node]\nname = \"cli\"\n\
                 \n[metrics]\nenabled = true\nlisten = \"{metrics_listen}\"\n\
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
            metrics_address: format!("{address}:{metrics_port}"),
            daemon: Some(daemon),
        }
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.directory.join("daemon.log")).unwrap_or_default()
    }

    /// Rewrites the configuration file in place.
    fn rewrite_config(&self, edit: impl FnOnce(&str) -> String) {
        let path = self.directory.join("config.toml");
        let text = std::fs::read_to_string(&path).expect("the configuration is readable");
        std::fs::write(&path, edit(&text)).expect("the configuration is writable");
    }

    /// Sends `SIGHUP` to this node's daemon, which is what a reload is.
    ///
    /// The pid is signalled directly rather than matched by name, because
    /// `ip netns exec` changes only the *network* namespace: every node shares one
    /// process table, so a `pkill -f` on the daemon's name signals every other
    /// node in the test as well.
    fn reload(&self) {
        let pid = self.daemon.as_ref().expect("the daemon is running").id();
        let raw = i32::try_from(pid).expect("a process id fits in the kernel's type");
        nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(raw),
            nix::sys::signal::Signal::SIGHUP,
        )
        .unwrap_or_else(|error| panic!("the reload signal reached pid {pid}: {error}"));
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
const _FIXTURE: (&str, &str) = (VIP, INTERFACE);

/// A scraper must be able to read the node the same way an operator can, and
/// both views must agree: the status comes from the registry, and so do the
/// metrics.
#[test]
fn metrics_and_status_agree_about_the_node() {
    let node = Node::start("metrics");
    assert!(wait_for_socket(&node), "no control socket:\n{}", node.log());

    // Metrics are enabled on this node's configuration, so the endpoint must
    // answer while the instance is still starting.
    // The scrape runs inside the node's namespace, against the node's own
    // address. A namespace has its own loopback and its own routes, so a scrape
    // from the test's own namespace would be testing a different network stack
    // entirely. The endpoint is bound to the node's address rather than to
    // loopback on purpose: an exporter that only answers on loopback is one no
    // scraper on the host can use.
    let address = node.metrics_address.clone();
    let started = Instant::now();
    let mut body = String::new();
    while started.elapsed() < Duration::from_secs(10) {
        let (ok, text) = scrape_in_namespace(&node, &address);
        if ok && text.contains("highland_up") {
            body = text;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        body.contains("highland_up 1"),
        "the endpoint never answered at {address}\n--- daemon ---\n{}\n--- metrics ---\n{body}",
        node.log()
    );

    // Once the node is master, the role in the metrics must match the role the
    // control API reports, because both are rendered from the same registry.
    let started = Instant::now();
    let mut agreed = false;
    while started.elapsed() < Duration::from_secs(10) {
        let (_, status) = cli(&node, &["status", "--json"]);
        let (_, text) = scrape_in_namespace(&node, &address);
        if status.contains("\"role\":\"MASTER\"")
            && text.contains("highland_instance_role{instance=\"api\"} 2")
        {
            agreed = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    let (_, status) = cli(&node, &["status", "--json"]);
    let (_, text) = scrape_in_namespace(&node, &address);
    assert!(
        agreed,
        "the two views disagree\\n--- status ---\\n{status}\\n--- metrics ---\\n{text}"
    );
    assert!(
        text.contains("highland_advertisements_sent_total{instance=\"api\",family=\"v4\"}"),
        "{text}"
    );
    assert!(
        text.contains(
            "highland_instance_transitions_total{instance=\"api\",from=\"BACKUP\",to=\"MASTER\"}"
        ),
        "the transition that took ownership is counted: {text}"
    );
    assert!(
        text.contains("highland_control_requests_total"),
        "control requests are counted: {text}"
    );
}

/// Scrapes the metrics endpoint from inside the node's namespace.
///
/// The daemon ships no HTTP client, and a test that pulled one in would be
/// testing that client as much as the server. `curl` is already needed by the
/// image the kernel tests run in, so the test uses it rather than a dependency.
fn scrape_in_namespace(node: &Node, address: &str) -> (bool, String) {
    let url = format!("http://{address}/metrics");
    let output = Command::new("ip")
        .args(["netns", "exec", &node.namespace])
        .args(["curl", "--silent", "--show-error", "--max-time", "2"])
        .arg(&url)
        .output();

    match output {
        Ok(output) if output.status.success() => {
            (true, String::from_utf8_lossy(&output.stdout).into_owned())
        }
        Ok(output) => (false, String::from_utf8_lossy(&output.stderr).into_owned()),
        Err(error) => (false, error.to_string()),
    }
}

/// A reload is a transaction, and the two halves of that have to be tested
/// against a real node: a change every instance can absorb is applied in place
/// without the address moving, and a change one instance cannot absorb is
/// refused with nothing touched.
#[test]
fn a_reload_is_applied_in_place_or_refused_whole() {
    let node = Node::start("reload");
    assert!(wait_for_socket(&node), "no control socket:\n{}", node.log());

    // Wait until it is master and actually owns the address.
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(10) {
        let (ok, text) = cli(&node, &["status", "--json"]);
        if ok && text.contains("\"vips_owned\":true") {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let (_, before) = cli(&node, &["status", "--json"]);
    assert!(
        before.contains("\"vips_owned\":true"),
        "the node never took the address: {before}"
    );

    // A priority change is reloadable: the role and the address must survive it.
    node.rewrite_config(|text| text.replace("priority = 150", "priority = 120"));
    node.reload();
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(10) {
        let (_, text) = cli(&node, &["status", "--json"]);
        if text.contains("\"priority\":120") {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let (_, applied) = cli(&node, &["status", "--json"]);
    assert!(
        applied.contains("\"priority\":120"),
        "the reload was not applied: {applied}\n--- daemon ---\n{}",
        node.log()
    );
    assert!(
        applied.contains("\"vips_owned\":true"),
        "a reload is not a restart: the address must not move\n{applied}\n--- daemon ---\n{}",
        node.log()
    );
    assert!(
        applied.contains("\"role\":\"MASTER\""),
        "and the role must survive it: {applied}"
    );

    // A VIP change is not reloadable, so the whole reload is refused and the
    // running configuration is untouched.
    node.rewrite_config(|text| text.replace("192.0.2.100/24", "192.0.2.101/24"));
    node.reload();
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(3) {
        let (_, text) = cli(&node, &["status", "--json"]);
        if text.contains("reload_rejected") || !text.contains("192.0.2.101") {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let (_, refused) = cli(&node, &["status", "--json"]);
    assert!(
        !refused.contains("192.0.2.101"),
        "a refused reload must not change what is running: {refused}"
    );
    assert!(
        refused.contains("\"priority\":120"),
        "and it must not undo the reload that was applied: {refused}"
    );
    assert!(
        node.log().contains("reload rejected") && node.log().contains("would need a restart"),
        "the refusal names the instance and the change:\n{}",
        node.log()
    );
}

/// A reload over the control socket and a `SIGHUP` must be the same reload.
/// They used to be two different code paths, and the test exists because that is
/// how they drift apart.
#[test]
fn a_reload_over_the_socket_applies_and_refuses_whole() {
    let node = Node::start("sockreload");
    assert!(wait_for_socket(&node), "no control socket:\n{}", node.log());

    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(10) {
        let (ok, text) = cli(&node, &["status", "--json"]);
        if ok && text.contains("\"vips_owned\":true") {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    // A priority change is reloadable, so the socket applies it.
    node.rewrite_config(|text| text.replace("priority = 150", "priority = 90"));
    let (ok, text) = cli(&node, &["reload", "--yes", "--json"]);
    assert!(
        ok,
        "the reload over the socket failed: {text}\n--- daemon ---\n{}",
        node.log()
    );
    assert!(
        text.contains("\"generation\":1"),
        "the generation advanced: {text}"
    );

    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(10) {
        let (_, status) = cli(&node, &["status", "--json"]);
        if status.contains("\"priority\":90") {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let (_, applied) = cli(&node, &["status", "--json"]);
    assert!(
        applied.contains("\"priority\":90"),
        "the reload did not reach the instance: {applied}"
    );
    assert!(
        applied.contains("\"vips_owned\":true"),
        "and it must not move the address: {applied}"
    );

    // A VIP change is not reloadable, so the socket refuses it and changes
    // nothing.
    node.rewrite_config(|text| text.replace("192.0.2.100/24", "192.0.2.101/24"));
    let (ok, text) = cli(&node, &["reload", "--yes", "--json"]);
    assert!(!ok, "a refused reload is a failure: {text}");
    assert!(text.contains("reload_rejected"), "and it says why: {text}");

    let (_, refused) = cli(&node, &["status", "--json"]);
    assert!(
        refused.contains("\"priority\":90"),
        "the applied reload stands: {refused}"
    );
    assert!(
        !refused.contains("192.0.2.101"),
        "the refused reload changed nothing: {refused}"
    );
}

/// The event history is what happened, not a reconstruction, and a follower
/// asks for what it has not seen.
#[test]
fn the_event_history_records_what_happened() {
    let node = Node::start("events");
    assert!(wait_for_socket(&node), "no control socket:\n{}", node.log());

    // Let the instance take over, so there is a transition to report.
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(10) {
        let (ok, text) = cli(&node, &["status", "--json"]);
        if ok && text.contains("\"role\":\"MASTER\"") {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    let (ok, history) = cli(&node, &["events", "--json", "--limit", "200"]);
    assert!(ok, "events failed: {history}");

    let transitions = history.matches("\"name\":\"role_transition\"").count();
    assert!(transitions >= 2, "the takeover must be recorded: {history}");
    assert!(
        history.contains("master_down_timeout"),
        "with its reason: {history}"
    );
    assert!(
        history.contains("\"sequence\""),
        "and a cursor, so a follower can resume: {history}"
    );

    // A cursor past the end returns nothing new, which is what makes following
    // cheap rather than a re-read of the whole buffer.
    let (_, empty) = cli(&node, &["events", "--json", "--since", "100000"]);
    assert!(
        !empty.contains("role_transition"),
        "asking past the end returns nothing: {empty}"
    );
}
