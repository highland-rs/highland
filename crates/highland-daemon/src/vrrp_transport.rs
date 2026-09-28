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

/// How many times a socket bind is attempted before giving up.
///
/// Enough to outlast duplicate address detection. The kernel's default is one
/// solicitation with a one-second retransmit, and a link that has just come up
/// can be a second or two from usable; six seconds is comfortably past that and
/// still short enough that a genuinely misconfigured address is reported without
/// an operator thinking the daemon has hung.
pub(crate) const BIND_ATTEMPTS: u32 = 6;

/// How long to wait between bind attempts.
pub(crate) const BIND_RETRY: Duration = Duration::from_secs(1);

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
    /// How many packets have been discarded for each reason, so the log can say
    /// "and this has now happened 3,000 times" without a line per packet.
    rejections: Arc<std::sync::Mutex<std::collections::BTreeMap<&'static str, u64>>>,
}

impl VrrpTransport {
    /// Binds a transport for one instance, waiting out IPv6 duplicate address
    /// detection if it is in the way.
    ///
    /// RFC 5798 §5.1.2.1 says an IPv6 advertisement is sent from the interface's
    /// link-local address, and a link-local address the kernel has just created is
    /// *tentative* for about a second while duplicate address detection runs. A
    /// socket cannot be bound to a tentative address — the kernel answers
    /// `EINVAL` — so a daemon that started in that window would refuse to start
    /// for a second, on every boot, with a message about a socket.
    ///
    /// Retrying is the difference between a node that is briefly late and a node
    /// that does not come up.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] when the socket cannot be created or bound,
    /// which needs `CAP_NET_RAW`, or when the address is still unusable after
    /// `BIND_ATTEMPTS` tries.
    pub async fn bind(
        plan: &InstancePlan,
        interface: &str,
        source: IpAddr,
        peering: Peering,
        metrics: Arc<crate::Metrics>,
        allow_unconforming_hop_limit: bool,
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

        // Strict unless the configuration says otherwise, and the policy is named
        // for the rule it relaxes rather than for the peer that needs it.
        let policy = if allow_unconforming_hop_limit {
            highland_net::ReceptionPolicy::LenientHopLimit
        } else {
            highland_net::ReceptionPolicy::Strict
        };
        if allow_unconforming_hop_limit {
            // Said once, at startup, and said plainly: a node in this mode is
            // accepting advertisements that may have crossed a router, and an
            // operator reading only the log must be able to learn that.
            tracing::warn!(
                instance = %plan.name,
                interface,
                "accepting advertisements whose hop limit is not 255; the check that proves an \
                 advertisement stayed on this link is disabled for this instance"
            );
        }

        let mut last = String::new();
        let mut inner = None;
        for attempt in 1..=BIND_ATTEMPTS {
            match SocketTransport::bind(family, interface, source, peering.clone(), policy) {
                Ok(transport) => {
                    inner = Some(transport);
                    break;
                }
                Err(error) => {
                    last = error.to_string();
                    if attempt < BIND_ATTEMPTS {
                        tracing::debug!(
                            instance = %plan.name,
                            %source,
                            attempt,
                            "waiting for the source address to become usable"
                        );
                        tokio::time::sleep(BIND_RETRY).await;
                    }
                }
            }
        }
        let inner = inner.ok_or_else(|| TransportError::Unavailable {
            reason: format!(
                "could not bind a VRRP socket to {source} on {interface} after {BIND_ATTEMPTS} \
                 attempts: {last}. An IPv6 link-local address is unusable while duplicate address \
                 detection is still running, and for a second or so after the link comes up; if \
                 the address is not IPv6, check that it is configured and not a virtual address"
            ),
        })?;
        Ok(Self {
            inner: Arc::new(inner),
            vrid: plan.vrid,
            name: plan.name.clone(),
            metrics,
            family: Self::family_name(family),
            rejections: Arc::new(std::sync::Mutex::new(std::collections::BTreeMap::new())),
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

    /// The TTL or hop limit of the last datagram read, for a rejection message.
    #[must_use]
    pub fn last_ttl(&self) -> u8 {
        self.inner.last_ttl()
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
                        // With the value that caused it: a `bad_ttl` rejection is
                        // a very different problem from a `bad_checksum` one, and
                        // "why" alone sends an operator looking in the wrong
                        // place.
                        let received_ttl = transport.last_ttl();
                        // Counted, never delivered. An unauthenticated peer must
                        // not be able to move a role, and must not be able to
                        // grow the metric set either (`R-20`).
                        transport.metrics.record_rejection(
                            &transport.name,
                            reason.as_str(),
                            crate::Metrics::known_rejections(),
                        );
                        // The first rejection of each reason is logged, and then
                        // every thousandth. A node silently discarding a peer's
                        // advertisements is the failure this very repository
                        // shipped: it was invisible without metrics enabled, and
                        // it looks like a working node until the failover is
                        // late. A flood is bounded by the counter rather than by
                        // the log volume.
                        let count = {
                            let mut seen = transport
                                .rejections
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            let entry = seen.entry(reason.as_str()).or_insert(0_u64);
                            *entry = entry.saturating_add(1);
                            *entry
                        };
                        if count == 1 || count % 1_000 == 0 {
                            tracing::warn!(
                                instance = %transport.name,
                                reason = reason.as_str(),
                                seen = count,
                                ttl = received_ttl,
                                "discarded a VRRP packet; if a peer is configured, it is not \
                                 being heard"
                            );
                        }
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
