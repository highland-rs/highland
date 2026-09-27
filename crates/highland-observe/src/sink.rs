// Rust guideline compliant 2026-09-27

//! Where events go.

use std::sync::{Arc, Mutex};

use crate::event::Event;

/// A destination for events.
pub trait EventSink: Send + Sync + std::fmt::Debug {
    /// Publishes one event.
    fn publish(&self, event: Event);

    /// Publishes several events in order.
    fn publish_all(&self, events: Vec<Event>) {
        for event in events {
            self.publish(event);
        }
    }
}

/// A sink that discards everything, used before logging is configured.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopSink;

impl EventSink for NoopSink {
    fn publish(&self, _event: Event) {}
}

/// A sink that keeps events in memory, used by tests and by `highland events`.
#[derive(Debug, Clone, Default)]
pub struct RecordingSink {
    events: Arc<Mutex<Vec<Event>>>,
}

impl RecordingSink {
    /// Creates an empty sink.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the events recorded so far, in order.
    #[must_use]
    pub fn events(&self) -> Vec<Event> {
        self.events
            .lock()
            .map(|events| events.clone())
            .unwrap_or_default()
    }

    /// Returns the number of events recorded.
    #[must_use]
    pub fn len(&self) -> usize {
        self.events.lock().map_or(0, |events| events.len())
    }

    /// Returns `true` when nothing has been recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl EventSink for RecordingSink {
    fn publish(&self, event: Event) {
        // A poisoned lock means another thread panicked while holding it. The
        // event is dropped rather than propagating a panic into a logging call
        // on an unrelated task.
        if let Ok(mut events) = self.events.lock() {
            events.push(event);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{EventLevel, EventName};

    #[test]
    fn the_recording_sink_keeps_events_in_order() {
        let sink = RecordingSink::new();
        sink.publish_all(vec![
            Event::new(
                EventName::DaemonLifecycle,
                EventLevel::Info,
                "node-a",
                None,
                "startup",
                "t0",
            ),
            Event::new(
                EventName::DaemonLifecycle,
                EventLevel::Info,
                "node-a",
                None,
                "shutdown",
                "t1",
            ),
        ]);

        let reasons: Vec<String> = sink
            .events()
            .into_iter()
            .map(|event| event.reason)
            .collect();
        assert_eq!(reasons, ["startup", "shutdown"]);
        assert!(!sink.is_empty());
    }

    #[test]
    fn the_noop_sink_accepts_events_silently() {
        NoopSink.publish(Event::new(
            EventName::DaemonLifecycle,
            EventLevel::Info,
            "node-a",
            None,
            "startup",
            "t0",
        ));
    }
}
