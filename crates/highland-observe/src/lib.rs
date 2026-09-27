// Rust guideline compliant 2026-09-27

//! Structured logging, metrics, and the event stream.
//!
//! Observability is not an afterthought in Highland: ownership changes, role
//! transitions, and operator actions must all be visible, with a
//! machine-readable reason (SPEC.md, §16). This crate owns the event model, the
//! redaction layer, the bounded ring buffer that makes the event history
//! replayable (`R-18`, `L-08`), and the metric primitives.
//!
//! | Concern | Where it lives |
//! |---|---|
//! | The event model, and a closed set of event names | [`Event`], [`EventName`] |
//! | Bounded event history | [`EventRing`] |
//! | Counters, gauges, histograms, and the scrape format | [`Counter`], [`Gauge`], [`Histogram`], [`Snapshot`] |
//! | Keeping secrets out of both | [`Redactor`] |
//! | Timestamps, so an event stream can be read | [`now_timestamp`] |
//!
//! The crate depends on nothing but `serde` and `thiserror`: a metrics library
//! that drags in a web framework is a metrics library that cannot be tested.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

mod event;
mod metrics;
mod redact;
mod ring;
mod sink;
mod timestamp;

pub use event::{Event, EventField, EventLevel, EventName};
pub use metrics::{Counter, DEFAULT_BUCKETS, Gauge, Histogram, Kind, Series, Snapshot};
pub use redact::{REDACTED, Redactor};
pub use ring::{EVENT_BUFFER_CAPACITY, EventRing};
pub use sink::{EventSink, NoopSink, RecordingSink};
pub use timestamp::{format as format_timestamp, now as now_timestamp};
