// Rust guideline compliant 2026-09-27

//! Protocol error types.
//!
//! Decoding untrusted input MUST NOT panic; every rejection is one of these
//! variants (SPEC.md, `I-05`).

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

    /// A priority outside the representable range was encountered.
    #[error("priority {priority} is outside {min}..={max}")]
    PriorityOutOfRange {
        /// The rejected priority.
        priority: u16,
        /// The lowest representable priority.
        min: u16,
        /// The highest representable priority.
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

    /// The addresses did not all belong to the requested family.
    #[error("cannot encode an advertisement for {expected} from the given addresses")]
    FamilyMismatch {
        /// The family the caller requested.
        expected: IpFamily,
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

    /// The packet declared a length inconsistent with its contents.
    #[error("declared length {declared} is inconsistent with the {family} address count {count}")]
    InconsistentLength {
        /// The length declared by the packet.
        declared: usize,
        /// The address count declared by the packet.
        count: u8,
        /// The family the packet was decoded as.
        family: IpFamily,
    },

    /// The packet was a different protocol type.
    #[error("unexpected packet type {packet_type}")]
    UnexpectedPacketType {
        /// The IP protocol number found in the packet.
        packet_type: u8,
    },
}
