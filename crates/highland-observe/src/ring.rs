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

    /// Returns how many events were dropped at or before `sequence`.
    ///
    /// This is the number a *client* needs, and it is what `dropped` alone
    /// cannot answer: a follower that reconnects with a cursor and finds the
    /// total has gone up does not know whether the loss happened before or after
    /// where it was. The count of events that fell off the front of the ring and
    /// are numbered at or below `sequence` is exactly the gap that client cannot
    /// see, because those events are no longer in the buffer to be counted.
    ///
    /// With no such event, zero.
    #[must_use]
    pub fn dropped_through(&self, sequence: u64) -> u64 {
        self.events.front().map_or(self.dropped, |(oldest, _)| {
            // The retained window starts at `oldest`, so everything numbered
            // below it is gone. A client that has already seen up to `sequence`
            // missed exactly the gone events numbered above `sequence`, which is
            // `oldest - 1 - sequence`, bounded by everything ever dropped so a
            // cursor past the end cannot inflate it.
            let last_gone = oldest.saturating_sub(1);
            last_gone.saturating_sub(sequence).min(self.dropped)
        })
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
            crate::timestamp::now(),
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
        assert_eq!(ring.since(0, 10), Vec::new(), "nothing is retained yet");
    }

    /// A client resuming from a cursor that has fallen off the front of the ring
    /// must be able to learn that it missed something.
    #[test]
    fn dropped_through_reports_the_gap_a_client_cannot_see() {
        let mut ring = EventRing::with_capacity(2);
        ring.push(event("one")); // sequence 1
        ring.push(event("two")); // sequence 2
        ring.push(event("three")); // sequence 3, drops 1
        ring.push(event("four")); // sequence 4, drops 2

        assert_eq!(ring.dropped(), 2);
        // Retained are 3 and 4. A client whose cursor is 2 has seen everything
        // up to 2, and 3 and 4 are both still here, so it missed nothing.
        assert_eq!(
            ring.dropped_through(2),
            0,
            "a client inside the window missed nothing"
        );
        // A client at sequence 0 has lost everything before 3.
        assert_eq!(
            ring.dropped_through(0),
            2,
            "a client from the start missed the first two"
        );
        // A cursor past the end has already seen everything it could, so there is
        // no gap to report -- and the count must not grow with the cursor.
        assert_eq!(
            ring.dropped_through(9_999),
            0,
            "a cursor past the end missed nothing"
        );
    }

    /// Before anything is dropped there is no gap to report, whatever the cursor.
    #[test]
    fn dropped_through_is_zero_while_nothing_has_been_dropped() {
        let mut ring = EventRing::with_capacity(8);
        ring.push(event("one"));
        ring.push(event("two"));
        assert_eq!(ring.dropped_through(0), 0);
        assert_eq!(ring.dropped_through(1), 0);
        assert_eq!(ring.dropped_through(999), 0);
    }

    /// A cursor one below the oldest retained event has missed exactly the one
    /// that was dropped, and one above it has missed nothing: the boundary.
    #[test]
    fn dropped_through_is_exact_at_the_boundary() {
        let mut ring = EventRing::with_capacity(2);
        for reason in ["one", "two", "three", "four"] {
            ring.push(event(reason));
        }
        // Retained are 3 and 4; 1 and 2 are gone.
        assert_eq!(ring.dropped_through(0), 2, "missed both");
        assert_eq!(ring.dropped_through(1), 1, "missed one");
        assert_eq!(ring.dropped_through(2), 0, "missed none");
    }

    /// An empty ring has dropped nothing, and must not report a gap.
    #[test]
    fn dropped_through_on_an_empty_ring_is_zero() {
        let ring = EventRing::with_capacity(4);
        assert_eq!(ring.dropped_through(0), 0);
        assert_eq!(ring.dropped_through(42), 0);
    }
}
