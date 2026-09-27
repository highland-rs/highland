// Rust guideline compliant 2026-09-27

#![cfg(target_os = "linux")]
//! The daemon's transport, backed by a real socket.
//!
//! `Transport` is a daemon trait, and the socket lives in `highland-net`, so
//! something has to adapt one to the other. That something is here, which keeps
//! the dependency pointing the right way: the daemon knows about sockets,
//! `highland-net` does not know about daemons.
//!
//! The reader is a separate task. Sending and receiving share one socket without
//! a lock between them, because a node that is mid-takeover is still
//! advertising while it is listening, and a lock between the two would be a
//! lock in the middle of the protocol.
//!
//! The reader turns an accepted advertisement into an
//! [`Instruction::Event`](crate::Instruction::Event), which is the only way
//! anything reaches the state machine. A rejected datagram never gets that far:
//! it is counted and dropped here (`I-43`, `S-05`).

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use highland_core::state::{Event, PeerAdvertisement};
use highland_net::{Accepted, Peering, RateWindow, SocketTransport};

use crate::executor::{Transport, TransportError};
use crate::options::InstancePlan;

/// How long the reader sleeps when nothing has arrived.
///
/// The socket read is non-blocking, so this is the whole of the reader's
/// waiting. Two milliseconds bounds the extra latency on an advertisement to
/// well under the advertisement interval, and it costs a wakeup rather than a
/// spin.
pub const READER_INTERVAL: Duration = Duration::from_millis(2);

/// A transport that carries VRRP over a real socket.
#[derive(Debug, Clone)]
pub struct VrrpTransport {
    inner: Arc<SocketTransport>,
    vrid: u8,
    name: String,
    metrics: Arc<crate::Metrics>,
    family: &'static str,
}

impl VrrpTransport {
    /// Binds a transport for one instance.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] when the socket cannot be created or bound,
    /// which needs `CAP_NET_RAW`.
    pub fn bind(
        plan: &InstancePlan,
        interface: &str,
        source: IpAddr,
        peering: Peering,
        metrics: Arc<crate::Metrics>,
    ) -> Result<Self, TransportError> {
        // The instance speaks the family of the address it owns, and a
        // configuration with no peer in that family is refused rather than
        // bound to a socket that could never send (`V-07`). A multicast instance
        // needs no peer list: the group is where the peers are.
        let family = highland_vrrp::IpFamily::of(&source);
        if let Peering::Unicast(peers) = &peering
            && peers.of_family(family).is_empty()
        {
            return Err(TransportError::NoPeers { family });
        }

        let inner = SocketTransport::bind(family, interface, source, peering).map_err(|error| {
            TransportError::Unavailable {
                reason: error.to_string(),
            }
        })?;
        Ok(Self {
            inner: Arc::new(inner),
            vrid: plan.vrid,
            name: plan.name.clone(),
            metrics,
            family: Self::family_name(family),
        })
    }

    /// The label value for an address family.
    ///
    /// A family this build does not know is labelled `other`, so a future family
    /// cannot add an unbounded label (`R-20`).
    fn family_name(family: highland_vrrp::IpFamily) -> &'static str {
        match family {
            highland_vrrp::IpFamily::V4 => "v4",
            highland_vrrp::IpFamily::V6 => "v6",
            _ => "other",
        }
    }

    /// Returns the mode this transport speaks in, for a diagnostic.
    #[must_use]
    pub fn peering_mode(&self) -> &'static str {
        self.inner.peering().mode()
    }

    /// Returns the peers this transport sends to.
    #[must_use]
    pub fn destinations(&self) -> &[IpAddr] {
        self.inner.destinations()
    }

    /// Returns the address advertisements are sent from.
    #[must_use]
    pub fn source(&self) -> IpAddr {
        self.inner.source()
    }

    /// Reads and validates one datagram without blocking.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] when the read itself fails.
    pub fn poll(
        &self,
        now: Duration,
        rate: &mut RateWindow,
    ) -> Result<Option<Accepted>, TransportError> {
        self.inner
            .poll(self.vrid, now, rate)
            .map_err(|error| TransportError::Unavailable {
                reason: error.to_string(),
            })
    }

    /// Starts a task that feeds accepted advertisements to the instance.
    ///
    /// The task ends when `instructions` closes, which happens when the
    /// instance's actor stops (`I-41`).
    pub fn spawn_reader(
        &self,
        instructions: crate::InstructionSender,
    ) -> tokio::task::JoinHandle<()> {
        let transport = self.clone();
        tokio::spawn(async move {
            let mut rate = RateWindow::default();
            let mut elapsed = Duration::ZERO;
            loop {
                if instructions.is_closed() {
                    return;
                }
                match transport.poll(elapsed, &mut rate) {
                    Ok(Some(Accepted::Advertisement(accepted))) => {
                        let advertisement = &accepted.advertisement;
                        // The peer is the address the datagram came from, which
                        // is the address the VRRP header was sent from. Using
                        // our own would name ourselves as the peer.
                        let event = Event::AdvertisementReceived(PeerAdvertisement {
                            vrid: advertisement.vrid().get(),
                            priority: advertisement.priority().get(),
                            source: accepted.source,
                            advert_interval: advertisement.advert_interval(),
                        });
                        transport
                            .metrics
                            .record_advertisement_received(&transport.name, transport.family);
                        if instructions
                            .send(crate::Instruction::Event(event))
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                    // A rejected datagram is counted, not delivered. There is
                    // nothing to do with it here, and inventing an event for it
                    // would let an unauthenticated peer drive the machine.
                    Ok(Some(Accepted::Rejected(reason))) => {
                        // Counted, never delivered. An unauthenticated peer must
                        // not be able to move a role, and must not be able to
                        // grow the metric set either (`R-20`).
                        transport.metrics.record_rejection(
                            &transport.name,
                            reason.as_str(),
                            crate::Metrics::known_rejections(),
                        );
                    }
                    Ok(None) => {}
                    Err(_) => {
                        // The socket failed. A reader that cannot read is not
                        // useful, and the instance will fault on its own timer
                        // when it stops hearing advertisements.
                        return;
                    }
                }
                elapsed = elapsed.saturating_add(READER_INTERVAL);
                tokio::time::sleep(READER_INTERVAL).await;
            }
        })
    }
}

impl Transport for VrrpTransport {
    fn send(
        &self,
        advertisement: &highland_vrrp::Advertisement,
    ) -> impl std::future::Future<Output = Result<usize, TransportError>> + Send {
        let answer = self
            .inner
            .send(advertisement)
            .map(|sent| {
                // Counted per successful send, so a scraper reads the number of
                // datagrams that actually went out rather than the number of
                // attempts.
                for _ in 0..sent.destinations {
                    self.metrics
                        .record_advertisement_sent(&self.name, self.family);
                }
                sent.destinations
            })
            .map_err(|error| TransportError::Unavailable {
                reason: error.to_string(),
            });
        std::future::ready(answer)
    }
}
