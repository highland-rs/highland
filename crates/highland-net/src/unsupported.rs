// Rust guideline compliant 2026-09-27

//! The backend for platforms with no Netlink.
//!
//! Highland is a Linux product, but the crate must still compile, and the
//! executor and its tests must still run, on a contributor's Mac. Rather than
//! pretending the operations succeed, every one of them returns
//! [`NetError::Unsupported`] with the operation named, so a missing platform is
//! an explicit error at the point of use rather than a mysterious failure later.

use std::net::IpAddr;

use crate::backend::NetworkBackend;
use crate::error::{NetError, Result};
use crate::types::{Interface, InterfaceId, IpCidr};

/// A backend that reports every operation as unsupported.
///
/// # Examples
///
/// ```
/// use highland_net::{NetError, NetworkBackend, UnsupportedBackend};
///
/// # async fn example() {
/// let backend = UnsupportedBackend;
/// let error = backend.interface("eth0").await.expect_err("there is no backend here");
/// assert!(matches!(error, NetError::Unsupported { .. }));
/// # }
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct UnsupportedBackend;

impl UnsupportedBackend {
    /// Creates the backend.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    fn unsupported<T>(operation: &'static str) -> Result<T> {
        Err(NetError::Unsupported { operation })
    }
}

/// Every method is `async` because the trait says so, and none of them awaits
/// anything: refusing immediately is the whole point.
macro_rules! unsupported {
    ($($name:ident($($argument:ident: $type:ty),*) -> $answer:ty),* $(,)?) => {
        $(
            fn $name(
                &self,
                $($argument: $type),*
            ) -> impl std::future::Future<Output = Result<$answer>> + Send {
                let _ = ($($argument,)*);
                let answer = Self::unsupported(stringify!($name));
                std::future::ready(answer)
            }
        )*
    };
}

impl NetworkBackend for UnsupportedBackend {
    unsupported! {
        interface(name: &str) -> Interface,
        add_address(interface: InterfaceId, address: IpCidr) -> (),
        remove_address(interface: InterfaceId, address: IpCidr) -> (),
        send_gratuitous_update(interface: InterfaceId, address: IpAddr) -> (),
    }
}
