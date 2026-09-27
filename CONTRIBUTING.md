# Contributing to Highland

Highland is a Rust project that follows the
[Microsoft Pragmatic Rust Guidelines](https://microsoft.github.io/rust-guidelines/).
The short version: memory safety first, no panics for control flow, public items
documented, and every new behavior traceable to the specification.

## The specification is the contract

[`docs/SPEC.md`](docs/SPEC.md) is normative. Every requirement carries an
identifier:

| Prefix | Meaning |
|---|---|
| `G-nn` | Goal |
| `R-nn` | Requirement |
| `I-nn` | Invariant |
| `V-nn` | Configuration validation rule |
| `L-nn` | Hard resource limit |
| `S-nn` | Security requirement |
| `M-nn` | Milestone exit criterion |

A pull request MUST either reference the identifiers it implements or state that
it is infrastructure. Adding behavior without an identifier means the
specification needs a change in the same pull request; identifiers are never
renumbered or reused.

## Toolchain and MSRV

- Build toolchain: stable, pinned by `rust-toolchain.toml`.
- Minimum supported Rust version: **1.85**, declared once as
  `workspace.package.rust-version`.

Policy for the MSRV:

1. The MSRV is raised at most once per calendar year.
2. A raise is a minor version bump, announced in `CHANGELOG.md`, with the
   migration note required by `SPEC.md` §26.
3. Between raises, CI verifies that the workspace builds and tests on the MSRV.
4. A dependency that requires a higher MSRV is a blocker, not a warning. Raise
   the MSRV deliberately or choose a different dependency.

## Build and test

Run these before opening a pull request. CI runs the same commands.

```console
$ cargo fmt --all --check
$ cargo clippy --all-targets --all-features -- -D warnings
$ cargo test --workspace
$ cargo deny check
$ cargo audit
```

Additional suites, described in [`docs/testing.md`](docs/testing.md):

```console
$ cargo test --workspace --features command-checks   # opt-in feature paths
$ cargo test -p highland-core --release              # timing-sensitive tests
```

## Lint policy

Workspace lints live in `Cargo.toml` under `[workspace.lints]` and every crate
opts in with `[lints] workspace = true`. Do not silence a lint in a crate
manifest. If a lint is wrong for the project, allow it in the workspace table
with a comment explaining why, so the decision is reviewed once.

`unsafe` code is forbidden workspace-wide (`unsafe_code = "forbid"`). If a
future milestone genuinely needs it, that is an ADR and a specification change,
not a local `#[allow]`.

## Test conventions

- One test per behavior, named as a sentence: `a_single_failure_does_not_change_the_verdict`.
- Name the specification identifier in the test name or the test body when the
  test exists to enforce one, for example `v06_rejects_a_duplicate_interface_and_vrid_pair`.
- Unit tests live in the module they test, under `#[cfg(test)]`.
- Integration tests live in `crates/<crate>/tests/`.
- Tests that need root or network namespaces are gated behind the `netns-tests`
  feature so that `cargo test --workspace` succeeds unprivileged (`R-01`).
- Time is injected through `highland_core::clock::Clock`. A test that reads the
  wall clock, sleeps, or uses a real random source is a defect.
- No test may require network access.

## Error conventions

- Libraries use `thiserror`. Binaries may use `anyhow` for context.
- Error variants carry structured fields, not pre-formatted strings:
  `NetError::AddAddress { interface, address, source }`.
- Every error surfaced to a user names the instance, the interface, the address
  or peer, and the underlying cause (`R-23`).
- A swallowed error MUST be counted, not ignored (`R-24`).

## Documentation

A change that adds a configuration key, a metric, a control operation, or a
transition reason MUST update the corresponding document in the same commit
(`R-33`). Public items MUST have doc comments with `# Errors` and `# Panics`
sections where they apply.

## Commit messages

One logical change per commit. The subject line is imperative and under 72
characters. The body explains why, not what.

## Pull requests

- Reference the specification identifiers and the milestone.
- Say what you tested and how.
- Note anything you deliberately left out.

## Security

Do not open a public issue for a security problem. See
[`SECURITY.md`](SECURITY.md).
