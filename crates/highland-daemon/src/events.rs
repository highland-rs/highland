// Rust guideline compliant 2026-09-27

//! The event log the control API and `highland events` read.
//!
//! Every instance publishes what it did here as it happens, so the history an
//! operator reads is the history that occurred, not a reconstruction from the
//! current state. The log is bounded (`L-08`) and every entry carries a
//! sequence number, so a follower can ask for what it has not seen.

use std::sync::Mutex;

use highland_observe::{Event, EventName, EventRing};

use crate::control::InstanceStatus;

/// The shared, bounded event history.
#[derive(Debug)]
pub struct EventLog {
    ring: Mutex<EventRing>,
}

impl Default for EventLog {
    fn default() -> Self {
        Self::new()
    }
}

impl EventLog {
    /// Creates an empty log with the standard capacity.
    #[must_use]
    pub fn new() -> Self {
        Self {
            ring: Mutex::new(EventRing::new()),
        }
    }

    /// Records an event, returning its sequence number.
    ///
    /// A poisoned lock is recovered from: losing an event is better than
    /// stopping a node that owns an address.
    pub fn record(&self, event: Event) -> u64 {
        self.lock().push(event)
    }

    /// Records a role change.
    pub fn record_transition(
        &self,
        node: &str,
        instance: &str,
        from: &str,
        to: &str,
        reason: &str,
    ) -> u64 {
        self.record(
            Event::new(
                EventName::RoleTransition,
                highland_observe::EventLevel::Info,
                node,
                Some(instance.to_owned()),
                reason,
                highland_observe::now_timestamp(),
            )
            .with_transition(from, to)
            .with_field("peer", "")
            .with_field("last_reason", reason),
        )
    }

    /// Records the outcome of a reload, and who asked for it.
    ///
    /// A reload is the most consequential thing an operator can do to a running
    /// node -- it can change priorities, start instances, and stop them -- and
    /// `SPEC.md` §22.1 lists it as destructive, so `R-28` requires an audit event
    /// naming the peer credential. `initiator` is the peer's description for a
    /// request over the control socket, and `SIGHUP` for the signal, which has no
    /// peer to name.
    pub fn record_reload(
        &self,
        node: &str,
        accepted: bool,
        generation: Option<u64>,
        summary: &str,
        initiator: &str,
    ) -> u64 {
        let name = if accepted {
            EventName::ReloadAccepted
        } else {
            EventName::ReloadRejected
        };
        let level = if accepted {
            highland_observe::EventLevel::Info
        } else {
            // A refusal is not a routine outcome, and an operator reading the
            // history should not have to compare sequence numbers to notice that
            // their reload did nothing.
            highland_observe::EventLevel::Warn
        };
        self.record(
            Event::new(
                name,
                level,
                node,
                None,
                summary,
                highland_observe::now_timestamp(),
            )
            .with_field("peer", initiator)
            .with_field(
                "generation",
                generation.map_or_else(|| "unchanged".to_owned(), |g| g.to_string()),
            ),
        )
    }

    /// Records an event the machine emitted under a name of its own, mapping it
    /// onto the closed event set.
    pub fn record_named(
        &self,
        node: &str,
        instance: &str,
        name: highland_observe::EventName,
        reason: &str,
    ) -> u64 {
        self.record(Event::new(
            name,
            highland_observe::EventLevel::Info,
            node,
            Some(instance.to_owned()),
            reason,
            highland_observe::now_timestamp(),
        ))
    }

    /// Records that an instance was published, which is how a client knows an
    /// instance exists before it has done anything.
    pub fn record_instance(&self, node: &str, status: &InstanceStatus) -> u64 {
        self.record(
            Event::new(
                EventName::DaemonLifecycle,
                highland_observe::EventLevel::Info,
                node,
                Some(status.name.clone()),
                "instance_published",
                highland_observe::now_timestamp(),
            )
            .with_transition(status.role.clone(), status.role.clone())
            .with_field("effective_priority", status.effective_priority)
            .with_field("vips_owned", status.owns_addresses),
        )
    }

    /// Returns events after `since`, newest sequence last.
    #[must_use]
    pub fn since(&self, since: u64, limit: usize) -> Vec<(u64, Event)> {
        self.lock()
            .since(since, limit)
            .into_iter()
            .map(|(sequence, event)| (sequence, event.clone()))
            .collect()
    }

    /// Returns the newest sequence number, or zero when nothing is recorded.
    #[must_use]
    pub fn latest(&self) -> u64 {
        self.lock().latest()
    }

    /// Returns how many events a client resuming from `sequence` can no longer
    /// see, because the ring has overwritten them.
    ///
    /// The history is bounded (`L-08`), so a follower that is away long enough
    /// misses events. The ring knew how many it dropped and nothing ever read
    /// that number: `dropped` was called from tests only. A client could detect
    /// the gap by noticing a jump in sequence numbers, but nothing in the product
    /// did, and a missed event is indistinguishable from a quiet daemon.
    #[must_use]
    pub fn dropped_since(&self, sequence: u64) -> u64 {
        self.lock().dropped_through(sequence)
    }

    /// Returns the number of entries retained.
    #[must_use]
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    /// Returns `true` when nothing is recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, EventRing> {
        self.ring
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_follower_asks_for_what_it_has_not_seen() {
        let log = EventLog::new();
        log.record_transition("node-a", "api", "INIT", "BACKUP", "startup");
        log.record_transition("node-a", "api", "BACKUP", "MASTER", "master_down_timeout");

        let first = log.since(0, 10);
        assert_eq!(first.len(), 2);
        let cursor = first.last().expect("two events").0;

        assert!(
            log.since(cursor, 10).is_empty(),
            "nothing new since the cursor"
        );
        log.record_transition("node-a", "api", "MASTER", "BACKUP", "operator_relinquish");
        assert_eq!(
            log.since(cursor, 10).len(),
            1,
            "and one once something does happen"
        );
    }

    #[test]
    fn the_log_is_bounded_and_says_so() {
        let log = EventLog::new();
        for index in 0..10_000 {
            log.record_transition("node-a", "api", "A", "B", &format!("event {index}"));
        }
        assert_eq!(log.len(), 4096, "L-08: the history is bounded");
    }

    /// The regression test for an invisible gap: the ring has always known how
    /// many events it dropped, and nothing read the number.
    #[test]
    fn a_log_that_has_overwritten_events_reports_the_gap() {
        let log = EventLog::new();
        // Comfortably past the capacity. The bound is read from a log that has
        // already overflowed, so a change to the capacity cannot silently stop
        // this test from overflowing the ring.
        let probe = EventLog::new();
        for index in 0..10_000 {
            probe.record_transition("node-a", "api", "A", "B", &format!("f{index}"));
        }
        let capacity = probe.len();
        assert!(capacity > 0, "the ring retains something");

        for index in 0..(capacity + 500) {
            log.record_transition("node-a", "api", "A", "B", &format!("event {index}"));
        }
        assert_eq!(log.len(), capacity, "L-08: the history is bounded");
        assert_eq!(
            log.dropped_since(0),
            500,
            "a client from the start is told what it missed"
        );
        assert_eq!(
            log.dropped_since(log.latest()),
            0,
            "a client that is current has missed nothing"
        );
    }

    /// Before the ring is full there is nothing to report, whatever the cursor.
    #[test]
    fn a_log_that_has_dropped_nothing_reports_no_gap() {
        let log = EventLog::new();
        log.record_transition("node-a", "api", "A", "B", "startup");
        assert_eq!(log.dropped_since(0), 0);
        assert_eq!(log.dropped_since(999), 0);
    }

    #[test]
    fn an_empty_log_reports_nothing() {
        let log = EventLog::new();
        assert!(log.is_empty());
        assert_eq!(log.latest(), 0);
        assert_eq!(log.since(0, 10), Vec::new(), "nothing is retained yet");
    }

    #[test]
    fn a_published_instance_is_visible_before_it_does_anything() {
        let log = EventLog::new();
        let status = InstanceStatus {
            name: "api".to_owned(),
            role: "BACKUP".to_owned(),
            priority: 150,
            effective_priority: 150,
            owns_addresses: false,
            vip_addresses: vec!["192.0.2.100/24".to_owned()],
            peers: vec!["192.0.2.11".to_owned()],
            last_reason: "startup".to_owned(),
        };
        log.record_instance("node-a", &status);

        let events = log.since(0, 10);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].1.instance.as_deref(), Some("api"));
    }
}
