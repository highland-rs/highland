// Rust guideline compliant 2026-09-27

//! Fuzzes control-request and response decoding.
//!
//! The control socket is local, but a request still crosses a process boundary
//! and can come from any local user. The contract is that a request is either
//! rejected with a typed error or accepted as a member of a closed operation
//! set, that an oversized request is refused rather than parsed, and that every
//! destructive operation remains identifiable as one (`R-28`, `L-14`).

#![no_main]

use highland_control::{ControlRequest, ControlResponse, MAX_REQUEST_BYTES};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };

    match ControlRequest::decode(text) {
        Ok(request) => {
            // An accepted request re-encodes, so the daemon can log exactly
            // what it received.
            let encoded = request.encode().expect("an accepted request re-encodes");
            assert_eq!(
                ControlRequest::decode(&encoded).expect("round trip"),
                request,
                "decoding is a fixed point"
            );
            assert!(encoded.len() <= MAX_REQUEST_BYTES);
            if request.is_destructive() {
                assert!(
                    matches!(
                        request,
                        ControlRequest::Reload
                            | ControlRequest::Pause { .. }
                            | ControlRequest::Resume { .. }
                            | ControlRequest::Relinquish { .. }
                            | ControlRequest::ForceTransition { .. }
                    ),
                    "the destructive set is closed"
                );
            }
        }
        Err(error) => {
            assert!(!error.to_string().is_empty());
        }
    }

    // The client parses responses from the same untrusted source.
    let _ = ControlResponse::decode(text);
});
