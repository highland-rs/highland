// Rust guideline compliant 2026-09-27

//! Running the checks and deciding when the verdict changed.
//!
//! The scheduler is the only place that knows about time: it runs each check on
//! its interval, bounds each probe by its timeout, applies the debounce
//! thresholds, and reduces the verdicts to the one number the state machine
//! consumes. Keeping that here means a check implementation cannot be wrong
//! about a threshold or a timeout, and a test can drive a whole instance's
//! health without a network.
//!
//! Three things it is careful about, each of which is an invariant rather than a
//! preference:
//!
//! - **A stale result is discarded** (`I-26`). A check from an older generation,
//!   or one that finished after a newer result, never overwrites it.
//! - **A timeout is a failure, not a separate state** (§15.3). A probe that hangs
//!   is a failing probe, and a check that hung is a failing check.
//! - **Nothing blocks the state machine** (`I-38`). `tick` is called by a task
//!   of its own and hands over a value; there is no await on the instance's task.

use std::sync::Arc;
use std::time::{Duration, Instant};

use highland_core::state::Generation;

use crate::check::{Check, Debouncer, Stability};
use crate::result::{CheckResult, HealthSummary};

/// One check's debounced state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The check is passing.
    Passing,
    /// The check is failing, and carries the weight it costs.
    Failing(u16),
    /// The check has not reached a conclusion yet.
    Unknown,
}

impl Verdict {
    /// Whether this verdict costs the instance priority.
    #[must_use]
    pub fn is_failing(self) -> bool {
        matches!(self, Verdict::Failing(_))
    }
}

/// What one round of probing produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthReport {
    /// The aggregated health, as the state machine consumes it.
    pub summary: HealthSummary,
    /// The checks whose verdict changed in this round, with the reason.
    pub changes: Vec<Change>,
}

/// One check changing its mind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// The check's name.
    pub check: String,
    /// What it was.
    pub from: Verdict,
    /// What it is now.
    pub to: Verdict,
    /// Why, in a sentence an operator can read.
    pub reason: String,
}

/// One check under the scheduler, with its debounce state and its last result.
///
/// Not `Debug`: it holds a trait object, and the scheduler's own `Debug` shows
/// the verdicts, which is what anyone reading a log actually wants.
struct Entry {
    check: Arc<dyn Check>,
    debouncer: Debouncer,
    /// The sequence to stamp on the next result.
    sequence: u64,
    /// The last accepted result, which a newer one must supersede.
    last: Option<CheckResult>,
    /// When the next probe is due.
    next_due: Instant,
    /// When the grace period ends, after which a failure counts.
    grace_until: Instant,
    verdict: Verdict,
    observed_at: Option<Instant>,
}

/// Runs an instance's checks and reduces them to a health summary.
pub struct Scheduler {
    entries: Vec<Entry>,
    /// The generation results are stamped with, which moves on a reload.
    generation: Generation,
    /// The instant this scheduler's clock reads zero.
    started: Instant,
}

impl std::fmt::Debug for Scheduler {
    /// Shows the generation and the verdicts, which is what a log needs; the
    /// check objects are opaque to anyone reading this, and a `Debug` that
    /// dumped them would be a wall of type names.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Scheduler")
            .field("generation", &self.generation)
            .field("verdicts", &self.verdicts())
            .field("checks", &self.entries.len())
            .field("started", &self.started)
            .finish()
    }
}

impl Scheduler {
    /// Creates a scheduler for `checks`, all in the current generation.
    #[must_use]
    pub fn new(checks: Vec<Arc<dyn Check>>) -> Self {
        Self::with_generation(checks, Generation::initial(), Instant::now())
    }

    /// Creates a scheduler with an explicit generation and clock, which is what
    /// a test drives.
    #[must_use]
    pub fn with_generation(
        checks: Vec<Arc<dyn Check>>,
        generation: Generation,
        now: Instant,
    ) -> Self {
        let entries = checks
            .into_iter()
            .map(|check| {
                let spec = check.spec();
                let grace_until = now + spec.initial_grace_period;
                Entry {
                    debouncer: Debouncer::new(spec),
                    next_due: now,
                    grace_until,
                    verdict: Verdict::Unknown,
                    sequence: 0,
                    last: None,
                    check,
                    observed_at: None,
                }
            })
            .collect();
        Self {
            entries,
            generation,
            started: now,
        }
    }

    /// The verdicts, one per check, in configuration order.
    #[must_use]
    pub fn verdicts(&self) -> Vec<(&str, Verdict)> {
        self.entries
            .iter()
            .map(|entry| (entry.check.spec().name.as_str(), entry.verdict))
            .collect()
    }

    /// The aggregated health as it stands, without probing.
    #[must_use]
    pub fn summary(&self) -> HealthSummary {
        summarise(&self.entries, self.generation)
    }

    /// The generation results are currently stamped with.
    #[must_use]
    pub fn generation(&self) -> Generation {
        self.generation
    }

    /// Moves the scheduler to a new configuration generation.
    ///
    /// The verdicts are kept: a reload that does not change a check must not make
    /// a healthy node look unknown, and the next result from the new generation
    /// is what supersedes the old one.
    pub fn set_generation(&mut self, generation: Generation, now: Instant) {
        self.generation = generation;
        for entry in &mut self.entries {
            entry.next_due = now;
            entry.grace_until = now + entry.check.spec().initial_grace_period;
        }
    }

    /// The instant at which the next check is due, or `None` when none is.
    #[must_use]
    pub fn next_due(&self) -> Option<Instant> {
        self.entries.iter().map(|entry| entry.next_due).min()
    }

    /// Probes everything that is due, and returns what changed.
    ///
    /// The probes are bounded by their own timeouts, so a hung service cannot
    /// hold the scheduler: the timeout is the caller's, and every probe here is
    /// wrapped in one.
    pub async fn tick(&mut self, now: Instant) -> HealthReport {
        let mut changes = Vec::new();
        for index in 0..self.entries.len() {
            if self.entries[index].next_due > now {
                continue;
            }
            let spec = self.entries[index].check.spec().clone();
            self.entries[index].next_due = now + spec.interval;

            let sequence = self.entries[index].sequence;
            let generation = self.generation;
            let result = probe(&*self.entries[index].check, generation, sequence).await;

            let entry = &mut self.entries[index];
            entry.sequence = entry.sequence.saturating_add(1);

            // `I-26`, in both directions: a result from an older generation, or
            // one that finished after a newer result, never overwrites it.
            if let Some(last) = &entry.last {
                if !result.supersedes(last) {
                    record_discarded(&mut changes, &spec.name, entry.verdict);
                    continue;
                }
            }
            entry.last = Some(result.clone());
            entry.observed_at = Some(now);

            // Inside the grace period a result is ignored entirely (§15.3): the
            // service has not had time to come up, and a node that demotes itself
            // for its own startup is a node that never joins the election. The
            // result is still recorded, so the sequence advances and the next one
            // supersedes it.
            if now < entry.grace_until {
                continue;
            }

            let status = result.status;
            let from = entry.verdict;
            match entry.debouncer.observe(status) {
                Some(stability) => {
                    let to = match stability {
                        Stability::Passing => Verdict::Passing,
                        Stability::Failing => Verdict::Failing(spec.weight),
                        // The debouncer never reports a move to unknown, and a
                        // state added later lands here as a non-failing verdict
                        // rather than as a missing match arm.
                        Stability::Unknown => entry.verdict,
                    };
                    entry.verdict = to;
                    changes.push(Change {
                        check: spec.name.clone(),
                        from,
                        to,
                        reason: result.reason.clone(),
                    });
                }
                None => {
                    // A single failed probe is still an event: the specification
                    // distinguishes a failed probe from a check that changed state.
                    if result.status.is_failure() {
                        changes.push(Change {
                            check: spec.name.clone(),
                            from: entry.verdict,
                            to: entry.verdict,
                            reason: result.reason.clone(),
                        });
                    }
                }
            }
        }

        HealthReport {
            summary: summarise(&self.entries, self.generation),
            changes,
        }
    }

    /// How long this scheduler has existed on its own clock.
    #[must_use]
    pub fn uptime(&self, now: Instant) -> Duration {
        now.saturating_duration_since(self.started)
    }
}

/// Records a discarded result, which is an event rather than a state change.
fn record_discarded(changes: &mut Vec<Change>, check: &str, verdict: Verdict) {
    changes.push(Change {
        check: check.to_owned(),
        from: verdict,
        to: verdict,
        reason: "a stale result was discarded".to_owned(),
    });
}

/// Runs one probe under its timeout, and turns a timeout into a failure.
///
/// A hung service is the case that matters: without this, a check that never
/// answers would leave the instance believing the last verdict forever, and the
/// failure would be silent rather than late.
async fn probe(check: &dyn Check, generation: Generation, sequence: u64) -> CheckResult {
    let name = check.spec().name.clone();
    let timeout = check.spec().timeout;
    match tokio::time::timeout(timeout, check.run(generation, sequence)).await {
        Ok(Ok(result)) => result,
        Ok(Err(error)) => CheckResult::failing(
            name,
            None,
            Instant::now(),
            generation,
            sequence,
            format!("the probe reported an error: {error}"),
        )
        .unwrap_or_else(|_| unreachable!("a reason was supplied")),
        Err(_) => {
            // The sequence is the one the timed-out probe was given, advanced so
            // the next result supersedes it rather than being discarded.
            CheckResult::timed_out(name, Instant::now(), generation, sequence + 1, timeout)
                .unwrap_or_else(|_| unreachable!("a reason was supplied"))
        }
    }
}

/// Reduces the verdicts to the aggregate the state machine consumes.
fn summarise(entries: &[Entry], generation: Generation) -> HealthSummary {
    let mut summary = HealthSummary::default();
    for entry in entries {
        match entry.verdict {
            Verdict::Failing(weight) => {
                summary.penalty = summary.penalty.saturating_add(weight);
                summary.failing = summary.failing.saturating_add(1);
            }
            Verdict::Passing => summary.passing = summary.passing.saturating_add(1),
            Verdict::Unknown => {}
        }
        if let Some(last) = &entry.last
            && last.is_stale_for(generation)
        {
            summary.stale_discarded = summary.stale_discarded.saturating_add(1);
        }
    }
    summary
}
