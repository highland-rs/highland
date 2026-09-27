// Rust guideline compliant 2026-09-27

//! The structured event model.

use std::fmt;

use serde::{Deserialize, Serialize};

/// The severity of an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventLevel {
    /// Normal operation.
    Info,
    /// A condition the operator should know about.
    Warn,
    /// A condition that prevented an intended action.
    Error,
}

impl fmt::Display for EventLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            EventLevel::Info => "info",
            EventLevel::Warn => "warn",
            EventLevel::Error => "error",
        })
    }
}

/// A closed set of event names.
///
/// The set is closed because operators and dashboards depend on it
/// (SPEC.md, `R-17`). Adding a name is a specification change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum EventName {
    /// An instance changed role.
    RoleTransition,
    /// A health check entered the failing state.
    CheckFailing,
    /// A health check recovered.
    CheckRecovered,
    /// A single probe failed without changing the check's verdict.
    CheckProbeFailed,
    /// An instance became ineligible to hold ownership.
    InstanceIneligible,
    /// An instance became eligible again.
    InstanceEligible,
    /// The daemon accepted a configuration reload.
    ReloadAccepted,
    /// The daemon rejected a configuration reload.
    ReloadRejected,
    /// A peer became reachable.
    PeerReachable,
    /// A peer became unreachable.
    PeerUnreachable,
    /// An operator performed a control action.
    OperatorAction,
    /// A packet was rejected before reaching the state machine.
    PacketRejected,
    /// An action could not be applied.
    ActionFailed,
    /// The daemon started or stopped.
    DaemonLifecycle,
}

impl EventName {
    /// Returns the stable, lower-case name used in logs and JSON output.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            EventName::RoleTransition => "role_transition",
            EventName::CheckFailing => "check_failing",
            EventName::CheckRecovered => "check_recovered",
            EventName::CheckProbeFailed => "check_probe_failed",
            EventName::InstanceIneligible => "instance_ineligible",
            EventName::InstanceEligible => "instance_eligible",
            EventName::ReloadAccepted => "reload_accepted",
            EventName::ReloadRejected => "reload_rejected",
            EventName::PeerReachable => "peer_reachable",
            EventName::PeerUnreachable => "peer_unreachable",
            EventName::OperatorAction => "operator_action",
            EventName::PacketRejected => "packet_rejected",
            EventName::ActionFailed => "action_failed",
            EventName::DaemonLifecycle => "daemon_lifecycle",
        }
    }
}

impl fmt::Display for EventName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A field in an event payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum EventField {
    /// A boolean value.
    Bool(bool),
    /// An integer value.
    Int(i64),
    /// A floating-point value.
    Float(f64),
    /// A text value.
    Text(String),
}

impl From<&str> for EventField {
    fn from(value: &str) -> Self {
        EventField::Text(value.to_owned())
    }
}

impl From<String> for EventField {
    fn from(value: String) -> Self {
        EventField::Text(value)
    }
}

impl From<u8> for EventField {
    fn from(value: u8) -> Self {
        EventField::Int(i64::from(value))
    }
}

impl From<u64> for EventField {
    fn from(value: u64) -> Self {
        EventField::Int(i64::try_from(value).unwrap_or(i64::MAX))
    }
}

impl From<usize> for EventField {
    fn from(value: usize) -> Self {
        EventField::Int(i64::try_from(value).unwrap_or(i64::MAX))
    }
}

impl From<bool> for EventField {
    fn from(value: bool) -> Self {
        EventField::Bool(value)
    }
}

/// A structured event.
///
/// Events carry the node, the instance, the previous and new state, and a
/// reason, so that a role change can always be explained
/// (SPEC.md, §16.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    /// The event name.
    pub name: EventName,
    /// The severity.
    pub level: EventLevel,
    /// The node name.
    pub node: String,
    /// The instance name, when the event concerns one instance.
    pub instance: Option<String>,
    /// The role the instance occupied before the change.
    pub from: Option<String>,
    /// The role the instance occupies after the change.
    pub to: Option<String>,
    /// The machine-readable reason.
    pub reason: String,
    /// The additional fields.
    pub fields: Vec<(String, EventField)>,
    /// When the event occurred, in RFC 3339 UTC form.
    pub timestamp: String,
}

impl Event {
    /// Creates an event with the mandatory fields and no extras.
    #[must_use]
    pub fn new(
        name: EventName,
        level: EventLevel,
        node: impl Into<String>,
        instance: Option<String>,
        reason: impl Into<String>,
        timestamp: impl Into<String>,
    ) -> Self {
        Self {
            name,
            level,
            node: node.into(),
            instance,
            from: None,
            to: None,
            reason: reason.into(),
            fields: Vec::new(),
            timestamp: timestamp.into(),
        }
    }

    /// Records the state change this event describes.
    #[must_use]
    pub fn with_transition(mut self, from: impl Into<String>, to: impl Into<String>) -> Self {
        self.from = Some(from.into());
        self.to = Some(to.into());
        self
    }

    /// Adds a field.
    #[must_use]
    pub fn with_field(mut self, key: impl Into<String>, value: impl Into<EventField>) -> Self {
        self.fields.push((key.into(), value.into()));
        self
    }

    /// Returns the value of a field, if present.
    #[must_use]
    pub fn field(&self, key: &str) -> Option<&EventField> {
        self.fields
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event() -> Event {
        Event::new(
            EventName::RoleTransition,
            EventLevel::Info,
            "node-a",
            Some("api".to_owned()),
            "higher_priority_peer_advertisement",
            "2027-01-03T12:00:14.123Z",
        )
        .with_transition("MASTER", "BACKUP")
        .with_field("local_priority", 120_u8)
        .with_field("peer", "192.0.2.11")
    }

    #[test]
    fn an_event_carries_the_documented_fields() {
        let event = event();
        assert_eq!(event.name.as_str(), "role_transition");
        assert_eq!(event.from.as_deref(), Some("MASTER"));
        assert_eq!(event.to.as_deref(), Some("BACKUP"));
        assert_eq!(event.reason, "higher_priority_peer_advertisement");
        assert_eq!(
            event.field("peer"),
            Some(&EventField::Text("192.0.2.11".to_owned()))
        );
        assert_eq!(event.field("local_priority"), Some(&EventField::Int(120)));
    }

    #[test]
    fn events_round_trip_through_json() {
        let text = serde_json::to_string(&event()).expect("serializes");
        let decoded: Event = serde_json::from_str(&text).expect("deserializes");
        assert_eq!(decoded, event());
    }

    #[test]
    fn event_names_render_in_snake_case() {
        assert_eq!(EventName::ReloadRejected.to_string(), "reload_rejected");
        assert_eq!(EventLevel::Warn.to_string(), "warn");
    }
}
