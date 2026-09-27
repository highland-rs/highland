// Rust guideline compliant 2026-09-27

//! The control socket server.
//!
//! The server is a Unix domain socket speaking newline-delimited JSON, and it
//! knows nothing about VRRP. It authenticates a peer, bounds what a peer can
//! ask for, decodes a [`ControlRequest`], and hands it to a [`Service`]. The
//! daemon supplies the service; this file supplies everything around it.
//!
//! # What the server guarantees
//!
//! - The socket is created with mode `0660`, never wider (`S-03`).
//! - A world-writable socket is refused at startup rather than served.
//! - Each peer is rate limited (`L-12`), and a peer that exceeds it gets a
//!   refusal, not silence.
//! - A request larger than [`MAX_REQUEST_BYTES`] is refused before it is parsed
//!   (`L-14`).
//! - A malformed request produces a typed refusal, never a panic and never a
//!   dropped connection.
//! - Nothing is exposed over a network transport, and there is no code path
//!   that could (`S-02`).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::time::timeout;

use crate::error::{ControlError, Result};
use crate::message::{ControlRequest, ControlResponse, MAX_REQUEST_BYTES};
use crate::rate::RateLimiter;

/// Answers control requests.
///
/// The daemon implements this. The trait exists so the server has no opinion
/// about VRRP, about instances, or about what a reload means.
// `Service` is `Send + Sync + 'static` because the server hands the service to
// a task per connection, and a borrowed service could not outlive it.
pub trait Service: Send + Sync + std::fmt::Debug + 'static {
    /// Handles one request.
    fn handle(
        &self,
        request: ControlRequest,
        peer: PeerIdentity,
    ) -> impl std::future::Future<Output = ControlResponse> + Send;
}

/// Who a request came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerIdentity {
    /// The peer's user id, when the platform reports one.
    pub uid: Option<u32>,
    /// The peer's group id, when the platform reports one.
    pub gid: Option<u32>,
    /// The peer's process id, when the platform reports one.
    pub pid: Option<u32>,
}

impl PeerIdentity {
    /// Returns a short description for an audit record.
    #[must_use]
    pub fn describe(self) -> String {
        match (self.uid, self.pid) {
            (Some(uid), Some(pid)) => format!("uid {uid} pid {pid}"),
            (Some(uid), None) => format!("uid {uid}"),
            _ => "an unidentified local process".to_owned(),
        }
    }
}

/// How the socket is created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SocketPolicy {
    /// The path to create.
    pub path: PathBuf,
    /// The group allowed to use the socket, by name.
    pub group: Option<String>,
    /// Whether to verify the peer's credentials.
    pub verify_peer_credentials: bool,
    /// How many requests a peer may make per second.
    pub requests_per_second: u32,
}

impl SocketPolicy {
    /// The default policy, matching the configuration defaults in
    /// `SPEC.md` §10.3.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            group: None,
            verify_peer_credentials: true,
            requests_per_second: 20,
        }
    }
}

/// The control socket server.
#[derive(Debug)]
pub struct Server<S> {
    listener: UnixListener,
    service: Arc<S>,
    policy: SocketPolicy,
    verify_peer_credentials: bool,
    requests_per_second: u32,
}

impl<S: Service> Server<S> {
    /// Binds the socket described by `policy`.
    ///
    /// # Errors
    ///
    /// Returns [`ControlError::Socket`] when the socket cannot be created, and
    /// [`ControlError::InsecureSocket`] when an existing socket is world
    /// writable, which is refused rather than served (`S-03`).
    pub async fn bind(service: Arc<S>, policy: SocketPolicy) -> Result<Self> {
        check_path_length(&policy.path)?;

        // A socket left behind by a previous run would be reused with the old
        // permissions, so it goes first.
        match tokio::fs::remove_file(&policy.path).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(ControlError::Socket {
                    path: policy.path.display().to_string(),
                    reason: error.to_string(),
                });
            }
        }

        let listener = UnixListener::bind(&policy.path).map_err(|error| ControlError::Socket {
            path: policy.path.display().to_string(),
            reason: error.to_string(),
        })?;
        apply_permissions(&policy).await?;

        Ok(Self {
            listener,
            service,
            verify_peer_credentials: policy.verify_peer_credentials,
            requests_per_second: policy.requests_per_second,
            policy,
        })
    }

    /// Returns the path this server listens on.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.policy.path
    }

    /// Serves until the process ends.
    ///
    /// Each connection is handled on its own task, so a client that stops
    /// reading cannot hold up another (`I-41`).
    ///
    /// # Errors
    ///
    /// Returns [`ControlError::Socket`] when accepting a connection fails. A
    /// failure to accept is fatal for the socket but not for the daemon, so
    /// the caller decides what to do.
    pub async fn serve(self) -> Result<()> {
        let service = Arc::clone(&self.service);
        let verify_peer_credentials = self.verify_peer_credentials;
        let requests_per_second = self.requests_per_second;
        let path = self.policy.path.clone();

        loop {
            let (stream, _address) =
                self.listener
                    .accept()
                    .await
                    .map_err(|error| ControlError::Socket {
                        path: path.display().to_string(),
                        reason: error.to_string(),
                    })?;
            let connection_service = Arc::clone(&service);
            let connection_path = path.clone();
            tokio::spawn(async move {
                let identity = if verify_peer_credentials {
                    peer_identity(&stream)
                } else {
                    None
                };
                if let Err(error) =
                    serve_connection(stream, connection_service, identity, requests_per_second)
                        .await
                {
                    // A failed connection is logged by the caller; the server
                    // keeps running, because one bad client is not a reason to
                    // stop administering a node.
                    tracing::warn!(
                        socket = %connection_path.display(),
                        error = %error,
                        "control connection ended"
                    );
                }
            });
        }
    }

    /// Serves one already-accepted connection.
    ///
    /// Split out so a test can drive the protocol over a socket pair without
    /// binding a path, and so the framing is testable on its own.
    ///
    /// # Errors
    ///
    /// Returns [`ControlError::Io`] when the connection cannot be read or
    /// written.
    pub async fn serve_connection(
        &self,
        stream: UnixStream,
        identity: Option<PeerIdentity>,
    ) -> Result<()> {
        serve_connection(
            stream,
            Arc::clone(&self.service),
            identity,
            self.requests_per_second,
        )
        .await
    }
}

async fn serve_connection<S: Service>(
    stream: UnixStream,
    service: Arc<S>,
    identity: Option<PeerIdentity>,
    requests_per_second: u32,
) -> Result<()> {
    let peer = identity.unwrap_or(PeerIdentity {
        uid: None,
        gid: None,
        pid: None,
    });
    let mut limiter = RateLimiter::per_second(requests_per_second.max(1));
    // The limiter stays driven by an explicit elapsed time rather than a clock
    // of its own, so the policy is testable without sleeping.
    let opened = std::time::Instant::now();
    let mut line = String::new();

    // The stream is split so that reading and writing do not each need the
    // whole socket, which is what `read_line` requires: `UnixStream` is not
    // itself buffered.
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    loop {
        line.clear();
        // A client that connects and says nothing must not hold this task
        // forever (`READ_TIMEOUT`).
        let read = timeout(READ_TIMEOUT, reader.read_line(&mut line))
            .await
            .map_err(|_| ControlError::Io {
                reason: "the client sent nothing and was timed out".to_owned(),
            })
            .and_then(|read| read.map_err(io_error))?;
        if read == 0 {
            // The client closed cleanly.
            return Ok(());
        }
        if line.len() > MAX_REQUEST_BYTES {
            return write(
                &mut writer,
                &ControlResponse::error(
                    "request_too_large",
                    format!("a request may be at most {MAX_REQUEST_BYTES} bytes"),
                ),
            )
            .await;
        }
        let elapsed = opened.elapsed().min(Duration::from_secs(3_600));
        if !limiter.admit(elapsed) {
            return write(
                &mut writer,
                &ControlResponse::error(
                    "rate_limited",
                    format!("at most {requests_per_second} requests per second are accepted"),
                ),
            )
            .await;
        }

        let response = match ControlRequest::decode(line.trim_end()) {
            Ok(request) => service.handle(request, peer).await,
            Err(error) => ControlResponse::error("malformed_request", error.to_string()),
        };
        write(&mut writer, &response).await?;
    }
}

async fn write(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    response: &ControlResponse,
) -> Result<()> {
    let mut encoded = response.encode().map_err(|error| ControlError::Protocol {
        reason: error.to_string(),
    })?;
    encoded.push('\n');
    writer
        .write_all(encoded.as_bytes())
        .await
        .map_err(io_error)?;
    writer.flush().await.map_err(io_error)
}

// The error is consumed rather than borrowed, so the signature takes it by
// value; the lint is answered here rather than with a blanket allow.
#[expect(
    clippy::needless_pass_by_value,
    reason = "map_err hands ownership over"
)]
fn io_error(error: std::io::Error) -> ControlError {
    ControlError::Io {
        reason: error.to_string(),
    }
}

/// Reads the peer's credentials, when the platform provides them.
///
/// The `peer_cred` shape differs between Linux and the BSDs, so the pieces are
/// taken separately: a platform that reports a user but no process still yields
/// a useful audit record rather than nothing.
#[cfg(target_os = "linux")]
fn peer_identity(stream: &UnixStream) -> Option<PeerIdentity> {
    let credentials = stream.peer_cred().ok()?;
    // On Linux `peer_cred` reports all three; the conversions are defensive
    // because a future platform may not.
    let raw_pid = credentials.pid();
    Some(PeerIdentity {
        uid: Some(credentials.uid()),
        gid: Some(credentials.gid()),
        pid: raw_pid.and_then(|pid| u32::try_from(pid).ok()),
    })
}

#[cfg(all(unix, not(target_os = "linux")))]
fn peer_identity(_stream: &UnixStream) -> Option<PeerIdentity> {
    None
}

/// Sets the socket's mode and, when configured, its group.
///
/// A socket left at the process umask is often `0755`, which lets any local user
/// connect. `0660` plus a group is the documented contract (`S-03`).
async fn apply_permissions(policy: &SocketPolicy) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let metadata =
        tokio::fs::metadata(&policy.path)
            .await
            .map_err(|error| ControlError::Socket {
                path: policy.path.display().to_string(),
                reason: error.to_string(),
            })?;
    let mode = metadata.permissions().mode() & 0o777;
    if mode & 0o002 != 0 {
        return Err(ControlError::InsecureSocket {
            path: policy.path.display().to_string(),
            mode,
        });
    }

    tokio::fs::set_permissions(&policy.path, std::fs::Permissions::from_mode(0o660))
        .await
        .map_err(|error| ControlError::Socket {
            path: policy.path.display().to_string(),
            reason: error.to_string(),
        })?;

    if let Some(group) = &policy.group {
        chown_to_group(&policy.path, group).await?;
    }
    Ok(())
}

/// Gives the socket to `group`.
///
/// A group that does not exist is an error rather than a warning: a socket the
/// operator believes their group can use, and cannot, is a support ticket.
#[cfg(unix)]
async fn chown_to_group(path: &Path, name: &str) -> Result<()> {
    use std::os::unix::fs::MetadataExt as _;

    let group = nix::unistd::Group::from_name(name)
        .map_err(|error| ControlError::Group {
            group: name.to_owned(),
            reason: error.to_string(),
        })?
        .ok_or_else(|| ControlError::Group {
            group: name.to_owned(),
            reason: "no such group".to_owned(),
        })?;

    let owner = tokio::fs::metadata(path)
        .await
        .map_err(|error| ControlError::Socket {
            path: path.display().to_string(),
            reason: error.to_string(),
        })?;

    // The owner is left alone; only the group changes, so the process that
    // created the socket can still remove it.
    nix::unistd::chown(
        path,
        Some(nix::unistd::Uid::from_raw(owner.uid())),
        Some(group.gid),
    )
    .map_err(|error| ControlError::Socket {
        path: path.display().to_string(),
        reason: error.to_string(),
    })
}

/// The longest path a Unix domain socket may have, including the terminating
/// zero (`SUN_LEN` on Linux, and the same limit on the BSDs).
///
/// This is a platform limit, not a policy one: a longer path is refused by
/// `bind` with a message that says only that the path is too long. Checking it
/// first turns a confusing error into a sentence an operator can act on.
pub const MAX_SOCKET_PATH: usize = 107;

/// Returns an error when `path` is too long to bind.
fn check_path_length(path: &Path) -> Result<()> {
    let length = path.as_os_str().len();
    if length > MAX_SOCKET_PATH {
        return Err(ControlError::SocketPathTooLong {
            path: path.display().to_string(),
            length,
        });
    }
    Ok(())
}

/// How long a client may take to send a request.
///
/// Without it, a client that connects and says nothing holds a task forever.
pub const READ_TIMEOUT: Duration = Duration::from_secs(5);
