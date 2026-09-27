// Rust guideline compliant 2026-09-27

//! The shutdown sequence.
//!
//! The order is fixed by `SPEC.md` §14.5: stop accepting control requests,
//! stop health checks, relinquish ownership, dump state, exit. Modeling it as
//! data keeps the sequence testable and keeps it identical on `SIGTERM`,
//! `SIGHUP`-after-shutdown, and a second signal (`I-31`).

use std::time::Duration;

/// The default shutdown budget.
pub const DEFAULT_SHUTDOWN_BUDGET: Duration = Duration::from_secs(5);

/// Why the daemon is shutting down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ShutdownReason {
    /// `SIGTERM` or `SIGINT`.
    Signal,
    /// The runtime failed.
    Fatal,
}

impl ShutdownReason {
    /// Returns the stable reason string used in events.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            ShutdownReason::Signal => "signal",
            ShutdownReason::Fatal => "fatal_error",
        }
    }
}

/// One step of the shutdown sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownStep {
    /// Refuse new control requests.
    StopAcceptingControlRequests,
    /// Stop health checks.
    StopHealthChecks,
    /// Relinquish one instance's VIPs.
    RelinquishInstance,
    /// Emit a final state dump.
    DumpState,
    /// Exit.
    Exit,
}

/// The shutdown sequence, as data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShutdownPlan {
    /// Why the daemon is shutting down.
    pub reason: ShutdownReason,
    /// The time the sequence may take.
    pub budget: Duration,
    /// The instances that must relinquish ownership, in configuration order.
    pub masters: Vec<String>,
}

impl ShutdownPlan {
    /// Creates a plan for `reason` owning `masters`.
    #[must_use]
    pub fn new(reason: ShutdownReason, masters: Vec<String>) -> Self {
        Self {
            reason,
            budget: DEFAULT_SHUTDOWN_BUDGET,
            masters,
        }
    }

    /// Returns the steps in the order they must be performed.
    #[must_use]
    pub fn steps(&self) -> Vec<ShutdownStep> {
        let mut steps = vec![
            ShutdownStep::StopAcceptingControlRequests,
            ShutdownStep::StopHealthChecks,
        ];
        steps.extend(std::iter::repeat_n(
            ShutdownStep::RelinquishInstance,
            self.masters.len(),
        ));
        steps.push(ShutdownStep::DumpState);
        steps.push(ShutdownStep::Exit);
        steps
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sequence_drops_control_before_ownership() {
        let plan = ShutdownPlan::new(ShutdownReason::Signal, vec!["api".to_owned()]);
        assert_eq!(
            plan.steps(),
            [
                ShutdownStep::StopAcceptingControlRequests,
                ShutdownStep::StopHealthChecks,
                ShutdownStep::RelinquishInstance,
                ShutdownStep::DumpState,
                ShutdownStep::Exit,
            ]
        );
    }

    #[test]
    fn a_plan_with_no_masters_still_dumps_and_exits() {
        let plan = ShutdownPlan::new(ShutdownReason::Signal, Vec::new());
        assert!(!plan.steps().contains(&ShutdownStep::RelinquishInstance));
        assert_eq!(plan.steps().last(), Some(&ShutdownStep::Exit));
    }

    #[test]
    fn the_default_budget_is_five_seconds() {
        assert_eq!(DEFAULT_SHUTDOWN_BUDGET, Duration::from_secs(5));
    }
}
