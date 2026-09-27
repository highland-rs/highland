# ADR-0002: Async runtime

- Status: proposed
- Date: 2026-09-27
- Blocks: `SPEC.md` `B-01`, Milestones 1 and 3

## Context

The daemon needs an async runtime for sockets, timers, and signal handling.
`SPEC.md` §20 requires a single runtime and requires `highland-core` and
`highland-vrrp` to be free of any runtime dependency, so that the state machine
and the codec stay testable with a fake clock and no executor.

The candidates are Tokio, smol, and other well-supported runtimes. The
evaluation criteria are timer precision, Linux socket support, cancellation
behavior, footprint, ecosystem compatibility, and test-time determinism.

## Decision

Not yet made. The examples in `SPEC.md` §21 use Tokio for readability, and the
Milestone 0 binaries use Tokio, but nothing in the library crates depends on it.

## Constraints on the eventual decision

- `highland-core` and `highland-vrrp` MUST NOT gain a runtime dependency. This
  is checked by reviewing `Cargo.toml`, not by a test.
- Exactly one runtime runs the daemon. A second runtime, even transitively, is
  a review finding.
- Timer semantics must support the exact-deadline assertions in
  `crates/highland-core/src/state.rs`.
- Cancellation must be structured, so that invariant `I-41` is enforceable.

## Consequences of choosing Tokio

Widest ecosystem and the best Linux socket coverage, at the cost of the largest
dependency tree. This is the option the current code implies; it is not yet a
decision.
