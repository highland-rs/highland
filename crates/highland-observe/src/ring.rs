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
    events: VecDeque<(u64, Event)>,
    capacity: usize,
    dropped: u64,
    next: u64,
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
            next: 1,
        }
    }

    /// Appends an event, dropping the oldest one when full.
    ///
    /// Every event is given a sequence number, so a client that has read up to
    /// one sequence can ask for what came after it. Without that, a follower
    /// either re-reads the whole buffer on every poll or has to guess.
    pub fn push(&mut self, event: Event) -> u64 {
        if self.events.len() == self.capacity {
            self.events.pop_front();
            self.dropped += 1;
        }
        let sequence = self.next;
        self.next += 1;
        self.events.push_back((sequence, event));
        sequence
    }

    /// Returns the retained events, oldest first.
    #[must_use]
    pub fn events(&self) -> Vec<&Event> {
        self.events.iter().map(|(_, event)| event).collect()
    }

    /// Returns the events after `sequence`, oldest first.
    ///
    /// A `sequence` older than the oldest retained event returns everything
    /// retained, because the client asked for something that no longer exists
    /// and silently returning nothing would look like a quiet daemon.
    #[must_use]
    pub fn since(&self, sequence: u64, limit: usize) -> Vec<(u64, &Event)> {
        let oldest = self.events.front().map_or(0, |(sequence, _)| *sequence);
        let floor = sequence.saturating_add(1).max(oldest);
        self.events
            .iter()
            .filter(|(number, _)| *number >= floor)
            .take(limit)
            .map(|(number, event)| (*number, event))
            .collect()
    }

    /// Returns the sequence number of the newest event, or zero when there is
    /// none.
    #[must_use]
    pub fn latest(&self) -> u64 {
        self.events.back().map_or(0, |(sequence, _)| *sequence)
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

#[cfg(test)]
mod sequence_tests {
    use super::*;
    use crate::event::{Event, EventLevel, EventName};

    fn event(reason: &str) -> Event {
        Event::new(
            EventName::RoleTransition,
            EventLevel::Info,
            "node-a",
            Some("api".to_owned()),
            reason,
            "1970-01-01T00:00:00Z",
        )
    }

    #[test]
    fn every_event_gets_a_sequence_that_increases() {
        let mut ring = EventRing::with_capacity(4);
        assert_eq!(ring.push(event("first")), 1);
        assert_eq!(ring.push(event("second")), 2);
        assert_eq!(ring.latest(), 2);
    }

    #[test]
    fn since_returns_only_what_a_client_has_not_seen() {
        let mut ring = EventRing::with_capacity(8);
        ring.push(event("first"));
        ring.push(event("second"));
        ring.push(event("third"));

        let after_first: Vec<String> = ring
            .since(1, 10)
            .into_iter()
            .map(|(_, event)| event.reason.clone())
            .collect();
        assert_eq!(after_first, ["second", "third"]);

        assert!(
            ring.since(3, 10).is_empty(),
            "nothing is newer than the last read"
        );
    }

    #[test]
    fn since_respects_a_limit() {
        let mut ring = EventRing::with_capacity(8);
        for index in 0..5 {
            ring.push(event(&format!("event {index}")));
        }
        assert_eq!(
            ring.since(0, 2).len(),
            2,
            "a follower asks for what it needs, not everything"
        );
    }

    #[test]
    fn a_sequence_older_than_the_buffer_returns_what_is_left() {
        let mut ring = EventRing::with_capacity(2);
        for index in 0..5 {
            ring.push(event(&format!("event {index}")));
        }

        // The client asked for events that have been dropped. Returning what is
        // retained is honest; returning nothing would look like a quiet daemon.
        let recovered = ring.since(0, 10);
        assert_eq!(recovered.len(), 2);
        assert_eq!(recovered[0].0, 4, "the oldest retained sequence");
    }

    #[test]
    fn an_empty_ring_reports_nothing_new() {
        let ring = EventRing::with_capacity(4);
        assert_eq!(ring.latest(), 0);
        assert!(ring.since(0, 10).is_empty());
    }
}
