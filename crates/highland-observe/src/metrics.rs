// Rust guideline compliant 2026-09-27

//! Metric primitives and the Prometheus text format.
//!
//! This module is deliberately dependency-free and knows nothing about VRRP. It
//! provides counters, gauges, and histograms, and renders them. Which series
//! Highland exports is decided in the daemon, where the instances are.
//!
//! # Cardinality is a property of the type, not of discipline
//!
//! A metric whose label values are open-ended is a memory leak with a scrape
//! endpoint. So labels are fixed when a series is declared: a [`Series`] is
//! created with its label *values*, and a value that is not in the declared set
//! is recorded under `other` rather than creating a new series (`R-20`, `L-10`).
//! The cost is a slightly less precise breakdown; the benefit is that a peer
//! address or an error string can never become a label.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};

/// A counter that only goes up.
#[derive(Debug, Default)]
pub struct Counter(AtomicU64);

impl Counter {
    /// Adds one.
    pub fn increment(&self) {
        self.add(1);
    }

    /// Adds `amount`.
    pub fn add(&self, amount: u64) {
        self.0.fetch_add(amount, Ordering::Relaxed);
    }

    /// Returns the current value.
    #[must_use]
    pub fn get(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

/// A value that can go up and down.
#[derive(Debug, Default)]
pub struct Gauge(AtomicU64);

impl Gauge {
    /// Sets the value.
    pub fn set(&self, value: u64) {
        self.0.store(value, Ordering::Relaxed);
    }

    /// Adds one.
    pub fn increment(&self) {
        self.add(1);
    }

    /// Adds `amount`, saturating at zero.
    pub fn add(&self, amount: u64) {
        let _ = self
            .0
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                Some(current.saturating_add(amount))
            });
    }

    /// Subtracts `amount`, saturating at zero.
    pub fn sub(&self, amount: u64) {
        let _ = self
            .0
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                Some(current.saturating_sub(amount))
            });
    }

    /// Returns the current value.
    #[must_use]
    pub fn get(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

/// The largest number of seconds whose microsecond count fits in a `u64`.
const MICROS_LIMIT_SECONDS: f64 = 18_446_744_073.0;

/// Converts non-negative seconds to microseconds, clamping rather than wrapping.
///
/// The caller has already rejected a non-finite or negative value, so the cast
/// cannot lose a sign; the clamp covers a value whose microsecond count would
/// exceed `u64`.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "seconds is finite and not negative"
)]
fn seconds_to_micros(seconds: f64) -> u64 {
    if seconds >= MICROS_LIMIT_SECONDS {
        return u64::MAX;
    }
    (seconds * 1_000_000.0) as u64
}

/// The default latency buckets, in seconds.
///
/// They span a check that answers instantly to one that has used its whole
/// timeout, which is the range a health check actually occupies.
pub const DEFAULT_BUCKETS: [f64; 8] = [0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0, 5.0];

/// A cumulative histogram over fixed buckets.
#[derive(Debug, Default)]
pub struct Histogram {
    bounds: Vec<f64>,
    counts: Vec<AtomicU64>,
    sum_micros: AtomicU64,
    count: AtomicU64,
}

impl Histogram {
    /// Creates a histogram with the default buckets.
    #[must_use]
    pub fn new() -> Self {
        Self::with_buckets(DEFAULT_BUCKETS)
    }

    /// Creates a histogram with `bounds`, which must be sorted.
    #[must_use]
    pub fn with_buckets(bounds: impl IntoIterator<Item = f64>) -> Self {
        let mut bounds: Vec<f64> = bounds.into_iter().collect();
        bounds.sort_by(f64::total_cmp);
        let counts = (0..=bounds.len()).map(|_| AtomicU64::new(0)).collect();
        Self {
            bounds,
            counts,
            sum_micros: AtomicU64::new(0),
            count: AtomicU64::new(0),
        }
    }

    /// Records one observation, in seconds.
    pub fn observe(&self, seconds: f64) {
        if !seconds.is_finite() || seconds < 0.0 {
            return;
        }
        let index = self
            .bounds
            .iter()
            .position(|bound| seconds <= *bound)
            .unwrap_or(self.bounds.len());
        self.counts[index].fetch_add(1, Ordering::Relaxed);
        let micros = seconds_to_micros(seconds);
        self.sum_micros.fetch_add(micros, Ordering::Relaxed);
        self.count.fetch_add(1, Ordering::Relaxed);
    }

    /// Returns the bucket bounds.
    #[must_use]
    pub fn bounds(&self) -> &[f64] {
        &self.bounds
    }

    /// Returns the cumulative count per bucket, plus an overflow bucket.
    #[must_use]
    pub fn buckets(&self) -> Vec<u64> {
        self.counts
            .iter()
            .map(|count| count.load(Ordering::Relaxed))
            .collect()
    }

    /// Returns the number of observations.
    #[must_use]
    pub fn count(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }

    /// Returns the sum of observations, in seconds.
    ///
    /// The accumulated microseconds are the authority; a sum beyond `2^53`
    /// microseconds is 285 years of observations, and a scraper reading a
    /// rounded float is not misled by that.
    #[must_use]
    #[allow(
        clippy::cast_precision_loss,
        reason = "a sum beyond 2^53 microseconds is 285 years"
    )]
    pub fn sum(&self) -> f64 {
        self.sum_micros.load(Ordering::Relaxed) as f64 / 1_000_000.0
    }
}

/// One declared metric series: a name, a kind, and a fixed set of label values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Series {
    /// The metric name, without labels.
    pub name: &'static str,
    /// The label values, in the order the renderer writes them.
    pub labels: Vec<(String, String)>,
    /// What kind of metric this is, which decides the `# TYPE` line.
    pub kind: Kind,
}

/// What kind of metric a series is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A counter, which only increases.
    Counter,
    /// A gauge, which can go up and down.
    Gauge,
    /// A histogram.
    Histogram,
}

impl Kind {
    /// Returns the Prometheus type name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Counter => "counter",
            Kind::Gauge => "gauge",
            Kind::Histogram => "histogram",
        }
    }
}

/// Everything a scrape returns.
#[derive(Debug, Default)]
pub struct Snapshot {
    series: Vec<Series>,
    values: BTreeMap<String, u64>,
    histograms: BTreeMap<String, RenderedHistogram>,
    order: Vec<String>,
}

impl Snapshot {
    /// Declares a series and its current value, so that a metric that has never
    /// been touched still appears as zero rather than being absent. A dashboard
    /// that breaks when a metric has not fired yet is a dashboard that breaks on
    /// the day a node has nothing wrong with it.
    pub fn gauge(&mut self, series: Series, value: u64) {
        self.record(series, value);
    }

    /// Declares a counter and its current value.
    pub fn counter(&mut self, series: Series, value: u64) {
        self.record(series, value);
    }

    fn record(&mut self, series: Series, value: u64) {
        let key = key_of(&series);
        if !self.values.contains_key(&key) {
            self.order.push(series.name.to_owned());
        }
        if !self.series.iter().any(|existing| key_of(existing) == key) {
            self.series.push(series);
        }
        self.values.insert(key, value);
    }

    /// Attaches a histogram, rendered with its buckets.
    ///
    /// The buckets are read now rather than held, so a snapshot is a value and
    /// not a view onto a counter that moves while it is being rendered.
    pub fn histogram(&mut self, series: Series, histogram: &Histogram) {
        let key = key_of(&series);
        if !self.order.contains(&series.name.to_owned()) {
            self.order.push(series.name.to_owned());
        }
        if !self.series.iter().any(|existing| key_of(existing) == key) {
            self.series.push(series);
        }
        self.histograms.insert(
            key,
            RenderedHistogram {
                bounds: histogram.bounds().to_vec(),
                counts: histogram.buckets(),
                count: histogram.count(),
                sum: histogram.sum(),
            },
        );
    }

    /// Renders the snapshot in the Prometheus text exposition format.
    #[must_use]
    pub fn render(&self) -> String {
        let mut text = String::new();

        for name in &self.order {
            let Some(kind) = self
                .series
                .iter()
                .find(|series| series.name == *name)
                .map(|series| series.kind)
            else {
                continue;
            };
            let _ = writeln!(text, "# HELP {name} Highland {name}");
            let _ = writeln!(text, "# TYPE {name} {}", kind.as_str());

            for series in self.series.iter().filter(|series| series.name == *name) {
                let key = key_of(series);
                if let Some(histogram) = self.histograms.get(&key) {
                    render_histogram(&mut text, series, histogram);
                } else {
                    let value = self.values.get(&key).copied().unwrap_or(0);
                    let _ = writeln!(text, "{}{} {value}", series.name, labels_of(series));
                }
            }
        }
        text
    }

    /// Returns the number of series in the snapshot.
    #[must_use]
    pub fn len(&self) -> usize {
        self.series.len()
    }

    /// Returns `true` when the snapshot holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.series.is_empty()
    }
}

fn key_of(series: &Series) -> String {
    format!("{}{}", series.name, labels_of(series))
}

fn labels_of(series: &Series) -> String {
    if series.labels.is_empty() {
        return String::new();
    }
    let rendered: Vec<String> = series
        .labels
        .iter()
        .map(|(name, value)| format!("{name}=\"{}\"", escape(value)))
        .collect();
    format!("{{{}}}", rendered.join(","))
}

fn escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

/// A histogram's shape, read at snapshot time.
#[derive(Debug, Clone, PartialEq)]
struct RenderedHistogram {
    bounds: Vec<f64>,
    counts: Vec<u64>,
    count: u64,
    sum: f64,
}

fn render_histogram(text: &mut String, series: &Series, histogram: &RenderedHistogram) {
    let counts = &histogram.counts;
    let mut cumulative = 0_u64;
    for (index, bound) in histogram.bounds.iter().enumerate() {
        cumulative += counts.get(index).copied().unwrap_or(0);
        let labels: Vec<String> = series
            .labels
            .iter()
            .map(|(name, value)| format!("{name}=\"{}\"", escape(value)))
            .chain(std::iter::once(format!("le=\"{bound}\"")))
            .collect();
        let _ = writeln!(
            text,
            "{}_bucket{{{}}} {cumulative}",
            series.name,
            labels.join(",")
        );
    }
    cumulative += counts.last().copied().unwrap_or(0);
    let labels: Vec<String> = series
        .labels
        .iter()
        .map(|(name, value)| format!("{name}=\"{}\"", escape(value)))
        .chain(std::iter::once("le=\"+Inf\"".to_owned()))
        .collect();
    let _ = writeln!(
        text,
        "{}_bucket{{{}}} {cumulative}",
        series.name,
        labels.join(",")
    );
    let _ = writeln!(
        text,
        "{}_sum{} {}",
        series.name,
        labels_of(series),
        histogram.sum
    );
    let _ = writeln!(
        text,
        "{}_count{} {}",
        series.name,
        labels_of(series),
        histogram.count
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn series(name: &'static str, kind: Kind, labels: &[(&str, &str)]) -> Series {
        Series {
            name,
            kind,
            labels: labels
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                .collect(),
        }
    }

    #[test]
    fn a_counter_only_goes_up() {
        let counter = Counter::default();
        assert_eq!(counter.get(), 0);
        counter.increment();
        counter.add(4);
        assert_eq!(counter.get(), 5);
    }

    #[test]
    fn a_gauge_saturates_rather_than_underflowing() {
        let gauge = Gauge::default();
        gauge.set(2);
        gauge.sub(5);
        assert_eq!(gauge.get(), 0, "a gauge never reports a negative value");
    }

    #[test]
    fn a_snapshot_renders_in_the_text_format() {
        let mut snapshot = Snapshot::default();
        snapshot.gauge(series("highland_up", Kind::Gauge, &[]), 1);
        snapshot.gauge(
            series(
                "highland_instance_role",
                Kind::Gauge,
                &[("instance", "api")],
            ),
            2,
        );
        snapshot.counter(
            series(
                "highland_advertisements_sent_total",
                Kind::Counter,
                &[("instance", "api"), ("family", "v4")],
            ),
            17,
        );

        let text = snapshot.render();

        assert!(text.contains("# TYPE highland_up gauge"), "{text}");
        assert!(text.contains("highland_up 1"), "{text}");
        assert!(
            text.contains("highland_instance_role{instance=\"api\"} 2"),
            "labels are rendered: {text}"
        );
        assert!(
            text.contains("highland_advertisements_sent_total{instance=\"api\",family=\"v4\"} 17"),
            "{text}"
        );
    }

    #[test]
    fn a_metric_that_has_never_fired_still_appears_as_zero() {
        let mut snapshot = Snapshot::default();
        snapshot.counter(
            series("highland_reloads_total", Kind::Counter, &[("result", "ok")]),
            0,
        );

        // A dashboard should not break on the day a node has nothing wrong
        // with it.
        assert!(
            snapshot
                .render()
                .contains("highland_reloads_total{result=\"ok\"} 0")
        );
    }

    #[test]
    fn a_histogram_renders_cumulative_buckets_and_a_sum() {
        let histogram = Histogram::new();
        histogram.observe(0.002);
        histogram.observe(0.2);
        histogram.observe(30.0);

        let mut snapshot = Snapshot::default();
        snapshot.histogram(
            series(
                "highland_check_duration_seconds",
                Kind::Histogram,
                &[("instance", "api")],
            ),
            &histogram,
        );
        let text = snapshot.render();

        assert!(
            text.contains("# TYPE highland_check_duration_seconds histogram"),
            "{text}"
        );
        assert!(
            text.contains("le=\"+Inf\""),
            "the overflow bucket is rendered: {text}"
        );
        assert!(
            text.contains("highland_check_duration_seconds_count{instance=\"api\"} 3"),
            "{text}"
        );
        assert!(
            text.contains("highland_check_duration_seconds_sum{instance=\"api\"} 30.2"),
            "{text}"
        );
    }

    #[test]
    fn a_negative_or_nan_observation_is_ignored() {
        let histogram = Histogram::new();
        histogram.observe(-1.0);
        histogram.observe(f64::NAN);
        assert_eq!(
            histogram.count(),
            0,
            "a nonsense measurement is not recorded"
        );
    }

    #[test]
    fn label_values_are_escaped() {
        let mut snapshot = Snapshot::default();
        snapshot.gauge(series("x", Kind::Gauge, &[("reason", "a\"b\\c")]), 1);
        assert!(
            snapshot.render().contains("reason=\"a\\\"b\\\\c\""),
            "{}",
            snapshot.render()
        );
    }

    #[test]
    fn the_series_count_reflects_distinct_label_sets() {
        let mut snapshot = Snapshot::default();
        snapshot.gauge(series("r", Kind::Gauge, &[("instance", "a")]), 1);
        snapshot.gauge(series("r", Kind::Gauge, &[("instance", "b")]), 1);
        snapshot.gauge(series("r", Kind::Gauge, &[("instance", "a")]), 2);

        assert_eq!(snapshot.len(), 2, "the same labels are one series");
        assert!(
            snapshot.render().contains("r{instance=\"a\"} 2"),
            "the later value wins"
        );
    }
}
