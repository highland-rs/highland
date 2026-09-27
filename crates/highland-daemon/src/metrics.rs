// Rust guideline compliant 2026-09-27

//! The metrics the daemon exports, and the endpoint that serves them.
//!
//! The series are declared here, beside the instances that produce them, and
//! rendered from the same status registry the control API reads. That is the
//! point: metrics and status cannot disagree about a role, because they are
//! rendered from one source (`SPEC.md` §16.2).
//!
//! # Only series with a producer are exported
//!
//! A metric that is always zero is a lie about the system, and a dashboard
//! built on it is a dashboard that never fires. The check and histogram series
//! named in `SPEC.md` §16.2 therefore appear when the checks that produce them
//! land in Milestone 6, and not before. Their absence is deliberate and is
//! recorded in the changelog.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use highland_observe::{Counter, Histogram, Kind, Series, Snapshot};

use crate::control::StatusRegistry;

/// The label value used when a value is not in the declared set.
///
/// The set of rejection reasons is closed (`R-20`), so this is a belt and
/// braces rather than a routine path.
const OTHER: &str = "other";

/// Counters and gauges for one instance.
#[derive(Debug, Default)]
struct InstanceMetrics {
    transitions: BTreeMap<(String, String), Counter>,
    advertisements_sent: BTreeMap<String, Counter>,
    advertisements_received: BTreeMap<String, Counter>,
    rejected_packets: BTreeMap<String, Counter>,
    master_down_events: Counter,
    vip_add_failures: Counter,
    vip_remove_failures: Counter,
}

/// Every counter and gauge the daemon keeps.
#[derive(Debug, Default)]
pub struct Metrics {
    instances: Mutex<BTreeMap<String, InstanceMetrics>>,
    reloads: Mutex<BTreeMap<String, Counter>>,
    control_requests: Mutex<BTreeMap<(String, String), Counter>>,
    check_failures: Mutex<BTreeMap<(String, String), Counter>>,
    check_duration: Mutex<BTreeMap<(String, String), Histogram>>,
    start: AtomicU64,
}

impl Metrics {
    /// Creates an empty set, with the process marked as started.
    #[must_use]
    pub fn new() -> Self {
        Self {
            start: AtomicU64::new(1),
            ..Self::default()
        }
    }

    /// Returns a handle the tasks can share.
    #[must_use]
    pub fn shared() -> Arc<Self> {
        Arc::new(Self::new())
    }

    /// Takes a lock, recovering from poisoning.
    ///
    /// A panic while recording a counter must not stop a node from forwarding
    /// addresses, so the data is taken even from a poisoned lock. The worst a
    /// counter can be is slightly wrong, and a node that has panicked in its
    /// metrics task is a node that still owns a VIP.
    fn lock<T>(cell: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        cell.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn with_instance(&self, name: &str, apply: impl FnOnce(&mut InstanceMetrics)) {
        let mut instances = Self::lock(&self.instances);
        let entry = instances.entry(name.to_owned()).or_default();
        apply(entry);
    }

    /// Records a role change.
    pub fn record_transition(&self, instance: &str, from: &str, to: &str) {
        self.with_instance(instance, |metrics| {
            metrics
                .transitions
                .entry((from.to_owned(), to.to_owned()))
                .or_default()
                .increment();
        });
    }

    /// Records one advertisement sent.
    pub fn record_advertisement_sent(&self, instance: &str, family: &str) {
        self.with_instance(instance, |metrics| {
            metrics
                .advertisements_sent
                .entry(family.to_owned())
                .or_default()
                .increment();
        });
    }

    /// Records one advertisement accepted.
    pub fn record_advertisement_received(&self, instance: &str, family: &str) {
        self.with_instance(instance, |metrics| {
            metrics
                .advertisements_received
                .entry(family.to_owned())
                .or_default()
                .increment();
        });
    }

    /// Records one rejected datagram.
    ///
    /// An unrecognised reason is folded into `other` rather than becoming a new
    /// label value, so a hostile peer cannot grow the metric set (`L-10`).
    pub fn record_rejection(&self, instance: &str, reason: &str, known: &[&str]) {
        let label = if known.contains(&reason) {
            reason
        } else {
            OTHER
        };
        self.with_instance(instance, |metrics| {
            metrics
                .rejected_packets
                .entry(label.to_owned())
                .or_default()
                .increment();
        });
    }

    /// Returns the rejection reasons this build exports.
    #[must_use]
    pub fn known_rejections() -> &'static [&'static str] {
        &[
            "bad_checksum",
            "bad_length",
            "bad_ttl",
            "bad_type",
            "bad_version",
            "rate_limited",
            "too_short",
            "unknown_peer",
            "wrong_vrid",
        ]
    }

    /// Records one master-down takeover.
    pub fn record_master_down(&self, instance: &str) {
        self.with_instance(instance, |metrics| metrics.master_down_events.increment());
    }

    /// Records a failed address addition.
    pub fn record_vip_add_failure(&self, instance: &str) {
        self.with_instance(instance, |metrics| metrics.vip_add_failures.increment());
    }

    /// Records a failed address removal.
    pub fn record_vip_remove_failure(&self, instance: &str) {
        self.with_instance(instance, |metrics| metrics.vip_remove_failures.increment());
    }

    /// Records a reload outcome.
    pub fn record_reload(&self, result: &str) {
        let mut reloads = Self::lock(&self.reloads);
        let label = if matches!(result, "accepted" | "rejected") {
            result
        } else {
            OTHER
        };
        reloads.entry(label.to_owned()).or_default().increment();
    }

    /// Records a control request.
    pub fn record_control_request(&self, command: &str, result: &str) {
        let mut requests = Self::lock(&self.control_requests);
        requests
            .entry((command.to_owned(), result.to_owned()))
            .or_default()
            .increment();
    }

    /// Renders every exported metric.
    ///
    /// Long because it is a declaration list rather than logic: every series
    /// Highland exports is named here, in one place, so that a reader can check
    /// the export against `SPEC.md` §16.2 without following the code.
    #[allow(
        clippy::too_many_lines,
        reason = "a declaration list reads better in one piece"
    )]
    #[must_use]
    pub fn snapshot(&self, registry: &StatusRegistry, version: &str) -> Snapshot {
        let mut snapshot = Snapshot::default();

        snapshot.gauge(
            Series {
                name: "highland_build_info",
                kind: Kind::Gauge,
                labels: vec![("version".into(), version.into())],
            },
            1,
        );
        snapshot.gauge(
            Series {
                name: "highland_up",
                kind: Kind::Gauge,
                labels: Vec::new(),
            },
            self.start.load(Ordering::Relaxed),
        );

        let status = registry.node_status();
        for instance in &status.instances {
            let labels = vec![("instance".to_owned(), instance.name.clone())];
            snapshot.gauge(
                Series {
                    name: "highland_instance_role",
                    kind: Kind::Gauge,
                    labels: labels.clone(),
                },
                u64::from(role_code(&instance.role)),
            );
            snapshot.gauge(
                Series {
                    name: "highland_instance_effective_priority",
                    kind: Kind::Gauge,
                    labels: labels.clone(),
                },
                u64::from(instance.effective_priority),
            );
            snapshot.gauge(
                Series {
                    name: "highland_instance_health",
                    kind: Kind::Gauge,
                    labels: labels.clone(),
                },
                u64::from(u8::from(instance.health == "healthy")),
            );
            snapshot.gauge(
                Series {
                    name: "highland_instance_vips_owned",
                    kind: Kind::Gauge,
                    labels,
                },
                u64::from(instance.vips_owned),
            );
        }

        let instances = Self::lock(&self.instances);
        for (name, metrics) in instances.iter() {
            let one = vec![("instance".to_owned(), name.clone())];
            for ((from, to), counter) in &metrics.transitions {
                snapshot.counter(
                    Series {
                        name: "highland_instance_transitions_total",
                        kind: Kind::Counter,
                        labels: [
                            ("instance".to_owned(), name.clone()),
                            ("from".to_owned(), from.clone()),
                            ("to".to_owned(), to.clone()),
                        ]
                        .into(),
                    },
                    counter.get(),
                );
            }
            for (family, counter) in &metrics.advertisements_sent {
                snapshot.counter(
                    Series {
                        name: "highland_advertisements_sent_total",
                        kind: Kind::Counter,
                        labels: [
                            ("instance".to_owned(), name.clone()),
                            ("family".to_owned(), family.clone()),
                        ]
                        .into(),
                    },
                    counter.get(),
                );
            }
            for (family, counter) in &metrics.advertisements_received {
                snapshot.counter(
                    Series {
                        name: "highland_advertisements_received_total",
                        kind: Kind::Counter,
                        labels: [
                            ("instance".to_owned(), name.clone()),
                            ("family".to_owned(), family.clone()),
                        ]
                        .into(),
                    },
                    counter.get(),
                );
            }
            for (reason, counter) in &metrics.rejected_packets {
                snapshot.counter(
                    Series {
                        name: "highland_rejected_packets_total",
                        kind: Kind::Counter,
                        labels: [
                            ("instance".to_owned(), name.clone()),
                            ("reason".to_owned(), reason.clone()),
                        ]
                        .into(),
                    },
                    counter.get(),
                );
            }
            snapshot.counter(
                Series {
                    name: "highland_master_down_events_total",
                    kind: Kind::Counter,
                    labels: one.clone(),
                },
                metrics.master_down_events.get(),
            );
            snapshot.counter(
                Series {
                    name: "highland_vip_add_failures_total",
                    kind: Kind::Counter,
                    labels: one.clone(),
                },
                metrics.vip_add_failures.get(),
            );
            snapshot.counter(
                Series {
                    name: "highland_vip_remove_failures_total",
                    kind: Kind::Counter,
                    labels: one,
                },
                metrics.vip_remove_failures.get(),
            );
        }
        for (result, counter) in Self::lock(&self.reloads).iter() {
            snapshot.counter(
                Series {
                    name: "highland_reloads_total",
                    kind: Kind::Counter,
                    labels: vec![("result".to_owned(), result.clone())],
                },
                counter.get(),
            );
        }
        for ((command, result), counter) in Self::lock(&self.control_requests).iter() {
            snapshot.counter(
                Series {
                    name: "highland_control_requests_total",
                    kind: Kind::Counter,
                    labels: [
                        ("command".to_owned(), command.clone()),
                        ("result".to_owned(), result.clone()),
                    ]
                    .into(),
                },
                counter.get(),
            );
        }
        for ((instance, check), counter) in Self::lock(&self.check_failures).iter() {
            snapshot.counter(
                Series {
                    name: "highland_check_failures_total",
                    kind: Kind::Counter,
                    labels: [
                        ("instance".to_owned(), instance.clone()),
                        ("check".to_owned(), check.clone()),
                    ]
                    .into(),
                },
                counter.get(),
            );
        }
        for ((instance, check), histogram) in Self::lock(&self.check_duration).iter() {
            snapshot.histogram(
                Series {
                    name: "highland_check_duration_seconds",
                    kind: Kind::Histogram,
                    labels: [
                        ("instance".to_owned(), instance.clone()),
                        ("check".to_owned(), check.clone()),
                    ]
                    .into(),
                },
                histogram,
            );
        }

        snapshot
    }

    /// Returns the number of instances with counters.
    #[must_use]
    pub fn tracked_instances(&self) -> usize {
        Self::lock(&self.instances).len()
    }
}

/// The numeric encoding of a role, which `R-19` fixes.
///
/// An unknown role reads as `INIT`, which is the conservative value: a scraper
/// that sees a role it does not recognize should conclude the node is not
/// forwarding rather than that it is.
fn role_code(role: &str) -> u8 {
    match role {
        "BACKUP" => 1,
        "MASTER" => 2,
        "FAULT" => 3,
        "DISABLED" => 4,
        _ => 0,
    }
}

/// Renders the snapshot as a scrape response.
#[must_use]
pub fn render(metrics: &Metrics, registry: &StatusRegistry, version: &str) -> String {
    metrics.snapshot(registry, version).render()
}

/// The content type a Prometheus scraper expects.
pub const CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::InstanceStatus;

    fn registry() -> StatusRegistry {
        let registry = StatusRegistry::new("node-a");
        registry.publish(InstanceStatus {
            name: "api".to_owned(),
            role: "MASTER".to_owned(),
            priority: 150,
            effective_priority: 150,
            owns_addresses: true,
            vip_addresses: vec!["192.0.2.100/24".to_owned()],
            peers: vec!["192.0.2.11".to_owned()],
            last_reason: "master_down_timeout".to_owned(),
        });
        registry
    }

    #[test]
    fn a_scrape_reports_the_role_of_each_instance() {
        let metrics = Metrics::new();
        let text = render(&metrics, &registry(), "0.1.0");

        assert!(text.contains("highland_up 1"), "{text}");
        assert!(
            text.contains("highland_build_info{version=\"0.1.0\"} 1"),
            "{text}"
        );
        assert!(
            text.contains("highland_instance_role{instance=\"api\"} 2"),
            "{text}"
        );
        assert!(
            text.contains("highland_instance_vips_owned{instance=\"api\"} 1"),
            "{text}"
        );
        assert!(
            text.contains("highland_instance_health{instance=\"api\"} 1"),
            "{text}"
        );
    }

    #[test]
    fn counters_appear_before_anything_happens() {
        let metrics = Metrics::new();
        metrics.record_transition("api", "BACKUP", "MASTER");
        let text = render(&metrics, &registry(), "0.1.0");

        assert!(
            text.contains("highland_instance_transitions_total{instance=\"api\",from=\"BACKUP\",to=\"MASTER\"} 1"),
            "{text}"
        );
        assert!(
            text.contains("highland_vip_add_failures_total{instance=\"api\"} 0"),
            "{text}"
        );
    }

    #[test]
    fn a_rejection_reason_outside_the_known_set_becomes_other() {
        let metrics = Metrics::new();
        metrics.record_rejection(
            "api",
            "peer address 192.0.2.99 said so",
            Metrics::known_rejections(),
        );
        metrics.record_rejection("api", "bad_ttl", Metrics::known_rejections());
        let text = render(&metrics, &registry(), "0.1.0");

        assert!(
            text.contains("highland_rejected_packets_total{instance=\"api\",reason=\"other\"} 1"),
            "{text}"
        );
        assert!(
            text.contains("highland_rejected_packets_total{instance=\"api\",reason=\"bad_ttl\"} 1"),
            "{text}"
        );
        assert!(
            !text.contains("192.0.2.99"),
            "R-20: a peer address must never become a label value: {text}"
        );
    }

    #[test]
    fn every_metric_is_of_the_documented_kind() {
        let metrics = Metrics::new();
        metrics.record_advertisement_sent("api", "v4");
        metrics.record_advertisement_received("api", "v4");
        metrics.record_master_down("api");
        metrics.record_vip_add_failure("api");
        metrics.record_reload("accepted");
        metrics.record_control_request("status", "ok");
        let text = render(&metrics, &registry(), "0.1.0");

        for name in [
            "highland_advertisements_sent_total",
            "highland_advertisements_received_total",
            "highland_master_down_events_total",
            "highland_vip_add_failures_total",
            "highland_reloads_total",
            "highland_control_requests_total",
        ] {
            assert!(
                text.contains(&format!("# TYPE {name} ")),
                "{name} is missing a type: {text}"
            );
        }
        assert!(
            text.contains("highland_advertisements_sent_total{instance=\"api\",family=\"v4\"} 1"),
            "{text}"
        );
    }

    #[test]
    fn a_metric_with_no_producer_is_not_exported() {
        // The check series are named in the specification but have no producer
        // until the checks land, and a metric that is always zero is a lie.
        let text = render(&Metrics::new(), &registry(), "0.1.0");
        assert!(!text.contains("highland_check_failures_total"), "{text}");
        assert!(!text.contains("highland_check_duration_seconds"), "{text}");
    }
}
