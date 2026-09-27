// Rust guideline compliant 2026-09-27

//! The network backend trait.

use std::net::IpAddr;

use crate::error::Result;
use crate::types::{Interface, InterfaceId, IpCidr};

/// The Linux operations the state machine's executor needs.
///
/// Implementations confirm the effect of a mutation by reading it back; a
/// successful return means the kernel state changed, not merely that a request
/// was accepted (SPEC.md, `I-19`).
///
/// The trait is synchronous. The production backend runs it on a blocking
/// thread or a dedicated runtime task so that the state machine task is never
/// blocked (`I-38`).
///
/// # Examples
///
/// ```
/// use highland_net::{Interface, InterfaceId, IpCidr, NetError, NetworkBackend, Result};
/// use std::net::IpAddr;
///
/// #[derive(Debug)]
/// struct FakeBackend {
///     interfaces: Vec<Interface>,
/// }
///
/// impl NetworkBackend for FakeBackend {
///     fn interface(&self, name: &str) -> Result<Interface> {
///         self.interfaces
///             .iter()
///             .find(|interface| interface.name == name)
///             .cloned()
///             .ok_or_else(|| NetError::InterfaceNotFound { name: name.to_owned() })
///     }
///
///     fn add_address(&self, _interface: InterfaceId, _address: IpCidr) -> Result<()> {
///         Ok(())
///     }
///
///     fn remove_address(&self, _interface: InterfaceId, _address: IpCidr) -> Result<()> {
///         Ok(())
///     }
///
///     fn send_gratuitous_update(
///         &self,
///         _interface: InterfaceId,
///         _address: IpAddr,
///     ) -> Result<()> {
///         Ok(())
///     }
/// }
/// ```
pub trait NetworkBackend: Send + Sync + std::fmt::Debug {
    /// Looks up an interface by name.
    ///
    /// # Errors
    ///
    /// Returns [`crate::NetError::InterfaceNotFound`] when no such interface exists.
    fn interface(&self, name: &str) -> Result<Interface>;

    /// Adds an address and confirms it is present.
    ///
    /// # Errors
    ///
    /// Returns [`crate::NetError::AddAddress`] on failure, including when the
    /// address already exists on another interface.
    fn add_address(&self, interface: InterfaceId, address: IpCidr) -> Result<()>;

    /// Removes an address and confirms it is absent.
    ///
    /// # Errors
    ///
    /// Returns [`crate::NetError::RemoveAddress`] on failure.
    fn remove_address(&self, interface: InterfaceId, address: IpCidr) -> Result<()>;

    /// Emits a gratuitous ARP for IPv4 or an unsolicited Neighbor Advertisement
    /// for IPv6.
    ///
    /// # Errors
    ///
    /// Returns [`crate::NetError::SendGratuitousUpdate`] on failure. Failure is
    /// logged and counted; it never invalidates ownership (`R-11`).
    fn send_gratuitous_update(&self, interface: InterfaceId, address: IpAddr) -> Result<()>;
}
