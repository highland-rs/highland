# Shared integration fixtures

Rust integration tests live in `crates/<crate>/tests/`, next to the crate whose
public API they exercise. This directory holds fixtures shared by more than one
crate: packet captures, recorded netlink transcripts, and topologies used by the
namespace harness.

Nothing here is compiled by `cargo test --workspace`. Files here are inputs to
tests, and each one names the tests that consume it in
`docs/testing.md`.
