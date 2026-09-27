// Rust guideline compliant 2026-09-27

//! Rate limiting for control requests.
//!
//! The limiter is a token bucket driven by an explicit elapsed duration, so it
//! is testable without a clock and cannot itself be a denial-of-service
//! vector (`L-12`, `S-05`).

/// A token bucket that admits a bounded number of requests per interval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RateLimiter {
    capacity: u32,
    tokens: u32,
    elapsed_nanos: u64,
    nanos_per_token: u64,
}

impl RateLimiter {
    /// Creates a limiter admitting `per_second` requests per second.
    ///
    /// # Panics
    ///
    /// Panics when `per_second` is zero, because a limiter that admits nothing
    /// would make the control API unusable.
    pub fn per_second(per_second: u32) -> Self {
        assert!(
            per_second > 0,
            "a rate limit of zero makes the control API unusable"
        );
        Self {
            capacity: per_second,
            tokens: per_second,
            elapsed_nanos: 0,
            nanos_per_token: 1_000_000_000 / u64::from(per_second),
        }
    }

    /// Returns the maximum burst size.
    #[must_use]
    pub fn capacity(&self) -> u32 {
        self.capacity
    }

    /// Refills the bucket for `elapsed` and consumes one token.
    ///
    /// Returns `false` when no token is available, which the caller MUST turn
    /// into a refusal, which the server sends as a `rate_limited` response.
    pub fn admit(&mut self, elapsed: std::time::Duration) -> bool {
        self.elapsed_nanos = self
            .elapsed_nanos
            .saturating_add(u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX));
        let refilled = u32::try_from(self.elapsed_nanos / self.nanos_per_token).unwrap_or(u32::MAX);
        self.tokens = self.capacity.min(self.tokens.saturating_add(refilled));
        self.elapsed_nanos %= self.nanos_per_token;

        if self.tokens == 0 {
            return false;
        }
        self.tokens -= 1;
        true
    }
}

impl Default for RateLimiter {
    /// Returns the documented default of 20 requests per second (`L-12`).
    fn default() -> Self {
        Self::per_second(20)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn a_burst_up_to_the_capacity_is_admitted() {
        let mut limiter = RateLimiter::per_second(3);
        for _ in 0..3 {
            assert!(limiter.admit(Duration::ZERO));
        }
        assert!(!limiter.admit(Duration::ZERO));
    }

    #[test]
    fn tokens_refill_over_time() {
        let mut limiter = RateLimiter::per_second(2);
        assert!(limiter.admit(Duration::ZERO));
        assert!(limiter.admit(Duration::ZERO));
        assert!(!limiter.admit(Duration::ZERO));

        assert!(limiter.admit(Duration::from_millis(500)));
    }

    #[test]
    fn the_default_limit_is_twenty_per_second() {
        assert_eq!(RateLimiter::default().capacity(), 20);
    }

    #[test]
    #[should_panic(expected = "unusable")]
    fn a_zero_limit_is_rejected() {
        let _ = RateLimiter::per_second(0);
    }
}
