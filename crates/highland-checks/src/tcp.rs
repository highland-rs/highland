// Rust guideline compliant 2026-09-27

//! The TCP probe: does a port accept a connection?
//!
//! The simplest check that answers a real question, and the one most operators
//! reach for first: is the service behind this address listening? It is a
//! connect and nothing more, because a check that sends traffic the service has
//! to understand is a different check with a different failure mode.
//!
//! The connect is bounded by the caller, not by this module: the scheduler owns
//! the timeout so that every check type is bounded the same way and there is one
//! place where the bound is applied.

use std::net::SocketAddr;
use std::time::Instant;

use highland_core::state::Generation;

use crate::check::{Check, CheckSpec};
use crate::error::{CheckError, Result};
use crate::result::CheckResult;

/// A check that opens a TCP connection to an address.
#[derive(Debug, Clone)]
pub struct TcpCheck {
    spec: CheckSpec,
    target: SocketAddr,
}

impl TcpCheck {
    /// Creates a TCP check for `target`.
    ///
    /// # Errors
    ///
    /// Returns [`CheckError::UnresolvableTarget`] when the address has no port,
    /// because a portless address cannot be connected to and the failure would
    /// otherwise surface as a confusing connect error on every probe.
    pub fn new(spec: CheckSpec, target: &str) -> Result<Self> {
        let target: SocketAddr = target.parse().map_err(|_| CheckError::UnresolvableTarget {
            check: spec.name.clone(),
            target: target.to_owned(),
            detail: "a TCP check needs an address with a port, like 192.0.2.10:8080".to_owned(),
        })?;
        Ok(Self { spec, target })
    }

    /// The address this check connects to.
    #[must_use]
    pub fn target(&self) -> SocketAddr {
        self.target
    }
}

impl Check for TcpCheck {
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

impl TcpCheck {
    /// The probe itself, which [`Check::run`] boxes.
    async fn probe(&self, generation: Generation, sequence: u64) -> Result<CheckResult> {
        let started = Instant::now();
        let name = self.spec.name.clone();
        let outcome = tokio::net::TcpStream::connect(self.target).await;
        let latency = started.elapsed();

        match outcome {
            Ok(stream) => {
                // The connection is closed immediately: a check that holds a
                // connection open would fill the peer's accept queue every
                // interval, and a health check should be invisible from the
                // service's side.
                drop(stream);
                CheckResult::passing(
                    name,
                    latency,
                    Instant::now(),
                    generation,
                    sequence,
                    format!("connected to {}", self.target),
                )
            }
            Err(error) => CheckResult::failing(
                name,
                None,
                Instant::now(),
                generation,
                sequence,
                format!("could not connect to {}: {error}", self.target),
            ),
        }
    }
}
