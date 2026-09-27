// Rust guideline compliant 2026-09-27

//! The control socket, end to end.
//!
//! A real socket, a real service, a real client. The two-node suite proves VRRP
//! works; this proves an operator can see that it worked, which is the other
//! half of running a node rather than watching a log.
//!
//! Each test binds its own socket in a temporary directory, so the suite needs
//! no privileges and leaves nothing behind.

#![cfg(unix)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use highland_control::{
    ControlRequest, ControlResponse, PeerIdentity, Server, Service, SocketPolicy,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

/// A service that records what it was asked and answers from a fixed status.
#[derive(Debug, Default)]
struct RecordingService {
    seen: std::sync::Mutex<Vec<String>>,
    force_transition_enabled: bool,
    known: Vec<String>,
}

impl RecordingService {
    fn with_forced_transitions() -> Self {
        Self {
            force_transition_enabled: true,
            known: vec!["api".to_owned()],
            ..Self::default()
        }
    }
}

impl Service for RecordingService {
    fn handle(
        &self,
        request: ControlRequest,
        peer: PeerIdentity,
    ) -> impl std::future::Future<Output = ControlResponse> + Send {
        let mut seen = self.seen.lock().expect("the lock is not poisoned");
        let name = request
            .as_instance()
            .map_or_else(|| request.operation().to_owned(), ToOwned::to_owned);
        seen.push(format!("{name} by {}", peer.describe()));

        let response = match request {
            ControlRequest::Status | ControlRequest::Instances => {
                ControlResponse::ok(node_status("node-a"))
            }
            ControlRequest::Show { ref instance } if self.known.contains(instance) => {
                ControlResponse::ok(node_status("node-a"))
            }
            ControlRequest::Show { ref instance } => ControlResponse::error(
                "unknown_instance",
                format!("no instance named {instance:?}"),
            ),
            ControlRequest::ForceTransition {
                ref role, confirm, ..
            } => {
                if !self.force_transition_enabled {
                    ControlResponse::error("operation_disabled", "disabled")
                } else if !confirm {
                    ControlResponse::error("confirmation_required", "needs --enable")
                } else if role == "sideways" {
                    ControlResponse::error("unknown_role", format!("{role:?} is not a role"))
                } else {
                    ControlResponse::ok(node_status("node-a"))
                }
            }
            other => ControlResponse::error("not_implemented", other.operation()),
        };
        std::future::ready(response)
    }
}

fn node_status(node: &str) -> highland_control::NodeStatus {
    highland_control::NodeStatus {
        node: node.to_owned(),
        generation: 3,
        uptime_seconds: 12.5,
        instances: vec![highland_control::InstanceSummary {
            name: "api".to_owned(),
            role: "MASTER".to_owned(),
            priority: 150,
            effective_priority: 150,
            vip_addresses: vec!["192.0.2.100/24".to_owned()],
            vips_owned: true,
            health: "healthy".to_owned(),
            master_down_remaining_ms: None,
            preemption_remaining_ms: None,
            last_reason: "master_down_timeout".to_owned(),
        }],
    }
}

/// A socket path unique to this test binary and run.
///
/// The path is short on purpose. A Unix socket path is limited to 108 bytes, and
/// `std::env::temp_dir()` on macOS is long enough to blow through that, which is
/// a reminder that a configured path is a real operational constraint and not
/// just a string.
fn socket_path(name: &str) -> PathBuf {
    let serial = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    PathBuf::from(format!(
        "/tmp/hl{}-{serial}-{name}.sock",
        std::process::id()
    ))
}

static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Starts a server and returns its path and a handle to stop it.
async fn serve(
    service: Arc<RecordingService>,
    path: PathBuf,
    requests_per_second: u32,
) -> tokio::task::JoinHandle<()> {
    let policy = SocketPolicy {
        path: path.clone(),
        group: None,
        verify_peer_credentials: false,
        requests_per_second,
    };
    let server = Server::bind(service, policy)
        .await
        .expect("the socket binds");
    let handle = tokio::spawn(async move {
        let _ = server.serve().await;
    });
    // Give the listener a moment to be accept-ready; a Unix socket is bound
    // before the task runs, so this only settles the accept loop.
    tokio::time::sleep(Duration::from_millis(20)).await;
    handle
}

/// Sends one request and reads one response.
async fn ask(path: &PathBuf, request: &ControlRequest) -> ControlResponse {
    let stream = UnixStream::connect(path)
        .await
        .expect("the socket connects");
    let (reader, mut writer) = stream.into_split();
    let mut line = request.encode().expect("the request encodes");
    line.push('\n');
    writer
        .write_all(line.as_bytes())
        .await
        .expect("the request is written");

    let mut reader = BufReader::new(reader);
    let mut response = String::new();
    reader
        .read_line(&mut response)
        .await
        .expect("the response is read");
    ControlResponse::decode(response.trim_end()).expect("the response decodes")
}

#[tokio::test]
async fn a_client_asks_for_status_and_is_answered() {
    let path = socket_path("status");
    let handle = serve(Arc::new(RecordingService::default()), path.clone(), 20).await;

    let response = ask(&path, &ControlRequest::Status).await;
    let status = response.status().expect("a status");

    assert_eq!(status.node, "node-a");
    assert_eq!(status.instances.len(), 1);
    assert_eq!(status.instances[0].role, "MASTER");
    assert!(status.instances[0].vips_owned);

    drop(handle);
    let _ = tokio::fs::remove_file(&path).await;
}

#[tokio::test]
async fn the_socket_is_created_with_mode_0660() {
    use std::os::unix::fs::PermissionsExt as _;

    let path = socket_path("mode");
    let handle = serve(Arc::new(RecordingService::default()), path.clone(), 20).await;

    let mode = std::fs::metadata(&path)
        .expect("the socket exists")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        mode, 0o660,
        "S-03: a wider socket lets any local user administer the node"
    );

    drop(handle);
    let _ = tokio::fs::remove_file(&path).await;
}

#[tokio::test]
async fn a_malformed_request_is_refused_rather_than_ignored() {
    let path = socket_path("malformed");
    let handle = serve(Arc::new(RecordingService::default()), path.clone(), 20).await;

    let stream = UnixStream::connect(&path)
        .await
        .expect("the socket connects");
    let (reader, mut writer) = stream.into_split();
    writer
        .write_all(b"{not json at all\n")
        .await
        .expect("written");

    let mut reader = BufReader::new(reader);
    let mut response = String::new();
    reader
        .read_line(&mut response)
        .await
        .expect("a response comes back");
    let decoded = ControlResponse::decode(response.trim_end()).expect("the refusal decodes");

    assert!(
        matches!(decoded, ControlResponse::Error { .. }),
        "a refusal, not silence"
    );
    assert!(response.contains("malformed_request"));

    drop(handle);
    let _ = tokio::fs::remove_file(&path).await;
}

#[tokio::test]
async fn a_peer_that_asks_too_often_is_refused() {
    let path = socket_path("rate");
    let handle = serve(Arc::new(RecordingService::default()), path.clone(), 2).await;

    let stream = UnixStream::connect(&path)
        .await
        .expect("the socket connects");
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let request = ControlRequest::Status.encode().expect("encodes");
    let mut limited = false;

    for _ in 0..6 {
        writer
            .write_all(format!("{request}\n").as_bytes())
            .await
            .expect("written");
        let mut response = String::new();
        if reader.read_line(&mut response).await.expect("a response") == 0 {
            break;
        }
        if response.contains("rate_limited") {
            limited = true;
            break;
        }
    }

    assert!(
        limited,
        "L-12: a peer that floods must be refused, not served"
    );

    drop(handle);
    let _ = tokio::fs::remove_file(&path).await;
}

#[tokio::test]
async fn an_oversized_request_is_refused_before_it_is_parsed() {
    let path = socket_path("oversize");
    let handle = serve(Arc::new(RecordingService::default()), path.clone(), 20).await;

    let stream = UnixStream::connect(&path)
        .await
        .expect("the socket connects");
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    // A megabyte of valid JSON-shaped text: longer than the framing allows, so
    // it must be refused without being parsed.
    let mut huge = vec![b'{'];
    huge.extend(std::iter::repeat_n(b'a', 1_048_576));
    huge.extend_from_slice(b"}\n");
    writer.write_all(&huge).await.expect("written");

    // The refusal is sent first, and only then is the connection closed: a
    // client that sent too much deserves to be told why, not to see a hangup.
    let mut response = String::new();
    let read = tokio::time::timeout(Duration::from_secs(2), reader.read_line(&mut response))
        .await
        .expect("a response arrives rather than the connection hanging")
        .expect("the read succeeds");
    assert!(
        read > 0,
        "the refusal is written before the connection closes"
    );
    assert!(response.contains("request_too_large"), "got: {response}");

    let mut trailing = String::new();
    let closed = tokio::time::timeout(Duration::from_secs(2), reader.read_line(&mut trailing))
        .await
        .expect("the connection closes")
        .expect("the read succeeds");
    assert_eq!(closed, 0, "and then the connection is closed");

    drop(handle);
    let _ = tokio::fs::remove_file(&path).await;
}

#[tokio::test]
async fn force_transition_is_refused_unless_the_daemon_enabled_it() {
    let path = socket_path("forced");
    let handle = serve(Arc::new(RecordingService::default()), path.clone(), 20).await;

    let response = ask(
        &path,
        &ControlRequest::ForceTransition {
            instance: "api".to_owned(),
            role: "backup".to_owned(),
            confirm: true,
        },
    )
    .await;
    assert!(
        response
            .encode()
            .expect("encodes")
            .contains("operation_disabled")
    );

    drop(handle);
    let _ = tokio::fs::remove_file(&path).await;
}

#[tokio::test]
async fn a_forced_transition_needs_its_own_confirmation() {
    let path = socket_path("confirm");
    let handle = serve(
        Arc::new(RecordingService::with_forced_transitions()),
        path.clone(),
        20,
    )
    .await;

    let unconfirmed = ask(
        &path,
        &ControlRequest::ForceTransition {
            instance: "api".to_owned(),
            role: "backup".to_owned(),
            confirm: false,
        },
    )
    .await;
    assert!(
        unconfirmed
            .encode()
            .expect("encodes")
            .contains("confirmation_required")
    );

    let confirmed = ask(
        &path,
        &ControlRequest::ForceTransition {
            instance: "api".to_owned(),
            role: "backup".to_owned(),
            confirm: true,
        },
    )
    .await;
    assert!(confirmed.status().is_some());

    drop(handle);
    let _ = tokio::fs::remove_file(&path).await;
}

#[tokio::test]
async fn an_unknown_instance_is_named_in_the_refusal() {
    let path = socket_path("unknown");
    let handle = serve(Arc::new(RecordingService::default()), path.clone(), 20).await;

    let response = ask(
        &path,
        &ControlRequest::Show {
            instance: "nope".to_owned(),
        },
    )
    .await;
    let encoded = response.encode().expect("encodes");

    assert!(encoded.contains("unknown_instance"));
    assert!(
        encoded.contains("nope"),
        "the refusal names what was asked for: {encoded}"
    );

    drop(handle);
    let _ = tokio::fs::remove_file(&path).await;
}

#[tokio::test]
async fn a_stale_socket_from_a_previous_run_is_replaced() {
    let path = socket_path("stale");
    std::fs::write(&path, b"").expect("a stale file can be created");

    let handle = serve(Arc::new(RecordingService::default()), path.clone(), 20).await;
    let response = ask(&path, &ControlRequest::Status).await;

    assert!(
        response.status().is_some(),
        "the stale socket did not stop the server binding"
    );

    drop(handle);
    let _ = tokio::fs::remove_file(&path).await;
}

#[tokio::test]
async fn a_path_that_is_too_long_is_refused_with_a_sentence() {
    // A Unix socket path is limited to 108 bytes, and the platform's own error
    // says only that the path is too long. An operator deserves better, and a
    // long path is easy to arrive at with a long temporary directory.
    let path = PathBuf::from(format!("/tmp/{}.sock", "x".repeat(200)));
    let error = Server::bind(
        Arc::new(RecordingService::default()),
        SocketPolicy::new(path),
    )
    .await
    .expect_err("the path is too long");

    let rendered = error.to_string();
    assert!(
        rendered.contains("107"),
        "the message states the limit: {rendered}"
    );
    assert!(
        rendered.contains("too long") || rendered.contains("at most"),
        "got: {rendered}"
    );
}
