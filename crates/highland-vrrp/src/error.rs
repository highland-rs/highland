// Rust guideline compliant 2026-09-27

//! Protocol error types.
//!
//! Decoding untrusted input MUST NOT panic; every rejection is one of these
//! variants (SPEC.md, `I-05`, `S-04`).

use thiserror::Error;

use crate::types::IpFamily;

/// Result alias for fallible protocol operations.
pub type Result<T, E = ProtocolError> = std::result::Result<T, E>;

/// An error raised by the protocol layer.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum ProtocolError {
    /// The version field carried an unknown value.
    #[error("unsupported VRRP version {version}")]
    UnsupportedVersion {
        /// The raw version field.
        version: u8,
    },

    /// The packet type is not one this version defines.
    #[error("unexpected VRRP packet type {packet_type}")]
    UnexpectedPacketType {
        /// The raw type field.
        packet_type: u8,
    },

    /// A VRID of `0` was encountered.
    #[error("VRID {vrid} is outside {min}..={max}")]
    VridOutOfRange {
        /// The rejected VRID.
        vrid: u8,
        /// The lowest valid VRID.
        min: u8,
        /// The highest valid VRID.
        max: u8,
    },

    /// The advertisement interval was outside the 12-bit field.
    #[error("max adver int {centiseconds}cs is outside {min}..={max}cs")]
    AdverIntOutOfRange {
        /// The rejected value in centiseconds.
        centiseconds: u16,
        /// The lowest accepted value.
        min: u16,
        /// The highest accepted value.
        max: u16,
    },

    /// The address count was outside the range the count field allows.
    #[error("address count {count} is outside {min}..={max}")]
    AddressCountOutOfRange {
        /// The rejected count.
        count: usize,
        /// The lowest accepted count.
        min: u16,
        /// The highest accepted count.
        max: u16,
    },

    /// An address belonged to a different family than the call requested.
    #[error("address {address} is not an {expected} address")]
    AddressFamilyMismatch {
        /// The address that was rejected.
        address: String,
        /// The family the caller requested.
        expected: IpFamily,
    },
}

/// An error raised while encoding an advertisement.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum EncodeError {
    /// A field value was not representable on the wire.
    #[error("field {field} cannot be encoded: {reason}")]
    InvalidField {
        /// The name of the offending field.
        field: &'static str,
        /// Why it cannot be encoded.
        reason: &'static str,
    },

    /// The addresses did not belong to the requested family.
    #[error("cannot encode an advertisement for {expected}: the addresses are {found}")]
    FamilyMismatch {
        /// The family the caller requested.
        expected: IpFamily,
        /// The family the addresses belong to.
        found: IpFamily,
    },

    /// The advertisement carried more addresses than the count field can hold.
    #[error("an advertisement carries at most 255 addresses, not {count}")]
    TooManyAddresses {
        /// The number of addresses supplied.
        count: usize,
    },

    /// The checksum could not be computed because the packet's addresses are
    /// unknown. An IPv6 VRRP checksum covers them, so producing a value without
    /// them would produce one that does not interoperate.
    #[error("the {family} checksum needs the packet's source and destination addresses")]
    UndecidableChecksum {
        /// The family whose checksum is undecidable.
        family: IpFamily,
    },
}

/// An error raised while decoding an advertisement.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum DecodeError {
    /// The buffer ended before a required field was read.
    #[error("packet truncated: needed {needed} bytes at offset {offset}, {available} available")]
    Truncated {
        /// Bytes required at `offset`.
        needed: usize,
        /// Bytes actually available.
        available: usize,
        /// Where the read started.
        offset: usize,
    },

    /// A field carried an invalid value.
    #[error("field {field} is invalid: {reason}")]
    InvalidField {
        /// The name of the offending field.
        field: &'static str,
        /// Why the value was rejected.
        reason: String,
    },

    /// The count field disagreed with the message length.
    #[error(
        "count {count} implies a {declared}-byte {family} message, but the buffer is {actual} bytes"
    )]
    InconsistentLength {
        /// The length the count implies.
        declared: usize,
        /// The length of the buffer.
        actual: usize,
        /// The count found in the header.
        count: u8,
        /// The family the message was decoded as.
        family: IpFamily,
    },

    /// The packet was a different protocol type.
    #[error("unexpected VRRP packet type {packet_type}")]
    UnexpectedPacketType {
        /// The type field found in the packet.
        packet_type: u8,
    },

    /// The checksum did not match.
    #[error("{family} checksum mismatch")]
    ChecksumMismatch {
        /// The family the packet was decoded as.
        family: IpFamily,
    },
}

/// The error returned when a checksum cannot be computed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum ChecksumScopeError {
    /// The scope needs the packet's addresses, which were not supplied.
    #[error("the checksum scope needs the packet's addresses")]
    Undecidable,
}
