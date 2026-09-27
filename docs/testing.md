# Testing

Testing is a product feature, not a cleanup task. The tiers below mirror
`SPEC.md` §21.

## Running everything

```console
$ cargo fmt --all --check
$ cargo clippy --all-targets --all-features -- -D warnings
$ cargo test --workspace
$ cargo deny check
$ cargo audit
```

## Where tests live

| Kind | Location | Runs in `cargo test --workspace` |
|---|---|---|
| Unit tests | `#[cfg(test)] mod tests` in the module | yes |
| Integration tests | `crates/<crate>/tests/` | yes |
| Netlink tests | `crates/highland-net/tests/netlink.rs` | only with `--features netlink-tests` and `CAP_NET_ADMIN` |
| Network-namespace tests | `tests/network-ns/`, not written yet | not until Milestone 4 |
| Compatibility tests | `tests/compatibility/` | no, requires Keepalived |
| Fuzz targets | `fuzz/fuzz_targets/` | no, requires `cargo-fuzz` and nightly |

Integration tests belong to the crate whose public API they exercise, which is
why `crates/highland-config/tests/validation.rs` exists rather than a directory
at the workspace root. `tests/integration/` holds fixtures shared by more than
one crate.

## Conventions

- One behavior per test, named as a sentence:
  `a_single_failure_does_not_change_the_verdict`.
- A test that exists to enforce a specification rule names the rule, either in
  the test name (`v06_rejects_a_duplicate_interface_and_vrid_pair`) or in the
  body.
- Time is injected through `highland_core::clock::Clock`. `ManualClock` is the
  default in tests. A test that sleeps, reads the wall clock, or uses a real
  random source is a defect.
- No test requires network access.
- No test requires root, unless it is behind the `netns-tests` feature.
- Panic-free parsers are asserted, not assumed: the fuzz targets are the
  mechanism, and a panic in a parser fails the build.

## Requirement traceability

Specification requirements are testable obligations. The mapping today:

| Requirement group | Covered by |
|---|---|
| `V-01`–`V-32` | `crates/highland-config/tests/validation.rs`, plus the loader tests in `src/loader.rs` |
| `I-01` to `I-04`, `I-14` to `I-16`, `I-20`, `I-21`, `I-23` to `I-33`, `I-35` to `I-45` (state machine and timers) | `crates/highland-core/tests/state_machine.rs`, one test per invariant, and `tests/properties.rs` |
| `R-26`, `R-27` (determinism, absolute deadlines) | `crates/highland-core/tests/properties.rs` and `src/clock.rs` |
| `I-05`, `I-06` (decoder never panics, interval round trip) | `crates/highland-vrrp/tests/properties.rs` and the two VRRP fuzz targets |
| `I-09` (a rejected reload changes nothing) | Milestone 4; Milestone 1 tests the generation guard only |
| `I-19` (confirmation means read-back) | Milestone 1 tests that confirmation is the only path to ownership; `crates/highland-daemon/tests/failover.rs` tests the read-back through the executor |
| `I-04`, `I-14`, `I-45` (ownership precedes advertising) | `crates/highland-daemon/tests/failover.rs`, driven through a scripted kernel |
| `L-05` (bounded instruction queue) | `crates/highland-daemon/tests/run_loop.rs` |
| `I-39` (a malformed packet never terminates the daemon) | `fuzz_vrrp_ipv4_packet` and `fuzz_vrrp_ipv6_packet` |
| `I-44` (advertisement handling in `BACKUP`) | `crates/highland-core/tests/state_machine.rs`, including the discard rule |
| `I-12`, `I-26` (stale results) | `crates/highland-checks/src/result.rs` |
| `L-08` (bounded event history) | `crates/highland-observe/src/ring.rs` |
| `L-12`, `L-14` (rate limit, request size) | `crates/highland-control/src/rate.rs` and `src/message.rs` |
| `S-01` (redaction) | `crates/highland-observe/src/redact.rs` |
| `L-06`, `V-26` (file size and permissions) | `crates/highland-config/src/loader.rs` |

## Property tests

`crates/highland-core/tests/properties.rs` uses `proptest` to assert the
invariants over randomly generated event sequences, because a state machine
produces its bugs in unusual orderings rather than in the common one. The
alphabet covers every transition path: startup, interface events,
advertisements, health changes, every timer, both executor outcomes, and operator
actions.

Three properties earn their keep on their own:

- the invariants hold after **every prefix** of any event sequence,
- the same events produce the same actions, which is what `R-26` promises,
- no advertisement is ever emitted without confirmed ownership.

Run more cases than CI does before pushing:

```console
$ PROPTEST_CASES=4096 cargo test -p highland-core --test properties
```

A failing case is written to `tests/properties.proptest-regressions` and is
checked in, so a bug found once is tested forever.

## Testing above the kernel

Everything from the state machine upward is tested without privileges, because
`highland-net` ships a `ScriptedBackend` and the executor is generic over it. A
test scripts the kernel's answers and then asserts the whole sequence: the calls
the backend received, and the advertisements the transport was asked to send.

That is what makes the failure paths affordable to test. "The address is already
in use", "the interface disappeared", "the add was refused" are one line of
script rather than a namespace and a root shell, and they run in milliseconds.

What a scripted kernel cannot prove is that Linux behaves as the backend expects.
That is the namespace suite's job, and it is the reason both exist.

## Network-namespace tests

These arrive with Milestone 4. The harness will build:

```text
node-a namespace ─ veth ─ bridge ─ veth ─ node-b namespace
```

and will need root. They are gated so that an unprivileged `cargo test
--workspace` stays green.

## Fuzzing

Fuzz targets arrive with Milestone 2:

```text
fuzz_vrrp_ipv4_packet
fuzz_vrrp_ipv6_packet
fuzz_config_document
fuzz_check_response
fuzz_netlink_event
fuzz_control_request
```

Each target must have bounded input, no network access, a checked-in regression
corpus for every fixed crash, and a CI smoke job.

## Continuous integration

`.github/workflows/ci.yml` runs, on every push and pull request:

| Job | Command |
|---|---|
| format | `cargo fmt --all --check` |
| lint | `cargo clippy --all-targets --all-features -- -D warnings` |
| test | `cargo test --workspace` |
| msrv | build and test on the declared MSRV |
| licenses and advisories | `cargo deny check`, `cargo audit` |
| documentation | `cargo doc --workspace --no-deps` with warnings denied |
| fuzz | `cargo fuzz build <target>` then a 60-second smoke per target, artifacts uploaded on failure |
| netns | present but disabled (`if: false`) until Milestone 4 lands the harness |

Six fuzz targets run in CI: `fuzz_vrrp_ipv4_packet`, `fuzz_vrrp_ipv6_packet`,
`fuzz_config_document`, `fuzz_check_response`, `fuzz_netlink_event`,
`fuzz_control_request`.
