// Rust guideline compliant 2026-09-27

//! The VRRP advertisement aggregate.

use std::net::IpAddr;
use std::time::Duration;

use thiserror::Error;

use crate::error::{EncodeError, ProtocolError};
use crate::types::{IpFamily, Priority, Version, Vrid};

/// The lowest advertisement interval, `10ms`, one centisecond.
pub const MIN_ADVERT_INTERVAL: Duration = Duration::from_millis(10);
/// The highest advertisement interval, `2550ms`, 255 centiseconds.
pub const MAX_ADVERT_INTERVAL: Duration = Duration::from_millis(2550);
/// The largest number of addresses one advertisement may carry (`L-01`).
pub const MAX_ADVERT_ADDRESSES: usize = 255;

/// A VRRP advertisement.
///
/// All fields are validated on construction, so an `Advertisement` that exists
/// is an advertisement that can be encoded.
///
/// # Examples
///
/// ```
/// use highland_vrrp::{Advertisement, IpFamily, Priority, Version, Vrid};
/// use std::time::Duration;
///
/// let advertisement = Advertisement::new(
///     Vrid::new(42).unwrap(),
///     Priority::new(150).unwrap(),
///     Duration::from_secs(1),
///     vec!["192.0.2.10".parse().unwrap()],
/// )
/// .unwrap();
///
/// assert_eq!(advertisement.family(), IpFamily::V4);
/// assert_eq!(advertisement.version(), Version::V3);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Advertisement {
    version: Version,
    vrid: Vrid,
    priority: Priority,
    advert_interval: Duration,
    addresses: Vec<IpAddr>,
}

impl Advertisement {
    /// Creates an advertisement.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::AddressFamilyMismatch`] when the addresses are
    /// empty or belong to more than one family, and
    /// [`EncodeError::InvalidField`] when the interval is outside
    /// [`MIN_ADVERT_INTERVAL`]..=[`MAX_ADVERT_INTERVAL`] or the address count
    /// exceeds [`MAX_ADVERT_ADDRESSES`].
    pub fn new(
        vrid: Vrid,
        priority: Priority,
        advert_interval: Duration,
        addresses: Vec<IpAddr>,
    ) -> std::result::Result<Self, AdvertisementError> {
        let Some(first) = addresses.first() else {
            return Err(AdvertisementError::NoAddresses);
        };
        let family = IpFamily::of(first);
        if let Some(offending) = addresses.iter().find(|address| !family.matches(address)) {
            return Err(ProtocolError::AddressFamilyMismatch {
                address: offending.to_string(),
                expected: family,
            }
            .into());
        }
        if addresses.len() > MAX_ADVERT_ADDRESSES {
            return Err(EncodeError::InvalidField {
                field: "address count",
                reason: "an advertisement carries at most 255 addresses",
            }
            .into());
        }
        if !(MIN_ADVERT_INTERVAL..=MAX_ADVERT_INTERVAL).contains(&advert_interval) {
            return Err(EncodeError::InvalidField {
                field: "advert_interval",
                reason: "the interval must be 10ms..=2550ms",
            }
            .into());
        }

        Ok(Self {
            version: Version::V3,
            vrid,
            priority,
            advert_interval,
            addresses,
        })
    }

    /// Returns the protocol version.
    #[must_use]
    pub fn version(&self) -> Version {
        self.version
    }

    /// Returns the VRID.
    #[must_use]
    pub fn vrid(&self) -> Vrid {
        self.vrid
    }

    /// Returns the announced priority.
    #[must_use]
    pub fn priority(&self) -> Priority {
        self.priority
    }

    /// Returns the announced advertisement interval.
    #[must_use]
    pub fn advert_interval(&self) -> Duration {
        self.advert_interval
    }

    /// Returns the advertised addresses.
    #[must_use]
    pub fn addresses(&self) -> &[IpAddr] {
        &self.addresses
    }

    /// Returns the family this advertisement belongs to.
    #[must_use]
    pub fn family(&self) -> IpFamily {
        self.addresses.first().map_or(IpFamily::V4, IpFamily::of)
    }
}

/// An error raised while constructing an [`Advertisement`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum AdvertisementError {
    /// The advertisement carried no addresses.
    #[error("an advertisement must carry at least one address")]
    NoAddresses,

    /// A field could not be represented on the wire.
    #[error(transparent)]
    Encode(#[from] EncodeError),

    /// A field was not a valid protocol value.
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn address(text: &str) -> IpAddr {
        text.parse().expect("literal is a valid address")
    }

    fn advertisement() -> Advertisement {
        Advertisement::new(
            Vrid::new(42).expect("42 is a valid VRID"),
            Priority::new(150).expect("150 is a valid priority"),
            Duration::from_secs(1),
            vec![address("192.0.2.10")],
        )
        .expect("the fixture is valid")
    }

    #[test]
    fn a_valid_advertisement_exposes_its_fields() {
        let advertisement = advertisement();
        assert_eq!(advertisement.vrid().get(), 42);
        assert_eq!(advertisement.priority().get(), 150);
        assert_eq!(advertisement.advert_interval(), Duration::from_secs(1));
        assert_eq!(advertisement.family(), IpFamily::V4);
    }

    #[test]
    fn an_advertisement_without_addresses_is_rejected() {
        let error = Advertisement::new(
            Vrid::new(42).expect("42 is a valid VRID"),
            Priority::new(150).expect("150 is a valid priority"),
            Duration::from_secs(1),
            Vec::new(),
        );
        assert_eq!(error, Err(AdvertisementError::NoAddresses));
    }

    #[test]
    fn mixed_family_addresses_are_rejected() {
        let error = Advertisement::new(
            Vrid::new(42).expect("42 is a valid VRID"),
            Priority::new(150).expect("150 is a valid priority"),
            Duration::from_secs(1),
            vec![address("192.0.2.10"), address("2001:db8::10")],
        );
        assert!(matches!(
            error,
            Err(AdvertisementError::Protocol(
                ProtocolError::AddressFamilyMismatch { .. }
            ))
        ));
    }

    #[test]
    fn an_interval_outside_the_protocol_range_is_rejected() {
        for interval in [Duration::from_millis(9), Duration::from_millis(2551)] {
            let error = Advertisement::new(
                Vrid::new(42).expect("42 is a valid VRID"),
                Priority::new(150).expect("150 is a valid priority"),
                interval,
                vec![address("192.0.2.10")],
            );
            assert!(matches!(
                error,
                Err(AdvertisementError::Encode(EncodeError::InvalidField { .. }))
            ));
        }
    }

    #[test]
    fn too_many_addresses_are_rejected() {
        let addresses: Vec<IpAddr> = (0..=MAX_ADVERT_ADDRESSES)
            .map(|last| address(&format!("192.0.2.{last}")))
            .collect();
        let error = Advertisement::new(
            Vrid::new(42).expect("42 is a valid VRID"),
            Priority::new(150).expect("150 is a valid priority"),
            Duration::from_secs(1),
            addresses,
        );
        assert!(matches!(
            error,
            Err(AdvertisementError::Encode(EncodeError::InvalidField { .. }))
        ));
    }
}
