// Rust guideline compliant 2026-09-27

//! The control request and response messages.
//!
//! One request and one response per line, encoded as JSON. The socket
//! transport itself arrives in Milestone 7; the message model and its encoding
//! are defined here so that the CLI and the daemon cannot disagree.

use serde::{Deserialize, Serialize};

/// The largest control request this release accepts (`L-14`).
pub const MAX_REQUEST_BYTES: usize = 64 * 1024;

/// A request sent by the CLI or another local client.
///
/// The variants match the operation table in `SPEC.md` §22.1 exactly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum ControlRequest {
    /// The whole-node status.
    Status,
    /// A summary of every instance.
    Instances,
    /// The full state of one instance.
    Show {
        /// The instance name.
        instance: String,
    },
    /// The event history, optionally following new events.
    Events {
        /// Return events after this sequence number.
        ///
        /// A client that has read up to a sequence sends it here, so a follower
        /// asks for what is new rather than re-reading the whole buffer on every
        /// poll.
        #[serde(default)]
        since: Option<u64>,
        /// The most to return.
        #[serde(default)]
        limit: Option<usize>,
        /// Whether the client intends to follow.
        ///
        /// The client polls with this set: the connection is answered once and
        /// the client asks again. Keeping the server from holding a connection
        /// open per follower is deliberate, because a client that disappeared
        /// mid-stream would otherwise leave a task waiting on a socket forever.
        #[serde(default)]
        follow: bool,
    },
    /// Re-read the configuration file.
    Reload,
    /// Stop an instance participating in election.
    Pause {
        /// The instance name.
        instance: String,
    },
    /// Resume a paused instance.
    Resume {
        /// The instance name.
        instance: String,
    },
    /// Ask a `MASTER` to relinquish its VIPs.
    Relinquish {
        /// The instance name.
        instance: String,
    },
    /// Force a role change. Disabled unless the daemon allows it.
    ForceTransition {
        /// The instance name.
        instance: String,
        /// The role to force.
        role: String,
        /// Must be true; the CLI requires an explicit `--enable`.
        confirm: bool,
    },
}

impl ControlRequest {
    /// The instance this request names, when it names one.
    #[must_use]
    pub fn as_instance(&self) -> Option<&str> {
        match self {
            ControlRequest::Show { instance }
            | ControlRequest::Pause { instance }
            | ControlRequest::Resume { instance }
            | ControlRequest::Relinquish { instance }
            | ControlRequest::ForceTransition { instance, .. } => Some(instance),
            ControlRequest::Status
            | ControlRequest::Instances
            | ControlRequest::Events { .. }
            | ControlRequest::Reload => None,
        }
    }

    /// The stable name of this operation, for an audit record.
    #[must_use]
    pub fn operation(&self) -> &'static str {
        match self {
            ControlRequest::Status => "status",
            ControlRequest::Instances => "instances",
            ControlRequest::Show { .. } => "show",
            ControlRequest::Events { .. } => "events",
            ControlRequest::Reload => "reload",
            ControlRequest::Pause { .. } => "pause",
            ControlRequest::Resume { .. } => "resume",
            ControlRequest::Relinquish { .. } => "relinquish",
            ControlRequest::ForceTransition { .. } => "force_transition",
        }
    }

    /// Returns `true` when the operation changes runtime state and therefore
    /// requires confirmation and an audit event (`R-28`).
    #[must_use]
    pub fn is_destructive(&self) -> bool {
        matches!(
            self,
            ControlRequest::Reload
                | ControlRequest::Pause { .. }
                | ControlRequest::Resume { .. }
                | ControlRequest::Relinquish { .. }
                | ControlRequest::ForceTransition { .. }
        )
    }

    /// Encodes the request as one protocol line, without the newline.
    ///
    /// # Errors
    ///
    /// Returns [`MessageError::TooLarge`] above [`MAX_REQUEST_BYTES`].
    pub fn encode(&self) -> std::result::Result<String, MessageError> {
        let text = serde_json::to_string(self).map_err(|error| MessageError::Malformed {
            reason: error.to_string(),
        })?;
        if text.len() > MAX_REQUEST_BYTES {
            return Err(MessageError::TooLarge {
                size: text.len(),
                limit: MAX_REQUEST_BYTES,
            });
        }
        Ok(text)
    }

    /// Decodes one protocol line into a request.
    ///
    /// # Errors
    ///
    /// Returns [`MessageError::TooLarge`] above [`MAX_REQUEST_BYTES`] and
    /// [`MessageError::Malformed`] for anything else.
    pub fn decode(line: &str) -> std::result::Result<Self, MessageError> {
        if line.len() > MAX_REQUEST_BYTES {
            return Err(MessageError::TooLarge {
                size: line.len(),
                limit: MAX_REQUEST_BYTES,
            });
        }
        serde_json::from_str(line).map_err(|error| MessageError::Malformed {
            reason: error.to_string(),
        })
    }
}

/// The state of one instance, as reported by the status API (§16.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstanceSummary {
    /// The instance name.
    pub name: String,
    /// The current role.
    pub role: String,
    /// The configured priority.
    pub priority: u8,
    /// The effective priority after health adjustment.
    pub effective_priority: u8,
    /// The configured VIPs.
    pub vip_addresses: Vec<String>,
    /// Whether the VIPs are currently present in the kernel.
    pub vips_owned: bool,
    /// The overall health verdict.
    pub health: String,
    /// Time left on the master-down timer, when armed.
    pub master_down_remaining_ms: Option<u64>,
    /// Time left on the preemption timer, when armed.
    pub preemption_remaining_ms: Option<u64>,
    /// The reason for the most recent transition.
    pub last_reason: String,
}

/// The whole-node status.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeStatus {
    /// The node name.
    pub node: String,
    /// The active configuration generation.
    pub generation: u64,
    /// How long the daemon has been running.
    pub uptime_seconds: f64,
    /// The instances this node manages.
    pub instances: Vec<InstanceSummary>,
}

/// A response to a control request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum ControlResponse {
    /// The request succeeded and carries a node status.
    Ok {
        /// The status.
        status: Box<NodeStatus>,
    },
    /// The request succeeded and carries an event stream as JSON lines.
    Events {
        /// The events, oldest first, each with its sequence number.
        events: Vec<serde_json::Value>,
        /// The sequence number of the newest event returned, which a client sends
        /// back as `since` on its next request.
        latest: u64,
        /// How many events were dropped from the history before `since`, because
        /// the ring overwrote them while the client was away.
        ///
        /// The history is bounded, so a client that is away long enough misses
        /// events, and without this the history a client receives is
        /// indistinguishable from a complete one. A client that sees a non-zero
        /// count knows its view has a gap, and can say so rather than reporting a
        /// quiet period that never happened.
        #[serde(default)]
        dropped: u64,
    },
    /// The request was refused.
    Error {
        /// The machine-readable reason.
        reason: String,
        /// The operator-facing message.
        message: String,
    },
}

impl ControlResponse {
    /// Builds a success response.
    #[must_use]
    pub fn ok(status: NodeStatus) -> Self {
        ControlResponse::Ok {
            status: Box::new(status),
        }
    }

    /// Builds an error response.
    #[must_use]
    pub fn error(reason: impl Into<String>, message: impl Into<String>) -> Self {
        ControlResponse::Error {
            reason: reason.into(),
            message: message.into(),
        }
    }

    /// Returns the node status, when this is a success response.
    #[must_use]
    pub fn status(&self) -> Option<&NodeStatus> {
        match self {
            ControlResponse::Ok { status } => Some(status),
            _ => None,
        }
    }

    /// Encodes the response as one protocol line, without the newline.
    ///
    /// # Errors
    ///
    /// Returns [`MessageError::Malformed`] when the response cannot be
    /// encoded.
    pub fn encode(&self) -> std::result::Result<String, MessageError> {
        serde_json::to_string(self).map_err(|error| MessageError::Malformed {
            reason: error.to_string(),
        })
    }

    /// Decodes one protocol line into a response.
    ///
    /// # Errors
    ///
    /// Returns [`MessageError::Malformed`] for anything else.
    pub fn decode(line: &str) -> std::result::Result<Self, MessageError> {
        serde_json::from_str(line).map_err(|error| MessageError::Malformed {
            reason: error.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_round_trip_through_the_wire_format() {
        let request = ControlRequest::Relinquish {
            instance: "api".to_owned(),
        };
        let line = request.encode().expect("encodes");
        assert_eq!(ControlRequest::decode(&line).expect("decodes"), request);
    }

    #[test]
    fn every_operation_encodes_with_its_tagged_name() {
        let requests = [
            ControlRequest::Status,
            ControlRequest::Instances,
            ControlRequest::Show {
                instance: "api".to_owned(),
            },
            ControlRequest::Events {
                limit: Some(10),
                follow: true,
                since: Some(0),
            },
            ControlRequest::Reload,
            ControlRequest::Pause {
                instance: "api".to_owned(),
            },
            ControlRequest::Resume {
                instance: "api".to_owned(),
            },
            ControlRequest::Relinquish {
                instance: "api".to_owned(),
            },
            ControlRequest::ForceTransition {
                instance: "api".to_owned(),
                role: "backup".to_owned(),
                confirm: true,
            },
        ];

        for request in requests {
            let line = request.encode().expect("encodes");
            assert!(
                line.starts_with("{\"command\":"),
                "unexpected encoding: {line}"
            );
            assert_eq!(ControlRequest::decode(&line).expect("decodes"), request);
        }
    }

    #[test]
    fn destructive_operations_are_identified() {
        assert!(!ControlRequest::Status.is_destructive());
        assert!(
            !ControlRequest::Show {
                instance: "api".to_owned()
            }
            .is_destructive()
        );
        assert!(ControlRequest::Reload.is_destructive());
        assert!(
            ControlRequest::Relinquish {
                instance: "api".to_owned()
            }
            .is_destructive()
        );
    }

    #[test]
    fn an_oversized_request_is_refused() {
        let request = ControlRequest::Show {
            instance: "x".repeat(MAX_REQUEST_BYTES),
        };
        assert!(matches!(
            request.encode(),
            Err(MessageError::TooLarge { .. })
        ));
    }

    #[test]
    fn a_malformed_request_names_the_problem() {
        let error = ControlRequest::decode("{not json").expect_err("rejects");
        assert!(matches!(error, MessageError::Malformed { .. }));
    }

    #[test]
    fn responses_round_trip() {
        let response = ControlResponse::error("unknown_instance", "no instance named \"api\"");
        let line = response.encode().expect("encodes");
        assert_eq!(ControlResponse::decode(&line).expect("decodes"), response);
    }

    #[test]
    fn a_status_response_exposes_its_status() {
        let status = NodeStatus {
            node: "node-a".to_owned(),
            generation: 7,
            uptime_seconds: 1.5,
            instances: Vec::new(),
        };
        let response = ControlResponse::ok(status);
        assert_eq!(response.status().map(|status| status.generation), Some(7));
    }
}

/// Why a control message could not be encoded or decoded.
///
/// The message types carry their own error rather than the transport's, so the
/// CLI can decode a response without depending on anything the server owns.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum MessageError {
    /// The message is larger than the framing allows.
    #[error("a control message may be at most {limit} bytes, not {size}")]
    TooLarge {
        /// The size that was offered.
        size: usize,
        /// The limit that applies.
        limit: usize,
    },

    /// The bytes are not a valid control message.
    #[error("not a valid control message: {reason}")]
    Malformed {
        /// What went wrong.
        reason: String,
    },
}
