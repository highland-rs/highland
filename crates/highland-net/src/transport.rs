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

use crate::error::{NetError, Result};
use crate::vrrp::{Accepted, Datagram, Destinations, PeerSet, RateWindow, VRRP_IP_PROTOCOL};

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
    peers: PeerSet,
    destinations: Destinations,
    source: IpAddr,
}

impl SocketTransport {
    /// Binds a transport for one instance.
    ///
    /// # Errors
    ///
    /// Returns [`NetError::Io`] when the socket cannot be created or bound,
    /// which needs `CAP_NET_RAW`.
    pub fn bind(family: IpFamily, interface: &str, source: IpAddr, peers: PeerSet) -> Result<Self> {
        let socket = crate::VrrpSocket::bind(family, interface, source)?;
        let destinations = Destinations::new(peers.of_family(family));
        Ok(Self {
            socket,
            family,
            peers,
            destinations,
            source,
        })
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
        let bytes = crate::vrrp::build(
            advertisement,
            self.family,
            self.source,
            self.family.default_group(),
        )?;

        let mut delivered = 0_usize;
        let mut first_failure: Option<NetError> = None;
        for peer in self.destinations.peers() {
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
        let outcome = crate::vrrp::validate(
            Datagram {
                bytes: &received.payload,
                source: received.source,
                ttl: received.ttl,
            },
            &self.peers,
            vrid,
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
