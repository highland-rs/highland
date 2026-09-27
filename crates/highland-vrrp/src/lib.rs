// Rust guideline compliant 2026-09-27

//! VRRP version 3 protocol types, encoding, decoding, and validation.
//!
//! This crate knows the wire format and nothing else. It MUST NOT know about
//! interfaces, peers, instance state, or any operating system: peer filtering
//! and instance context belong to the caller (SPEC.md, §9.2).
//!
//! The milestone delivered here is the typed protocol surface: version, VRID,
//! priority, address family, and the [`Advertisement`] aggregate, with
//! validation up front so that invalid values cannot be represented. Encoding,
//! decoding, and the fuzz targets arrive in Milestone 2.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

mod advertisement;
mod error;
mod types;

pub use advertisement::{
    Advertisement, AdvertisementError, MAX_ADVERT_ADDRESSES, MAX_ADVERT_INTERVAL,
    MIN_ADVERT_INTERVAL,
};
pub use error::{DecodeError, EncodeError, ProtocolError, Result};
pub use types::{IpFamily, Priority, Version, Vrid};
