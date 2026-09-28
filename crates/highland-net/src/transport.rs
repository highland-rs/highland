// Rust guideline compliant 2026-09-27

//! The socket-backed transport.
//!
//! Where `crate::vrrp` decides whether a datagram is an acceptable
//! advertisement, this module does the sending and receiving. The split is
//! deliberate: the rules are testable without a socket, and the socket is
//! testable without the rules.
//!
//! One transport serves one instance: it owns the socket and the peer list, both
//! of which are per-instance configuration. The rate window belongs to the
//! reader rather than the transport, so that reading and sending can share one
//! instance without a lock.
//!
//! Linux-only, because it holds a socket. A platform without one gets
//! [`UnavailableTransport`](crate::UnavailableTransport) instead.

use std::net::IpAddr;
use std::time::{Duration, Instant};

use highland_vrrp::{Advertisement, IpFamily};

use crate::vrrp::ReceptionPolicy;

use crate::error::{NetError, Result};
use crate::vrrp::{Accepted, Datagram, Destinations, Peering, RateWindow, VRRP_IP_PROTOCOL};

/// What one send achieved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sent {
    /// How many peers the advertisement was written to.
    pub destinations: usize,
}

/// The socket-backed transport for one instance.
#[derive(Debug)]
pub struct SocketTransport {
    socket: crate::VrrpSocket,
    family: IpFamily,
    peering: Peering,
    destinations: Destinations,
    source: IpAddr,
    /// The TTL or hop limit of the last datagram read, so a caller that is told
    /// *why* a packet was refused can also say what it saw.
    last_ttl: std::sync::atomic::AtomicU8,
    /// What this transport insists on beyond the protocol.
    policy: ReceptionPolicy,
}

impl SocketTransport {
    /// Binds a transport for one instance.
    ///
    /// # Errors
    ///
    /// Returns [`NetError::Io`] when the socket cannot be created or bound,
    /// which needs `CAP_NET_RAW`.
    pub fn bind(
        family: IpFamily,
        interface: &str,
        source: IpAddr,
        peering: Peering,
        policy: ReceptionPolicy,
    ) -> Result<Self> {
        // A group transport's socket is bound for receiving, which on IPv4 means
        // not being bound to the source address at all.
        let socket = match &peering {
            Peering::Unicast(_) => crate::VrrpSocket::bind(family, interface, source)?,
            Peering::Multicast { .. } => {
                crate::VrrpSocket::bind_for_group(family, interface, source)?
            }
        };
        let destinations = match &peering {
            Peering::Unicast(peers) => Destinations::new(peers.of_family(family)),
            Peering::Multicast { group, ttl } => {
                if IpFamily::of(group) != family {
                    return Err(NetError::Encode {
                        family,
                        reason: format!("{group} is not an {family} group"),
                    });
                }
                // Joined here rather than by the daemon, so a transport that
                // exists has already joined what it intends to speak for: a
                // membership that is remembered somewhere else is a membership
                // that is forgotten.
                socket.join_group(*group, *ttl)?;
                Destinations::new([*group])
            }
        };
        Ok(Self {
            socket,
            family,
            peering,
            destinations,
            source,
            last_ttl: std::sync::atomic::AtomicU8::new(0),
            policy,
        })
    }

    /// What this transport insists on beyond the protocol, for a diagnostic.
    #[must_use]
    pub fn policy(&self) -> ReceptionPolicy {
        self.policy
    }

    /// The TTL or hop limit of the last datagram read, or zero when nothing has
    /// been read yet.
    #[must_use]
    pub fn last_ttl(&self) -> u8 {
        self.last_ttl.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Returns how this transport reaches its peers.
    pub fn peering(&self) -> &Peering {
        &self.peering
    }

    /// Leaves any multicast group this transport joined.
    ///
    /// Called when the instance is torn down, so a stopped or reloaded instance
    /// stops being addressed by the group it no longer answers for.
    ///
    /// # Errors
    ///
    /// Returns [`NetError::Io`] when the membership cannot be left. A unicast
    /// transport has nothing to leave and returns `Ok`.
    pub fn leave(&self) -> Result<()> {
        match &self.peering {
            Peering::Unicast(_) => Ok(()),
            Peering::Multicast { group, .. } => self.socket.leave_group(*group),
        }
    }

    /// Returns the peer addresses this transport validates against.
    ///
    /// In multicast mode there is no peer list, because a member of the group is
    /// a peer. This is empty then, and the transport is still reachable: what
    /// arrives is validated by interface, VRID, and TTL instead.
    #[must_use]
    pub fn peers(&self) -> &[IpAddr] {
        match &self.peering {
            Peering::Unicast(peers) => peers.list(),
            Peering::Multicast { .. } => &[],
        }
    }

    /// Returns the family this transport speaks.
    #[must_use]
    pub fn family(&self) -> IpFamily {
        self.family
    }

    /// Returns the address advertisements are sent from.
    #[must_use]
    pub fn source(&self) -> IpAddr {
        self.source
    }

    /// Returns the peers this transport will send to.
    #[must_use]
    pub fn destinations(&self) -> &[IpAddr] {
        self.destinations.peers()
    }

    /// Returns the IP protocol this transport speaks, for a diagnostic.
    #[must_use]
    pub fn protocol() -> i32 {
        VRRP_IP_PROTOCOL
    }

    /// Sends one advertisement to every configured peer.
    ///
    /// # Errors
    ///
    /// Returns [`NetError::Io`] when a datagram cannot be written. A peer that
    /// fails does not stop the others: the failure is reported once, after every
    /// peer has been tried, because one unreachable peer is not a reason to stop
    /// telling the others that this node is master.
    pub fn send(&self, advertisement: &Advertisement) -> Result<Sent> {
        let mut delivered = 0_usize;
        let mut first_failure: Option<NetError> = None;
        for peer in self.destinations.peers() {
            // Built per destination, because an IPv6 advertisement's checksum
            // covers the pseudo-header and therefore the address the packet is
            // sent to. One build reused across destinations would produce
            // packets that no IPv6 receiver could validate.
            let bytes = match crate::vrrp::build(advertisement, self.family, self.source, *peer) {
                Ok(bytes) => bytes,
                Err(error) => {
                    if first_failure.is_none() {
                        first_failure = Some(error);
                    }
                    continue;
                }
            };
            match self.socket.send_to(&bytes, *peer) {
                Ok(_) => delivered += 1,
                Err(error) => {
                    if first_failure.is_none() {
                        first_failure = Some(error);
                    }
                }
            }
        }

        if delivered == 0
            && let Some(error) = first_failure
        {
            return Err(error);
        }
        Ok(Sent {
            destinations: delivered,
        })
    }

    /// Reads and validates one datagram, without blocking.
    ///
    /// The rate window is a parameter rather than a field, so that sending and
    /// receiving can share one transport without a lock between them: a node
    /// that is mid-takeover is still advertising while it listens.
    ///
    /// Returns `Ok(None)` when nothing has arrived, which is the normal case.
    ///
    /// # Errors
    ///
    /// Returns [`NetError::Io`] when the read itself fails.
    pub fn poll(&self, vrid: u8, now: Duration, rate: &mut RateWindow) -> Result<Option<Accepted>> {
        let Some(received) = self.socket.receive()? else {
            return Ok(None);
        };
        // Recorded before validation, because the reason a packet was refused is
        // only half the story without what it carried.
        self.last_ttl
            .store(received.ttl, std::sync::atomic::Ordering::Relaxed);
        // Our own advertisements come back to us in multicast mode: the kernel
        // loops group traffic back to the sending host, so a node hears itself
        // once a second. Treating that as a peer's advertisement makes a master
        // step down against itself, and since it is then a backup it times out,
        // takes over again, and flaps forever. A node ignores its own.
        if received.source == self.source {
            return Ok(None);
        }

        // The destination is needed for an IPv6 checksum, and it is known rather
        // than guessed: a multicast datagram arrived at the group this transport
        // joined, and a unicast one at the address this socket is bound to.
        let destination = match &self.peering {
            Peering::Unicast(_) => self.source,
            Peering::Multicast { group, .. } => *group,
        };
        let allowed = match &self.peering {
            Peering::Unicast(peers) => crate::vrrp::AllowedSources::Peers(peers.clone()),
            Peering::Multicast { group, .. } => {
                crate::vrrp::AllowedSources::Group { group: *group }
            }
        };
        let outcome = crate::vrrp::validate(
            Datagram {
                bytes: &received.payload,
                source: received.source,
                destination: received.destination,
                ttl: received.ttl,
            },
            &allowed,
            destination,
            vrid,
            self.policy,
            now,
            Some(rate),
        );
        Ok(Some(outcome))
    }

    /// Waits up to `timeout` for a datagram.
    ///
    /// # Errors
    ///
    /// Returns [`NetError::Io`] when the read itself fails.
    pub fn receive_until(
        &self,
        vrid: u8,
        timeout: Duration,
        rate: &mut RateWindow,
    ) -> Result<Option<Accepted>> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if let Some(outcome) = self.poll(vrid, remaining, rate)? {
                return Ok(Some(outcome));
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            // The read is non-blocking, so a short sleep between attempts is
            // what turns it into a bounded wait rather than a spin.
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}
