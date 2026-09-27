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

### Known limitations

- No VRRP traffic is sent or received. VIP ownership moves only once Milestone 3
  lands.
- `highland run` starts the daemon skeleton and stops; there is no control
  socket listener yet.
- Several errors in `SPEC.md` §18 are aggregated per crate rather than in one
  `HighlandError`, because the CLI and the daemon do not share a dependency set.
  See `docs/architecture.md`.

## [0.0.0]

- Initial repository.
