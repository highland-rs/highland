// Rust guideline compliant 2026-09-27

//! Structured logging, metrics, and the event stream.
//!
//! Observability is not an afterthought in Highland: ownership changes, role
//! transitions, and operator actions must all be visible, with a
//! machine-readable reason (SPEC.md, §16). This crate owns the event model,
//! the redaction layer, and the bounded ring buffer that makes the event
//! history replayable (`R-18`, `L-08`).

#![deny(missing_docs)]
#![forbid(unsafe_code)]

mod event;
mod redact;
mod ring;
mod sink;

pub use event::{Event, EventField, EventLevel, EventName};
pub use redact::{REDACTED, Redactor};
pub use ring::{EVENT_BUFFER_CAPACITY, EventRing};
pub use sink::{EventSink, NoopSink, RecordingSink};
