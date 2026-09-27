// Rust guideline compliant 2026-09-27

//! The [`NetworkBackend`] contract.
//!
//! The methods are `async` because the Linux implementation is: Netlink is an
//! asynchronous socket, and blocking a runtime thread on it would break the
//! rule that the state machine's task never blocks (`I-38`). A synchronous trait
//! would have forced the real backend to hand-roll a blocking wrapper, which is
//! the mistake this shape avoids.
//!
//! Making the methods `async` costs nothing for a fake: a scripted backend
//! answers immediately, and a test needs no runtime to drive one.

use std::future::Future;
use std::net::IpAddr;

use crate::error::Result;
use crate::types::{Interface, InterfaceId, IpCidr};

/// The Linux operations the state machine's executor needs.
///
/// Implementations confirm the effect of a mutation by reading it back; a
/// successful return means the kernel state changed, not merely that a request
/// was accepted (SPEC.md, `I-19`).
///
/// # Examples
///
/// ```
/// use highland_net::{Interface, InterfaceId, IpCidr, NetError, NetworkBackend};
/// use std::net::IpAddr;
///
/// #[derive(Debug)]
/// struct FakeBackend;
///
/// impl NetworkBackend for FakeBackend {
///     async fn interface(&self, name: &str) -> Result<Interface, NetError> {
///         Err(NetError::InterfaceNotFound { name: name.to_owned() })
///     }
///
///     async fn add_address(&self, _interface: InterfaceId, _address: IpCidr) -> Result<(), NetError> {
///         Ok(())
///     }
///
///     async fn remove_address(
///         &self,
///         _interface: InterfaceId,
///         _address: IpCidr,
///     ) -> Result<(), NetError> {
///         Ok(())
///     }
///
///     async fn send_gratuitous_update(
///         &self,
///         _interface: InterfaceId,
///         _address: IpAddr,
///     ) -> Result<(), NetError> {
///         Ok(())
///     }
/// }
///
/// # let _ = |backend: FakeBackend| async move { backend.interface("eth0").await };
/// ```
pub trait NetworkBackend: Send + Sync + std::fmt::Debug {
    /// Looks an interface up by name, including its addresses.
    ///
    /// # Errors
    ///
    /// Returns [`NetError::InterfaceNotFound`](crate::NetError::InterfaceNotFound)
    /// when no such interface exists.
    fn interface(&self, name: &str) -> impl Future<Output = Result<Interface>> + Send;

    /// Adds an address and confirms it is present.
    ///
    /// # Errors
    ///
    /// Returns [`NetError::AddAddress`](crate::NetError::AddAddress) on failure,
    /// including when the address already exists on another interface.
    fn add_address(
        &self,
        interface: InterfaceId,
        address: IpCidr,
    ) -> impl Future<Output = Result<()>> + Send;

    /// Removes an address and confirms it is absent.
    ///
    /// # Errors
    ///
    /// Returns [`NetError::RemoveAddress`](crate::NetError::RemoveAddress) on
    /// failure.
    fn remove_address(
        &self,
        interface: InterfaceId,
        address: IpCidr,
    ) -> impl Future<Output = Result<()>> + Send;

    /// Emits a gratuitous ARP for IPv4 or an unsolicited Neighbor Advertisement
    /// for IPv6.
    ///
    /// # Errors
    ///
    /// Returns [`NetError::SendGratuitousUpdate`](crate::NetError::SendGratuitousUpdate) on
    /// failure. Failure is logged and counted; it never invalidates ownership
    /// (`R-11`).
    fn send_gratuitous_update(
        &self,
        interface: InterfaceId,
        address: IpAddr,
    ) -> impl Future<Output = Result<()>> + Send;
}
