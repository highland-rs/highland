// Rust guideline compliant 2026-09-27

//! The scheduler: thresholds, timeouts, staleness, and the summary.
//!
//! The probes answer questions; the scheduler decides what they mean. It is where
//! the timeouts live, where the debounce thresholds are applied, and where a
//! stale result is thrown away — three invariants that are invisible in a probe
//! and load-bearing in a node.
//!
//! A scripted check stands in for a network. That is the point: the interesting
//! failures of a scheduler are sequences ("three failures, then a success"), and a
//! sequence is easier to state exactly with a script than with sockets.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use highland_checks::{Check, CheckKind, CheckResult, CheckSpec, CheckStatus, Scheduler, Verdict};
use highland_core::state::Generation;

/// A check whose answers are decided by the test, in order.
#[derive(Debug)]
struct Scripted {
    spec: CheckSpec,
    answers: Mutex<Vec<CheckStatus>>,
    /// How long the probe pretends to take, so the timeout can be exercised.
    delay: Duration,
    /// The results this check was asked for, in order.
    asked: Mutex<Vec<u64>>,
}

impl Scripted {
    fn new(spec: CheckSpec, answers: Vec<CheckStatus>) -> Arc<Self> {
        Arc::new(Self {
            spec,
            answers: Mutex::new(answers),
            delay: Duration::ZERO,
            asked: Mutex::new(Vec::new()),
        })
    }

    fn slow(spec: CheckSpec, answers: Vec<CheckStatus>, delay: Duration) -> Arc<Self> {
        let check = Self::new(spec, answers);
        // `Arc::get_mut` on a value nothing else holds yet, which is true for a
        // check a test has just built.
        let mut check = Arc::into_inner(check).expect("the check has one owner");
        check.delay = delay;
        Arc::new(check)
    }

    fn next(&self) -> CheckStatus {
        let mut answers = self.answers.lock().expect("the script is not poisoned");
        if answers.is_empty() {
            return CheckStatus::Passing;
        }
        answers.remove(0)
    }
}

impl Check for Scripted {
    fn spec(&self) -> &CheckSpec {
        &self.spec
    }

    fn run(
        &self,
        generation: Generation,
        sequence: u64,
    ) -> Pin<Box<dyn Future<Output = highland_checks::Result<CheckResult>> + Send + '_>> {
        self.asked
            .lock()
            .expect("the log is not poisoned")
            .push(sequence);
        let status = self.next();
        let delay = self.delay;
        let spec = self.spec.clone();
        Box::pin(async move {
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            let reason = match status {
                CheckStatus::Passing => "the probe passed",
                CheckStatus::TimedOut => "the probe did not answer",
                _ => "the probe failed",
            };
            CheckResult::new(
                spec.name,
                status,
                None,
                Instant::now(),
                generation,
                sequence,
                reason,
            )
        })
    }
}

/// A specification with a name, a weight, and both thresholds.
fn spec(name: &str, weight: u16, failures: u32, successes: u32) -> CheckSpec {
    let mut spec = CheckSpec::new(
        name,
        CheckKind::Tcp,
        Duration::from_millis(10),
        Duration::from_millis(200),
        failures,
        successes,
        weight,
    )
    .expect("the fixture is valid");
    spec.initial_grace_period = Duration::ZERO;
    spec
}

#[tokio::test]
async fn one_failure_does_not_demote_and_the_threshold_does() {
    let check = Scripted::new(
        spec("api", 100, 3, 2),
        vec![
            CheckStatus::Failing,
            CheckStatus::Failing,
            CheckStatus::Passing,
            CheckStatus::Failing,
        ],
    );
    let mut scheduler = Scheduler::new(vec![check]);
    let now = Instant::now();

    // A single failed probe is not a failing check: the thresholds exist so a
    // blip does not move a priority on a live segment.
    let report = scheduler.tick(now).await;
    assert_eq!(report.summary.penalty, 0, "one failure of three");
    assert!(
        !report.changes.is_empty(),
        "the failed probe is still reported as an event: {:?}",
        report.changes
    );

    let report = scheduler.tick(now + Duration::from_millis(10)).await;
    assert_eq!(report.summary.penalty, 0, "two failures of three");
    assert_eq!(report.summary.passing, 0);
    assert_eq!(report.summary.failing, 0);

    // A success resets the failure count, and a check that starts unknown needs
    // `success_threshold` successes to be declared passing: one success of two
    // is not a verdict.
    let report = scheduler.tick(now + Duration::from_millis(20)).await;
    assert_eq!(
        report.summary.passing, 0,
        "one success of two is not a verdict"
    );

    // Now three consecutive failures demote it, and the change says why.
    let check = Scripted::new(
        spec("api", 100, 3, 2),
        vec![
            CheckStatus::Failing,
            CheckStatus::Failing,
            CheckStatus::Failing,
            CheckStatus::Failing,
        ],
    );
    let mut scheduler = Scheduler::new(vec![check]);
    for step in 0..3 {
        scheduler.tick(now + Duration::from_millis(10 * step)).await;
    }
    let report = scheduler.tick(now + Duration::from_millis(30)).await;
    assert_eq!(report.summary.penalty, 100, "the weight is the cost");
    assert_eq!(report.summary.failing, 1);
    assert_eq!(scheduler.verdicts(), vec![("api", Verdict::Failing(100))]);

    let change = report
        .changes
        .iter()
        .find(|change| change.to.is_failing())
        .expect("the demotion is a change, not just a number");
    assert_eq!(
        change.from,
        Verdict::Unknown,
        "a check that never passed has nothing to step down from"
    );
    assert!(
        change.reason.contains("failed"),
        "a demotion is explained: {}",
        change.reason
    );
}

#[tokio::test]
async fn recovery_requires_consecutive_successes() {
    let check = Scripted::new(
        spec("api", 100, 1, 2),
        vec![
            CheckStatus::Failing,
            CheckStatus::Passing,
            CheckStatus::Failing,
            CheckStatus::Passing,
            CheckStatus::Passing,
            CheckStatus::Passing,
        ],
    );
    let mut scheduler = Scheduler::new(vec![check]);
    let now = Instant::now();

    let report = scheduler.tick(now).await;
    assert_eq!(report.summary.failing, 1, "one failure of one is a failure");

    // A single success does not recover it: the success threshold is what stops
    // a flapping service from taking the address back and forth.
    let report = scheduler.tick(now + Duration::from_millis(10)).await;
    assert_eq!(report.summary.failing, 1, "one success of three");
    let report = scheduler.tick(now + Duration::from_millis(20)).await;
    assert_eq!(
        report.summary.failing, 1,
        "the success count was reset by a failure"
    );
    let report = scheduler.tick(now + Duration::from_millis(30)).await;
    assert_eq!(report.summary.failing, 1, "two of three");
    let report = scheduler.tick(now + Duration::from_millis(40)).await;
    assert_eq!(
        report.summary.passing, 1,
        "three consecutive successes recover it"
    );
    assert_eq!(report.summary.penalty, 0, "and the penalty is given back");
}

#[tokio::test]
async fn a_probe_that_overruns_is_a_failure_and_does_not_stall_the_scheduler() {
    // A probe that takes longer than its timeout, which is the hung service.
    let check = Scripted::slow(
        spec("hung", 100, 1, 1),
        vec![CheckStatus::Passing, CheckStatus::Passing],
        Duration::from_millis(400),
    );
    let mut scheduler = Scheduler::new(vec![check]);
    let now = Instant::now();

    let started = Instant::now();
    let report = scheduler.tick(now).await;
    let elapsed = started.elapsed();

    assert!(
        elapsed < Duration::from_millis(300),
        "the tick returned at {elapsed:?}, so the timeout bounds the probe"
    );
    assert_eq!(
        report.summary.failing, 1,
        "a timeout is a failure, not a separate state (§15.3)"
    );
}

#[tokio::test]
async fn a_result_from_a_previous_generation_never_overwrites_a_newer_one() {
    let check = Scripted::new(
        spec("api", 100, 1, 1),
        vec![
            CheckStatus::Passing,
            CheckStatus::Passing,
            CheckStatus::Failing,
            CheckStatus::Failing,
        ],
    );
    let mut scheduler = Scheduler::new(vec![check]);
    let now = Instant::now();

    scheduler.tick(now).await;
    scheduler.tick(now + Duration::from_millis(10)).await;
    assert_eq!(scheduler.summary().passing, 1, "the check passes");
    assert_eq!(scheduler.summary().stale_discarded, 0);

    // A reload moves the generation. The verdicts survive it — a reload that did
    // not change a check must not make a healthy node look unknown.
    let next = Generation::initial().next();
    scheduler.set_generation(next, now + Duration::from_millis(20));
    assert_eq!(scheduler.generation(), next);
    assert_eq!(
        scheduler.summary().passing,
        1,
        "the verdict survives a reload"
    );

    // The next result is stamped with the new generation and is accepted.
    let report = scheduler.tick(now + Duration::from_millis(20)).await;
    assert_eq!(
        report.summary.failing, 1,
        "the new generation's result is used"
    );
    assert_eq!(
        report.summary.stale_discarded, 0,
        "and the old one was superseded, not counted as stale"
    );
}

#[tokio::test]
async fn results_are_produced_in_sequence_and_asked_for_once_each() {
    let check = Scripted::new(
        spec("api", 100, 1, 1),
        vec![
            CheckStatus::Passing,
            CheckStatus::Passing,
            CheckStatus::Failing,
        ],
    );
    let asked = Arc::clone(&check);
    let mut scheduler = Scheduler::new(vec![check]);
    let now = Instant::now();

    scheduler.tick(now).await;
    scheduler.tick(now + Duration::from_millis(10)).await;
    scheduler.tick(now + Duration::from_millis(20)).await;

    let asked = asked.asked.lock().expect("the log is not poisoned").clone();
    assert_eq!(
        asked,
        vec![0, 1, 2],
        "sequence numbers increase, so a result that finishes late is discarded rather \\
         than applied out of order (`I-26`)"
    );
}

#[tokio::test]
async fn nothing_is_probed_before_its_interval_elapses() {
    let check = Scripted::new(spec("api", 100, 1, 1), vec![CheckStatus::Passing]);
    let asked = Arc::clone(&check);
    let mut scheduler = Scheduler::new(vec![check]);
    let now = Instant::now();

    scheduler.tick(now).await;
    let report = scheduler.tick(now + Duration::from_millis(1)).await;
    assert!(
        report.changes.is_empty(),
        "a check is not probed twice in one interval: {:?}",
        report.changes
    );
    assert_eq!(
        asked.asked.lock().expect("the log is not poisoned").len(),
        1
    );

    // And it is probed again once the interval has passed.
    scheduler.tick(now + Duration::from_millis(10)).await;
    assert_eq!(
        asked.asked.lock().expect("the log is not poisoned").len(),
        2
    );
}

#[tokio::test]
async fn the_grace_period_ignores_failures_entirely() {
    let mut with_grace = spec("api", 100, 1, 1);
    with_grace.initial_grace_period = Duration::from_millis(50);
    let check = Scripted::new(
        with_grace,
        vec![
            CheckStatus::Failing,
            CheckStatus::Failing,
            CheckStatus::Passing,
        ],
    );
    let started = Instant::now();
    let mut scheduler = Scheduler::with_generation(vec![check], Generation::initial(), started);

    // Inside the grace period a failure is not yet a failure: the service has not
    // had time to come up, and a node that demotes itself for its own startup is
    // a node that never joins the election.
    let report = scheduler.tick(started).await;
    assert_eq!(
        report.summary.failing, 0,
        "the grace period ignores results"
    );
    assert_eq!(report.summary.penalty, 0);

    // Past the grace period the same failure counts, which is the point of
    // having one: it bounds the service's startup, not the check's judgement.
    let report = scheduler.tick(started + Duration::from_millis(60)).await;
    assert_eq!(
        report.summary.failing, 1,
        "after the grace period it counts"
    );
}

#[tokio::test]
async fn the_summary_sums_the_weights_of_the_failing_checks() {
    let first = Scripted::new(
        spec("api", 40, 1, 1),
        vec![CheckStatus::Failing, CheckStatus::Failing],
    );
    let second = Scripted::new(
        spec("db", 60, 1, 1),
        vec![CheckStatus::Failing, CheckStatus::Failing],
    );
    let third = Scripted::new(
        spec("cache", 0, 1, 1),
        vec![CheckStatus::Failing, CheckStatus::Failing],
    );
    let mut scheduler = Scheduler::new(vec![first, second, third]);
    let now = Instant::now();

    let report = scheduler.tick(now).await;
    assert_eq!(report.summary.failing, 3, "three checks are failing");
    assert_eq!(
        report.summary.penalty, 100,
        "and only the weighted ones cost priority: a weight of zero is observational (`R-14`)"
    );
}
