// Rust guideline compliant 2026-09-27

//! VRRP version 3 protocol types, encoding, decoding, and validation.
//!
//! The wire format is RFC 5798, and the specification text is the authority for
//! every field in this crate. Where the RFC and this crate disagree, the RFC is
//! right and the crate is a bug.
//!
//! This crate knows the wire format and nothing else. It MUST NOT know about
//! interfaces, peers, or instance state: peer filtering belongs to the caller
//! (SPEC.md, §9.2).
//!
//! # Shape
//!
//! | Concern | Where it lives |
//! |---|---|
//! | Field values, validated on construction | [`Version`], [`Vrid`], [`Priority`], [`MaxAdverInt`], [`IpFamily`] |
//! | The RFC 1071 checksum and the RFC 2460 pseudo-header | [`Checksum`], [`ChecksumScope`] |
//! | Encoding, decoding, and the two-phase header read | [`Advertisement`], [`Peek`] |
//!
//! # Example
//!
//! ```
//! use highland_vrrp::{Advertisement, IpFamily, MaxAdverInt, Priority, Vrid};
//! use std::time::Duration;
//!
//! let advertisement = Advertisement::new(
//!     Vrid::new(42)?,
//!     Priority::new(150)?,
//!     MaxAdverInt::from_duration(Duration::from_secs(1))?,
//!     vec!["192.0.2.10".parse()?],
//! )?;
//!
//! let bytes = advertisement.encode_v4()?;
//! assert_eq!(bytes.len(), 12);
//! assert_eq!(Advertisement::decode_verified(&bytes, IpFamily::V4)?.vrid().get(), 42);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

#![deny(missing_docs)]
#![forbid(unsafe_code)]

mod checksum;
mod error;
mod message;
mod types;

pub use checksum::{CHECKSUM_UNCOMPUTED, Checksum, ChecksumScope, checksum, verify};
pub use error::{ChecksumScopeError, DecodeError, EncodeError, ProtocolError, Result};
pub use message::{Advertisement, AdvertisementError, CHECKSUM_OFFSET, MAX_ADDRESSES, Peek};
pub use types::{
    HEADER_LEN, IpFamily, MAX_MAX_ADVER_INT, MIN_MAX_ADVER_INT, MaxAdverInt, PacketType, Priority,
    REQUIRED_TTL, TYPE_ADVERTISEMENT, VERSION_3, VRRP_PROTOCOL, Version, Vrid,
};
