// Rust guideline compliant 2026-09-27

//! The Unix socket probe: does a path accept a connection?
//!
//! A local service, checked the same way a network one is: connect, then close.
//! The path is confined to an allowed base directory by the configuration layer
//! (`V-27`), and this check does not relax that — a path that reached here was
//! already confined.

use std::path::{Path, PathBuf};
use std::time::Instant;

use highland_core::state::Generation;

use crate::check::{Check, CheckKind, CheckSpec};
use crate::error::Result;
use crate::result::CheckResult;

/// A check that connects to a Unix domain socket.
#[derive(Debug, Clone)]
pub struct UnixCheck {
    spec: CheckSpec,
    path: PathBuf,
}

impl UnixCheck {
    /// Creates a check for `path`.
    #[must_use]
    pub fn new(spec: CheckSpec, path: impl Into<PathBuf>) -> Self {
        debug_assert_eq!(spec.kind, CheckKind::Unix);
        Self {
            spec,
            path: path.into(),
        }
    }

    /// The path this check connects to.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Check for UnixCheck {
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

impl UnixCheck {
    /// The probe itself, which [`Check::run`] boxes.
    async fn probe(&self, generation: Generation, sequence: u64) -> Result<CheckResult> {
        let started = Instant::now();
        let name = self.spec.name.clone();
        let path = self.path.clone();

        match tokio::net::UnixStream::connect(&path).await {
            Ok(stream) => {
                drop(stream);
                CheckResult::passing(
                    name,
                    started.elapsed(),
                    Instant::now(),
                    generation,
                    sequence,
                    format!("connected to {}", path.display()),
                )
            }
            Err(error) => CheckResult::failing(
                name,
                None,
                Instant::now(),
                generation,
                sequence,
                format!("could not connect to {}: {error}", path.display()),
            ),
        }
    }
}
