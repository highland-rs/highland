// Rust guideline compliant 2026-09-27

//! Applying the state machine's actions.
//!
//! The machine requests; this module does. Every action becomes a call on a
//! [`NetworkBackend`], and every outcome comes back to the machine as an event,
//! because an action that is not confirmed is not done (`R-11`, `I-45`).
//!
//! The ownership handshake is the part that matters. `AddVirtualAddresses` asks
//! the backend to add every configured address; the machine only enters `MASTER`
//! when the executor reports success. A failure is reported as
//! `Event::ActionFailed { kind: AddAddresses }`, which the machine turns into
//! `FAULT` with a hold-down, not into a retry loop.
//!
//! # Advertisement sending
//!
//! Sending is separated from the backend on purpose. The backend owns addresses
//! and interfaces; the wire is a socket, and a socket is not a kernel mutation.
//! In this milestone a [`Transport`] implementation is supplied by the caller,
//! and the executor is generic over both, so the failover logic is testable
//! without a socket, a namespace, or privileges.

use std::sync::Arc;

use highland_core::state::{Action, ActionKind, Event};
use highland_net::{Call, IpCidr, NetworkBackend, PeerSet, ScriptedBackend};
use highland_vrrp::{Advertisement, IpFamily, MaxAdverInt, Priority, Vrid};

use crate::options::InstancePlan;

/// What one instance is configured to own, in the terms the executor needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ownership {
    /// The interface the instance is bound to.
    pub interface: String,
    /// The addresses to add when the instance becomes master.
    pub addresses: Vec<IpCidr>,
    /// The peers allowed to speak VRRP to this instance.
    pub peers: PeerSet,
}

impl Ownership {
    /// Creates an ownership description.
    #[must_use]
    pub fn new(interface: impl Into<String>, addresses: Vec<IpCidr>, peers: PeerSet) -> Self {
        Self {
            interface: interface.into(),
            addresses,
            peers,
        }
    }

    /// Returns the address family this instance protects, or `None` when it has
    /// no addresses.
    #[must_use]
    pub fn family(&self) -> Option<IpFamily> {
        self.addresses
            .first()
            .map(|address| IpFamily::of(&address.address()))
    }
}

/// Sends advertisements to peers.
///
/// The trait exists so the executor can be driven without a socket, and so the
/// transport can be replaced without touching the failover logic.
pub trait Transport: Send + Sync + std::fmt::Debug {
    /// Sends `advertisement` to every peer, returning how many destinations were
    /// written to.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] when the datagram could not be written at all.
    fn send(
        &self,
        advertisement: &Advertisement,
    ) -> impl std::future::Future<Output = Result<usize, TransportError>> + Send;
}

/// A transport that records what it was asked to send and writes nowhere.
#[derive(Debug, Default)]
pub struct RecordingTransport {
    sent: std::sync::Arc<std::sync::Mutex<Vec<Advertisement>>>,
}

impl RecordingTransport {
    /// Creates a transport that records without sending.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the advertisements this transport was asked to send.
    #[must_use]
    pub fn sent(&self) -> Vec<Advertisement> {
        self.sent
            .lock()
            .map(|sent| sent.clone())
            .unwrap_or_default()
    }
}

impl Transport for RecordingTransport {
    /// The recorder answers without awaiting anything, so its future is already
    /// complete.
    fn send(
        &self,
        advertisement: &Advertisement,
    ) -> impl std::future::Future<Output = Result<usize, TransportError>> + Send {
        let count = usize::from(!advertisement.addresses().is_empty());
        if let Ok(mut sent) = self.sent.lock() {
            sent.push(advertisement.clone());
        }
        std::future::ready(Ok(count))
    }
}

/// A transport failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum TransportError {
    /// The datagram could not be written to the socket.
    #[error("could not send an advertisement to {destinations} peer(s): {reason}")]
    Send {
        /// How many peers were addressed.
        destinations: usize,
        /// Why the write failed.
        reason: String,
    },

    /// The advertisement could not be encoded.
    #[error("could not encode an advertisement: {reason}")]
    Encode {
        /// Why encoding failed.
        reason: String,
    },
}

/// Applies a machine's actions to a backend and a transport.
#[derive(Debug)]
pub struct Executor<B, T> {
    backend: Arc<B>,
    transport: Arc<T>,
    plan: InstancePlan,
    ownership: Ownership,
    interface: Option<highland_net::InterfaceId>,
}

impl<B, T> Executor<B, T>
where
    B: NetworkBackend,
    T: Transport,
{
    /// Creates an executor.
    #[must_use]
    pub fn new(
        backend: Arc<B>,
        transport: Arc<T>,
        plan: InstancePlan,
        ownership: Ownership,
    ) -> Self {
        Self {
            backend,
            transport,
            plan,
            ownership,
            interface: None,
        }
    }

    /// Returns the plan this executor applies.
    #[must_use]
    pub fn plan(&self) -> &InstancePlan {
        &self.plan
    }

    /// Returns the addresses this instance owns.
    #[must_use]
    pub fn addresses(&self) -> &[IpCidr] {
        &self.ownership.addresses
    }

    /// Ensures the interface is known, looking it up once.
    async fn interface_id(&mut self) -> Result<highland_net::InterfaceId, Event> {
        if let Some(id) = self.interface {
            return Ok(id);
        }
        match self.backend.interface(&self.ownership.interface).await {
            Ok(interface) => {
                self.interface = Some(interface.id);
                Ok(interface.id)
            }
            Err(error) => Err(Event::ActionFailed {
                kind: ActionKind::AddAddresses,
                error: error.to_string(),
            }),
        }
    }

    /// Applies one action and returns the event to feed back, if any.
    ///
    /// `None` means the action needs no confirmation, which is true of timers,
    /// roles, events, logs, and the effective priority: the machine already
    /// knows those happened. Everything that touches the kernel or the wire
    /// answers with the outcome, so the machine never assumes one.
    pub async fn apply(&mut self, action: &Action) -> Option<Event> {
        match action {
            // Timers, roles, events, logs, and the effective priority are the
            // machine's own bookkeeping. None of them touches the kernel or the
            // wire, so none of them is confirmed.
            Action::ArmTimer { .. }
            | Action::CancelTimer { .. }
            | Action::EnterRole { .. }
            | Action::EmitEvent { .. }
            | Action::Log { .. }
            | Action::SetEffectivePriority { .. } => None,

            Action::SendAdvertisement { priority } => self.answer_send(*priority).await,
            Action::SendGratuitousUpdates => self.answer_gratuitous().await,
            Action::AddVirtualAddresses => self.answer_add_addresses().await,
            Action::RemoveVirtualAddresses => self.answer_remove_addresses().await,
            // The action set is `#[non_exhaustive]`, so an action added later
            // reports failure rather than being silently ignored.
            _ => Some(Event::ActionFailed {
                kind: action.kind(),
                error: "this executor does not implement that action".to_owned(),
            }),
        }
    }

    async fn answer_add_addresses(&mut self) -> Option<Event> {
        let id = match self.interface_id().await {
            Ok(id) => id,
            Err(event) => return Some(event),
        };
        for address in self.ownership.addresses.clone() {
            if let Err(error) = self.backend.add_address(id, address).await {
                return Some(Event::ActionFailed {
                    kind: ActionKind::AddAddresses,
                    error: error.to_string(),
                });
            }
        }
        Some(Event::ActionSucceeded {
            kind: ActionKind::AddAddresses,
        })
    }

    async fn answer_remove_addresses(&mut self) -> Option<Event> {
        let id = match self.interface_id().await {
            Ok(id) => id,
            Err(event) => return Some(event),
        };
        for address in self.ownership.addresses.clone() {
            if let Err(error) = self.backend.remove_address(id, address).await {
                return Some(Event::ActionFailed {
                    kind: ActionKind::RemoveAddresses,
                    error: error.to_string(),
                });
            }
        }
        Some(Event::ActionSucceeded {
            kind: ActionKind::RemoveAddresses,
        })
    }

    async fn answer_gratuitous(&mut self) -> Option<Event> {
        let id = match self.interface_id().await {
            Ok(id) => id,
            Err(event) => return Some(event),
        };
        let mut failures = Vec::new();
        for address in self.ownership.addresses.clone() {
            let _ = address;
            failures.push(
                self.backend
                    .send_gratuitous_update(id, address.address())
                    .await,
            );
        }
        if failures.iter().all(Result::is_ok) {
            Some(Event::ActionSucceeded {
                kind: ActionKind::GratuitousUpdate,
            })
        } else {
            // A gratuitous update that fails is logged and counted; it never
            // invalidates ownership (`R-11`). The machine is told so it can
            // record it, but it does not fault.
            Some(Event::ActionFailed {
                kind: ActionKind::GratuitousUpdate,
                error: "a gratuitous update was not delivered".to_owned(),
            })
        }
    }

    async fn answer_send(&mut self, priority: u8) -> Option<Event> {
        let advertisement = match self.advertisement(priority) {
            Ok(advertisement) => advertisement,
            Err(error) => {
                return Some(Event::ActionFailed {
                    kind: ActionKind::Advertisement,
                    error: error.to_string(),
                });
            }
        };

        if self.ownership.family().is_none() {
            return Some(Event::ActionFailed {
                kind: ActionKind::Advertisement,
                error: "the instance has no addresses to advertise".to_owned(),
            });
        }

        match self.transport.send(&advertisement).await {
            Ok(_) => Some(Event::ActionSucceeded {
                kind: ActionKind::Advertisement,
            }),
            Err(error) => Some(Event::ActionFailed {
                kind: ActionKind::Advertisement,
                error: error.to_string(),
            }),
        }
    }

    /// Builds the advertisement this instance would send at `priority`.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError::Encode`] when the plan's fields cannot be
    /// represented on the wire.
    pub fn advertisement_for(&self, priority: u8) -> Option<Advertisement> {
        self.advertisement(priority).ok()
    }

    /// Builds the advertisement this instance would send.
    fn advertisement(&self, priority: u8) -> Result<Advertisement, TransportError> {
        let vrid = Vrid::new(self.plan.vrid).map_err(|error| TransportError::Encode {
            reason: error.to_string(),
        })?;
        let priority = Priority::new(priority).map_err(|error| TransportError::Encode {
            reason: error.to_string(),
        })?;
        let interval =
            MaxAdverInt::from_duration(self.plan.advertisement_interval).map_err(|error| {
                TransportError::Encode {
                    reason: error.to_string(),
                }
            })?;
        let addresses: Vec<std::net::IpAddr> = self
            .ownership
            .addresses
            .iter()
            .map(IpCidr::address)
            .collect();

        Advertisement::new(vrid, priority, interval, addresses).map_err(|error| {
            TransportError::Encode {
                reason: error.to_string(),
            }
        })
    }
}

/// A convenience constructor for tests: a scripted backend and a recording
/// transport.
#[derive(Debug)]
pub struct TestHarness {
    /// The scripted backend.
    pub backend: Arc<ScriptedBackend>,
    /// The transport that records what it was asked to send.
    pub transport: Arc<RecordingTransport>,
}

impl TestHarness {
    /// Creates a harness with one interface and no scripted failures.
    #[must_use]
    pub fn new(interface: &str) -> Self {
        Self {
            backend: Arc::new(ScriptedBackend::new().with_interface(interface)),
            transport: Arc::new(RecordingTransport::new()),
        }
    }

    /// Returns the calls the backend recorded.
    #[must_use]
    pub fn calls(&self) -> Vec<Call> {
        self.backend.calls()
    }

    /// Returns the advertisements the transport was asked to send.
    #[must_use]
    pub fn sent(&self) -> Vec<Advertisement> {
        self.transport.sent()
    }
}
