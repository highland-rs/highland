// Rust guideline compliant 2026-09-27

// On a platform with no instances there is nothing to run per instance, and the
// only caller of this module is the Linux-only per-instance path.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

//! The per-instance health task.
//!
//! One task per instance, beside the actor rather than inside it. That placement
//! is the requirement `I-38` and `R-05` make: a probe waits on a socket, and a
//! state machine that waited on a socket would stop deciding anything while a
//! service was slow. The task owns the scheduler, the scheduler owns the
//! timeouts, and the only thing that crosses to the instance is a summary.
//!
//! The summary is sent when it changes rather than on every tick, because a
//! health event per interval would be an event per interval in the log for a
//! check that is simply passing.

use std::sync::Arc;
use std::time::{Duration, Instant};

use highland_checks::{Check, Scheduler};

use crate::driver::{Instruction, InstructionSender};
use crate::metrics::Metrics;

/// How long the task waits when nothing is due.
///
/// Short enough that a check started by a reload is not visibly late, long
/// enough that an idle instance is not waking fifty times a second.
const IDLE: Duration = Duration::from_millis(200);

/// Runs one instance's checks until `shutdown` resolves.
///
/// # Errors
///
/// Returns a message when a check cannot be built from its plan, which is a
/// configuration error and stops the instance rather than starting a scheduler
/// that can only fail.
/// Runs the probes for one instance.
///
/// The probes arrive already built. The runner builds them *before* the instance
/// is created, and refuses the instance if one cannot be built, so by the time
/// this runs every probe is known to work. It used to be built here, and a
/// failure was logged while the instance carried on without the check.
pub(crate) async fn run(
    instance: &str,
    checks: Vec<Arc<dyn Check>>,
    sender: InstructionSender,
    metrics: Arc<Metrics>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) -> Result<(), String> {
    if checks.is_empty() {
        // Nothing to run. Waiting on the shutdown signal alone is the honest
        // thing: a task that woke to do nothing would be a wakeup per instance
        // per interval forever.
        let _ = shutdown.changed().await;
        return Ok(());
    }

    let mut scheduler = Scheduler::new(checks);
    let mut reported = scheduler.summary();

    loop {
        if *shutdown.borrow() {
            return Ok(());
        }

        let now = Instant::now();
        let wait = match scheduler.next_due() {
            Some(due) => due.saturating_duration_since(now),
            // No check is due, which means there are none: an instance with no
            // checks waits on the signal rather than waking to find nothing.
            None => IDLE,
        }
        .max(Duration::from_millis(1));

        tokio::select! {
            biased;
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    return Ok(());
                }
            }
            () = tokio::time::sleep(wait) => {
                let report = scheduler.tick(Instant::now()).await;
                for change in &report.changes {
                    let result = scheduler.last_result(&change.check);
                    metrics.record_check(
                        instance,
                        &change.check,
                        change.to.is_failing(),
                        result.and_then(|result| result.latency),
                    );
                    tracing::debug!(
                        instance,
                        check = %change.check,
                        from = ?change.from,
                        to = ?change.to,
                        "a check reported a change: {}",
                        change.reason
                    );
                }
                if report.summary != reported {
                    reported = report.summary;
                    // The summary is the whole interface: the state machine does
                    // the policy arithmetic, and this task does not interpret it.
                    if sender
                        .send(Instruction::Health(reported))
                        .await
                        .is_err()
                    {
                        // The instance is gone, so the task has nothing left to
                        // report to.
                        return Ok(());
                    }
                }
            }
        }
    }
}
