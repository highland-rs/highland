# Changelog

All notable changes to Highland are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- `docs/SPEC.md`: the normative specification, with scope tiers
  (`[I]` initial release, `[1]` version 1.0, `[F]` post-1.0), RFC 2119 language,
  and stable requirement identifiers (`G`, `R`, `I`, `V`, `L`, `S`, `M`, `D`).
- A nine-crate workspace matching the dependency rules in `SPEC.md` §9.
- `highland-core`: `Role`, `Event`, `Action`, `TimerId`, `Generation`,
  `TransitionReason`, `InstanceStateMachine`, and the `Clock` and `Rng`
  abstractions, with a deterministic `ManualClock`.
- `highland-vrrp`: `Version`, `Vrid`, `Priority`, `IpFamily`, and `Advertisement`,
  validated on construction so an invalid wire value cannot be represented.
- `highland-net`: the `NetworkBackend` trait, typed interface and CIDR types, and
  `NetError` with structured context.
- `highland-checks`: `CheckSpec`, `CheckResult` with sequence numbers, the
  `Stability` debouncer, and the `Check` trait. The `command-checks` feature
  exists and is off by default.
- `highland-config`: the typed configuration model, a strict TOML parser with
  explicit-unit durations, and the `V-01` through `V-32` validation rules, each
  with its own test.
- `highland-observe`: the event model, the redaction layer, and a bounded event
  ring.
- `highland-control`: the control request and response messages, the error
  taxonomy, and a token-bucket rate limiter.
- `highland-daemon` and `highland-cli` binaries, with a working
  `check-config` command and a `run` command that delegates to the daemon binary.
- Continuous integration covering formatting, lints, tests, dependency policy,
  advisories, and the MSRV.
- Project documentation skeleton: architecture, configuration, operations,
  threat model, compatibility, testing, and three ADRs.

### Changed

- `highland-core` is now the real state machine rather than a skeleton. It adds
  `timer` (a `TimerSet` and a `RetryPolicy`), `health` (the policy arithmetic of
  `SPEC.md` §12.2), `election` (the four-step tie-break of §12.3), and `machine`
  (the full `InstanceStateMachine`).
- Ownership is now a two-phase handshake: the machine requests addresses and
  enters `MASTER` only on `Event::ActionSucceeded`. There is no path from
  `BACKUP` to an advertisement without confirmed ownership, which makes `I-04`
  structural rather than a matter of action ordering.
- A failed address removal keeps the instance `MASTER` with advertising stopped,
  because the addresses are still present. The role changes once removal is
  confirmed.
- A shutdown or a pause cancels an in-flight ownership request and owes a
  best-effort removal, so a late confirmation cannot revive an instance.

### Added in this milestone

- `Event::ActionSucceeded` and `Event::InterfaceBroughtUp`, and the transition
  reason `preemption_delay_elapsed`.
- 58 state-machine tests, one per invariant, and 14 property tests that assert
  the invariants across randomly generated event sequences.
- `proptest` as a workspace dev-dependency.
- The invariants below are now enforced by tests: `I-01` to `I-04`, `I-09` (the
  generation guard), `I-10` to `I-15`, `I-19` (the state machine's share),
  `I-20`, `I-21`, `I-23` to `I-33`, `I-35` to `I-45`.

### Known limitations

- No VRRP traffic is sent or received. VIP ownership moves only once Milestone 3
  lands.
- `highland run` starts the daemon skeleton and stops; there is no control
  socket listener yet.
- Several errors in `SPEC.md` §18 are aggregated per crate rather than in one
  `HighlandError`, because the CLI and the daemon do not share a dependency set.
  See `docs/architecture.md`.
- No executor applies the machine's actions yet. `highland-net` has the trait but
  no Linux implementation, so nothing adds or removes an address. The socket
  listener arrives with Milestone 7.
- `I-09` and `I-39` are not yet covered: they belong to the reload planner
  (Milestone 4) and the decoder (Milestone 2) respectively. `SPEC.md` §27 says
  which part of each is tested here.

## [0.0.0]

- Initial repository.
