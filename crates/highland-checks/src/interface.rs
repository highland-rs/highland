// Rust guideline compliant 2026-09-27

//! The interface probe: is this link present, up, and carrying?
//!
//! The check for the failure that needs no network at all. A node whose link is
//! down will fail over on a timer, and the operator's first question is whether
//! the thing they are about to reboot is even plugged in.
//!
//! It asks the kernel rather than the routing table, because a link can be
//! administratively up with no carrier and a route that exists says nothing
//! about either.
//!
//! The reading is a trait, not a netlink call. A link's state is a system call,
//! and a system call is something to mock: the daemon supplies the Netlink
//! implementation and a test supplies three states in a row, which is how the
//! three failure modes of this check get tested without three namespaces.

use std::fmt;
use std::sync::Arc;
use std::time::Instant;

use highland_core::state::Generation;

use crate::check::{Check, CheckKind, CheckSpec};
use crate::error::{CheckError, Result};
use crate::result::CheckResult;

/// What the kernel says about one interface.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LinkState {
    /// Whether the interface exists at all.
    pub present: bool,
    /// Whether it is administratively up.
    pub up: bool,
    /// Whether it has carrier.
    pub carrier: bool,
    /// The addresses configured on it, as text.
    pub addresses: Vec<String>,
}

/// Reads the state of an interface.
///
/// Implementations do I/O; the check does the deciding.
pub trait LinkProbe: Send + Sync + fmt::Debug {
    /// Returns the interface's state, or an error when it cannot be read.
    ///
    /// Boxed for the same reason [`Check::run`](crate::Check::run) is: the
    /// daemon's implementation reads netlink, a test's returns a fixed value, and
    /// both have to be usable as the same type.
    fn state(
        &self,
        interface: &str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = std::result::Result<LinkState, CheckError>>
                + Send
                + '_,
        >,
    >;
}

/// A check that an interface exists, is up, and has carrier.
#[derive(Debug, Clone)]
pub struct InterfaceCheck {
    spec: CheckSpec,
    interface: String,
    /// An address that must be present, which catches a link that is up but
    /// unconfigured.
    expected_address: Option<String>,
    probe: Arc<dyn LinkProbe>,
}

impl InterfaceCheck {
    /// Creates a check for `interface`, optionally requiring an address.
    #[must_use]
    pub fn new(
        spec: CheckSpec,
        interface: impl Into<String>,
        expected_address: Option<String>,
        probe: Arc<dyn LinkProbe>,
    ) -> Self {
        debug_assert_eq!(spec.kind, CheckKind::Interface);
        Self {
            spec,
            interface: interface.into(),
            expected_address,
            probe,
        }
    }

    /// The interface this check watches.
    #[must_use]
    pub fn interface(&self) -> &str {
        &self.interface
    }
}

impl Check for InterfaceCheck {
    fn spec(&self) -> &CheckSpec {
        &self.spec
    }

    fn run(
        &self,
        generation: Generation,
        sequence: u64,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<CheckResult>> + Send + '_>> {
        Box::pin(self.probe(generation, sequence))
    }
}

impl InterfaceCheck {
    /// The probe itself, which [`Check::run`] boxes.
    async fn probe(&self, generation: Generation, sequence: u64) -> Result<CheckResult> {
        let name = self.spec.name.clone();
        let interface = self.interface.clone();
        let expected = self.expected_address.clone();

        match self.probe.state(&interface).await {
            Ok(state) => {
                if let Some(address) = expected {
                    if !state.addresses.iter().any(|present| present == &address) {
                        return CheckResult::failing(
                            name,
                            None,
                            Instant::now(),
                            generation,
                            sequence,
                            format!("{interface} does not hold {address}"),
                        );
                    }
                }
                if !state.present {
                    return CheckResult::failing(
                        name,
                        None,
                        Instant::now(),
                        generation,
                        sequence,
                        format!("{interface} does not exist"),
                    );
                }
                if !state.carrier {
                    return CheckResult::failing(
                        name,
                        None,
                        Instant::now(),
                        generation,
                        sequence,
                        format!("{interface} has no carrier"),
                    );
                }
                if !state.up {
                    return CheckResult::failing(
                        name,
                        None,
                        Instant::now(),
                        generation,
                        sequence,
                        format!("{interface} is administratively down"),
                    );
                }
                CheckResult::passing(
                    name,
                    std::time::Duration::ZERO,
                    Instant::now(),
                    generation,
                    sequence,
                    format!("{interface} is up with carrier"),
                )
            }
            Err(error) => CheckResult::failing(
                name,
                None,
                Instant::now(),
                generation,
                sequence,
                format!("could not read {interface}: {error}"),
            ),
        }
    }
}
