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
//! [clock](clock) and [`Rng`](clock::Rng) abstractions, so that unit tests are
//! deterministic.
//!
//! I/O is not performed here. The state machine consumes events and returns
//! [`Action`](state::Action) values for an executor in `highland-net` to
//! apply; see SPEC.md, §11.
//!
//! # Example
//!
//! ```
//! use highland_core::clock::ManualClock;
//! use highland_core::state::{Action, Event, InstanceConfig, InstanceStateMachine, Role};
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
//! assert_eq!(machine.role(), Role::Init);
//!
//! let actions = machine.handle(Event::Startup);
//! assert_eq!(machine.role(), Role::Backup);
//! assert!(actions.iter().any(|a| matches!(a, Action::ArmTimer { .. })));
//!
//! // The master-down timer fires once no advertisement has been seen.
//! clock.advance(Duration::from_secs(3));
//! assert_eq!(machine.handle(Event::StartupDelayElapsed), vec![]);
//! ```

#![deny(missing_docs)]
#![forbid(unsafe_code)]

pub mod clock;
pub mod error;
pub mod state;

pub use error::{CoreError, Result};
pub use state::{Action, ActionKind, Event, InstanceConfig, InstanceStateMachine, Role, TimerId};
