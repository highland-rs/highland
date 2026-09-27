// Rust guideline compliant 2026-09-27

//! The bounded event history.

use std::collections::VecDeque;

use crate::event::Event;

/// The number of events retained in memory (`L-08`).
pub const EVENT_BUFFER_CAPACITY: usize = 4096;

/// A fixed-capacity ring of recent events.
///
/// The buffer is bounded, so a flood of events cannot grow memory without
/// limit (SPEC.md, `S-05`, `L-08`). When it is full, the oldest event is
/// dropped and the drop is counted, never hidden.
#[derive(Debug, Clone)]
pub struct EventRing {
    events: VecDeque<Event>,
    capacity: usize,
    dropped: u64,
}

impl EventRing {
    /// Creates a ring with the standard capacity.
    #[must_use]
    pub fn new() -> Self {
        Self::with_capacity(EVENT_BUFFER_CAPACITY)
    }

    /// Creates a ring with an explicit capacity.
    ///
    /// # Panics
    ///
    /// Panics when `capacity` is zero, because a zero-capacity ring could
    /// never retain an event.
    pub fn with_capacity(capacity: usize) -> Self {
        assert!(capacity > 0, "EventRing requires a non-zero capacity");
        Self {
            events: VecDeque::with_capacity(capacity),
            capacity,
            dropped: 0,
        }
    }

    /// Appends an event, dropping the oldest one when full.
    pub fn push(&mut self, event: Event) {
        if self.events.len() == self.capacity {
            self.events.pop_front();
            self.dropped += 1;
        }
        self.events.push_back(event);
    }

    /// Returns the retained events, oldest first.
    #[must_use]
    pub fn events(&self) -> Vec<&Event> {
        self.events.iter().collect()
    }

    /// Returns the number of retained events.
    #[must_use]
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// Returns `true` when nothing is retained.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Returns how many events were dropped because the ring was full.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.dropped
    }
}

impl Default for EventRing {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{EventLevel, EventName};

    fn event(reason: &str) -> Event {
        Event::new(
            EventName::RoleTransition,
            EventLevel::Info,
            "node-a",
            Some("api".to_owned()),
            reason,
            "2027-01-03T12:00:14Z",
        )
    }

    #[test]
    fn events_are_retained_in_order() {
        let mut ring = EventRing::with_capacity(4);
        ring.push(event("first"));
        ring.push(event("second"));

        let reasons: Vec<&str> = ring
            .events()
            .into_iter()
            .map(|event| event.reason.as_str())
            .collect();
        assert_eq!(reasons, ["first", "second"]);
        assert_eq!(ring.dropped(), 0);
    }

    #[test]
    fn the_oldest_event_is_dropped_when_the_ring_is_full() {
        let mut ring = EventRing::with_capacity(2);
        ring.push(event("first"));
        ring.push(event("second"));
        ring.push(event("third"));

        let reasons: Vec<&str> = ring
            .events()
            .into_iter()
            .map(|event| event.reason.as_str())
            .collect();
        assert_eq!(reasons, ["second", "third"]);
        assert_eq!(ring.dropped(), 1);
        assert_eq!(ring.len(), 2);
    }

    #[test]
    #[should_panic(expected = "non-zero capacity")]
    fn a_zero_capacity_ring_is_rejected() {
        let _ = EventRing::with_capacity(0);
    }
}
