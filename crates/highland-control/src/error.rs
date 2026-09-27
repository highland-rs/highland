// Rust guideline compliant 2026-09-27

//! Control-API error types.

use thiserror::Error;

use crate::server::MAX_SOCKET_PATH;

/// Result alias for fallible control operations.
pub type Result<T, E = ControlError> = std::result::Result<T, E>;

/// An error raised by the control API.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ControlError {
    /// The socket could not be created, bound, or used.
    #[error("control socket {path} could not be used: {reason}")]
    Socket {
        /// The socket path.
        path: String,
        /// What the operating system reported.
        reason: String,
    },

    /// The socket path is longer than the platform allows.
    #[error(
        "control socket path {path} is {length} bytes; a Unix socket path may be at most {MAX_SOCKET_PATH}"
    )]
    SocketPathTooLong {
        /// The path that was offered.
        path: String,
        /// How long it is.
        length: usize,
    },

    /// The socket exists with permissions that are too permissive.
    #[error("control socket {path} has mode {mode:o}; it must not be world-writable")]
    InsecureSocket {
        /// The socket path.
        path: String,
        /// The mode that was observed.
        mode: u32,
    },

    /// The configured group does not exist.
    #[error("group {group} could not be resolved: {reason}")]
    Group {
        /// The group name from the configuration.
        group: String,
        /// Why it could not be resolved.
        reason: String,
    },

    /// Reading or writing a connection failed.
    #[error("control connection failed: {reason}")]
    Io {
        /// What the operating system reported.
        reason: String,
    },

    /// A message could not be encoded or decoded.
    #[error("control protocol error: {reason}")]
    Protocol {
        /// What went wrong.
        reason: String,
    },
}
