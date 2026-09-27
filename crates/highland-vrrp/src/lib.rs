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

#[cfg(test)]
mod hygiene {
    //! Guards against the mistake that left a file behind.

    //! A file in `src/` that no module declares is never compiled, so it cannot
    //! fail a build, cannot fail a test, and cannot be caught by a reader who
    //! does not remember it. That is exactly how `advertisement.rs` survived the
    //! codec rewrite carrying the superseded 8-bit interval bound and a second,
    //! contradictory `Advertisement` type. The check is one line of filesystem
    //! work, and it makes the rot impossible rather than merely unlikely.

    use std::fs;
    use std::path::Path;

    #[test]
    fn every_source_file_is_a_declared_module() {
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let declared = fs::read_to_string(source.join("lib.rs")).expect("lib.rs is readable");

        let mut orphans = Vec::new();
        for entry in fs::read_dir(&source).expect("src/ is readable") {
            let path = entry.expect("the entry is readable").path();
            if path.extension().is_none_or(|extension| extension != "rs") {
                continue;
            }
            let name = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or_default()
                .to_owned();
            if name == "lib" {
                continue;
            }
            if !declared.contains(&format!("mod {name};")) {
                orphans.push(name);
            }
        }
        orphans.sort();

        assert!(
            orphans.is_empty(),
            "these files are in src/ but no module declares them, so nothing compiles or tests them: {orphans:?}"
        );
    }
}
