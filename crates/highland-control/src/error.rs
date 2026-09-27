// Rust guideline compliant 2026-09-27

//! Control-API error types.

use thiserror::Error;

use crate::message::MAX_REQUEST_BYTES;

/// Result alias for fallible control operations.
pub type Result<T, E = ControlError> = std::result::Result<T, E>;

/// An error raised by the control API.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ControlError {
    /// The socket could not be created, bound, or connected.
    #[error("control socket {path} could not be used: {source}")]
    Socket {
        /// The socket path.
        path: String,
        /// The operating-system error.
        source: std::io::Error,
    },

    /// The socket exists with permissions that are too permissive.
    #[error("control socket {path} has mode {mode:o}; it must not be world-writable")]
    InsecureSocket {
        /// The socket path.
        path: String,
        /// The observed mode.
        mode: u32,
    },

    /// A request exceeded the size limit (`L-14`).
    #[error("request is {size} bytes, above the {MAX_REQUEST_BYTES} byte limit")]
    RequestTooLarge {
        /// The observed size.
        size: usize,
    },

    /// A request was not valid JSON, or did not match the schema.
    #[error("request is not a valid control request: {message}")]
    MalformedRequest {
        /// The diagnostic.
        message: String,
    },

    /// The request referenced an instance the daemon does not have.
    #[error("no instance named {name:?}")]
    UnknownInstance {
        /// The requested name.
        name: String,
    },

    /// The operation requires a flag the daemon was not started with.
    #[error("{operation} is disabled; start the daemon with {flag}")]
    OperationDisabled {
        /// The operation that was refused.
        operation: &'static str,
        /// The flag that would enable it.
        flag: &'static str,
    },

    /// The peer exceeded the request rate limit (`L-12`).
    #[error("rate limit exceeded: at most {limit} requests per second are accepted")]
    RateLimited {
        /// The configured limit.
        limit: u32,
    },

    /// The peer is not permitted to use the socket.
    #[error("peer credentials are not permitted to use the control socket")]
    Unauthorized,
}
