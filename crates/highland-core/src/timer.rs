// Rust guideline compliant 2026-09-27

//! Timers owned by an instance.
//!
//! Every timer has exactly one owner, one arming point, and one cancellation
//! path (`SPEC.md` §13). Deadlines are absolute, in [`Clock`](crate::clock::Clock)
//! units, so that a test asserts a deadline instead of sleeping, and so that the
//! machine's output does not depend on when the executor ran (`R-27`).

use std::collections::BTreeMap;
use std::time::Duration;

use crate::state::TimerId;

/// The set of timers owned by one instance.
///
/// A timer is either armed with a deadline or absent. Re-arming replaces the
/// deadline, which is what makes "the master-down timer is reset by every
/// advertisement" a single line of logic.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TimerSet {
    deadlines: BTreeMap<TimerId, Duration>,
}

impl TimerSet {
    /// Creates an empty set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Arms `timer` at `deadline`, replacing any existing deadline.
    pub fn arm(&mut self, timer: TimerId, deadline: Duration) {
        self.deadlines.insert(timer, deadline);
    }

    /// Cancels `timer`. Cancellation is idempotent (`I-29`).
    pub fn cancel(&mut self, timer: TimerId) {
        self.deadlines.remove(&timer);
    }

    /// Cancels every timer. Used when an instance leaves participation.
    pub fn cancel_all(&mut self) {
        self.deadlines.clear();
    }

    /// Returns the deadline of `timer`, when it is armed.
    #[must_use]
    pub fn deadline(&self, timer: TimerId) -> Option<Duration> {
        self.deadlines.get(&timer).copied()
    }

    /// Returns `true` when `timer` is armed.
    #[must_use]
    pub fn is_armed(&self, timer: TimerId) -> bool {
        self.deadlines.contains_key(&timer)
    }

    /// Returns `true` when `timer` is armed and its deadline has passed.
    #[must_use]
    pub fn is_due(&self, timer: TimerId, now: Duration) -> bool {
        self.deadline(timer).is_some_and(|deadline| now >= deadline)
    }

    /// Returns the time left on `timer`, or `None` when it is not armed.
    ///
    /// A timer that has already expired reports zero rather than a negative
    /// duration, so a caller can render it directly.
    #[must_use]
    pub fn remaining(&self, timer: TimerId, now: Duration) -> Option<Duration> {
        self.deadline(timer)
            .map(|deadline| deadline.saturating_sub(now))
    }

    /// Returns the time left on `timer` in milliseconds, for the status API.
    #[must_use]
    pub fn remaining_millis(&self, timer: TimerId, now: Duration) -> Option<u64> {
        self.remaining(timer, now)
            .and_then(|remaining| u64::try_from(remaining.as_millis()).ok())
    }

    /// Returns the timers whose deadline has passed, in a stable order.
    #[must_use]
    pub fn due(&self, now: Duration) -> Vec<TimerId> {
        self.deadlines
            .iter()
            .filter(|(_, deadline)| now >= **deadline)
            .map(|(timer, _)| *timer)
            .collect()
    }

    /// Returns the armed timers, in a stable order.
    #[must_use]
    pub fn armed(&self) -> Vec<TimerId> {
        self.deadlines.keys().copied().collect()
    }

    /// Returns the number of armed timers.
    #[must_use]
    pub fn len(&self) -> usize {
        self.deadlines.len()
    }

    /// Returns `true` when nothing is armed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.deadlines.is_empty()
    }
}

/// Bounded exponential backoff for a failed action.
///
/// The bounds are fixed by `R-12`: base 1s, cap 30s, at most 10 attempts. After
/// the cap the instance stays in `FAULT` and stops retrying until an operator or
/// a new generation intervenes, which is what stops a permanently broken
/// interface from becoming a retry loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// The delay before the first retry.
    pub base: Duration,
    /// The longest delay between retries.
    pub cap: Duration,
    /// The number of attempts after the initial failure.
    pub max_attempts: u32,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            base: Duration::from_secs(1),
            cap: Duration::from_secs(30),
            max_attempts: 10,
        }
    }
}

impl RetryPolicy {
    /// Returns the delay before attempt number `attempt`, counting from one.
    ///
    /// Returns `None` once `max_attempts` is exceeded, which is the signal to
    /// stop retrying.
    #[must_use]
    pub fn delay_for(&self, attempt: u32) -> Option<Duration> {
        if attempt == 0 || attempt > self.max_attempts {
            return None;
        }
        let shift = attempt.saturating_sub(1).min(16);
        let multiplier = 1_u32 << shift;
        self.base
            .checked_mul(multiplier)
            .map(|delay| delay.min(self.cap))
            .or(Some(self.cap))
    }

    /// Returns `true` when another attempt is permitted.
    #[must_use]
    pub fn permits(&self, attempt: u32) -> bool {
        attempt <= self.max_attempts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arming_records_a_deadline_and_cancelling_removes_it() {
        let mut timers = TimerSet::new();
        assert!(timers.is_empty());

        timers.arm(TimerId::MasterDown, Duration::from_secs(3));
        assert!(timers.is_armed(TimerId::MasterDown));
        assert_eq!(
            timers.deadline(TimerId::MasterDown),
            Some(Duration::from_secs(3))
        );

        timers.cancel(TimerId::MasterDown);
        assert!(!timers.is_armed(TimerId::MasterDown));
        assert_eq!(timers.remaining(TimerId::MasterDown, Duration::ZERO), None);
    }

    #[test]
    fn cancellation_is_idempotent() {
        let mut timers = TimerSet::new();
        timers.cancel(TimerId::Advertisement);
        timers.cancel(TimerId::Advertisement);
        assert!(timers.is_empty());
    }

    #[test]
    fn re_arming_replaces_the_deadline() {
        let mut timers = TimerSet::new();
        timers.arm(TimerId::MasterDown, Duration::from_secs(3));
        timers.arm(TimerId::MasterDown, Duration::from_secs(6));
        assert_eq!(
            timers.deadline(TimerId::MasterDown),
            Some(Duration::from_secs(6))
        );
        assert_eq!(timers.len(), 1);
    }

    #[test]
    fn a_timer_is_due_only_at_or_after_its_deadline() {
        let mut timers = TimerSet::new();
        timers.arm(TimerId::MasterDown, Duration::from_millis(3410));

        assert!(!timers.is_due(TimerId::MasterDown, Duration::from_millis(3409)));
        assert!(timers.is_due(TimerId::MasterDown, Duration::from_millis(3410)));
        assert!(timers.is_due(TimerId::MasterDown, Duration::from_millis(3411)));
    }

    #[test]
    fn an_expired_timer_reports_zero_remaining() {
        let mut timers = TimerSet::new();
        timers.arm(TimerId::Advertisement, Duration::from_secs(1));
        assert_eq!(
            timers.remaining(TimerId::Advertisement, Duration::from_secs(5)),
            Some(Duration::ZERO)
        );
    }

    #[test]
    fn remaining_millis_is_none_when_the_timer_is_not_armed() {
        let timers = TimerSet::new();
        assert_eq!(
            timers.remaining_millis(TimerId::PreemptionDelay, Duration::ZERO),
            None
        );
    }

    #[test]
    fn due_timers_are_reported_in_a_stable_order() {
        let mut timers = TimerSet::new();
        timers.arm(TimerId::Retry, Duration::from_secs(1));
        timers.arm(TimerId::MasterDown, Duration::from_secs(1));
        timers.arm(TimerId::HoldDown, Duration::from_secs(5));

        assert_eq!(
            timers.due(Duration::from_secs(2)),
            [TimerId::MasterDown, TimerId::Retry]
        );
    }

    #[test]
    fn cancel_all_empties_the_set() {
        let mut timers = TimerSet::new();
        timers.arm(TimerId::MasterDown, Duration::from_secs(1));
        timers.arm(TimerId::HoldDown, Duration::from_secs(2));
        timers.cancel_all();
        assert!(timers.is_empty());
        assert!(timers.due(Duration::from_secs(100)).is_empty());
    }

    #[test]
    fn backoff_doubles_from_the_base_and_saturates_at_the_cap() {
        let policy = RetryPolicy::default();
        assert_eq!(policy.delay_for(1), Some(Duration::from_secs(1)));
        assert_eq!(policy.delay_for(2), Some(Duration::from_secs(2)));
        assert_eq!(policy.delay_for(3), Some(Duration::from_secs(4)));
        assert_eq!(policy.delay_for(5), Some(Duration::from_secs(16)));
        assert_eq!(policy.delay_for(6), Some(Duration::from_secs(30)));
        assert_eq!(policy.delay_for(10), Some(Duration::from_secs(30)));
    }

    #[test]
    fn backoff_stops_after_the_documented_attempt_count() {
        let policy = RetryPolicy::default();
        assert!(policy.permits(10));
        assert!(!policy.permits(11));
        assert_eq!(policy.delay_for(0), None);
        assert_eq!(policy.delay_for(11), None);
    }
}
