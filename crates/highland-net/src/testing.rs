// Rust guideline compliant 2026-09-27

//! A scripted backend, for testing anything above the kernel.
//!
//! This is what `R-03` asks for: the daemon, the executor, and the failure
//! paths are all testable without privileges, without a namespace, and without
//! a network, because every kernel interaction is a record on a list rather
//! than a syscall.
//!
//! A script is a sequence of outcomes, one per call, so a test can say "the
//! second address addition fails" and get exactly that, every run.

use std::net::IpAddr;
use std::sync::Mutex;

use crate::backend::NetworkBackend;
use crate::error::{NetError, Result};
use crate::types::{Interface, InterfaceId, IpCidr, LinkState};

/// What a scripted call should do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Answer with the interface as it is currently modelled.
    Interface,
    /// Succeed.
    Ok,
    /// Fail with a permission error, the most common real failure.
    PermissionDenied,
    /// Fail with a "the address is already configured" error.
    AddressInUse,
    /// Fail with an arbitrary message.
    Failed(String),
}

impl Outcome {
    /// Returns the address of the interface the outcome is about.
    fn address(&self) -> Option<IpAddr> {
        match self {
            Outcome::AddressInUse => Some(IpAddr::from([192, 0, 2, 99])),
            _ => None,
        }
    }
}

/// One recorded interaction with the kernel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Call {
    /// An interface was looked up by name.
    Interface(String),
    /// An address was added.
    AddAddress(InterfaceId, IpCidr),
    /// An address was removed.
    RemoveAddress(InterfaceId, IpCidr),
    /// A gratuitous update was requested.
    GratuitousUpdate(InterfaceId, IpAddr),
}

/// A [`NetworkBackend`] whose answers are scripted.
///
/// # Examples
///
/// ```
/// use highland_net::{IpCidr, InterfaceId, NetworkBackend, Outcome, ScriptedBackend};
///
/// # async fn example() {
/// let backend = ScriptedBackend::new()
///     .with_interface("eth0")
///     .with_outcomes([Outcome::Interface, Outcome::Ok]);
/// let id = InterfaceId::new(2).expect("a valid index");
/// let address = IpCidr::parse("192.0.2.10/24").expect("valid");
///
/// backend.add_address(id, address).await.expect("the script permits it");
/// assert_eq!(backend.calls()[0], highland_net::Call::AddAddress(id, address));
/// # }
/// ```
#[derive(Debug)]
pub struct ScriptedBackend {
    script: Mutex<Vec<Outcome>>,
    interfaces: Mutex<Vec<Interface>>,
    addresses: Mutex<Vec<(InterfaceId, IpCidr)>>,
    calls: Mutex<Vec<Call>>,
}

impl ScriptedBackend {
    /// Creates a backend that refuses to look up any interface.
    #[must_use]
    pub fn new() -> Self {
        Self {
            script: Mutex::new(Vec::new()),
            interfaces: Mutex::new(Vec::new()),
            addresses: Mutex::new(Vec::new()),
            calls: Mutex::new(Vec::new()),
        }
    }

    /// Adds an interface the backend will report.
    #[must_use]
    pub fn with_interface(self, name: &str) -> Self {
        let id = self.interface_index(2);
        if let Ok(mut list) = self.interfaces.lock() {
            list.push(Interface {
                id,
                name: name.to_owned(),
                state: LinkState::Up,
                addresses: Vec::new(),
                mtu: Some(1500),
            });
        }
        self
    }

    /// Returns a deterministic interface index, so a test's expectations do not
    /// depend on insertion order.
    fn interface_index(&self, start: i32) -> InterfaceId {
        let used = self.interfaces.lock().map_or(0, |list| list.len());
        InterfaceId::new(start + i32::try_from(used).unwrap_or(0))
            .unwrap_or(InterfaceId::new(start).expect("a positive index is valid"))
    }

    /// Sets the outcomes returned by successive calls, in order.
    ///
    /// Once the script is exhausted, [`Outcome::Ok`] is returned, so a test that
    /// cares about one failure does not have to enumerate the calls after it.
    #[must_use]
    pub fn with_outcomes(self, outcomes: impl IntoIterator<Item = Outcome>) -> Self {
        if let Ok(mut script) = self.script.lock() {
            script.extend(outcomes);
        }
        self
    }

    /// Returns the interactions recorded so far, in order.
    #[must_use]
    pub fn calls(&self) -> Vec<Call> {
        self.calls
            .lock()
            .map(|calls| calls.clone())
            .unwrap_or_default()
    }

    /// Returns the addresses currently modelled as present.
    #[must_use]
    pub fn present(&self) -> Vec<(InterfaceId, IpCidr)> {
        self.addresses
            .lock()
            .map(|list| list.clone())
            .unwrap_or_default()
    }

    /// Marks an address as already present, so a test starts from a machine that
    /// is already a master.
    pub fn seed_address(&self, interface: InterfaceId, address: IpCidr) {
        if let Ok(mut list) = self.addresses.lock() {
            list.push((interface, address));
        }
    }

    /// Returns the index of an interface by name, for a test that needs one.
    #[must_use]
    pub fn interface_id(&self, name: &str) -> Option<InterfaceId> {
        self.interfaces.lock().ok().and_then(|list| {
            list.iter()
                .find(|interface| interface.name == name)
                .map(|interface| interface.id)
        })
    }

    fn next_outcome(&self) -> Outcome {
        self.script
            .lock()
            .ok()
            .and_then(|mut script| {
                if script.is_empty() {
                    None
                } else {
                    Some(script.remove(0))
                }
            })
            .unwrap_or(Outcome::Ok)
    }

    fn record(&self, call: Call) {
        if let Ok(mut calls) = self.calls.lock() {
            calls.push(call);
        }
    }
}

impl Default for ScriptedBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl NetworkBackend for ScriptedBackend {
    fn interface(&self, name: &str) -> impl std::future::Future<Output = Result<Interface>> + Send {
        // The script answers without awaiting anything, so the futures are
        // already complete. `async` here would be a promise the body does not
        // keep.
        let answer = self.answer_interface(name);
        std::future::ready(answer)
    }

    fn add_address(
        &self,
        interface: InterfaceId,
        address: IpCidr,
    ) -> impl std::future::Future<Output = Result<()>> + Send {
        let answer = self.answer_add_address(interface, address);
        std::future::ready(answer)
    }

    fn remove_address(
        &self,
        interface: InterfaceId,
        address: IpCidr,
    ) -> impl std::future::Future<Output = Result<()>> + Send {
        let answer = self.answer_remove_address(interface, address);
        std::future::ready(answer)
    }

    fn send_gratuitous_update(
        &self,
        interface: InterfaceId,
        address: IpAddr,
    ) -> impl std::future::Future<Output = Result<()>> + Send {
        let answer = self.answer_gratuitous_update(interface, address);
        std::future::ready(answer)
    }
}

impl ScriptedBackend {
    fn answer_interface(&self, name: &str) -> Result<Interface> {
        self.record(Call::Interface(name.to_owned()));
        let outcome = self.next_outcome();
        if !matches!(outcome, Outcome::Interface | Outcome::Ok) {
            return Err(error_for(&outcome));
        }
        self.interfaces
            .lock()
            .ok()
            .and_then(|list| {
                list.iter()
                    .find(|interface| interface.name == name)
                    .cloned()
            })
            .map(|mut interface| {
                interface.addresses = self
                    .present()
                    .into_iter()
                    .filter(|(id, _)| *id == interface.id)
                    .map(|(_, address)| address)
                    .collect();
                interface
            })
            .ok_or(NetError::InterfaceNotFound {
                name: name.to_owned(),
            })
    }

    fn answer_add_address(&self, interface: InterfaceId, address: IpCidr) -> Result<()> {
        self.record(Call::AddAddress(interface, address));
        let outcome = self.next_outcome();
        if !matches!(outcome, Outcome::Interface | Outcome::Ok) {
            return Err(error_for(&outcome));
        }
        if let Ok(mut list) = self.addresses.lock() {
            list.retain(|(id, present)| *id != interface || *present != address);
            list.push((interface, address));
        }
        Ok(())
    }

    fn answer_remove_address(&self, interface: InterfaceId, address: IpCidr) -> Result<()> {
        self.record(Call::RemoveAddress(interface, address));
        let outcome = self.next_outcome();
        if !matches!(outcome, Outcome::Interface | Outcome::Ok) {
            return Err(error_for(&outcome));
        }
        if let Ok(mut list) = self.addresses.lock() {
            list.retain(|(id, present)| *id != interface || *present != address);
        }
        Ok(())
    }

    fn answer_gratuitous_update(&self, interface: InterfaceId, address: IpAddr) -> Result<()> {
        self.record(Call::GratuitousUpdate(interface, address));
        let outcome = self.next_outcome();
        if matches!(outcome, Outcome::Ok) {
            Ok(())
        } else {
            Err(error_for(&outcome))
        }
    }
}

fn error_for(outcome: &Outcome) -> NetError {
    match outcome {
        Outcome::Ok | Outcome::Interface => NetError::Unsupported {
            operation: "an outcome the script did not expect",
        },
        Outcome::PermissionDenied => NetError::AddAddress {
            interface: "scripted".to_owned(),
            address: IpCidr::new(IpAddr::from([192, 0, 2, 10]), 24),
            source: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        },
        Outcome::AddressInUse => NetError::AddressAlreadyPresent {
            address: IpCidr::new(
                outcome.address().unwrap_or(IpAddr::from([192, 0, 2, 99])),
                24,
            ),
            existing: "eth1".to_owned(),
        },
        Outcome::Failed(message) => NetError::Io {
            operation: "a scripted failure",
            source: std::io::Error::other(message.clone()),
        },
    }
}
