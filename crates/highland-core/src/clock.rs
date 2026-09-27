// Rust guideline compliant 2026-09-27

//! Clock and randomness abstractions.
//!
//! Every duration in the state machine is derived from a [`Clock`], never from
//! a wall clock, so that tests can assert exact timer fire times
//! (SPEC.md, `R-27`). [`SystemClock`] is the only implementation that touches
//! the operating system; [`ManualClock`] is the fake used by tests.
//!
//! Timers are modeled as absolute deadlines computed by the state machine.
//! Advancing a [`ManualClock`] does not itself fire timers; a test or executor
//! decides which [`TimerId`](crate::state::TimerId) has expired and feeds the
//! corresponding event back into the machine. That keeps firing decisions in
//! one place.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use thiserror::Error;

/// A monotonic time source.
///
/// Implementations MUST be monotonic: [`Clock::now`] MUST never decrease,
/// including across a wall-clock adjustment (SPEC.md, §29.7).
///
/// # Examples
///
/// ```
/// use highland_core::clock::{Clock, ManualClock};
/// use std::time::Duration;
///
/// let clock = ManualClock::new();
/// assert_eq!(clock.now(), Duration::ZERO);
///
/// clock.advance(Duration::from_millis(250));
/// assert_eq!(clock.now(), Duration::from_millis(250));
/// assert_eq!(clock.remaining_until(Duration::from_millis(200)), Duration::ZERO);
/// ```
pub trait Clock: Send + Sync + std::fmt::Debug {
    /// Returns the time elapsed since the clock's origin.
    ///
    /// # Returns
    ///
    /// The elapsed monotonic duration. The value at construction is
    /// implementation-defined and MUST be treated as an opaque origin.
    fn now(&self) -> Duration;

    /// Returns the time remaining until `deadline`, or zero if it has passed.
    fn remaining_until(&self, deadline: Duration) -> Duration {
        deadline.saturating_sub(self.now())
    }
}

/// A monotonic [`Clock`] backed by the operating system.
#[derive(Debug, Clone, Copy)]
pub struct SystemClock {
    origin: std::time::Instant,
}

impl SystemClock {
    /// Creates a clock whose origin is the current instant.
    #[must_use]
    pub fn new() -> Self {
        Self {
            origin: std::time::Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn now(&self) -> Duration {
        self.origin.elapsed()
    }
}

/// A deterministic [`Clock`] advanced explicitly by tests.
///
/// # Examples
///
/// ```
/// use highland_core::clock::{Clock, ManualClock};
/// use std::time::Duration;
///
/// let clock = ManualClock::new();
/// clock.advance(Duration::from_secs(3));
/// assert_eq!(clock.now(), Duration::from_secs(3));
/// ```
#[derive(Debug, Clone, Default)]
pub struct ManualClock {
    elapsed_nanos: Arc<AtomicU64>,
}

impl ManualClock {
    /// Creates a clock positioned at the origin.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Advances the clock by `by` and returns the new position.
    ///
    /// # Panics
    ///
    /// Panics if `by` is zero, or if it cannot be represented. Advancing by
    /// nothing is a mistake in the caller rather than a runtime condition.
    pub fn advance(&self, by: Duration) -> Duration {
        assert!(
            !by.is_zero(),
            "ManualClock::advance requires a non-zero duration"
        );
        let current = self.now();
        let target = current
            .checked_add(by)
            .expect("ManualClock::advance overflowed the representable duration range");
        self.set(target);
        target
    }

    /// Moves the clock to an absolute position.
    ///
    /// # Panics
    ///
    /// Panics if `to` is earlier than the current position, because a
    /// [`Clock`] MUST be monotonic.
    pub fn advance_to(&self, to: Duration) {
        let current = self.now();
        assert!(
            to >= current,
            "ManualClock::advance_to must not move backwards"
        );
        self.set(to);
    }

    fn set(&self, to: Duration) {
        let Ok(nanos) = u64::try_from(to.as_nanos()) else {
            panic!("ManualClock cannot represent {to:?} as nanoseconds");
        };
        self.elapsed_nanos.store(nanos, Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Duration {
        Duration::from_nanos(self.elapsed_nanos.load(Ordering::SeqCst))
    }
}

/// A source of non-deterministic values, expressed as a trait so that tests
/// can substitute a deterministic sequence.
pub trait Rng: Send + Sync + std::fmt::Debug {
    /// Returns the next value in the inclusive range `low..=high`.
    ///
    /// # Errors
    ///
    /// Returns [`RngError::EmptyRange`] when `low > high`.
    fn next_inclusive(&self, low: u64, high: u64) -> Result<u64, RngError>;
}

/// Errors raised by [`Rng`] implementations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum RngError {
    /// The requested range was empty.
    #[error("random range {low}..={high} is empty")]
    EmptyRange {
        /// The inclusive lower bound.
        low: u64,
        /// The inclusive upper bound.
        high: u64,
    },
}

/// A deterministic [`Rng`] that replays a fixed sequence and then repeats its
/// last value, intended for tests and for the simulation harness.
#[derive(Debug, Clone, Default)]
pub struct SequenceRng {
    values: Vec<u64>,
    cursor: Arc<AtomicU64>,
}

impl SequenceRng {
    /// Creates a generator that replays `values` in order.
    #[must_use]
    pub fn new(values: Vec<u64>) -> Self {
        Self {
            values,
            cursor: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Returns the next value in the sequence, repeating the last value once
    /// the sequence is exhausted.
    fn next_value(&self) -> Option<u64> {
        if self.values.is_empty() {
            return None;
        }
        let index =
            usize::try_from(self.cursor.fetch_add(1, Ordering::SeqCst)).unwrap_or(usize::MAX);
        Some(self.values[index.min(self.values.len() - 1)])
    }
}

impl Rng for SequenceRng {
    fn next_inclusive(&self, low: u64, high: u64) -> Result<u64, RngError> {
        if low > high {
            return Err(RngError::EmptyRange { low, high });
        }
        let Some(next) = self.next_value() else {
            return Ok(low);
        };
        let span = high - low + 1;
        Ok(if span == 0 { next } else { low + next % span })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_system_clock_is_monotonic() {
        let clock = SystemClock::new();
        let first = clock.now();
        let second = clock.now();
        assert!(second >= first);
    }

    #[test]
    fn the_manual_clock_never_moves_backwards() {
        let clock = ManualClock::new();
        assert_eq!(
            clock.advance(Duration::from_secs(1)),
            Duration::from_secs(1)
        );
        clock.advance_to(Duration::from_secs(2));
        assert_eq!(clock.now(), Duration::from_secs(2));
        assert_eq!(
            clock.remaining_until(Duration::from_millis(500)),
            Duration::ZERO
        );
        assert_eq!(
            clock.remaining_until(Duration::from_secs(5)),
            Duration::from_secs(3)
        );
    }

    #[test]
    #[should_panic(expected = "must not move backwards")]
    fn the_manual_clock_rejects_backwards_advances() {
        let clock = ManualClock::new();
        clock.advance(Duration::from_secs(1));
        clock.advance_to(Duration::ZERO);
    }

    #[test]
    #[should_panic(expected = "non-zero duration")]
    fn the_manual_clock_rejects_zero_advances() {
        let clock = ManualClock::new();
        clock.advance(Duration::ZERO);
    }

    #[test]
    fn the_sequence_generator_replays_and_repeats_its_last_value() {
        let rng = SequenceRng::new(vec![0, 1, 2]);
        assert_eq!(rng.next_inclusive(0, 10).unwrap(), 0);
        assert_eq!(rng.next_inclusive(0, 10).unwrap(), 1);
        assert_eq!(rng.next_inclusive(0, 10).unwrap(), 2);
        assert_eq!(rng.next_inclusive(0, 10).unwrap(), 2);
    }

    #[test]
    fn an_empty_random_range_is_rejected() {
        let rng = SequenceRng::new(vec![1]);
        assert_eq!(
            rng.next_inclusive(5, 4),
            Err(RngError::EmptyRange { low: 5, high: 4 })
        );
    }
}
