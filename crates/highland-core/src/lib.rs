// Rust guideline compliant 2026-09-27

//! Deterministic VRRP domain logic for Highland.
//!
//! This crate holds the parts of Highland that must be testable without Linux,
//! without a network, and without an async runtime:
//!
//! - the per-instance state machine and its [role][state::Role] transitions,
//! - election, priority, and preemption policy,
//! - timer arithmetic,
//! - health-weight aggregation,
//! - configuration-independent policy evaluation.
//!
//! # Design constraints
//!
//! The crate is pure Rust domain logic. It MUST NOT open sockets, modify
//! interfaces, read files, spawn processes, depend on Linux, or log directly
//! (SPEC.md, §9.1). Time and randomness are reached only through the
//! [`Clock`] and [`Rng`] abstractions, so that unit tests are
//! deterministic.
//!
//! I/O is not performed here. The state machine consumes events and returns
//! [`Action`] values for an executor in `highland-net` to
//! apply; see SPEC.md, §11.
//!
//! # Example
//!
//! ```
//! use highland_core::clock::ManualClock;
//! use highland_core::machine::InstanceStateMachine;
//! use highland_core::state::{Action, Event, InstanceConfig, Role, TimerId};
//! use std::time::Duration;
//!
//! let clock = ManualClock::new();
//! let config = InstanceConfig {
//!     name: "api".to_owned(),
//!     vrid: 42,
//!     priority: 150,
//!     ..InstanceConfig::default()
//! };
//!
//! let mut machine = InstanceStateMachine::new(config, clock.clone());
//! let actions = machine.handle(Event::Startup);
//!
//! assert_eq!(machine.role(), Role::Backup);
//! assert!(actions.contains(&Action::ArmTimer {
//!     timer: TimerId::MasterDown,
//!     deadline: Duration::from_millis(3410),
//! }));
//!
//! // Timers are absolute deadlines, so a test asserts the deadline rather than
//! // sleeping. Firing one is an explicit event, not a side effect of time.
//! clock.advance(Duration::from_millis(3410));
//! assert!(machine.is_due(TimerId::MasterDown));
//! machine.handle(Event::TimerExpired(TimerId::MasterDown));
//! ```

#![deny(missing_docs)]
#![forbid(unsafe_code)]

pub mod clock;
pub mod election;
pub mod error;
pub mod health;
pub mod machine;
pub mod state;
pub mod timer;

pub use clock::{Clock, ManualClock, Rng, SequenceRng, SystemClock};
pub use error::{CoreError, Result};
pub use machine::{InstanceStateMachine, PendingOwnership};
pub use state::{
    Action, ActionKind, Event, Generation, InstanceConfig, PeerAdvertisement, Role, TimerId,
    TransitionReason,
};
